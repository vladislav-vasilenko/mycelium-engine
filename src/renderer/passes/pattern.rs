//! Проход `pattern`: compute-узор для проверки цепочки compute → fragment.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx, size2};
use crate::reference::PATTERN_SIZE;
use crate::renderer::frame::Table;
use crate::renderer::resources::label;

pub(in crate::renderer) struct PatternPass;

impl FramePass for PatternPass {
    fn name(&self) -> &'static str {
        "pattern"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("pattern compute")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Pattern);
        unsafe {
            table.setTexture_atIndex(ctx.r.pattern_tex.gpuResourceID(), 0);
            table.setAddress_atIndex(ctx.upload_addr, 0);
        }
        enc.setComputePipelineState(&ctx.r.pipelines.pattern);
        enc.setArgumentTable(Some(table));
        enc.dispatchThreads_threadsPerThreadgroup(
            size2(PATTERN_SIZE as usize, PATTERN_SIZE as usize),
            size2(16, 16),
        );
        enc.endEncoding();
    }
}
