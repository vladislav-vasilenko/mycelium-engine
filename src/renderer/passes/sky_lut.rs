//! Проход `sky_lut` (04.1): LUT атмосферы, свет неба и экспозиция в одном
//! compute-кодировщике. Пропускание и многократное рассеяние считаются один
//! раз, вид неба и свет — каждый кадр. Между зависимыми dispatch — барьеры
//! внутри кодировщика.

use std::sync::atomic::Ordering;

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx, size2};
use crate::renderer::frame::{SKY_OFFSET, Table};
use crate::renderer::resources::label;
use crate::renderer::sky::{AMBIENT_THREADS, MULTISCATTER_SIZE, SKYVIEW_SIZE, TRANSMITTANCE_SIZE};

pub(in crate::renderer) struct SkyLutPass;

fn between_dispatches<E: MTL4CommandEncoder + objc2::Message>(enc: &E) {
    enc.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
        MTLStages::Dispatch,
        MTLStages::Dispatch,
        MTL4VisibilityOptions::Device,
    );
}

impl FramePass for SkyLutPass {
    fn name(&self) -> &'static str {
        "sky_lut"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        if ctx.input.sky.is_none() {
            return;
        }
        let sky = &ctx.r.sky;
        let p = &ctx.r.pipelines;
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("sky LUTs and ambient")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Sky);
        let size = |(w, h): (u32, u32)| size2(w as usize, h as usize);
        unsafe {
            table.setAddress_atIndex(ctx.upload_addr, 1);
            table.setAddress_atIndex(ctx.upload_addr + SKY_OFFSET as u64, 2);
            table.setAddress_atIndex(sky.frame.gpuAddress(), 3);
        }
        if !sky.static_ready.swap(true, Ordering::Relaxed) {
            unsafe { table.setTexture_atIndex(sky.transmittance.gpuResourceID(), 0) };
            enc.setComputePipelineState(&p.sky_transmittance);
            enc.setArgumentTable(Some(table));
            enc.dispatchThreads_threadsPerThreadgroup(size(TRANSMITTANCE_SIZE), size2(16, 8));
            between_dispatches(&*enc);
            unsafe {
                table.setTexture_atIndex(sky.multiscatter.gpuResourceID(), 0);
                table.setTexture_atIndex(sky.transmittance.gpuResourceID(), 1);
            }
            enc.setComputePipelineState(&p.sky_multiscatter);
            enc.setArgumentTable(Some(table));
            enc.dispatchThreads_threadsPerThreadgroup(size(MULTISCATTER_SIZE), size2(8, 8));
            between_dispatches(&*enc);
        }
        unsafe {
            table.setTexture_atIndex(sky.skyview.gpuResourceID(), 0);
            table.setTexture_atIndex(sky.transmittance.gpuResourceID(), 1);
            table.setTexture_atIndex(sky.multiscatter.gpuResourceID(), 2);
        }
        enc.setComputePipelineState(&p.sky_view);
        enc.setArgumentTable(Some(table));
        enc.dispatchThreads_threadsPerThreadgroup(size(SKYVIEW_SIZE), size2(16, 8));
        unsafe { table.setTexture_atIndex(sky.skyview_moon.gpuResourceID(), 0) };
        enc.setComputePipelineState(&p.sky_view_moon);
        enc.setArgumentTable(Some(table));
        enc.dispatchThreads_threadsPerThreadgroup(size(SKYVIEW_SIZE), size2(16, 8));
        between_dispatches(&*enc);
        unsafe {
            table.setTexture_atIndex(sky.skyview.gpuResourceID(), 0);
            table.setTexture_atIndex(sky.transmittance.gpuResourceID(), 1);
            table.setTexture_atIndex(sky.skyview_moon.gpuResourceID(), 2);
        }
        enc.setComputePipelineState(&p.sky_ambient);
        enc.setArgumentTable(Some(table));
        enc.dispatchThreadgroups_threadsPerThreadgroup(size2(1, 1), size2(AMBIENT_THREADS, 1));
        enc.endEncoding();
    }
}
