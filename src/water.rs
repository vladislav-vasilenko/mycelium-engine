//! Море и прибой (подэтап 03.1): состояние моря, поле берега, функция волн
//! и CPU-запрос `sample_water`.
//!
//! Функция волн повторяет `shaders/metal/water.h` построчно: открытое море —
//! волны Герстнера по ветру, у берега — береговая волна, гребни которой идут
//! по линиям равного расстояния до кромки (фронт параллелен берегу), с
//! обмелением, обрушением и заплеском на пляж. Самопроверка сравнивает CPU и
//! GPU в контрольных точках.

use glam::{Vec2, Vec3};

use crate::layout::{GerstnerWave, SEA_WAVES, SeaParams, WATER_PROBES, WaterLevel};
use crate::world::World;
use crate::world::noise::{hash01, smoothstep};

const G: f32 = 9.81;
const PI: f32 = std::f32::consts::PI;

/// Уровней кольцевой сетки воды: шаг 0,5 м × 2^L, последний уровень ~32 км.
pub const WATER_LEVELS: usize = 11;
/// Ячеек на сторону уровня.
pub const WATER_GRID_N: u32 = 64;
/// Шаг самого мелкого уровня, м.
pub const WATER_CELL0_M: f32 = 0.5;
/// Время высыхания мокрого песка, с.
pub const DRY_TIME_S: f32 = 20.0;

/// Состояние моря: уровень, ветер, волны. Детерминировано от seed.
#[derive(Clone, Debug)]
pub struct SeaState {
    pub params: SeaParams,
}

impl SeaState {
    pub fn new(seed: u64) -> SeaState {
        // Ветер с юго-востока, волны в секторе ±0,6 рад от него.
        let wind = Vec2::new(-0.55, -0.83).normalize();
        let specs: [(f32, f32); 6] = [
            (38.0, 0.30),
            (24.0, 0.20),
            (15.0, 0.12),
            (9.5, 0.07),
            (6.0, 0.045),
            (3.8, 0.025),
        ];
        let n = specs.len();
        let mut waves = [GerstnerWave::default(); SEA_WAVES];
        for (i, &(wavelength, amplitude)) in specs.iter().enumerate() {
            let spread = (hash01(seed ^ 0x5EA, i as i32, 1) - 0.5) * 1.2;
            let (s, c) = spread.sin_cos();
            let dir = Vec2::new(wind.x * c - wind.y * s, wind.x * s + wind.y * c);
            let k = 2.0 * PI / wavelength;
            waves[i] = GerstnerWave {
                dir: dir.to_array(),
                wavelength,
                amplitude,
                // Сумма Q·k·A ≤ 0,8: гребни острые, но без петель.
                steepness: 0.8 / (k * amplitude * n as f32),
                phase: hash01(seed ^ 0x5EA, i as i32, 2) * 2.0 * PI,
                omega: (G * k).sqrt(),
                _pad: 0.0,
            };
        }
        SeaState {
            params: SeaParams {
                sea_level: crate::world::generator::SEA_LEVEL,
                wave_count: n as u32,
                shore_wavelength: 30.0,
                shore_amplitude: 0.28,
                shore_omega: 2.0 * PI / 7.0,
                swash_sand_m: 0.7,
                swash_rock_m: 0.25,
                dry_time_s: DRY_TIME_S,
                field_size: 0,
                field_step_m: 1.0,
                dt: 0.0,
                _pad: 0.0,
                waves,
                probe: [[0.0; 4]; WATER_PROBES],
            },
        }
    }

    /// Параметры с размером и шагом поля берега.
    pub fn params_for(&self, field: &ShoreField) -> SeaParams {
        let mut p = self.params;
        p.field_size = field.size as u32;
        p.field_step_m = field.step_m;
        p
    }
}

/// Поле берега на мировой сетке: (высота дна или грунта, знаковое расстояние
/// до кромки, тип берега, 0). То же поле читает GPU (RGBA32Float).
#[derive(Clone, Debug)]
pub struct ShoreField {
    pub size: usize,
    pub step_m: f32,
    pub data: Vec<[f32; 4]>,
}

impl ShoreField {
    pub fn from_world(world: &World) -> ShoreField {
        let size = world.tiles_x as usize * (crate::world::TILE_SAMPLES - 1) + 1;
        let g = &world.generator;
        let mut data = vec![[0.0f32; 4]; size * size];
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let rows = size.div_ceil(threads);
        std::thread::scope(|s| {
            for (ci, chunk) in data.chunks_mut(rows * size).enumerate() {
                let fields = &world.fields;
                s.spawn(move || {
                    for (k, v) in chunk.iter_mut().enumerate() {
                        let j = ci * rows + k / size;
                        let i = k % size;
                        let f = &fields[j * size + i];
                        let p = Vec2::new(i as f32, j as f32) * crate::world::SAMPLE_STEP_M;
                        *v = [f.height, g.coast_distance(p), f.rocky, 0.0];
                    }
                });
            }
        });
        ShoreField {
            size,
            step_m: crate::world::SAMPLE_STEP_M,
            data,
        }
    }

    fn bilinear(&self, p: Vec2) -> [f32; 4] {
        let n = (self.size - 1) as f32;
        let g = (p / self.step_m).clamp(Vec2::ZERO, Vec2::splat(n));
        let g0 = g.floor().min(Vec2::splat(n - 1.0));
        let f = g - g0;
        let (i, j) = (g0.x as usize, g0.y as usize);
        let at = |di: usize, dj: usize| self.data[(j + dj) * self.size + i + di];
        let (a, b, c, d) = (at(0, 0), at(1, 0), at(0, 1), at(1, 1));
        std::array::from_fn(|k| {
            let ab = a[k] + (b[k] - a[k]) * f.x;
            let cd = c[k] + (d[k] - c[k]) * f.x;
            ab + (cd - ab) * f.y
        })
    }

    fn shore_at(&self, p: Vec2) -> ShoreSample {
        let v = self.bilinear(p);
        let e = 2.0 * self.step_m;
        let gx = self.bilinear(p + Vec2::new(e, 0.0))[1] - self.bilinear(p - Vec2::new(e, 0.0))[1];
        let gz = self.bilinear(p + Vec2::new(0.0, e))[1] - self.bilinear(p - Vec2::new(0.0, e))[1];
        let grad = Vec2::new(gx, gz);
        let len = grad.length();
        ShoreSample {
            bed: v[0],
            coast_d: v[1],
            rocky: v[2],
            dir: if len > 1e-6 { grad / len } else { Vec2::Y },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ShoreSample {
    pub bed: f32,
    pub coast_d: f32,
    pub rocky: f32,
    /// Направление к берегу, единичное.
    pub dir: Vec2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveOut {
    pub pos: Vec3,
    pub foam: f32,
    pub swash: f32,
    pub shore: f32,
}

fn alongshore_phase(p: Vec2) -> f32 {
    1.7 * (p.x * 0.013).sin() + 1.9 * (p.y * 0.011 + 1.3).sin()
}

fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// Точка поверхности для узла `base` в момент `t` (`water.h`, `wave_at`).
pub fn wave_at(
    field: &ShoreField,
    sp: &SeaParams,
    base: Vec2,
    t: f32,
    filter_cell: f32,
) -> WaveOut {
    let s = field.shore_at(base);
    let depth0 = sp.sea_level - s.bed;
    let mut disp = Vec3::ZERO;

    let w_deep = smoothstep(4.0, 14.0, depth0);
    for w in &sp.waves[..sp.wave_count as usize] {
        let filt = if filter_cell > 0.0 {
            smoothstep(4.0 * filter_cell, 8.0 * filter_cell, w.wavelength)
        } else {
            1.0
        };
        let k = 2.0 * PI / w.wavelength;
        let dir = Vec2::from(w.dir);
        let th = k * dir.dot(base) - w.omega * t + w.phase;
        let c = th.cos();
        let a = w.amplitude * w_deep * filt;
        disp.x += dir.x * w.steepness * a * c;
        disp.z += dir.y * w.steepness * a * c;
        disp.y += a * th.sin();
    }

    let u = (-s.coast_d).max(0.0);
    let ks = 2.0 * PI / sp.shore_wavelength;
    let th_s = ks * (u + 8.0 * (1.0 + u / 8.0).ln()) + sp.shore_omega * t + alongshore_phase(base);
    let d0 = depth0.max(0.0);
    let mut amp = sp.shore_amplitude * (1.0 + 0.8 * (1.0 - smoothstep(1.0, 10.0, depth0)));
    amp = amp.min(0.39 * d0 + 0.02);
    let w_shore = (1.0 - w_deep) * smoothstep(-0.2, 0.6, depth0);
    let sn = th_s.sin();
    disp.x += s.dir.x * 0.6 * amp * th_s.cos() * w_shore;
    disp.z += s.dir.y * 0.6 * amp * th_s.cos() * w_shore;
    disp.y += amp * sn * w_shore;
    let breaking =
        smoothstep(0.5, 0.78, 2.0 * amp / d0.max(0.05)) * smoothstep(0.3, 0.9, sn) * w_shore;
    let splash = s.rocky
        * (1.0 - smoothstep(0.0, 6.0, depth0))
        * smoothstep(0.2, 1.0, sn)
        * smoothstep(-0.2, 0.3, depth0);

    let phase = fract((sp.shore_omega * t + alongshore_phase(base) - 0.5 * PI) / (2.0 * PI));
    let profile = if phase < 0.2 {
        smoothstep(0.0, 0.2, phase)
    } else {
        1.0 - smoothstep(0.2, 1.0, phase)
    };
    let run_up = (sp.swash_sand_m + (sp.swash_rock_m - sp.swash_sand_m) * s.rocky) * profile;
    let w_swash = (1.0 - smoothstep(0.3, 1.5, depth0)) * (1.0 - smoothstep(10.0, 40.0, s.coast_d));

    let mut y = sp.sea_level + disp.y;
    y += (y.max(sp.sea_level + run_up) - y) * w_swash;
    WaveOut {
        pos: Vec3::new(base.x + disp.x, y, base.y + disp.z),
        foam: (breaking + splash).clamp(0.0, 1.0),
        swash: w_swash,
        shore: w_shore,
    }
}

/// Результат запроса к воде.
#[derive(Clone, Copy, Debug)]
pub struct WaterSample {
    pub height: f32,
    /// Глубина до дна или грунта, м (≤ 0 — сухо).
    pub depth: f32,
    pub dry: bool,
}

/// Высота воды над мировой точкой `p` в момент `t`. Горизонтальное смещение
/// волн обращается итерациями: ищется узел, который волна сдвинула в `p`.
pub fn sample_water(field: &ShoreField, sp: &SeaParams, p: Vec2, t: f32) -> WaterSample {
    let mut base = p;
    let mut w = wave_at(field, sp, base, t, 0.0);
    for _ in 0..4 {
        base += p - Vec2::new(w.pos.x, w.pos.z);
        w = wave_at(field, sp, base, t, 0.0);
    }
    let bed = field.bilinear(p)[0];
    let depth = w.pos.y - bed;
    WaterSample {
        height: w.pos.y,
        depth,
        dry: depth <= 0.0,
    }
}

/// Уровни кольцевой сетки воды вокруг камеры. Каждый уровень привязан к своей
/// мировой решётке шагом 2 × cell, поэтому вершины не «плывут». Внутреннюю
/// часть уровня рисует предыдущий, более мелкий уровень.
pub fn water_levels(camera: Vec2) -> [WaterLevel; WATER_LEVELS] {
    let n = WATER_GRID_N as f32;
    let mut levels = [WaterLevel::default(); WATER_LEVELS];
    let mut prev: Option<(Vec2, Vec2)> = None;
    for (l, level) in levels.iter_mut().enumerate() {
        let cell = WATER_CELL0_M * (1u32 << l) as f32;
        let snap = 2.0 * cell;
        let origin = (camera / snap).floor() * snap - Vec2::splat(0.5 * n * cell);
        let (inner_min, inner_max) = prev.unwrap_or((Vec2::splat(1.0), Vec2::splat(-1.0)));
        *level = WaterLevel {
            origin: origin.to_array(),
            cell,
            morph_edge: (l + 1 < WATER_LEVELS) as u32,
            inner_min: inner_min.to_array(),
            inner_max: inner_max.to_array(),
        };
        prev = Some((origin, origin + Vec2::splat(n * cell)));
    }
    levels
}

/// Индексы сетки уровня (N + 1)² вершин, CCW сверху.
pub fn grid_indices() -> Vec<u16> {
    let n = WATER_GRID_N;
    let mut idx = Vec::with_capacity((n * n * 6) as usize);
    for j in 0..n {
        for i in 0..n {
            let a = (j * (n + 1) + i) as u16;
            let b = a + 1;
            let d = a + (n + 1) as u16;
            let c = d + 1;
            idx.extend_from_slice(&[a, c, b, a, d, c]);
        }
    }
    idx
}

/// Контрольные точки проверки CPU ↔ GPU: открытое море, мелководье, кромка
/// бухты, пляж, каменистый мыс; разные моменты времени.
pub fn probe_points(world: &World) -> [[f32; 4]; WATER_PROBES] {
    let g = &world.generator;
    let mouth = g.mouth;
    let cape = g.cape_center;
    [
        [60.0, 60.0, 3.0, 0.0],
        [512.0, 980.0, 17.25, 0.0],
        [mouth.x - 20.0, mouth.y - 10.0, 5.5, 0.0],
        [mouth.x - 40.0, mouth.y - 25.0, 9.75, 0.0],
        [mouth.x - 60.0, mouth.y - 40.0, 1.0, 0.0],
        [cape.x + 40.0, cape.y + 40.0, 4.2, 0.0],
        [cape.x + 25.0, cape.y + 25.0, 12.6, 0.0],
        [900.0, 300.0, 33.3, 0.0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    fn setup() -> &'static (World, ShoreField, SeaParams) {
        static S: OnceLock<(World, ShoreField, SeaParams)> = OnceLock::new();
        S.get_or_init(|| {
            let world = World::generate(
                0xEC40,
                crate::world::FIRST_MAP_TILES,
                crate::world::FIRST_MAP_TILES,
            );
            let field = ShoreField::from_world(&world);
            let mut sp = SeaState::new(world.seed).params;
            sp.field_size = field.size as u32;
            (world, field, sp)
        })
    }

    /// Точка песчаного пляжа у кромки и направление вглубь суши.
    fn beach(world: &World, field: &ShoreField) -> (Vec2, Vec2) {
        let g = &world.generator;
        let c = g.island_center;
        for k in 0..720 {
            let a = k as f32 / 720.0 * std::f32::consts::TAU;
            let dir = Vec2::new(a.cos(), a.sin()) * g.island_radii;
            let mut t = 0.6;
            while g.coast_distance(c + dir * t) > 0.0 {
                t += 0.002;
            }
            let p = c + dir * t;
            let s = field.shore_at(p);
            if s.rocky < 0.05 {
                return (p, -dir.normalize());
            }
        }
        panic!("песчаный пляж не найден");
    }

    #[test]
    fn sea_state_is_deterministic_and_waves_do_not_loop() {
        let a = SeaState::new(0xEC40).params;
        let b = SeaState::new(0xEC40).params;
        assert_eq!(bytemuck::bytes_of(&a), bytemuck::bytes_of(&b));
        let qka: f32 = a.waves[..a.wave_count as usize]
            .iter()
            .map(|w| w.steepness * 2.0 * PI / w.wavelength * w.amplitude)
            .sum();
        assert!(qka <= 0.81, "сумма Q·k·A = {qka}");
    }

    /// Кромка воды на пляже поднимается и откатывается с периодом наката,
    /// не выше высоты заплеска.
    #[test]
    fn swash_runs_up_and_back_on_sandy_beach() {
        let (world, field, sp) = setup();
        let (shore, inland) = beach(world, field);
        let period = 2.0 * PI / sp.shore_omega;
        // Самая дальняя точка вглубь суши, до которой дошла вода за период.
        let mut reach = Vec::new();
        for k in 0..40 {
            let t = 100.0 + period * k as f32 / 40.0;
            let mut far = 0.0f32;
            for step in 0..60 {
                let d = step as f32 * 0.5;
                let p = shore + inland * d;
                if !sample_water(field, sp, p, t).dry {
                    far = d;
                }
            }
            reach.push(far);
        }
        let (lo, hi) = reach
            .iter()
            .fold((f32::MAX, 0.0f32), |(a, b), &r| (a.min(r), b.max(r)));
        assert!(hi - lo > 2.0, "кромка ходит {lo}…{hi} м");
        for k in 0..20 {
            let t = 50.0 + k as f32 * 0.37;
            for step in 0..120 {
                let p = shore + inland * step as f32 * 0.5;
                let w = sample_water(field, sp, p, t);
                if field.shore_at(p).coast_d > 2.0 {
                    assert!(
                        w.height <= sp.sea_level + sp.swash_sand_m + 0.05,
                        "вода выше заплеска в {p}"
                    );
                }
            }
        }
    }

    /// У кромки работает только береговая волна, её фронт перпендикулярен
    /// градиенту расстояния до берега — то есть параллелен береговой линии.
    #[test]
    fn near_shore_only_shore_waves_parallel_to_coast() {
        let (world, field, sp) = setup();
        let g = &world.generator;
        for k in 0..36 {
            let a = k as f32 / 36.0 * std::f32::consts::TAU;
            let dir = Vec2::new(a.cos(), a.sin()) * g.island_radii;
            let mut t = 0.6;
            while g.coast_distance(g.island_center + dir * t) > -15.0 {
                t += 0.002;
            }
            let p = g.island_center + dir * t;
            let s = field.shore_at(p);
            let depth = sp.sea_level - s.bed;
            if depth < 4.0 {
                let w = wave_at(field, sp, p, 10.0, 0.0);
                assert!(
                    w.shore > 0.99,
                    "в {p} глубина {depth}, вес береговой волны {}",
                    w.shore
                );
                // Направление волны — к берегу: шаг вдоль dir уменьшает расстояние до кромки.
                let ahead = field.shore_at(p + s.dir * 2.0).coast_d;
                assert!(ahead > s.coast_d, "волна в {p} идёт не к берегу");
            }
        }
    }

    #[test]
    fn rocky_coast_swash_is_lower_than_sandy() {
        let (_, _, sp) = setup();
        assert!(sp.swash_rock_m < 0.5 * sp.swash_sand_m);
    }

    #[test]
    fn levels_nest_without_gaps() {
        for cam in [
            Vec2::new(0.3, 0.7),
            Vec2::new(512.4, 497.9),
            Vec2::new(-3000.0, 2000.25),
        ] {
            let levels = water_levels(cam);
            for l in 1..WATER_LEVELS {
                let (a, b) = (&levels[l - 1], &levels[l]);
                let n = WATER_GRID_N as f32;
                let fine_min = Vec2::from(a.origin);
                let fine_max = fine_min + Vec2::splat(n * a.cell);
                let coarse_min = Vec2::from(b.origin);
                let coarse_max = coarse_min + Vec2::splat(n * b.cell);
                assert!(
                    fine_min.cmpge(coarse_min).all() && fine_max.cmple(coarse_max).all(),
                    "уровень {l}"
                );
                assert_eq!(Vec2::from(b.inner_min), fine_min);
                // Граница мелкого уровня лежит на линиях грубой сетки.
                let rel = (fine_min - coarse_min) / b.cell;
                assert!(
                    (rel - rel.round()).abs().max_element() < 1e-4,
                    "уровень {l}: {rel}"
                );
            }
            let far = &levels[WATER_LEVELS - 1];
            assert!(far.cell * WATER_GRID_N as f32 >= 30_000.0);
        }
    }
}
