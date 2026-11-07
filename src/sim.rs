//! Фиксированный шаг симуляции: свободный полёт и ходьба по грунту.
//!
//! Симуляция обновляется тиками 1/60 с. Рендер интерполирует между двумя
//! последними состояниями. В режиме ходьбы игрок стоит на поверхности
//! `Ground`, поднимается на ступеньки до `MAX_STEP_M`, не идёт вверх по склону
//! круче `MAX_SLOPE_DEG` и падает с уступов под действием тяжести.
//! Соглашения: метры, правая система, Y вверх, при yaw = 0 камера смотрит в −Z.

use glam::{Mat4, Vec2, Vec3};

use crate::world::Ground;

pub const TICK_DT: f64 = 1.0 / 60.0;
/// Предел догоняющих тиков за кадр. Остаток времени отбрасывается.
pub const MAX_CATCHUP_TICKS: u32 = 5;
/// Кадр длиннее этого считается паузой (отладчик, сворачивание).
pub const MAX_FRAME_DT: f64 = 0.25;

const MOVE_SPEED: f32 = 4.0;
const FAST_MULTIPLIER: f32 = 3.0;
pub const MOUSE_SENSITIVITY: f32 = 0.0025;
const PITCH_LIMIT: f32 = 1.55;

/// Высота глаз над грунтом, м.
pub const EYE_HEIGHT_M: f32 = 1.7;
/// Наибольшая ступенька, на которую игрок поднимается без прыжка, м.
pub const MAX_STEP_M: f32 = 0.35;
/// Наибольший уклон, по которому можно идти вверх, градусы.
pub const MAX_SLOPE_DEG: f32 = 40.0;
pub const WALK_SPEED: f32 = 1.6;
pub const RUN_SPEED: f32 = 4.0;
pub const GRAVITY: f32 = 9.81;

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
        }
    }

    /// Переключает режим. При переходе в ходьбу игрок встаёт на грунт под собой.
    pub fn set_mode(&mut self, mode: MoveMode, ground: &dyn Ground) {
        self.mode = mode;
        self.vertical_speed = 0.0;
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
        let c = &mut self.curr;
        c.yaw -= input.look_dx * MOUSE_SENSITIVITY;
        c.pitch = (c.pitch - input.look_dy * MOUSE_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);

        if let (MoveMode::Walk, Some(ground)) = (self.mode, ground) {
            self.walk(input, ground);
            self.tick += 1;
            return;
        }
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
        c.position += wish * speed * TICK_DT as f32;
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
        let speed = if input.fast { RUN_SPEED } else { WALK_SPEED };
        let delta = wish * speed * dt;

        let pos = Vec2::new(c.position.x, c.position.z);
        let mut feet = c.position.y - EYE_HEIGHT_M;

        // Полный шаг, иначе скольжение вдоль одной оси.
        let mut new_pos = pos;
        let mut stepped_to = None;
        if delta != Vec2::ZERO {
            // Скольжение только по составляющей заметной длины: sin/cos от
            // yaw дают ~1e-8 вместо нуля, и такой «шаг» не должен считаться движением.
            const MIN_SLIDE_M: f32 = 1e-4;
            for candidate in [
                pos + delta,
                pos + Vec2::new(delta.x, 0.0),
                pos + Vec2::new(0.0, delta.y),
            ] {
                if (candidate - pos).length() < MIN_SLIDE_M {
                    continue;
                }
                if let Some(h) = Self::can_step(ground, candidate, feet) {
                    new_pos = candidate;
                    stepped_to = Some(h);
                    break;
                }
            }
            if stepped_to.is_none() {
                self.blocked_ticks += 1;
            }
        }

        // Вертикаль: подъём на ступеньку сразу, спуск — по земле или падением.
        let ground_h = match stepped_to {
            Some(h) => h,
            None => ground
                .sample_ground(new_pos.x, new_pos.y)
                .map_or(feet, |g| g.height),
        };
        if ground_h >= feet {
            feet = ground_h;
            self.vertical_speed = 0.0;
            self.on_ground = true;
        } else if self.on_ground && feet - ground_h <= MAX_STEP_M {
            // Шаг вниз по склону или со ступеньки без отрыва от земли.
            feet = ground_h;
            self.vertical_speed = 0.0;
        } else {
            self.vertical_speed -= GRAVITY * dt;
            feet += self.vertical_speed * dt;
            if feet <= ground_h {
                feet = ground_h;
                self.vertical_speed = 0.0;
                self.on_ground = true;
            } else {
                self.on_ground = false;
            }
        }
        c.position = Vec3::new(new_pos.x, feet + EYE_HEIGHT_M, new_pos.y);
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
                    sim.step(&input, None);
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

    use crate::world::{GroundSample, Substrate, TileId};

    /// Поверхность для тестов: высота задаётся функцией, нормаль — по разностям.
    struct Synthetic<F: Fn(f32, f32) -> f32>(F);

    impl<F: Fn(f32, f32) -> f32> Ground for Synthetic<F> {
        fn sample_ground(&self, x: f32, z: f32) -> Option<GroundSample> {
            let e = 0.01;
            let h = (self.0)(x, z);
            let dx = ((self.0)(x + e, z) - (self.0)(x - e, z)) / (2.0 * e);
            let dz = ((self.0)(x, z + e) - (self.0)(x, z - e)) / (2.0 * e);
            Some(GroundSample {
                height: h,
                normal: Vec3::new(-dx, 1.0, -dz).normalize(),
                substrate: Substrate::Soil,
                on_trail: false,
                wet: false,
                tile: TileId { x: 0, z: 0 },
            })
        }
    }

    /// Идти вдоль +X (yaw = −90°) из x = −2 заданное число тиков.
    fn walk_east(ground: &dyn Ground, ticks: u32) -> Sim {
        let mut sim = Sim::new(CameraState {
            position: Vec3::new(-2.0, 10.0, 0.0),
            yaw: -std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
        });
        sim.set_mode(MoveMode::Walk, ground);
        let input = TickInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..ticks {
            sim.step(&input, Some(ground));
        }
        sim
    }

    fn feet(sim: &Sim) -> f32 {
        sim.curr.position.y - EYE_HEIGHT_M
    }

    #[test]
    fn walks_up_a_low_step() {
        let ground = Synthetic(|x: f32, _z: f32| if x > 0.0 { 0.3 } else { 0.0 });
        let sim = walk_east(&ground, 240);
        assert!(sim.curr.position.x > 2.0, "x = {}", sim.curr.position.x);
        assert!((feet(&sim) - 0.3).abs() < 1e-5);
    }

    #[test]
    fn stops_at_a_high_step() {
        let ground = Synthetic(|x: f32, _z: f32| if x > 0.0 { 0.5 } else { 0.0 });
        let sim = walk_east(&ground, 240);
        assert!(sim.curr.position.x <= 0.0, "x = {}", sim.curr.position.x);
        assert!(feet(&sim).abs() < 1e-5);
        assert!(sim.blocked_ticks > 0);
    }

    #[test]
    fn climbs_gentle_slope_but_not_steep_one() {
        let gentle = Synthetic(|x: f32, _z: f32| x.max(0.0) * 20f32.to_radians().tan());
        let sim = walk_east(&gentle, 300);
        assert!(sim.curr.position.x > 4.0);
        let expected = sim.curr.position.x * 20f32.to_radians().tan();
        assert!(
            (feet(&sim) - expected).abs() < 1e-3,
            "на склоне стоит на грунте"
        );

        let steep = Synthetic(|x: f32, _z: f32| x.max(0.0) * 50f32.to_radians().tan());
        let sim = walk_east(&steep, 300);
        assert!(sim.curr.position.x < 0.1, "x = {}", sim.curr.position.x);
    }

    #[test]
    fn falls_from_a_ledge_and_lands() {
        let ground = Synthetic(|x: f32, _z: f32| if x > 0.0 { -3.0 } else { 0.0 });
        let sim = walk_east(&ground, 240);
        assert!(sim.curr.position.x > 1.0);
        assert!((feet(&sim) + 3.0).abs() < 1e-5);
        assert!(sim.on_ground);
    }

    #[test]
    fn walking_is_deterministic_across_frame_rates() {
        let ground = Synthetic(|x: f32, z: f32| 0.3 * (x * 0.7).sin() + 0.2 * (z * 0.4).cos());
        let run = |frame_dt: f64| {
            let mut sim = Sim::new(start());
            sim.set_mode(MoveMode::Walk, &ground);
            let mut clock = FixedClock::default();
            while sim.tick < 600 {
                let (ticks, _) = clock.advance(frame_dt);
                for _ in 0..ticks {
                    if sim.tick < 600 {
                        let input = script(sim.tick);
                        sim.step(&input, Some(&ground));
                    }
                }
            }
            sim.curr
        };
        assert_eq!(run(1.0 / 30.0), run(1.0 / 120.0));
    }

    #[test]
    fn stays_on_world_ground_across_tile_borders() {
        let world = crate::world::World::generate(0xEC40, 3, 3);
        // Идём вдоль z = 100 через границы тайлов x = 64 и x = 128.
        let mut sim = Sim::new(CameraState {
            position: Vec3::new(40.0, 50.0, 100.0),
            yaw: -std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
        });
        sim.set_mode(MoveMode::Walk, &world);
        let input = TickInput {
            forward: 1.0,
            fast: true,
            ..Default::default()
        };
        let mut crossed = std::collections::BTreeSet::new();
        for _ in 0..1800 {
            sim.step(&input, Some(&world));
            let p = sim.curr.position;
            let g = world.sample_ground(p.x, p.z).unwrap();
            crossed.insert(g.tile);
            if sim.on_ground {
                assert!(
                    (feet(&sim) - g.height).abs() < 1e-4,
                    "на грунте в x = {}",
                    p.x
                );
            }
        }
        assert!(crossed.len() >= 2, "пройдено тайлов: {}", crossed.len());
    }
}
