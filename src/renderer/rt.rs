//! Сцена для лучей (этап 11, сессия 1): ускоряющие структуры Metal 4.
//!
//! - Нижние структуры растений (кора, камень, почва — без карточек листвы)
//!   строятся один раз при загрузке леса: деревья LOD 1, валун, ствол, пень,
//!   вывороченное дерево.
//! - На слот кадра: сетка рельефа 96 × 96 м вокруг камеры (`rt_terrain_grid`)
//!   и её нижняя структура, экземпляры из списков леса (`rt_instances`) и
//!   косвенная верхняя структура (число экземпляров — в буфере GPU).
//!
//! Шейдеры рельефа и растений пускают по ней лучи затенения (`rt_ao.h`).

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSArray;
use objc2_metal::*;

use super::forest::{CAMERA_LISTS, ForestGpu};
use super::frame::FRAME_SLOTS;
use super::resources::{as_allocation, buffer_with, label, shared_buffer};
use super::shaders::Obj;
use crate::plants::PlantAssets;

/// Узлов сетки рельефа по стороне (`RT_GRID` в `rt.metal`).
pub const RT_GRID: usize = 161;
pub const RT_MAX_INSTANCES: usize = 8192;
/// `MTLIndirectAccelerationStructureInstanceDescriptor`, байт.
const INSTANCE_STRIDE: usize = 72;

/// Сетки растений для лучей: (вид, LOD, список леса с экземплярами).
/// Деревья — LOD 1 и списки теней (все в 160 м); валун и валежник — LOD 1
/// сетки и LOD 0 списка (у камеры).
const MESHES: [(usize, usize, usize); 10] = [
    (0, 1, CAMERA_LISTS),
    (1, 1, CAMERA_LISTS + 1),
    (2, 1, CAMERA_LISTS + 2),
    (3, 1, CAMERA_LISTS + 3),
    (4, 1, CAMERA_LISTS + 4),
    (5, 1, CAMERA_LISTS + 5),
    (6, 1, 6 * 3),
    (10, 1, 10 * 3),
    (11, 1, 11 * 3),
    (12, 1, 12 * 3),
];

pub(super) struct RtGpu {
    /// Нижние структуры растений (в порядке `MESHES`) и их индексы.
    meshes: Vec<Obj<dyn MTLAccelerationStructure>>,
    _mesh_indices: Obj<dyn MTLBuffer>,
    terrain_indices: Obj<dyn MTLBuffer>,
    /// (список, сетка) для `rt_instances`.
    pub sources: Obj<dyn MTLBuffer>,
    pub terrain_vertices: Vec<Obj<dyn MTLBuffer>>,
    pub terrain_blas: Vec<Obj<dyn MTLAccelerationStructure>>,
    pub instances: Vec<Obj<dyn MTLBuffer>>,
    pub count: Vec<Obj<dyn MTLBuffer>>,
    /// Идентификаторы нижних структур: 0 — рельеф слота, 1 + сетка — растения.
    pub blas_ids: Vec<Obj<dyn MTLBuffer>>,
    pub tlas: Vec<Obj<dyn MTLAccelerationStructure>>,
    pub scratch: Vec<Obj<dyn MTLBuffer>>,
    terrain_scratch_size: usize,
    /// Пробы освещённости (11, сессия 3): `PROBE_COUNT × 36` float4 и мировая
    /// клетка каждой пробы (int4) — общие для кадров.
    pub probes: Obj<dyn MTLBuffer>,
    pub probe_cells: Obj<dyn MTLBuffer>,
}

/// Проб в сетке (`PROBE_DIM` в `probes.h`: 32 × 8 × 32).
pub const PROBE_COUNT: usize = 32 * 8 * 32;
/// Лучей пробы за кадр (`PROBE_RAYS` в `rt.metal`).
pub const PROBE_RAYS: usize = 64;

fn range(b: &ProtocolObject<dyn MTLBuffer>, offset: usize, len: usize) -> MTL4BufferRange {
    MTL4BufferRange {
        bufferAddress: b.gpuAddress() + offset as u64,
        length: len as u64,
    }
}

fn resource_u64(id: MTLResourceID) -> u64 {
    // SAFETY: MTLResourceID — обёртка над u64 того же размера.
    unsafe { std::mem::transmute::<MTLResourceID, u64>(id) }
}

fn triangle_descriptor(
    vertices: MTL4BufferRange,
    stride: usize,
    indices: MTL4BufferRange,
    triangles: usize,
) -> Retained<MTL4PrimitiveAccelerationStructureDescriptor> {
    let g = MTL4AccelerationStructureTriangleGeometryDescriptor::new();
    unsafe {
        g.setVertexBuffer(vertices);
        g.setVertexStride(stride);
        g.setVertexFormat(MTLAttributeFormat::Float3);
        g.setIndexBuffer(indices);
        g.setIndexType(MTLIndexType::UInt32);
        g.setTriangleCount(triangles);
        g.setOpaque(true);
    }
    let d = MTL4PrimitiveAccelerationStructureDescriptor::new();
    let geo: Retained<MTL4AccelerationStructureGeometryDescriptor> = Retained::into_super(g);
    d.setGeometryDescriptors(Some(&NSArray::from_retained_slice(&[geo])));
    d
}

impl RtGpu {
    /// Нижние структуры растений строятся сразу (однократная команда).
    pub fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        queue: &ProtocolObject<dyn MTL4CommandQueue>,
        residency: &ProtocolObject<dyn MTLResidencySet>,
        forest: &ForestGpu,
        a: &PlantAssets,
    ) -> Result<RtGpu, String> {
        // Индексы без листвы: материалы 0, 1 (кора), 4 (камень), 8 (почва).
        let solid = |v: u32| matches!(a.vertices[v as usize].attr[0], 0 | 1 | 4 | 8);
        let mut indices: Vec<u32> = Vec::new();
        let mut spans = Vec::new();
        for &(species, lod, _) in &MESHES {
            let r = a.lods[species][lod];
            let start = indices.len();
            let tris = &a.indices[r.index_start as usize..(r.index_start + r.index_count) as usize];
            for t in tris.as_chunks::<3>().0 {
                let g = [
                    t[0] + r.base_vertex,
                    t[1] + r.base_vertex,
                    t[2] + r.base_vertex,
                ];
                if g.iter().all(|&v| solid(v)) {
                    indices.extend_from_slice(&g);
                }
            }
            spans.push((start, (indices.len() - start) / 3));
        }
        let mesh_indices = buffer_with(device, bytemuck::cast_slice(&indices), "rt: plant indices");
        residency.addAllocation(as_allocation(&*mesh_indices));
        let vbytes = a.vertices.len() * std::mem::size_of::<crate::layout::PlantVertex>();
        let mut meshes = Vec::new();
        let mut builds = Vec::new();
        let mut scratch_max = 0usize;
        for &(start, tris) in &spans {
            let d = triangle_descriptor(
                range(&forest.vertices, 0, vbytes),
                std::mem::size_of::<crate::layout::PlantVertex>(),
                range(&mesh_indices, start * 4, tris.max(1) * 12),
                tris,
            );
            let sizes = device.accelerationStructureSizesWithDescriptor(&d);
            let s = device
                .newAccelerationStructureWithSize(sizes.accelerationStructureSize)
                .ok_or("rt: нижняя структура растения")?;
            s.setLabel(Some(&label("rt: plant BLAS")));
            residency.addAllocation(as_allocation(&*s));
            scratch_max = scratch_max.max(sizes.buildScratchBufferSize);
            meshes.push(s);
            builds.push(d);
        }
        // Сетка рельефа: индексы общие для слотов.
        let n = RT_GRID as u32;
        let mut ti = Vec::with_capacity((RT_GRID - 1) * (RT_GRID - 1) * 6);
        for j in 0..n - 1 {
            for i in 0..n - 1 {
                let a0 = j * n + i;
                ti.extend_from_slice(&[a0, a0 + n + 1, a0 + 1, a0, a0 + n, a0 + n + 1]);
            }
        }
        let terrain_indices = buffer_with(device, bytemuck::cast_slice(&ti), "rt: terrain indices");
        residency.addAllocation(as_allocation(&*terrain_indices));
        let tri_count = (RT_GRID - 1) * (RT_GRID - 1) * 2;
        let vb = RT_GRID * RT_GRID * 12;
        let mut g = RtGpu {
            meshes,
            _mesh_indices: mesh_indices,
            sources: buffer_with(
                device,
                bytemuck::cast_slice(
                    &MESHES
                        .iter()
                        .enumerate()
                        .flat_map(|(m, &(_, _, list))| [list as u32, m as u32])
                        .collect::<Vec<_>>(),
                ),
                "rt: sources",
            ),
            terrain_indices,
            terrain_vertices: Vec::new(),
            terrain_blas: Vec::new(),
            instances: Vec::new(),
            count: Vec::new(),
            blas_ids: Vec::new(),
            tlas: Vec::new(),
            scratch: Vec::new(),
            terrain_scratch_size: 0,
            // Нули: w = 0 — проба не прогрета, шейдер её не берёт.
            probes: buffer_with(device, &vec![0u8; PROBE_COUNT * 36 * 16], "rt: probes"),
            probe_cells: buffer_with(
                device,
                bytemuck::cast_slice(&vec![i32::MIN; PROBE_COUNT * 4]),
                "rt: probe cells",
            ),
        };
        residency.addAllocation(as_allocation(&*g.probes));
        residency.addAllocation(as_allocation(&*g.probe_cells));
        residency.addAllocation(as_allocation(&*g.sources));
        for s in 0..FRAME_SLOTS {
            let tv = shared_buffer(device, vb, &format!("rt: terrain grid, slot {s}"));
            let td = triangle_descriptor(
                range(&tv, 0, vb),
                12,
                range(&g.terrain_indices, 0, ti.len() * 4),
                tri_count,
            );
            td.setUsage(MTLAccelerationStructureUsage::PreferFastBuild);
            let tsz = device.accelerationStructureSizesWithDescriptor(&td);
            let tblas = device
                .newAccelerationStructureWithSize(tsz.accelerationStructureSize)
                .ok_or("rt: нижняя структура рельефа")?;
            tblas.setLabel(Some(&label(&format!("rt: terrain BLAS, slot {s}"))));
            let inst = shared_buffer(
                device,
                RT_MAX_INSTANCES * INSTANCE_STRIDE,
                &format!("rt: instances, slot {s}"),
            );
            let count = shared_buffer(device, 16, &format!("rt: instance count, slot {s}"));
            let mut ids = vec![resource_u64(tblas.gpuResourceID())];
            ids.extend(g.meshes.iter().map(|m| resource_u64(m.gpuResourceID())));
            let ids = buffer_with(
                device,
                bytemuck::cast_slice(&ids),
                &format!("rt: BLAS ids, slot {s}"),
            );
            let td2 = g.tlas_descriptor(&inst, &count);
            let isz = device.accelerationStructureSizesWithDescriptor(&td2);
            let tlas = device
                .newAccelerationStructureWithSize(isz.accelerationStructureSize)
                .ok_or("rt: верхняя структура")?;
            tlas.setLabel(Some(&label(&format!("rt: TLAS, slot {s}"))));
            let scratch_size = isz
                .buildScratchBufferSize
                .max(tsz.buildScratchBufferSize)
                .max(scratch_max);
            g.terrain_scratch_size = g.terrain_scratch_size.max(scratch_size);
            let scratch = device
                .newBufferWithLength_options(
                    scratch_size.max(16),
                    MTLResourceOptions::StorageModePrivate,
                )
                .ok_or("rt: scratch")?;
            for b in [&tv, &inst, &count, &ids, &scratch] {
                residency.addAllocation(as_allocation(&**b));
            }
            residency.addAllocation(as_allocation(&*tblas));
            residency.addAllocation(as_allocation(&*tlas));
            g.terrain_vertices.push(tv);
            g.terrain_blas.push(tblas);
            g.instances.push(inst);
            g.count.push(count);
            g.blas_ids.push(ids);
            g.tlas.push(tlas);
            g.scratch.push(scratch);
        }
        residency.commit();
        // Однократная постройка нижних структур растений.
        let allocator = device.newCommandAllocator().ok_or("rt allocator")?;
        let cmd = device.newCommandBuffer().ok_or("rt command buffer")?;
        cmd.setLabel(Some(&label("rt: plant BLAS build")));
        cmd.beginCommandBufferWithAllocator(&allocator);
        let enc = cmd.computeCommandEncoder().ok_or("rt encoder")?;
        for (s, d) in g.meshes.iter().zip(&builds) {
            unsafe {
                enc.buildAccelerationStructure_descriptor_scratchBuffer(
                    s,
                    d,
                    range(&g.scratch[0], 0, g.scratch[0].length() as usize),
                );
            }
            enc.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
                MTLStages::AccelerationStructure,
                MTLStages::AccelerationStructure,
                MTL4VisibilityOptions::Device,
            );
        }
        enc.endEncoding();
        cmd.endCommandBuffer();
        let done = device.newSharedEvent().ok_or("rt event")?;
        let mut ptr = std::ptr::NonNull::from(&*cmd);
        unsafe { queue.commit_count(std::ptr::NonNull::from(&mut ptr), 1) };
        let ev: &ProtocolObject<dyn MTLEvent> = ProtocolObject::from_ref(&*done);
        queue.signalEvent_value(ev, 1);
        if !done.waitUntilSignaledValue_timeoutMS(1, 10_000) {
            return Err("rt: постройка нижних структур не завершилась за 10 с".into());
        }
        Ok(g)
    }

    /// Косвенная верхняя структура слота: экземпляры и их число — в буферах GPU.
    pub fn tlas_descriptor(
        &self,
        inst: &ProtocolObject<dyn MTLBuffer>,
        count: &ProtocolObject<dyn MTLBuffer>,
    ) -> Retained<MTL4IndirectInstanceAccelerationStructureDescriptor> {
        let d = MTL4IndirectInstanceAccelerationStructureDescriptor::new();
        unsafe {
            d.setInstanceDescriptorBuffer(range(inst, 0, RT_MAX_INSTANCES * INSTANCE_STRIDE));
            d.setInstanceDescriptorStride(INSTANCE_STRIDE);
            d.setMaxInstanceCount(RT_MAX_INSTANCES);
            d.setInstanceCountBuffer(range(count, 0, 4));
            d.setInstanceDescriptorType(MTLAccelerationStructureInstanceDescriptorType::Indirect);
        }
        d.setUsage(MTLAccelerationStructureUsage::PreferFastBuild);
        d
    }

    /// Описание нижней структуры рельефа слота.
    pub fn terrain_descriptor(
        &self,
        slot: usize,
    ) -> Retained<MTL4PrimitiveAccelerationStructureDescriptor> {
        let vb = RT_GRID * RT_GRID * 12;
        let d = triangle_descriptor(
            range(&self.terrain_vertices[slot], 0, vb),
            12,
            range(&self.terrain_indices, 0, (RT_GRID - 1) * (RT_GRID - 1) * 24),
            (RT_GRID - 1) * (RT_GRID - 1) * 2,
        );
        d.setUsage(MTLAccelerationStructureUsage::PreferFastBuild);
        d
    }

    pub fn scratch_range(&self, slot: usize) -> MTL4BufferRange {
        range(&self.scratch[slot], 0, self.terrain_scratch_size.max(16))
    }

    pub fn source_count(&self) -> u32 {
        MESHES.len() as u32
    }
}
