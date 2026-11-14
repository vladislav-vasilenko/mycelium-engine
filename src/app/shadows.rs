//! Каскады теней кадра (04.2): три каскада вокруг камеры и один на весь остров.
//! Матрицы света — в f64 с привязкой к сетке текселей (тень не «ползёт» при
//! движении камеры); на GPU уходит `vp_rel` — проекция света × вид относительно
//! камеры, как у основной камеры (AD-49).

#[cfg(test)]
use glam::DVec4;
use glam::{DMat4, DVec3, Vec3};

use crate::layout::ShadowUniforms;
use crate::renderer::{CASCADES, SHADOW_SIZE, ShadowFrame};
use crate::terrain_clusters::{ClusterSet, View, select};

/// Полуширина каскадов вокруг камеры, м; каскад 3 накрывает весь остров.
pub const CASCADE_RADII_M: [f64; 3] = [30.0, 150.0, 800.0];
/// Запас глубины к свету и от него сверх полуширины каскада: хребты до 2 км
/// высотой отбрасывают тень в каскад, даже если сами вне его.
const DEPTH_MARGIN_M: f64 = 4000.0;

/// Ортографическая камера света: центр, полуширина, направление к свету.
/// Центр привязан к сетке текселей в осях света.
pub fn light_view_proj(center: DVec3, radius: f64, to_light: DVec3) -> DMat4 {
    let up = if to_light.y.abs() > 0.99 {
        DVec3::Z
    } else {
        DVec3::Y
    };
    let rot = DMat4::look_to_rh(DVec3::ZERO, -to_light, up);
    let texel = 2.0 * radius / SHADOW_SIZE as f64;
    let c = rot.transform_point3(center);
    let snapped = DVec3::new(
        (c.x / texel).round() * texel,
        (c.y / texel).round() * texel,
        c.z,
    );
    let eye = rot.inverse().transform_point3(snapped);
    let view = DMat4::look_to_rh(eye, -to_light, up);
    let depth = radius + DEPTH_MARGIN_M;
    let proj = DMat4::orthographic_rh(-radius, radius, -radius, radius, -depth, depth);
    proj * view
}

/// Каскады кадра и камеры их выбора кластеров. С `set` (выбор на CPU) списки
/// каскадов идут подряд в `visible` (очищается); `budget` — сколько места в
/// буфере кластеров слота осталось после основного списка. Без `set` выбор
/// делает GPU (02.5) по возвращённым камерам. `None` — светило под горизонтом.
pub fn shadow_frame(
    set: Option<&ClusterSet>,
    camera: Vec3,
    map_size_m: f32,
    to_light: Vec3,
    budget: usize,
    visible: &mut Vec<u32>,
    scratch: &mut Vec<u32>,
) -> Option<(ShadowFrame, [View; CASCADES])> {
    visible.clear();
    if to_light.y <= 0.0 {
        return None;
    }
    let l = to_light.as_dvec3().normalize();
    let cam = camera.as_dvec3();
    let half = map_size_m as f64 * 0.5;
    let island = DVec3::new(half, 900.0, half);
    let cascades: [(DVec3, f64); CASCADES] = [
        (cam, CASCADE_RADII_M[0]),
        (cam, CASCADE_RADII_M[1]),
        (cam, CASCADE_RADII_M[2]),
        (island, half * std::f64::consts::SQRT_2 + 1000.0),
    ];
    let mut u = ShadowUniforms {
        vp_rel: [[[0.0; 4]; 4]; CASCADES],
        splits: [
            CASCADE_RADII_M[0] as f32,
            CASCADE_RADII_M[1] as f32,
            CASCADE_RADII_M[2] as f32,
            f32::INFINITY,
        ],
        texel_m: [0.0; 4],
        params: [1.0, 0.0, 0.0, 0.0],
    };
    let mut ranges = [(0u32, 0u32); CASCADES];
    let mut views = [View::ortho(glam::Mat4::IDENTITY, 1.0, 1.0); CASCADES];
    for (k, &(center, radius)) in cascades.iter().enumerate() {
        let vp = light_view_proj(center, radius, l);
        // Относительно камеры: мир = rel + камера.
        let vp_rel = vp * DMat4::from_translation(cam);
        u.vp_rel[k] = vp_rel.as_mat4().to_cols_array_2d();
        let texel = 2.0 * radius / SHADOW_SIZE as f64;
        u.texel_m[k] = texel as f32;
        let view = View::ortho(vp.as_mat4(), (1.0 / texel) as f32, 1.0);
        views[k] = view;
        let Some(set) = set else { continue };
        select(set, &view, true, scratch);
        let start = visible.len();
        let n = scratch.len().min(budget - start);
        visible.extend_from_slice(&scratch[..n]);
        ranges[k] = (start as u32, n as u32);
    }
    Some((
        ShadowFrame {
            uniforms: u,
            ranges,
        },
        views,
    ))
}

/// Точка в мире → (u, v, глубина) карты каскада; для тестов.
#[cfg(test)]
pub fn project(vp: DMat4, p: DVec3) -> DVec3 {
    let c = vp * DVec4::new(p.x, p.y, p.z, 1.0);
    DVec3::new(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5, c.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Точка ближе к свету — меньшая глубина; центр каскада — в середине карты.
    #[test]
    fn light_depth_grows_away_from_light() {
        let l = DVec3::new(0.3, 0.8, -0.2).normalize();
        let c = DVec3::new(1234.5, 300.0, 777.7);
        let vp = light_view_proj(c, 150.0, l);
        let mid = project(vp, c);
        assert!((mid.x - 0.5).abs() < 2.0 / SHADOW_SIZE as f64);
        assert!((mid.y - 0.5).abs() < 2.0 / SHADOW_SIZE as f64);
        let near = project(vp, c + l * 100.0);
        assert!(near.z < mid.z && (0.0..1.0).contains(&near.z));
    }

    /// Сдвиг камеры на долю текселя не двигает сетку текселей карты.
    #[test]
    fn cascades_snap_to_texels() {
        let l = DVec3::new(-0.5, 0.6, 0.3).normalize();
        let r = 30.0;
        let texel = 2.0 * r / SHADOW_SIZE as f64;
        let p = DVec3::new(500.0, 50.0, 500.0);
        let a = project(light_view_proj(DVec3::new(10.0, 20.0, 30.0), r, l), p);
        let b = project(
            light_view_proj(DVec3::new(10.0 + 0.2 * texel, 20.0, 30.0), r, l),
            p,
        );
        let frac = |v: f64| (v * SHADOW_SIZE as f64).fract();
        assert!((frac(a.x) - frac(b.x)).abs() < 1e-6 || (frac(a.x) - frac(b.x)).abs() > 1.0 - 1e-6);
        assert!((frac(a.y) - frac(b.y)).abs() < 1e-6 || (frac(a.y) - frac(b.y)).abs() > 1.0 - 1e-6);
    }
}
