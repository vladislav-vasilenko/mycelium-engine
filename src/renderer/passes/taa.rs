//! Проход `taa` (04.5, сессия 1): временное сглаживание HDR-кадра сцены до
//! рук. Ядро `taa_resolve` пишет историю этого кадра (чётность номера кадра),
//! затем она копируется обратно в HDR — дальше кадр идёт как прежде.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx, size2};
use crate::renderer::frame::{TAA_OFFSET, Table};
use crate::renderer::resources::label;

pub(in crate::renderer) struct TaaPass;

impl FramePass for TaaPass {
    fn name(&self) -> &'static str {
        "taa"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        if ctx.input.taa.is_none() {
            return;
        }
        let t = ctx.targets;
        let cur = (ctx.frame_index % 2) as usize;
        let (write, read) = (&t.history[cur], &t.history[1 - cur]);
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("taa: resolve")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Taa);
        unsafe {
            table.setAddress_atIndex(ctx.upload_addr + TAA_OFFSET as u64, 0);
            table.setTexture_atIndex(t.hdr.gpuResourceID(), 0);
            table.setTexture_atIndex(t.depth.gpuResourceID(), 1);
            table.setTexture_atIndex(read.gpuResourceID(), 2);
            table.setTexture_atIndex(write.gpuResourceID(), 3);
        }
        enc.setComputePipelineState(&ctx.r.pipelines.taa_resolve);
        enc.setArgumentTable(Some(table));
        enc.dispatchThreads_threadsPerThreadgroup(
            size2(t.width as usize, t.height as usize),
            size2(16, 16),
        );
        // Ядро дописало историю — копия в HDR (тот же формат и размер).
        enc.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
            MTLStages::Dispatch,
            MTLStages::Blit,
            MTL4VisibilityOptions::Device,
        );
        unsafe { enc.copyFromTexture_toTexture(write, &t.hdr) };
        enc.endEncoding();
    }
}
