//! Игровая зона с мелким рельефом (02.7, шаг 5) внутри поля острова.
//!
//! Зона — квадрат клеток острова [i0, i0 + cells) × [j0, j0 + cells); каждая
//! клетка делится на `ratio × ratio` (Якусима: 8, шаг 0,9375 м). Каждый
//! `ratio`-й узел зоны совпадает с узлом острова и имеет ту же высоту, поэтому
//! сетки стыкуются. Внешнее кольцо клеток острова, примыкающих к зоне стороной,
//! триангулируется веером от дальнего угла к узлам границы зоны — без
//! Т-образных стыков. Тот же разбор — у ходьбы (`sample`) и у кластеров (`mesh`).

use glam::{Vec2, Vec3};

use super::heightfield::HeightField;

#[derive(Clone, Debug)]
pub struct ZoneField {
    pub i0: usize,
    pub j0: usize,
    pub cells: usize,
    pub ratio: usize,
    /// Высоты зоны; начало координат поля — угол зоны (i0, j0).
    pub grid: HeightField,
}

/// Сторона клетки кольца, примыкающая к зоне.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    /// Зона к востоку от клетки (ребро b–c).
    East,
    /// Зона к западу (ребро a–d).
    West,
    /// Зона к югу (ребро d–c, больший z).
    South,
    /// Зона к северу (ребро a–b).
    North,
}

impl ZoneField {
    pub fn origin(&self, far: &HeightField) -> Vec2 {
        Vec2::new(self.i0 as f32, self.j0 as f32) * far.step_m
    }

    fn inside_cell(&self, i: usize, j: usize) -> bool {
        (self.i0..self.i0 + self.cells).contains(&i) && (self.j0..self.j0 + self.cells).contains(&j)
    }

    /// Клетка острова примыкает к зоне стороной (не углом)?
    fn ring_side(&self, i: usize, j: usize) -> Option<Side> {
        let (i0, j0, n) = (self.i0, self.j0, self.cells);
        let in_j = (j0..j0 + n).contains(&j);
        let in_i = (i0..i0 + n).contains(&i);
        if in_j && i + 1 == i0 {
            Some(Side::East)
        } else if in_j && i == i0 + n {
            Some(Side::West)
        } else if in_i && j + 1 == j0 {
            Some(Side::South)
        } else if in_i && j == j0 + n {
            Some(Side::North)
        } else {
            None
        }
    }

    /// Узлы ребра клетки кольца на границе зоны: позиции в мире по порядку от
    /// угла a/b/d к углу c (вдоль оси ребра).
    fn border_points(&self, far: &HeightField, i: usize, j: usize, side: Side) -> Vec<Vec3> {
        let r = self.ratio;
        let zs = self.grid.step_m;
        let o = self.origin(far);
        let (zi, zj, along_x) = match side {
            Side::East => ((i + 1 - self.i0) * r, (j - self.j0) * r, false),
            Side::West => ((i - self.i0) * r, (j - self.j0) * r, false),
            Side::South => ((i - self.i0) * r, (j + 1 - self.j0) * r, true),
            Side::North => ((i - self.i0) * r, (j - self.j0) * r, true),
        };
        (0..=r)
            .map(|k| {
                let (a, b) = if along_x { (zi + k, zj) } else { (zi, zj + k) };
                let (a, b) = (a.min(self.grid.nx - 1), b.min(self.grid.nz - 1));
                Vec3::new(
                    o.x + a as f32 * zs,
                    self.grid.heights[b * self.grid.nx + a],
                    o.y + b as f32 * zs,
                )
            })
            .collect()
    }

    /// Высота и нормаль грани в точке, если точка в зоне или в её кольце;
    /// `None` — точка на обычной части острова.
    pub fn sample(&self, far: &HeightField, x: f32, z: f32) -> Option<(f32, Vec3)> {
        let gx = x / far.step_m;
        let gz = z / far.step_m;
        if gx < 0.0 || gz < 0.0 {
            return None;
        }
        let (i, j) = (gx as usize, gz as usize);
        if self.inside_cell(i, j) {
            let o = self.origin(far);
            return self.grid.sample(x - o.x, z - o.y);
        }
        let side = self.ring_side(i, j)?;
        let p = Vec2::new(x, z);
        for t in self.fan(far, i, j, side) {
            if let Some(h) = triangle_height(&t, p) {
                let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize();
                return Some((h, if n.y < 0.0 { -n } else { n }));
            }
        }
        None
    }

    /// Веер треугольников клетки кольца (мировые позиции).
    fn fan(&self, far: &HeightField, i: usize, j: usize, side: Side) -> Vec<[Vec3; 3]> {
        let border = self.border_points(far, i, j, side);
        // Дальний угол, из которого идёт веер, и второй дальний угол.
        let (apex, other) = match side {
            Side::East => ((i, j), (i, j + 1)),
            Side::West => ((i + 1, j), (i + 1, j + 1)),
            Side::South => ((i, j), (i + 1, j)),
            Side::North => ((i, j + 1), (i + 1, j + 1)),
        };
        let a = far.position(apex.0, apex.1);
        let b = far.position(other.0, other.1);
        let mut tris: Vec<[Vec3; 3]> = border.windows(2).map(|w| [a, w[0], w[1]]).collect();
        // Замыкающий треугольник: апекс, второй дальний угол, ближний к нему конец границы.
        let near = if (border[0] - b).length() < (border[border.len() - 1] - b).length() {
            border[0]
        } else {
            border[border.len() - 1]
        };
        tris.push([a, b, near]);
        tris
    }
}

/// Высота плоскости треугольника в точке `p` (XZ), если точка внутри.
fn triangle_height(t: &[Vec3; 3], p: Vec2) -> Option<f32> {
    let (a, b, c) = (t[0], t[1], t[2]);
    let v0 = Vec2::new(b.x - a.x, b.z - a.z);
    let v1 = Vec2::new(c.x - a.x, c.z - a.z);
    let v2 = p - Vec2::new(a.x, a.z);
    let den = v0.x * v1.y - v1.x * v0.y;
    if den.abs() < 1e-9 {
        return None;
    }
    let v = (v2.x * v1.y - v1.x * v2.y) / den;
    let w = (v0.x * v2.y - v2.x * v0.y) / den;
    let e = -1e-5;
    (v >= e && w >= e && v + w <= 1.0 - e).then_some(a.y + v * (b.y - a.y) + w * (c.y - a.y))
}

/// Общая сетка острова и зоны для сборки кластеров.
pub struct CombinedMesh {
    /// Позиции узлов: острова (`0..nx·nz` по строкам), затем узлов зоны, не
    /// совпадающих с узлами острова.
    pub positions: Vec<[f32; 3]>,
    /// Треугольники, обход CCW сверху.
    pub indices: Vec<u32>,
    /// Для каждого добавленного узла зоны — его плоский номер в сетке зоны
    /// (`b · n + a`): GPU берёт высоту из поля зоны по этому номеру.
    pub extra_to_zone: Vec<u32>,
}

/// Общая сетка: клетки острова вне зоны, веер в кольце, клетки зоны.
pub fn combined_mesh(far: &HeightField, zone: &ZoneField) -> CombinedMesh {
    let far_count = far.nx * far.nz;
    let zn = zone.grid.nx;
    let r = zone.ratio;
    let o = zone.origin(far);
    let mut positions = far.positions();
    let mut zone_index = vec![0u32; zn * zn];
    let mut extra_to_zone = Vec::new();
    for b in 0..zn {
        for a in 0..zn {
            zone_index[b * zn + a] = if a % r == 0 && b % r == 0 {
                ((zone.j0 + b / r) * far.nx + zone.i0 + a / r) as u32
            } else {
                positions.push([
                    o.x + a as f32 * zone.grid.step_m,
                    zone.grid.heights[b * zn + a],
                    o.y + b as f32 * zone.grid.step_m,
                ]);
                extra_to_zone.push((b * zn + a) as u32);
                (positions.len() - 1) as u32
            };
        }
    }
    let _ = far_count;
    let mut idx = Vec::with_capacity((far.nx - 1) * (far.nz - 1) * 6 + (zn - 1) * (zn - 1) * 6);
    let push_ccw = |idx: &mut Vec<u32>, p: &Vec<[f32; 3]>, t: [u32; 3]| {
        let (a, b, c) = (p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]);
        // Обход CCW сверху (как у сетки острова): y-компонента нормали > 0.
        let cross_y = (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]);
        if cross_y >= 0.0 {
            idx.extend_from_slice(&t);
        } else {
            idx.extend_from_slice(&[t[0], t[2], t[1]]);
        }
    };
    let nx = far.nx as u32;
    for j in 0..far.nz - 1 {
        for i in 0..far.nx - 1 {
            if zone.inside_cell(i, j) {
                continue;
            }
            let a = j as u32 * nx + i as u32;
            let (b, d) = (a + 1, a + nx);
            let c = d + 1;
            match zone.ring_side(i, j) {
                None => {
                    idx.extend_from_slice(&[a, c, b, a, d, c]);
                }
                Some(side) => {
                    let (zi, zj, along_x) = match side {
                        Side::East => ((i + 1 - zone.i0) * r, (j - zone.j0) * r, false),
                        Side::West => ((i - zone.i0) * r, (j - zone.j0) * r, false),
                        Side::South => ((i - zone.i0) * r, (j + 1 - zone.j0) * r, true),
                        Side::North => ((i - zone.i0) * r, (j - zone.j0) * r, true),
                    };
                    let border: Vec<u32> = (0..=r)
                        .map(|k| {
                            let (za, zb) = if along_x { (zi + k, zj) } else { (zi, zj + k) };
                            zone_index[zb * zn + za]
                        })
                        .collect();
                    let (apex, other) = match side {
                        Side::East => (a, d),
                        Side::West => (b, c),
                        Side::South => (a, b),
                        Side::North => (d, c),
                    };
                    for w in border.windows(2) {
                        push_ccw(&mut idx, &positions, [apex, w[0], w[1]]);
                    }
                    // Замыкание: второй дальний угол с ближним концом границы.
                    let pb = positions[other as usize];
                    let d0 = Vec3::from(positions[border[0] as usize]) - Vec3::from(pb);
                    let d1 = Vec3::from(positions[border[r] as usize]) - Vec3::from(pb);
                    let near = if d0.length() < d1.length() {
                        border[0]
                    } else {
                        border[r]
                    };
                    push_ccw(&mut idx, &positions, [apex, other, near]);
                }
            }
        }
    }
    let zn32 = zn as u32;
    for b in 0..zn32 - 1 {
        for a in 0..zn32 - 1 {
            let ia = zone_index[(b * zn32 + a) as usize];
            let ib = zone_index[(b * zn32 + a + 1) as usize];
            let id = zone_index[((b + 1) * zn32 + a) as usize];
            let ic = zone_index[((b + 1) * zn32 + a + 1) as usize];
            idx.extend_from_slice(&[ia, ic, ib, ia, id, ic]);
        }
    }
    CombinedMesh {
        positions,
        indices: idx,
        extra_to_zone,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Остров 17 × 17 узлов по 8 м, зона — клетки [4, 12) × [5, 13) по 4 на клетку.
    fn sample_world() -> (HeightField, ZoneField) {
        let f = |x: f32, z: f32| {
            3.0 * (x * 0.05).sin() + 2.0 * (z * 0.07).cos() + 0.3 * (x * z * 0.01).sin()
        };
        let n = 17;
        let step = 8.0;
        let mut far_h = Vec::new();
        for j in 0..n {
            for i in 0..n {
                far_h.push(f(i as f32 * step, j as f32 * step));
            }
        }
        let mut far = HeightField::new(n, n, step, far_h);
        let (i0, j0, cells, r) = (4, 5, 8, 4);
        let zn = cells * r + 1;
        let zs = step / r as f32;
        let mut zh = Vec::new();
        for b in 0..zn {
            for a in 0..zn {
                let (x, z) = (
                    i0 as f32 * step + a as f32 * zs,
                    j0 as f32 * step + b as f32 * zs,
                );
                zh.push(f(x, z) + 0.4 * (x * 1.3).sin() * (z * 1.1).cos());
            }
        }
        for b in (0..zn).step_by(r) {
            for a in (0..zn).step_by(r) {
                far.heights[(j0 + b / r) * n + i0 + a / r] = zh[b * zn + a];
            }
        }
        let zone = ZoneField {
            i0,
            j0,
            cells,
            ratio: r,
            grid: HeightField::new(zn, zn, zs, zh),
        };
        (far, zone)
    }

    /// Каждое внутреннее ребро — у двух треугольников, Т-стыков нет; все
    /// треугольники обходятся CCW сверху и покрывают площадь острова.
    #[test]
    fn combined_mesh_is_watertight() {
        let (far, zone) = sample_world();
        let m = combined_mesh(&far, &zone);
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        let mut area = 0.0f32;
        for t in m.indices.as_chunks::<3>().0 {
            let p: Vec<Vec3> = t
                .iter()
                .map(|&v| Vec3::from(m.positions[v as usize]))
                .collect();
            let cy = (p[1].z - p[0].z) * (p[2].x - p[0].x) - (p[1].x - p[0].x) * (p[2].z - p[0].z);
            assert!(cy > 0.0, "треугольник {t:?} не CCW или вырожден");
            area += cy * 0.5;
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let size = (far.nx - 1) as f32 * far.step_m;
        assert!(
            (area - size * size).abs() < 1e-2 * size * size,
            "площадь {area}"
        );
        let on_border = |v: u32| {
            let p = m.positions[v as usize];
            p[0] <= 0.0 || p[2] <= 0.0 || p[0] >= size - 1e-3 || p[2] >= size - 1e-3
        };
        for (&(a, b), &k) in &edges {
            assert!(
                k == 2 || (k == 1 && on_border(a) && on_border(b)),
                "ребро ({a}, {b}) у {k}"
            );
        }
    }

    /// Ходьба совпадает с треугольниками общей сетки в зоне, кольце и снаружи.
    #[test]
    fn sample_matches_mesh_triangles() {
        let (far, zone) = sample_world();
        let m = combined_mesh(&far, &zone);
        let tris: Vec<[Vec3; 3]> = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| t.map(|v| Vec3::from(m.positions[v as usize])))
            .collect();
        let mut checked = 0;
        for k in 0..400 {
            let x = 20.0 + (k as f32 * 0.37).fract() * 88.0;
            let z = 20.0 + (k as f32 * 0.61).fract() * 88.0;
            let h = zone
                .sample(&far, x, z)
                .or_else(|| far.sample(x, z))
                .unwrap()
                .0;
            let want = tris
                .iter()
                .find_map(|t| triangle_height(t, Vec2::new(x, z)))
                .unwrap();
            assert!((h - want).abs() < 1e-3, "({x}, {z}): {h} вместо {want}");
            checked += 1;
        }
        assert_eq!(checked, 400);
    }

    /// Вдоль линии через границу зоны высота меняется без скачков.
    #[test]
    fn walking_across_zone_border_is_continuous() {
        let (far, zone) = sample_world();
        let h = |x: f32, z: f32| {
            zone.sample(&far, x, z)
                .or_else(|| far.sample(x, z))
                .unwrap()
                .0
        };
        let z = 60.3;
        let mut prev = h(10.0, z);
        for k in 1..1000 {
            let x = 10.0 + k as f32 * 0.1;
            let cur = h(x, z);
            assert!((cur - prev).abs() < 0.2, "скачок {prev} → {cur} у x = {x}");
            prev = cur;
        }
    }
}
