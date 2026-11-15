//! Гидрология острова (03.2, сессия 2): реки поднимаются от дождя.
//!
//! Сеть — полилинии рек `FeatureSet` (после привязки к тальвегу). Участок
//! течёт от высокого конца к низкому; приток — участок, чей нижний конец
//! ближе `JOIN_M` к другому участку. Площадь водосбора — по закону Хака из
//! длины русел выше по течению (L = 1,4 · A^0,6, км). Сток — линейный
//! резервуар на водосбор: dS/dt = дождь · коэффициент стока · A − S / K,
//! расход Q = базовый + S / K; K растёт с площадью (большая река медленнее).
//! Глубина — по Маннингу для прямоугольного русла с каменистым дном; подъём
//! сверх меженной глубины расширяет полосу реки (разлив на берег).

use glam::Vec2;

use crate::world::features::{FeatureSet, KIND_RIVER};

/// Нижний конец ближе этого к другой реке — приток, м.
const JOIN_M: f32 = 20.0;
/// Ливень (сила дождя 1), м/с: ~30 мм/ч.
const RAIN_M_S: f32 = 8.3e-6;
/// Базовый сток с площади, м/с (≈0,04 м³/с с км² — Якусима очень влажная).
const BASE_M_S: f32 = 4e-8;
/// Коэффициент Маннинга: русло из валунов.
const MANNING_N: f32 = 0.05;
/// Расширение полосы на метр подъёма воды, м (пологий берег).
const BANK_SPREAD: f32 = 4.0;
/// Наибольшее расширение (запас ячеек отрезков — 6 м).
const MAX_SPREAD_M: f32 = 5.0;

/// Площадь водосбора по стоку D8 (`flow.png` из `tools/geo/yakushima.py`):
/// log₂ числа клеток × 2048, сетка карты острова (строки по z).
pub struct FlowGrid {
    n: usize,
    step_m: f32,
    log2: Vec<u16>,
}

impl FlowGrid {
    pub fn load(path: &std::path::Path, map_size_m: f32) -> Result<FlowGrid, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let dec = png::Decoder::new(std::io::BufReader::new(file));
        let mut reader = dec
            .read_info()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
        let info = reader
            .next_frame(&mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if info.bit_depth != png::BitDepth::Sixteen || info.width != info.height {
            return Err(format!(
                "{}: нужен квадратный 16-битный PNG",
                path.display()
            ));
        }
        let n = info.width as usize;
        let log2 = buf[..info.buffer_size()]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        Ok(FlowGrid {
            n,
            step_m: map_size_m / (n - 1) as f32,
            log2,
        })
    }

    /// Наибольшая площадь водосбора в радиусе `r` клеток от точки, м²
    /// (русло D8 и линия реки расходятся на клетку-другую).
    pub fn max_area_near(&self, p: Vec2, r: i32) -> f32 {
        let c = (p / self.step_m).round();
        let mut best = 0u16;
        for dz in -r..=r {
            for dx in -r..=r {
                let (x, z) = (c.x as i32 + dx, c.y as i32 + dz);
                if x >= 0 && z >= 0 && (x as usize) < self.n && (z as usize) < self.n {
                    best = best.max(self.log2[z as usize * self.n + x as usize]);
                }
            }
        }
        (best as f32 / 2048.0).exp2() * self.step_m * self.step_m
    }
}

#[derive(Clone, Debug)]
pub struct Reach {
    /// Отрезки участка в `FeatureSet::segments`.
    pub segs: std::ops::Range<usize>,
    pub length_m: f32,
    /// Площадь водосбора выше нижнего конца, м².
    pub area_m2: f32,
    pub width_m: f32,
    pub slope: f32,
    /// Постоянная резервуара, игровые с.
    pub k_s: f32,
    /// Запас воды в резервуаре, м³.
    pub storage: f32,
    /// Расход, м³/с; глубина и меженная глубина, м.
    pub q: f32,
    pub depth: f32,
    pub base_depth: f32,
    /// Нижний конец (устье участка) и куда впадает.
    pub mouth: Vec2,
    pub into: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Hydrology {
    pub reaches: Vec<Reach>,
}

fn manning_depth(q: f32, width: f32, slope: f32) -> f32 {
    (q * MANNING_N / (width * slope.sqrt())).max(0.0).powf(0.6)
}

impl Hydrology {
    /// Сеть рек и водосборы; `height` — высота рельефа.
    pub fn build(
        set: &FeatureSet,
        height: impl Fn(Vec2) -> Option<f32>,
        flow: Option<&FlowGrid>,
    ) -> Hydrology {
        // Участки — полилинии рек (как в `world::rivers`).
        let segs = &set.segments;
        let mut ranges = Vec::new();
        let mut i = 0;
        while i < segs.len() {
            if segs[i].kind != KIND_RIVER {
                i += 1;
                continue;
            }
            let s = i;
            while i + 1 < segs.len() && segs[i + 1].kind == KIND_RIVER && segs[i + 1].a == segs[i].b
            {
                i += 1;
            }
            i += 1;
            ranges.push(s..i);
        }
        let mut reaches: Vec<Reach> = ranges
            .into_iter()
            .map(|r| {
                let a = Vec2::from(segs[r.start].a);
                let b = Vec2::from(segs[r.end - 1].b);
                let len: f32 = segs[r.clone()]
                    .iter()
                    .map(|s| (Vec2::from(s.b) - Vec2::from(s.a)).length())
                    .sum();
                let (ha, hb) = (height(a).unwrap_or(0.0), height(b).unwrap_or(0.0));
                let mouth = if hb <= ha { b } else { a };
                let slope = ((ha - hb).abs() / len.max(1.0)).clamp(0.01, 0.3);
                Reach {
                    width_m: (segs[r.start].half_width * 2.0).clamp(1.0, 30.0),
                    segs: r,
                    length_m: len,
                    area_m2: 0.0,
                    slope,
                    k_s: 0.0,
                    storage: 0.0,
                    q: 0.0,
                    depth: 0.0,
                    base_depth: 0.0,
                    mouth,
                    into: None,
                }
            })
            .collect();
        // Притоки: нижний конец у другого участка.
        for j in 0..reaches.len() {
            let m = reaches[j].mouth;
            let mut best = (JOIN_M, None);
            for (k, r) in reaches.iter().enumerate() {
                if k == j {
                    continue;
                }
                for s in &segs[r.segs.clone()] {
                    let (a, b) = (Vec2::from(s.a), Vec2::from(s.b));
                    let ab = b - a;
                    let t = ((m - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    let d = (m - (a + ab * t)).length();
                    if d < best.0 {
                        best = (d, Some(k));
                    }
                }
            }
            reaches[j].into = best.1;
        }
        // Длина русел выше по течению: проталкиваем вниз по цепочкам (без
        // циклов — глубина ограничена числом участков).
        let mut up_len: Vec<f32> = reaches.iter().map(|r| r.length_m).collect();
        for j in 0..reaches.len() {
            let mut k = reaches[j].into;
            let mut hops = 0;
            while let Some(n) = k {
                if n == j || hops > reaches.len() {
                    break;
                }
                up_len[n] += reaches[j].length_m;
                k = reaches[n].into;
                hops += 1;
            }
        }
        for (r, l) in reaches.iter_mut().zip(&up_len) {
            // Водосбор — по стоку D8, если есть; иначе — закон Хака по длине.
            let l_km = (l / 1000.0).max(0.05);
            let hack = (l_km / 1.4).powf(1.0 / 0.6) * 1e6;
            r.area_m2 = flow.map_or(hack, |f| f.max_area_near(r.mouth, 2).max(hack));
            let a_km2 = r.area_m2 / 1e6;
            r.k_s = 1800.0 * a_km2.max(0.05).powf(0.35);
            let q0 = r.area_m2 * BASE_M_S;
            r.q = q0;
            r.base_depth = manning_depth(q0, r.width_m, r.slope);
            r.depth = r.base_depth;
        }
        Hydrology { reaches }
    }

    /// Шаг на `dt` игровых секунд при силе дождя `rain` и влажности земли `wet`.
    pub fn update(&mut self, dt: f32, rain: f32, wet: f32) {
        let coef = 0.2 + 0.6 * wet.clamp(0.0, 1.0);
        for r in &mut self.reaches {
            let inflow = rain.max(0.0) * RAIN_M_S * coef * r.area_m2;
            // Неявный шаг: устойчив при любом dt.
            r.storage = (r.storage + inflow * dt) / (1.0 + dt / r.k_s);
            r.q = r.area_m2 * BASE_M_S + r.storage / r.k_s;
            r.depth = manning_depth(r.q, r.width_m, r.slope);
        }
    }

    /// Расширение полосы реки на каждом отрезке от подъёма воды, м.
    pub fn spread(&self, segment_count: usize) -> Vec<f32> {
        let mut out = vec![0.0; segment_count];
        for r in &self.reaches {
            let s = ((r.depth - r.base_depth) * BANK_SPREAD).clamp(0.0, MAX_SPREAD_M);
            for i in r.segs.clone() {
                if i < out.len() {
                    out[i] = s;
                }
            }
        }
        out
    }

    /// Участок, ближайший к точке (подпись в заголовке), и расстояние, м.
    pub fn nearest(&self, set: &FeatureSet, p: Vec2) -> Option<(&Reach, f32)> {
        let mut best: Option<(usize, f32)> = None;
        for (k, r) in self.reaches.iter().enumerate() {
            for s in &set.segments[r.segs.clone()] {
                let (a, b) = (Vec2::from(s.a), Vec2::from(s.b));
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                let d = (p - (a + ab * t)).length();
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((k, d));
                }
            }
        }
        best.map(|(k, d)| (&self.reaches[k], d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network() -> (FeatureSet, Hydrology) {
        // Ручей впадает в реку; высота падает к x = 0.
        let json = r#"{"rivers": [
            {"kind": "river", "width_m": 8.0, "line": [[1000, 500], [500, 500], [10, 500]]},
            {"kind": "stream", "width_m": 3.0, "line": [[700, 1500], [700, 900], [700, 505]]}],
            "trails": [], "roads": []}"#;
        let set = FeatureSet::from_json(json, 2048.0).unwrap();
        let h = Hydrology::build(
            &set,
            |p| Some(p.x * 0.05 + (p.y - 500.0).abs() * 0.05),
            None,
        );
        (set, h)
    }

    #[test]
    fn tributary_joins_and_grows_area() {
        let (_, h) = network();
        let stream = h.reaches.iter().find(|r| r.width_m < 5.0).unwrap();
        let river = h.reaches.iter().find(|r| r.width_m > 5.0).unwrap();
        assert!(stream.into.is_some(), "ручей впадает");
        assert!(river.area_m2 > stream.area_m2, "у реки водосбор больше");
    }

    #[test]
    fn river_rises_in_rain_and_falls_after() {
        let (_, mut h) = network();
        let d0: Vec<f32> = h.reaches.iter().map(|r| r.depth).collect();
        for _ in 0..360 {
            h.update(10.0, 1.0, 1.0); // час ливня
        }
        let peak: Vec<f32> = h.reaches.iter().map(|r| r.depth).collect();
        for (a, b) in d0.iter().zip(&peak) {
            assert!(b > a, "уровень растёт: {a} → {b}");
        }
        for _ in 0..1440 {
            h.update(10.0, 0.0, 0.5); // четыре часа без дождя
        }
        for (p, r) in peak.iter().zip(&h.reaches) {
            assert!(r.depth < *p, "спадает");
            assert!(r.depth >= r.base_depth - 1e-4, "не ниже межени");
        }
    }
}
