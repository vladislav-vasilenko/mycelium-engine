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
        for k in 0..n {
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
        for s in &set.segments {
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
}
