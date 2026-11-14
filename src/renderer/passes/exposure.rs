//! Проход `exposure` (04.1): замер яркости HDR-кадра и плавная адаптация
//! экспозиции; тонмаппинг читает результат. Без неба (тестовые кубы) проход
//! пуст — экспозиция фиксированная.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx, size2};
use crate::renderer::frame::{SKY_OFFSET, Table};
use crate::renderer::resources::label;

pub(in crate::renderer) struct ExposurePass;

/// Потоков в группе (`METER_THREADS` в `sky.metal`).
const METER_THREADS: usize = 256;

impl FramePass for ExposurePass {
    fn name(&self) -> &'static str {
        "exposure"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        if ctx.input.sky.is_none() {
            return;
        }
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("exposure meter")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Exposure);
        unsafe {
            table.setAddress_atIndex(ctx.upload_addr, 1);
            table.setAddress_atIndex(ctx.upload_addr + SKY_OFFSET as u64, 2);
            table.setAddress_atIndex(ctx.r.sky.state.gpuAddress(), 4);
            table.setTexture_atIndex(ctx.targets.hdr.gpuResourceID(), 0);
        }
        enc.setComputePipelineState(&ctx.r.pipelines.exposure_meter);
        enc.setArgumentTable(Some(table));
        enc.dispatchThreadgroups_threadsPerThreadgroup(size2(1, 1), size2(METER_THREADS, 1));
        enc.endEncoding();
    }
}
