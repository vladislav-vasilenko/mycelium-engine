//! Генератор рельефа и полей среды первой карты.
//!
//! Мировая функция `sample(x, z)` даёт высоту и поля в любой точке. Тайлы
//! запекаются из неё на общей мировой сетке, поэтому граничные точки соседних
//! тайлов совпадают по построению. Авторская часть — сплайны реки и тропы,
//! чаша торфяника и береговая линия; остальное — шум от `seed`.

use glam::Vec2;

use super::noise::{fbm, lerp, smoothstep};
use super::{BiomeWeights, Substrate};

/// Уровень моря, м.
pub const SEA_LEVEL: f32 = 0.0;

/// Радиус, в котором `Path::nearest` находит ось. Дальше ответ — бесконечность:
/// все влияния сплайнов на рельеф и поля заканчиваются ближе.
pub const PATH_QUERY_RADIUS: f32 = 60.0;
const GRID_CELL: f32 = 16.0;

/// Ломаная, уплотнённая сплайном Катмулла — Рома, с сеткой ускорения:
/// для каждой ячейки 16 × 16 м — сегменты в пределах `PATH_QUERY_RADIUS`.
#[derive(Clone, Debug)]
pub struct Path {
    pub points: Vec<Vec2>,
    /// Накопленная длина до каждой точки, м.
    pub lengths: Vec<f32>,
    grid_min: Vec2,
    grid_w: i32,
    grid_h: i32,
    cells: Vec<Vec<u32>>,
}

impl Path {
    pub fn catmull_rom(control: &[Vec2], per_segment: usize) -> Path {
        let n = control.len();
        let at = |i: isize| control[i.clamp(0, n as isize - 1) as usize];
        let mut points = Vec::new();
        for i in 0..n - 1 {
            let (p0, p1, p2, p3) = (
                at(i as isize - 1),
                at(i as isize),
                at(i as isize + 1),
                at(i as isize + 2),
            );
            for s in 0..per_segment {
                let t = s as f32 / per_segment as f32;
                let (t2, t3) = (t * t, t * t * t);
                points.push(
                    0.5 * ((2.0 * p1)
                        + (p2 - p0) * t
                        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3),
                );
            }
        }
        points.push(control[n - 1]);
        let mut lengths = vec![0.0];
        for w in points.windows(2) {
            lengths.push(lengths.last().unwrap() + w[0].distance(w[1]));
        }
        let (mut lo, mut hi) = (points[0], points[0]);
        for p in &points {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let grid_min = lo - Vec2::splat(PATH_QUERY_RADIUS);
        let extent = hi + Vec2::splat(PATH_QUERY_RADIUS) - grid_min;
        let grid_w = (extent.x / GRID_CELL).ceil() as i32 + 1;
        let grid_h = (extent.y / GRID_CELL).ceil() as i32 + 1;
        let mut cells = vec![Vec::new(); (grid_w * grid_h) as usize];
        // Сегмент попадает в ячейку, если его AABB, расширенный на радиус
        // запроса и диагональ ячейки, пересекает ячейку.
        let pad = PATH_QUERY_RADIUS + GRID_CELL;
        for (k, w) in points.windows(2).enumerate() {
            let (a, b) = (
                w[0].min(w[1]) - Vec2::splat(pad),
                w[0].max(w[1]) + Vec2::splat(pad),
            );
            let (x0, z0) = (
                ((a.x - grid_min.x) / GRID_CELL).floor() as i32,
                ((a.y - grid_min.y) / GRID_CELL).floor() as i32,
            );
            let (x1, z1) = (
                ((b.x - grid_min.x) / GRID_CELL).floor() as i32,
                ((b.y - grid_min.y) / GRID_CELL).floor() as i32,
            );
            for cz in z0.max(0)..=z1.min(grid_h - 1) {
                for cx in x0.max(0)..=x1.min(grid_w - 1) {
                    cells[(cz * grid_w + cx) as usize].push(k as u32);
                }
            }
        }
        Path {
            points,
            lengths,
            grid_min,
            grid_w,
            grid_h,
            cells,
        }
    }

    pub fn length(&self) -> f32 {
        *self.lengths.last().unwrap()
    }

    /// Ближайшая точка: (расстояние, параметр t ∈ [0, 1], точка на оси).
    /// Дальше `PATH_QUERY_RADIUS` — `(INFINITY, 0, p)`.
    pub fn nearest(&self, p: Vec2) -> (f32, f32, Vec2) {
        let mut best = (f32::INFINITY, 0.0, p);
        let c = ((p - self.grid_min) / GRID_CELL).floor();
        let (cx, cz) = (c.x as i32, c.y as i32);
        if cx < 0 || cz < 0 || cx >= self.grid_w || cz >= self.grid_h {
            return best;
        }
        for &k in &self.cells[(cz * self.grid_w + cx) as usize] {
            let i = k as usize;
            let (a, b) = (self.points[i], self.points[i + 1]);
            let ab = b - a;
            let s = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
            let q = a + ab * s;
            let d = p.distance(q);
            if d < best.0 && d <= PATH_QUERY_RADIUS {
                let len = self.lengths[i] + ab.length() * s;
                best = (d, len / self.length(), q);
            }
        }
        best
    }

    /// Точка на оси по параметру t ∈ [0, 1].
    pub fn point_at(&self, t: f32) -> Vec2 {
        let target = t.clamp(0.0, 1.0) * self.length();
        let i = self
            .lengths
            .partition_point(|&l| l < target)
            .clamp(1, self.points.len() - 1);
        let (l0, l1) = (self.lengths[i - 1], self.lengths[i]);
        let s = if l1 > l0 {
            (target - l0) / (l1 - l0)
        } else {
            0.0
        };
        self.points[i - 1].lerp(self.points[i], s)
    }
}

/// Поля в одной точке мира.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fields {
    pub height: f32,
    /// Влажность почвы, 0..1.
    pub moisture: f32,
    /// Температура воздуха у земли, °C.
    pub temperature: f32,
    /// Маска тропы, 1 на оси.
    pub trail: f32,
    /// Маска речного русла, 1 в русле.
    pub river: f32,
    /// Близость берега моря, 1 у кромки воды.
    pub coast: f32,
    /// Вес чаши торфяника.
    pub basin: f32,
}

#[derive(Clone, Debug)]
pub struct Generator {
    pub seed: u64,
    pub size_m: f32,
    pub river: Path,
    pub trail: Path,
    pub basin_center: Vec2,
    pub basin_radius: f32,
    /// Средняя линия берега по z, м.
    pub shore_z: f32,
}

/// Половина ширины русла по параметру t от истока (0) к устью (1).
pub fn river_half_width(t: f32) -> f32 {
    lerp(3.0, 9.0, t)
}

impl Generator {
    pub fn first_map(seed: u64, size_m: f32) -> Generator {
        let v = |x: f32, z: f32| Vec2::new(x, z);
        // Река с северо-запада к морю на юге; проходит западнее торфяника.
        let river = Path::catmull_rom(
            &[
                v(150.0, -10.0),
                v(185.0, 110.0),
                v(240.0, 215.0),
                v(270.0, 330.0),
                v(300.0, 450.0),
                v(380.0, 545.0),
                v(470.0, 625.0),
                v(520.0, 700.0),
                v(545.0, 790.0),
            ],
            24,
        );
        // Тропа: лес на северо-западе → торфяник → мост через реку → каменистый берег.
        let trail = Path::catmull_rom(
            &[
                v(95.0, 120.0),
                v(150.0, 175.0),
                v(215.0, 250.0),
                v(300.0, 300.0),
                v(390.0, 360.0),
                v(440.0, 440.0),
                v(430.0, 540.0),
                v(470.0, 600.0),
                v(560.0, 640.0),
                v(640.0, 665.0),
            ],
            24,
        );
        Generator {
            seed,
            size_m,
            river,
            trail,
            basin_center: v(395.0, 390.0),
            basin_radius: 115.0,
            shore_z: 700.0,
        }
    }

    fn seed_of(&self, salt: u64) -> u64 {
        super::noise::mix64(self.seed ^ salt)
    }

    /// Береговая линия: z кромки воды для данного x.
    pub fn shoreline_z(&self, x: f32) -> f32 {
        self.shore_z + 22.0 * fbm(self.seed_of(11), x / 90.0, 0.5, 3)
    }

    /// Рельеф без тропы: основа, торфяник, море, русло.
    fn terrain_without_trail(&self, p: Vec2) -> (f32, f32, f32, f32) {
        let (x, z) = (p.x, p.y);
        let north = (1.0 - (z / self.size_m).clamp(0.0, 1.0)).powf(1.3);
        let mut h = 9.0
            + 24.0 * north
            + 10.0 * fbm(self.seed_of(1), x / 170.0, z / 170.0, 5)
            + 2.5 * fbm(self.seed_of(2), x / 35.0, z / 35.0, 3);

        // Каменистые гряды у берега к востоку от устья.
        let ridge = 1.0 - fbm(self.seed_of(3), x / 28.0, z / 28.0, 3).abs();
        let east = smoothstep(540.0, 620.0, x);
        let near_shore = 1.0 - smoothstep(0.0, 80.0, (z - self.shoreline_z(x)).abs());
        h += 6.0 * ridge * ridge * east * near_shore;

        // Чаша торфяника: плоская низина с мелкими кочками.
        let noisy_r =
            self.basin_radius * (1.0 + 0.18 * fbm(self.seed_of(4), x / 60.0, z / 60.0, 2));
        let basin = 1.0 - smoothstep(noisy_r * 0.55, noisy_r, p.distance(self.basin_center));
        h = lerp(
            h,
            7.5 + 0.5 * fbm(self.seed_of(5), x / 12.0, z / 12.0, 2),
            basin,
        );

        // Море: за береговой линией дно уходит до −7 м.
        let shore = self.shoreline_z(x);
        let coast = 1.0 - smoothstep(0.0, 45.0, (shore - z).max(0.0));
        let sea = smoothstep(shore - 25.0, shore + 45.0, z);
        h = lerp(h, -7.0, sea);

        // Русло: дно ниже берегов и не выше профиля от истока к устью.
        let (d, t, _) = self.river.nearest(p);
        let half = river_half_width(t);
        let profile = lerp(17.0, -2.5, t);
        let banks = h;
        let bed = profile.min(banks - 1.8);
        let channel = 1.0 - smoothstep(half * 0.6, half * 1.9, d);
        h = lerp(h, bed, channel);
        let river = 1.0 - smoothstep(half * 0.9, half * 1.2, d);

        (h, river, coast, basin)
    }

    /// Высота и поля в мировой точке.
    pub fn sample(&self, x: f32, z: f32) -> Fields {
        let p = Vec2::new(x, z);
        let (mut h, river, coast, basin) = self.terrain_without_trail(p);

        // Тропа: поперечный профиль выравнивается к высоте оси, у реки — мост.
        let (dt, _, axis) = self.trail.nearest(p);
        let (dr, tr, _) = self.river.nearest(axis);
        let bridge = smoothstep(river_half_width(tr) + 1.0, river_half_width(tr) + 5.0, dr);
        let trail = (1.0 - smoothstep(1.1, 2.6, dt)) * bridge;
        if trail > 0.0 {
            let (axis_h, ..) = self.terrain_without_trail(axis);
            h = lerp(h, axis_h, 0.85 * trail);
        }

        let river_d = self.river.nearest(p).0;
        let moisture = (0.45
            + 0.22 * fbm(self.seed_of(6), x / 90.0, z / 90.0, 3)
            + 0.45 * basin
            + 0.35 * (1.0 - smoothstep(0.0, 25.0, river_d))
            + 0.15 * coast
            - 0.012 * (h - 10.0).max(0.0))
        .clamp(0.0, 1.0);
        let temperature =
            15.0 - 0.0065 * h.max(0.0) + 1.5 * fbm(self.seed_of(7), x / 200.0, z / 200.0, 2);

        Fields {
            height: h,
            moisture,
            temperature,
            trail,
            river,
            coast,
            basin,
        }
    }
}

/// Веса биомов из полей, наклона (1 − ny) и авторских масок. Сумма равна 1.
pub fn biome_weights(f: &Fields, slope: f32) -> BiomeWeights {
    let shore = (f.coast + 0.4 * f.river).clamp(0.0, 1.0) + 0.3 * slope;
    let peat = f.basin * (0.4 + f.moisture) * (1.0 - (slope * 4.0).min(1.0));
    let forest = 0.35 + 0.6 * (1.0 - (f.moisture - 0.55).abs() * 2.0).max(0.0)
        - 0.8 * f.coast
        - 0.5 * f.basin;
    let raw = [forest.max(0.0), peat.max(0.0), shore.max(0.0)].map(|v| v * v);
    let sum: f32 = raw.iter().sum::<f32>().max(1e-6);
    BiomeWeights {
        forest: raw[0] / sum,
        peat: raw[1] / sum,
        shore: raw[2] / sum,
    }
}

/// Субстрат по полям, наклону и весам.
pub fn substrate(f: &Fields, slope: f32, w: &BiomeWeights, x: f32) -> Substrate {
    if f.height < SEA_LEVEL + 0.4 && f.coast > 0.5 {
        Substrate::Sand
    } else if slope > 0.35 {
        Substrate::Rock
    } else if f.river > 0.5 {
        Substrate::Gravel
    } else if w.shore > 0.5 {
        if x > 540.0 {
            Substrate::Rock
        } else {
            Substrate::Sand
        }
    } else if w.peat > 0.5 {
        Substrate::Peat
    } else {
        Substrate::Soil
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_nearest_matches_brute_force_within_radius() {
        let g = Generator::first_map(1, 768.0);
        for k in 0..500 {
            let p = Vec2::new((k * 37 % 768) as f32 + 0.5, (k * 91 % 768) as f32 + 0.25);
            let (d, _, _) = g.river.nearest(p);
            let brute = g
                .river
                .points
                .windows(2)
                .map(|w| {
                    let ab = w[1] - w[0];
                    let s = ((p - w[0]).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
                    p.distance(w[0] + ab * s)
                })
                .fold(f32::INFINITY, f32::min);
            if brute <= PATH_QUERY_RADIUS {
                assert!((d - brute).abs() < 1e-3, "p={p} grid={d} brute={brute}");
            } else {
                assert!(d.is_infinite() || d <= PATH_QUERY_RADIUS);
            }
        }
    }
}
