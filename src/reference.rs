//! CPU-эталоны для проверки GPU: диагностический узор и глубина сцены.

use glam::{Mat4, Vec3, Vec4};

pub const PATTERN_SIZE: u32 = 256;
/// Время узора в режиме самопроверки.
pub const SELFTEST_PATTERN_TIME: f32 = 1.25;

/// Повторяет `pattern_color` из `shaders/metal/common.h`.
pub fn pattern_color(x: u32, y: u32, size: u32, t: f32) -> [f32; 3] {
    let u = x as f32 / (size - 1) as f32;
    let v = y as f32 / (size - 1) as f32;
    let cell = ((x / 32) + (y / 32)) % 2;
    let k = if cell == 0 { 1.0 } else { 0.6 };
    let stripe = 0.5 + 0.5 * (t * 2.0 + (x + y) as f32 * 0.05).sin();
    [u * k, v * k, stripe * k]
}

/// Сравнивает RGBA8 изображение узора с эталоном. Возвращает число
/// несовпадений и максимальное отклонение в единицах 1/255.
pub fn compare_pattern(pixels: &[u8], size: u32, t: f32, tolerance: u8) -> (usize, u8) {
    let mut mismatches = 0;
    let mut max_diff = 0u8;
    for y in 0..size {
        for x in 0..size {
            let expected = pattern_color(x, y, size, t);
            let i = ((y * size + x) * 4) as usize;
            for c in 0..3 {
                let e = (expected[c].clamp(0.0, 1.0) * 255.0).round() as i32;
                let d = (pixels[i + c] as i32 - e).unsigned_abs() as u8;
                max_diff = max_diff.max(d);
                if d > tolerance {
                    mismatches += 1;
                }
            }
        }
    }
    (mismatches, max_diff)
}

/// Куб сцены: единичный куб [−0.5, 0.5]³, преобразованный матрицей `model`.
pub struct Box3 {
    pub model: Mat4,
}

/// Пересечение луча с преобразованным кубом. Возвращает ближайшую точку входа
/// с t ≥ 0 в мировых координатах.
fn ray_box(origin: Vec3, dir: Vec3, b: &Box3) -> Option<Vec3> {
    let inv = b.model.inverse();
    let o = inv.transform_point3(origin);
    let d = inv.transform_vector3(dir);
    let mut t0 = f32::NEG_INFINITY;
    let mut t1 = f32::INFINITY;
    for axis in 0..3 {
        let (oa, da) = (o[axis], d[axis]);
        if da.abs() < 1e-12 {
            if !(-0.5..=0.5).contains(&oa) {
                return None;
            }
        } else {
            let a = (-0.5 - oa) / da;
            let c = (0.5 - oa) / da;
            t0 = t0.max(a.min(c));
            t1 = t1.min(a.max(c));
        }
    }
    if t1 < t0.max(0.0) {
        return None;
    }
    // Луч задан в мире, t одинаково для точки в локальном и мировом виде.
    let t = if t0 >= 0.0 { t0 } else { t1 };
    Some(origin + dir * t)
}

/// Ожидаемая reversed-Z глубина в пикселе (центр пикселя, начало вверху слева).
/// 0 означает пустой фон (значение очистки).
pub fn expected_depth(
    px: u32,
    py: u32,
    width: u32,
    height: u32,
    view: Mat4,
    proj: Mat4,
    boxes: &[Box3],
) -> f32 {
    let ndc_x = (px as f32 + 0.5) / width as f32 * 2.0 - 1.0;
    let ndc_y = 1.0 - (py as f32 + 0.5) / height as f32 * 2.0;
    let view_proj = proj * view;
    let inv = view_proj.inverse();
    let near = inv * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
    let far = inv * Vec4::new(ndc_x, ndc_y, 0.5, 1.0);
    let origin = near.truncate() / near.w;
    let dir = (far.truncate() / far.w - origin).normalize();
    let mut best: Option<f32> = None;
    for b in boxes {
        if let Some(hit) = ray_box(origin, dir, b) {
            let clip = view_proj * hit.extend(1.0);
            let depth = clip.z / clip.w;
            best = Some(best.map_or(depth, |d: f32| d.max(depth)));
        }
    }
    best.unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closer_box_wins_in_reversed_z() {
        let view = Mat4::look_to_rh(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z, Vec3::Y);
        let proj = crate::sim::projection(1.0);
        let far_box = Box3 {
            model: Mat4::from_translation(Vec3::new(0.0, 0.0, -2.0)),
        };
        let near_box = Box3 {
            model: Mat4::from_translation(Vec3::new(0.0, 0.0, 1.0)),
        };
        let only_far = expected_depth(
            50,
            50,
            101,
            101,
            view,
            proj,
            &[Box3 {
                model: far_box.model,
            }],
        );
        let both = expected_depth(50, 50, 101, 101, view, proj, &[far_box, near_box]);
        assert!(
            both > only_far,
            "ближний куб должен иметь большую reversed-Z глубину"
        );
        assert_eq!(expected_depth(0, 0, 101, 101, view, proj, &[]), 0.0);
    }

    #[test]
    fn pattern_reference_is_in_range() {
        for (x, y) in [(0, 0), (255, 255), (31, 32), (100, 7)] {
            for c in pattern_color(x, y, PATTERN_SIZE, SELFTEST_PATTERN_TIME) {
                assert!((0.0..=1.0).contains(&c));
            }
        }
    }
}
