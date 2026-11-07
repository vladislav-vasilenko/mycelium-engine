//! Мир: тайлы рельефа, поля среды, веса биомов и запрос `sample_ground`.
//!
//! Карта разбита на тайлы 64 × 64 м с целочисленными координатами. LOD 0
//! хранит 65 × 65 высот с шагом 1 м, граничные точки общие с соседями.
//! Высоты, нормали и поля берутся из одной мировой сетки, запечённой
//! генератором, поэтому стыки совпадают точно.
//!
//! Треугольники ячейки: диагональ из (i, j) в (i+1, j+1),
//! треугольники (a, c, b) и (a, d, c) с обходом CCW сверху.

pub mod generator;
pub mod noise;

use glam::{Vec2, Vec3};

use generator::{Fields, Generator, SEA_LEVEL};

pub const TILE_SIZE_M: f32 = 64.0;
/// Точек на сторону тайла в LOD 0.
pub const TILE_SAMPLES: usize = 65;
pub const SAMPLE_STEP_M: f32 = 1.0;
/// Версия алгоритма генерации. Меняется вместе с результатом генератора.
pub const GENERATOR_VERSION: u32 = 1;

/// Стабильный идентификатор тайла: целые координаты в сетке тайлов.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub x: i32,
    pub z: i32,
}

impl TileId {
    pub fn origin(self) -> Vec2 {
        Vec2::new(self.x as f32, self.z as f32) * TILE_SIZE_M
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Substrate {
    Soil = 0,
    Peat = 1,
    Sand = 2,
    Rock = 3,
    Gravel = 4,
}

/// Нормированные веса биомов: лес, торфяник, прибрежный грунт.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiomeWeights {
    pub forest: f32,
    pub peat: f32,
    pub shore: f32,
}

/// Данные одной точки тайла.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub height: f32,
    pub normal: Vec3,
    pub moisture: f32,
    pub temperature: f32,
    pub substrate: Substrate,
    pub weights: BiomeWeights,
    pub trail: f32,
    pub river: f32,
}

#[derive(Clone, Debug)]
pub struct Tile {
    pub id: TileId,
    pub world_seed: u64,
    pub generator_version: u32,
    /// 65 × 65 точек, строка за строкой по z, внутри строки по x.
    pub samples: Vec<Sample>,
}

impl Tile {
    pub fn at(&self, i: usize, j: usize) -> &Sample {
        &self.samples[j * TILE_SAMPLES + i]
    }
}

/// Результат запроса к грунту.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundSample {
    pub height: f32,
    /// Нормаль треугольника LOD 0 под точкой.
    pub normal: Vec3,
    pub substrate: Substrate,
    pub on_trail: bool,
    /// Точка в русле или ниже уровня моря.
    pub wet: bool,
    pub tile: TileId,
}

/// Поверхность для движения. Реализуется миром и тестовыми поверхностями.
pub trait Ground {
    /// `None` за пределами карты.
    fn sample_ground(&self, x: f32, z: f32) -> Option<GroundSample>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerKind {
    TrailStart,
    Viewpoint,
    BridgeSite,
    RiverMouth,
    ShoreEnd,
}

/// Авторская точка маршрута.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marker {
    pub name: &'static str,
    pub kind: MarkerKind,
    pub position: Vec3,
}

pub struct World {
    pub seed: u64,
    pub tiles_x: i32,
    pub tiles_z: i32,
    pub generator: Generator,
    /// Тайлы по строкам: индекс `z * tiles_x + x`.
    pub tiles: Vec<Tile>,
    pub markers: Vec<Marker>,
}

impl World {
    pub fn size_m(&self) -> Vec2 {
        Vec2::new(self.tiles_x as f32, self.tiles_z as f32) * TILE_SIZE_M
    }

    pub fn tile(&self, id: TileId) -> Option<&Tile> {
        if id.x < 0 || id.z < 0 || id.x >= self.tiles_x || id.z >= self.tiles_z {
            return None;
        }
        self.tiles.get((id.z * self.tiles_x + id.x) as usize)
    }

    /// Генерирует и запекает карту. Строки мировой сетки считаются параллельно,
    /// результат от числа потоков не зависит.
    pub fn generate(seed: u64, tiles_x: i32, tiles_z: i32) -> World {
        let nx = tiles_x as usize * (TILE_SAMPLES - 1) + 1;
        let nz = tiles_z as usize * (TILE_SAMPLES - 1) + 1;
        let generator = Generator::first_map(seed, tiles_z as f32 * TILE_SIZE_M);

        // 1. Поля на общей мировой сетке.
        let mut fields = vec![
            Fields {
                height: 0.0,
                moisture: 0.0,
                temperature: 0.0,
                trail: 0.0,
                river: 0.0,
                coast: 0.0,
                basin: 0.0,
            };
            nx * nz
        ];
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let rows_per = nz.div_ceil(threads);
        std::thread::scope(|s| {
            for (chunk_i, chunk) in fields.chunks_mut(rows_per * nx).enumerate() {
                let generator = &generator;
                s.spawn(move || {
                    for (k, f) in chunk.iter_mut().enumerate() {
                        let j = chunk_i * rows_per + k / nx;
                        let i = k % nx;
                        *f = generator.sample(i as f32 * SAMPLE_STEP_M, j as f32 * SAMPLE_STEP_M);
                    }
                });
            }
        });

        // 2. Нормали центральными разностями по той же сетке (у края карты — односторонние).
        let h = |i: isize, j: isize| {
            let i = i.clamp(0, nx as isize - 1) as usize;
            let j = j.clamp(0, nz as isize - 1) as usize;
            fields[j * nx + i].height
        };
        let normal_at = |i: usize, j: usize| {
            let (i, j) = (i as isize, j as isize);
            let dx = (h(i + 1, j) - h(i - 1, j)) / (2.0 * SAMPLE_STEP_M);
            let dz = (h(i, j + 1) - h(i, j - 1)) / (2.0 * SAMPLE_STEP_M);
            Vec3::new(-dx, 1.0, -dz).normalize()
        };

        // 3. Нарезка на тайлы с общими граничными точками.
        let mut tiles = Vec::with_capacity((tiles_x * tiles_z) as usize);
        for tz in 0..tiles_z {
            for tx in 0..tiles_x {
                let mut samples = Vec::with_capacity(TILE_SAMPLES * TILE_SAMPLES);
                for j in 0..TILE_SAMPLES {
                    for i in 0..TILE_SAMPLES {
                        let gi = tx as usize * (TILE_SAMPLES - 1) + i;
                        let gj = tz as usize * (TILE_SAMPLES - 1) + j;
                        let f = fields[gj * nx + gi];
                        let n = normal_at(gi, gj);
                        let slope = 1.0 - n.y;
                        let weights = generator::biome_weights(&f, slope);
                        samples.push(Sample {
                            height: f.height,
                            normal: n,
                            moisture: f.moisture,
                            temperature: f.temperature,
                            substrate: generator::substrate(&f, slope, &weights, gi as f32),
                            weights,
                            trail: f.trail,
                            river: f.river,
                        });
                    }
                }
                tiles.push(Tile {
                    id: TileId { x: tx, z: tz },
                    world_seed: seed,
                    generator_version: GENERATOR_VERSION,
                    samples,
                });
            }
        }

        let mut world = World {
            seed,
            tiles_x,
            tiles_z,
            generator,
            tiles,
            markers: Vec::new(),
        };
        world.markers = world.place_markers();
        world
    }

    fn place_markers(&self) -> Vec<Marker> {
        let g = &self.generator;
        let on_ground = |p: Vec2| {
            let h = self.sample_ground(p.x, p.y).map_or(0.0, |s| s.height);
            Vec3::new(p.x, h, p.y)
        };
        // Мост — пересечение тропы с руслом: ближайшая к реке точка тропы.
        let bridge = (0..=400)
            .map(|k| g.trail.point_at(k as f32 / 400.0))
            .min_by(|a, b| g.river.nearest(*a).0.total_cmp(&g.river.nearest(*b).0))
            .unwrap();
        // Устье — точка реки на береговой линии.
        let mouth = (0..=400)
            .map(|k| g.river.point_at(k as f32 / 400.0))
            .find(|p| p.y >= g.shoreline_z(p.x))
            .unwrap_or(g.river.point_at(1.0));
        vec![
            Marker {
                name: "начало тропы",
                kind: MarkerKind::TrailStart,
                position: on_ground(g.trail.point_at(0.0)),
            },
            Marker {
                name: "склон над лесом",
                kind: MarkerKind::Viewpoint,
                position: on_ground(g.trail.point_at(0.18)),
            },
            Marker {
                name: "край торфяника",
                kind: MarkerKind::Viewpoint,
                position: on_ground(g.trail.point_at(0.45)),
            },
            Marker {
                name: "место моста",
                kind: MarkerKind::BridgeSite,
                position: on_ground(bridge),
            },
            Marker {
                name: "устье",
                kind: MarkerKind::RiverMouth,
                position: on_ground(mouth),
            },
            Marker {
                name: "каменистый берег",
                kind: MarkerKind::ShoreEnd,
                position: on_ground(g.trail.point_at(1.0)),
            },
        ]
    }

    pub fn marker(&self, kind: MarkerKind) -> Option<&Marker> {
        self.markers.iter().find(|m| m.kind == kind)
    }

    /// Контрольная сумма запечённых данных: для проверки детерминизма.
    pub fn checksum(&self) -> u64 {
        let mut h = noise::mix64(self.seed ^ GENERATOR_VERSION as u64);
        for t in &self.tiles {
            h = noise::mix64(h ^ t.world_seed ^ ((t.generator_version as u64) << 32));
            h = noise::mix64(h ^ ((t.id.x as u32 as u64) << 32 | t.id.z as u32 as u64));
            for s in &t.samples {
                for v in [
                    s.height, s.normal.x, s.normal.y, s.normal.z, s.moisture, s.trail,
                ] {
                    h = noise::mix64(h ^ v.to_bits() as u64);
                }
                h = noise::mix64(h ^ s.substrate as u64);
            }
        }
        h
    }
}

impl Ground for World {
    /// Высота по треугольникам LOD 0, как их рисует рендер.
    fn sample_ground(&self, x: f32, z: f32) -> Option<GroundSample> {
        let size = self.size_m();
        if !(0.0..=size.x).contains(&x) || !(0.0..=size.y).contains(&z) {
            return None;
        }
        // Точка на правом или нижнем краю карты относится к последнему тайлу.
        let id = TileId {
            x: ((x / TILE_SIZE_M) as i32).min(self.tiles_x - 1),
            z: ((z / TILE_SIZE_M) as i32).min(self.tiles_z - 1),
        };
        let tile = self.tile(id)?;
        let local = (Vec2::new(x, z) - id.origin()) / SAMPLE_STEP_M;
        let i = (local.x.floor() as usize).min(TILE_SAMPLES - 2);
        let j = (local.y.floor() as usize).min(TILE_SAMPLES - 2);
        let (fx, fz) = (local.x - i as f32, local.y - j as f32);
        let a = tile.at(i, j);
        let b = tile.at(i + 1, j);
        let c = tile.at(i + 1, j + 1);
        let d = tile.at(i, j + 1);
        let s = SAMPLE_STEP_M;
        let (height, normal) = if fx >= fz {
            // Треугольник (a, c, b).
            let h = a.height + fx * (b.height - a.height) + fz * (c.height - b.height);
            let n =
                Vec3::new(s, c.height - a.height, s).cross(Vec3::new(s, b.height - a.height, 0.0));
            (h, n.normalize())
        } else {
            // Треугольник (a, d, c).
            let h = a.height + fz * (d.height - a.height) + fx * (c.height - d.height);
            let n =
                Vec3::new(0.0, d.height - a.height, s).cross(Vec3::new(s, c.height - a.height, s));
            (h, n.normalize())
        };
        // Нечисловые свойства — от ближайшей точки сетки.
        let near = tile.at(
            (local.x.round() as usize).min(TILE_SAMPLES - 1),
            (local.y.round() as usize).min(TILE_SAMPLES - 1),
        );
        Some(GroundSample {
            height,
            normal,
            substrate: near.substrate,
            on_trail: near.trail > 0.5,
            wet: near.river > 0.5 || height < SEA_LEVEL,
            tile: id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    pub fn test_world() -> &'static World {
        static W: OnceLock<World> = OnceLock::new();
        W.get_or_init(|| World::generate(0xEC40, 12, 12))
    }

    #[test]
    fn neighbour_tiles_share_border_heights_and_normals() {
        let w = test_world();
        for tz in 0..w.tiles_z {
            for tx in 0..w.tiles_x - 1 {
                let a = w.tile(TileId { x: tx, z: tz }).unwrap();
                let b = w.tile(TileId { x: tx + 1, z: tz }).unwrap();
                for j in 0..TILE_SAMPLES {
                    assert_eq!(a.at(TILE_SAMPLES - 1, j), b.at(0, j));
                }
            }
        }
        for tz in 0..w.tiles_z - 1 {
            for tx in 0..w.tiles_x {
                let a = w.tile(TileId { x: tx, z: tz }).unwrap();
                let b = w.tile(TileId { x: tx, z: tz + 1 }).unwrap();
                for i in 0..TILE_SAMPLES {
                    assert_eq!(a.at(i, TILE_SAMPLES - 1), b.at(i, 0));
                }
            }
        }
    }

    #[test]
    fn same_world_coordinates_give_same_ground_from_both_tiles() {
        let w = test_world();
        // Точки ровно на границе тайлов: ответ не зависит от того, какой тайл выбран.
        for k in 0..40 {
            let x = 64.0 * (1 + k % 10) as f32;
            let z = 13.37 + 17.0 * k as f32;
            let s = w.sample_ground(x, z).unwrap();
            let left_tile = w
                .tile(TileId {
                    x: x as i32 / 64 - 1,
                    z: (z / 64.0) as i32,
                })
                .unwrap();
            let j = ((z - left_tile.id.origin().y) / SAMPLE_STEP_M) as usize;
            let fz = (z - left_tile.id.origin().y) / SAMPLE_STEP_M - j as f32;
            let h0 = left_tile.at(64, j).height;
            let h1 = left_tile.at(64, j + 1).height;
            assert!(
                (s.height - (h0 + (h1 - h0) * fz)).abs() < 1e-4,
                "x={x} z={z}"
            );
        }
    }

    #[test]
    fn sample_ground_matches_lod0_vertices() {
        let w = test_world();
        let t = w.tile(TileId { x: 3, z: 5 }).unwrap();
        for (i, j) in [(0, 0), (10, 20), (63, 1), (64, 64), (31, 47)] {
            let p = t.id.origin() + Vec2::new(i as f32, j as f32);
            let s = w.sample_ground(p.x, p.y).unwrap();
            assert!((s.height - t.at(i, j).height).abs() < 1e-4);
        }
    }

    #[test]
    fn generation_is_deterministic() {
        let a = World::generate(0xEC40, 3, 3);
        let b = World::generate(0xEC40, 3, 3);
        assert_eq!(a.checksum(), b.checksum());
        let c = World::generate(0xEC41, 3, 3);
        assert_ne!(a.checksum(), c.checksum());
        assert!(
            a.tiles
                .iter()
                .all(|t| t.world_seed == 0xEC40 && t.generator_version == GENERATOR_VERSION)
        );
    }

    #[test]
    fn biome_weights_are_normalized() {
        let w = test_world();
        for t in &w.tiles {
            for s in t.samples.iter().step_by(97) {
                let sum = s.weights.forest + s.weights.peat + s.weights.shore;
                assert!((sum - 1.0).abs() < 1e-4, "{sum}");
            }
        }
    }

    #[test]
    fn route_has_trail_river_and_sea() {
        let w = test_world();
        let g = &w.generator;
        let start = w.marker(MarkerKind::TrailStart).unwrap().position;
        let on_trail = w.sample_ground(start.x, start.z).unwrap();
        assert!(
            on_trail.on_trail && !on_trail.wet,
            "начало тропы сухое и на тропе"
        );
        let src = g.river.point_at(0.3);
        assert!(
            w.sample_ground(src.x, src.y).unwrap().wet,
            "середина русла мокрая"
        );
        let sea = w.sample_ground(300.0, 760.0).unwrap();
        assert!(sea.height < 0.0 && sea.wet, "юг карты — море");
        let peat = w.sample_ground(g.basin_center.x, g.basin_center.y).unwrap();
        assert_eq!(peat.substrate, Substrate::Peat);
        assert_eq!(w.markers.len(), 6);
    }
}

#[cfg(test)]
mod dump {
    use super::*;

    /// Карта для глаз: `cargo test dump_map -- --ignored`, затем
    /// `python3 scripts/rgb-to-png.py`. Пишет target/world-map.rgb (768 × 768 × RGB).
    #[test]
    #[ignore]
    fn dump_map() {
        let w = World::generate(0xEC40, 12, 12);
        let n = 768;
        let mut rgb = Vec::with_capacity(n * n * 3);
        for z in 0..n {
            for x in 0..n {
                let s = w.sample_ground(x as f32 + 0.5, z as f32 + 0.5).unwrap();
                let shade = (0.55 + 0.45 * s.normal.dot(Vec3::new(-0.5, 0.8, -0.3).normalize()))
                    .clamp(0.2, 1.0);
                let base: [f32; 3] = if s.height < 0.0 {
                    let d = (-s.height / 7.0).clamp(0.0, 1.0);
                    [0.1, 0.35 - 0.2 * d, 0.6 - 0.25 * d]
                } else if s.wet {
                    [0.2, 0.45, 0.75]
                } else if s.on_trail {
                    [0.85, 0.7, 0.45]
                } else {
                    match s.substrate {
                        Substrate::Soil => [0.25, 0.42, 0.18],
                        Substrate::Peat => [0.38, 0.3, 0.18],
                        Substrate::Sand => [0.85, 0.8, 0.6],
                        Substrate::Rock => [0.55, 0.55, 0.55],
                        Substrate::Gravel => [0.6, 0.55, 0.45],
                    }
                };
                let hl = (s.height / 40.0).clamp(0.0, 1.0) * 0.25;
                for c in base {
                    rgb.push(((c + hl) * shade * 255.0).clamp(0.0, 255.0) as u8);
                }
            }
        }
        for m in &w.markers {
            for dz in -4i32..=4 {
                for dx in -4i32..=4 {
                    let (x, z) = (m.position.x as i32 + dx, m.position.z as i32 + dz);
                    if (0..n as i32).contains(&x) && (0..n as i32).contains(&z) {
                        let i = (z as usize * n + x as usize) * 3;
                        rgb[i..i + 3].copy_from_slice(&[230, 40, 40]);
                    }
                }
            }
        }
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/target/world-map.rgb"),
            rgb,
        )
        .unwrap();
        for m in &w.markers {
            println!("{:<18} {:?} {:.1?}", m.name, m.kind, m.position);
        }
    }
}
