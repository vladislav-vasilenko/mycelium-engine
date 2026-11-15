//! Аргументы командной строки и загрузка мира.

use glam::Vec3;

use super::{WORLD_SEED, WORLD_TILES};
use crate::terrain_materials::TerrainView;
use crate::world::{self, World};

#[derive(Default)]
pub struct Args {
    pub selftest: bool,
    /// Старт в полёте с контрольной камеры ландшафта.
    pub overview: bool,
    pub capture_frame: Option<u64>,
    /// Каталог слоёв карты вместо генератора.
    pub world: Option<std::path::PathBuf>,
    /// Записать слои мира в каталог и выйти.
    pub export_world: Option<std::path::PathBuf>,
    /// Стартовая камера в полёте: откуда и куда смотреть, мировые метры.
    pub camera: Option<(Vec3, Vec3)>,
    /// Стартовый вид ландшафта.
    pub view: Option<TerrainView>,
    /// Стартовая поза рук: индекс в `hands::POSE_NAMES`.
    pub pose: Option<usize>,
    /// Рельеф тайлами уровня 0 вместо кластеров (сравнение, 02.3).
    pub tiles: bool,
    /// Срез кластеров на CPU (02.3) вместо GPU (02.5) — для сравнения.
    pub cpu_cull: bool,
    /// Чувство мицелия включено без удержания C (05.2, отладка и снимки).
    pub sense: bool,
    /// Без временного сглаживания (04.5) — для сравнения.
    pub no_taa: bool,
    /// Без лучей затенения (11) — для сравнения.
    pub no_rt: bool,
    /// Печатать время проходов GPU раз в 2 с.
    pub gpu_times: bool,
    /// Масштаб рендера сцены (04.6): по умолчанию 0,67 с апскейлом MetalFX; 1 — без апскейла.
    pub render_scale: Option<f32>,
    /// Погода при старте (04.3): clear, cloudy, drizzle, rain, storm.
    pub weather: Option<String>,
    /// Начальный уровень луж 0…1 (04.3): без ожидания долгого ливня.
    pub puddles: Option<f32>,
    /// Демо-сплав по реке (03.2).
    pub raft: bool,
    /// Сплав сразу под водой (проверка подводного вида).
    pub dive: bool,
    /// Пешком в реке у начала тропы (проверка плавания, 03.6).
    pub swim: bool,
    /// Ночной побег от дрона (05.6): ночь, дрон, маяки к водопаду.
    pub escape: bool,
    /// Предмет в руке с начала (0 — корзина, 1 — совок, 2 — фонарь, 3 — рация).
    pub prop: Option<usize>,
    /// Действие предметом с начала (проверка): `talk`, `dig`, `aim`.
    pub act: Option<String>,
    /// Небо 04.1: дата, местное время в часах, скорость хода, фиксированная экспозиция.
    pub date: Option<(i32, u32, u32)>,
    pub time_h: Option<f64>,
    pub timescale: Option<f64>,
    pub exposure: Option<f32>,
    /// Старт пешком на маркере: `trail`, `mouth` (устье реки), `beach`.
    pub at: Option<crate::world::MarkerKind>,
}

impl Args {
    pub fn parse() -> Args {
        let mut args = Args::default();
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--selftest" => args.selftest = true,
                "--overview" => args.overview = true,
                "--capture-frame" => args.capture_frame = it.next().and_then(|v| v.parse().ok()),
                "--world" => args.world = it.next().map(Into::into),
                "--export-world" => args.export_world = it.next().map(Into::into),
                "--camera" => {
                    let v: Vec<f32> = it
                        .next()
                        .unwrap_or_default()
                        .split(',')
                        .filter_map(|x| x.trim().parse().ok())
                        .collect();
                    if v.len() == 6 {
                        args.camera =
                            Some((Vec3::new(v[0], v[1], v[2]), Vec3::new(v[3], v[4], v[5])));
                    } else {
                        eprintln!("--camera x,y,z,tx,ty,tz — нужно 6 чисел");
                    }
                }
                "--view" => {
                    args.view = it.next().and_then(|n| TerrainView::parse(&n));
                    if args.view.is_none() {
                        eprintln!(
                            "--view: lit, height, slope, biome, substrate, coast, wetness, materials"
                        );
                    }
                }
                "--tiles" => args.tiles = true,
                "--cpu-cull" => args.cpu_cull = true,
                "--sense" => args.sense = true,
                "--no-taa" => args.no_taa = true,
                "--no-rt" => args.no_rt = true,
                "--gpu-times" => args.gpu_times = true,
                "--render-scale" => args.render_scale = it.next().and_then(|v| v.parse().ok()),
                "--weather" => args.weather = it.next(),
                "--puddles" => args.puddles = it.next().and_then(|v| v.parse().ok()),
                "--raft" => args.raft = true,
                "--dive" => args.dive = true,
                "--swim" => args.swim = true,
                "--escape" => {
                    // Ночь без ускорения времени, лёгкая морось — дымка в луче.
                    args.escape = true;
                    args.time_h.get_or_insert(23.5);
                    args.timescale.get_or_insert(1.0);
                    args.weather.get_or_insert_with(|| "drizzle".to_string());
                }
                "--act" => args.act = it.next(),
                "--prop" => args.prop = it.next().and_then(|v| v.parse::<usize>().ok()),
                "--date" => {
                    let v: Vec<u32> = it
                        .next()
                        .unwrap_or_default()
                        .split('-')
                        .filter_map(|p| p.parse().ok())
                        .collect();
                    args.date = (v.len() == 3).then(|| (v[0] as i32, v[1], v[2]));
                    if args.date.is_none() {
                        eprintln!("--date ГГГГ-ММ-ДД");
                    }
                }
                "--time" => {
                    let v: Vec<f64> = it
                        .next()
                        .unwrap_or_default()
                        .split(':')
                        .filter_map(|p| p.parse().ok())
                        .collect();
                    args.time_h = match v.as_slice() {
                        [h] => Some(*h),
                        [h, m] => Some(h + m / 60.0),
                        _ => {
                            eprintln!("--time ЧЧ:ММ");
                            None
                        }
                    };
                }
                "--timescale" => args.timescale = it.next().and_then(|v| v.parse().ok()),
                "--exposure" => {
                    args.exposure = match it.next().as_deref() {
                        Some("auto") | None => None,
                        Some(v) => v.parse().ok(),
                    }
                }
                "--at" => {
                    use crate::world::MarkerKind;
                    args.at = match it.next().as_deref() {
                        Some("trail") => Some(MarkerKind::TrailStart),
                        Some("mouth") => Some(MarkerKind::RiverMouth),
                        Some("beach") => Some(MarkerKind::ShoreEnd),
                        Some("view") => Some(MarkerKind::Viewpoint),
                        _ => {
                            eprintln!(
                                "--at: trail, mouth, beach, view (первая смотровая точка: у Якусимы — Дзёмон-суги)"
                            );
                            None
                        }
                    };
                }
                "--pose" => {
                    let names = crate::hands::POSE_NAMES;
                    args.pose = it.next().and_then(|n| names.iter().position(|&p| p == n));
                    if args.pose.is_none() {
                        eprintln!("--pose: {}", names.join(", "));
                    }
                }
                other => eprintln!("неизвестный аргумент {other}"),
            }
        }
        args
    }
}

/// Мир из каталога слоёв (`--world`) или из генератора. Ошибка слоёв
/// завершает программу с кодом 2 и понятным сообщением.
pub fn load_world(args: &Args) -> World {
    match &args.world {
        Some(dir) => world::layers::import(dir).unwrap_or_else(|e| {
            eprintln!("слои карты: {e}");
            std::process::exit(2);
        }),
        None => World::generate(WORLD_SEED, WORLD_TILES, WORLD_TILES),
    }
}
