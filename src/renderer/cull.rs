//! Выбор кластеров рельефа на GPU (02.5): буферы дерева групп, фронт обхода и
//! счётчики слотов, разметка списков видимых по видам.
//!
//! Видов `CULL_VIEWS`: 0 — камера, 1–4 — каскады теней. Списки видимых лежат
//! в буфере кластеров слота: вид 0 — с начала (`MAX_VISIBLE_CLUSTERS`),
//! каскады — следом по `CASCADE_CAP`.

use objc2::runtime::ProtocolObject;
use objc2_metal::*;

use super::frame::FRAME_SLOTS;
use super::resources::{as_allocation, buffer_with, shared_buffer, upload_bytes};
use super::shaders::Obj;
use super::terrain_clusters::MAX_VISIBLE_CLUSTERS;
use crate::layout::CullView;
use crate::terrain_clusters::CullData;

pub const CULL_VIEWS: usize = 5;
/// Ёмкость списка одного каскада теней.
pub const CASCADE_CAP: usize = 1 << 16;
/// Длина буфера видимых кластеров слота.
pub const VISIBLE_TOTAL: usize = MAX_VISIBLE_CLUSTERS + 4 * CASCADE_CAP;
/// u32 на вид в буфере счётчиков (`CULL_STRIDE` в `cull.metal`).
const STRIDE: usize = 64;
const OUT: usize = 48;
const OVERFLOW: usize = 49;
/// Начало indirect-аргументов в буфере счётчиков, байты.
pub const ARGS_OFFSET: usize = CULL_VIEWS * STRIDE * 4;
const COUNTERS_BYTES: usize = ARGS_OFFSET + CULL_VIEWS * 16;

/// Вход кадра: камеры видов (выключенный вид — `params.w` = 0).
#[derive(Clone, Copy, Debug)]
pub struct CullFrame {
    pub views: [CullView; CULL_VIEWS],
}

/// Итог выбора последнего завершённого кадра слота.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CullStats {
    pub visible: [u32; CULL_VIEWS],
    pub overflow: u32,
    /// Номер кадра, к которому относятся числа; 0 — ещё не было.
    pub frame: u64,
}

pub fn region_base(view: usize) -> usize {
    if view == 0 {
        0
    } else {
        MAX_VISIBLE_CLUSTERS + (view - 1) * CASCADE_CAP
    }
}

pub(super) struct CullGpu {
    pub nodes: Obj<dyn MTLBuffer>,
    pub groups: Obj<dyn MTLBuffer>,
    pub clusters: Obj<dyn MTLBuffer>,
    pub group_clusters: Obj<dyn MTLBuffer>,
    /// `CullLevel` по глубинам (16 Б).
    pub levels: Obj<dyn MTLBuffer>,
    /// `CullRegions` (48 Б).
    pub regions: Obj<dyn MTLBuffer>,
    /// Узлов на глубине — размер dispatch глубины.
    pub depth_sizes: Vec<u32>,
    roots: u32,
    /// На слот: фронт обхода (видов × узлов, u32) и счётчики с indirect-аргументами.
    pub frontier: Vec<Obj<dyn MTLBuffer>>,
    pub counters: Vec<Obj<dyn MTLBuffer>>,
    /// Кадр, чьи счётчики лежат в слоте; 0 — нет.
    pub counted_frame: [u64; FRAME_SLOTS],
}

impl CullGpu {
    pub fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        residency: &ProtocolObject<dyn MTLResidencySet>,
        d: &CullData,
    ) -> CullGpu {
        let buf = |bytes: &[u8], name: &str| buffer_with(device, bytes, name);
        let mut regions = [0u32; 12];
        for v in 0..4 {
            regions[v] = region_base(v) as u32;
            regions[4 + v] = if v == 0 {
                MAX_VISIBLE_CLUSTERS
            } else {
                CASCADE_CAP
            } as u32;
        }
        regions[8] = region_base(4) as u32;
        regions[9] = CASCADE_CAP as u32;
        regions[10] = CULL_VIEWS as u32;
        let total = d.nodes.len();
        let mut frontier = Vec::new();
        let mut counters = Vec::new();
        let roots: Vec<u32> = (0..d.roots).collect();
        for s in 0..FRAME_SLOTS {
            let f = shared_buffer(
                device,
                CULL_VIEWS * total * 4,
                &format!("cull frontier, slot {s}"),
            );
            // Фронт глубины 0 — корни, он не меняется.
            for v in 0..CULL_VIEWS {
                upload_bytes(&f, v * total * 4, bytemuck::cast_slice(&roots));
            }
            frontier.push(f);
            counters.push(shared_buffer(
                device,
                COUNTERS_BYTES,
                &format!("cull counters, slot {s}"),
            ));
        }
        let g = CullGpu {
            nodes: buf(bytemuck::cast_slice(&d.nodes), "cull: group tree"),
            groups: buf(bytemuck::cast_slice(&d.groups), "cull: group bounds"),
            clusters: buf(bytemuck::cast_slice(&d.clusters), "cull: clusters"),
            group_clusters: buf(
                bytemuck::cast_slice(&d.group_clusters),
                "cull: group clusters",
            ),
            levels: buf(bytemuck::cast_slice(&d.depths), "cull: levels"),
            regions: buf(bytemuck::cast_slice(&regions), "cull: regions"),
            depth_sizes: d.depth_sizes.clone(),
            roots: d.roots,
            frontier,
            counters,
            counted_frame: [0; FRAME_SLOTS],
        };
        for b in [
            &g.nodes,
            &g.groups,
            &g.clusters,
            &g.group_clusters,
            &g.levels,
            &g.regions,
        ]
        .into_iter()
        .chain(&g.frontier)
        .chain(&g.counters)
        {
            residency.addAllocation(as_allocation(&**b));
        }
        g
    }

    /// Счётчики завершённого кадра слота (до сброса) и сброс для нового кадра.
    pub fn take_and_reset(&mut self, slot: usize, frame: u64) -> Option<CullStats> {
        let c = &self.counters[slot];
        // SAFETY: shared-буфер слота; GPU закончил кадр слота (ожидание события).
        let words = unsafe {
            std::slice::from_raw_parts_mut(c.contents().as_ptr().cast::<u32>(), COUNTERS_BYTES / 4)
        };
        let stats = (self.counted_frame[slot] != 0).then(|| CullStats {
            visible: std::array::from_fn(|v| words[v * STRIDE + OUT]),
            overflow: (0..CULL_VIEWS).map(|v| words[v * STRIDE + OVERFLOW]).sum(),
            frame: self.counted_frame[slot],
        });
        words.fill(0);
        for v in 0..CULL_VIEWS {
            words[v * STRIDE] = self.roots;
        }
        self.counted_frame[slot] = frame;
        stats
    }
}
