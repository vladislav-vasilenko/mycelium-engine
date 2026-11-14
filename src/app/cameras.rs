//! Камеры: старт на тропе, обзор острова, контрольные камеры самопроверки,
//! столбики маркеров маршрута.

use glam::{Mat4, Quat, Vec2, Vec3};

use crate::layout::InstanceData;
use crate::sim::{self, CameraState};
use crate::world::{Ground, MarkerKind, World};

/// Поза на начале тропы. Мир из генератора смотрит вдоль сплайна тропы; мир из
/// слоёв (Якусима) — с маркера начала тропы в сторону следующего маркера.
pub(super) fn trail_start_camera(world: &World) -> CameraState {
    let (a, dir) = if world.geo.is_none() {
        let trail = &world.generator.trail;
        let a = trail.point_at(0.0);
        (a, trail.point_at(0.02) - a)
    } else {
        let start = world
            .marker(MarkerKind::TrailStart)
            .map_or(world.size_m() * 0.5, |m| {
                Vec2::new(m.position.x, m.position.z)
            });
        let next = world
            .markers
            .iter()
            .find(|m| m.kind != MarkerKind::TrailStart)
            .map_or(start + Vec2::X, |m| Vec2::new(m.position.x, m.position.z));
        (start, next - start)
    };
    let h = world.sample_ground(a.x, a.y).map_or(0.0, |g| g.height);
    CameraState {
        position: Vec3::new(a.x, h + sim::EYE_HEIGHT_M, a.y),
        // forward = (−sin yaw, −cos yaw) в плоскости XZ.
        yaw: (-dir.x).atan2(-dir.y),
        pitch: -0.05,
    }
}

/// Камера на маркере `kind` в рост человека. Если маркер в воде (устье лежит
/// на кромке, а поле 7,5 м точнее сетки маркеров 30 м), игрок встаёт на
/// ближайший сухой грунт и смотрит на сам маркер; на суше — в сторону самого
/// низкого грунта в 60 м (к морю). `None`, если маркера нет.
pub(super) fn marker_camera(world: &World, kind: MarkerKind) -> Option<CameraState> {
    let m = world.marker(kind)?;
    let target = Vec2::new(m.position.x, m.position.z);
    let dry = |q: Vec2| {
        world
            .sample_ground(q.x, q.y)
            .filter(|g| !g.wet && g.height > 1.0)
            .map(|g| g.height)
    };
    // Ближайшая сухая точка: кольца через 5 м до 400 м.
    let p = (0..=80)
        .flat_map(|r| {
            let n = (r * 6).max(1);
            (0..n).map(move |k| {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                target + Vec2::new(a.cos(), a.sin()) * (r as f32 * 5.0)
            })
        })
        .find(|&q| dry(q).is_some())?;
    let h = dry(p)?;
    let dir = if (p - target).length() > 1.0 {
        target - p
    } else {
        (0..32)
            .map(|k| {
                let a = k as f32 / 32.0 * std::f32::consts::TAU;
                Vec2::new(a.cos(), a.sin())
            })
            .min_by(|a, b| {
                let g = |d: &Vec2| {
                    let q = p + *d * 60.0;
                    world.sample_ground(q.x, q.y).map_or(f32::MAX, |g| g.height)
                };
                g(a).total_cmp(&g(b))
            })
            .unwrap_or(Vec2::X)
    };
    Some(CameraState {
        position: Vec3::new(p.x, h + sim::EYE_HEIGHT_M, p.y),
        yaw: (-dir.x).atan2(-dir.y),
        pitch: -0.08,
    })
}

/// Камера из точки `from` с взглядом на `to`.
pub(super) fn look_camera(from: Vec3, to: Vec3) -> CameraState {
    let dir = to - from;
    CameraState {
        position: from,
        yaw: (-dir.x).atan2(-dir.z),
        pitch: (dir.y / dir.length()).asin(),
    }
}

/// Вид на весь остров с высоты над юго-западным морем; высота растёт с картой.
pub(super) fn island_overview_camera(world: &World) -> CameraState {
    let size = world.size_m();
    let top = world.fields.iter().fold(0.0f32, |a, f| a.max(f.height));
    let from = Vec3::new(0.12 * size.x, 0.32 * size.x + top, 1.12 * size.y);
    let to = Vec3::new(0.52 * size.x, 0.0, 0.45 * size.y);
    let dir = to - from;
    CameraState {
        position: from,
        yaw: (-dir.x).atan2(-dir.z),
        pitch: (dir.y / dir.length()).asin(),
    }
}

/// Контрольная камера ландшафта: над точкой обзора, взгляд на торфяник.
pub(super) fn terrain_check_camera(world: &World) -> CameraState {
    let from = world.marker(MarkerKind::Viewpoint).unwrap().position + Vec3::new(0.0, 12.0, 0.0);
    let to = world.generator.basin_center;
    let dir = Vec3::new(to.x, 6.0, to.y) - from;
    CameraState {
        position: from,
        yaw: (-dir.x).atan2(-dir.z),
        pitch: (dir.y / dir.length()).asin(),
    }
}

/// Столбики 0,5 × 4 м на маркерах маршрута.
pub(super) fn marker_posts(world: &World) -> Vec<InstanceData> {
    world
        .markers
        .iter()
        .map(|m| {
            let tint = match m.kind {
                MarkerKind::TrailStart => [1.0, 0.9, 0.3, 1.0],
                MarkerKind::Viewpoint => [0.4, 1.0, 0.5, 1.0],
                MarkerKind::BridgeSite => [1.0, 0.45, 0.2, 1.0],
                MarkerKind::RiverMouth => [0.3, 0.6, 1.0, 1.0],
                MarkerKind::ShoreEnd => [0.9, 0.9, 0.95, 1.0],
            };
            let model = Mat4::from_scale_rotation_translation(
                Vec3::new(0.5, 4.0, 0.5),
                Quat::IDENTITY,
                m.position + Vec3::new(0.0, 2.0, 0.0),
            );
            InstanceData {
                model: model.to_cols_array_2d(),
                tint,
            }
        })
        .collect()
}

pub(super) fn start_camera() -> CameraState {
    CameraState {
        position: Vec3::new(0.4, 1.4, 3.6),
        yaw: 0.0,
        pitch: -0.3,
    }
}
