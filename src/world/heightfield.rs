//! Поле высот полного разрешения (02.3): источник кластеров рельефа и ходьбы.
//!
//! У генератора это сетка мира (1 м), у мира из реальных данных — более
//! мелкая сетка, чем слои биомов (Якусима: 7,5 м против 30 м). Триангуляция
//! ячейки (i, j) — диагональ a–c, треугольники (a, c, b) и (a, d, c), как у
//! тайлов LOD 0 и у кластеров уровня 0: ходьба идёт по видимой вблизи сетке.

use glam::{Vec2, Vec3};

#[derive(Clone, Debug)]
pub struct HeightField {
    pub nx: usize,
    pub nz: usize,
    /// Шаг сетки, м. Точка (i, j) лежит в мире в (i·step, h, j·step).
    pub step_m: f32,
    /// Высоты по строкам z.
    pub heights: Vec<f32>,
}

impl HeightField {
    pub fn new(nx: usize, nz: usize, step_m: f32, heights: Vec<f32>) -> HeightField {
        assert_eq!(heights.len(), nx * nz, "поле высот {nx}×{nz}");
        HeightField {
            nx,
            nz,
            step_m,
            heights,
        }
    }

    /// Высота узла; за краем — ближайший узел.
    pub fn at(&self, i: isize, j: isize) -> f32 {
        let i = i.clamp(0, self.nx as isize - 1) as usize;
        let j = j.clamp(0, self.nz as isize - 1) as usize;
        self.heights[j * self.nx + i]
    }

    /// Нормаль узла центральными разностями (у края — с повтором крайнего узла),
    /// тот же расчёт, что в `World::from_grid` и в шейдере кластеров.
    #[cfg(test)]
    pub fn normal_at(&self, i: usize, j: usize) -> Vec3 {
        let (i, j) = (i as isize, j as isize);
        let dx = (self.at(i + 1, j) - self.at(i - 1, j)) / (2.0 * self.step_m);
        let dz = (self.at(i, j + 1) - self.at(i, j - 1)) / (2.0 * self.step_m);
        Vec3::new(-dx, 1.0, -dz).normalize()
    }

    pub fn size_m(&self) -> Vec2 {
        Vec2::new(
            (self.nx - 1) as f32 * self.step_m,
            (self.nz - 1) as f32 * self.step_m,
        )
    }

    /// Мировая позиция узла.
    pub fn position(&self, i: usize, j: usize) -> Vec3 {
        Vec3::new(
            i as f32 * self.step_m,
            self.heights[j * self.nx + i],
            j as f32 * self.step_m,
        )
    }

    /// Высота и нормаль грани треугольника под точкой; `None` за картой.
    pub fn sample(&self, x: f32, z: f32) -> Option<(f32, Vec3)> {
        let size = self.size_m();
        if !(0.0..=size.x).contains(&x) || !(0.0..=size.y).contains(&z) {
            return None;
        }
        let gx = x / self.step_m;
        let gz = z / self.step_m;
        let i = (gx.floor() as usize).min(self.nx - 2);
        let j = (gz.floor() as usize).min(self.nz - 2);
        let (fx, fz) = (gx - i as f32, gz - j as f32);
        let (ii, jj) = (i as isize, j as isize);
        let a = self.at(ii, jj);
        let b = self.at(ii + 1, jj);
        let c = self.at(ii + 1, jj + 1);
        let d = self.at(ii, jj + 1);
        let s = self.step_m;
        Some(if fx >= fz {
            // Треугольник (a, c, b).
            let h = a + fx * (b - a) + fz * (c - b);
            let n = Vec3::new(s, c - a, s).cross(Vec3::new(s, b - a, 0.0));
            (h, n.normalize())
        } else {
            // Треугольник (a, d, c).
            let h = a + fz * (d - a) + fx * (c - d);
            let n = Vec3::new(0.0, d - a, s).cross(Vec3::new(s, c - a, s));
            (h, n.normalize())
        })
    }

    /// Треугольники всей сетки (индексы узлов), тот же разбор, что у `sample`.
    pub fn triangles(&self) -> Vec<u32> {
        let nx = self.nx as u32;
        let mut idx = Vec::with_capacity((self.nx - 1) * (self.nz - 1) * 6);
        for j in 0..self.nz as u32 - 1 {
            for i in 0..nx - 1 {
                let a = j * nx + i;
                let b = a + 1;
                let d = a + nx;
                let c = d + 1;
                idx.extend_from_slice(&[a, c, b, a, d, c]);
            }
        }
        idx
    }

    /// Позиции всех узлов (для сборки кластеров).
    pub fn positions(&self) -> Vec<[f32; 3]> {
        let mut p = Vec::with_capacity(self.nx * self.nz);
        for j in 0..self.nz {
            for i in 0..self.nx {
                p.push(self.position(i, j).to_array());
            }
        }
        p
    }
}
