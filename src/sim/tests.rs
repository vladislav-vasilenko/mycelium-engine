//! Тесты фиксированного шага, полёта и ходьбы.

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

fn flat() -> Synthetic<impl Fn(f32, f32) -> f32> {
    Synthetic(|_x: f32, _z: f32| 0.0)
}

#[test]
fn accelerates_to_walk_speed_and_stops_quickly() {
    let ground = flat();
    let mut sim = walk_east(&ground, 12); // 0,2 с
    let speed = sim.velocity.length();
    assert!((speed - WALK_SPEED).abs() < 1e-4, "скорость {speed}");
    let x = sim.curr.position.x;
    for _ in 0..15 {
        sim.step(&TickInput::default(), Some(&ground));
    }
    assert_eq!(sim.velocity, Vec3::ZERO, "остановка за 0,25 с");
    let slide = sim.curr.position.x - x;
    assert!(slide > 0.0 && slide < 0.5, "выбег {slide} м");
}

#[test]
fn uphill_is_slower_than_flat() {
    let slope = Synthetic(|x: f32, _z: f32| x * 25f32.to_radians().tan());
    let up = walk_east(&slope, 120).curr.position.x;
    let level = walk_east(&flat(), 120).curr.position.x;
    assert!(up < level * 0.8, "вверх {up}, ровно {level}");
}

#[test]
fn jump_lifts_and_lands_on_ground() {
    let ground = flat();
    let mut sim = walk_east(&ground, 10);
    let jump = TickInput {
        up: 1.0,
        ..Default::default()
    };
    sim.step(&jump, Some(&ground));
    assert!(!sim.on_ground);
    let mut apex: f32 = 0.0;
    for _ in 0..120 {
        sim.step(&TickInput::default(), Some(&ground));
        apex = apex.max(feet(&sim));
    }
    assert!(apex > 0.7 && apex < 1.1, "высота прыжка {apex}");
    assert!(sim.on_ground && feet(&sim).abs() < 1e-5);
}

#[test]
fn step_up_is_smoothed_only_in_view() {
    let ground = Synthetic(|x: f32, _z: f32| if x > 0.0 { 0.3 } else { 0.0 });
    let mut sim = Sim::new(CameraState {
        position: Vec3::new(-0.2, 10.0, 0.0),
        yaw: -std::f32::consts::FRAC_PI_2,
        pitch: 0.0,
    });
    sim.set_mode(MoveMode::Walk, &ground);
    sim.head_bob = false;
    let input = TickInput {
        forward: 1.0,
        ..Default::default()
    };
    let mut seen_lag = false;
    for _ in 0..30 {
        sim.step(&input, Some(&ground));
        if sim.curr.position.x > 0.0 && !seen_lag {
            // Физически уже на ступеньке, глаза поднимаются плавно.
            assert!((feet(&sim) - 0.3).abs() < 1e-5);
            assert!(sim.view_dy < -0.1, "view_dy = {}", sim.view_dy);
            seen_lag = true;
        }
    }
    assert!(seen_lag);
    assert!(sim.view_dy.abs() < 0.01, "сглаживание затухает");
}

#[test]
fn look_applies_without_interpolation_lag() {
    let mut sim = Sim::new(start());
    sim.look(100.0, 0.0);
    let expected = -100.0 * MOUSE_SENSITIVITY;
    assert!((sim.interpolated(0.0).yaw - expected).abs() < 1e-6);
    assert!((sim.interpolated(1.0).yaw - expected).abs() < 1e-6);
}

const FIRST: i32 = crate::world::FIRST_MAP_TILES;

/// Бот проходит всю тропу первой карты бегом, держа точку в 3 м впереди
/// на оси тропы. Замена ручного прохода из критериев 02.1.
#[test]
fn bot_walks_whole_trail_from_start_to_shore() {
    walk_trail(&crate::world::World::generate(0xEC40, FIRST, FIRST));
}

/// Тот же проход по миру, собранному из экспортированных слоёв PNG.
#[test]
fn bot_walks_whole_trail_on_world_from_layers() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-layers/bot");
    let _ = std::fs::remove_dir_all(&dir);
    crate::world::layers::export(&crate::world::World::generate(0xEC40, FIRST, FIRST), &dir)
        .unwrap();
    walk_trail(&crate::world::layers::import(&dir).unwrap());
}

fn walk_trail(world: &crate::world::World) {
    let trail = &world.generator.trail;
    let len = trail.length();
    let start = trail.point_at(0.0);
    let end = trail.point_at(1.0);
    let mut sim = Sim::new(CameraState {
        position: Vec3::new(start.x, 100.0, start.y),
        yaw: 0.0,
        pitch: 0.0,
    });
    sim.set_mode(MoveMode::Walk, world);
    let input = TickInput {
        forward: 1.0,
        fast: true,
        ..Default::default()
    };
    let max_ticks = (len / 2.0 * 60.0) as u64; // не медленнее 2 м/с в среднем
    let mut airborne = 0u64;
    let mut reached = false;
    for _ in 0..max_ticks {
        let p = Vec2::new(sim.curr.position.x, sim.curr.position.z);
        let (_, t, _) = trail.nearest(p);
        let target = trail.point_at(t + 3.0 / len);
        let d = target - p;
        let yaw = (-d.x).atan2(-d.y);
        sim.curr.yaw = yaw;
        sim.prev.yaw = yaw;
        sim.step(&input, Some(world));
        let g = world
            .sample_ground(sim.curr.position.x, sim.curr.position.z)
            .expect("бот ушёл за карту");
        assert!(feet(&sim) >= g.height - 1e-3, "провал под грунт в {p}");
        if sim.on_ground {
            assert!((feet(&sim) - g.height).abs() < 1e-3, "не на грунте в {p}");
        } else {
            airborne += 1;
        }
        if (Vec2::new(sim.curr.position.x, sim.curr.position.z) - end).length() < 3.0 {
            reached = true;
            break;
        }
    }
    let p = sim.curr.position;
    if !reached {
        let (dr, _, _) = world.generator.river.nearest(Vec2::new(p.x, p.z));
        let (dt, t, _) = trail.nearest(Vec2::new(p.x, p.z));
        eprintln!(
            "до реки {dr:.1} м, до тропы {dt:.1} м, t {t:.3}, ticks {} blocked {}",
            sim.tick, sim.blocked_ticks
        );
        for k in -4..=4 {
            let q = trail.point_at(t + k as f32 * 1.0 / len);
            let g = world.sample_ground(q.x, q.y).unwrap();
            eprintln!(
                "  {k:+} м: h {:.2} n.y {:.3} wet {}",
                g.height, g.normal.y, g.wet
            );
        }
    }
    assert!(
        reached,
        "не дошёл: остановился в ({:.1}, {:.1}) из {len:.0} м",
        p.x, p.z
    );
    assert!(
        sim.blocked_ticks * 100 < sim.tick,
        "заблокирован {} тиков из {}",
        sim.blocked_ticks,
        sim.tick
    );
    eprintln!(
        "тропа {len:.0} м за {:.1} с, тиков упора {}, в воздухе {airborne}",
        sim.tick as f32 / 60.0,
        sim.blocked_ticks
    );
}
