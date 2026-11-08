//! Приложение: окно, ввод, симуляция и кадр поверх рендерера.
//!
//! Подмодули: `args` — аргументы командной строки и загрузка мира,
//! `cameras` — стартовые и контрольные камеры, `input` — клавиатура и мышь,
//! `frame` — сборка и отправка кадра, `title` — заголовок окна,
//! `handler` — события winit, `setup` — создание рендерера и ресурсов сцены,
//! `selftest` — сценарий самопроверки и сверки с эталонами.

mod args;
mod cameras;
mod frame;
mod handler;
mod input;
mod selftest;
mod setup;
mod title;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use winit::keyboard::{KeyCode, ModifiersState};
use winit::window::Window;

use crate::platform;
use crate::renderer::Renderer;
use crate::sim::{FixedClock, MoveMode, Sim};
use crate::terrain_materials::TerrainView;
use crate::water;
use crate::world::{self, Ground, World};

pub use args::{Args, load_world};
use cameras::{island_overview_camera, look_camera, trail_start_camera};
use selftest::SelfTest;

const LIGHT_DIR: Vec3 = Vec3::new(-0.45, -0.8, -0.4);
/// Seed первой карты и её размер в тайлах (1024 × 1024 м, остров).
const WORLD_SEED: u64 = 0xEC40;
const WORLD_TILES: i32 = world::FIRST_MAP_TILES;
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

pub struct App {
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
    /// Вид ландшафта (клавиша V).
    view_mode: TerrainView,
    /// Море: волны и прибой (03.1), поле берега после загрузки рендерера.
    sea: water::SeaState,
    shore: Option<water::ShoreField>,
    /// Длительность прошлого кадра для маски мокрого песка, с.
    frame_dt: f32,
    grabbed: bool,
    last_frame: Instant,
    title_timer: Instant,
    frames_since_title: u32,
    last_error: Option<String>,
    readback_requested: bool,
    capture_requested: bool,
    selftest: Option<SelfTest>,
    pub exit_code: i32,
}

impl App {
    pub fn new(args: Args) -> App {
        let start_view = args.view.unwrap_or(TerrainView::Lit);
        let selftest = args.selftest.then(SelfTest::new);
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
        let mut sim = Sim::new(if let Some((from, to)) = args.camera {
            look_camera(from, to)
        } else if args.overview {
            island_overview_camera(&world)
        } else {
            trail_start_camera(&world)
        });
        let scene = if args.selftest {
            SceneMode::Cubes
        } else {
            SceneMode::Terrain
        };
        if !args.selftest && !args.overview && args.camera.is_none() {
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
            view_mode: start_view,
            sea: water::SeaState::new(WORLD_SEED),
            shore: None,
            frame_dt: 0.0,
            grabbed: false,
            last_frame: Instant::now(),
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

    /// Переход в ходьбу. Над сушей игрок встаёт на грунт под камерой; над морем,
    /// за картой или при `to_trail_start` — на начало тропы, взгляд вдоль неё.
    fn land(&mut self, to_trail_start: bool) {
        let p = self.sim.curr.position;
        let dry = self.world.sample_ground(p.x, p.z).is_some_and(|g| !g.wet);
        if to_trail_start || !dry {
            let head_bob = self.sim.head_bob;
            self.sim = Sim::new(trail_start_camera(&self.world));
            self.sim.head_bob = head_bob;
        }
        self.sim.set_mode(MoveMode::Walk, &self.world);
    }

    fn is_minimized(&self) -> bool {
        self.window
            .as_ref()
            .and_then(|w| w.is_minimized())
            .unwrap_or(false)
    }
}
