//! Фиксированный шаг симуляции и свободная камера.
//!
//! Симуляция обновляется тиками 1/60 с. Рендер интерполирует между двумя
//! последними состояниями. Камера свободная, ходьба по грунту появится в этапе 02.
//! Соглашения: метры, правая система, Y вверх, при yaw = 0 камера смотрит в −Z.

use glam::{Mat4, Vec3};

pub const TICK_DT: f64 = 1.0 / 60.0;
/// Предел догоняющих тиков за кадр. Остаток времени отбрасывается.
pub const MAX_CATCHUP_TICKS: u32 = 5;
/// Кадр длиннее этого считается паузой (отладчик, сворачивание).
pub const MAX_FRAME_DT: f64 = 0.25;

const MOVE_SPEED: f32 = 4.0;
const FAST_MULTIPLIER: f32 = 3.0;
pub const MOUSE_SENSITIVITY: f32 = 0.0025;
const PITCH_LIMIT: f32 = 1.55;

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
    Mat4::perspective_infinite_reverse_rh(70f32.to_radians(), aspect, 0.05)
}

#[derive(Clone, Debug)]
pub struct Sim {
    pub tick: u64,
    pub prev: CameraState,
    pub curr: CameraState,
}

impl Sim {
    pub fn new(start: CameraState) -> Self {
        Sim {
            tick: 0,
            prev: start,
            curr: start,
        }
    }

    pub fn step(&mut self, input: &TickInput) {
        self.prev = self.curr;
        let c = &mut self.curr;
        c.yaw -= input.look_dx * MOUSE_SENSITIVITY;
        c.pitch = (c.pitch - input.look_dy * MOUSE_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);

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
        c.position += wish * speed * TICK_DT as f32;
        self.tick += 1;
    }

    pub fn interpolated(&self, alpha: f32) -> CameraState {
        self.prev.lerp(&self.curr, alpha)
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
mod tests {
    use super::*;

    fn start() -> CameraState {
        CameraState {
            position: Vec3::new(0.0, 1.0, 4.0),
            yaw: 0.0,
            pitch: 0.0,
        }
    }

    /// Сценарий команд: зависит только от номера тика.
    fn script(tick: u64) -> TickInput {
        TickInput {
            forward: if tick % 90 < 60 { 1.0 } else { -0.5 },
            right: ((tick / 20) % 3) as f32 - 1.0,
            up: if tick % 50 < 10 { 1.0 } else { 0.0 },
            look_dx: ((tick % 7) as f32) - 3.0,
            look_dy: ((tick % 5) as f32) - 2.0,
            fast: tick % 120 > 100,
        }
    }

    fn run(frame_dt: f64, total_ticks: u64) -> Sim {
        let mut sim = Sim::new(start());
        let mut clock = FixedClock::default();
        while sim.tick < total_ticks {
            let (ticks, _) = clock.advance(frame_dt);
            for _ in 0..ticks {
                if sim.tick < total_ticks {
                    let input = script(sim.tick);
                    sim.step(&input);
                }
            }
        }
        sim
    }

    /// Упражнение из маршрута чтения (§8.5.2): один набор команд по тикам
    /// при кадрах 1/30 и 1/120 с даёт одинаковое состояние на одном тике.
    #[test]
    fn same_ticks_same_state_at_30_60_120_fps() {
        let a = run(1.0 / 30.0, 600);
        let b = run(1.0 / 60.0, 600);
        let c = run(1.0 / 120.0, 600);
        assert_eq!(a.curr, b.curr);
        assert_eq!(a.curr, c.curr);
        assert_eq!(a.tick, 600);
    }

    #[test]
    fn catchup_is_limited_after_pause() {
        let mut clock = FixedClock::default();
        let (ticks, alpha) = clock.advance(5.0);
        assert_eq!(ticks, MAX_CATCHUP_TICKS);
        assert!((0.0..1.0).contains(&alpha));
        assert!(clock.dropped_ticks > 0);
        // После паузы аккумулятор не тянет долг в следующий кадр.
        let (next, _) = clock.advance(TICK_DT);
        assert!(next <= 2);
    }

    #[test]
    fn yaw_zero_looks_down_negative_z() {
        let f = start().forward();
        assert!((f - Vec3::NEG_Z).length() < 1e-6);
    }

    #[test]
    fn reversed_z_maps_near_to_one_and_far_to_zero() {
        let p = projection(16.0 / 9.0);
        let near = p * glam::Vec4::new(0.0, 0.0, -0.05, 1.0);
        let far = p * glam::Vec4::new(0.0, 0.0, -1.0e6, 1.0);
        assert!((near.z / near.w - 1.0).abs() < 1e-5);
        assert!(far.z / far.w < 1e-6);
        assert!(far.z / far.w >= 0.0);
    }
}
