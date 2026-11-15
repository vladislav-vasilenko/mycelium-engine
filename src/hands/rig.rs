//! Движение рук: смена поз со смешиванием, покачивание от шага, отставание
//! от поворота взгляда и приседание при приземлении. Всё в пространстве глаза.
//!
//! Живость без клипов (10.1, сессия 1): отставание — пружина с перелётом
//! (руки догоняют взгляд и чуть проскакивают), смещение вбок при повороте,
//! дыхание в покое, встречный мах рук при ходьбе, переходы поз с разгоном и
//! торможением, лёгкое движение пальцев, руки ближе к взгляду при взгляде
//! вниз, смена предмета — руки уходят вниз и возвращаются.

use glam::{Mat4, Quat, Vec2, Vec3};

use super::skeleton::{Pose, Skeleton};

/// Позы ассета в порядке клавиш 1–4.
pub const POSE_NAMES: [&str; 4] = ["relaxed", "grip", "point", "reach"];
/// Время перехода между позами, с.
pub const POSE_BLEND_S: f32 = 0.32;
/// Отставание от поворота: доля поворота, которую руки «не успели» повторить,
/// и предел, рад. Возврат — пружина: частота, затухание (< 1 — с перелётом).
const LAG_GAIN: f32 = 0.6;
const LAG_MAX_RAD: f32 = 0.12;
const LAG_SPRING_HZ: f32 = 3.2;
const LAG_DAMPING: f32 = 0.45;
/// Сдвиг рук вбок и вниз на радиан отставания, м.
const LAG_SHIFT_M: f32 = 0.18;
/// Дыхание: период, размах вверх-вниз (м) и наклон (рад).
const BREATH_S: f32 = 4.2;
const BREATH_M: f32 = 0.004;
const BREATH_RAD: f32 = 0.008;
/// Встречный мах рук при ходьбе (вперёд-назад), м.
const ARM_SWING_M: f32 = 0.018;
/// Подъём рук к взгляду при взгляде вниз, м на радиан.
const LOOK_DOWN_RISE_M: f32 = 0.05;
/// Смена предмета: длительность (вниз и обратно), глубина, наклон.
pub const SWAP_S: f32 = 0.55;
const SWAP_DROP_M: f32 = 0.32;
const SWAP_TILT_RAD: f32 = 0.7;
/// Пальцы: размах лёгкого движения, рад.
const FINGER_IDLE_RAD: f32 = 0.05;

/// Действие правой руки с предметом (10.1, сессия 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Рация ко рту, пока держат кнопку (как в Firewatch).
    Talk,
    /// Совком: опустить, два раза копнуть, поднять гриб к корзине, вернуть.
    Dig,
    /// Фонарь: поднять и направить вперёд.
    Aim,
}

/// Длительность разового действия и вход/выход удерживаемого, с.
const DIG_S: f32 = 1.8;
const HOLD_IN_S: f32 = 0.35;

struct ActionState {
    kind: Action,
    /// Время с начала, с.
    t: f32,
    /// Удерживаемое: отпущено в момент `released_at`.
    released_at: Option<f32>,
}

/// Ключевой кадр действия: сдвиг кисти в глазу (м) и повороты XYZ вокруг кисти (рад).
type Key = (f32, [f32; 3], [f32; 3]);

/// Кадры копания: время (доля), сдвиг, поворот.
const DIG_KEYS: [Key; 8] = [
    (0.00, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
    (0.18, [-0.04, -0.22, -0.10], [-0.9, 0.0, 0.2]),
    (0.30, [-0.04, -0.17, -0.06], [-0.4, 0.0, 0.2]),
    (0.42, [-0.04, -0.23, -0.12], [-1.0, 0.0, 0.2]),
    (0.54, [-0.04, -0.16, -0.05], [-0.3, 0.0, 0.2]),
    (0.72, [-0.16, -0.04, 0.02], [0.3, 0.3, 0.5]),
    (0.84, [-0.18, -0.08, 0.02], [0.5, 0.3, 0.9]),
    (1.00, [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
];

/// Поза «удерживаемого»: рация у рта (ближе к центру, вверх, к лицу, микрофон
/// к губам) и фонарь вперёд.
const TALK_KEY: ([f32; 3], [f32; 3]) = ([-0.08, 0.12, 0.04], [0.35, 0.3, -0.2]);
const AIM_KEY: ([f32; 3], [f32; 3]) = ([-0.06, 0.12, -0.10], [0.25, 0.1, 0.0]);

fn sample_keys(keys: &[Key], u: f32) -> (Vec3, Vec3) {
    let u = u.clamp(0.0, 1.0);
    let i = keys
        .iter()
        .rposition(|k| k.0 <= u)
        .unwrap_or(0)
        .min(keys.len() - 2);
    let (a, b) = (keys[i], keys[i + 1]);
    let k = smoothstep((u - a.0) / (b.0 - a.0).max(1e-4));
    (
        Vec3::from(a.1).lerp(Vec3::from(b.1), k),
        Vec3::from(a.2).lerp(Vec3::from(b.2), k),
    )
}
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
    /// Дополнительный сдвиг рук в пространстве глаза от игры (05.2), м.
    pub offset: Vec3,
}

pub struct HandRig {
    poses: Vec<Pose>,
    /// Поза в момент начала перехода и цель.
    from: Pose,
    target: usize,
    /// Прогресс перехода 0…1.
    t: f32,
    lag: Vec2,
    lag_vel: Vec2,
    prev_look: Option<(f32, f32)>,
    dy_smooth: f32,
    /// Время с начала, с (дыхание, пальцы).
    clock: f32,
    /// Смена предмета: прогресс 0…1 (`None` — нет смены) и пройдена ли середина.
    swap: Option<f32>,
    swap_mid: bool,
    /// Суставы для живости: ключицы (мах) и первые фаланги пальцев.
    joints: RigJoints,
    action: Option<ActionState>,
}

#[derive(Default)]
struct RigJoints {
    clavicle: [Option<usize>; 2],
    hands: [Option<usize>; 2],
    fingers: Vec<(usize, f32)>,
}

/// Перелёт в конце перехода (ease-out-back): разгон, торможение с небольшим
/// перелётом и возврат — движение руки, а не механическая интерполяция.
fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let c1 = 1.2;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
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
            lag_vel: Vec2::ZERO,
            prev_look: None,
            dy_smooth: 0.0,
            clock: 0.0,
            swap: None,
            swap_mid: false,
            joints: RigJoints::default(),
            action: None,
        }
    }

    /// Находит суставы живости в скелете (без них — только движение корня).
    pub fn bind(&mut self, skel: &Skeleton) {
        self.joints.clavicle = [skel.joint("clavicle.L"), skel.joint("clavicle.R")];
        self.joints.hands = [skel.joint("hand.L"), skel.joint("hand.R")];
        self.joints.fingers = ["index", "middle", "ring", "pinky"]
            .iter()
            .enumerate()
            .flat_map(|(k, f)| {
                ["L", "R"]
                    .into_iter()
                    .enumerate()
                    .filter_map(move |(s, side)| {
                        skel.joint(&format!("f_{f}.01.{side}"))
                            .map(|j| (j, k as f32 * 1.7 + s as f32 * 0.9))
                    })
            })
            .collect();
    }

    /// Начинает действие правой руки (если не идёт другое и нет смены предмета).
    pub fn act(&mut self, kind: Action) {
        if self.action.is_none() && self.swap.is_none() {
            self.action = Some(ActionState {
                kind,
                t: 0.0,
                released_at: None,
            });
        }
    }

    /// Кнопку отпустили: удерживаемое действие (рация, фонарь) уходит.
    pub fn release(&mut self) {
        if let Some(a) = self.action.as_mut()
            && a.kind != Action::Dig
            && a.released_at.is_none()
        {
            a.released_at = Some(a.t);
        }
    }

    /// Сдвиг и поворот правой кисти от действия (в глазу).
    fn action_offset(&self) -> (Vec3, Vec3) {
        let Some(a) = &self.action else {
            return (Vec3::ZERO, Vec3::ZERO);
        };
        match a.kind {
            Action::Dig => sample_keys(&DIG_KEYS, a.t / DIG_S),
            Action::Talk | Action::Aim => {
                let (t, r) = if a.kind == Action::Talk {
                    TALK_KEY
                } else {
                    AIM_KEY
                };
                let up = ease_out_back(a.t / HOLD_IN_S);
                let down = a
                    .released_at
                    .map_or(1.0, |r0| 1.0 - smoothstep((a.t - r0) / HOLD_IN_S));
                let k = up * down;
                // Говорит: рука чуть покачивается у рта.
                let talk = if a.kind == Action::Talk {
                    Vec3::new(0.0, 0.006 * (a.t * 7.0).sin(), 0.0) * k
                } else {
                    Vec3::ZERO
                };
                (Vec3::from(t) * k + talk, Vec3::from(r) * k)
            }
        }
    }

    /// Правая кисть — к цели: сдвиг `dt` и поворот `dr` (XYZ, рад) вокруг
    /// запястья в пространстве глаза. Кисть висит на IK-кости `hand.R.control`
    /// (корневая, как в риге Blender), а плечо и предплечье доворачиваются
    /// аналитическим IK из двух костей (Грегори, гл. 12): локоть остаётся в
    /// своей плоскости (полюс — текущее положение локтя).
    fn reach_right(&self, p: &mut Pose, skel: &Skeleton, dt: Vec3, dr: Vec3) {
        let (Some(ua), Some(fa), Some(hc), Some(hand)) = (
            skel.joint("upper_arm.R"),
            skel.joint("forearm.R"),
            skel.joint("hand.R.control"),
            self.joints.hands[1],
        ) else {
            return;
        };
        let g = skel.globals(p);
        let pos = |m: Mat4| m.w_axis.truncate();
        let (s, e, w) = (pos(g[ua]), pos(g[fa]), pos(g[hand]));
        // Кисть: поворот вокруг запястья и сдвиг.
        let delta = Mat4::from_translation(w + dt)
            * Mat4::from_quat(Quat::from_euler(glam::EulerRot::XYZ, dr.x, dr.y, dr.z))
            * Mat4::from_translation(-w);
        let set_local = |p: &mut Pose, j: usize, parent: Mat4, global: Mat4| {
            let (sc, r, t) = (parent * skel.joints[j].pre)
                .inverse()
                .mul_mat4(&global)
                .to_scale_rotation_translation();
            p.0[j] = super::skeleton::Trs { t, r, s: sc };
        };
        let hc_parent = skel.joints[hc].parent.map_or(Mat4::IDENTITY, |q| g[q]);
        set_local(p, hc, hc_parent, delta * g[hc]);
        // Плечо и предплечье: цель запястья — `w + dt`.
        let target = w + dt;
        let (a, b) = ((e - s).length(), (w - e).length());
        let to = target - s;
        let d = to.length().clamp((a - b).abs() + 1e-3, a + b - 1e-3);
        let u = to.normalize_or_zero();
        let pole = (e - s) - (w - s).normalize_or_zero() * (e - s).dot((w - s).normalize_or_zero());
        let n = (pole - u * pole.dot(u)).normalize_or(Vec3::NEG_Y);
        let x = (a * a - b * b + d * d) / (2.0 * d);
        let h = (a * a - x * x).max(0.0).sqrt();
        let e2 = s + u * x + n * h;
        let rotate_about = |m: Mat4, at: Vec3, q: Quat| {
            Mat4::from_translation(at) * Mat4::from_quat(q) * Mat4::from_translation(-at) * m
        };
        let q1 = Quat::from_rotation_arc((e - s).normalize_or_zero(), (e2 - s).normalize_or_zero());
        let ua_parent = skel.joints[ua].parent.map_or(Mat4::IDENTITY, |q| g[q]);
        let g_ua = rotate_about(g[ua], s, q1);
        set_local(p, ua, ua_parent, g_ua);
        // Предплечье после поворота плеча — и довернуть к цели.
        let g_fa = g_ua * skel.joints[fa].pre * p.0[fa].to_mat4();
        let e_now = pos(g_fa);
        let dir_now = q1 * (w - e);
        let q2 = Quat::from_rotation_arc(
            dir_now.normalize_or_zero(),
            (target - e_now).normalize_or_zero(),
        );
        set_local(p, fa, g_ua, rotate_about(g_fa, e_now, q2));
    }

    /// Начинает смену предмета: руки уходят вниз и возвращаются.
    pub fn start_swap(&mut self) {
        if self.swap.is_none() {
            self.swap = Some(0.0);
            self.swap_mid = false;
        }
    }

    pub fn swapping(&self) -> bool {
        self.swap.is_some()
    }

    /// Середина смены пройдена (руки внизу, вне кадра): пора менять предмет.
    /// Возвращает `true` один раз за смену.
    pub fn take_swap_mid(&mut self) -> bool {
        match self.swap {
            Some(t) if t >= 0.5 && !self.swap_mid => {
                self.swap_mid = true;
                true
            }
            _ => false,
        }
    }

    /// Глубина ухода рук при смене 0…1.
    fn swap_depth(&self) -> f32 {
        self.swap.map_or(0.0, |t| {
            let k = if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 };
            smoothstep(k)
        })
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
    /// Поза перехода без живости пальцев (проверки).
    pub fn base_pose(&self) -> Pose {
        self.from
            .blend(&self.poses[self.target], ease_out_back(self.t))
    }

    pub fn pose(&self) -> Pose {
        let mut p = self.base_pose();
        // Пальцы: медленное, несинхронное движение первых фаланг (сгиб вокруг
        // локальной оси X) — кисть не выглядит застывшей.
        for &(j, ph) in &self.joints.fingers {
            if let Some(tr) = p.0.get_mut(j) {
                let a = FINGER_IDLE_RAD
                    * (self.clock * 0.9 + ph).sin()
                    * (0.6 + 0.4 * (self.clock * 0.37 + ph * 2.0).sin());
                tr.r = (tr.r * Quat::from_rotation_x(a)).normalize();
            }
        }
        p
    }

    /// Поза кадра: мах рук от шага (ключицы вперёд-назад в противофазе) и
    /// действие правой руки — жёсткий сдвиг и поворот всей руки вокруг кисти
    /// через ключицу (плечо вне кадра, кисть идёт по ключевым кадрам).
    pub fn pose_frame(&self, inp: &RigInput, skel: &Skeleton) -> Pose {
        let mut p = self.pose();
        let (dt, dr) = self.action_offset();
        if dt.length_squared() > 1e-10 || dr.length_squared() > 1e-10 {
            self.reach_right(&mut p, skel, dt, dr);
        }
        let w = inp.bob_weight;
        for (side, j) in self.joints.clavicle.iter().enumerate() {
            let (Some(j), sign) = (*j, if side == 0 { 1.0 } else { -1.0 }) else {
                continue;
            };
            if let Some(tr) = p.0.get_mut(j) {
                let swing = ARM_SWING_M * w * (inp.bob_phase * 0.5).sin() * sign;
                tr.t += Vec3::new(0.0, 0.0, swing);
            }
        }
        p
    }

    pub fn update(&mut self, inp: &RigInput) {
        let dt = inp.dt.max(0.0);
        self.t = (self.t + dt / POSE_BLEND_S).min(1.0);

        let (dyaw, dpitch) = match self.prev_look {
            Some((y, p)) => (wrap_angle(inp.yaw - y), inp.pitch - p),
            None => (0.0, 0.0),
        };
        self.prev_look = Some((inp.yaw, inp.pitch));
        // Отставание: толчок от поворота и возврат пружиной (полунеявный
        // Эйлер, шаги ≤ 4 мс — устойчиво при любом кадре).
        self.lag = (self.lag - Vec2::new(dyaw, dpitch) * LAG_GAIN)
            .clamp(Vec2::splat(-LAG_MAX_RAD), Vec2::splat(LAG_MAX_RAD));
        let w0 = std::f32::consts::TAU * LAG_SPRING_HZ;
        let steps = (dt / 0.004).ceil().max(1.0) as usize;
        let h = dt / steps as f32;
        for _ in 0..steps {
            let acc = -w0 * w0 * self.lag - 2.0 * LAG_DAMPING * w0 * self.lag_vel;
            self.lag_vel += acc * h;
            self.lag += self.lag_vel * h;
        }
        self.clock += dt;
        if let Some(a) = self.action.as_mut() {
            a.t += dt;
            let done = match (a.kind, a.released_at) {
                (Action::Dig, _) => a.t >= DIG_S,
                (_, Some(r0)) => a.t >= r0 + HOLD_IN_S,
                _ => false,
            };
            if done {
                self.action = None;
            }
        }
        if let Some(t) = self.swap.as_mut() {
            *t += dt / SWAP_S;
            if *t >= 1.0 {
                self.swap = None;
            }
        }

        self.dy_smooth += (inp.view_dy - self.dy_smooth) * (1.0 - (-dt / DIP_TAU_S).exp());
    }

    /// Трансформация рук относительно глаза: отставание поворотом вокруг глаза,
    /// покачивание и проседание сдвигом.
    pub fn root(&self, inp: &RigInput) -> Mat4 {
        let w = inp.bob_weight;
        // Шаг — по дуге (восьмёркой): вбок, вниз на каждом шаге, чуть вперёд.
        let sway = Vec3::new(
            SWAY_X_M * w * inp.bob_phase.cos(),
            -SWAY_Y_M * w * inp.bob_phase.sin().abs() + DIP_GAIN * self.dy_smooth.min(0.0),
            -0.004 * w * (inp.bob_phase * 2.0).sin(),
        );
        // Дыхание (сильнее в покое).
        let b = (self.clock * std::f32::consts::TAU / BREATH_S).sin() * (1.0 - 0.7 * w);
        let breath = Vec3::new(0.0, BREATH_M * b, 0.0);
        // Отставание сдвигает руки к внешней стороне поворота.
        let shift = Vec3::new(self.lag.x, self.lag.y, 0.0) * LAG_SHIFT_M;
        let look = Vec3::new(0.0, LOOK_DOWN_RISE_M * (-inp.pitch).max(0.0), 0.0);
        let d = self.swap_depth();
        let swap = Vec3::new(0.02 * d, -SWAP_DROP_M * d, 0.05 * d);
        Mat4::from_translation(sway + breath + shift + look + swap + inp.offset)
            * Mat4::from_rotation_y(self.lag.x)
            * Mat4::from_rotation_x(self.lag.y + BREATH_RAD * b - SWAP_TILT_RAD * d)
            * Mat4::from_rotation_z(-self.lag.x * 0.4)
    }

    /// Отставание от поворота, рад.
    #[cfg(test)]
    pub fn lag(&self) -> Vec2 {
        self.lag
    }
}
