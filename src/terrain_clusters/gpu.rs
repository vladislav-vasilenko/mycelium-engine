//! Данные выбора кластеров на GPU (02.5): дерево групп, группы и кластеры в
//! раскладке `layout::Cull*`, разметка фронта обхода по глубинам дерева.

use super::{Bounds, ClusterSet, View};
use crate::layout::{CullBounds, CullCluster, CullNode, CullView};

/// Глубин дерева не больше: под них счётчики фронта в буфере счётчиков вида.
pub const MAX_DEPTH: usize = 48;

/// «Бесконечная» ошибка на GPU: шейдеры собираются с быстрой математикой,
/// где сравнение с INFINITY не определено.
const GPU_INFINITY: f32 = 1e30;

pub struct CullData {
    pub nodes: Vec<CullNode>,
    pub groups: Vec<CullBounds>,
    pub clusters: Vec<CullCluster>,
    pub group_clusters: Vec<[u32; 2]>,
    /// На глубину: (глубина, начало её фронта, начало фронта следующей, всего узлов).
    pub depths: Vec<[u32; 4]>,
    /// Узлов на глубине — наибольший возможный фронт (размер dispatch).
    pub depth_sizes: Vec<u32>,
    pub roots: u32,
}

fn sphere(b: &Bounds) -> [f32; 4] {
    [b.center.x, b.center.y, b.center.z, b.radius]
}

fn error(b: &Bounds) -> f32 {
    if b.error.is_finite() {
        b.error
    } else {
        GPU_INFINITY
    }
}

pub fn cull_data(set: &ClusterSet) -> CullData {
    let roots = set.levels.min(set.nodes.len() as u32);
    // Глубина узлов обходом в ширину от корней.
    let mut depth = vec![u32::MAX; set.nodes.len()];
    let mut queue: std::collections::VecDeque<u32> = (0..roots).collect();
    for r in 0..roots {
        depth[r as usize] = 0;
    }
    let mut sizes: Vec<u32> = Vec::new();
    while let Some(n) = queue.pop_front() {
        let d = depth[n as usize] as usize;
        if sizes.len() <= d {
            sizes.resize(d + 1, 0);
        }
        sizes[d] += 1;
        let node = &set.nodes[n as usize];
        if node.group < 0 {
            for c in node.child_offset..node.child_offset + node.child_count {
                depth[c as usize] = d as u32 + 1;
                queue.push_back(c);
            }
        }
    }
    assert!(sizes.len() <= MAX_DEPTH, "дерево групп глубже {MAX_DEPTH}");
    let total = set.nodes.len() as u32;
    let mut offsets = vec![0u32; sizes.len() + 1];
    for d in 0..sizes.len() {
        offsets[d + 1] = offsets[d] + sizes[d];
    }
    let depths = (0..sizes.len())
        .map(|d| [d as u32, offsets[d], offsets[d + 1], total])
        .collect();
    CullData {
        nodes: set
            .nodes
            .iter()
            .map(|n| CullNode {
                sphere: sphere(&n.bounds),
                error: error(&n.bounds),
                group: n.group,
                child_offset: n.child_offset,
                child_count: n.child_count,
            })
            .collect(),
        groups: set
            .groups
            .iter()
            .map(|b| CullBounds {
                sphere: sphere(b),
                error: error(b),
                _pad: [0.0; 3],
            })
            .collect(),
        clusters: set
            .info
            .iter()
            .map(|c| CullCluster {
                sphere: sphere(&c.bounds),
                group: c.group,
                refined: c.refined,
                _pad: [0; 2],
            })
            .collect(),
        group_clusters: set.group_clusters.clone(),
        depths,
        depth_sizes: sizes,
        roots,
    }
}

impl View {
    /// Камера в раскладке GPU.
    pub fn to_gpu(self) -> CullView {
        CullView {
            planes: self.planes.map(|p| p.to_array()),
            camera: self.camera.extend(self.pixels_per_unit).to_array(),
            params: [
                self.znear,
                self.threshold_px,
                if self.ortho { 1.0 } else { 0.0 },
                1.0,
            ],
        }
    }
}
