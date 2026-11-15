//! Проход `forest` (02.4): расстановка и отсечение растений на GPU; функции
//! рисования леса в проходах `scene` и `shadows` по indirect-аргументам.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx, RenderEncoder, size2};
use crate::layout::PlantInstance;
use crate::renderer::forest::{
    ARGS_OFFSET, CAMERA_LISTS, FOREST_CAP, FOREST_GRID, FOREST_LISTS, PROPS_LIST,
};
use crate::renderer::frame::{CULL_OFFSET, SHADOW_OFFSET, Table};
use crate::renderer::resources::label;
use crate::renderer::resources::upload_bytes;

pub(in crate::renderer) struct ForestPass;

/// Лес в кадре: загружен, есть кластерный рельеф и выбор на GPU (камера вида 0).
pub(in crate::renderer) fn active(ctx: &PassCtx) -> bool {
    ctx.input.forest
        && ctx.input.cull.is_some()
        && ctx.r.forest.is_some()
        && ctx.r.clusters.is_some()
}

impl FramePass for ForestPass {
    fn name(&self) -> &'static str {
        "forest"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        if !active(ctx) {
            return;
        }
        let (f, c, t) = (
            ctx.r.forest.as_ref().unwrap(),
            ctx.r.clusters.as_ref().unwrap(),
            ctx.r.terrain.as_ref().expect("ландшафт"),
        );
        let s = ctx.slot_index;
        // Грибы с CPU (05.1): экземпляры и их число — до запуска ядра (слот свободен).
        let n = ctx.input.props.len().min(FOREST_CAP);
        let isz = std::mem::size_of::<PlantInstance>();
        upload_bytes(
            &f.instances[s],
            PROPS_LIST * FOREST_CAP * isz,
            bytemuck::cast_slice(&ctx.input.props[..n]),
        );
        upload_bytes(
            &f.counters[s],
            PROPS_LIST * 4,
            bytemuck::bytes_of(&(n as u32)),
        );
        let enc = ctx
            .slot
            .cmd
            .computeCommandEncoder()
            .expect("compute encoder");
        enc.setLabel(Some(&label("forest: placement and culling")));
        barrier.apply(&*enc);
        let table = ctx.slot.table(Table::Forest);
        unsafe {
            table.setAddress_atIndex(c.heights.gpuAddress(), 0);
            table.setAddress_atIndex(ctx.upload_addr, 1);
            table.setAddress_atIndex(t.feat_segments.gpuAddress(), 2);
            table.setAddress_atIndex(t.feat_cells.gpuAddress(), 3);
            table.setAddress_atIndex(t.feat_list.gpuAddress(), 4);
            table.setAddress_atIndex(c.zone_heights.gpuAddress(), 5);
            table.setAddress_atIndex(c.zone_info.gpuAddress(), 6);
            table.setAddress_atIndex(ctx.upload_addr + CULL_OFFSET as u64, 7);
            table.setAddress_atIndex(f.instances[s].gpuAddress(), 8);
            table.setAddress_atIndex(f.counters[s].gpuAddress(), 9);
            table.setAddress_atIndex(f.lods.gpuAddress(), 10);
            table.setTexture_atIndex(c.cover.gpuResourceID(), 0);
            table.setTexture_atIndex(f.vegetation.gpuResourceID(), 1);
            table.setTexture_atIndex(c.macro_detail.gpuResourceID(), 2);
            table.setAddress_atIndex(f.vegetation_table.gpuAddress(), 11);
        }
        enc.setComputePipelineState(&ctx.r.pipelines.forest_place);
        enc.setArgumentTable(Some(table));
        enc.dispatchThreads_threadsPerThreadgroup(size2(FOREST_GRID, FOREST_GRID), size2(16, 16));
        enc.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
            MTLStages::Dispatch,
            MTLStages::Dispatch,
            MTL4VisibilityOptions::Device,
        );
        enc.setComputePipelineState(&ctx.r.pipelines.forest_finish);
        enc.dispatchThreads_threadsPerThreadgroup(size2(FOREST_LISTS, 1), size2(32, 1));
        // Вода на земле (04.3, сессия 6): 4 подшага A → B → A → B → A.
        enc.setComputePipelineState(&ctx.r.pipelines.ground_water);
        for k in 0..4 {
            enc.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
                MTLStages::Dispatch,
                MTLStages::Dispatch,
                MTL4VisibilityOptions::Device,
            );
            unsafe {
                table.setTexture_atIndex(f.water[k % 2].gpuResourceID(), 3);
                table.setTexture_atIndex(f.water[1 - k % 2].gpuResourceID(), 4);
            }
            enc.setArgumentTable(Some(table));
            let n = crate::renderer::forest::WATER_N as usize;
            enc.dispatchThreads_threadsPerThreadgroup(size2(n, n), size2(16, 16));
        }
        enc.endEncoding();
    }
}

/// Поле высот для драпировки растений по рельефу (02.10): буферы 6–8.
fn bind_ground(ctx: &PassCtx, table: &objc2::runtime::ProtocolObject<dyn MTL4ArgumentTable>) {
    let c = ctx.r.clusters.as_ref().unwrap();
    unsafe {
        table.setAddress_atIndex(c.heights.gpuAddress(), 6);
        table.setAddress_atIndex(c.zone_heights.gpuAddress(), 7);
        table.setAddress_atIndex(c.zone_info.gpuAddress(), 8);
    }
}

fn list_addr(ctx: &PassCtx, list: usize) -> u64 {
    let f = ctx.r.forest.as_ref().unwrap();
    f.instances[ctx.slot_index].gpuAddress()
        + (list * FOREST_CAP * std::mem::size_of::<PlantInstance>()) as u64
}

fn draw_list(
    ctx: &PassCtx,
    enc: &RenderEncoder,
    table: &objc2::runtime::ProtocolObject<dyn MTL4ArgumentTable>,
    list: usize,
) {
    let f = ctx.r.forest.as_ref().unwrap();
    unsafe { table.setAddress_atIndex(list_addr(ctx, list), 2) };
    enc.setArgumentTable_atStages(table, MTLRenderStages::Vertex | MTLRenderStages::Fragment);
    unsafe {
        enc.drawIndexedPrimitives_indexType_indexBuffer_indexBufferLength_indirectBuffer(
            MTLPrimitiveType::Triangle,
            MTLIndexType::UInt32,
            f.indices.gpuAddress(),
            f.indices.length(),
            f.counters[ctx.slot_index].gpuAddress() + (ARGS_OFFSET + list * 20) as u64,
        );
    }
}

/// Растения в проходе `scene` (после рельефа, до воды).
pub(in crate::renderer) fn draw_scene(ctx: &PassCtx, enc: &RenderEncoder) {
    if !active(ctx) {
        return;
    }
    let f = ctx.r.forest.as_ref().unwrap();
    let t = ctx.r.terrain.as_ref().unwrap();
    let table = ctx.slot.table(Table::Plant);
    unsafe {
        table.setAddress_atIndex(f.vertices.gpuAddress(), 0);
        table.setAddress_atIndex(ctx.upload_addr, 1);
        table.setAddress_atIndex(ctx.r.sky.frame.gpuAddress(), 3);
        table.setAddress_atIndex(ctx.upload_addr + SHADOW_OFFSET as u64, 4);
        table.setTexture_atIndex(f.albedo.gpuResourceID(), 0);
        table.setTexture_atIndex(ctx.r.shadows.map.gpuResourceID(), 1);
        table.setTexture_atIndex(ctx.r.sky.aerial.gpuResourceID(), 2);
        table.setTexture_atIndex(t.albedo.gpuResourceID(), 3);
        table.setTexture_atIndex(f.bark_normal.gpuResourceID(), 4);
        table.setTexture_atIndex(f.scan_albedo.gpuResourceID(), 6);
        table.setTexture_atIndex(f.scan_normal.gpuResourceID(), 7);
        table.setTexture_atIndex(ctx.targets.rt_ao.gpuResourceID(), 5);
        let probes = ctx.r.rt.as_ref().map_or(&f.vertices, |rt| &rt.probes);
        table.setAddress_atIndex(probes.gpuAddress(), 9);
    }
    bind_ground(ctx, table);
    // Предпроход глубины (02.13): отсечение по альфе пишет глубину листвы,
    // затенение — без `discard`, на равенство глубины: GPU Apple удаляет
    // скрытые слои карточек до фрагментного шейдера (было +8 мс на пихте).
    enc.pushDebugGroup(&label("forest: plants depth prepass (indirect)"));
    enc.setRenderPipelineState(&ctx.r.pipelines.plant_depth);
    enc.setCullMode(MTLCullMode::None);
    for list in (0..CAMERA_LISTS).chain([PROPS_LIST]) {
        draw_list(ctx, enc, table, list);
    }
    enc.popDebugGroup();
    enc.pushDebugGroup(&label("forest: plants (indirect)"));
    enc.setRenderPipelineState(&ctx.r.pipelines.plant_prepassed);
    enc.setDepthStencilState(Some(&ctx.r.depth_equal_state));
    for list in (0..CAMERA_LISTS).chain([PROPS_LIST]) {
        draw_list(ctx, enc, table, list);
    }
    enc.setDepthStencilState(Some(&ctx.r.depth_state));
    enc.popDebugGroup();
}

/// Тени деревьев в каскаде `k` (только ближние каскады 0 и 1).
pub(in crate::renderer) fn draw_shadow(ctx: &PassCtx, enc: &RenderEncoder, k: usize) {
    if !active(ctx) || k > 1 {
        return;
    }
    draw_occluders(ctx, enc, ctx.upload_addr + (SHADOW_OFFSET + k * 64) as u64);
}

/// Кроны в карту укрытия от дождя (04.3): те же списки теней деревьев,
/// матрица — `ShadowUniforms.rain_vp_rel` (смещение 304 байта).
pub(in crate::renderer) fn draw_rain_occluders(ctx: &PassCtx, enc: &RenderEncoder) {
    if !active(ctx) {
        return;
    }
    draw_occluders(ctx, enc, ctx.upload_addr + (SHADOW_OFFSET + 304) as u64);
}

fn draw_occluders(ctx: &PassCtx, enc: &RenderEncoder, matrix_addr: u64) {
    let f = ctx.r.forest.as_ref().unwrap();
    let table = ctx.slot.table(Table::PlantShadow);
    unsafe {
        table.setAddress_atIndex(f.vertices.gpuAddress(), 0);
        table.setAddress_atIndex(ctx.upload_addr, 1);
        table.setAddress_atIndex(matrix_addr, 5);
        table.setTexture_atIndex(f.albedo.gpuResourceID(), 0);
        table.setTexture_atIndex(f.scan_albedo.gpuResourceID(), 2);
    }
    bind_ground(ctx, table);
    enc.setRenderPipelineState(&ctx.r.pipelines.plant_shadow);
    for list in CAMERA_LISTS..PROPS_LIST {
        draw_list(ctx, enc, table, list);
    }
}
