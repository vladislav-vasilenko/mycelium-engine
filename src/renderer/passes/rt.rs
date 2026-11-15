//! Проход `rt` (этап 11, сессия 1): сетка рельефа у камеры, экземпляры
//! растений из списков леса этого кадра, затем нижняя структура рельефа и
//! косвенная верхняя структура — для лучей затенения в `scene`.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx, size2};
use crate::renderer::forest::FOREST_CAP;
use crate::renderer::frame::{SHADOW_OFFSET, TAA_OFFSET, Table};
use crate::renderer::resources::{label, upload_bytes};
use crate::renderer::rt::RT_GRID;

pub(in crate::renderer) struct RtPass;

/// Лучи в кадре: сцена для лучей есть, лес и тени нарисованы, TAA усредняет шум.
pub(in crate::renderer) fn active(ctx: &PassCtx) -> bool {
    ctx.input.rt
        && ctx.r.rt.is_some()
        && super::forest::active(ctx)
        && ctx.input.taa.is_some()
        && ctx.input.shadows.is_some()
}

impl FramePass for RtPass {
    fn name(&self) -> &'static str {
        "rt"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        if !active(ctx) {
            return;
        }
        let (rt, f, c) = (
            ctx.r.rt.as_ref().unwrap(),
            ctx.r.forest.as_ref().unwrap(),
            ctx.r.clusters.as_ref().unwrap(),
        );
        let s = ctx.slot_index;
        upload_bytes(&rt.count[s], 0, bytemuck::bytes_of(&0u32));
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("rt: scene for rays")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Rt);
        unsafe {
            table.setAddress_atIndex(c.heights.gpuAddress(), 0);
            table.setAddress_atIndex(ctx.upload_addr, 1);
            table.setAddress_atIndex(c.zone_heights.gpuAddress(), 2);
            table.setAddress_atIndex(c.zone_info.gpuAddress(), 3);
            table.setAddress_atIndex(rt.terrain_vertices[s].gpuAddress(), 4);
        }
        enc.setArgumentTable(Some(table));
        enc.setComputePipelineState(&ctx.r.pipelines.rt_terrain_grid);
        enc.dispatchThreads_threadsPerThreadgroup(size2(RT_GRID, RT_GRID), size2(16, 16));
        // Экземпляры: счётчики и списки леса этого кадра (проход `forest`).
        unsafe {
            table.setAddress_atIndex(f.counters[s].gpuAddress(), 0);
            table.setAddress_atIndex(f.instances[s].gpuAddress(), 1);
            table.setAddress_atIndex(rt.sources.gpuAddress(), 2);
            table.setAddress_atIndex(rt.blas_ids[s].gpuAddress(), 3);
            table.setAddress_atIndex(rt.instances[s].gpuAddress(), 4);
            table.setAddress_atIndex(rt.count[s].gpuAddress(), 5);
        }
        enc.setComputePipelineState(&ctx.r.pipelines.rt_instances);
        enc.dispatchThreads_threadsPerThreadgroup(
            size2(FOREST_CAP, rt.source_count() as usize),
            size2(256, 1),
        );
        enc.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
            MTLStages::Dispatch,
            MTLStages::Dispatch,
            MTL4VisibilityOptions::Device,
        );
        enc.setComputePipelineState(&ctx.r.pipelines.rt_count_finish);
        enc.dispatchThreads_threadsPerThreadgroup(size2(1, 1), size2(1, 1));
        enc.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
            MTLStages::Dispatch,
            MTLStages::AccelerationStructure,
            MTL4VisibilityOptions::Device,
        );
        unsafe {
            enc.buildAccelerationStructure_descriptor_scratchBuffer(
                &rt.terrain_blas[s],
                &rt.terrain_descriptor(s),
                rt.scratch_range(s),
            );
        }
        enc.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
            MTLStages::AccelerationStructure,
            MTLStages::AccelerationStructure,
            MTL4VisibilityOptions::Device,
        );
        unsafe {
            enc.buildAccelerationStructure_descriptor_scratchBuffer(
                &rt.tlas[s],
                &rt.tlas_descriptor(&rt.instances[s], &rt.count[s]),
                rt.scratch_range(s),
            );
        }
        enc.endEncoding();
    }
}

/// Проход `rt_ao` (11): лучи затенения по глубине кадра в половине разрешения.
pub(in crate::renderer) struct RtAoPass;

impl FramePass for RtAoPass {
    fn name(&self) -> &'static str {
        "rt_ao"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        if !active(ctx) {
            return;
        }
        let rt = ctx.r.rt.as_ref().unwrap();
        let t = ctx.targets;
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("rt: ambient occlusion")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Rt);
        unsafe {
            table.setTexture_atIndex(t.depth.gpuResourceID(), 0);
            table.setTexture_atIndex(t.rt_ao.gpuResourceID(), 1);
            table.setAddress_atIndex(ctx.upload_addr, 1);
            table.setAddress_atIndex(ctx.upload_addr + TAA_OFFSET as u64, 2);
            table.setResource_atBufferIndex(rt.tlas[ctx.slot_index].gpuResourceID(), 3);
            table.setAddress_atIndex(ctx.upload_addr + SHADOW_OFFSET as u64, 4);
        }
        enc.setArgumentTable(Some(table));
        enc.setComputePipelineState(&ctx.r.pipelines.rt_ao);
        enc.dispatchThreads_threadsPerThreadgroup(
            size2(t.rt_ao.width(), t.rt_ao.height()),
            size2(16, 16),
        );
        enc.endEncoding();
    }
}
