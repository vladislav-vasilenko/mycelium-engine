//! Макро-осмотр гриба (05.2): F на грибе — герой приседает, камера плавно
//! опускается к шляпке на ~30 см, угол обзора сужается до макро; мышь
//! слегка поворачивает взгляд вокруг гриба. Запись в журнал — когда
//! присел. Повторное F — встать.

use glam::{Vec2, Vec3};

use super::Game;
use crate::sim::CameraState;
use crate::world::{Ground, World};

/// Время приседания и подъёма, с.
const DOWN_S: f32 = 0.9;
const UP_S: f32 = 0.6;
/// Угол обзора стоя и в макро, градусы.
pub const FOV_DEG: f32 = 70.0;
pub const MACRO_FOV_DEG: f32 = 30.0;
/// Мышь в макро поворачивает взгляд не дальше этих углов, рад.
const LOOK_YAW: f32 = 0.35;
const LOOK_PITCH: f32 = 0.25;

#[derive(Default)]
pub struct Closeup {
    /// Гриб осмотра.
    pub target: Option<usize>,
    /// Прогресс приседания 0…1.
    pub t: f32,
    leaving: bool,
    /// Взгляд игрока при входе: сдвиг мыши от него поворачивает макро-вид.
    base: (f32, f32),
    recorded: bool,
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn wrap(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

impl Closeup {
    /// Осмотр идёт (приседание, макро или подъём): ходьба и чувство выключены.
    pub fn active(&self) -> bool {
        self.target.is_some()
    }

    /// Вес макро-вида 0…1 со сглаживанием.
    pub fn weight(&self) -> f32 {
        smooth(self.t)
    }
}

impl Game {
    /// F: присесть к грибу под взглядом или встать.
    pub fn toggle_closeup(&mut self, camera: &CameraState) {
        let c = &mut self.closeup;
        if c.active() {
            c.leaving = true;
            return;
        }
        if let Some(super::Focus::Mushroom(i)) = self.focus {
            *c = Closeup {
                target: Some(i),
                t: 0.0,
                leaving: false,
                base: (camera.yaw, camera.pitch),
                recorded: false,
            };
        }
    }

    pub fn update_closeup(&mut self, dt: f32) {
        let c = &mut self.closeup;
        let Some(i) = c.target else { return };
        if c.leaving {
            c.t -= dt / UP_S;
            if c.t <= 0.0 {
                *c = Closeup::default();
            }
            return;
        }
        c.t = (c.t + dt / DOWN_S).min(1.0);
        if c.t >= 1.0 && !c.recorded {
            c.recorded = true;
            self.record_mushroom(i);
        }
    }

    /// Камера кадра: стоячий вид игрока, смешанный с макро-видом у гриба.
    pub fn view(&self, cam: CameraState, world: &World) -> CameraState {
        let c = &self.closeup;
        let Some(i) = c.target else { return cam };
        let m = &self.mushrooms[i];
        let e = c.weight();
        let s = m.scale.max(0.8);
        let center = m.position + Vec3::Y * 0.05 * m.scale;
        let mut to = Vec2::new(m.position.x - cam.position.x, m.position.z - cam.position.z);
        if to.length_squared() < 1e-4 {
            to = Vec2::new(-cam.yaw.sin(), -cam.yaw.cos());
        }
        let back = to.normalize() * 0.30 * s;
        let ex = m.position.x - back.x;
        let ez = m.position.z - back.y;
        let ground = world
            .sample_ground(ex, ez)
            .map_or(m.position.y, |g| g.height);
        let eye = Vec3::new(ex, (ground + 0.09).max(center.y + 0.05 * s), ez);
        let d = (center - eye).normalize();
        let yaw0 = (-d.x).atan2(-d.z);
        let pitch0 = d.y.asin();
        let yaw = yaw0 + wrap(cam.yaw - c.base.0).clamp(-LOOK_YAW, LOOK_YAW);
        let pitch = pitch0 + (cam.pitch - c.base.1).clamp(-LOOK_PITCH, LOOK_PITCH);
        // Голова идёт по дуге: сначала вниз, потом вперёд к грибу.
        let down = smooth((c.t * 1.6).min(1.0));
        let fwd = e;
        let pos = Vec3::new(
            cam.position.x + (eye.x - cam.position.x) * fwd,
            cam.position.y + (eye.y - cam.position.y) * down,
            cam.position.z + (eye.z - cam.position.z) * fwd,
        );
        CameraState {
            position: pos,
            yaw: cam.yaw + wrap(yaw - cam.yaw) * e,
            pitch: cam.pitch + (pitch - cam.pitch) * e,
        }
    }

    /// Угол обзора кадра, градусы.
    pub fn fov_deg(&self) -> f32 {
        FOV_DEG + (MACRO_FOV_DEG - FOV_DEG) * self.closeup.weight()
    }

    /// Поза рук (номер в `hands::POSE_NAMES`) и сдвиг в пространстве глаза:
    /// в чувстве мицелия ладони тянутся к земле, при осмотре — указывают.
    pub fn hands_hint(&self) -> (Option<usize>, Vec3) {
        let e = self.closeup.weight();
        let l = self.sense.level;
        if self.closeup.active() {
            (Some(2), Vec3::new(0.0, -0.10, 0.02) * e)
        } else if l > 0.0 {
            (Some(3), Vec3::new(0.0, -0.07, -0.03) * l)
        } else {
            (None, Vec3::ZERO)
        }
    }
}
