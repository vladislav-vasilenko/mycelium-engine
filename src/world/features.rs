//! Объекты карты уже шага сетки (02.7): тропы, дороги и реки как отрезки из
//! `features.json` (векторы OpenStreetMap в мировых метрах).
//!
//! Отрезки разложены по ячейкам `FEATURE_CELL_M`: в каждой ячейке — список
//! отрезков, полоса которых (с запасом `FEATURE_MARGIN_M`) её задевает. Шейдер
//! ландшафта и `sample_ground` считают точное расстояние до отрезков своей
//! ячейки — линия шириной 1,6 м остаётся чёткой при сетке рельефа 30 м.
//! Поле расстояний в текстуре так не умеет: беззнаковое расстояние ломается на
//! самой линии и при билинейной выборке не доходит до нуля.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::layout::FeatureSegment;

/// Описание слоя в `world.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeaturesMeta {
    pub file: String,
}

/// Сторона ячейки списков, м (та же в `features.h`).
pub const FEATURE_CELL_M: f32 = 32.0;
/// Запас вокруг полосы: берег русла и сглаживание края.
pub const FEATURE_MARGIN_M: f32 = 6.0;

pub const KIND_TRAIL: u32 = 0;
pub const KIND_RIVER: u32 = 1;
pub const KIND_ROAD: u32 = 2;

#[derive(Deserialize)]
struct Line {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    width_m: f32,
    line: Vec<[f32; 2]>,
}

#[derive(Deserialize)]
struct FeaturesFile {
    rivers: Vec<Line>,
    trails: Vec<Line>,
    roads: Vec<Line>,
}

/// Полуширина по виду OSM, м.
fn half_width(kind: u32, osm: &str, width_m: f32) -> f32 {
    match (kind, osm) {
        (KIND_RIVER, _) => (width_m * 0.5).max(0.5),
        (KIND_TRAIL, "track") => 1.4,
        (KIND_TRAIL, "footway") => 1.0,
        (KIND_TRAIL, _) => 0.8,
        (_, "primary" | "trunk") => 3.5,
        (_, "secondary") => 3.0,
        (_, "tertiary") => 2.75,
        (_, "service") => 1.8,
        _ => 2.5,
    }
}

/// Расстояния до края ближайших полос, м (< 0 — внутри полосы).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeatureSample {
    pub trail_m: f32,
    pub river_m: f32,
    pub road_m: f32,
}

/// Далеко от всех объектов.
pub const FAR: f32 = 1e3;

#[derive(Clone, Debug, Default)]
pub struct FeatureSet {
    /// Исходный `features.json` — для экспорта слоёв без потерь.
    pub json: String,
    pub segments: Vec<FeatureSegment>,
    /// Ячеек по стороне карты.
    pub cells_n: u32,
    /// (начало, число) в `list` для каждой ячейки, строки по z.
    pub cells: Vec<[u32; 2]>,
    pub list: Vec<u32>,
}

impl FeatureSet {
    /// Пустой набор для карты `map_size_m` (у генератора объектов нет).
    pub fn empty(map_size_m: f32) -> FeatureSet {
        let n = cells_for(map_size_m);
        FeatureSet {
            json: String::new(),
            segments: Vec::new(),
            cells_n: n,
            cells: vec![[0, 0]; (n * n) as usize],
            list: Vec::new(),
        }
    }

    /// Разбор `features.json` и раскладка по ячейкам.
    pub fn from_json(text: &str, map_size_m: f32) -> Result<FeatureSet, String> {
        let f: FeaturesFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut segments = Vec::new();
        for (kind, lines) in [
            (KIND_TRAIL, &f.trails),
            (KIND_RIVER, &f.rivers),
            (KIND_ROAD, &f.roads),
        ] {
            for l in lines {
                let hw = half_width(kind, &l.kind, l.width_m);
                for w in l.line.windows(2) {
                    segments.push(FeatureSegment {
                        a: w[0],
                        b: w[1],
                        half_width: hw,
                        kind,
                        _pad: [0.0; 2],
                    });
                }
            }
        }
        let n = cells_for(map_size_m);
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); (n * n) as usize];
        for (i, s) in segments.iter().enumerate() {
            let r = s.half_width + FEATURE_MARGIN_M;
            let lo = Vec2::from(s.a).min(Vec2::from(s.b)) - r;
            let hi = Vec2::from(s.a).max(Vec2::from(s.b)) + r;
            let c0 = (lo / FEATURE_CELL_M).floor().max(Vec2::ZERO);
            let c1 = (hi / FEATURE_CELL_M)
                .floor()
                .min(Vec2::splat(n as f32 - 1.0));
            for cz in c0.y as u32..=c1.y as u32 {
                for cx in c0.x as u32..=c1.x as u32 {
                    // Ячейка задета, если её центр ближе к отрезку, чем полоса + полдиагонали.
                    let center = (Vec2::new(cx as f32, cz as f32) + 0.5) * FEATURE_CELL_M;
                    if segment_distance(center, s) <= r + FEATURE_CELL_M * 0.7072 {
                        buckets[(cz * n + cx) as usize].push(i as u32);
                    }
                }
            }
        }
        let mut cells = Vec::with_capacity(buckets.len());
        let mut list = Vec::new();
        for b in &buckets {
            cells.push([list.len() as u32, b.len() as u32]);
            list.extend_from_slice(b);
        }
        Ok(FeatureSet {
            json: text.to_string(),
            segments,
            cells_n: n,
            cells,
            list,
        })
    }

    /// Точные расстояния в точке — тот же расчёт, что в `features.h`.
    pub fn sample(&self, x: f32, z: f32) -> FeatureSample {
        let mut out = FeatureSample {
            trail_m: FAR,
            river_m: FAR,
            road_m: FAR,
        };
        let p = Vec2::new(x, z);
        let c = (p / FEATURE_CELL_M).floor();
        if c.x < 0.0 || c.y < 0.0 || c.x >= self.cells_n as f32 || c.y >= self.cells_n as f32 {
            return out;
        }
        let [start, count] = self.cells[(c.y as u32 * self.cells_n + c.x as u32) as usize];
        for &i in &self.list[start as usize..(start + count) as usize] {
            let s = &self.segments[i as usize];
            let d = segment_distance(p, s) - s.half_width;
            let slot = match s.kind {
                KIND_TRAIL => &mut out.trail_m,
                KIND_RIVER => &mut out.river_m,
                _ => &mut out.road_m,
            };
            *slot = slot.min(d);
        }
        out
    }

    /// Самая длинная ячейка — оценка худшего случая для шейдера.
    pub fn max_per_cell(&self) -> u32 {
        self.cells.iter().map(|c| c[1]).max().unwrap_or(0)
    }
}

fn cells_for(map_size_m: f32) -> u32 {
    (map_size_m / FEATURE_CELL_M).ceil().max(1.0) as u32
}

fn segment_distance(p: Vec2, s: &FeatureSegment) -> f32 {
    let (a, b) = (Vec2::from(s.a), Vec2::from(s.b));
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{
        "rivers": [{"kind": "river", "width_m": 10.0, "line": [[100, 50], [100, 150]]}],
        "trails": [{"kind": "path", "line": [[10, 10], [90, 10]]}],
        "roads": []
    }"#;

    /// На оси тропы расстояние −полуширина, в стороне — точное; река шириной 10 м.
    #[test]
    fn exact_distance_from_cells() {
        let f = FeatureSet::from_json(JSON, 256.0).unwrap();
        let s = f.sample(50.0, 10.0);
        assert!((s.trail_m + 0.8).abs() < 1e-5);
        assert!((f.sample(50.0, 13.0).trail_m - 2.2).abs() < 1e-5);
        assert!((f.sample(103.0, 100.0).river_m + 2.0).abs() < 1e-5);
        assert_eq!(f.sample(200.0, 200.0).trail_m, FAR);
        // Каждая точка у полосы видит свой отрезок во всех задетых ячейках.
        for i in 0..200 {
            let x = 10.0 + i as f32 * 0.4;
            for dz in [-6.0, 0.0, 6.0] {
                let d = f.sample(x, 10.0 + dz).trail_m;
                assert!((d - dz.abs() + 0.8).abs() < 1e-4, "x {x} dz {dz}: {d}");
            }
        }
    }
}
