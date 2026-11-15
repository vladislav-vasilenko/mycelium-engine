//! Проход `taa` (04.5, сессия 1): временное сглаживание HDR-кадра сцены до
//! рук. Ядро `taa_resolve` пишет историю этого кадра (чётность номера кадра),
//! затем она копируется обратно в HDR — дальше кадр идёт как прежде.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx, size2};
use crate::renderer::frame::{SHADOW_OFFSET, SKY_OFFSET, TAA_OFFSET, Table};
use crate::renderer::resources::label;

pub(in crate::renderer) struct TaaPass;

impl FramePass for TaaPass {
    fn name(&self) -> &'static str {
        "taa"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        let Some(taa) = ctx.input.taa.as_ref() else {
            return;
        };
        let t = ctx.targets;
        if let (true, Some(scaler)) = (t.scaled(), ctx.r.upscaler.as_ref()) {
            encode_upscale(ctx, barrier, scaler, taa);
            return;
        }
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
            table.setTexture_atIndex(t.vol.gpuResourceID(), 4);
            table.setTexture_atIndex(t.clouds[cur].gpuResourceID(), 6);
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

/// Апскейл MetalFX (04.6): подготовка (объёмный свет, векторы движения),
/// затем апскейлер пишет выходной HDR. Кодировщики MetalFX — свои: порядок
/// с соседями — барьерами очереди пустых кодировщиков до и после.
fn encode_upscale(
    ctx: &PassCtx,
    barrier: &PassBarrier,
    scaler: &objc2::runtime::ProtocolObject<dyn objc2_metal_fx::MTL4FXTemporalScaler>,
    taa: &crate::layout::TaaUniforms,
) {
    let t = ctx.targets;
    let enc = ctx
        .slot
        .cmd
        .computeCommandEncoder()
        .expect("compute encoder");
    enc.setLabel(Some(&label("upscale: prep (volumetric, motion)")));
    barrier.apply(&*enc);
    let table = ctx.slot.table(Table::Taa);
    unsafe {
        table.setAddress_atIndex(ctx.upload_addr + TAA_OFFSET as u64, 0);
        table.setTexture_atIndex(t.scene.gpuResourceID(), 0);
        table.setTexture_atIndex(t.scene_depth.gpuResourceID(), 1);
        table.setTexture_atIndex(t.motion.gpuResourceID(), 3);
        table.setTexture_atIndex(t.vol.gpuResourceID(), 4);
        table.setTexture_atIndex(t.wind.gpuResourceID(), 5);
        table.setTexture_atIndex(t.clouds[(ctx.frame_index % 2) as usize].gpuResourceID(), 6);
    }
    enc.setComputePipelineState(&ctx.r.pipelines.upscale_prep);
    enc.setArgumentTable(Some(table));
    enc.dispatchThreads_threadsPerThreadgroup(size2(t.rw as usize, t.rh as usize), size2(16, 16));
    // Работа MetalFX ждёт подготовку.
    enc.barrierAfterStages_beforeQueueStages_visibilityOptions(
        MTLStages::Dispatch,
        MTLStages::All,
        MTL4VisibilityOptions::Device,
    );
    enc.endEncoding();
    crate::renderer::upscale::encode(scaler, &ctx.slot.cmd, t, taa);
    // Следующие проходы ждут MetalFX.
    let fence = ctx
        .slot
        .cmd
        .computeCommandEncoder()
        .expect("compute encoder");
    fence.setLabel(Some(&label("upscale: barrier")));
    fence.barrierAfterQueueStages_beforeStages_visibilityOptions(
        MTLStages::All,
        MTLStages::All,
        MTL4VisibilityOptions::Device,
    );
    fence.barrierAfterStages_beforeQueueStages_visibilityOptions(
        MTLStages::All,
        MTLStages::All,
        MTL4VisibilityOptions::Device,
    );
    fence.endEncoding();
}

/// Проход `volumetric` (11, сессия 2): лучи солнца в дымке — четверть
/// разрешения по глубине и карте теней; при каждом TAA (без теней — нули).
pub(in crate::renderer) struct VolumetricPass;

impl FramePass for VolumetricPass {
    fn name(&self) -> &'static str {
        "volumetric"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        if ctx.input.taa.is_none() {
            return;
        }
        let t = ctx.targets;
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("volumetric light")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Volumetric);
        unsafe {
            table.setTexture_atIndex(t.scene_depth.gpuResourceID(), 0);
            table.setTexture_atIndex(ctx.r.shadows.map.gpuResourceID(), 1);
            table.setTexture_atIndex(t.vol.gpuResourceID(), 2);
            table.setAddress_atIndex(ctx.upload_addr, 1);
            table.setAddress_atIndex(ctx.upload_addr + TAA_OFFSET as u64, 2);
            table.setAddress_atIndex(ctx.upload_addr + SHADOW_OFFSET as u64, 3);
            table.setAddress_atIndex(ctx.r.sky.frame.gpuAddress(), 4);
        }
        enc.setComputePipelineState(&ctx.r.pipelines.volumetric_light);
        enc.setArgumentTable(Some(table));
        enc.dispatchThreads_threadsPerThreadgroup(
            size2(t.vol.width(), t.vol.height()),
            size2(16, 16),
        );
        // Объёмные облака (04.3, сессия 4б): по глубине сцены — закрывают
        // вершины в облачном слое; склейка — в `upscale_prep` / `taa_resolve`.
        let cur = (ctx.frame_index % 2) as usize;
        let ct = ctx.slot.table(Table::Clouds);
        unsafe {
            ct.setTexture_atIndex(t.clouds[cur].gpuResourceID(), 0);
            ct.setTexture_atIndex(t.clouds[1 - cur].gpuResourceID(), 1);
            ct.setTexture_atIndex(ctx.r.sky.transmittance.gpuResourceID(), 2);
            ct.setTexture_atIndex(t.scene_depth.gpuResourceID(), 3);
            ct.setAddress_atIndex(ctx.upload_addr, 1);
            ct.setAddress_atIndex(ctx.upload_addr + SKY_OFFSET as u64, 2);
            ct.setAddress_atIndex(ctx.r.sky.frame.gpuAddress(), 3);
            ct.setAddress_atIndex(ctx.upload_addr + TAA_OFFSET as u64, 4);
        }
        enc.setComputePipelineState(&ctx.r.pipelines.clouds_march);
        enc.setArgumentTable(Some(ct));
        enc.dispatchThreads_threadsPerThreadgroup(
            size2(t.clouds[cur].width(), t.clouds[cur].height()),
            size2(8, 8),
        );
        enc.endEncoding();
    }
}
