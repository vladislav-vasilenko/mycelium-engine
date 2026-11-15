//! Приложение: окно, ввод, симуляция и кадр поверх рендерера.
//!
//! Подмодули: `args` — аргументы командной строки и загрузка мира,
//! `cameras` — стартовые и контрольные камеры, `input` — клавиатура и мышь,
//! `frame` — сборка и отправка кадра, `hands` — руки в кадре, `title` — заголовок окна,
//! `handler` — события winit, `setup` — создание рендерера и ресурсов сцены,
//! `selftest` — сценарий самопроверки и сверки с эталонами.

mod args;
mod cameras;
mod frame;
mod handler;
mod hands;
mod input;
mod raft;
mod selftest;
mod setup;
mod shadows;
mod sky;
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
use cameras::{island_overview_camera, look_camera, marker_camera, trail_start_camera};
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
    /// Мир общий с потоком сборки кластеров (02.10).
    world: std::sync::Arc<World>,
    /// Сборка кластеров в фоне: пока идёт — рельеф тайлами, в интерфейсе ход загрузки.
    cluster_job: Option<ClusterJob>,
    /// TAA (04.5): (проекция × поворот вида без сдвига, камера, размер) прошлого кадра.
    taa_prev: Option<(glam::Mat4, glam::Vec3, (u32, u32))>,
    /// Время мира прошлого кадра (04.6): шаг ветра для векторов движения.
    prev_world_time: f32,
    /// Погода (04.3).
    weather: crate::weather::Weather,
    /// Гидрология острова (03.2): уровень рек от дождя.
    hydrology: crate::hydrology::Hydrology,
    /// Время до следующей передачи разлива рек в рендер, с.
    river_push_s: f32,
    /// Демо-сплав по реке (`--raft`).
    raft: Option<raft::Raft>,
    /// Уровень воды рек на концах отрезков (03.2), обновляется с разливом.
    river_levels: Vec<[f32; 4]>,
    /// Кинематографичная обработка в тонмаппинге (04.5), клавиша K.
    grade: bool,
    taa_frame: u32,
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
    /// Руки от первого лица (этап 10), если ассет загрузился.
    hands: Option<crate::hands::Hands>,
    /// Руки видны (клавиша H).
    hands_visible: bool,
    /// Игровой срез (05.1): грибы, осмотр, журнал; шрифт интерфейса.
    game: crate::game::Game,
    font: Option<crate::ui::Font>,
    /// Кластерный рельеф (02.3): иерархия, срез кадра, режим (клавиша L).
    clusters: Option<crate::terrain_clusters::ClusterSet>,
    visible_clusters: Vec<u32>,
    /// Кластеры каскадов теней подряд (04.2) и список одного каскада.
    shadow_clusters: Vec<u32>,
    shadow_scratch: Vec<u32>,
    /// Растения загружены в рендерер (02.4).
    forest_loaded: bool,
    use_clusters: bool,
    cluster_stats: crate::terrain_clusters::SelectStats,
    cluster_select_ms: f64,
    /// Небо 04.1: место наблюдателя и игровое время.
    site: crate::sky::celestial::Site,
    time: crate::sky::time::TimeOfDay,
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

/// Фоновая сборка кластеров рельефа (02.10): результат и время начала.
pub(super) struct ClusterJob {
    pub rx: std::sync::mpsc::Receiver<(crate::terrain_clusters::ClusterSet, bool)>,
    pub started: Instant,
}

impl App {
    pub fn new(args: Args) -> App {
        let mut weather = crate::weather::Weather::new(
            args.weather
                .as_deref()
                .and_then(crate::weather::Preset::from_id)
                .unwrap_or(crate::weather::Preset::Clear),
        );
        if let Some(p) = args.puddles {
            weather.puddles = p.clamp(0.0, 1.0);
        }
        let start_view = args.view.unwrap_or(TerrainView::Lit);
        let selftest = args.selftest.then(SelfTest::new);
        let started = Instant::now();
        let world = load_world(&args);
        let flow = args.world.as_ref().and_then(|d| {
            crate::hydrology::FlowGrid::load(&d.join("flow.png"), world.size_m().x)
                .map_err(|e| eprintln!("гидрология: {e} — водосбор по длине рек"))
                .ok()
        });
        let hydrology = world.features.as_ref().map_or_else(Default::default, |f| {
            crate::hydrology::Hydrology::build(
                f,
                |p| world.sample_ground(p.x, p.y).map(|g| g.height),
                flow.as_ref(),
            )
        });
        eprintln!("гидрология: {} участков рек", hydrology.reaches.len());
        if std::env::var_os("ECHO_MOUTHS").is_some() {
            // Устья в море (03.2): крупнейшие по водосбору — для камер.
            let mut m: Vec<_> = hydrology
                .reaches
                .iter()
                .filter(|r| r.into.is_none())
                .collect();
            m.sort_by(|a, b| b.area_m2.total_cmp(&a.area_m2));
            for r in m.iter().take(8) {
                let h = world
                    .sample_ground(r.mouth.x, r.mouth.y)
                    .map(|g| (g.height, g.wet));
                eprintln!(
                    "устье: ({:.0}, {:.0}) площадь {:.1} км² ширина {:.0} м грунт {h:?}",
                    r.mouth.x,
                    r.mouth.y,
                    r.area_m2 / 1e6,
                    r.width_m
                );
            }
        }
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
        let game = crate::game::Game::new(&world);
        let mut sim = Sim::new(if let Some((mut from, to)) = args.camera {
            // Глаз не ниже 0,3 м над землёй: из-под рельефа видна его изнанка
            // и небо — кадр выглядит перевёрнутым (02.10).
            if let Some(g) = world.sample_ground(from.x, from.z) {
                from.y = from.y.max(g.height + 0.3);
            }
            look_camera(from, to)
        } else if args.overview {
            island_overview_camera(&world)
        } else if let Some(c) = args.at.and_then(|k| marker_camera(&world, k)) {
            // Та же камера флагом `--camera` — для повторяемых снимков.
            let f = c.position + c.forward() * 10.0;
            eprintln!(
                "камера --at: --camera {:.1},{:.1},{:.1},{:.1},{:.1},{:.1}",
                c.position.x, c.position.y, c.position.z, f.x, f.y, f.z
            );
            c
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
        // Демо-сплав (03.2): от ближайшей реки вниз по течению.
        let raft = if args.raft {
            let r = raft::Raft::new(&world, &hydrology, sim.curr.position).map(|mut r| {
                r.dive = args.dive;
                r
            });
            if r.is_none() {
                eprintln!("сплав: реки рядом нет");
            }
            r
        } else {
            None
        };
        let tiles_only = args.tiles;
        let site = sky::site_of(&world);
        let time = sky::initial_time(&args, site);
        App {
            args,
            world: std::sync::Arc::new(world),
            cluster_job: None,
            taa_prev: None,
            prev_world_time: 0.0,
            weather,
            hydrology,
            river_push_s: 0.0,
            raft,
            river_levels: Vec::new(),
            grade: true,
            taa_frame: 0,
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
            hands: None,
            hands_visible: true,
            game,
            font: None,
            clusters: None,
            visible_clusters: Vec::new(),
            shadow_clusters: Vec::new(),
            shadow_scratch: Vec::new(),
            forest_loaded: false,
            use_clusters: !tiles_only,
            cluster_stats: Default::default(),
            cluster_select_ms: 0.0,
            site,
            time,
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
        self.sim.set_mode(MoveMode::Walk, &*self.world);
    }

    fn is_minimized(&self) -> bool {
        self.window
            .as_ref()
            .and_then(|w| w.is_minimized())
            .unwrap_or(false)
    }
}
