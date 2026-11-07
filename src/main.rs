//! «Эхо мицелия»: окно, камера от первого лица, ландшафт маршрута (этап 02),
//! compute → raster, HDR и тонмаппинг, перезагрузка MSL по F5.
//!
//! Управление: клик — захват мыши, WASD — движение, Shift — бег/быстрее,
//! Space — прыжок (в полёте вверх), G — ходьба/полёт, E/Q — вверх/вниз в полёте,
//! B — покачивание головы, T — ландшафт/тестовые кубы,
//! Esc — отпустить мышь, ⌘M — свернуть, F5 — перезагрузить MSL,
//! F9 — readback и сверка с эталоном, F12 — GPU capture одного кадра.
//!
//! Режимы: `--selftest` — автоматическая проверка с кодом выхода,
//! `--overview` — старт в полёте над маршрутом,
//! `--world DIR` — собрать мир из слоёв PNG в каталоге (`world.json`),
//! `--export-world DIR` — записать слои сгенерированного мира и выйти,
//! `--capture-frame N` — записать кадр N в `captures/`.

mod frame_graph;
mod layout;
mod platform;
mod reference;
mod renderer;
mod scene;
mod sim;
mod terrain_mesh;
mod world;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Quat, Vec3};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

use layout::{FrameUniforms, InstanceData};
use renderer::{FrameInput, FrameOutcome, Readback, Renderer};
use sim::{CameraState, FixedClock, MoveMode, Sim, TickInput};
use world::{Ground, MarkerKind, World};

const LIGHT_DIR: Vec3 = Vec3::new(-0.45, -0.8, -0.4);
/// Seed первой карты и её размер в тайлах (768 × 768 м).
const WORLD_SEED: u64 = 0xEC40;
const WORLD_TILES: i32 = 12;
const DETAIL_TEXTURE_SIZE: u32 = 512;
const SKY_COLOR: [f32; 3] = [0.30, 0.42, 0.62];
const TEST_SCENE_CLEAR: [f32; 3] = [0.035, 0.045, 0.06];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SceneMode {
    /// Тестовая сцена этапа 01: два куба.
    Cubes,
    /// Ландшафт маршрута и маркеры.
    Terrain,
}

#[derive(Default)]
struct Args {
    selftest: bool,
    /// Старт в полёте с контрольной камеры ландшафта.
    overview: bool,
    capture_frame: Option<u64>,
    /// Каталог слоёв карты вместо генератора.
    world: Option<std::path::PathBuf>,
    /// Записать слои мира в каталог и выйти.
    export_world: Option<std::path::PathBuf>,
}

impl Args {
    fn parse() -> Args {
        let mut args = Args::default();
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--selftest" => args.selftest = true,
                "--overview" => args.overview = true,
                "--capture-frame" => args.capture_frame = it.next().and_then(|v| v.parse().ok()),
                "--world" => args.world = it.next().map(Into::into),
                "--export-world" => args.export_world = it.next().map(Into::into),
                other => eprintln!("неизвестный аргумент {other}"),
            }
        }
        args
    }
}

/// Мир из каталога слоёв (`--world`) или из генератора. Ошибка слоёв
/// завершает программу с кодом 2 и понятным сообщением.
fn load_world(args: &Args) -> World {
    match &args.world {
        Some(dir) => world::layers::import(dir).unwrap_or_else(|e| {
            eprintln!("слои карты: {e}");
            std::process::exit(2);
        }),
        None => World::generate(WORLD_SEED, WORLD_TILES, WORLD_TILES),
    }
}

/// Поза на начале тропы, взгляд вдоль тропы.
fn trail_start_camera(world: &World) -> CameraState {
    let trail = &world.generator.trail;
    let a = trail.point_at(0.0);
    let dir = trail.point_at(0.02) - a;
    let h = world.sample_ground(a.x, a.y).map_or(0.0, |g| g.height);
    CameraState {
        position: Vec3::new(a.x, h + sim::EYE_HEIGHT_M, a.y),
        // forward = (−sin yaw, −cos yaw) в плоскости XZ.
        yaw: (-dir.x).atan2(-dir.y),
        pitch: -0.05,
    }
}

/// Контрольная камера ландшафта: над точкой обзора, взгляд на торфяник.
fn terrain_check_camera(world: &World) -> CameraState {
    let from = world.marker(MarkerKind::Viewpoint).unwrap().position + Vec3::new(0.0, 12.0, 0.0);
    let to = world.generator.basin_center;
    let dir = Vec3::new(to.x, 6.0, to.y) - from;
    CameraState {
        position: from,
        yaw: (-dir.x).atan2(-dir.z),
        pitch: (dir.y / dir.length()).asin(),
    }
}

/// Столбики 0,5 × 4 м на маркерах маршрута.
fn marker_posts(world: &World) -> Vec<InstanceData> {
    world
        .markers
        .iter()
        .map(|m| {
            let tint = match m.kind {
                MarkerKind::TrailStart => [1.0, 0.9, 0.3, 1.0],
                MarkerKind::Viewpoint => [0.4, 1.0, 0.5, 1.0],
                MarkerKind::BridgeSite => [1.0, 0.45, 0.2, 1.0],
                MarkerKind::RiverMouth => [0.3, 0.6, 1.0, 1.0],
                MarkerKind::ShoreEnd => [0.9, 0.9, 0.95, 1.0],
            };
            let model = Mat4::from_scale_rotation_translation(
                Vec3::new(0.5, 4.0, 0.5),
                Quat::IDENTITY,
                m.position + Vec3::new(0.0, 2.0, 0.0),
            );
            InstanceData {
                model: model.to_cols_array_2d(),
                tint,
            }
        })
        .collect()
}

fn start_camera() -> CameraState {
    CameraState {
        position: Vec3::new(0.4, 1.4, 3.6),
        yaw: 0.0,
        pitch: -0.3,
    }
}

/// Сценарий самопроверки. Шаги идут по очереди, каждый ждёт своё условие.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TestStep {
    Warmup,
    ReadbackA,
    HotReload,
    AfterBrokenReload {
        frames: u32,
    },
    /// Окно свёрнуто; разворачивает `about_to_wait` по таймеру.
    /// `presented_at` — счётчик кадров в момент, когда система подтвердила сворачивание.
    Minimized {
        since: Instant,
        presented_at: Option<u64>,
    },
    /// Окно развёрнуто, ждём возобновления вывода.
    Restored {
        presented_before: u64,
        presented_while_minimized: u64,
    },
    Resize,
    ReadbackB,
    /// Сцена переключена на ландшафт с контрольной камерой.
    TerrainView,
    TerrainReadback,
    Done,
}

struct SelfTest {
    step: TestStep,
    frames_in_step: u32,
    failures: Vec<String>,
    log: Vec<String>,
    resize_target: PhysicalSize<u32>,
    size_before: Option<(u32, u32)>,
}

impl SelfTest {
    fn check(&mut self, ok: bool, what: String) {
        let line = format!("[{}] {what}", if ok { "pass" } else { "FAIL" });
        println!("{line}");
        self.log.push(line);
        if !ok {
            self.failures.push(what);
        }
    }
}

struct App {
    args: Args,
    world: World,
    scene: SceneMode,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    sim: Sim,
    clock: FixedClock,
    keys: HashSet<KeyCode>,
    modifiers: ModifiersState,
    look: (f32, f32),
    grabbed: bool,
    last_frame: Instant,
    started: Instant,
    title_timer: Instant,
    frames_since_title: u32,
    last_error: Option<String>,
    readback_requested: bool,
    capture_requested: bool,
    selftest: Option<SelfTest>,
    exit_code: i32,
}

impl App {
    fn new(args: Args) -> App {
        let selftest = args.selftest.then(|| SelfTest {
            step: TestStep::Warmup,
            frames_in_step: 0,
            failures: Vec::new(),
            log: Vec::new(),
            resize_target: PhysicalSize::new(960, 600),
            size_before: None,
        });
        let started = Instant::now();
        let world = load_world(&args);
        eprintln!(
            "мир: {}, seed {:#x}, {}×{} тайлов, генератор v{}, собран за {:.0} мс, контрольная сумма {:016x}",
            args.world
                .as_ref()
                .map_or("генератор".to_string(), |d| format!(
                    "слои {}",
                    d.display()
                )),
            world.seed,
            world.tiles_x,
            world.tiles_z,
            world::GENERATOR_VERSION,
            started.elapsed().as_secs_f64() * 1000.0,
            world.checksum()
        );
        let mut sim = Sim::new(if args.overview {
            terrain_check_camera(&world)
        } else {
            trail_start_camera(&world)
        });
        let scene = if args.selftest {
            SceneMode::Cubes
        } else {
            SceneMode::Terrain
        };
        if !args.selftest && !args.overview {
            sim.set_mode(MoveMode::Walk, &world);
        }
        App {
            args,
            world,
            scene,
            window: None,
            renderer: None,
            sim,
            clock: FixedClock::default(),
            keys: HashSet::new(),
            modifiers: ModifiersState::empty(),
            look: (0.0, 0.0),
            grabbed: false,
            last_frame: Instant::now(),
            started: Instant::now(),
            title_timer: Instant::now(),
            frames_since_title: 0,
            last_error: None,
            readback_requested: false,
            capture_requested: false,
            selftest,
            exit_code: 0,
        }
    }

    fn set_grab(&mut self, grab: bool) {
        let Some(window) = &self.window else { return };
        platform::grab_cursor(window, grab);
        self.grabbed = grab;
        self.look = (0.0, 0.0);
    }

    fn reload_shaders(&mut self) {
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        match r.reload_shaders(None) {
            Ok(generation) => {
                eprintln!("[shaders] перезагружено, поколение {generation}");
                self.last_error = None;
            }
            Err(e) => {
                eprintln!("[shaders] ошибка, работает предыдущий pipeline:\n{e}");
                self.last_error = Some(format!("{} [{}]", e.file, e.stage));
            }
        }
        self.update_title(true);
    }

    fn tick_input(&self) -> TickInput {
        let axis = |pos: KeyCode, neg: KeyCode| {
            (self.keys.contains(&pos) as i32 - self.keys.contains(&neg) as i32) as f32
        };
        TickInput {
            forward: axis(KeyCode::KeyW, KeyCode::KeyS),
            right: axis(KeyCode::KeyD, KeyCode::KeyA),
            up: (axis(KeyCode::KeyE, KeyCode::KeyQ)
                + self.keys.contains(&KeyCode::Space) as i32 as f32)
                .clamp(-1.0, 1.0),
            look_dx: 0.0,
            look_dy: 0.0,
            fast: self.keys.contains(&KeyCode::ShiftLeft)
                || self.keys.contains(&KeyCode::ShiftRight),
        }
    }

    fn update_title(&mut self, force: bool) {
        let elapsed = self.title_timer.elapsed().as_secs_f64();
        if !force && elapsed < 0.5 {
            return;
        }
        let (Some(window), Some(r)) = (&self.window, &self.renderer) else {
            return;
        };
        let fps = self.frames_since_title as f64 / elapsed.max(1e-6);
        let size = r
            .size()
            .map_or("свёрнуто".to_string(), |(w, h)| format!("{w}×{h}"));
        let status = match &self.last_error {
            Some(e) => format!(
                "ОШИБКА MSL {e}, работает поколение {}",
                r.pipeline_generation()
            ),
            None => format!("MSL поколение {}", r.pipeline_generation()),
        };
        let p = self.sim.curr.position;
        let place = match (self.scene, self.world.sample_ground(p.x, p.z)) {
            (SceneMode::Terrain, Some(g)) => format!(
                " · {} · тайл ({}, {}) · {:?}{}{} · {:.1} м",
                if self.sim.mode == MoveMode::Walk {
                    "ходьба"
                } else {
                    "полёт"
                },
                g.tile.x,
                g.tile.z,
                g.substrate,
                if g.on_trail { ", тропа" } else { "" },
                if g.wet { ", вода" } else { "" },
                g.height
            ),
            (SceneMode::Terrain, None) => " · за картой".into(),
            (SceneMode::Cubes, _) => " · тестовые кубы".into(),
        };
        window.set_title(&format!(
            "Эхо мицелия · {} · {size} · {fps:.0} fps · CPU {:.2} мс · {status}{place}",
            r.info.name, r.stats.cpu_encode_ms
        ));
        self.title_timer = Instant::now();
        self.frames_since_title = 0;
    }

    fn is_minimized(&self) -> bool {
        self.window
            .as_ref()
            .and_then(|w| w.is_minimized())
            .unwrap_or(false)
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        if self.is_minimized() {
            // Свёрнутое окно не видно: кадр не кодируем, ресурсы освобождаем.
            if let Some(r) = self.renderer.as_mut() {
                r.collect_completed();
            }
            return;
        }
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f64();
        self.last_frame = now;

        // Поворот мыши применяется в каждом кадре, без ожидания тика (AD-32).
        self.sim.look(self.look.0, self.look.1);
        self.look = (0.0, 0.0);
        let (ticks, alpha) = self.clock.advance(dt);
        let input = self.tick_input();
        for _ in 0..ticks {
            self.sim.step(&input, Some(&self.world));
        }

        let selftest = self.selftest.is_some();
        let camera = match (selftest, self.scene) {
            (true, SceneMode::Cubes) => start_camera(),
            (true, SceneMode::Terrain) => terrain_check_camera(&self.world),
            (false, _) => self.sim.interpolated(alpha),
        };
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        let Some((w, h)) = r.size() else {
            r.collect_completed();
            return;
        };
        let proj = sim::projection(w as f32 / h as f32);
        let view = camera.view();
        let time = self.started.elapsed().as_secs_f32();
        let uniforms = FrameUniforms {
            view_proj: (proj * view).to_cols_array_2d(),
            camera_pos: camera.position.extend(1.0).to_array(),
            light_dir: LIGHT_DIR.normalize().extend(0.0).to_array(),
            time,
            exposure: 1.0,
            pattern_time: if selftest {
                reference::SELFTEST_PATTERN_TIME
            } else {
                time
            },
            frame_index: 0,
        };
        let instances = match self.scene {
            SceneMode::Cubes => scene::instances().to_vec(),
            SceneMode::Terrain => marker_posts(&self.world),
        };
        let capture = (self.capture_requested || self.args.capture_frame == Some(r.frame_index()))
            .then(|| {
                let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("captures");
                let _ = std::fs::create_dir_all(&dir);
                dir.join(format!("frame-{}.gputrace", r.frame_index()))
            });
        self.capture_requested = false;
        let readback = std::mem::take(&mut self.readback_requested);
        let outcome = r.render(&FrameInput {
            uniforms,
            instances: &instances,
            draw_terrain: self.scene == SceneMode::Terrain,
            clear_color: match self.scene {
                SceneMode::Cubes => TEST_SCENE_CLEAR,
                SceneMode::Terrain => SKY_COLOR,
            },
            readback,
            capture,
        });
        if readback && outcome != FrameOutcome::Presented {
            self.readback_requested = true;
        }
        if outcome == FrameOutcome::Presented {
            self.frames_since_title += 1;
        }

        let readbacks = r.take_readbacks();
        for rb in &readbacks {
            if !selftest {
                match self.scene {
                    SceneMode::Cubes => report_readback(rb, view, proj),
                    SceneMode::Terrain => report_terrain_readback(rb, view, proj, &self.world),
                }
            }
        }
        if selftest {
            self.selftest_step(event_loop, readbacks, view, proj);
        }
        self.update_title(false);
    }

    fn selftest_step(
        &mut self,
        event_loop: &ActiveEventLoop,
        readbacks: Vec<Readback>,
        view: Mat4,
        proj: Mat4,
    ) {
        let mut t = self.selftest.take().unwrap();
        if t.step == TestStep::Done {
            // winit может доставить ещё кадр после exit(): итог уже напечатан.
            self.selftest = Some(t);
            return;
        }
        let r = self.renderer.as_mut().unwrap();
        t.frames_in_step += 1;
        let total_frames = r.frame_index();
        match t.step {
            TestStep::Warmup => {
                if t.frames_in_step == 1 {
                    println!("{}", r.info);
                    println!("план проходов (с readback):\n{}", r.plan_summary());
                }
                // Захват мыши как по клику: скрытие курсора и Locked-режим.
                if let Some(window) = &self.window {
                    if t.frames_in_step == 2 {
                        platform::grab_cursor(window, true);
                    }
                    if t.frames_in_step == 4 {
                        platform::grab_cursor(window, false);
                        t.check(true, "захват и освобождение курсора".into());
                    }
                }
                if t.frames_in_step >= 5 {
                    self.readback_requested = true;
                    t.step = TestStep::ReadbackA;
                    t.frames_in_step = 0;
                }
            }
            TestStep::ReadbackA | TestStep::ReadbackB => {
                if let Some(rb) = readbacks.first() {
                    verify_readback(&mut t, rb, view, proj);
                    if t.step == TestStep::ReadbackA {
                        t.step = TestStep::HotReload;
                    } else {
                        self.scene = SceneMode::Terrain;
                        t.step = TestStep::TerrainView;
                    }
                    t.frames_in_step = 0;
                }
            }
            TestStep::HotReload => {
                let before = r.pipeline_generation();
                let ok = r.reload_shaders(None);
                t.check(
                    ok.is_ok(),
                    format!(
                        "перезагрузка MSL без изменений: {:?}",
                        ok.as_ref().map_err(|e| e.to_string())
                    ),
                );
                let generation = r.pipeline_generation();
                t.check(
                    generation == before + 1,
                    format!("поколение pipeline {before} → {generation}"),
                );
                let broken = "#include \"common.h\"\nkernel void pattern_main( { syntax error }\n";
                let err = r.reload_shaders(Some(("pattern.metal", broken)));
                match &err {
                    Err(e) => {
                        println!(
                            "       диагностика: {}",
                            e.to_string().lines().next().unwrap_or("")
                        );
                        t.check(
                            e.file == "pattern.metal" && e.stage == "compile MSL",
                            format!(
                                "ошибочная правка отклонена: файл {}, стадия {}",
                                e.file, e.stage
                            ),
                        );
                    }
                    Ok(_) => t.check(false, "ошибочная правка не должна компилироваться".into()),
                }
                t.check(
                    r.pipeline_generation() == generation,
                    "после ошибки остаётся предыдущее поколение pipeline".into(),
                );
                t.step = TestStep::AfterBrokenReload { frames: 0 };
                t.frames_in_step = 0;
            }
            TestStep::AfterBrokenReload { frames } => {
                if frames >= 3 {
                    t.check(
                        true,
                        format!("кадры рисуются после ошибочной правки ({frames} кадра)"),
                    );
                    let ok = r.reload_shaders(None);
                    t.check(
                        ok.is_ok(),
                        "исправленная правка подхватывается в том же запуске".into(),
                    );
                    if let Some(window) = &self.window {
                        window.set_minimized(true);
                    }
                    t.step = TestStep::Minimized {
                        since: Instant::now(),
                        presented_at: None,
                    };
                    t.frames_in_step = 0;
                } else {
                    t.step = TestStep::AfterBrokenReload { frames: frames + 1 };
                }
            }
            // Пока окно свёрнуто, кадры могут не приходить вовсе; разворачивает
            // окно `about_to_wait` через секунду.
            TestStep::Minimized { .. } => {}
            TestStep::Restored {
                presented_before,
                presented_while_minimized,
            } => {
                let presented = r.stats.presented - presented_before;
                if presented >= 5 {
                    let minimized = self.window.as_ref().and_then(|w| w.is_minimized());
                    t.check(
                        presented_while_minimized == 0,
                        format!("в свёрнутом окне вывод пропускается: выведено {presented_while_minimized} кадров"),
                    );
                    t.check(
                        minimized == Some(false),
                        format!(
                            "после разворота вывод возобновился: {presented} кадров, пропусков без drawable {}",
                            r.stats.skipped_no_drawable
                        ),
                    );
                    t.size_before = r.size();
                    if let Some(window) = &self.window {
                        let _ = window.request_inner_size(t.resize_target);
                    }
                    t.step = TestStep::Resize;
                    t.frames_in_step = 0;
                } else if t.frames_in_step > 300 {
                    t.check(false, "вывод не возобновился после разворота окна".into());
                    t.step = TestStep::Done;
                }
            }
            TestStep::Resize => {
                let size = r.size();
                if let Some(new_size) =
                    size.filter(|_| size != t.size_before && t.frames_in_step >= 4)
                {
                    t.check(true, format!("resize: новые цели {new_size:?}"));
                    self.readback_requested = true;
                    t.step = TestStep::ReadbackB;
                    t.frames_in_step = 0;
                } else if t.frames_in_step > 120 {
                    t.check(false, "resize не пришёл за 120 кадров".into());
                    t.step = TestStep::Done;
                }
            }
            TestStep::TerrainView => {
                if t.frames_in_step >= 3 {
                    self.readback_requested = true;
                    t.step = TestStep::TerrainReadback;
                    t.frames_in_step = 0;
                }
            }
            TestStep::TerrainReadback => {
                if let Some(rb) = readbacks.first() {
                    verify_terrain_readback(&mut t, rb, view, proj, &self.world);
                    t.step = TestStep::Done;
                }
            }
            TestStep::Done => {}
        }
        if t.step != TestStep::Done && total_frames > 1000 {
            t.check(false, format!("самопроверка зависла на шаге {:?}", t.step));
            t.step = TestStep::Done;
        }
        if t.step == TestStep::Done {
            let s = &r.stats;
            println!(
                "кадров {}, пропуски: занят слот {}, нет drawable {}; отложенно освобождено {}; CPU кодирование последнего кадра {:.3} мс",
                s.presented,
                s.skipped_busy,
                s.skipped_no_drawable,
                s.released_deferred,
                s.cpu_encode_ms
            );
            if t.failures.is_empty() {
                println!("SELFTEST PASSED ({} проверок)", t.log.len());
                self.exit_code = 0;
            } else {
                println!("SELFTEST FAILED: {} из {}", t.failures.len(), t.log.len());
                self.exit_code = 1;
            }
            event_loop.exit();
        }
        self.selftest = Some(t);
    }
}

fn verify_readback(t: &mut SelfTest, rb: &Readback, view: Mat4, proj: Mat4) {
    let (mismatches, max_diff) =
        reference::compare_pattern(&rb.pattern, reference::PATTERN_SIZE, rb.pattern_time, 2);
    t.check(
        mismatches == 0,
        format!(
            "compute-узор {}×{} совпадает с эталоном CPU: несовпадений {mismatches}, макс. отклонение {max_diff}/255 (кадр {})",
            reference::PATTERN_SIZE,
            reference::PATTERN_SIZE,
            rb.frame
        ),
    );
    let boxes = scene::boxes();
    let mut worst = 0.0f32;
    let mut hits = 0;
    let mut covered = [false; 2];
    let mut failures = 0;
    for &(x, y, got) in &rb.depth {
        let expected = reference::expected_depth(x, y, rb.width, rb.height, view, proj, &boxes);
        if expected > 0.0 {
            hits += 1;
            // Какой куб ближе: сравнить с глубиной только первого куба.
            let only_a =
                reference::expected_depth(x, y, rb.width, rb.height, view, proj, &boxes[..1]);
            covered[usize::from(only_a != expected)] = true;
        }
        // Допуск: растеризация берёт центр пикселя, эталон — тот же центр;
        // на гранях куба разница — интерполяция float.
        let err = (got - expected).abs();
        let tol = 1e-4 + expected.abs() * 1e-3;
        if err > tol {
            failures += 1;
            println!("       глубина ({x},{y}): GPU {got:.6}, эталон {expected:.6}");
        }
        worst = worst.max(err);
    }
    t.check(
        failures == 0 && hits >= 4 && covered == [true, true],
        format!(
            "reversed-Z глубина в {} точках {}×{} (попаданий в кубы {hits}, видны оба куба {}): макс. ошибка {worst:.2e}",
            rb.depth.len(),
            rb.width,
            rb.height,
            covered == [true, true]
        ),
    );
    let expected = layout::expected_probe();
    let mismatched: Vec<_> = (0..layout::LAYOUT_PROBE_LEN)
        .filter(|&i| rb.probe[i] != expected[i])
        .map(|i| {
            format!(
                "{} GPU {} ≠ CPU {}",
                layout::LAYOUT_PROBE_NAMES[i],
                rb.probe[i],
                expected[i]
            )
        })
        .collect();
    t.check(
        mismatched.is_empty(),
        format!(
            "раскладка структур CPU/GPU ({} полей) {}",
            expected.len(),
            mismatched.join(", ")
        ),
    );
}

/// Глубина ландшафта с GPU против трассировки луча по `SampleGround` на CPU.
/// Возвращает (точек на ландшафте, точек неба, расхождений, наибольшая
/// относительная ошибка глубины).
fn compare_terrain_depth(
    rb: &Readback,
    view: Mat4,
    proj: Mat4,
    world: &World,
    log: bool,
) -> (u32, u32, u32, f32) {
    // Для reversed-Z с бесконечной дальней плоскостью глубина = near / z_view.
    let near = sim::NEAR_PLANE_M;
    let (mut hits, mut sky, mut bad, mut worst) = (0, 0, 0, 0.0f32);
    for &(x, y, got) in &rb.depth {
        let expected = reference::expected_terrain_view_depth(
            (x, y),
            (rb.width, rb.height),
            view,
            proj,
            world,
            3000.0,
        );
        let ok = match expected {
            Some(z) => {
                hits += 1;
                let gpu_z = if got > 0.0 { near / got } else { f32::INFINITY };
                let rel = (gpu_z - z).abs() / z;
                worst = worst.max(rel);
                if log {
                    println!("       ландшафт ({x},{y}): GPU {gpu_z:.3} м, CPU {z:.3} м");
                }
                rel < 2e-3
            }
            None => {
                sky += 1;
                got == 0.0
            }
        };
        if !ok {
            bad += 1;
            println!("       расхождение в ({x},{y}): GPU глубина {got:.7}, CPU {expected:?}");
        }
    }
    (hits, sky, bad, worst)
}

fn verify_terrain_readback(t: &mut SelfTest, rb: &Readback, view: Mat4, proj: Mat4, world: &World) {
    let (hits, sky, bad, worst) = compare_terrain_depth(rb, view, proj, world, false);
    t.check(
        bad == 0 && hits >= 4,
        format!(
            "ландшафт: глубина GPU совпадает с лучом по SampleGround в {hits} точках (небо {sky}), макс. отн. ошибка {worst:.1e}"
        ),
    );
    let expected = layout::expected_probe();
    t.check(
        rb.probe == expected,
        format!(
            "раскладка TerrainVertex и общих структур ({} полей)",
            expected.len()
        ),
    );
}

fn report_terrain_readback(rb: &Readback, view: Mat4, proj: Mat4, world: &World) {
    let (hits, sky, bad, worst) = compare_terrain_depth(rb, view, proj, world, true);
    eprintln!(
        "[readback] ландшафт: точек {hits}, небо {sky}, расхождений {bad}, макс. отн. ошибка {worst:.1e}"
    );
}

fn report_readback(rb: &Readback, view: Mat4, proj: Mat4) {
    let (mismatches, max_diff) =
        reference::compare_pattern(&rb.pattern, reference::PATTERN_SIZE, rb.pattern_time, 2);
    eprintln!(
        "[readback] кадр {}: узор — несовпадений {mismatches}, макс. {max_diff}/255",
        rb.frame
    );
    let boxes = scene::boxes();
    for &(x, y, got) in &rb.depth {
        let expected = reference::expected_depth(x, y, rb.width, rb.height, view, proj, &boxes);
        eprintln!("[readback] глубина ({x},{y}) GPU {got:.6} эталон {expected:.6}");
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Эхо мицелия")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("не удалось создать окно: {e}");
                self.exit_code = 2;
                event_loop.exit();
                return;
            }
        };
        let result = (|| {
            let (device, info) = renderer::probe_device()?;
            eprintln!("{info}");
            let layer = platform::attach_metal_layer(&window)?;
            let (vertices, indices) = scene::cube();
            let mut r = Renderer::new(device, info, layer, &vertices, &indices)?;
            let t0 = Instant::now();
            let terrain = terrain_mesh::vertices(&self.world);
            let detail =
                terrain_mesh::detail_texture(self.world.seed, DETAIL_TEXTURE_SIZE as usize);
            r.set_terrain(
                &terrain,
                &terrain_mesh::tile_indices(),
                self.world.tiles.len(),
                &detail,
                DETAIL_TEXTURE_SIZE,
            )?;
            eprintln!(
                "ландшафт: {} вершин, {} треугольников, загружен за {:.0} мс",
                terrain.len(),
                self.world.tiles.len() * 64 * 64 * 2,
                t0.elapsed().as_secs_f64() * 1000.0
            );
            Ok::<_, String>(r)
        })();
        match result {
            Ok(mut r) => {
                let size = window.inner_size();
                r.resize(size.width, size.height, window.scale_factor());
                eprintln!(
                    "кадр {}×{} px, шейдеры из {}",
                    size.width,
                    size.height,
                    r.shader_dir().display()
                );
                self.renderer = Some(r);
            }
            Err(e) => {
                eprintln!("ОШИБКА: {e}");
                self.exit_code = 2;
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
        self.update_title(true);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width, size.height, scale);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.set_grab(false);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !self.grabbed {
                    self.set_grab(true);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat,
                        ..
                    },
                ..
            } => match state {
                ElementState::Pressed => {
                    self.keys.insert(code);
                    if repeat {
                        return;
                    }
                    match code {
                        KeyCode::Escape => self.set_grab(false),
                        KeyCode::F5 => self.reload_shaders(),
                        KeyCode::F9 => self.readback_requested = true,
                        KeyCode::F12 => self.capture_requested = true,
                        KeyCode::KeyG => {
                            let mode = match self.sim.mode {
                                MoveMode::Walk => MoveMode::Fly,
                                MoveMode::Fly => MoveMode::Walk,
                            };
                            self.sim.set_mode(mode, &self.world);
                            self.update_title(true);
                        }
                        KeyCode::KeyB => {
                            self.sim.head_bob = !self.sim.head_bob;
                        }
                        KeyCode::KeyT => {
                            self.scene = match self.scene {
                                SceneMode::Terrain => SceneMode::Cubes,
                                SceneMode::Cubes => SceneMode::Terrain,
                            };
                            self.update_title(true);
                        }
                        // Меню winit на macOS не содержит пункта Window → Minimize,
                        // поэтому ⌘M обрабатываем сами.
                        KeyCode::KeyM if self.modifiers.super_key() => {
                            self.keys.clear();
                            self.set_grab(false);
                            if let Some(w) = &self.window {
                                w.set_minimized(true);
                            }
                        }
                        _ => {}
                    }
                }
                ElementState::Released => {
                    self.keys.remove(&code);
                }
            },
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.grabbed
        {
            self.look.0 += delta.0 as f32;
            self.look.1 += delta.1 as f32;
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Самопроверка: сворачивание асинхронное (анимация macOS). Счёт кадров
        // начинается, когда окно действительно свёрнуто; через секунду — разворот.
        let minimized_now = self.is_minimized();
        if let (Some(t), Some(w), Some(r)) = (self.selftest.as_mut(), &self.window, &self.renderer)
            && let TestStep::Minimized {
                since,
                presented_at,
            } = &mut t.step
        {
            if presented_at.is_none() && minimized_now {
                *presented_at = Some(r.stats.presented);
                *since = Instant::now();
            }
            let elapsed = since.elapsed().as_secs_f64();
            match *presented_at {
                Some(at) if elapsed >= 1.0 => {
                    w.set_minimized(false);
                    w.focus_window();
                    t.step = TestStep::Restored {
                        presented_before: r.stats.presented,
                        presented_while_minimized: r.stats.presented - at,
                    };
                    t.frames_in_step = 0;
                }
                None if elapsed >= 5.0 => {
                    t.check(false, "окно не свернулось за 5 с".into());
                    w.set_minimized(false);
                    t.step = TestStep::Done;
                }
                _ => {}
            }
        }
        if self.is_minimized() {
            // Не крутить цикл вхолостую: проверять состояние 10 раз в секунду.
            if let Some(r) = self.renderer.as_mut() {
                r.collect_completed();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + std::time::Duration::from_millis(100),
            ));
            // Пауза не должна превратиться в догоняющие тики после разворота.
            self.last_frame = Instant::now();
        } else {
            event_loop.set_control_flow(ControlFlow::Poll);
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Дождаться GPU до освобождения ресурсов.
        self.renderer.take();
    }
}

fn main() {
    let args = Args::parse();
    if let Some(dir) = &args.export_world {
        let world = load_world(&args);
        match world::layers::export(&world, dir) {
            Ok(m) => {
                eprintln!(
                    "слои записаны в {}: {}×{} точек, контрольная сумма мира {}",
                    dir.display(),
                    m.size_samples[0],
                    m.size_samples[1],
                    m.exported_checksum
                );
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("экспорт слоёв: {e}");
                std::process::exit(2);
            }
        }
    }
    if args.capture_frame.is_some() && std::env::var_os("MTL_CAPTURE_ENABLED").is_none() {
        eprintln!("[capture] для записи .gputrace запусти с MTL_CAPTURE_ENABLED=1");
    }
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(args);
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop: {e}");
        std::process::exit(2);
    }
    std::process::exit(app.exit_code);
}
