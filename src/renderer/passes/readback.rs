//! Проход `readback`: раскладка структур на GPU, копии узора, глубины и HDR
//! в контрольных точках, проверка функции волн.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx, size2};
use crate::layout::WATER_PROBES;
use crate::reference::PATTERN_SIZE;
use crate::renderer::frame::{SEA_OFFSET, Table};
use crate::renderer::readback::{
    RB_DEPTH_OFFSET, RB_HDR_OFFSET, RB_PATTERN_BYTES, RB_PROBE_OFFSET, RB_WATER_OFFSET,
    sample_pixels,
};
use crate::renderer::resources::label;

pub(in crate::renderer) struct ReadbackPass;

impl FramePass for ReadbackPass {
    fn name(&self) -> &'static str {
        "readback"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        let r = ctx.r;
        let rb = &r.readback_buf;
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("readback copy")));
        barrier.apply_always(&*enc);
        let table = ctx.slot.table(Table::Readback);
        unsafe { table.setAddress_atIndex(rb.gpuAddress() + RB_PROBE_OFFSET as u64, 0) };
        enc.setComputePipelineState(&r.pipelines.layout_probe);
        enc.setArgumentTable(Some(table));
        enc.dispatchThreads_threadsPerThreadgroup(size2(1, 1), size2(1, 1));
        unsafe {
            enc.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                &r.pattern_tex,
                0,
                0,
                MTLOrigin { x: 0, y: 0, z: 0 },
                size2(PATTERN_SIZE as usize, PATTERN_SIZE as usize),
                rb,
                0,
                (PATTERN_SIZE * 4) as usize,
                RB_PATTERN_BYTES,
            );
        }
        let t = ctx.targets;
        for (i, (x, y)) in sample_pixels(t.width, t.height).into_iter().enumerate() {
            let origin = MTLOrigin {
                x: x as usize,
                y: y as usize,
                z: 0,
            };
            unsafe {
                enc.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                    &t.depth, 0, 0, origin, size2(1, 1), rb, RB_DEPTH_OFFSET + i * 4, 4, 4,
                );
                enc.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                    &t.hdr, 0, 0, origin, size2(1, 1), rb, RB_HDR_OFFSET + i * 8, 8, 8,
                );
            }
        }
        if let Some((w, _)) = ctx.water {
            let ptable = ctx.slot.table(Table::WaterProbe);
            unsafe {
                ptable.setAddress_atIndex(rb.gpuAddress() + RB_WATER_OFFSET as u64, 0);
                ptable.setAddress_atIndex(ctx.upload_addr + SEA_OFFSET as u64, 2);
                ptable.setTexture_atIndex(w.field.gpuResourceID(), 0);
            }
            enc.setComputePipelineState(&r.pipelines.water_probe);
            enc.setArgumentTable(Some(ptable));
            enc.dispatchThreads_threadsPerThreadgroup(
                size2(WATER_PROBES, 1),
                size2(WATER_PROBES, 1),
            );
        }
        enc.endEncoding();
    }
}
