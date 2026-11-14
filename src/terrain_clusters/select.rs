//! Срез иерархии кластеров на CPU (02.3; на GPU — 02.5).
//!
//! Кластер рисуется, если упрощённая версия его группы уже слишком груба
//! (ошибка на экране > порога), а версия, из которой он получен, — достаточно
//! точна (≤ порога) или он из исходной сетки. Оба числа считаются от сфер
//! групп, общих для всех соседей, поэтому соседи решают одинаково и щелей нет.

use glam::{Mat4, Vec3, Vec4};

use super::{Bounds, ClusterSet};

/// Камера для выбора.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub camera: Vec3,
    /// Пикселей на радиан у центра экрана: `proj[1][1] · 0,5 · высота кадра`.
    pub pixels_per_unit: f32,
    pub znear: f32,
    /// Плоскости пирамиды видимости (xyz — нормаль внутрь, w — смещение).
    pub planes: [Vec4; 6],
    /// Порог экранной ошибки, пиксели.
    pub threshold_px: f32,
}

impl View {
    /// Камера из матриц вида и проекции reversed-Z (бесконечная дальняя плоскость).
    pub fn new(view: Mat4, proj: Mat4, height_px: f32, znear: f32, threshold_px: f32) -> View {
        let m = (proj * view).transpose();
        let (r0, r1, r2, r3) = (m.x_axis, m.y_axis, m.z_axis, m.w_axis);
        let norm = |p: Vec4| {
            let l = p.truncate().length();
            if l > 1e-6 {
                p / l
            } else {
                Vec4::new(0.0, 0.0, 0.0, 1.0)
            }
        };
        View {
            camera: view.inverse().w_axis.truncate(),
            pixels_per_unit: proj.y_axis.y * 0.5 * height_px,
            znear,
            // Левая, правая, нижняя, верхняя; reversed-Z: ближняя z ≤ w, дальняя z ≥ 0.
            planes: [
                norm(r3 + r0),
                norm(r3 - r0),
                norm(r3 + r1),
                norm(r3 - r1),
                norm(r3 - r2),
                norm(r2),
            ],
            threshold_px,
        }
    }

    /// Ошибка сферы на экране, пиксели (оценка без перспективного искажения,
    /// как в clusterlod: ошибка / расстояние до ближайшей точки сферы).
    pub fn projected_error(&self, b: &Bounds) -> f32 {
        if b.error.is_infinite() {
            return f32::INFINITY;
        }
        let d = ((b.center - self.camera).length() - b.radius).max(self.znear);
        b.error / d * self.pixels_per_unit
    }

    pub fn sphere_visible(&self, center: Vec3, radius: f32) -> bool {
        self.planes
            .iter()
            .all(|p| p.truncate().dot(center) + p.w >= -radius)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SelectStats {
    pub clusters: u32,
    pub triangles: u32,
    /// Отброшено пирамидой видимости среди выбранных по ошибке.
    pub culled: u32,
}

/// Список видимых кластеров среза в `out` (очищается). Обход деревьев над
/// группами: поддерево, чья наибольшая ошибка на экране уже ≤ порога, не
/// содержит групп, которые надо уточнять, и пропускается целиком.
pub fn select(set: &ClusterSet, view: &View, cull: bool, out: &mut Vec<u32>) -> SelectStats {
    out.clear();
    let mut stats = SelectStats::default();
    let t = view.threshold_px;
    let mut stack: Vec<u32> = (0..set.levels.min(set.nodes.len() as u32)).collect();
    while let Some(n) = stack.pop() {
        let node = &set.nodes[n as usize];
        if view.projected_error(&node.bounds) <= t {
            continue;
        }
        if node.group >= 0 {
            let [first, count] = set.group_clusters[node.group as usize];
            for i in first..first + count {
                test_cluster(set, view, cull, i as usize, out, &mut stats);
            }
        } else {
            stack.extend(node.child_offset..node.child_offset + node.child_count);
        }
    }
    stats
}

/// Тот же срез перебором всех кластеров — эталон для тестов обхода.
#[cfg(test)]
pub fn select_all(set: &ClusterSet, view: &View, cull: bool, out: &mut Vec<u32>) -> SelectStats {
    out.clear();
    let mut stats = SelectStats::default();
    for i in 0..set.info.len() {
        test_cluster(set, view, cull, i, out, &mut stats);
    }
    stats
}

fn test_cluster(
    set: &ClusterSet,
    view: &View,
    cull: bool,
    i: usize,
    out: &mut Vec<u32>,
    stats: &mut SelectStats,
) {
    let t = view.threshold_px;
    let c = &set.info[i];
    {
        let coarse_ok = view.projected_error(&set.groups[c.group as usize]) <= t;
        if coarse_ok {
            return;
        }
        let fine_ok = c.refined < 0 || view.projected_error(&set.groups[c.refined as usize]) <= t;
        if !fine_ok {
            return;
        }
        if cull && !view.sphere_visible(c.bounds.center, c.bounds.radius) {
            stats.culled += 1;
            return;
        }
        out.push(i as u32);
        stats.clusters += 1;
        stats.triangles += set.gpu[i].triangle_count;
    }
}
