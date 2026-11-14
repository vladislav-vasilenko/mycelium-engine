//! Мир: тайлы рельефа, поля среды, веса биомов и запрос `sample_ground`.
//!
//! Карта разбита на тайлы 64 × 64 м с целочисленными координатами. LOD 0
//! хранит 65 × 65 высот с шагом 1 м, граничные точки общие с соседями.
//! Высоты, нормали и поля берутся из одной мировой сетки, запечённой
//! генератором, поэтому стыки совпадают точно.
//!
//! Треугольники ячейки: диагональ из (i, j) в (i+1, j+1),
//! треугольники (a, c, b) и (a, d, c) с обходом CCW сверху.

pub mod features;
pub mod generator;
pub mod heightfield;
pub mod layers;
pub mod noise;

use glam::{Vec2, Vec3};

use generator::{Fields, Generator, SEA_LEVEL};

/// Точек на сторону тайла в LOD 0.
pub const TILE_SAMPLES: usize = 65;
/// Шаг сетки мира генератора, м. Мир из слоёв задаёт свой шаг (`World::step_m`),
/// размер тайла — 64 шага.
pub const SAMPLE_STEP_M: f32 = 1.0;
/// Версия алгоритма генерации. Меняется вместе с результатом генератора.
pub const GENERATOR_VERSION: u32 = 4;
/// Первая карта: 16 × 16 тайлов, 1024 × 1024 м.
pub const FIRST_MAP_TILES: i32 = 16;

/// Стабильный идентификатор тайла: целые координаты в сетке тайлов.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub x: i32,
    pub z: i32,
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
    /// Тип берега: 0 — песок, 1 — камень.
    pub rocky: f32,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MarkerKind {
    TrailStart,
    Viewpoint,
    BridgeSite,
    RiverMouth,
    ShoreEnd,
}

/// Авторская точка маршрута.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub name: String,
    pub kind: MarkerKind,
    pub position: Vec3,
}

/// Поля среды на общей мировой сетке `nx × nz` с шагом `step_m`,
/// строка за строкой по z. `nx = tiles_x · 64 + 1`.
#[derive(Clone, Debug)]
pub struct FieldGrid {
    pub nx: usize,
    pub nz: usize,
    pub step_m: f32,
    pub fields: Vec<Fields>,
}

impl FieldGrid {
    /// Поля генератора. Строки считаются параллельно, результат от числа
    /// потоков не зависит.
    pub fn generate(generator: &Generator, tiles_x: i32, tiles_z: i32) -> FieldGrid {
        let nx = tiles_x as usize * (TILE_SAMPLES - 1) + 1;
        let nz = tiles_z as usize * (TILE_SAMPLES - 1) + 1;
        let mut fields = vec![Fields::default(); nx * nz];
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let rows_per = nz.div_ceil(threads);
        std::thread::scope(|s| {
            for (chunk_i, chunk) in fields.chunks_mut(rows_per * nx).enumerate() {
                s.spawn(move || {
                    for (k, f) in chunk.iter_mut().enumerate() {
                        let j = chunk_i * rows_per + k / nx;
                        let i = k % nx;
                        *f = generator.sample(i as f32 * SAMPLE_STEP_M, j as f32 * SAMPLE_STEP_M);
                    }
                });
            }
        });
        FieldGrid {
            nx,
            nz,
            step_m: SAMPLE_STEP_M,
            fields,
        }
    }
}

/// Ручные слои, заменяющие вычисленные значения. Индекс — точка мировой сетки.
#[derive(Clone, Debug, Default)]
pub struct DerivedOverrides {
    pub weights: Option<Vec<BiomeWeights>>,
    pub substrate: Option<Vec<Substrate>>,
}

pub struct World {
    pub seed: u64,
    pub tiles_x: i32,
    pub tiles_z: i32,
    /// Шаг сетки, м: 1 у генератора, у мира из слоёв — из `world.json`.
    pub step_m: f32,
    /// Привязка к Земле у мира из реальных данных; `None` у генератора.
    pub geo: Option<layers::GeoRef>,
    pub generator: Generator,
    /// Тайлы по строкам: индекс `z * tiles_x + x`.
    pub tiles: Vec<Tile>,
    pub markers: Vec<Marker>,
    /// Поля мировой сетки, из которых собраны тайлы. Нужны экспорту слоёв:
    /// маски торфяника и берега есть только здесь, а не в `Sample`.
    pub fields: Vec<Fields>,
    /// Поле высот полного разрешения: кластеры рельефа и ходьба (02.3). У
    /// генератора — та же сетка, что `fields`.
    pub heightfield: heightfield::HeightField,
    /// Тропы, дороги и реки уже шага сетки (`features.json`), если есть.
    pub features: Option<features::FeatureSet>,
    /// Пояса растительности (`cover.png`), если есть (02.7).
    pub cover: Option<CoverLayer>,
}

/// Пояса растительности на квадратной сетке на всю карту: RGBA, сумма 255 на
/// суше (низинный лес, кедровый лес, якудзаса и луга, гранит и застройка).
#[derive(Clone, Debug)]
pub struct CoverLayer {
    pub size: u32,
    pub rgba: Vec<u8>,
}

impl CoverLayer {
    /// Веса поясов в ближайшем узле (0…1); за картой — нули.
    pub fn at(&self, x: f32, z: f32, map_size_m: f32) -> [f32; 4] {
        let n = self.size as f32;
        let i = (x / map_size_m * (n - 1.0)).round();
        let j = (z / map_size_m * (n - 1.0)).round();
        if !(0.0..n).contains(&i) || !(0.0..n).contains(&j) {
            return [0.0; 4];
        }
        let k = (j as usize * self.size as usize + i as usize) * 4;
        std::array::from_fn(|c| self.rgba[k + c] as f32 / 255.0)
    }
}

impl World {
    /// Сторона тайла, м: 64 шага сетки.
    pub fn tile_size_m(&self) -> f32 {
        (TILE_SAMPLES - 1) as f32 * self.step_m
    }

    /// Угол тайла в мировых метрах.
    pub fn tile_origin(&self, id: TileId) -> Vec2 {
        Vec2::new(id.x as f32, id.z as f32) * self.tile_size_m()
    }

    pub fn size_m(&self) -> Vec2 {
        Vec2::new(self.tiles_x as f32, self.tiles_z as f32) * self.tile_size_m()
    }

    pub fn tile(&self, id: TileId) -> Option<&Tile> {
        if id.x < 0 || id.z < 0 || id.x >= self.tiles_x || id.z >= self.tiles_z {
            return None;
        }
        self.tiles.get((id.z * self.tiles_x + id.x) as usize)
    }

    /// Генерирует и запекает карту: поля генератора, затем сборка тайлов.
    pub fn generate(seed: u64, tiles_x: i32, tiles_z: i32) -> World {
        let generator = Generator::first_map(seed);
        let grid = FieldGrid::generate(&generator, tiles_x, tiles_z);
        World::from_grid(seed, generator, grid, &DerivedOverrides::default(), None)
    }

    /// Собирает мир из полей на мировой сетке: нормали, веса биомов, субстрат,
    /// нарезка на тайлы с общими граничными точками. `overrides` заменяет
    /// вычисленные веса и субстрат ручными слоями. Без `markers` маркеры
    /// расставляются по сплайнам генератора, с ними — получают высоту грунта.
    pub fn from_grid(
        seed: u64,
        generator: Generator,
        grid: FieldGrid,
        overrides: &DerivedOverrides,
        markers: Option<Vec<Marker>>,
    ) -> World {
        let (nx, nz) = (grid.nx, grid.nz);
        let tiles_x = ((nx - 1) / (TILE_SAMPLES - 1)) as i32;
        let tiles_z = ((nz - 1) / (TILE_SAMPLES - 1)) as i32;
        let fields = &grid.fields;

        // Нормали центральными разностями по той же сетке (у края карты — односторонние).
        let h = |i: isize, j: isize| {
            let i = i.clamp(0, nx as isize - 1) as usize;
            let j = j.clamp(0, nz as isize - 1) as usize;
            fields[j * nx + i].height
        };
        let normal_at = |i: usize, j: usize| {
            let (i, j) = (i as isize, j as isize);
            let dx = (h(i + 1, j) - h(i - 1, j)) / (2.0 * grid.step_m);
            let dz = (h(i, j + 1) - h(i, j - 1)) / (2.0 * grid.step_m);
            Vec3::new(-dx, 1.0, -dz).normalize()
        };

        let mut tiles = Vec::with_capacity((tiles_x * tiles_z) as usize);
        for tz in 0..tiles_z {
            for tx in 0..tiles_x {
                let mut samples = Vec::with_capacity(TILE_SAMPLES * TILE_SAMPLES);
                for j in 0..TILE_SAMPLES {
                    for i in 0..TILE_SAMPLES {
                        let gi = tx as usize * (TILE_SAMPLES - 1) + i;
                        let gj = tz as usize * (TILE_SAMPLES - 1) + j;
                        let k = gj * nx + gi;
                        let f = fields[k];
                        let n = normal_at(gi, gj);
                        let slope = 1.0 - n.y;
                        let weights = overrides
                            .weights
                            .as_ref()
                            .map_or_else(|| generator::biome_weights(&f, slope), |w| w[k]);
                        let substrate = overrides
                            .substrate
                            .as_ref()
                            .map_or_else(|| generator::substrate(&f, slope, &weights), |s| s[k]);
                        samples.push(Sample {
                            height: f.height,
                            normal: n,
                            moisture: f.moisture,
                            temperature: f.temperature,
                            substrate,
                            weights,
                            trail: f.trail,
                            river: f.river,
                            rocky: f.rocky,
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
            step_m: grid.step_m,
            geo: None,
            generator,
            tiles,
            markers: Vec::new(),
            heightfield: heightfield::HeightField::new(
                nx,
                nz,
                grid.step_m,
                grid.fields.iter().map(|f| f.height).collect(),
            ),
            fields: grid.fields,
            features: None,
            cover: None,
        };
        world.markers = match markers {
            Some(mut m) => {
                for marker in &mut m {
                    let p = marker.position;
                    marker.position.y = world.sample_ground(p.x, p.z).map_or(0.0, |g| g.height);
                }
                m
            }
            None => world.place_markers(),
        };
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
        let mouth = g.mouth;
        vec![
            Marker {
                name: "начало тропы".into(),
                kind: MarkerKind::TrailStart,
                position: on_ground(g.trail.point_at(0.0)),
            },
            Marker {
                name: "склон над лесом".into(),
                kind: MarkerKind::Viewpoint,
                position: on_ground(g.trail.point_at(0.07)),
            },
            Marker {
                name: "край торфяника".into(),
                kind: MarkerKind::Viewpoint,
                position: on_ground(g.trail.point_at(0.45)),
            },
            Marker {
                name: "место моста".into(),
                kind: MarkerKind::BridgeSite,
                position: on_ground(bridge),
            },
            Marker {
                name: "устье".into(),
                kind: MarkerKind::RiverMouth,
                position: on_ground(mouth),
            },
            Marker {
                name: "каменистый берег".into(),
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
            x: ((x / self.tile_size_m()) as i32).min(self.tiles_x - 1),
            z: ((z / self.tile_size_m()) as i32).min(self.tiles_z - 1),
        };
        let tile = self.tile(id)?;
        let local = (Vec2::new(x, z) - self.tile_origin(id)) / self.step_m;
        // Высота и нормаль — по треугольникам поля высот полного разрешения:
        // это уровень 0 кластеров, видимый вблизи.
        let (height, normal) = self.heightfield.sample(x, z)?;
        // Нечисловые свойства — от ближайшей точки сетки.
        let near = tile.at(
            (local.x.round() as usize).min(TILE_SAMPLES - 1),
            (local.y.round() as usize).min(TILE_SAMPLES - 1),
        );
        // Слой объектов точнее сетки: тропа и русло — по расстоянию до них.
        let (on_trail, in_river) = match &self.features {
            Some(f) => {
                let s = f.sample(x, z);
                (s.trail_m < 0.0 || s.road_m < 0.0, s.river_m < 0.0)
            }
            None => (near.trail > 0.5, near.river > 0.5),
        };
        Some(GroundSample {
            height,
            normal,
            substrate: near.substrate,
            on_trail,
            wet: in_river || height < SEA_LEVEL,
            tile: id,
        })
    }
}

#[cfg(test)]
mod tests;
