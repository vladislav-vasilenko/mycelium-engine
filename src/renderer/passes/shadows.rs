//! Проход `shadows` (04.2): кластеры рельефа в каскады карты теней. Каждый
//! каскад — свой кодировщик с очисткой слоя массива; список кластеров каскада
//! — диапазон общего буфера видимых кластеров слота.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx};
use crate::renderer::cull::{ARGS_OFFSET, region_base};
use crate::renderer::frame::{SHADOW_OFFSET, Table};
use crate::renderer::resources::label;
use crate::renderer::shadows::{CASCADES, SHADOW_SIZE};
use crate::renderer::terrain_clusters::{CLUSTER_VERTEX_COUNT, MAX_VISIBLE_CLUSTERS};

pub(in crate::renderer) struct ShadowsPass;

impl FramePass for ShadowsPass {
    fn name(&self) -> &'static str {
        "shadows"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        let (Some(sf), Some(c)) = (&ctx.input.shadows, &ctx.r.clusters) else {
            return;
        };
        let sh = &ctx.r.shadows;
        // Списки каскадов — после основного списка в буфере слота.
        let base = ctx
            .input
            .clusters
            .map_or(0, |l| l.len().min(MAX_VISIBLE_CLUSTERS)) as u64;
        // Выбор на GPU (02.5): список каскада — своя область, число — в indirect.
        let gpu = ctx.input.cull.is_some();
        let counters = c.cull.counters[ctx.slot_index].gpuAddress();
        for (k, &(start, count)) in sf.ranges.iter().enumerate().take(CASCADES) {
            let rp = MTL4RenderPassDescriptor::new();
            let depth = rp.depthAttachment();
            depth.setTexture(Some(&sh.map));
            depth.setSlice(k);
            depth.setLoadAction(MTLLoadAction::Clear);
            depth.setClearDepth(1.0);
            depth.setStoreAction(MTLStoreAction::Store);
            rp.setRenderTargetWidth(SHADOW_SIZE as usize);
            rp.setRenderTargetHeight(SHADOW_SIZE as usize);
            let enc = ctx
                .slot
                .cmd
                .renderCommandEncoderWithDescriptor(&rp)
                .expect("render encoder");
            enc.setLabel(Some(&label(&format!(
                "shadow cascade {k}: {count} clusters"
            ))));
            if k == 0 {
                barrier.apply(&*enc);
            }
            if count == 0 && !gpu {
                enc.endEncoding();
                continue;
            }
            let table = ctx.slot.table(Table::Shadow);
            unsafe {
                table.setAddress_atIndex(c.heights.gpuAddress(), 0);
                table.setAddress_atIndex(ctx.upload_addr, 1);
                table.setAddress_atIndex(c.clusters.gpuAddress(), 5);
                table.setAddress_atIndex(c.vertices.gpuAddress(), 6);
                table.setAddress_atIndex(c.triangles.gpuAddress(), 7);
                let first = if gpu {
                    region_base(k + 1) as u64
                } else {
                    base + start as u64
                };
                table.setAddress_atIndex(ctx.slot.visible.gpuAddress() + first * 4, 8);
                table.setAddress_atIndex(c.zone_heights.gpuAddress(), 10);
                table.setAddress_atIndex(c.zone_info.gpuAddress(), 11);
                table.setAddress_atIndex(ctx.upload_addr + (SHADOW_OFFSET + k * 64) as u64, 12);
            }
            enc.setRenderPipelineState(&ctx.r.pipelines.terrain_shadow);
            enc.setDepthStencilState(Some(&sh.depth_state));
            enc.setCullMode(MTLCullMode::None);
            // Смещение глубины по наклону: склоны под скользящим светом без «прыщей».
            enc.setDepthBias_slopeScale_clamp(0.0, 1.5, 0.01);
            enc.setArgumentTable_atStages(table, MTLRenderStages::Vertex);
            if gpu {
                enc.drawPrimitives_indirectBuffer(
                    MTLPrimitiveType::Triangle,
                    counters + (ARGS_OFFSET + (k + 1) * 16) as u64,
                );
            } else {
                unsafe {
                    enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        CLUSTER_VERTEX_COUNT,
                        count as usize,
                    );
                }
            }
            enc.endEncoding();
        }
    }
}
