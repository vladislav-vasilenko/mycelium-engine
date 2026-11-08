//! Камеры: старт на тропе, обзор острова, контрольные камеры самопроверки,
//! столбики маркеров маршрута.

use glam::{Mat4, Quat, Vec3};

use crate::layout::InstanceData;
use crate::sim::{self, CameraState};
use crate::world::{Ground, MarkerKind, World};

/// Поза на начале тропы, взгляд вдоль тропы.
pub(super) fn trail_start_camera(world: &World) -> CameraState {
    let trail = &world.generator.trail;
    let a = trail.point_at(0.0);
    let dir = trail.point_at(0.02) - a;
    let h = world.sample_ground(a.x, a.y).map_or(0.0, |g| g.height);
    CameraState {
        position: Vec3::new(a.x, h + sim::EYE_HEIGHT_M, a.y),
        // forward = (−sin yaw, −cos yaw) в плоскости XZ.
        yaw: (-dir.x).atan2(-dir.y),
        pitch: -0.05,
    }
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

/// Вид на весь остров с высоты над юго-западным морем.
pub(super) fn island_overview_camera(world: &World) -> CameraState {
    let size = world.size_m();
    let from = Vec3::new(0.12 * size.x, 330.0, 1.12 * size.y);
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
