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
//! - Маршрут — по тропам от леса мха (Кокэмуси-но-мори) к водопаду Хирю-отоси
//!   (`trail_route`, Дейкстра по графу троп), грибы — у края тропы через 7 м;
//!   волна свечения бежит от игрока к водопаду: лес «показывает дорогу».

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
const ALERT_RISE: f32 = 0.45;
const ALERT_DECAY: f32 = 0.25;
fn smoothstep01(x: f32) -> f32 {
    let t = x.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Шаг грибов-маяков вдоль реки, м; отступ от оси к берегу, м.
const GUIDE_STEP_M: f32 = 7.0;

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
    /// Связь с мицелием (сессия 2): 0 — нет, 1 — нити видны; секунд с касания
    /// первого гриба (`None` — ещё не касался); сила галлюциногенного эффекта.
    pub connected: f32,
    contact: Option<f32>,
    pub trip: f32,
}

impl NightEscape {
    /// Уровень по маршруту `route` (тропы от старта к цели): маяки — у края
    /// тропы через `GUIDE_STEP_M`, цель — конец маршрута.
    pub fn on_route(world: &World, route: &[Vec2]) -> Option<NightEscape> {
        if route.len() < 2 {
            return None;
        }
        let ground = |p: Vec2| world.sample_ground(p.x, p.y).map_or(0.0, |g| g.height);
        let mut guide = Vec::new();
        let mut acc = 0.0;
        let mut next = 4.0;
        for w in route.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = a.distance(b);
            let dir = (b - a).normalize_or(Vec2::X);
            while next <= acc + len {
                let p = a + dir * (next - acc);
                // У края тропы, сторона меняется каждые ~60 м.
                let side = if (next as usize / 60).is_multiple_of(2) {
                    1.0
                } else {
                    -1.0
                };
                let q = p + Vec2::new(-dir.y, dir.x) * side * 1.6;
                guide.push(Vec3::new(q.x, ground(q), q.y));
                next += GUIDE_STEP_M;
            }
            acc += len;
        }
        let g = *route.last().unwrap();
        let goal = Vec3::new(g.x, ground(g), g.y);
        Some(Self::assemble(world, route[0], guide, goal))
    }

    fn assemble(world: &World, start: Vec2, guide: Vec<Vec3>, goal: Vec3) -> NightEscape {
        let ground = |p: Vec2| world.sample_ground(p.x, p.y).map_or(0.0, |g| g.height);
        NightEscape {
            drone: Drone {
                pos: Vec3::new(start.x + 60.0, ground(start) + DRONE_ALT_M, start.y + 40.0),
                beam: Vec3::NEG_Y,
                // Начало: дрон прочёсывает тропу впереди игрока.
                search: guide
                    .get(7)
                    .or(guide.last())
                    .map_or(start, |g: &Vec3| Vec2::new(g.x, g.z)),
                retarget_s: 15.0,
                sweep: 0.0,
                alert: 0.0,
                seed: 0x0D0E,
            },
            guide,
            goal,
            checkpoint: 0,
            phase: Phase::Playing,
            time: 0.0,
            connected: 0.0,
            contact: None,
            trip: 0.0,
        }
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
        // Соединение с мицелием: касание первого светящегося гриба — споры,
        // переход (2 с нарастания, 3 с пика, 6 с спада до фона), нити
        // проступают под почвой; эффект сильнее у маяков.
        if self.contact.is_none()
            && let Some(g) = self.guide.first()
            && Vec2::new(g.x, g.z).distance(p2) < 2.5
        {
            self.contact = Some(0.0);
        }
        if let Some(c) = self.contact.as_mut() {
            *c += dt;
            let c = *c;
            let peak = smoothstep01(c / 2.0) * (1.0 - smoothstep01((c - 5.0) / 6.0));
            let near = self
                .guide
                .iter()
                .map(|g| Vec2::new(g.x, g.z).distance(p2))
                .fold(f32::INFINITY, f32::min);
            let node = 0.15 * (1.0 - (near / 6.0).min(1.0));
            self.trip = (peak + 0.06 + node).min(1.0);
            self.connected = smoothstep01((c - 1.5) / 4.0);
        }
        if self.connected < 0.5 {
            // Дрон появляется после соединения: сначала — гриб и переход.
            return None;
        }
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
            // До соединения светится только первый гриб — зовёт к себе.
            if i > 0 && self.connected < 0.05 {
                continue;
            }
            // Волна свечения от игрока к водопаду: «лес показывает дорогу».
            let ahead = i as f32 - self.checkpoint as f32;
            let wave = ((ahead * 0.6 - self.time * 2.2).rem_euclid(9.0) - 4.5).abs();
            let pulse = (1.0 - wave / 1.6).max(0.0);
            let base = if ahead >= -1.0 { 1.0 } else { 0.35 };
            // До соединения первый гриб зовёт — ровное яркое свечение.
            let pulse = if self.connected < 0.05 {
                0.8 + 0.2 * (self.time * 2.0).sin()
            } else {
                pulse
            };
            for k in 0..3u32 {
                let h = hash(i as i32, k as i32, 0x6F);
                let a = (h & 0xff) as f32 / 255.0 * std::f32::consts::TAU;
                let r = 0.15 + ((h >> 8) & 0xff) as f32 / 255.0 * 0.35;
                out.push(PlantInstance {
                    position: (*g + Vec3::new(a.cos() * r, 0.0, a.sin() * r)).to_array(),
                    scale: 0.8 + (k as f32) * 0.2,
                    yaw: a,
                    tint: (0.6 + 2.5 * pulse) * base,
                    phase: 0.0,
                    _pad: 0.0,
                });
            }
        }
        // Огни дрона: белый прожектор и мигающий красный (тело — сессия 4).
        if self.connected >= 0.5
            && (self.phase != Phase::Escaped || self.drone.pos.distance(camera) < 400.0)
        {
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
        if self.phase == Phase::Escaped || self.connected < 0.5 {
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

    /// Связь с мицелием для `FrameUniforms.mycelium` (цель — водопад).
    pub fn mycelium(&self, cam_hi: Vec3) -> [f32; 4] {
        [
            self.connected,
            self.trip,
            self.goal.x - cam_hi.x,
            self.goal.z - cam_hi.z,
        ]
    }

    /// Проверка: как будто гриба коснулись `t` секунд назад.
    pub fn debug_contact(&mut self, t: f32) {
        self.contact = Some(t);
    }

    /// Рука тянется к грибу в первые секунды касания.
    pub fn reaching(&self) -> bool {
        self.contact.is_some_and(|c| c < 2.5)
    }

    /// Строка состояния для интерфейса.
    pub fn status(&self) -> String {
        match self.contact {
            None => return "У ручья светится гриб. Подойди и коснись его.".to_string(),
            Some(c) if c < 6.0 => {
                return "Споры… земля под ногами начинает светиться. Лес говорит с тобой."
                    .to_string();
            }
            _ => {}
        }
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

/// Кратчайший путь по тропам и дорогам (`FeatureSet`) от `from` к `to`:
/// граф из концов отрезков (сшиты с точностью 1 м), Дейкстра. Концы — к
/// ближайшим узлам; первая и последняя точки маршрута — сами `from` и `to`.
/// Маршрут: точки от старта к цели и отрезки объектов карты по порядку —
/// (индекс, цель ближе к концу `b`, путь до цели от конца `a`, м).
pub struct Route {
    pub points: Vec<Vec2>,
    pub segs: Vec<(usize, bool, f32)>,
}

impl Route {
    /// Служебные поля `FeatureSegment._pad` для отрезков маршрута (05.6):
    /// `[1 или 2, путь от a]` — шейдер рельефа ведёт по ним нити мицелия.
    pub fn pads(&self) -> impl Iterator<Item = (usize, [f32; 2])> + '_ {
        self.segs
            .iter()
            .map(|&(i, to_b, left_a)| (i, [if to_b { 1.0 } else { 2.0 }, left_a]))
    }
}

pub fn trail_route(
    set: &crate::world::features::FeatureSet,
    from: Vec2,
    to: Vec2,
) -> Option<Route> {
    use crate::world::features::{KIND_ROAD, KIND_TRAIL};
    use std::collections::{BinaryHeap, HashMap};
    let key = |p: [f32; 2]| ((p[0]).round() as i64, (p[1]).round() as i64);
    let mut ids: HashMap<(i64, i64), usize> = HashMap::new();
    let mut pos: Vec<Vec2> = Vec::new();
    let mut adj: Vec<Vec<(usize, f32, usize)>> = Vec::new();
    let node = |p: [f32; 2],
                ids: &mut HashMap<(i64, i64), usize>,
                pos: &mut Vec<Vec2>,
                adj: &mut Vec<Vec<(usize, f32, usize)>>| {
        *ids.entry(key(p)).or_insert_with(|| {
            pos.push(Vec2::from(p));
            adj.push(Vec::new());
            pos.len() - 1
        })
    };
    for (si, s) in set.segments.iter().enumerate() {
        if s.kind != KIND_TRAIL && s.kind != KIND_ROAD {
            continue;
        }
        let a = node(s.a, &mut ids, &mut pos, &mut adj);
        let b = node(s.b, &mut ids, &mut pos, &mut adj);
        let w = pos[a].distance(pos[b]) * if s.kind == KIND_ROAD { 1.5 } else { 1.0 };
        adj[a].push((b, w, si));
        adj[b].push((a, w, si));
    }
    let nearest =
        |p: Vec2| (0..pos.len()).min_by(|&i, &j| pos[i].distance(p).total_cmp(&pos[j].distance(p)));
    let (s0, t0) = (nearest(from)?, nearest(to)?);
    #[derive(PartialEq)]
    struct Item(f32, usize);
    impl Eq for Item {}
    impl PartialOrd for Item {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Item {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            o.0.total_cmp(&self.0)
        }
    }
    let mut dist = vec![f32::INFINITY; pos.len()];
    let mut prev = vec![(usize::MAX, usize::MAX); pos.len()];
    let mut heap = BinaryHeap::new();
    dist[s0] = 0.0;
    heap.push(Item(0.0, s0));
    while let Some(Item(d, u)) = heap.pop() {
        if u == t0 {
            break;
        }
        if d > dist[u] {
            continue;
        }
        for &(v, w, si) in &adj[u] {
            if d + w < dist[v] {
                dist[v] = d + w;
                prev[v] = (u, si);
                heap.push(Item(d + w, v));
            }
        }
    }
    if !dist[t0].is_finite() {
        return None;
    }
    // Назад от цели: точки и отрезки; путь до цели копится от конца.
    let mut points = vec![to];
    let mut segs = Vec::new();
    let mut left = pos[t0].distance(to);
    let mut u = t0;
    while u != usize::MAX {
        points.push(pos[u]);
        let (p, si) = prev[u];
        if p != usize::MAX {
            let s = &set.segments[si];
            let len = Vec2::from(s.a).distance(Vec2::from(s.b));
            // Цель ближе к концу, совпадающему с `u`.
            let to_b = Vec2::from(s.b).distance(pos[u]) < Vec2::from(s.a).distance(pos[u]);
            let left_a = if to_b { left + len } else { left };
            segs.push((si, to_b, left_a));
            left += len;
        }
        u = p;
    }
    points.push(from);
    points.reverse();
    segs.reverse();
    Some(Route { points, segs })
}
