//! Чувство мицелия (05.2): игрок удерживает C, по земле от ног бегут
//! светящиеся нити к неосмотренным грибам, следам зверей и узлу грибницы.
//! Узел у старой якусуги воспроизводит фразу профессора Мориты — первый
//! поворот главы I ([narrative.md](../../specs/narrative.md)).
//!
//! Нити — бусины из экземпляров гриба (`PlantInstance._pad` 8–10 —
//! цвет свечения, `tint` — яркость); отдельного прохода рендера нет.

use std::collections::VecDeque;

use glam::{Vec2, Vec3};

use super::{Game, hash, unit};
use crate::layout::PlantInstance;
use crate::world::{Ground, MarkerKind, World};

/// Скорость волны импульса по нити, м/с.
pub const PULSE_SPEED: f32 = 9.0;
/// Период импульсов при удержании, с.
pub const PULSE_PERIOD_S: f32 = 2.6;
/// Дальность нитей к грибам, следам и узлу, м (нить к узлу длиннее не рисуется).
const MUSHROOM_RANGE_M: f32 = 35.0;
const TRACE_RANGE_M: f32 = 45.0;
const NODE_RANGE_M: f32 = 160.0;
const THREAD_MAX_M: f32 = 45.0;
/// Шаг бусин по нити, м.
const BEAD_STEP_M: f32 = 0.25;
/// Узел отвечает, если стоять в этом радиусе и слушать дольше `NODE_LISTEN_S`.
pub const NODE_RADIUS_M: f32 = 6.0;
pub const NODE_LISTEN_S: f32 = 1.5;

/// Цвет свечения бусины (`PlantInstance._pad`): грибы, следы, узел.
pub const GLOW_MUSHROOM: f32 = 8.0;
pub const GLOW_TRACE: f32 = 9.0;
pub const GLOW_NODE: f32 = 10.0;

pub struct TraceKind {
    pub name: &'static str,
    pub note: &'static str,
}

pub const TRACE_KINDS: [TraceKind; 2] = [
    TraceKind {
        name: "Следы якусики",
        note: "Раздвоенные копытца небольшого оленя в мягкой подстилке. Тропа зверя идёт вдоль ручья.",
    },
    TraceKind {
        name: "Объеденные плоды",
        note: "Надкусанные плоды и шелуха под деревом: здесь кормились макаки якузару.",
    },
];

/// След зверя в лесу. Виден в чувстве мицелия и ещё `marked` секунд после.
pub struct Trace {
    pub position: Vec3,
    pub kind: u8,
    pub found: bool,
    pub marked: f32,
}

/// Узел грибницы у старого дерева.
pub struct Node {
    pub position: Vec3,
    pub heard: bool,
    pub hinted: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Target {
    Mushroom(usize),
    Trace(usize),
    Node,
}

struct Thread {
    /// Бусины: позиция и путь от начала, м.
    beads: Vec<(Vec3, f32)>,
    /// Полная длина до цели (может быть больше нарисованной).
    length: f32,
    target: Target,
    arrived: bool,
}

/// Реплика субтитров: говорящий, текст, время показа.
#[derive(Clone, Copy)]
pub struct Subtitle {
    pub speaker: &'static str,
    pub text: &'static str,
    pub seconds: f32,
}

/// Запись журнала помимо видов: следы, аномалии, гипотезы.
pub struct Note {
    pub title: String,
    pub text: String,
}

#[derive(Default)]
pub struct Sense {
    /// Сила чувства 0…1: растёт при удержании C, спадает после.
    pub level: f32,
    /// Сколько секунд C удерживается без перерыва.
    pub held_s: f32,
    next_pulse: f32,
    pulse_age: f32,
    threads: Vec<Thread>,
    /// Часы игры для мерцания узла, с.
    clock: f32,
}

const NODE_ECHO: [Subtitle; 3] = [
    Subtitle {
        speaker: "Голос в грибнице",
        text: "…не торопись… лес отвечает… тем, кто умеет ждать…",
        seconds: 5.5,
    },
    Subtitle {
        speaker: "Я",
        text: "Это его слова. Профессор Морита говорил так в лаборатории, над каждой чашкой с культурой.",
        seconds: 6.0,
    },
    Subtitle {
        speaker: "Я",
        text: "Грибница не может знать его голос. Значит, он был здесь. Или она… помнит?",
        seconds: 6.0,
    },
];

impl Game {
    /// Кадр чувства мицелия: импульсы, приход волны к целям, ответ узла.
    pub fn update_sense(&mut self, dt: f32, eye: Vec3, sensing: bool, world: &World) {
        let s = &mut self.sense;
        s.clock += dt;
        let rate = if sensing { 1.0 / 0.6 } else { -1.0 / 0.8 };
        s.level = (s.level + dt * rate).clamp(0.0, 1.0);
        let feet = world
            .sample_ground(eye.x, eye.z)
            .map_or(eye, |g| Vec3::new(eye.x, g.height, eye.z));
        if sensing {
            s.held_s += dt;
            s.next_pulse -= dt;
            if s.next_pulse <= 0.0 {
                s.next_pulse = PULSE_PERIOD_S;
                s.pulse_age = 0.0;
                self.sense.threads = threads(self, feet, world);
            }
        } else {
            s.held_s = 0.0;
            s.next_pulse = 0.0;
        }
        let s = &mut self.sense;
        s.pulse_age += dt;
        if s.level == 0.0 {
            s.threads.clear();
        }
        // Волна дошла до цели: цель «помечена» и светится ещё какое-то время.
        let front = s.pulse_age * PULSE_SPEED;
        for t in &mut s.threads {
            if t.arrived || front < t.length {
                continue;
            }
            t.arrived = true;
            match t.target {
                Target::Mushroom(i) => self.mushrooms[i].marked = 10.0,
                Target::Trace(i) => self.traces[i].marked = 25.0,
                Target::Node => {}
            }
        }
        for m in &mut self.mushrooms {
            m.marked = (m.marked - dt).max(0.0);
        }
        for t in &mut self.traces {
            t.marked = (t.marked - dt).max(0.0);
        }
        // Узел: подсказка рядом и эхо Мориты после долгого вслушивания.
        let held = self.sense.held_s;
        if let Some(node) = &mut self.node {
            let d = node.position.distance(feet);
            if !node.hinted && !node.heard && d < 25.0 {
                node.hinted = true;
                self.toast = Some((
                    "Здесь грибница гуще. Удерживай C, чтобы прислушаться".into(),
                    5.0,
                ));
            }
            if !node.heard && sensing && held > NODE_LISTEN_S && d < NODE_RADIUS_M {
                node.heard = true;
                self.subtitles.extend(NODE_ECHO);
                self.notes.push(Note {
                    title: "Аномалия у старого дерева".into(),
                    text: "Узел грибницы у корней старой якусуги воспроизвёл фразу профессора \
                           Мориты. Гипотеза: сеть хранит звуки или переживания людей, бывших с \
                           ней в контакте. Проверить, повторится ли эхо у другого узла."
                        .into(),
                });
            }
        }
    }

    /// Осмотр следа под взглядом: запись в журнал.
    pub fn inspect_trace(&mut self, i: usize) {
        let t = &mut self.traces[i];
        if t.found {
            return;
        }
        t.found = true;
        let k = t.kind as usize;
        let first = !self
            .traces
            .iter()
            .enumerate()
            .any(|(j, o)| j != i && o.found && o.kind as usize == k);
        let kind = &TRACE_KINDS[k];
        if first {
            self.notes.push(Note {
                title: kind.name.into(),
                text: kind.note.into(),
            });
        }
        self.toast = Some((format!("В журнале: {}", kind.name), 4.0));
    }

    /// Бусины нитей и колец вокруг помеченных целей.
    pub fn sense_beads(&self, camera: Vec3, world: &World) -> Vec<PlantInstance> {
        let s = &self.sense;
        let mut out = Vec::new();
        let front = s.pulse_age * PULSE_SPEED;
        for t in &s.threads {
            let glow = match t.target {
                Target::Mushroom(_) => GLOW_MUSHROOM,
                Target::Trace(_) => GLOW_TRACE,
                Target::Node => GLOW_NODE,
            };
            for &(p, d) in &t.beads {
                let w = (d - front) / 1.4;
                let b = s.level * (0.12 + 1.5 * (-w * w).exp());
                out.push(bead(p, b, glow, 0.55));
            }
        }
        let mut ring = |c: Vec3, r: f32, n: usize, b: f32, glow: f32, phase: f32| {
            for k in 0..n {
                let a = phase + k as f32 / n as f32 * std::f32::consts::TAU;
                let x = c.x + r * a.cos();
                let z = c.z + r * a.sin();
                let y = world.sample_ground(x, z).map_or(c.y, |g| g.height);
                out.push(bead(Vec3::new(x, y + 0.02, z), b, glow, 0.6));
            }
        };
        for m in &self.mushrooms {
            if m.marked > 0.0 && m.position.distance_squared(camera) < 60.0 * 60.0 {
                ring(
                    m.position,
                    0.2 * m.scale.max(1.0),
                    10,
                    (m.marked / 3.0).min(1.0) * 0.8,
                    GLOW_MUSHROOM,
                    s.clock * 0.5,
                );
            }
        }
        for t in &self.traces {
            if t.marked > 0.0 && t.position.distance_squared(camera) < 80.0 * 80.0 {
                ring(
                    t.position,
                    0.45,
                    14,
                    (t.marked / 5.0).min(1.0) * 0.8,
                    GLOW_TRACE,
                    s.clock * 0.3,
                );
            }
        }
        if let Some(node) = &self.node
            && node.position.distance_squared(camera) < 50.0 * 50.0
        {
            let breathe = 0.12 + 0.08 * (s.clock * 1.3).sin();
            ring(
                node.position,
                1.3,
                28,
                breathe + 0.8 * s.level,
                GLOW_NODE,
                -s.clock * 0.2,
            );
            ring(
                node.position,
                0.7,
                16,
                breathe * 0.7 + 0.6 * s.level,
                GLOW_NODE,
                s.clock * 0.35,
            );
        }
        out
    }

    /// Следующая реплика субтитров по времени.
    pub fn tick_subtitles(&mut self, dt: f32) {
        if let Some(l) = self.subtitles.front_mut() {
            l.seconds -= dt;
            if l.seconds <= 0.0 {
                self.subtitles.pop_front();
            }
        }
    }
}

fn bead(p: Vec3, brightness: f32, glow: f32, scale: f32) -> PlantInstance {
    PlantInstance {
        position: p.to_array(),
        scale,
        yaw: 0.0,
        tint: brightness,
        phase: 0.0,
        _pad: glow,
    }
}

/// Нити нового импульса: к ближайшим неосмотренным грибам, следам и узлу.
fn threads(g: &Game, feet: Vec3, world: &World) -> Vec<Thread> {
    let mut out = Vec::new();
    let mut near: Vec<(f32, Target, Vec3)> = g
        .mushrooms
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.inspected)
        .map(|(i, m)| (m.position.distance(feet), Target::Mushroom(i), m.position))
        .filter(|(d, ..)| *d < MUSHROOM_RANGE_M)
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    near.truncate(4);
    let mut traces: Vec<(f32, Target, Vec3)> = g
        .traces
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.found)
        .map(|(i, t)| (t.position.distance(feet), Target::Trace(i), t.position))
        .filter(|(d, ..)| *d < TRACE_RANGE_M)
        .collect();
    traces.sort_by(|a, b| a.0.total_cmp(&b.0));
    traces.truncate(3);
    near.extend(traces);
    if let Some(n) = &g.node {
        let d = n.position.distance(feet);
        if d < NODE_RANGE_M && d > 1.0 {
            near.push((d, Target::Node, n.position));
        }
    }
    for (k, (_, target, to)) in near.into_iter().enumerate() {
        let seed = hash(to.x as i32, to.z as i32, k as u32);
        out.push(path(world, feet, to, seed, target));
    }
    out
}

/// Извилистая нить по земле от `from` к `to`; дальше `THREAD_MAX_M` не рисуется.
fn path(world: &World, from: Vec3, to: Vec3, seed: u32, target: Target) -> Thread {
    let d = Vec2::new(to.x - from.x, to.z - from.z);
    let length = d.length().max(0.01);
    let dir = d / length;
    let perp = Vec2::new(-dir.y, dir.x);
    let drawn = length.min(THREAD_MAX_M);
    let n = (drawn / BEAD_STEP_M) as usize;
    let ph = unit(seed, 1) * std::f32::consts::TAU;
    let amp = 0.5 + 0.6 * unit(seed, 2);
    let mut beads = Vec::with_capacity(n);
    for k in 1..=n {
        let s = k as f32 * BEAD_STEP_M;
        let taper = (std::f32::consts::PI * s / length).sin();
        let wobble = (s * 0.42 + ph).sin() * amp + (s * 1.3 + ph * 2.0).sin() * 0.2;
        let p = Vec2::new(from.x, from.z) + dir * s + perp * wobble * taper;
        let Some(gs) = world.sample_ground(p.x, p.y) else {
            break;
        };
        beads.push((Vec3::new(p.x, gs.height + 0.02, p.y), s));
    }
    Thread {
        beads,
        length,
        target,
        arrived: false,
    }
}

/// Узел грибницы: в 120–450 м от начала тропы Сиратани, в 6–14 м от тропы,
/// в самом влажном кедровом месте. Без маркера тропы — нет узла.
pub fn place_node(world: &World) -> Option<Node> {
    let start = world
        .markers
        .iter()
        .find(|m| m.kind == MarkerKind::TrailStart)?;
    let c = Vec2::new(start.position.x, start.position.z);
    let mut best: Option<(f32, Vec3)> = None;
    let step = 10.0;
    let n = (450.0 / step) as i32;
    for j in -n..=n {
        for i in -n..=n {
            let p = c + Vec2::new(i as f32, j as f32) * step;
            let dist = p.distance(c);
            if !(120.0..450.0).contains(&dist) {
                continue;
            }
            let Some(g) = world.sample_ground(p.x, p.y) else {
                continue;
            };
            if g.on_trail || g.wet || g.normal.y < 0.8 {
                continue;
            }
            let near_trail = (0..8).any(|k| {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                [6.0f32, 10.0, 14.0].iter().any(|r| {
                    world
                        .sample_ground(p.x + r * a.cos(), p.y + r * a.sin())
                        .is_some_and(|s| s.on_trail)
                })
            });
            if !near_trail {
                continue;
            }
            let (_, cedar) = super::community(world, p);
            let mc = super::macro_at(world, p);
            let hollow = ((0.5 - mc[1]) * 2.5).clamp(0.0, 1.0);
            let score = cedar + 0.5 * mc[2] + 0.3 * hollow - (dist - 280.0).abs() / 600.0;
            if best.is_none_or(|(b, _)| score > b) {
                best = Some((score, Vec3::new(p.x, g.height, p.y)));
            }
        }
    }
    best.map(|(_, position)| Node {
        position,
        heard: false,
        hinted: false,
    })
}

/// Следы зверей: клетка 30 м в 700 м от начала тропы, в лесу, не на тропе.
pub fn place_traces(world: &World) -> Vec<Trace> {
    let Some(start) = world
        .markers
        .iter()
        .find(|m| m.kind == MarkerKind::TrailStart)
    else {
        return Vec::new();
    };
    let c = Vec2::new(start.position.x, start.position.z);
    let cell = 30.0;
    let n = (700.0 / cell) as i32;
    let mut out = Vec::new();
    for j in -n..=n {
        for i in -n..=n {
            let gx = (c.x / cell) as i32 + i;
            let gz = (c.y / cell) as i32 + j;
            let h = hash(gx, gz, 0x7EAC);
            if unit(h, 1) > 0.14 {
                continue;
            }
            let p = Vec2::new(
                gx as f32 + 0.2 + 0.6 * unit(h, 2),
                gz as f32 + 0.2 + 0.6 * unit(h, 3),
            ) * cell;
            if p.distance(c) > 700.0 || super::community(world, p).0 < 0.3 {
                continue;
            }
            let Some(g) = world.sample_ground(p.x, p.y) else {
                continue;
            };
            if g.on_trail || g.wet || g.normal.y < 0.7 {
                continue;
            }
            out.push(Trace {
                position: Vec3::new(p.x, g.height, p.y),
                kind: (unit(h, 4) < 0.6) as u8 ^ 1,
                found: false,
                marked: 0.0,
            });
        }
    }
    out
}

pub type Subtitles = VecDeque<Subtitle>;
