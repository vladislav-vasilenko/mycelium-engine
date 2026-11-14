//! Проход `tonemap`: HDR → drawable (экспозиция и ACES).

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx};
use crate::renderer::frame::Table;
use crate::renderer::resources::label;

pub(in crate::renderer) struct TonemapPass;

impl FramePass for TonemapPass {
    fn name(&self) -> &'static str {
        "tonemap"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        let rp = MTL4RenderPassDescriptor::new();
        let color = unsafe { rp.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(ctx.drawable));
        color.setLoadAction(MTLLoadAction::DontCare);
        color.setStoreAction(MTLStoreAction::Store);
        let enc = ctx
            .slot
            .cmd
            .renderCommandEncoderWithDescriptor(&rp)
            .expect("render encoder");
        enc.setLabel(Some(&label("tone map")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Tonemap);
        unsafe {
            table.setAddress_atIndex(ctx.upload_addr, 1);
            table.setAddress_atIndex(ctx.r.sky.state.gpuAddress(), 2);
            table.setTexture_atIndex(ctx.targets.hdr.gpuResourceID(), 0);
        }
        enc.setRenderPipelineState(&ctx.r.pipelines.tonemap);
        enc.setArgumentTable_atStages(table, MTLRenderStages::Vertex | MTLRenderStages::Fragment);
        unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) };
        enc.endEncoding();
    }
}
