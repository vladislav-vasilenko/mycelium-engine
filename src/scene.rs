//! Тестовая сцена этапа 01: индексированный куб и два пересекающихся экземпляра.

use crate::layout::{InstanceData, Vertex};
use crate::reference::Box3;
use glam::{Mat4, Quat, Vec3};

/// Куб [−0.5, 0.5]³, 24 вершины и 36 индексов. Треугольники обходятся
/// против часовой стрелки при взгляде снаружи (лицевая сторона CCW).
pub fn cube() -> (Vec<Vertex>, Vec<u16>) {
    // (нормаль, ось u, ось v) для каждой грани. u × v = нормаль, поэтому
    // обход 0-1-2 даёт CCW снаружи.
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (n, u, v) in faces {
        debug_assert!((u.cross(v) - n).length() < 1e-6);
        let base = vertices.len() as u16;
        for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p = n * 0.5 + u * (0.5 * su) + v * (0.5 * sv);
            vertices.push(Vertex {
                position: p.to_array(),
                normal: n.to_array(),
                uv: [(su + 1.0) * 0.5, 1.0 - (sv + 1.0) * 0.5],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (vertices, indices)
}

/// Два куба, которые пересекаются: перекрытие проверяет буфер глубины.
pub fn instance_models() -> [Mat4; 2] {
    [
        Mat4::from_scale_rotation_translation(
            Vec3::splat(1.2),
            Quat::from_rotation_y(0.4),
            Vec3::new(0.0, 0.0, 0.0),
        ),
        Mat4::from_scale_rotation_translation(
            Vec3::splat(0.9),
            Quat::from_rotation_x(0.6) * Quat::from_rotation_y(-0.3),
            Vec3::new(0.65, 0.35, 0.45),
        ),
    ]
}

pub fn instances() -> [InstanceData; 2] {
    let m = instance_models();
    [
        InstanceData {
            model: m[0].to_cols_array_2d(),
            tint: [1.0, 0.85, 0.7, 1.0],
        },
        InstanceData {
            model: m[1].to_cols_array_2d(),
            tint: [0.7, 0.85, 1.0, 1.0],
        },
    ]
}

pub fn boxes() -> Vec<Box3> {
    instance_models()
        .into_iter()
        .map(|model| Box3 { model })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_triangles_face_outward() {
        let (v, idx) = cube();
        for tri in idx.chunks(3) {
            let a = Vec3::from(v[tri[0] as usize].position);
            let b = Vec3::from(v[tri[1] as usize].position);
            let c = Vec3::from(v[tri[2] as usize].position);
            let n = (b - a).cross(c - a);
            let centroid = (a + b + c) / 3.0;
            assert!(n.dot(centroid) > 0.0, "треугольник {tri:?} обращён внутрь");
        }
    }
}
