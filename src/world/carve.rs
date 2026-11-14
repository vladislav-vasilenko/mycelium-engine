//! Тропы и дороги, врезанные в рельеф (02.7): поперёк полотна склон убирается,
//! вдоль — остаётся. Узел у линии тянется к высоте исходного рельефа в
//! ближайшей точке осевой линии: в пределах полуширины полностью, на бровке —
//! плавно. Так полотно тропы — ровная полка, а не полоса краски на склоне,
//! и ходьба, кластеры и шейдер видят одну и ту же геометрию.

use glam::Vec2;

use super::features::{FeatureSet, KIND_ROAD, KIND_TRAIL};
use super::heightfield::HeightField;

/// Ширина бровки за полотном, м: на ней узел плавно возвращается к склону.
const SHOULDER_TRAIL_M: f32 = 2.5;
const SHOULDER_ROAD_M: f32 = 4.0;

/// Врезает тропы и дороги в поле `hf`, лежащее в мире с углом `origin` (XZ).
/// Возвращает число изменённых узлов.
pub fn carve_paths(hf: &mut HeightField, origin: Vec2, features: &FeatureSet) -> usize {
    let src = hf.clone();
    let step = hf.step_m;
    // На узел: (вес, целевая высота) самой сильной линии.
    let mut best: Vec<(f32, f32)> = vec![(0.0, 0.0); hf.heights.len()];
    let size = Vec2::new((hf.nx - 1) as f32, (hf.nz - 1) as f32) * step;
    for s in &features.segments {
        let shoulder = match s.kind {
            KIND_TRAIL => SHOULDER_TRAIL_M,
            KIND_ROAD => SHOULDER_ROAD_M,
            _ => continue,
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
        let len2 = ab.length_squared().max(1e-6);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let p = Vec2::new(i as f32, j as f32) * step;
                let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                let c = a + ab * t;
                let d = p.distance(c);
                if d >= reach {
                    continue;
                }
                let x = ((d - s.half_width) / shoulder).clamp(0.0, 1.0);
                let w = 1.0 - x * x * (3.0 - 2.0 * x);
                let k = j * hf.nx + i;
                if w > best[k].0
                    && let Some((h, _)) = src.sample(c.x, c.y)
                {
                    best[k] = (w, h);
                }
            }
        }
    }
    let mut changed = 0;
    for (h, (w, target)) in hf.heights.iter_mut().zip(best) {
        if w > 0.0 {
            *h += (target - *h) * w;
            changed += 1;
        }
    }
    changed
}
