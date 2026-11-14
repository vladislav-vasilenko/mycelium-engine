//! Скелет, позы и палитра матриц костей (глава 12 Грегори: поза — набор
//! локальных трансформаций суставов; палитра — глобальная матрица сустава,
//! умноженная на обратную матрицу привязки).

use glam::{Mat4, Quat, Vec3};

use crate::layout::SkinnedVertex;

/// Локальная трансформация сустава относительно родителя: сдвиг, поворот, масштаб.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trs {
    pub t: Vec3,
    pub r: Quat,
    pub s: Vec3,
}

impl Trs {
    pub fn to_mat4(self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.s, self.r, self.t)
    }

    /// Смешивание: линейно для сдвига и масштаба, сферически для поворота
    /// (по короткой дуге).
    pub fn blend(self, other: Trs, k: f32) -> Trs {
        Trs {
            t: self.t.lerp(other.t, k),
            r: self.r.slerp(other.r, k).normalize(),
            s: self.s.lerp(other.s, k),
        }
    }
}

/// Сустав: родитель в том же скелете, неподвижные узлы между ним и родителем,
/// трансформация покоя и обратная матрица привязки.
#[derive(Clone, Debug)]
pub struct Joint {
    pub name: String,
    pub parent: Option<usize>,
    /// Произведение локальных матриц узлов-не-суставов между родителем-суставом
    /// (или корнем сцены) и этим суставом.
    pub pre: Mat4,
    pub rest: Trs,
    pub inverse_bind: Mat4,
}

#[derive(Clone, Debug)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
    /// Порядок обхода: родитель раньше потомка.
    order: Vec<usize>,
}

/// Поза — локальные трансформации всех суставов скелета.
#[derive(Clone, Debug, PartialEq)]
pub struct Pose(pub Vec<Trs>);

impl Pose {
    /// Смешивание двух поз одного скелета, `k` = 0 → `self`, 1 → `other`.
    pub fn blend(&self, other: &Pose, k: f32) -> Pose {
        Pose(
            self.0
                .iter()
                .zip(&other.0)
                .map(|(a, b)| a.blend(*b, k))
                .collect(),
        )
    }
}

impl Skeleton {
    /// Скелет из суставов; родители должны ссылаться внутрь списка без циклов.
    pub fn new(joints: Vec<Joint>) -> Result<Skeleton, String> {
        let n = joints.len();
        let mut order = Vec::with_capacity(n);
        let mut placed = vec![false; n];
        // Топологическая сортировка: не больше n проходов.
        for _ in 0..n {
            for (i, j) in joints.iter().enumerate() {
                if !placed[i] && j.parent.is_none_or(|p| placed[p]) {
                    placed[i] = true;
                    order.push(i);
                }
            }
            if order.len() == n {
                break;
            }
        }
        if order.len() != n {
            return Err("скелет: цикл в иерархии суставов".into());
        }
        Ok(Skeleton { joints, order })
    }

    pub fn rest_pose(&self) -> Pose {
        Pose(self.joints.iter().map(|j| j.rest).collect())
    }

    pub fn joint(&self, name: &str) -> Option<usize> {
        self.joints.iter().position(|j| j.name == name)
    }

    /// Глобальные матрицы суставов в пространстве модели.
    pub fn globals(&self, pose: &Pose) -> Vec<Mat4> {
        let mut g = vec![Mat4::IDENTITY; self.joints.len()];
        for &i in &self.order {
            let j = &self.joints[i];
            let parent = j.parent.map_or(Mat4::IDENTITY, |p| g[p]);
            g[i] = parent * j.pre * pose.0[i].to_mat4();
        }
        g
    }

    /// Палитра скиннинга: `root × глобальная × обратная привязки` для каждого сустава.
    pub fn palette(&self, pose: &Pose, root: Mat4) -> Vec<Mat4> {
        self.globals(pose)
            .iter()
            .zip(&self.joints)
            .map(|(g, j)| root * *g * j.inverse_bind)
            .collect()
    }
}

/// Позиция вершины после скиннинга — эталон для ядра `skin_probe` (тот же
/// порядок операций, что и в `hands.metal`).
pub fn skin_position(v: &SkinnedVertex, palette: &[Mat4]) -> Vec3 {
    let p = Vec3::from(v.position).extend(1.0);
    let mut out = glam::Vec4::ZERO;
    for k in 0..4 {
        out += palette[v.joints[k] as usize] * p * v.weights[k];
    }
    out.truncate()
}
