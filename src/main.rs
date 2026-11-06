//! «Эхо мицелия», этап 01: окно, камера от первого лица, compute → raster,
//! HDR и тонмаппинг, перезагрузка MSL по F5.
//!
//! Управление: клик — захват мыши, WASD — движение, E/Q — вверх/вниз,
//! Shift — быстрее, Esc — отпустить мышь, F5 — перезагрузить MSL,
//! F9 — readback и сверка с эталоном, F12 — GPU capture одного кадра.
//!
//! Режимы: `--selftest` — автоматическая проверка с кодом выхода,
//! `--capture-frame N` — записать кадр N в `captures/`.

mod frame_graph;
mod layout;
mod platform;
mod reference;
mod renderer;
mod scene;
mod sim;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use glam::{Mat4, Vec3};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use layout::FrameUniforms;
use renderer::{FrameInput, FrameOutcome, Readback, Renderer};
use sim::{CameraState, FixedClock, Sim, TickInput};

const LIGHT_DIR: Vec3 = Vec3::new(-0.45, -0.8, -0.4);

#[derive(Default)]
struct Args {
    selftest: bool,
    capture_frame: Option<u64>,
}

impl Args {
    fn parse() -> Args {
        let mut args = Args::default();
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--selftest" => args.selftest = true,
                "--capture-frame" => args.capture_frame = it.next().and_then(|v| v.parse().ok()),
                other => eprintln!("неизвестный аргумент {other}"),
            }
        }
        args
    }
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
    AfterBrokenReload { frames: u32 },
    Resize,
    ReadbackB,
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
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    sim: Sim,
    clock: FixedClock,
    keys: HashSet<KeyCode>,
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
        App {
            args,
            window: None,
            renderer: None,
            sim: Sim::new(start_camera()),
            clock: FixedClock::default(),
            keys: HashSet::new(),
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
        if grab {
            if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
                let _ = window.set_cursor_grab(CursorGrabMode::Confined);
            }
            window.set_cursor_visible(false);
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
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
            up: axis(KeyCode::KeyE, KeyCode::KeyQ),
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
        window.set_title(&format!(
            "Эхо мицелия · {} · macOS {} · {size} · Metal 4 · {fps:.0} fps · CPU {:.2} мс · {status}",
            r.info.name,
            r.info.os.trim_start_matches("Version "),
            r.stats.cpu_encode_ms
        ));
        self.title_timer = Instant::now();
        self.frames_since_title = 0;
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f64();
        self.last_frame = now;

        let (ticks, alpha) = self.clock.advance(dt);
        let mut input = self.tick_input();
        for i in 0..ticks {
            if i == 0 {
                // Поворот мыши за кадр уходит в первый тик кадра.
                input.look_dx = self.look.0;
                input.look_dy = self.look.1;
            } else {
                input.look_dx = 0.0;
                input.look_dy = 0.0;
            }
            self.sim.step(&input);
        }
        if ticks > 0 {
            self.look = (0.0, 0.0);
        }

        let selftest = self.selftest.is_some();
        let camera = if selftest {
            start_camera()
        } else {
            self.sim.interpolated(alpha)
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
        let instances = scene::instances();
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
                report_readback(rb, view, proj);
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
        let r = self.renderer.as_mut().unwrap();
        t.frames_in_step += 1;
        let total_frames = r.frame_index();
        match t.step {
            TestStep::Warmup => {
                if t.frames_in_step == 1 {
                    println!("{}", r.info);
                    println!("план проходов (с readback):\n{}", r.plan_summary());
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
                        t.step = TestStep::Done;
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
                    t.size_before = r.size();
                    if let Some(window) = &self.window {
                        let _ = window.request_inner_size(t.resize_target);
                    }
                    t.step = TestStep::Resize;
                    t.frames_in_step = 0;
                } else {
                    t.step = TestStep::AfterBrokenReload { frames: frames + 1 };
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
            Renderer::new(device, info, layer, &vertices, &indices)
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

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Дождаться GPU до освобождения ресурсов.
        self.renderer.take();
    }
}

fn main() {
    let args = Args::parse();
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
