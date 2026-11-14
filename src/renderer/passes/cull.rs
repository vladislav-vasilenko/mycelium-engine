//! Проход `cull` (02.5): срез кластеров рельефа на GPU для камеры и каскадов
//! теней. Dispatch на глубину дерева групп, между ними — барьеры внутри
//! кодировщика; в конце — indirect-аргументы отрисовки.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx, size2};
use crate::renderer::frame::{CULL_OFFSET, Table};
use crate::renderer::resources::label;

pub(in crate::renderer) struct CullPass;

fn between<E: MTL4CommandEncoder + objc2::Message>(enc: &E) {
    enc.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
        MTLStages::Dispatch,
        MTLStages::Dispatch,
        MTL4VisibilityOptions::Device,
    );
}

impl FramePass for CullPass {
    fn name(&self) -> &'static str {
        "cull"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        let (Some(_), Some(c)) = (&ctx.input.cull, &ctx.r.clusters) else {
            return;
        };
        let g = &c.cull;
        let s = ctx.slot_index;
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("cull: cluster cut on GPU")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Cull);
        unsafe {
            table.setAddress_atIndex(g.nodes.gpuAddress(), 0);
            table.setAddress_atIndex(g.groups.gpuAddress(), 1);
            table.setAddress_atIndex(g.clusters.gpuAddress(), 2);
            table.setAddress_atIndex(g.group_clusters.gpuAddress(), 3);
            table.setAddress_atIndex(ctx.upload_addr + CULL_OFFSET as u64, 4);
            table.setAddress_atIndex(g.frontier[s].gpuAddress(), 5);
            table.setAddress_atIndex(g.counters[s].gpuAddress(), 6);
            table.setAddress_atIndex(ctx.slot.visible.gpuAddress(), 7);
            table.setAddress_atIndex(g.regions.gpuAddress(), 9);
        }
        let views = crate::renderer::cull::CULL_VIEWS;
        enc.setComputePipelineState(&ctx.r.pipelines.cull_clusters);
        for (d, &n) in g.depth_sizes.iter().enumerate() {
            unsafe { table.setAddress_atIndex(g.levels.gpuAddress() + d as u64 * 16, 8) };
            enc.setArgumentTable(Some(table));
            enc.dispatchThreads_threadsPerThreadgroup(size2(n as usize, views), size2(64, 1));
            between(&*enc);
        }
        enc.setComputePipelineState(&ctx.r.pipelines.cull_finish);
        enc.setArgumentTable(Some(table));
        enc.dispatchThreads_threadsPerThreadgroup(size2(views, 1), size2(8, 1));
        enc.endEncoding();
    }
}
