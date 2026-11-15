//! Игровой срез Сиратани (05.1): грибы, осмотр взглядом, журнал миколога.
//!
//! Грибы расставляются детерминированно в игровой зоне 1 м (ущелье Сиратани)
//! по сообществам карты растительности, влажности и впадинам `macro.png`;
//! не на тропах и не на крутых склонах. Определение вида — осмотр (клавиша F);
//! журнал (J) хранит найденные виды.
//!
//! 05.2: чувство мицелия и узел Мориты — [`sense`], макро-осмотр — [`closeup`],
//! интерфейс — [`ui`].

pub mod closeup;
pub mod sense;
mod ui;

use glam::{Vec2, Vec3};

use crate::layout::PlantInstance;
use crate::world::{Ground, World};

/// Вид гриба для журнала. Сведения — для проверки миколога (см. отчёт 05.1).
pub struct Species {
    pub name: &'static str,
    pub latin: &'static str,
    pub note: &'static str,
    /// Масштаб модели гриба (10 см при 1).
    pub scale: f32,
}

pub const SPECIES: [Species; 5] = [
    Species {
        name: "Светящийся мицен",
        latin: "Mycena lux-coeli",
        note: "Светится зелёным на гниющей древесине кастанопсиса в сезон дождей. Ищите ночью у влажных ложбин широколиственного леса.",
        scale: 0.6,
    },
    Species {
        name: "Трутовик плоский",
        latin: "Ganoderma applanatum",
        note: "Твёрдое плодовое тело на старых стволах и пнях; отпечаток спор коричневый.",
        scale: 1.6,
    },
    Species {
        name: "Шиитаке",
        latin: "Lentinula edodes",
        note: "Растёт на мёртвой древесине широколиственных деревьев, весной и осенью после дождей.",
        scale: 1.0,
    },
    Species {
        name: "Рейши",
        latin: "Ganoderma lucidum",
        note: "Лаковая шляпка у основания старых деревьев. В легендах острова — гриб долголетия.",
        scale: 1.2,
    },
    Species {
        name: "Мухомор пантерный",
        latin: "Amanita pantherina",
        note: "Ядовит. Суше и выше по склонам, у хвойных. Легко спутать со съедобными видами.",
        scale: 1.1,
    },
];

pub struct Mushroom {
    pub position: Vec3,
    pub species: u8,
    pub yaw: f32,
    pub scale: f32,
    pub inspected: bool,
    /// Волна чувства мицелия дошла: гриб обведён ещё столько секунд.
    pub marked: f32,
}

/// Что под взглядом.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Focus {
    Mushroom(usize),
    Trace(usize),
}

pub struct Game {
    pub mushrooms: Vec<Mushroom>,
    /// Найдено экземпляров по видам.
    pub found: [u32; SPECIES.len()],
    pub journal_open: bool,
    /// Сообщение и оставшееся время показа, с.
    pub toast: Option<(String, f32)>,
    /// Гриб или след под взглядом.
    pub focus: Option<Focus>,
    pub sense: sense::Sense,
    pub traces: Vec<sense::Trace>,
    pub node: Option<sense::Node>,
    pub closeup: closeup::Closeup,
    /// Записи журнала помимо видов (следы, аномалии).
    pub notes: Vec<sense::Note>,
    pub subtitles: sense::Subtitles,
    /// Игра управляет позой рук (App возвращает позу 1, когда перестаёт).
    pub hands_override: bool,
}

pub(crate) fn hash(x: i32, z: i32, salt: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (z as u32).wrapping_mul(0xd816_3841)
        ^ salt.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

pub(crate) fn unit(h: u32, k: u32) -> f32 {
    (hash(h as i32, k as i32, 0x51) & 0xff_ffff) as f32 / 16_777_216.0
}

impl Game {
    /// Грибы, следы и узел зоны Сиратани; без зоны — пусто (мир генератора).
    pub fn new(world: &World) -> Game {
        let zone = world.zone.is_some();
        Game {
            mushrooms: place(world),
            found: [0; SPECIES.len()],
            journal_open: false,
            toast: None,
            focus: None,
            sense: sense::Sense::default(),
            traces: if zone {
                sense::place_traces(world)
            } else {
                Vec::new()
            },
            node: if zone { sense::place_node(world) } else { None },
            closeup: closeup::Closeup::default(),
            notes: Vec::new(),
            subtitles: Default::default(),
            hands_override: false,
        }
    }

    /// Под взглядом: гриб в 3 м (луч в 12 см от шляпки) или помеченный
    /// чувством след в 4 м (луч в 50 см).
    pub fn update_focus(&mut self, eye: Vec3, dir: Vec3) {
        if self.closeup.active() {
            return;
        }
        self.focus = None;
        let mut best = f32::MAX;
        for (i, m) in self.mushrooms.iter().enumerate() {
            let c = m.position + Vec3::Y * 0.06 * m.scale;
            let t = (c - eye).dot(dir);
            if !(0.0..3.0).contains(&t) {
                continue;
            }
            let miss = (eye + dir * t).distance(c);
            if miss < 0.12 + 0.04 * m.scale && t < best {
                best = t;
                self.focus = Some(Focus::Mushroom(i));
            }
        }
        for (i, tr) in self.traces.iter().enumerate() {
            if tr.marked <= 0.0 && !tr.found {
                continue;
            }
            let t = (tr.position - eye).dot(dir);
            if !(0.0..4.0).contains(&t) {
                continue;
            }
            if (eye + dir * t).distance(tr.position) < 0.5 && t < best {
                best = t;
                self.focus = Some(Focus::Trace(i));
            }
        }
    }

    /// F: присесть к грибу (макро-осмотр), встать или осмотреть след.
    pub fn inspect(&mut self, camera: &crate::sim::CameraState) {
        match self.focus {
            Some(Focus::Trace(i)) if !self.closeup.active() => self.inspect_trace(i),
            _ => self.toggle_closeup(camera),
        }
    }

    /// Запись гриба в журнал (когда игрок присел к нему).
    fn record_mushroom(&mut self, i: usize) {
        let m = &mut self.mushrooms[i];
        if m.inspected {
            return;
        }
        m.inspected = true;
        let s = m.species as usize;
        self.found[s] += 1;
        let sp = &SPECIES[s];
        let text = if self.found[s] == 1 {
            format!("Новый вид в журнале: {} ({})", sp.name, sp.latin)
        } else {
            format!("{} — найдено {}", sp.name, self.found[s])
        };
        self.toast = Some((text, 4.0));
    }

    pub fn tick(&mut self, dt: f32) {
        self.update_closeup(dt);
        self.tick_subtitles(dt);
        if let Some((_, t)) = &mut self.toast {
            *t -= dt;
            if *t <= 0.0 {
                self.toast = None;
            }
        }
    }

    /// Экземпляры для отрисовки (в 120 м от камеры).
    pub fn instances(&self, camera: Vec3) -> Vec<PlantInstance> {
        self.mushrooms
            .iter()
            .filter(|m| m.position.distance_squared(camera) < 120.0 * 120.0)
            .map(|m| PlantInstance {
                position: m.position.to_array(),
                scale: m.scale,
                yaw: m.yaw,
                tint: 0.5,
                phase: 0.0,
                _pad: m.species as f32,
            })
            .collect()
    }
}

/// Доля леса и якусуги в сообществе карты растительности под точкой.
pub(crate) fn community(world: &World, p: Vec2) -> (f32, f32) {
    let map = world.size_m().x;
    world.vegetation.as_ref().map_or((0.5, 0.5), |v| {
        let k = ((p.y / map * (v.size - 1) as f32).round() as usize) * v.size as usize
            + (p.x / map * (v.size - 1) as f32).round() as usize;
        let f = v.forest[v.ids[k.min(v.ids.len() - 1)] as usize];
        (f[0], f[1])
    })
}

/// Маски `macro.png`: видимость неба, кривизна, влажность, осыпь.
pub(crate) fn macro_at(world: &World, p: Vec2) -> [f32; 4] {
    let map = world.size_m().x;
    world
        .macro_detail
        .as_ref()
        .map_or([1.0, 0.5, 0.3, 0.0], |m| m.at(p.x, p.y, map))
}

/// Расстановка в зоне 1 м: кандидат в клетке 3 м, вид по сообществу и маскам.
fn place(world: &World) -> Vec<Mushroom> {
    let Some(zone) = &world.zone else {
        return Vec::new();
    };
    let origin = zone.origin(&world.heightfield);
    let size = (zone.grid.nx - 1) as f32 * zone.grid.step_m;
    let cell = 3.0;
    let n = (size / cell) as i32;
    let mut out = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let gx = (origin.x / cell) as i32 + i;
            let gz = (origin.y / cell) as i32 + j;
            let h = hash(gx, gz, 0x6D05);
            let p = Vec2::new(
                gx as f32 + 0.1 + 0.8 * unit(h, 1),
                gz as f32 + 0.1 + 0.8 * unit(h, 2),
            ) * cell;
            let (forest, cedar) = community(world, p);
            let mc = macro_at(world, p);
            let damp = mc[2];
            let hollow = ((0.5 - mc[1]) * 2.5).clamp(0.0, 1.0);
            let ridge = ((mc[1] - 0.5) * 2.5).clamp(0.0, 1.0);
            let prob = forest * (0.04 + 0.12 * damp + 0.08 * hollow);
            if unit(h, 3) > prob {
                continue;
            }
            let Some(g) = world.sample_ground(p.x, p.y) else {
                continue;
            };
            if g.height < 2.0 || g.normal.y < 0.8 || g.on_trail {
                continue;
            }
            // Вид: светящийся мицен и шиитаке — влажный широколиственный лес,
            // трутовики — везде в лесу, мухомор — суше, у хвойных, на гребнях.
            let broad = 1.0 - cedar;
            let w = [
                broad * damp * 1.5,
                0.5,
                broad * 0.8,
                0.35,
                cedar * 0.6 + ridge * 0.8,
            ];
            let sum: f32 = w.iter().sum();
            let mut r = unit(h, 4) * sum;
            let mut species = 0u8;
            for (k, wk) in w.iter().enumerate() {
                if r < *wk {
                    species = k as u8;
                    break;
                }
                r -= wk;
            }
            let sp = &SPECIES[species as usize];
            out.push(Mushroom {
                position: Vec3::new(p.x, g.height, p.y),
                species,
                yaw: unit(h, 5) * std::f32::consts::TAU,
                scale: sp.scale * (0.8 + 0.4 * unit(h, 6)),
                inspected: false,
                marked: 0.0,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn shiratani_has_mushrooms_of_every_species() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/world/yakushima/far");
        let w = crate::world::layers::import(&dir).unwrap();
        let g = super::Game::new(&w);
        assert!(g.mushrooms.len() > 100, "грибов {}", g.mushrooms.len());
        for s in 0..super::SPECIES.len() {
            assert!(
                g.mushrooms.iter().any(|m| m.species as usize == s),
                "нет вида {s}"
            );
        }
        // Детерминизм.
        assert_eq!(super::Game::new(&w).mushrooms.len(), g.mushrooms.len());
    }

    /// 05.2: узел у тропы, следы зверей; импульс доходит до узла, долгое
    /// вслушивание у узла запускает эхо Мориты один раз.
    #[test]
    fn sense_reaches_node_and_echo_plays_once() {
        use glam::Vec3;
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/world/yakushima/far");
        let w = crate::world::layers::import(&dir).unwrap();
        let mut g = super::Game::new(&w);
        let node = g.node.as_ref().expect("узел у тропы Сиратани").position;
        assert!(g.traces.len() > 20, "следов {}", g.traces.len());
        assert!(g.traces.iter().any(|t| t.kind == 0) && g.traces.iter().any(|t| t.kind == 1));
        // Слушаем в 3 м от узла: нити есть, эхо — одно.
        let eye = node + Vec3::new(3.0, 1.7, 0.0);
        for _ in 0..240 {
            g.update_sense(1.0 / 60.0, eye, true, &w);
        }
        assert!(!g.sense_beads(eye, &w).is_empty());
        assert!(g.node.as_ref().unwrap().heard);
        assert_eq!(g.subtitles.len(), 3);
        assert_eq!(g.notes.len(), 1);
        for _ in 0..240 {
            g.update_sense(1.0 / 60.0, eye, true, &w);
        }
        assert_eq!(g.notes.len(), 1);
    }
}
