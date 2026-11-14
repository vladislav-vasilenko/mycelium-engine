//! Кластерный рельеф в духе Nanite (02.3, AD-46).
//!
//! - `build` — сборка иерархии из поля высот через `meshopt::clod_build`
//!   (порт clusterlod из meshoptimizer): кластеры по ≤ 128 треугольников,
//!   группы, упрощение с замороженной границей группы, ошибки и сферы групп;
//! - `select` — срез иерархии по экранной ошибке и пирамиде видимости на CPU.
//!
//! Вершина кластера — номер узла поля высот: позицию и нормаль вершинный
//! шейдер берёт из поля, поэтому кластер хранит только 4 байта на вершину и
//! 4 байта на треугольник (три локальных индекса u8).

mod cache;
mod select;
#[cfg(test)]
mod tests;

use glam::Vec3;
use meshopt::{
    ClodMesh, VertexDataAdapter, clod_build, clod_build_hierarchy, clod_default_config,
    clod_local_indices,
};

use crate::layout::TerrainCluster;
use crate::world::heightfield::HeightField;

pub use cache::load_or_build;
pub use select::{SelectStats, View, select};

/// Ширина дерева над группами (детей у узла не больше).
const NODE_WIDTH: usize = 8;

/// Узел дерева над группами (по уровню иерархии — своё дерево): границы с
/// наибольшей ошибкой поддерева; лист ссылается на группу.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub bounds: Bounds,
    /// Лист: номер группы; внутренний узел: −1.
    pub group: i32,
    pub child_offset: u32,
    pub child_count: u32,
}

/// Треугольников в кластере не больше.
pub const MAX_TRIANGLES: usize = 128;

/// Сфера и ошибка в метрах мира.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub center: Vec3,
    pub radius: f32,
    /// Ошибка упрощения, м; `f32::INFINITY` — дальше упрощать некуда.
    pub error: f32,
}

impl Bounds {
    fn from_clod(b: &meshopt::ClodBounds) -> Bounds {
        Bounds {
            center: Vec3::from(b.center),
            radius: b.radius,
            // clusterlod помечает терминальные группы ошибкой FLT_MAX.
            error: if b.error >= f32::MAX * 0.5 {
                f32::INFINITY
            } else {
                b.error
            },
        }
    }
}

/// Кластер для выбора на CPU.
#[derive(Clone, Copy, Debug)]
pub struct ClusterInfo {
    /// Группа, в которой кластер лежит: её упрощённая версия — «родитель».
    pub group: u32,
    /// Группа, упрощением которой кластер получен; −1 — исходная сетка.
    pub refined: i32,
    /// Сфера для отсечения пирамидой видимости.
    pub bounds: Bounds,
}

/// Иерархия кластеров рельефа.
#[derive(Clone, Debug, Default)]
pub struct ClusterSet {
    /// Упрощённые границы и ошибка каждой группы.
    pub groups: Vec<Bounds>,
    pub info: Vec<ClusterInfo>,
    /// Для GPU: кластеры, номера узлов поля, треугольники (u8 × 3 в u32).
    pub gpu: Vec<TerrainCluster>,
    pub vertices: Vec<u32>,
    pub triangles: Vec<u32>,
    /// Уровней иерархии.
    pub levels: u32,
    /// Кластеры группы: (первый, число) — в `info`/`gpu` они идут подряд.
    pub group_clusters: Vec<[u32; 2]>,
    /// Лес деревьев над группами: узлы `0..levels` — корни уровней.
    pub nodes: Vec<Node>,
}

impl ClusterSet {
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// Треугольники кластера в номерах узлов поля высот.
    #[cfg(test)]
    pub fn cluster_triangles(&self, c: usize) -> impl Iterator<Item = [u32; 3]> + '_ {
        let g = self.gpu[c];
        let verts = &self.vertices[g.vertex_offset as usize..];
        self.triangles[g.triangle_offset as usize..(g.triangle_offset + g.triangle_count) as usize]
            .iter()
            .map(move |&t| {
                [
                    verts[(t & 0xff) as usize],
                    verts[((t >> 8) & 0xff) as usize],
                    verts[((t >> 16) & 0xff) as usize],
                ]
            })
    }
}

/// Собирает иерархию кластеров из всего поля высот.
pub fn build(hf: &HeightField) -> ClusterSet {
    build_mesh(&hf.positions(), &hf.triangles())
}

/// Иерархия для мира: поле острова или общая сетка острова и игровой зоны.
/// Номера узлов зоны в кластерах переводятся в `nx·nz + плоский номер в зоне`,
/// чтобы вершинный шейдер нашёл высоту в поле зоны.
pub fn build_world(world: &crate::world::World) -> ClusterSet {
    let Some(zone) = &world.zone else {
        return build(&world.heightfield);
    };
    let mesh = crate::world::zone::combined_mesh(&world.heightfield, zone);
    let mut set = build_mesh(&mesh.positions, &mesh.indices);
    let far_count = (world.heightfield.nx * world.heightfield.nz) as u32;
    for v in &mut set.vertices {
        if *v >= far_count {
            *v = far_count + mesh.extra_to_zone[(*v - far_count) as usize];
        }
    }
    set
}

/// Иерархия по произвольной сетке узлов (позиции и треугольники CCW сверху).
pub fn build_mesh(positions: &[[f32; 3]], indices: &[u32]) -> ClusterSet {
    let adapter =
        VertexDataAdapter::new(bytemuck::cast_slice(positions), 12, 0).expect("позиции поля высот");
    let config = clod_default_config(MAX_TRIANGLES);
    let mut set = ClusterSet::default();
    let mut max_depth = 0;
    let mut raw_groups = Vec::new();
    clod_build(
        config,
        &ClodMesh::new(indices, &adapter),
        |group, clusters| {
            let gid = set.groups.len() as u32;
            raw_groups.push(group);
            set.groups.push(Bounds::from_clod(&group.simplified));
            set.group_clusters
                .push([set.gpu.len() as u32, clusters.len() as u32]);
            max_depth = max_depth.max(group.depth.max(0) as u32);
            for c in clusters {
                let (verts, tris) = clod_local_indices(c.indices);
                set.gpu.push(TerrainCluster {
                    vertex_offset: set.vertices.len() as u32,
                    triangle_offset: set.triangles.len() as u32,
                    triangle_count: (tris.len() / 3) as u32,
                    depth: group.depth.max(0) as u32,
                });
                set.vertices.extend_from_slice(&verts);
                set.triangles.extend(
                    tris.as_chunks::<3>()
                        .0
                        .iter()
                        .map(|t| t[0] as u32 | (t[1] as u32) << 8 | (t[2] as u32) << 16),
                );
                set.info.push(ClusterInfo {
                    group: gid,
                    refined: c.refined,
                    bounds: Bounds::from_clod(&c.bounds),
                });
            }
            gid as i32
        },
    );
    set.levels = max_depth + 1;
    set.nodes = clod_build_hierarchy(&raw_groups, NODE_WIDTH, set.levels as usize)
        .iter()
        .map(|n| Node {
            bounds: Bounds::from_clod(&n.bounds),
            group: n.group,
            child_offset: n.child_offset,
            child_count: n.child_count,
        })
        .collect();
    set
}
