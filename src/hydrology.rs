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
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
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

/// Наименьшая глубина воды демо, м (`LEVEL_MIN_M` в `river.metal`): ручьи по
/// гидрологии — сантиметры; демо: ×3 от прежних 0,3 м и ещё +3 м по просьбе.
pub const LEVEL_MIN_M: f32 = 3.9;
/// Наибольший подпор плёса над обычной глубиной, м.
pub const POOL_MAX_M: f32 = 1.0;

#[derive(Clone, Debug, Default)]
pub struct Hydrology {
    pub reaches: Vec<Reach>,
    /// Дно на концах каждого отрезка объектов карты (a, b), м; у не-рек — 0.
    pub bed: Vec<[f32; 2]>,
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
        let mut bed = vec![[0.0f32; 2]; segs.len()];
        for r in &reaches {
            for i in r.segs.clone() {
                let s = &segs[i];
                bed[i] = [
                    height(Vec2::from(s.a)).unwrap_or(0.0),
                    height(Vec2::from(s.b)).unwrap_or(0.0),
                ];
            }
        }
        Hydrology { reaches, bed }
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

    /// На каждый отрезок: (расширение полосы от подъёма воды, глубина), м.
    pub fn spread(&self, segment_count: usize) -> Vec<[f32; 2]> {
        let mut out = vec![[0.0; 2]; segment_count];
        for r in &self.reaches {
            let s = ((r.depth - r.base_depth) * BANK_SPREAD).clamp(0.0, MAX_SPREAD_M);
            for i in r.segs.clone() {
                if i < out.len() {
                    out[i] = [s, r.depth];
                }
            }
        }
        out
    }

    /// Уровень воды на концах каждого отрезка (a, b), м: вода заполняет
    /// русло до уровня и стремится к горизонтали. Идя от устья вверх по
    /// течению, уровень не ниже уровня ниже по течению — ямы и ступени дна
    /// заполняются до кромки следующего порога (плёсы), перепады дна больше
    /// глубины остаются скатами; затем сглаживание с теми же ограничениями
    /// (не ниже дна + половина глубины, не в гору). `scale` — множитель глубины.
    /// Глубина демо участка, м: гидрология × `scale`, не меньше
    /// `LEVEL_MIN_M` на равнинных реках; на крутых ручьях подъём падает (на
    /// уклоне от 12 % — десятая часть): 4 м воды поперёк склона висели над
    /// откосом.
    pub fn demo_depth(r: &Reach, scale: f32) -> f32 {
        let k = (1.0 - (r.slope - 0.03) * 10.0).clamp(0.1, 1.0);
        (r.depth * scale).max(LEVEL_MIN_M * k)
    }

    /// Возвращает на отрезок `[уровень a, уровень b, глубина демо, 0]`.
    pub fn levels(&self, segment_count: usize, scale: f32) -> Vec<[f32; 4]> {
        let mut out = vec![[0.0; 4]; segment_count];
        for r in &self.reaches {
            let segs: Vec<usize> = r.segs.clone().filter(|&i| i < self.bed.len()).collect();
            if segs.is_empty() {
                continue;
            }
            let dd = Self::demo_depth(r, scale);
            let d = dd + 0.12;
            // Точки линии: a каждого отрезка и b последнего.
            let mut bed: Vec<f32> = segs.iter().map(|&i| self.bed[i][0]).collect();
            bed.push(self.bed[segs[segs.len() - 1]][1]);
            let n = bed.len();
            // Порядок вниз по течению: к низкому концу.
            let down: Vec<usize> = if bed[0] >= bed[n - 1] {
                (0..n).collect()
            } else {
                (0..n).rev().collect()
            };
            let mut w = vec![0.0f32; n];
            let fill = |w: &mut Vec<f32>, min_d: f32| {
                // От устья вверх: не ниже уровня ниже по течению.
                let last = down[n - 1];
                w[last] = w[last]
                    .min(bed[last] + d + POOL_MAX_M)
                    .max(bed[last] + min_d);
                for k in (0..n - 1).rev() {
                    let (here, below) = (down[k], down[k + 1]);
                    // Подпор плёса — не выше `POOL_MAX_M` над глубиной: горб дна
                    // выше (линия реки режет склон, рельеф GPU отличается) не
                    // поднимает всю воду выше по течению до своей вершины —
                    // иначе лента висит над склоном.
                    let own = bed[here] + min_d;
                    // Сглаживание тоже не поднимает воду выше подпора.
                    let cap = bed[here] + d + POOL_MAX_M;
                    w[here] = w[here].min(cap).max(own).max(w[below].min(cap));
                }
            };
            fill(&mut w, d);
            // Сглаживание ~±20 м: перепад растягивается в слив, а не ступень
            // (монотонный профиль после усреднения остаётся монотонным).
            for _ in 0..24 {
                let src = w.clone();
                for k in 1..n - 1 {
                    w[k] = src[k - 1] * 0.25 + src[k] * 0.5 + src[k + 1] * 0.25;
                }
                fill(&mut w, d * 0.5);
            }
            // Устье: не ниже моря — река переходит в его поверхность.
            for x in &mut w {
                *x = x.max(crate::world::generator::SEA_LEVEL + 0.05);
            }
            for (j, &i) in segs.iter().enumerate() {
                out[i] = [w[j], w[j + 1], dd, 0.0];
            }
        }
        out
    }

    /// Вода реки над точкой `p` (03.6, сессия 4): уровень, м, и скорость
    /// течения (x, z), м/с — вниз по уровню (на плёсе — по дну), величина
    /// 0,6 м/с + уклон поверхности × 15, до 3,5 м/с; у берега (край полосы)
    /// течение слабеет. `None` — не в реке или рельеф выше воды.
    pub fn water_at(
        &self,
        set: &FeatureSet,
        levels: &[[f32; 4]],
        p: Vec2,
        ground: f32,
    ) -> Option<(f32, Vec2)> {
        let mut best: Option<(f32, f32, Vec2)> = None;
        for r in &self.reaches {
            for i in r.segs.clone() {
                let (Some(s), Some(lv)) = (set.segments.get(i), levels.get(i)) else {
                    continue;
                };
                let (a, b) = (Vec2::from(s.a), Vec2::from(s.b));
                let ab = b - a;
                let len = ab.length().max(0.1);
                let t = ((p - a).dot(ab) / (len * len)).clamp(0.0, 1.0);
                let d = p.distance(a + ab * t);
                let half = s.half_width + lv[2] * 2.5;
                if d > half {
                    continue;
                }
                let w = lv[0] + (lv[1] - lv[0]) * t;
                if w <= ground || best.is_some_and(|(bd, _, _)| d >= bd) {
                    continue;
                }
                let bed = self.bed.get(i).copied().unwrap_or([lv[0], lv[1]]);
                let down = if (lv[0] - lv[1]).abs() > 1e-3 {
                    lv[0] > lv[1]
                } else {
                    bed[0] >= bed[1]
                };
                let dir = if down { ab / len } else { -ab / len };
                let slope = (lv[0] - lv[1]).abs() / len;
                let speed = (0.6 + slope * 15.0).min(3.5) * (1.0 - (d / half).powi(2)).max(0.15);
                best = Some((d, w, dir * speed));
            }
        }
        best.map(|(_, w, f)| (w, f))
    }

    /// Уровень воды реки над точкой `p` (м) — по профилю `levels`
    /// (`Hydrology::levels`), если точка в полосе разлива реки (полуширина +
    /// подъём × 2,5, как у ленты в `river.metal`) и рельеф под ней ниже уровня.
    pub fn level_at(
        &self,
        set: &FeatureSet,
        levels: &[[f32; 4]],
        p: Vec2,
        ground: f32,
    ) -> Option<f32> {
        let mut best: Option<(f32, f32)> = None;
        for r in &self.reaches {
            for i in r.segs.clone() {
                let (Some(s), Some(lv)) = (set.segments.get(i), levels.get(i)) else {
                    continue;
                };
                let reach = lv[2] * 2.5;
                let (a, b) = (Vec2::from(s.a), Vec2::from(s.b));
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                let d = p.distance(a + ab * t);
                if d > s.half_width + reach {
                    continue;
                }
                let w = lv[0] + (lv[1] - lv[0]) * t;
                if w > ground && best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, w));
                }
            }
        }
        best.map(|(_, w)| w)
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

    #[test]
    fn water_level_is_flat_in_pools_and_never_uphill() {
        // Река из 6 отрезков, дно падает к x = 0.
        let json = r#"{"rivers": [{"kind": "river", "width_m": 8.0,
            "line": [[600, 500], [500, 500], [400, 500], [300, 500], [200, 500], [100, 500], [0, 500]]}],
            "trails": [], "roads": []}"#;
        let set = FeatureSet::from_json(json, 2048.0).unwrap();
        let mut h = Hydrology::build(&set, |p| Some(p.x * 0.05), None);
        // Мелкая яма 0,6 м в точке x = 300 (конец отрезка 2, начало 3) —
        // заполняется; горб 20 м на x = 100 не поднимает воду выше по течению
        // больше чем на `POOL_MAX_M`.
        h.bed[2][1] -= 0.6;
        h.bed[3][0] -= 0.6;
        h.bed[4][1] += 20.0;
        h.bed[5][0] += 20.0;
        let lv = h.levels(set.segments.len(), 1.0);
        for i in 0..6 {
            assert!(
                lv[i][0] >= h.bed[i][0] - 1e-3 && lv[i][1] >= h.bed[i][1] - 1e-3,
                "не ниже дна"
            );
        }
        // Над ямой уровень не проваливается вслед за дном.
        assert!(lv[2][1] >= lv[3][1] - 1e-3, "яма: {lv:?}");
        // Выше горба вода — не выше дна + глубина + подпор.
        let d = h.reaches[0].depth.max(LEVEL_MIN_M) + 0.12;
        assert!(
            lv[3][1] <= h.bed[3][1] + d + POOL_MAX_M + 1e-3,
            "висит над горбом: {lv:?}, дно {:?}",
            h.bed
        );
    }
}
