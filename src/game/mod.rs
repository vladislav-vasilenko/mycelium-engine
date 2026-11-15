//! Игровой срез Сиратани (05.1): грибы, осмотр взглядом, журнал миколога.
//!
//! Грибы расставляются детерминированно в игровой зоне 1 м (ущелье Сиратани)
//! по сообществам карты растительности, влажности и впадинам `macro.png`;
//! не на тропах и не на крутых склонах. Определение вида — осмотр (клавиша F);
//! журнал (J) хранит найденные виды.

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
}

pub struct Game {
    pub mushrooms: Vec<Mushroom>,
    /// Найдено экземпляров по видам.
    pub found: [u32; SPECIES.len()],
    pub journal_open: bool,
    /// Сообщение и оставшееся время показа, с.
    pub toast: Option<(String, f32)>,
    /// Гриб под взглядом (номер в `mushrooms`).
    pub focus: Option<usize>,
}

fn hash(x: i32, z: i32, salt: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (z as u32).wrapping_mul(0xd816_3841)
        ^ salt.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

fn unit(h: u32, k: u32) -> f32 {
    (hash(h as i32, k as i32, 0x51) & 0xff_ffff) as f32 / 16_777_216.0
}

impl Game {
    /// Грибы зоны Сиратани; без зоны — пусто (мир генератора).
    pub fn new(world: &World) -> Game {
        Game {
            mushrooms: place(world),
            found: [0; SPECIES.len()],
            journal_open: false,
            toast: None,
            focus: None,
        }
    }

    /// Гриб под взглядом: ближайший в 3 м, луч проходит в 12 см от шляпки.
    pub fn update_focus(&mut self, eye: Vec3, dir: Vec3) {
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
                self.focus = Some(i);
            }
        }
    }

    /// Осмотр гриба под взглядом: запись в журнал.
    pub fn inspect(&mut self) {
        let Some(i) = self.focus else { return };
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

    /// Интерфейс: прицел, подсказка осмотра, сообщение, журнал.
    pub fn ui(&self, b: &mut crate::ui::UiBuilder, w: f32, h: f32) {
        let (cx, cy) = (w * 0.5, h * 0.5);
        let focus = self.focus.map(|i| &self.mushrooms[i]);
        let dot = if focus.is_some() { 6.0 } else { 3.0 };
        b.rect(
            cx - dot * 0.5,
            cy - dot * 0.5,
            dot,
            dot,
            [255, 255, 255, 170],
        );
        if let Some(m) = focus {
            let sp = &SPECIES[m.species as usize];
            let what = if self.found[m.species as usize] > 0 {
                sp.name
            } else {
                "неизвестный гриб"
            };
            let label = if m.inspected {
                format!("{what} — уже в журнале")
            } else {
                format!("F — осмотреть: {what}")
            };
            let tw = b.font.width(&label, 18.0);
            b.rect(
                cx - tw * 0.5 - 10.0,
                cy + 26.0,
                tw + 20.0,
                30.0,
                [10, 14, 10, 140],
            );
            b.text(cx - tw * 0.5, cy + 29.0, 18.0, [235, 240, 225, 255], &label);
        }
        if let Some((t, left)) = &self.toast {
            let a = (left.min(1.0) * 255.0) as u8;
            let tw = b.font.width(t, 20.0);
            b.rect(
                cx - tw * 0.5 - 14.0,
                40.0,
                tw + 28.0,
                36.0,
                [10, 20, 12, (a as f32 * 0.6) as u8],
            );
            b.text(cx - tw * 0.5, 45.0, 20.0, [210, 245, 200, a], t);
        }
        let total: u32 = self.found.iter().sum();
        let known = self.found.iter().filter(|&&n| n > 0).count();
        b.text(
            20.0,
            h - 34.0,
            16.0,
            [220, 225, 210, 200],
            &format!(
                "J — журнал · видов {known}/{} · находок {total}",
                SPECIES.len()
            ),
        );
        if self.journal_open {
            let (pw, ph) = (640.0f32.min(w - 40.0), 520.0f32.min(h - 80.0));
            let (px, py) = ((w - pw) * 0.5, (h - ph) * 0.5);
            b.rect(px, py, pw, ph, [18, 22, 16, 225]);
            b.text(
                px + 24.0,
                py + 18.0,
                28.0,
                [240, 235, 210, 255],
                "Полевой журнал — Сиратани",
            );
            let mut y = py + 70.0;
            for (s, sp) in SPECIES.iter().enumerate() {
                if self.found[s] == 0 {
                    b.text(
                        px + 24.0,
                        y,
                        18.0,
                        [140, 140, 130, 255],
                        "? ? ?  — вид не найден",
                    );
                    y += 34.0;
                    continue;
                }
                b.text(
                    px + 24.0,
                    y,
                    20.0,
                    [230, 240, 215, 255],
                    &format!("{} — {} (×{})", sp.name, sp.latin, self.found[s]),
                );
                y += b.line(20.0);
                for line in wrap(b, sp.note, 16.0, pw - 48.0) {
                    b.text(px + 32.0, y, 16.0, [190, 200, 180, 255], &line);
                    y += b.line(16.0);
                }
                y += 10.0;
            }
        }
    }
}

fn wrap(b: &crate::ui::UiBuilder, text: &str, px: f32, width: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let cand = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if b.font.width(&cand, px) > width && !line.is_empty() {
            out.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = cand;
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// Расстановка в зоне 1 м: кандидат в клетке 3 м, вид по сообществу и маскам.
fn place(world: &World) -> Vec<Mushroom> {
    let Some(zone) = &world.zone else {
        return Vec::new();
    };
    let origin = zone.origin(&world.heightfield);
    let size = (zone.grid.nx - 1) as f32 * zone.grid.step_m;
    let map = world.size_m().x;
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
            let (forest, cedar) = world.vegetation.as_ref().map_or((0.5, 0.5), |v| {
                let k = ((p.y / map * (v.size - 1) as f32).round() as usize) * v.size as usize
                    + (p.x / map * (v.size - 1) as f32).round() as usize;
                let f = v.forest[v.ids[k.min(v.ids.len() - 1)] as usize];
                (f[0], f[1])
            });
            let mc = world
                .macro_detail
                .as_ref()
                .map_or([1.0, 0.5, 0.3, 0.0], |m| m.at(p.x, p.y, map));
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
}
