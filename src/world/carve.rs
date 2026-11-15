//! Тропы и дороги, врезанные в рельеф (02.7, 02.10): поперёк полотна склон
//! убирается, вдоль — остаётся, но сглаженным. Отрезки собираются в полилинии;
//! продольный профиль — высота исходного рельефа через 1 м вдоль линии,
//! сглаженная окном `SMOOTH_*` (шум DEM и уступы не попадают на полотно).
//! Узел у линии тянется к высоте профиля в ближайшей точке ближайшей осевой
//! линии: в пределах полуширины полностью, на бровке — плавно. Так полотно —
//! ровная полка без ступеней на стыках отрезков, а ходьба, кластеры и шейдер
//! видят одну и ту же геометрию.

use glam::Vec2;

use super::features::{FeatureSet, KIND_ROAD, KIND_TRAIL};
use super::heightfield::HeightField;

/// Ширина бровки за полотном, м: на ней узел плавно возвращается к склону.
const SHOULDER_TRAIL_M: f32 = 2.5;
const SHOULDER_ROAD_M: f32 = 4.0;
/// Полуокно сглаживания продольного профиля, м.
const SMOOTH_TRAIL_M: f32 = 5.0;
const SMOOTH_ROAD_M: f32 = 12.0;
/// Шаг выборки профиля вдоль линии, м.
const PROFILE_STEP_M: f32 = 1.0;

/// Полилиния: отрезки `segs` подряд, длина пути до начала каждого и профиль.
struct Line {
    segs: std::ops::Range<usize>,
    start_s: Vec<f32>,
    profile: Vec<f32>,
}

impl Line {
    fn height_at(&self, s: f32) -> f32 {
        let f = (s / PROFILE_STEP_M).clamp(0.0, (self.profile.len() - 1) as f32);
        let i = (f as usize).min(self.profile.len().saturating_sub(2));
        let t = f - i as f32;
        if self.profile.len() == 1 {
            return self.profile[0];
        }
        self.profile[i] + (self.profile[i + 1] - self.profile[i]) * t
    }
}

/// Полилинии троп и дорог с профилями по полю `src` (мир с углом `origin`).
fn lines(src: &HeightField, origin: Vec2, features: &FeatureSet) -> Vec<Line> {
    let segs = &features.segments;
    let mut out = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        let kind = segs[i].kind;
        let mut j = i + 1;
        while j < segs.len() && segs[j].kind == kind && segs[j].a == segs[j - 1].b {
            j += 1;
        }
        if kind == KIND_TRAIL || kind == KIND_ROAD {
            let mut start_s = Vec::with_capacity(j - i);
            let mut s = 0.0;
            for seg in &segs[i..j] {
                start_s.push(s);
                s += Vec2::from(seg.a).distance(Vec2::from(seg.b));
            }
            // Сырой профиль через 1 м; вне поля — ближайшее известное значение.
            let n = (s / PROFILE_STEP_M).ceil() as usize + 1;
            let mut raw: Vec<Option<f32>> = Vec::with_capacity(n);
            let mut k = 0;
            for p in 0..n {
                let d = p as f32 * PROFILE_STEP_M;
                while k + 1 < j - i && start_s[k + 1] <= d {
                    k += 1;
                }
                let seg = &segs[i + k];
                let (a, b) = (Vec2::from(seg.a) - origin, Vec2::from(seg.b) - origin);
                let len = a.distance(b).max(1e-6);
                let t = ((d - start_s[k]) / len).clamp(0.0, 1.0);
                let c = a + (b - a) * t;
                raw.push(src.sample(c.x, c.y).map(|(h, _)| h));
            }
            if raw.iter().any(Option::is_some) {
                let half = if kind == KIND_ROAD {
                    SMOOTH_ROAD_M
                } else {
                    SMOOTH_TRAIL_M
                };
                let w = (half / PROFILE_STEP_M) as isize;
                let mut profile = Vec::with_capacity(n);
                let mut last = raw.iter().flatten().next().copied().unwrap_or(0.0);
                for p in 0..n as isize {
                    let (mut sum, mut cnt) = (0.0, 0.0);
                    for q in (p - w).max(0)..=(p + w).min(n as isize - 1) {
                        if let Some(h) = raw[q as usize] {
                            // Треугольное окно: ближние точки весят больше.
                            let k = 1.0 - (q - p).abs() as f32 / (w + 1) as f32;
                            sum += h * k;
                            cnt += k;
                        }
                    }
                    if cnt > 0.0 {
                        last = sum / cnt;
                    }
                    profile.push(last);
                }
                out.push(Line {
                    segs: i..j,
                    start_s,
                    profile,
                });
            }
        }
        i = j;
    }
    out
}

/// Врезает тропы и дороги в поле `hf`, лежащее в мире с углом `origin` (XZ).
/// Возвращает число изменённых узлов.
pub fn carve_paths(hf: &mut HeightField, origin: Vec2, features: &FeatureSet) -> usize {
    let src = hf.clone();
    let step = hf.step_m;
    // На узел: (расстояние до ближайшей оси, вес, целевая высота).
    let mut best: Vec<(f32, f32, f32)> = vec![(f32::MAX, 0.0, 0.0); hf.heights.len()];
    let size = Vec2::new((hf.nx - 1) as f32, (hf.nz - 1) as f32) * step;
    for line in lines(&src, origin, features) {
        for (k, s) in features.segments[line.segs.clone()].iter().enumerate() {
            let shoulder = if s.kind == KIND_ROAD {
                SHOULDER_ROAD_M
            } else {
                SHOULDER_TRAIL_M
            };
            let a = Vec2::from(s.a) - origin;
            let b = Vec2::from(s.b) - origin;
            let reach = s.half_width + shoulder;
            let lo = a.min(b) - Vec2::splat(reach);
            let hi = a.max(b) + Vec2::splat(reach);
            if hi.x < 0.0 || hi.y < 0.0 || lo.x > size.x || lo.y > size.y {
                continue;
            }
            let i0 = (lo.x / step).floor().max(0.0) as usize;
            let j0 = (lo.y / step).floor().max(0.0) as usize;
            let i1 = ((hi.x / step).ceil() as usize).min(hf.nx - 1);
            let j1 = ((hi.y / step).ceil() as usize).min(hf.nz - 1);
            let ab = b - a;
            let len = ab.length().max(1e-6);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let p = Vec2::new(i as f32, j as f32) * step;
                    let t = ((p - a).dot(ab) / (len * len)).clamp(0.0, 1.0);
                    let d = p.distance(a + ab * t);
                    if d >= reach {
                        continue;
                    }
                    let n = j * hf.nx + i;
                    // Ближайшая ось решает: на стыках и пересечениях нет ступеней.
                    if d < best[n].0 {
                        let x = ((d - s.half_width) / shoulder).clamp(0.0, 1.0);
                        let w = 1.0 - x * x * (3.0 - 2.0 * x);
                        best[n] = (d, w, line.height_at(line.start_s[k] + t * len));
                    }
                }
            }
        }
    }
    let mut changed = 0;
    for (h, (_, w, target)) in hf.heights.iter_mut().zip(best) {
        if w > 0.0 {
            *h += (target - *h) * w;
            changed += 1;
        }
    }
    changed
}
