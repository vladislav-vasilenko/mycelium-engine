//! Реки по дну долин (03.2, сессия 1): линии OSM обводились по снимкам, а
//! рельеф — из модели высот; они расходятся на 10–30 м, и река шла по склону.
//! Каждая точка полилинии реки сдвигается поперёк течения к самой низкой
//! точке рельефа в полосе ±`SEARCH_M` (со штрафом за дальность — не
//! перескакивать в соседнюю долину), затем линия сглаживается. Тропы и дороги
//! в ущельях идут по дну или полкой над ручьём: точки ближе `PATH_CLEAR_M` к
//! ним запрещены — первая версия (±30 м) утягивала ручей на тропу.

use glam::Vec2;

use super::features::{FeatureSet, KIND_RIVER};

/// Полуширина поиска дна поперёк течения, м.
pub const SEARCH_M: f32 = 8.0;
/// Шаг поиска, м.
const STEP_M: f32 = 1.0;
/// Штраф за сдвиг, м высоты на метр: при равной высоте — ближе к OSM.
const SHIFT_PENALTY: f32 = 0.12;
/// Ближе к краю тропы или дороги русло не ставится, м.
const PATH_CLEAR_M: f32 = 3.0;

/// Полилинии рек: отрезки подряд, у которых конец совпадает с началом следующего.
fn river_lines(set: &FeatureSet) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    let segs = &set.segments;
    while i < segs.len() {
        if segs[i].kind != KIND_RIVER {
            i += 1;
            continue;
        }
        let start = i;
        while i + 1 < segs.len() && segs[i + 1].kind == KIND_RIVER && segs[i + 1].a == segs[i].b {
            i += 1;
        }
        i += 1;
        out.push(start..i);
    }
    out
}

/// Шаг сгущения линии реки, м: точки OSM стоят через десятки метров, и прямой
/// отрезок между ними резал склоны — линия сгущается, затем каждая точка
/// привязывается к дну.
const DENSE_M: f32 = 4.0;

/// Сдвиг рек к тальвегу по высоте `height(xz)` и пересборка ячеек. Возвращает
/// средний сдвиг точек, м (для журнала).
pub fn snap_to_valleys(
    set: &mut FeatureSet,
    map_size_m: f32,
    height: impl Fn(Vec2) -> Option<f32>,
) -> f32 {
    let mut total = 0.0;
    let mut count = 0usize;
    let lines = river_lines(set);
    let mut out: Vec<crate::layout::FeatureSegment> = Vec::with_capacity(set.segments.len() * 2);
    let mut next_line = 0;
    let mut i = 0;
    while i < set.segments.len() {
        let Some(r) = lines.get(next_line).filter(|r| r.start == i).cloned() else {
            out.push(set.segments[i]);
            i += 1;
            continue;
        };
        next_line += 1;
        i = r.end;
        let tmpl = set.segments[r.start];
        // Сгущение.
        let mut orig: Vec<Vec2> = vec![Vec2::from(set.segments[r.start].a)];
        for s in &set.segments[r.clone()] {
            let (a, b) = (Vec2::from(s.a), Vec2::from(s.b));
            let steps = ((b - a).length() / DENSE_M).ceil().max(1.0) as usize;
            for k in 1..=steps {
                orig.push(a + (b - a) * (k as f32 / steps as f32));
            }
        }
        let n = orig.len();
        let mut pts = orig.clone();
        // Концы — на месте: на них стыкуются куски одной реки и притоки
        // (сдвиг концов давал разрывы ленты воды на стыках).
        for k in 1..n.saturating_sub(1) {
            let t = (orig[(k + 1).min(n - 1)] - orig[k.saturating_sub(1)]).normalize_or_zero();
            if t == Vec2::ZERO {
                continue;
            }
            let nrm = Vec2::new(-t.y, t.x);
            let mut best = (f32::INFINITY, 0.0f32);
            let mut d = -SEARCH_M;
            while d <= SEARCH_M {
                let q = orig[k] + nrm * d;
                // Высота — среднее по окну 3 м вдоль течения: без шума рельефа.
                let hs = [q - t * 1.5, q, q + t * 1.5].map(&height);
                if let [Some(h0), Some(h1), Some(h2)] = hs {
                    let h = (h0 + h1 + h2) / 3.0;
                    let fs = set.sample(q.x, q.y);
                    let on_path = fs.trail_m.min(fs.road_m) < PATH_CLEAR_M;
                    let score = h + d.abs() * SHIFT_PENALTY + if on_path { 50.0 } else { 0.0 };
                    if score < best.0 {
                        best = (score, d);
                    }
                }
                d += STEP_M;
            }
            pts[k] = orig[k] + nrm * best.1;
        }
        // Сглаживание (окно ~±8 м): без зигзагов между ложбинками; концы — на
        // месте (стыки с притоками).
        for _ in 0..4 {
            let src = pts.clone();
            for k in 1..n.saturating_sub(1) {
                pts[k] = src[k - 1] * 0.25 + src[k] * 0.5 + src[k + 1] * 0.25;
            }
        }
        for k in 0..n {
            total += (pts[k] - orig[k]).length();
            count += 1;
        }
        for w in pts.windows(2) {
            let mut seg = tmpl;
            seg.a = w[0].to_array();
            seg.b = w[1].to_array();
            out.push(seg);
        }
    }
    set.segments = out;
    set.index(map_size_m);
    if count > 0 { total / count as f32 } else { 0.0 }
}

/// Глубина ложа под профилем дна, м; откос берега (подъём на метр в сторону).
const BED_DEPTH_M: f32 = 0.35;
const BANK_SLOPE: f32 = 0.6;
/// Ширина откоса за полосой реки, м.
const BANK_M: f32 = 4.0;

/// Врезка ложа рек (03.2): вдоль каждой реки профиль дна по рельефу делается
/// невозрастающим вниз по течению (горбы срезаются — лента воды не уходит под
/// рельеф), и узлы поля у русла опускаются до «профиль − глубина ложа» с
/// откосом к берегу. Только опускание: высокие места не засыпаются.
/// `origin` — мировые XZ узла (0, 0) поля. Возвращает число изменённых узлов.
pub fn carve_beds(
    hf: &mut super::heightfield::HeightField,
    origin: Vec2,
    set: &FeatureSet,
) -> usize {
    let src = hf.clone();
    let step = hf.step_m;
    let size = Vec2::new((hf.nx - 1) as f32, (hf.nz - 1) as f32) * step;
    let h_at = |p: Vec2| src.sample(p.x - origin.x, p.y - origin.y).map(|(h, _)| h);
    let mut changed = 0;
    for r in river_lines(set) {
        let segs = &set.segments[r.clone()];
        let mut pts: Vec<Vec2> = segs.iter().map(|s| Vec2::from(s.a)).collect();
        pts.push(Vec2::from(segs[segs.len() - 1].b));
        let Some(mut prof): Option<Vec<f32>> = pts.iter().map(|p| h_at(*p)).collect() else {
            continue;
        };
        // Вниз по течению — к низкому концу; профиль не растёт по ходу воды.
        let down = prof[0] >= prof[prof.len() - 1];
        if down {
            for k in 1..prof.len() {
                prof[k] = prof[k].min(prof[k - 1]);
            }
        } else {
            for k in (0..prof.len() - 1).rev() {
                prof[k] = prof[k].min(prof[k + 1]);
            }
        }
        for (k, s) in segs.iter().enumerate() {
            let a = Vec2::from(s.a) - origin;
            let b = Vec2::from(s.b) - origin;
            let reach = s.half_width + BANK_M;
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
                    let d = p.distance(a + ab * t);
                    if d >= reach {
                        continue;
                    }
                    let bed = prof[k] + (prof[k + 1] - prof[k]) * t - BED_DEPTH_M;
                    let allowed = bed + (d - s.half_width).max(0.0) * BANK_SLOPE;
                    let n = j * hf.nx + i;
                    if hf.heights[n] > allowed {
                        hf.heights[n] = allowed;
                        changed += 1;
                    }
                }
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn river_moves_to_valley_floor() {
        // Долина вдоль z с дном на x = 120; река OSM — на склоне, x = 114.
        let json = r#"{"rivers": [{"kind": "stream", "width_m": 4.0,
            "line": [[114, 20], [114, 60], [114, 100], [114, 140]]}], "trails": [], "roads": []}"#;
        let mut set = FeatureSet::from_json(json, 256.0).unwrap();
        let shift = snap_to_valleys(&mut set, 256.0, |p| Some((p.x - 120.0).abs() * 0.5));
        assert!(shift > 4.0, "сдвиг {shift}");
        // Внутренние точки — на дне (концы стыков остаются на месте).
        let n = set.segments.len();
        for s in &set.segments[3..n - 3] {
            assert!((s.a[0] - 120.0).abs() < 2.0, "точка {:?} не на дне", s.a);
        }
        // Ячейки пересобраны: река найдена у дна.
        assert!(set.sample(120.0, 80.0).river_m < 0.0);
    }

    #[test]
    fn river_keeps_off_trail_on_valley_floor() {
        // По дну (x = 120) идёт тропа: ручей встаёт рядом, а не на полотно.
        let json = r#"{"rivers": [{"kind": "stream", "width_m": 4.0,
            "line": [[116, 20], [116, 60], [116, 100], [116, 140]]}],
            "trails": [{"kind": "path", "line": [[120, 0], [120, 160]]}], "roads": []}"#;
        let mut set = FeatureSet::from_json(json, 256.0).unwrap();
        snap_to_valleys(&mut set, 256.0, |p| Some((p.x - 120.0).abs() * 0.5));
        for s in set.segments.iter().filter(|s| s.kind == KIND_RIVER) {
            assert!((s.a[0] - 120.0).abs() > 3.0, "ручей на тропе: {:?}", s.a);
        }
    }

    #[test]
    fn bed_cuts_through_hump() {
        use crate::world::heightfield::HeightField;
        // Долина вниз по x с горбом 3 м посередине; река вдоль x.
        let n = 64;
        let mut h = vec![0.0; n * n];
        for j in 0..n {
            for i in 0..n {
                let x = i as f32;
                h[j * n + i] = 50.0 - x * 0.3 + if (28..36).contains(&i) { 3.0 } else { 0.0 };
            }
        }
        let mut hf = HeightField::new(n, n, 1.0, h);
        let json = r#"{"rivers": [{"kind": "stream", "width_m": 2.0,
            "line": [[2, 32], [20, 32], [40, 32], [60, 32]]}], "trails": [], "roads": []}"#;
        let set = FeatureSet::from_json(json, 64.0).unwrap();
        assert!(carve_beds(&mut hf, Vec2::ZERO, &set) > 0);
        // На горбе дно не выше, чем до него.
        let before = hf.at(25, 32);
        let hump = hf.at(32, 32);
        assert!(hump <= before, "горб срезан: {hump} > {before}");
    }
}
