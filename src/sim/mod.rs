//! Фиксированный шаг симуляции: свободный полёт и ходьба по грунту.
//!
//! Симуляция обновляется тиками 1/60 с. Рендер интерполирует положение между
//! двумя последними состояниями, а поворот мыши применяется сразу в каждом
//! кадре (`Sim::look`). Скорость набирается и гасится с ускорением. В режиме
//! ходьбы игрок стоит на поверхности `Ground`, поднимается на ступеньки до
//! `MAX_STEP_M`, не идёт вверх по склону круче `MAX_SLOPE_DEG`, замедляется
//! на подъёме, прыгает и падает с уступов под действием тяжести. Подъём на
//! ступеньку, приземление и шаги сглаживаются только в положении глаз
//! (`view_dy`), физическое положение остаётся точным.
//! Соглашения: метры, правая система, Y вверх, при yaw = 0 камера смотрит в −Z.

use glam::{Mat4, Vec2, Vec3};

use crate::world::Ground;

pub const TICK_DT: f64 = 1.0 / 60.0;
/// Предел догоняющих тиков за кадр. Остаток времени отбрасывается.
pub const MAX_CATCHUP_TICKS: u32 = 5;
/// Кадр длиннее этого считается паузой (отладчик, сворачивание).
pub const MAX_FRAME_DT: f64 = 0.25;

/// Скорость полёта, м/с, и множитель с Shift.
const MOVE_SPEED: f32 = 8.0;
const FAST_MULTIPLIER: f32 = 4.0;
/// Разгон и торможение в полёте, м/с².
const FLY_ACCEL: f32 = 40.0;
pub const MOUSE_SENSITIVITY: f32 = 0.0025;
const PITCH_LIMIT: f32 = 1.55;

/// Высота глаз над грунтом, м.
pub const EYE_HEIGHT_M: f32 = 1.7;
/// Наибольшая ступенька, на которую игрок поднимается без прыжка, м.
pub const MAX_STEP_M: f32 = 0.35;
/// Наибольший уклон, по которому можно идти вверх, градусы.
pub const MAX_SLOPE_DEG: f32 = 40.0;
/// Скорость шага и бега, м/с. Быстрее реальной ходьбы (1,4 м/с), чтобы
/// маршрут ~850 м проходился за минуты, а не за десятки минут.
pub const WALK_SPEED: f32 = 4.0;
pub const RUN_SPEED: f32 = 7.0;
/// Разгон к нужной скорости на грунте, м/с²: 0 → шаг за ~0,1 с.
pub const GROUND_ACCEL: f32 = 40.0;
/// Торможение без ввода на грунте, м/с²: остановка с бега за ~0,25 с.
pub const GROUND_FRICTION: f32 = 28.0;
/// Управление в воздухе, м/с².
pub const AIR_ACCEL: f32 = 6.0;
/// Замедление на подъёме: скорость × 1 / (1 + k · tg уклона по направлению).
pub const UPHILL_SLOWDOWN: f32 = 0.8;
/// Начальная скорость прыжка, м/с: высота ≈ v² / 2g ≈ 0,9 м.
pub const JUMP_SPEED: f32 = 4.2;
pub const GRAVITY: f32 = 9.81;

/// Сглаживание подъёма глаз после ступеньки, постоянная времени, с.
const STEP_SMOOTH_TAU: f32 = 0.07;
/// Подъём короче этого за тик не сглаживается, м. Даже бег вверх по склону
/// у предела (с замедлением на подъёме) поднимает меньше 0,06 м за тик.
const STEP_SMOOTH_MIN: f32 = 0.08;
/// Приседание при приземлении: м на 1 м/с скорости падения и предел, м.
const LAND_DIP_PER_SPEED: f32 = 0.025;
const LAND_DIP_MAX: f32 = 0.25;
/// Покачивание головы: длина шага, м, и амплитуда при шаге и беге, м.
const BOB_STRIDE_M: f32 = 0.8;
const BOB_WALK_M: f32 = 0.03;
const BOB_RUN_M: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveMode {
    Fly,
    Walk,
}

/// Команда на один тик. Оси в диапазоне [−1, 1], поворот в пикселях мыши.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TickInput {
    pub forward: f32,
    pub right: f32,
    pub up: f32,
    pub look_dx: f32,
    pub look_dy: f32,
    pub fast: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraState {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

impl CameraState {
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, sp, -cy * cp)
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_to_rh(self.position, self.forward(), Vec3::Y)
    }

    pub fn lerp(&self, other: &CameraState, t: f32) -> CameraState {
        CameraState {
            position: self.position.lerp(other.position, t),
            yaw: self.yaw + (other.yaw - self.yaw) * t,
            pitch: self.pitch + (other.pitch - self.pitch) * t,
        }
    }
}

/// Проекция с reversed-Z: ближняя плоскость → 1, бесконечность → 0.
pub fn projection(aspect: f32) -> Mat4 {
    Mat4::perspective_infinite_reverse_rh(70f32.to_radians(), aspect, NEAR_PLANE_M)
}

/// Ближняя плоскость камеры, м. Глубина reversed-Z: `depth = NEAR_PLANE_M / z_view`.
pub const NEAR_PLANE_M: f32 = 0.05;

#[derive(Clone, Debug)]
pub struct Sim {
    pub tick: u64,
    pub prev: CameraState,
    pub curr: CameraState,
    pub mode: MoveMode,
    /// Вертикальная скорость в режиме ходьбы, м/с (вниз — отрицательная).
    pub vertical_speed: f32,
    pub on_ground: bool,
    /// Тиков, в которые горизонтальное движение было полностью заблокировано.
    pub blocked_ticks: u64,
    /// Горизонтальная скорость (x, z) при ходьбе или полная скорость в полёте, м/с.
    pub velocity: Vec3,
    /// Сдвиг глаз по вертикали только для вида: ступенька, приземление, шаги.
    pub view_dy_prev: f32,
    pub view_dy: f32,
    /// Сглаживание ступенек и приземления, м (≤ 0).
    eye_lag: f32,
    /// Фаза шага, радианы, и доля амплитуды покачивания (0..1).
    bob_phase: f32,
    bob_weight: f32,
    /// Покачивание головы при ходьбе (клавиша B).
    pub head_bob: bool,
}

impl Sim {
    pub fn new(start: CameraState) -> Self {
        Sim {
            tick: 0,
            prev: start,
            curr: start,
            mode: MoveMode::Fly,
            vertical_speed: 0.0,
            on_ground: false,
            blocked_ticks: 0,
            velocity: Vec3::ZERO,
            view_dy_prev: 0.0,
            view_dy: 0.0,
            eye_lag: 0.0,
            bob_phase: 0.0,
            bob_weight: 0.0,
            head_bob: true,
        }
    }

    /// Поворот мыши в пикселях, применяется сразу к обоим состояниям, чтобы
    /// интерполяция не добавляла задержку в один тик.
    pub fn look(&mut self, dx: f32, dy: f32) {
        for c in [&mut self.prev, &mut self.curr] {
            c.yaw -= dx * MOUSE_SENSITIVITY;
            c.pitch = (c.pitch - dy * MOUSE_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }
    }

    /// Переключает режим. При переходе в ходьбу игрок встаёт на грунт под собой.
    pub fn set_mode(&mut self, mode: MoveMode, ground: &dyn Ground) {
        self.mode = mode;
        self.vertical_speed = 0.0;
        self.velocity = Vec3::ZERO;
        self.eye_lag = 0.0;
        if mode == MoveMode::Walk
            && let Some(g) = ground.sample_ground(self.curr.position.x, self.curr.position.z)
        {
            self.curr.position.y = g.height + EYE_HEIGHT_M;
            self.prev = self.curr;
            self.on_ground = true;
        }
    }

    /// Один тик. В режиме ходьбы нужен `ground`; без него ходьба ведёт себя как полёт.
    pub fn step(&mut self, input: &TickInput, ground: Option<&dyn Ground>) {
        self.prev = self.curr;
        self.view_dy_prev = self.view_dy;
        let c = &mut self.curr;
        c.yaw -= input.look_dx * MOUSE_SENSITIVITY;
        c.pitch = (c.pitch - input.look_dy * MOUSE_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);

        if let (MoveMode::Walk, Some(ground)) = (self.mode, ground) {
            self.walk(input, ground);
            self.tick += 1;
            return;
        }
        self.view_dy = 0.0;
        self.eye_lag = 0.0;
        self.bob_weight = 0.0;
        let c = &mut self.curr;

        let forward = c.forward();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let mut wish = forward * input.forward + right * input.right + Vec3::Y * input.up;
        if wish.length_squared() > 1.0 {
            wish = wish.normalize();
        }
        let speed = if input.fast {
            MOVE_SPEED * FAST_MULTIPLIER
        } else {
            MOVE_SPEED
        };
        let dt = TICK_DT as f32;
        self.velocity = approach(self.velocity, wish * speed, FLY_ACCEL * dt);
        c.position += self.velocity * dt;
        self.tick += 1;
    }

    /// Можно ли встать в точку `xz`, стоя ногами на высоте `feet`.
    /// Возвращает высоту грунта в точке, если шаг разрешён.
    fn can_step(ground: &dyn Ground, xz: Vec2, feet: f32) -> Option<f32> {
        let g = ground.sample_ground(xz.x, xz.y)?;
        let rise = g.height - feet;
        if rise > MAX_STEP_M {
            return None; // уступ выше ступеньки
        }
        let max_slope_cos = MAX_SLOPE_DEG.to_radians().cos();
        if rise > 1e-4 && g.normal.y < max_slope_cos {
            return None; // подъём по слишком крутому склону
        }
        Some(g.height)
    }

    fn walk(&mut self, input: &TickInput, ground: &dyn Ground) {
        let dt = TICK_DT as f32;
        let c = &mut self.curr;
        let (sy, cy) = c.yaw.sin_cos();
        let forward = Vec2::new(-sy, -cy);
        let right = Vec2::new(cy, -sy);
        let mut wish = forward * input.forward + right * input.right;
        if wish.length_squared() > 1.0 {
            wish = wish.normalize();
        }
        let pos = Vec2::new(c.position.x, c.position.z);
        let mut feet = c.position.y - EYE_HEIGHT_M;

        // Нужная скорость: шаг или бег, медленнее на подъёме.
        let mut target_speed = if input.fast { RUN_SPEED } else { WALK_SPEED };
        if let (Some(dir), Some(g)) = (wish.try_normalize(), ground.sample_ground(pos.x, pos.y))
            && g.normal.y > 1e-3
        {
            // Нормаль по горизонтали смотрит вниз по склону.
            let rise = -(dir.x * g.normal.x + dir.y * g.normal.z) / g.normal.y;
            target_speed /= 1.0 + UPHILL_SLOWDOWN * rise.max(0.0);
        }
        let target = wish * target_speed;
        let mut vel = Vec2::new(self.velocity.x, self.velocity.z);
        let rate = if !self.on_ground {
            AIR_ACCEL
        } else if wish == Vec2::ZERO {
            GROUND_FRICTION
        } else {
            GROUND_ACCEL
        };
        vel = approach2(vel, target, rate * dt);
        let delta = vel * dt;

        // Полный шаг, иначе скольжение вдоль одной оси.
        let mut new_pos = pos;
        let mut stepped_to = None;
        if delta != Vec2::ZERO {
            // Скольжение только по составляющей заметной длины: sin/cos от
            // yaw дают ~1e-8 вместо нуля, и такой «шаг» не должен считаться движением.
            const MIN_SLIDE_M: f32 = 1e-4;
            for (candidate, keep) in [
                (pos + delta, Vec2::ONE),
                (pos + Vec2::new(delta.x, 0.0), Vec2::X),
                (pos + Vec2::new(0.0, delta.y), Vec2::Y),
            ] {
                if (candidate - pos).length() < MIN_SLIDE_M {
                    continue;
                }
                if let Some(h) = Self::can_step(ground, candidate, feet) {
                    new_pos = candidate;
                    stepped_to = Some(h);
                    // Упёрлись по одной оси — гасим скорость по ней.
                    vel *= keep;
                    break;
                }
            }
            if stepped_to.is_none() {
                self.blocked_ticks += 1;
                vel = Vec2::ZERO;
            }
        }
        self.velocity = Vec3::new(vel.x, 0.0, vel.y);

        // Прыжок с грунта.
        if self.on_ground && input.up > 0.0 {
            self.vertical_speed = JUMP_SPEED;
            self.on_ground = false;
        }

        // Вертикаль: подъём на ступеньку сразу, спуск — по земле или падением.
        let ground_h = match stepped_to {
            Some(h) => h,
            None => ground
                .sample_ground(new_pos.x, new_pos.y)
                .map_or(feet, |g| g.height),
        };
        let old_feet = feet;
        let was_airborne = !self.on_ground;
        if self.vertical_speed <= 0.0 && ground_h >= feet {
            feet = ground_h;
            self.vertical_speed = 0.0;
            self.on_ground = true;
        } else if self.on_ground && self.vertical_speed <= 0.0 && feet - ground_h <= MAX_STEP_M {
            // Шаг вниз по склону или со ступеньки без отрыва от земли.
            feet = ground_h;
            self.vertical_speed = 0.0;
        } else {
            let fall_speed = -self.vertical_speed;
            self.vertical_speed -= GRAVITY * dt;
            feet += self.vertical_speed * dt;
            if feet <= ground_h {
                feet = ground_h;
                self.vertical_speed = 0.0;
                self.on_ground = true;
                // Приседание при приземлении.
                self.eye_lag -= (fall_speed.max(0.0) * LAND_DIP_PER_SPEED).min(LAND_DIP_MAX);
            } else {
                self.on_ground = false;
            }
        }
        c.position = Vec3::new(new_pos.x, feet + EYE_HEIGHT_M, new_pos.y);

        // Вид: ступенька поднимает глаза плавно, а не рывком.
        let rise = feet - old_feet;
        if !was_airborne && self.on_ground && rise > STEP_SMOOTH_MIN {
            self.eye_lag -= rise;
        }
        self.eye_lag =
            (self.eye_lag * (-dt / STEP_SMOOTH_TAU).exp()).max(-MAX_STEP_M - LAND_DIP_MAX);

        // Покачивание головы по пройденному пути.
        let moved = (new_pos - pos).length();
        self.bob_phase =
            (self.bob_phase + moved / BOB_STRIDE_M * std::f32::consts::PI) % std::f32::consts::TAU;
        let speed_now = moved / dt;
        let weight_target = if self.on_ground && self.head_bob {
            (speed_now / WALK_SPEED).min(1.0)
        } else {
            0.0
        };
        self.bob_weight += (weight_target - self.bob_weight) * (1.0 - (-dt / 0.15).exp());
        let amplitude = if speed_now > 0.5 * (WALK_SPEED + RUN_SPEED) {
            BOB_RUN_M
        } else {
            BOB_WALK_M
        };
        // |sin|: глаза ниже всего в момент постановки ноги, дважды за цикл шага.
        let bob = -amplitude * self.bob_weight * (1.0 - self.bob_phase.sin().abs());
        self.view_dy = self.eye_lag + bob;
    }

    /// Фаза шага (рад, π на шаг) и вес покачивания 0…1 — для покачивания рук.
    pub fn bob(&self) -> (f32, f32) {
        (self.bob_phase, self.bob_weight)
    }

    /// Камера для вида: интерполированное положение плюс сдвиг глаз `view_dy`.
    pub fn interpolated(&self, alpha: f32) -> CameraState {
        let mut c = self.prev.lerp(&self.curr, alpha);
        c.position.y += self.view_dy_prev + (self.view_dy - self.view_dy_prev) * alpha;
        c
    }
}

/// Сдвигает `v` к `target` не больше чем на `max_delta`.
fn approach(v: Vec3, target: Vec3, max_delta: f32) -> Vec3 {
    let d = target - v;
    let len = d.length();
    if len <= max_delta {
        target
    } else {
        v + d * (max_delta / len)
    }
}

fn approach2(v: Vec2, target: Vec2, max_delta: f32) -> Vec2 {
    let d = target - v;
    let len = d.length();
    if len <= max_delta {
        target
    } else {
        v + d * (max_delta / len)
    }
}

/// Аккумулятор времени: сколько тиков выполнить за кадр и доля для интерполяции.
#[derive(Clone, Debug, Default)]
pub struct FixedClock {
    accumulator: f64,
    pub dropped_ticks: u64,
}

impl FixedClock {
    pub fn advance(&mut self, frame_dt: f64) -> (u32, f32) {
        self.accumulator += frame_dt.clamp(0.0, MAX_FRAME_DT);
        let mut ticks = (self.accumulator / TICK_DT).floor() as u32;
        if ticks > MAX_CATCHUP_TICKS {
            self.dropped_ticks += u64::from(ticks - MAX_CATCHUP_TICKS);
            ticks = MAX_CATCHUP_TICKS;
            self.accumulator = self.accumulator.rem_euclid(TICK_DT);
        } else {
            self.accumulator -= f64::from(ticks) * TICK_DT;
        }
        (ticks, (self.accumulator / TICK_DT) as f32)
    }
}

#[cfg(test)]
mod tests;
