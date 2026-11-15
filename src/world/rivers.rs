//! Реки по дну долин (03.2, сессия 1): линии OSM обводились по снимкам, а
//! рельеф — из модели высот; они расходятся на 10–30 м, и река шла по склону.
//! Каждая точка полилинии реки сдвигается поперёк течения к самой низкой
//! точке рельефа в полосе ±`SEARCH_M` (с лёгким штрафом за дальность — не
//! перескакивать в соседнюю долину), затем линия сглаживается.

use glam::Vec2;

use super::features::{FeatureSet, KIND_RIVER};

/// Полуширина поиска дна поперёк течения, м.
pub const SEARCH_M: f32 = 30.0;
/// Шаг поиска, м.
const STEP_M: f32 = 1.5;
/// Штраф за сдвиг, м высоты на метр: при равной высоте — ближе к OSM.
const SHIFT_PENALTY: f32 = 0.03;

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

/// Сдвиг рек к тальвегу по высоте `height(xz)` и пересборка ячеек. Возвращает
/// средний сдвиг точек, м (для журнала).
pub fn snap_to_valleys(
    set: &mut FeatureSet,
    map_size_m: f32,
    height: impl Fn(Vec2) -> Option<f32>,
) -> f32 {
    let mut total = 0.0;
    let mut count = 0usize;
    for r in river_lines(set) {
        let mut pts: Vec<Vec2> = set.segments[r.clone()]
            .iter()
            .map(|s| Vec2::from(s.a))
            .collect();
        pts.push(Vec2::from(set.segments[r.end - 1].b));
        let n = pts.len();
        let orig = pts.clone();
        for k in 0..n {
            let prev = orig[k.saturating_sub(1)];
            let next = orig[(k + 1).min(n - 1)];
            let t = (next - prev).normalize_or_zero();
            if t == Vec2::ZERO {
                continue;
            }
            let nrm = Vec2::new(-t.y, t.x);
            let mut best = (f32::INFINITY, 0.0f32);
            let mut d = -SEARCH_M;
            while d <= SEARCH_M {
                if let Some(h) = height(orig[k] + nrm * d) {
                    let score = h + d.abs() * SHIFT_PENALTY;
                    if score < best.0 {
                        best = (score, d);
                    }
                }
                d += STEP_M;
            }
            pts[k] = orig[k] + nrm * best.1;
        }
        // Сглаживание: дно, найденное по точкам, не должно зигзагом скакать
        // между соседними ложбинками; концы — на месте (стыки с притоками).
        for _ in 0..3 {
            let src = pts.clone();
            for k in 1..n.saturating_sub(1) {
                pts[k] = src[k - 1] * 0.25 + src[k] * 0.5 + src[k + 1] * 0.25;
            }
        }
        for (k, p) in pts.iter().enumerate() {
            total += (*p - orig[k]).length();
            count += 1;
        }
        for (j, i) in r.enumerate() {
            set.segments[i].a = pts[j].to_array();
            set.segments[i].b = pts[j + 1].to_array();
        }
    }
    set.index(map_size_m);
    if count > 0 { total / count as f32 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn river_moves_to_valley_floor() {
        // Долина вдоль z с дном на x = 120; река OSM — на склоне, x = 100.
        let json = r#"{"rivers": [{"kind": "stream", "width_m": 4.0,
            "line": [[100, 20], [100, 60], [100, 100], [100, 140]]}], "trails": [], "roads": []}"#;
        let mut set = FeatureSet::from_json(json, 256.0).unwrap();
        let shift = snap_to_valleys(&mut set, 256.0, |p| Some((p.x - 120.0).abs() * 0.5));
        assert!(shift > 15.0, "сдвиг {shift}");
        for s in &set.segments {
            assert!((s.a[0] - 120.0).abs() < 2.0, "точка {:?} не на дне", s.a);
        }
        // Ячейки пересобраны: река найдена у дна.
        assert!(set.sample(120.0, 80.0).river_m < 0.0);
    }
}
