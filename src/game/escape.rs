//! Ночной побег от дрона (05.6, сессия 1): дрон с прожектором ищет игрока в
//! тёмном лесу; светящиеся грибы вдоль реки указывают дорогу к водопаду, где
//! дрон теряет след.
//!
//! - Дрон летит над кронами (28 м над рельефом), ведёт пятно прожектора по
//!   кругу вокруг точки поиска; точка — рядом с игроком с ошибкой, которая
//!   сужается, пока дрон что-то замечает («тепловые подсказки»).
//! - Обнаружение: игрок в конусе луча и не скрыт. Кроны над игроком прячут
//!   большую часть следа, вода (нырок) — весь. Тревога растёт в луче и спадает
//!   вне его; на 1 — пойман: обратно к последнему достигнутому грибу-маяку.
//! - Тропа — ось реки от начала тропы вниз по течению (`Raft`), грибы — по
//!   берегу через 7 м; волна свечения бежит от игрока к водопаду: лес
//!   «показывает дорогу». Водопад — первый крутой перепад уровня реки.

use glam::{Vec2, Vec3};

use super::hash;
use crate::layout::PlantInstance;
use crate::world::{Ground, World};

/// Высота полёта над рельефом, м; скорость поиска и погони, м/с.
const DRONE_ALT_M: f32 = 28.0;
const SEARCH_SPEED: f32 = 5.0;
const CHASE_SPEED: f32 = 7.5;
/// Половина угла конуса прожектора, рад; дальность обнаружения, м.
pub const BEAM_HALF_RAD: f32 = 0.24;
const DETECT_RANGE_M: f32 = 80.0;
/// Сила прожектора (в единицах `spot_light`: освещённость × d²).
pub const BEAM_INTENSITY: f32 = 0.05;
/// Рост тревоги в луче на открытом месте, 1/с; спад вне луча, 1/с.
const ALERT_RISE: f32 = 0.9;
const ALERT_DECAY: f32 = 0.25;
/// Шаг грибов-маяков вдоль реки, м; отступ от оси к берегу, м.
const GUIDE_STEP_M: f32 = 7.0;
/// Водопад: перепад уровня на 20 м пути, м; поиск — не дальше, м.
const FALL_DROP_M: f32 = 2.5;
const FALL_SEARCH_M: f32 = 900.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Playing,
    /// Пойман: секунд до возврата к маяку.
    Caught(f32),
    /// У водопада: дрон потерял след.
    Escaped,
}

pub struct Drone {
    pub pos: Vec3,
    /// Куда смотрит луч (нормированное).
    pub beam: Vec3,
    /// Точка поиска на земле (xz) и время до её смены, с.
    search: Vec2,
    retarget_s: f32,
    /// Угол обхода пятна вокруг точки поиска, рад.
    sweep: f32,
    pub alert: f32,
    seed: u32,
}

pub struct NightEscape {
    pub drone: Drone,
    /// Маяки (грибы) вдоль реки к водопаду.
    pub guide: Vec<Vec3>,
    pub goal: Vec3,
    /// Последний пройденный маяк — точка возврата после поимки.
    pub checkpoint: usize,
    pub phase: Phase,
    time: f32,
}

impl NightEscape {
    /// Уровень по оси реки `path` (точки вниз по течению, уровень воды в них).
    pub fn new(world: &World, path: &[(Vec2, f32)]) -> Option<NightEscape> {
        if path.len() < 4 {
            return None;
        }
        // Водопад: первый участок с перепадом уровня > FALL_DROP_M на 20 м,
        // не ближе 120 м от старта; иначе — конец поиска.
        let mut acc = vec![0.0f32; path.len()];
        for i in 1..path.len() {
            acc[i] = acc[i - 1] + path[i].0.distance(path[i - 1].0);
        }
        let mut goal_i = path.len() - 1;
        'find: for i in 0..path.len() {
            if acc[i] > FALL_SEARCH_M {
                goal_i = i;
                break;
            }
            if acc[i] < 120.0 {
                continue;
            }
            for j in i + 1..path.len() {
                if acc[j] - acc[i] > 20.0 {
                    if path[i].1 - path[j].1 > FALL_DROP_M {
                        goal_i = i;
                        break 'find;
                    }
                    break;
                }
            }
        }
        let ground = |p: Vec2| world.sample_ground(p.x, p.y).map_or(0.0, |g| g.height);
        // Маяки по берегу: отступ поперёк течения, сторона чередуется редко.
        let mut guide = Vec::new();
        let mut next = 0.0;
        for i in 1..=goal_i {
            if acc[i] < next {
                continue;
            }
            next = acc[i] + GUIDE_STEP_M;
            let dir = (path[i].0 - path[i - 1].0).normalize_or(Vec2::X);
            let side = if (i / 9) % 2 == 0 { 1.0 } else { -1.0 };
            // Сухой берег: отступ от оси, пока земля не выше воды на 0,3 м
            // (разлив демо затопил бы маяки), сначала с выбранной стороны.
            let n = Vec2::new(-dir.y, dir.x);
            let level = path[i].1;
            let dry = [side, -side].iter().find_map(|&sd| {
                (4..30)
                    .map(|k| path[i].0 + n * sd * k as f32)
                    .find(|q| ground(*q) > level + 0.3)
            });
            let Some(p) = dry else {
                continue;
            };
            guide.push(Vec3::new(p.x, ground(p), p.y));
        }
        let g = path[goal_i].0;
        let goal = Vec3::new(g.x, ground(g), g.y);
        let start = path[0].0;
        Some(NightEscape {
            drone: Drone {
                pos: Vec3::new(start.x + 60.0, ground(start) + DRONE_ALT_M, start.y + 40.0),
                beam: Vec3::NEG_Y,
                // Начало: дрон прочёсывает тропу впереди игрока.
                search: guide.get(3).map_or(start, |g: &Vec3| Vec2::new(g.x, g.z)),
                retarget_s: 12.0,
                sweep: 0.0,
                alert: 0.0,
                seed: 0x0D0E,
            },
            guide,
            goal,
            checkpoint: 0,
            phase: Phase::Playing,
            time: 0.0,
        })
    }

    /// Точка возврата после поимки: у последнего пройденного маяка.
    pub fn respawn(&self) -> Vec3 {
        self.guide
            .get(self.checkpoint)
            .copied()
            .unwrap_or(self.goal)
    }

    /// Шаг уровня. `cover` — доля крон над игроком 0…1, `underwater` — глаза
    /// под водой. Возвращает точку, куда вернуть игрока (поимка).
    pub fn update(
        &mut self,
        world: &World,
        dt: f32,
        player: Vec3,
        cover: f32,
        underwater: bool,
    ) -> Option<Vec3> {
        self.time += dt;
        let p2 = Vec2::new(player.x, player.z);
        // Маяки: дошёл — следующий становится точкой возврата.
        while self.checkpoint + 1 < self.guide.len()
            && Vec2::new(
                self.guide[self.checkpoint + 1].x,
                self.guide[self.checkpoint + 1].z,
            )
            .distance(p2)
                < 6.0
        {
            self.checkpoint += 1;
        }
        match self.phase {
            Phase::Escaped => {
                // Дрон уходит прочь и гаснет.
                self.drone.pos += Vec3::new(6.0, 1.0, -4.0) * dt;
                return None;
            }
            Phase::Caught(t) => {
                let t = t - dt;
                if t > 0.0 {
                    self.phase = Phase::Caught(t);
                    return None;
                }
                self.phase = Phase::Playing;
                self.drone.alert = 0.0;
                // Дрон отлетает искать заново.
                self.drone.pos += Vec3::new(50.0, 0.0, -50.0);
                return Some(self.respawn());
            }
            Phase::Playing => {}
        }
        if Vec2::new(self.goal.x, self.goal.z).distance(p2) < 9.0 {
            self.phase = Phase::Escaped;
            return None;
        }
        let d = &mut self.drone;
        // Точка поиска: рядом с игроком, ошибка сужается с тревогой.
        d.retarget_s -= dt;
        if d.retarget_s <= 0.0 || d.alert > 0.5 {
            d.seed = d.seed.wrapping_add(1);
            let h = hash(d.seed as i32, 7, 0xD7);
            let a = (h & 0xffff) as f32 / 65535.0 * std::f32::consts::TAU;
            let r =
                35.0 * (1.0 - d.alert).max(0.1) * (0.4 + 0.6 * ((h >> 16) & 0xff) as f32 / 255.0);
            d.search = p2 + Vec2::new(a.cos(), a.sin()) * r;
            d.retarget_s = 7.0;
        }
        // Пятно луча обходит точку поиска по кругу; при тревоге — прямо на игрока.
        d.sweep += dt * 0.7;
        let spot = if d.alert > 0.6 {
            p2
        } else {
            d.search + Vec2::new(d.sweep.cos(), d.sweep.sin()) * 9.0
        };
        let ground = |p: Vec2| world.sample_ground(p.x, p.y).map_or(0.0, |g| g.height);
        // Полёт: к точке над поиском (чуть позади пятна — луч наклонный).
        let hover =
            d.search + (Vec2::new(d.pos.x, d.pos.z) - d.search).normalize_or(Vec2::X) * 12.0;
        let target = Vec3::new(hover.x, ground(hover) + DRONE_ALT_M, hover.y);
        let speed = if d.alert > 0.5 {
            CHASE_SPEED
        } else {
            SEARCH_SPEED
        };
        let to = target - d.pos;
        d.pos += to.clamp_length_max(speed * dt);
        d.pos.y += (ground(Vec2::new(d.pos.x, d.pos.z)) + DRONE_ALT_M - d.pos.y).max(0.0)
            * (1.0 - (-dt * 2.0).exp());
        let spot3 = Vec3::new(spot.x, ground(spot), spot.y);
        let want = (spot3 - d.pos).normalize_or(Vec3::NEG_Y);
        d.beam = d
            .beam
            .lerp(want, 1.0 - (-dt * 3.0).exp())
            .normalize_or(Vec3::NEG_Y);
        // Обнаружение: в конусе, в дальности, не под водой; кроны прячут.
        let to_p = player - d.pos;
        let in_cone = to_p.length() < DETECT_RANGE_M
            && to_p.normalize_or_zero().dot(d.beam) > BEAM_HALF_RAD.cos();
        let visible = if underwater {
            0.0
        } else {
            1.0 - 0.85 * cover.clamp(0.0, 1.0)
        };
        if in_cone && visible > 0.0 {
            d.alert = (d.alert + ALERT_RISE * visible * dt).min(1.0);
        } else {
            d.alert = (d.alert - ALERT_DECAY * dt).max(0.0);
        }
        if d.alert >= 1.0 {
            self.phase = Phase::Caught(2.0);
        }
        None
    }

    /// Грибы-маяки и огни дрона для отрисовки (`PlantInstance`, вид гриба 0 —
    /// светящийся мицен; 8 и 10 — светящиеся бусины).
    pub fn instances(&self, camera: Vec3) -> Vec<PlantInstance> {
        let mut out = Vec::new();
        let near = Vec2::new(camera.x, camera.z);
        for (i, g) in self.guide.iter().enumerate() {
            if Vec2::new(g.x, g.z).distance(near) > 120.0 {
                continue;
            }
            // Волна свечения от игрока к водопаду: «лес показывает дорогу».
            let ahead = i as f32 - self.checkpoint as f32;
            let wave = ((ahead * 0.6 - self.time * 2.2).rem_euclid(9.0) - 4.5).abs();
            let pulse = (1.0 - wave / 1.6).max(0.0);
            let base = if ahead >= -1.0 { 1.0 } else { 0.35 };
            for k in 0..3u32 {
                let h = hash(i as i32, k as i32, 0x6F);
                let a = (h & 0xff) as f32 / 255.0 * std::f32::consts::TAU;
                let r = 0.15 + ((h >> 8) & 0xff) as f32 / 255.0 * 0.35;
                out.push(PlantInstance {
                    position: (*g + Vec3::new(a.cos() * r, 0.0, a.sin() * r)).to_array(),
                    scale: 2.0 + (k as f32) * 0.6,
                    yaw: a,
                    tint: (0.6 + 2.5 * pulse) * base,
                    phase: 0.0,
                    _pad: 0.0,
                });
            }
            // Споры над маяками впереди — зелёные бусины, видны над травой.
            if ahead >= -1.0 {
                for k in 0..3u32 {
                    let h = hash(i as i32, k as i32 + 7, 0x5B);
                    let a = (h & 0xff) as f32 / 255.0 * std::f32::consts::TAU + self.time * 0.3;
                    let y = 0.8
                        + 0.5 * (((h >> 8) & 0xff) as f32 / 255.0)
                        + 0.15 * (self.time * 1.3 + k as f32).sin();
                    out.push(PlantInstance {
                        position: (*g + Vec3::new(a.cos() * 0.4, y, a.sin() * 0.4)).to_array(),
                        scale: 0.5,
                        yaw: 0.0,
                        tint: 0.35 + 1.4 * pulse,
                        phase: 0.0,
                        _pad: 8.0,
                    });
                }
            }
        }
        // Огни дрона: белый прожектор и мигающий красный (тело — сессия 2).
        if self.phase != Phase::Escaped || self.drone.pos.distance(camera) < 400.0 {
            let d = &self.drone;
            out.push(PlantInstance {
                position: d.pos.to_array(),
                scale: 4.0,
                yaw: 0.0,
                tint: 3.0,
                phase: 0.0,
                _pad: 10.0,
            });
            if (self.time * 1.5).fract() < 0.2 {
                out.push(PlantInstance {
                    position: (d.pos + Vec3::new(0.6, 0.2, 0.0)).to_array(),
                    scale: 2.0,
                    yaw: 0.0,
                    tint: 2.0,
                    phase: 0.0,
                    _pad: 9.0,
                });
            }
        }
        out
    }

    /// Прожектор для `FrameUniforms` (относительно `cam_hi`).
    pub fn spot(&self, cam_hi: Vec3) -> ([f32; 4], [f32; 4]) {
        if self.phase == Phase::Escaped {
            return ([0.0; 4], [0.0; 4]);
        }
        let p = self.drone.pos - cam_hi;
        (
            [p.x, p.y, p.z, BEAM_HALF_RAD.cos()],
            [
                self.drone.beam.x,
                self.drone.beam.y,
                self.drone.beam.z,
                BEAM_INTENSITY,
            ],
        )
    }

    /// Строка состояния для интерфейса.
    pub fn status(&self) -> String {
        match self.phase {
            Phase::Escaped => "Шум водопада скрыл тебя. Дрон потерял след.".to_string(),
            Phase::Caught(_) => {
                "Прожектор нашёл тебя! Назад к последнему светящемуся грибу…".to_string()
            }
            Phase::Playing => {
                let a = self.drone.alert;
                let s = if a > 0.6 {
                    "дрон видит тебя — уходи под кроны или в воду!"
                } else if a > 0.1 {
                    "дрон что-то заметил"
                } else {
                    "дрон ищет"
                };
                format!(
                    "Иди за светящимися грибами к водопаду · {s} · тревога {:.0} % · маяк {}/{}",
                    a * 100.0,
                    self.checkpoint + 1,
                    self.guide.len()
                )
            }
        }
    }
}
