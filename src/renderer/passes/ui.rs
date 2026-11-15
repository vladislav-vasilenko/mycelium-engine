//! Проход `ui` (05.1): интерфейс поверх готового кадра в drawable.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx};
use crate::renderer::frame::Table;
use crate::renderer::resources::{label, upload_bytes};
use crate::renderer::ui::{UI_MAX_VERTS, UI_VERTS_OFFSET};

pub(in crate::renderer) struct UiPass;

impl FramePass for UiPass {
    fn name(&self) -> &'static str {
        "ui"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        let Some(ui) = &ctx.r.ui else { return };
        let n = ctx.input.ui.len().min(UI_MAX_VERTS);
        if n == 0 {
            return;
        }
        let buf = &ui.buffers[ctx.slot_index];
        let screen = [ctx.drawable.width() as f32, ctx.drawable.height() as f32];
        upload_bytes(buf, 0, bytemuck::cast_slice(&screen));
        upload_bytes(
            buf,
            UI_VERTS_OFFSET,
            bytemuck::cast_slice(&ctx.input.ui[..n]),
        );
        let rp = MTL4RenderPassDescriptor::new();
        let color = unsafe { rp.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(ctx.drawable));
        color.setLoadAction(MTLLoadAction::Load);
        color.setStoreAction(MTLStoreAction::Store);
        let enc = ctx
            .slot
            .cmd
            .renderCommandEncoderWithDescriptor(&rp)
            .expect("render encoder");
        enc.setLabel(Some(&label("ui overlay")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Ui);
        unsafe {
            table.setAddress_atIndex(buf.gpuAddress() + UI_VERTS_OFFSET as u64, 0);
            table.setAddress_atIndex(buf.gpuAddress(), 1);
            table.setTexture_atIndex(ui.font.gpuResourceID(), 0);
        }
        enc.setRenderPipelineState(&ctx.r.pipelines.ui);
        enc.setArgumentTable_atStages(table, MTLRenderStages::Vertex | MTLRenderStages::Fragment);
        unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, n) };
        enc.endEncoding();
    }
}
