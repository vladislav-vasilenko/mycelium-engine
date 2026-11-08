//! Проход `wet_sand`: маска мокрого песка от наката волн (03.1).

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx, size2};
use crate::renderer::frame::{SEA_OFFSET, Table};
use crate::renderer::resources::label;

pub(in crate::renderer) struct WetSandPass;

impl FramePass for WetSandPass {
    fn name(&self) -> &'static str {
        "wet_sand"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        // Без воды проход пустой; барьер следующего прохода от этого не зависит.
        let Some((w, t)) = ctx.water else { return };
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("wet sand update")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::WetSand);
        unsafe {
            table.setAddress_atIndex(ctx.upload_addr, 1);
            table.setAddress_atIndex(ctx.upload_addr + SEA_OFFSET as u64, 2);
            table.setTexture_atIndex(t.wetness.gpuResourceID(), 0);
            table.setTexture_atIndex(w.field.gpuResourceID(), 1);
        }
        enc.setComputePipelineState(&ctx.r.pipelines.wet_sand);
        enc.setArgumentTable(Some(table));
        enc.dispatchThreads_threadsPerThreadgroup(
            size2(t.wetness.width(), t.wetness.height()),
            size2(16, 16),
        );
        enc.endEncoding();
    }
}
