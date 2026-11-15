//! Проход `bloom` (04.5, сессия 2): цепочка вниз от HDR-кадра до 1/64 и
//! обратно до 1/2 — каждый уровень своей текстурой (`Targets.bloom_*`).
//! Итог `bloom_up[0]` подмешивает тонмаппинг.

use objc2::runtime::ProtocolObject;
use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx, size2};
use crate::renderer::frame::Table;
use crate::renderer::resources::label;
use crate::renderer::targets::BLOOM_LEVELS;

pub(in crate::renderer) struct BloomPass;

impl FramePass for BloomPass {
    fn name(&self) -> &'static str {
        "bloom"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        if !ctx.input.bloom {
            return;
        }
        let t = ctx.targets;
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("bloom chain")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Bloom);
        enc.setArgumentTable(Some(table));
        let between = || {
            enc.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
                MTLStages::Dispatch,
                MTLStages::Dispatch,
                MTL4VisibilityOptions::Device,
            )
        };
        let groups = |tex: &ProtocolObject<dyn MTLTexture>| size2(tex.width(), tex.height());
        enc.setComputePipelineState(&ctx.r.pipelines.bloom_down);
        for k in 0..BLOOM_LEVELS {
            let src = if k == 0 { &t.hdr } else { &t.bloom_down[k - 1] };
            let dst = &t.bloom_down[k];
            unsafe {
                table.setTexture_atIndex(src.gpuResourceID(), 0);
                table.setTexture_atIndex(dst.gpuResourceID(), 1);
            }
            enc.dispatchThreads_threadsPerThreadgroup(groups(dst), size2(16, 16));
            between();
        }
        enc.setComputePipelineState(&ctx.r.pipelines.bloom_up);
        for k in (0..BLOOM_LEVELS - 1).rev() {
            let low = if k == BLOOM_LEVELS - 2 {
                &t.bloom_down[k + 1]
            } else {
                &t.bloom_up[k + 1]
            };
            let dst = &t.bloom_up[k];
            unsafe {
                table.setTexture_atIndex(low.gpuResourceID(), 0);
                table.setTexture_atIndex(t.bloom_down[k].gpuResourceID(), 1);
                table.setTexture_atIndex(dst.gpuResourceID(), 2);
            }
            enc.dispatchThreads_threadsPerThreadgroup(groups(dst), size2(16, 16));
            if k > 0 {
                between();
            }
        }
        enc.endEncoding();
    }
}
