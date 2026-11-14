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
