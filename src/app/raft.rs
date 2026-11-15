//! Демо-сплав по реке (03.2): камера плывёт вниз по течению по цепочке
//! участков рек (`Hydrology`), на высоте плота над уровнем воды, со скоростью
//! по уклону, смотрит вперёд по руслу и покачивается на волне.

use glam::{Vec2, Vec3};

use crate::hydrology::Hydrology;
use crate::sim::CameraState;
use crate::world::{Ground, World};

/// Множитель уровня воды демо (тот же в `river.metal`, `LEVEL_SCALE`).
pub const LEVEL_SCALE: f32 = 3.0;
/// Глаза над водой на плоту, м.
const EYE_M: f32 = 1.0;

pub struct Raft {
    /// Ось реки вниз по течению: точка, отрезок и его конец (0 — a, 1 — b).
    path: Vec<(Vec2, usize, usize)>,
    /// Пройдено вдоль пути, м.
    s: f32,
    y: f32,
    t: f32,
    /// Нырок (клавиша Z): глаза на 1,5 м под водой.
    pub dive: bool,
}

impl Raft {
    /// Путь от ближайшей к `start` реки шириной от 3 м вниз по цепочке притоков.
    pub fn new(world: &World, hydro: &Hydrology, start: Vec3) -> Option<Raft> {
        let set = world.features.as_ref()?;
        let p0 = Vec2::new(start.x, start.z);
        let first = hydro
            .reaches
            .iter()
            .enumerate()
            .filter(|(_, r)| r.width_m >= 3.0)
            .min_by(|(_, a), (_, b)| {
                let da = a.mouth.distance(p0) + a.length_m * 0.0;
                let db = b.mouth.distance(p0);
                da.total_cmp(&db)
            })?
            .0;
        let mut path: Vec<(Vec2, usize, usize)> = Vec::new();
        let mut cur = Some(first);
        let mut hops = 0;
        while let Some(k) = cur {
            let r = &hydro.reaches[k];
            let segs = &set.segments[r.segs.clone()];
            let mut pts: Vec<(Vec2, usize, usize)> = r
                .segs
                .clone()
                .map(|i| (Vec2::from(set.segments[i].a), i, 0))
                .collect();
            pts.push((Vec2::from(segs[segs.len() - 1].b), r.segs.end - 1, 1));
            // Вниз по течению — к устью участка.
            if pts[0].0.distance(r.mouth) < pts[pts.len() - 1].0.distance(r.mouth) {
                pts.reverse();
            }
            // В следующем участке — с точки, ближайшей к концу пути.
            let from = path.last().map_or(0, |(e, _, _)| {
                (0..pts.len())
                    .min_by(|&i, &j| pts[i].0.distance(*e).total_cmp(&pts[j].0.distance(*e)))
                    .unwrap_or(0)
            });
            path.extend(pts[from..].iter().copied());
            cur = r.into;
            hops += 1;
            if hops > 40 {
                break;
            }
        }
        if path.len() < 2 {
            return None;
        }
        // Старт — точка пути, ближайшая к игроку.
        let mut s = 0.0;
        let mut best = (f32::INFINITY, 0.0);
        for w in path.windows(2) {
            let d = w[0].0.distance(p0);
            if d < best.0 {
                best = (d, s);
            }
            s += w[0].0.distance(w[1].0);
        }
        eprintln!("сплав: путь {:.0} м по {hops} участкам рек", s);
        Some(Raft {
            path,
            s: best.1,
            y: start.y,
            t: 0.0,
            dive: false,
        })
    }

    /// Ось реки от старта вниз по течению с уровнем воды, шаг ~4 м (05.6).
    pub fn sampled(&self, levels: &[[f32; 4]], len_m: f32) -> Vec<(Vec2, f32)> {
        let mut out = Vec::new();
        let mut s = self.s;
        while s < self.s + len_m {
            out.push(self.at(s, levels));
            s += 4.0;
        }
        out
    }

    /// Точка старта на оси реки и направление вниз по течению (для `--swim`).
    pub fn start(&self) -> (Vec2, Vec2) {
        let lv: Vec<[f32; 4]> = Vec::new();
        let (p, _) = self.at(self.s, &lv);
        let (q, _) = self.at(self.s + 2.0, &lv);
        (p, (q - p).normalize_or(Vec2::X))
    }

    /// Точка и уровень воды на расстоянии `s` вдоль пути (`levels` —
    /// `Hydrology::levels`, тот же профиль, что рисует реку).
    fn at(&self, s: f32, levels: &[[f32; 4]]) -> (Vec2, f32) {
        let lv = |e: &(Vec2, usize, usize)| levels.get(e.1).map_or(0.0, |l| l[e.2]);
        let mut acc = 0.0;
        for w in self.path.windows(2) {
            let l = w[0].0.distance(w[1].0);
            if acc + l >= s {
                let t = ((s - acc) / l.max(1e-4)).clamp(0.0, 1.0);
                return (
                    w[0].0.lerp(w[1].0, t),
                    lv(&w[0]) + (lv(&w[1]) - lv(&w[0])) * t,
                );
            }
            acc += l;
        }
        let e = self.path.last().unwrap();
        (e.0, lv(e))
    }

    /// Шаг сплава на `dt` реальных секунд; возвращает камеру.
    pub fn step(&mut self, world: &World, dt: f32, levels: &[[f32; 4]]) -> CameraState {
        self.t += dt;
        let ground = |p: Vec2| world.sample_ground(p.x, p.y).map_or(0.0, |g| g.height);
        let (p, water0) = self.at(self.s, levels);
        let (_, water_ahead) = self.at(self.s + 6.0, levels);
        // Скорость — по уклону воды: плёс медленно, слив быстро.
        let slope = ((water0 - water_ahead) / 6.0).max(0.0);
        let speed = (1.5_f32 + slope * 25.0).min(5.0);
        self.s += speed * dt;
        // Уровни ещё не пришли (первые кадры) — по рельефу.
        let water = if water0 > 0.0 {
            water0
        } else {
            ground(p) + 0.5
        };
        let eye = if self.dive { -1.5 } else { EYE_M };
        let target = water + eye + (self.t * 1.7).sin() * 0.06;
        self.y += (target - self.y) * (1.0 - (-dt * 4.0).exp());
        let (look, water_look) = self.at(self.s + 14.0, levels);
        let eye = Vec3::new(p.x, self.y, p.y);
        let to = Vec3::new(look.x, water_look.max(ground(look)) + 0.3, look.y) - eye;
        let f = to.normalize_or(Vec3::NEG_Z);
        CameraState {
            position: eye,
            yaw: (-f.x).atan2(-f.z),
            pitch: f.y.asin().clamp(-0.6, 0.3) + (self.t * 1.3).sin() * 0.01,
        }
    }
}
