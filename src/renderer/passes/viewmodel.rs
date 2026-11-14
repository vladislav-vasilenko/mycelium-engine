//! Проход `viewmodel`: руки поверх готовой сцены своей проекцией и с очищенной
//! глубиной — руки не уходят в склон и не обрезаются ближней плоскостью мира.
//! Идёт после `readback`, поэтому проверки сцены видят мир без рук.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx};
use crate::renderer::frame::{HANDS_OFFSET, PALETTE_OFFSET, Table};
use crate::renderer::resources::label;

pub(in crate::renderer) struct ViewmodelPass;

impl FramePass for ViewmodelPass {
    fn name(&self) -> &'static str {
        "viewmodel"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        let (Some(_), Some(h)) = (&ctx.input.hands, &ctx.r.hands) else {
            return;
        };
        let rp = MTL4RenderPassDescriptor::new();
        let color = unsafe { rp.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(&ctx.targets.hdr));
        color.setLoadAction(MTLLoadAction::Load);
        color.setStoreAction(MTLStoreAction::Store);
        let depth = rp.depthAttachment();
        depth.setTexture(Some(&ctx.targets.depth));
        depth.setLoadAction(MTLLoadAction::Clear);
        depth.setClearDepth(0.0);
        depth.setStoreAction(MTLStoreAction::DontCare);
        let enc = ctx
            .slot
            .cmd
            .renderCommandEncoderWithDescriptor(&rp)
            .expect("render encoder");
        enc.setLabel(Some(&label("viewmodel: hands")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Hands);
        unsafe {
            table.setAddress_atIndex(h.vertices.gpuAddress(), 0);
            table.setAddress_atIndex(ctx.upload_addr + HANDS_OFFSET as u64, 1);
            table.setAddress_atIndex(ctx.upload_addr + PALETTE_OFFSET as u64, 2);
            table.setAddress_atIndex(ctx.upload_addr, 3);
            table.setTexture_atIndex(h.albedo.gpuResourceID(), 0);
        }
        enc.setRenderPipelineState(&ctx.r.pipelines.hands);
        enc.setDepthStencilState(Some(&ctx.r.depth_state));
        // Материал двусторонний: рукава видны изнутри. Лицевые грани glTF —
        // против часовой стрелки (у Metal по умолчанию — по часовой); шейдер
        // разворачивает нормаль у задних граней по `front_facing`.
        enc.setFrontFacingWinding(MTLWinding::CounterClockwise);
        enc.setCullMode(MTLCullMode::None);
        enc.setArgumentTable_atStages(table, MTLRenderStages::Vertex | MTLRenderStages::Fragment);
        unsafe {
            enc.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferLength(
                MTLPrimitiveType::Triangle,
                h.index_count,
                MTLIndexType::UInt16,
                h.indices.gpuAddress(),
                h.indices.length(),
            );
        }
        enc.endEncoding();
    }
}
