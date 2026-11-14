//! Движение рук: смена поз со смешиванием, покачивание от шага, отставание
//! от поворота взгляда и приседание при приземлении. Всё в пространстве глаза.

use glam::{Mat4, Vec2, Vec3};

use super::skeleton::Pose;

/// Позы ассета в порядке клавиш 1–4.
pub const POSE_NAMES: [&str; 4] = ["relaxed", "grip", "point", "reach"];
/// Время перехода между позами, с.
pub const POSE_BLEND_S: f32 = 0.25;
/// Отставание от поворота: доля поворота, которую руки «не успели» повторить,
/// постоянная возврата и предел, рад.
const LAG_GAIN: f32 = 0.6;
const LAG_TAU_S: f32 = 0.07;
const LAG_MAX_RAD: f32 = 0.08;
/// Покачивание от шага: размах вбок и вниз, м.
const SWAY_X_M: f32 = 0.008;
const SWAY_Y_M: f32 = 0.006;
/// Руки повторяют вертикальный сдвиг глаз с запаздыванием: при приземлении
/// они проседают сильнее головы и возвращаются позже.
const DIP_GAIN: f32 = 0.6;
const DIP_TAU_S: f32 = 0.12;

/// Состояние камеры и шага на кадр.
#[derive(Clone, Copy, Debug, Default)]
pub struct RigInput {
    pub dt: f32,
    pub yaw: f32,
    pub pitch: f32,
    /// Фаза шага, рад: π на шаг (как у покачивания головы).
    pub bob_phase: f32,
    /// Вес покачивания 0…1: 0 стоя, в полёте и при выключенном покачивании (B).
    pub bob_weight: f32,
    /// Сдвиг глаз по вертикали (ступеньки, приземление, покачивание), м.
    pub view_dy: f32,
}

pub struct HandRig {
    poses: Vec<Pose>,
    /// Поза в момент начала перехода и цель.
    from: Pose,
    target: usize,
    /// Прогресс перехода 0…1.
    t: f32,
    lag: Vec2,
    prev_look: Option<(f32, f32)>,
    dy_smooth: f32,
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Разность углов в (−π, π].
fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (a + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}

impl HandRig {
    /// `poses` — позы в порядке `POSE_NAMES`; начальная — первая.
    pub fn new(poses: Vec<Pose>) -> HandRig {
        let from = poses[0].clone();
        HandRig {
            poses,
            from,
            target: 0,
            t: 1.0,
            lag: Vec2::ZERO,
            prev_look: None,
            dy_smooth: 0.0,
        }
    }

    pub fn target(&self) -> usize {
        self.target
    }

    /// Начинает переход к позе `i` из текущей (даже если прошлый не закончен).
    pub fn set_pose(&mut self, i: usize) {
        if i == self.target || i >= self.poses.len() {
            return;
        }
        self.from = self.pose();
        self.target = i;
        self.t = 0.0;
    }

    /// Текущая поза: смешивание начальной и целевой по сглаженному прогрессу.
    pub fn pose(&self) -> Pose {
        self.from
            .blend(&self.poses[self.target], smoothstep(self.t))
    }

    pub fn update(&mut self, inp: &RigInput) {
        let dt = inp.dt.max(0.0);
        self.t = (self.t + dt / POSE_BLEND_S).min(1.0);

        let (dyaw, dpitch) = match self.prev_look {
            Some((y, p)) => (wrap_angle(inp.yaw - y), inp.pitch - p),
            None => (0.0, 0.0),
        };
        self.prev_look = Some((inp.yaw, inp.pitch));
        let decay = (-dt / LAG_TAU_S).exp();
        self.lag = ((self.lag - Vec2::new(dyaw, dpitch) * LAG_GAIN) * decay)
            .clamp(Vec2::splat(-LAG_MAX_RAD), Vec2::splat(LAG_MAX_RAD));

        self.dy_smooth += (inp.view_dy - self.dy_smooth) * (1.0 - (-dt / DIP_TAU_S).exp());
    }

    /// Трансформация рук относительно глаза: отставание поворотом вокруг глаза,
    /// покачивание и проседание сдвигом.
    pub fn root(&self, inp: &RigInput) -> Mat4 {
        let w = inp.bob_weight;
        let sway = Vec3::new(
            SWAY_X_M * w * inp.bob_phase.cos(),
            -SWAY_Y_M * w * inp.bob_phase.sin().abs() + DIP_GAIN * self.dy_smooth.min(0.0),
            0.0,
        );
        Mat4::from_translation(sway)
            * Mat4::from_rotation_y(self.lag.x)
            * Mat4::from_rotation_x(self.lag.y)
    }

    /// Отставание от поворота, рад.
    #[cfg(test)]
    pub fn lag(&self) -> Vec2 {
        self.lag
    }
}
