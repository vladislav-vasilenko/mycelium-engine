//! Проход `scene`: ландшафт, кубы (тестовая сцена или маркеры), затем
//! прозрачная вода. Цели — HDR и глубина reversed-Z.

use objc2_metal::*;

use super::{FramePass, PassBarrier, PassCtx};
use crate::renderer::cull::ARGS_OFFSET;
use crate::renderer::frame::{
    INSTANCE_OFFSET, LEVELS_OFFSET, SEA_OFFSET, SHADOW_OFFSET, SKY_OFFSET, Table,
};
use crate::renderer::resources::label;
use crate::renderer::terrain_clusters::{CLUSTER_VERTEX_COUNT, MAX_VISIBLE_CLUSTERS};
use crate::water::WATER_LEVELS;

pub(in crate::renderer) struct ScenePass;

use super::RenderEncoder;

const BOTH_STAGES: MTLRenderStages =
    MTLRenderStages(MTLRenderStages::Vertex.0 | MTLRenderStages::Fragment.0);

impl FramePass for ScenePass {
    fn name(&self) -> &'static str {
        "scene"
    }

    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier) {
        let enc = begin(ctx);
        barrier.apply(&*enc);
        let r = ctx.r;
        enc.setDepthStencilState(Some(&r.depth_state));
        enc.setFrontFacingWinding(MTLWinding::CounterClockwise);
        enc.setCullMode(MTLCullMode::Back);
        enc.setViewport(MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: ctx.targets.rw as f64,
            height: ctx.targets.rh as f64,
            znear: 0.0,
            zfar: 1.0,
        });
        if ctx.input.draw_terrain {
            draw_terrain(ctx, &enc);
            super::forest::draw_scene(ctx, &enc);
        }
        draw_cubes(ctx, &enc);
        draw_sky(ctx, &enc);
        enc.endEncoding();
        // Вода — своим кодировщиком без вложения ветра (04.6, сессия 2):
        // смешивание с двумя источниками несовместимо с несколькими целями.
        let enc = begin_water(ctx);
        enc.setFrontFacingWinding(MTLWinding::CounterClockwise);
        enc.setCullMode(MTLCullMode::Back);
        enc.setViewport(MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: ctx.targets.rw as f64,
            height: ctx.targets.rh as f64,
            znear: 0.0,
            zfar: 1.0,
        });
        draw_water(ctx, &enc);
        enc.endEncoding();
    }
}

/// Кодировщик прохода: очистка HDR цветом неба и глубины нулём (reversed-Z);
/// глубина сохраняется только для readback.
fn begin(ctx: &PassCtx) -> objc2::rc::Retained<RenderEncoder> {
    let rp = MTL4RenderPassDescriptor::new();
    let color = unsafe { rp.colorAttachments().objectAtIndexedSubscript(0) };
    color.setTexture(Some(&ctx.targets.scene));
    color.setLoadAction(MTLLoadAction::Clear);
    color.setStoreAction(MTLStoreAction::Store);
    let c = ctx.input.clear_color;
    color.setClearColor(MTLClearColor {
        red: c[0] as f64,
        green: c[1] as f64,
        blue: c[2] as f64,
        alpha: 1.0,
    });
    // Сдвиг от ветра (04.6, сессия 2): растения пишут, остальное — ноль.
    let wind = unsafe { rp.colorAttachments().objectAtIndexedSubscript(1) };
    wind.setTexture(Some(&ctx.targets.wind));
    wind.setLoadAction(MTLLoadAction::Clear);
    wind.setStoreAction(MTLStoreAction::Store);
    wind.setClearColor(MTLClearColor {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        alpha: 0.0,
    });
    let depth = rp.depthAttachment();
    depth.setTexture(Some(&ctx.targets.scene_depth));
    depth.setLoadAction(MTLLoadAction::Clear);
    depth.setClearDepth(0.0);
    // Глубину читают кодировщик воды, readback и TAA.
    depth.setStoreAction(MTLStoreAction::Store);
    let enc = ctx
        .slot
        .cmd
        .renderCommandEncoderWithDescriptor(&rp)
        .expect("render encoder");
    enc.setLabel(Some(&label("scene raster")));
    enc
}

/// Кодировщик воды: цвет и глубина сцены загружаются; ждёт фрагменты
/// предыдущего кодировщика прохода.
fn begin_water(ctx: &PassCtx) -> objc2::rc::Retained<RenderEncoder> {
    let rp = MTL4RenderPassDescriptor::new();
    let color = unsafe { rp.colorAttachments().objectAtIndexedSubscript(0) };
    color.setTexture(Some(&ctx.targets.scene));
    color.setLoadAction(MTLLoadAction::Load);
    color.setStoreAction(MTLStoreAction::Store);
    let depth = rp.depthAttachment();
    depth.setTexture(Some(&ctx.targets.scene_depth));
    depth.setLoadAction(MTLLoadAction::Load);
    depth.setStoreAction(if ctx.readback || ctx.input.taa.is_some() {
        MTLStoreAction::Store
    } else {
        MTLStoreAction::DontCare
    });
    let enc = ctx
        .slot
        .cmd
        .renderCommandEncoderWithDescriptor(&rp)
        .expect("render encoder");
    enc.setLabel(Some(&label("scene water")));
    enc.barrierAfterQueueStages_beforeStages_visibilityOptions(
        MTLStages::Fragment,
        MTLStages::Fragment,
        MTL4VisibilityOptions::Device,
    );
    enc
}

/// Рельеф: кластеры среза (02.3) или все тайлы уровня 0 одним instanced draw.
fn draw_terrain(ctx: &PassCtx, enc: &RenderEncoder) {
    let Some(terrain) = &ctx.r.terrain else {
        return;
    };
    if let Some(c) = &ctx.r.clusters {
        if ctx.input.cull.is_some() {
            // Участок детализации — первым (02.10, сессия 4): кластеры под ним
            // утоплены и отбрасываются ранним тестом глубины — дорогой
            // `terrain_fs` не затеняет пиксели у ног дважды (−2,5 мс).
            if ctx.input.forest {
                bind_clusters(ctx, enc, terrain, c);
                draw_patch(ctx, enc);
            }
            draw_clusters(ctx, enc, terrain, c, None);
            return;
        }
        if let Some(list) = ctx.input.clusters {
            draw_clusters(
                ctx,
                enc,
                terrain,
                c,
                Some(list.len().min(MAX_VISIBLE_CLUSTERS)),
            );
            return;
        }
    }
    let table = ctx.slot.table(Table::Terrain);
    unsafe {
        table.setAddress_atIndex(terrain.vertices.gpuAddress(), 0);
        table.setAddress_atIndex(ctx.upload_addr, 1);
        table.setTexture_atIndex(terrain.detail.gpuResourceID(), 0);
        table.setTexture_atIndex(terrain.albedo.gpuResourceID(), 1);
        table.setTexture_atIndex(terrain.normal.gpuResourceID(), 2);
        table.setTexture_atIndex(terrain.wetness.gpuResourceID(), 3);
        table.setAddress_atIndex(ctx.r.sky.frame.gpuAddress(), 9);
        table.setAddress_atIndex(ctx.upload_addr + SHADOW_OFFSET as u64, 12);
        table.setTexture_atIndex(ctx.r.shadows.map.gpuResourceID(), 7);
        table.setTexture_atIndex(ctx.r.sky.aerial.gpuResourceID(), 8);
        table.setAddress_atIndex(terrain.feat_segments.gpuAddress(), 2);
        table.setAddress_atIndex(terrain.feat_cells.gpuAddress(), 3);
        table.setAddress_atIndex(terrain.feat_list.gpuAddress(), 4);
        // Крупная детализация (02.9) — из кластеров; без них — любая текстура
        // (тайлы без кластеров — только отладка).
        let macro_tex = ctx
            .r
            .clusters
            .as_ref()
            .map_or(&terrain.detail, |c| &c.macro_detail);
        table.setTexture_atIndex(macro_tex.gpuResourceID(), 9);
        // Затенение и пробы (11) не используются (params.z/y = 0), но
        // привязки шейдера должны быть заданы.
        table.setTexture_atIndex(ctx.targets.rt_ao.gpuResourceID(), 10);
        let probes = ctx.r.rt.as_ref().map_or(&terrain.vertices, |rt| &rt.probes);
        table.setAddress_atIndex(probes.gpuAddress(), 13);
    }
    enc.pushDebugGroup(&label(&format!("terrain: {} tiles", terrain.tile_count)));
    enc.setRenderPipelineState(&ctx.r.pipelines.terrain);
    enc.setArgumentTable_atStages(table, BOTH_STAGES);
    unsafe {
        enc.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferLength_instanceCount(
            MTLPrimitiveType::Triangle,
            terrain.index_count,
            MTLIndexType::UInt16,
            terrain.indices.gpuAddress(),
            terrain.indices.length(),
            terrain.tile_count,
        );
    }
    enc.popDebugGroup();
}

/// Экземпляр — видимый кластер; вершинный шейдер берёт его треугольники из
/// общего буфера и позиции узлов из поля высот. Лишние вершины (у кластера
/// меньше 128 треугольников) схлопываются в точку.
fn draw_clusters(
    ctx: &PassCtx,
    enc: &RenderEncoder,
    terrain: &crate::renderer::terrain::TerrainGpu,
    c: &crate::renderer::terrain_clusters::ClustersGpu,
    count: Option<usize>,
) {
    if count == Some(0) {
        return;
    }
    enc.pushDebugGroup(&label(&match count {
        Some(n) => format!("terrain: {n} clusters (CPU cut)"),
        None => "terrain: clusters (GPU cut, indirect)".to_string(),
    }));
    bind_clusters(ctx, enc, terrain, c);
    match count {
        // Срез на GPU (02.5): число экземпляров — из indirect-аргументов вида 0.
        None => enc.drawPrimitives_indirectBuffer(
            MTLPrimitiveType::Triangle,
            c.cull.counters[ctx.slot_index].gpuAddress() + ARGS_OFFSET as u64,
        ),
        Some(n) => unsafe {
            enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                MTLPrimitiveType::Triangle,
                0,
                CLUSTER_VERTEX_COUNT,
                n,
            );
        },
    }
    enc.popDebugGroup();
}

/// Привязки кластерного рельефа: таблица `Terrain` и конвейер кластеров
/// (участок детализации ставит свой конвейер поверх тех же привязок).
fn bind_clusters(
    ctx: &PassCtx,
    enc: &RenderEncoder,
    terrain: &crate::renderer::terrain::TerrainGpu,
    c: &crate::renderer::terrain_clusters::ClustersGpu,
) {
    let table = ctx.slot.table(Table::Terrain);
    unsafe {
        table.setAddress_atIndex(c.heights.gpuAddress(), 0);
        table.setAddress_atIndex(ctx.upload_addr, 1);
        table.setAddress_atIndex(terrain.feat_segments.gpuAddress(), 2);
        table.setAddress_atIndex(terrain.feat_cells.gpuAddress(), 3);
        table.setAddress_atIndex(terrain.feat_list.gpuAddress(), 4);
        table.setAddress_atIndex(c.clusters.gpuAddress(), 5);
        table.setAddress_atIndex(c.vertices.gpuAddress(), 6);
        table.setAddress_atIndex(c.triangles.gpuAddress(), 7);
        table.setAddress_atIndex(ctx.slot.visible.gpuAddress(), 8);
        table.setAddress_atIndex(ctx.r.sky.frame.gpuAddress(), 9);
        table.setAddress_atIndex(ctx.upload_addr + SHADOW_OFFSET as u64, 12);
        table.setTexture_atIndex(ctx.r.shadows.map.gpuResourceID(), 7);
        table.setTexture_atIndex(ctx.r.sky.aerial.gpuResourceID(), 8);
        table.setAddress_atIndex(c.zone_heights.gpuAddress(), 10);
        table.setAddress_atIndex(c.zone_info.gpuAddress(), 11);
        table.setTexture_atIndex(terrain.detail.gpuResourceID(), 0);
        table.setTexture_atIndex(terrain.albedo.gpuResourceID(), 1);
        table.setTexture_atIndex(terrain.normal.gpuResourceID(), 2);
        table.setTexture_atIndex(terrain.wetness.gpuResourceID(), 3);
        table.setTexture_atIndex(c.weights.gpuResourceID(), 4);
        table.setTexture_atIndex(c.material.gpuResourceID(), 5);
        table.setTexture_atIndex(c.cover.gpuResourceID(), 6);
        table.setTexture_atIndex(c.macro_detail.gpuResourceID(), 9);
        // Затенение лучами прошлого кадра (11): шейдер читает его при params.z = 1.
        table.setTexture_atIndex(ctx.targets.rt_ao.gpuResourceID(), 10);
        // Пробы освещённости (11, сессия 3); без сцены лучей — любой буфер
        // (шейдер его не читает при params.y = 0).
        let probes = ctx.r.rt.as_ref().map_or(&c.heights, |rt| &rt.probes);
        table.setAddress_atIndex(probes.gpuAddress(), 13);
    }
    enc.setRenderPipelineState(&ctx.r.pipelines.terrain_clusters);
    enc.setArgumentTable_atStages(table, BOTH_STAGES);
}

/// Вершин детальной заплатки у камеры: 256² ячеек × 2 треугольника
/// (`PATCH_VERTEX_COUNT` в `terrain_patch.h`).
const PATCH_VERTEX_COUNT: usize = 256 * 256 * 6;

/// Детальная заплатка рельефа у камеры (02.10, сессия 2) — та же таблица
/// `terrain`, что у кластеров (привязки — `bind_clusters`), свой вершинный шейдер.
/// Вне игрового режима или вне зоны шейдер схлопывает все вершины.
fn draw_patch(ctx: &PassCtx, enc: &RenderEncoder) {
    enc.pushDebugGroup(&label("terrain: detail patch"));
    enc.setRenderPipelineState(&ctx.r.pipelines.terrain_patch);
    unsafe {
        enc.drawPrimitives_vertexStart_vertexCount(
            MTLPrimitiveType::Triangle,
            0,
            PATCH_VERTEX_COUNT,
        );
    }
    enc.popDebugGroup();
}

/// Кубы: тестовая сцена этапа 01 или столбики маркеров.
fn draw_cubes(ctx: &PassCtx, enc: &RenderEncoder) {
    let r = ctx.r;
    let table = ctx.slot.table(Table::Scene);
    unsafe {
        table.setAddress_atIndex(r.vertex_buf.gpuAddress(), 0);
        table.setAddress_atIndex(ctx.upload_addr, 1);
        table.setAddress_atIndex(ctx.upload_addr + INSTANCE_OFFSET as u64, 2);
        table.setTexture_atIndex(r.pattern_tex.gpuResourceID(), 0);
    }
    enc.setRenderPipelineState(&r.pipelines.scene);
    enc.setArgumentTable_atStages(table, BOTH_STAGES);
    if ctx.instance_count > 0 {
        unsafe {
            enc.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferLength_instanceCount(
                MTLPrimitiveType::Triangle,
                r.index_count,
                MTLIndexType::UInt16,
                r.index_buf.gpuAddress(),
                r.index_buf.length(),
                ctx.instance_count,
            );
        }
    }
}

/// Небо (04.1) за непрозрачной геометрией: полноэкранный треугольник на глубине
/// 0 (reversed-Z — бесконечность) проходит тест только там, где пусто.
fn draw_sky(ctx: &PassCtx, enc: &RenderEncoder) {
    if ctx.input.sky.is_none() {
        return;
    }
    let sky = &ctx.r.sky;
    let table = ctx.slot.table(Table::SkyDraw);
    unsafe {
        table.setAddress_atIndex(ctx.upload_addr, 1);
        table.setAddress_atIndex(ctx.upload_addr + SKY_OFFSET as u64, 2);
        table.setAddress_atIndex(sky.frame.gpuAddress(), 3);
        table.setTexture_atIndex(sky.skyview.gpuResourceID(), 0);
        table.setTexture_atIndex(sky.transmittance.gpuResourceID(), 1);
        table.setTexture_atIndex(sky.skyview_moon.gpuResourceID(), 2);
    }
    enc.pushDebugGroup(&label("sky"));
    enc.setRenderPipelineState(&ctx.r.pipelines.sky);
    enc.setDepthStencilState(Some(&ctx.r.water_depth_state));
    enc.setCullMode(MTLCullMode::None);
    enc.setArgumentTable_atStages(table, BOTH_STAGES);
    unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) };
    if let Some((stars, n)) = &sky.stars {
        unsafe { table.setAddress_atIndex(stars.gpuAddress(), 0) };
        enc.setRenderPipelineState(&ctx.r.pipelines.stars);
        enc.setArgumentTable_atStages(table, BOTH_STAGES);
        unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Point, 0, *n) };
    }
    enc.popDebugGroup();
    enc.setDepthStencilState(Some(&ctx.r.depth_state));
}

/// Прозрачная вода после непрозрачной геометрии: проверка глубины без записи.
fn draw_water(ctx: &PassCtx, enc: &RenderEncoder) {
    let Some((w, t)) = ctx.water else { return };
    let table = ctx.slot.table(Table::Water);
    unsafe {
        table.setAddress_atIndex(ctx.upload_addr, 1);
        table.setAddress_atIndex(ctx.upload_addr + SEA_OFFSET as u64, 2);
        table.setAddress_atIndex(ctx.upload_addr + LEVELS_OFFSET as u64, 3);
        table.setTexture_atIndex(w.field.gpuResourceID(), 0);
        table.setTexture_atIndex(t.detail.gpuResourceID(), 1);
        table.setAddress_atIndex(ctx.r.sky.frame.gpuAddress(), 4);
        table.setAddress_atIndex(ctx.upload_addr + SKY_OFFSET as u64, 5);
        table.setTexture_atIndex(ctx.r.sky.skyview.gpuResourceID(), 2);
        table.setTexture_atIndex(ctx.r.sky.aerial.gpuResourceID(), 3);
    }
    enc.pushDebugGroup(&label(&format!("water: {WATER_LEVELS} levels")));
    enc.setRenderPipelineState(&ctx.r.pipelines.water);
    enc.setDepthStencilState(Some(&ctx.r.water_depth_state));
    enc.setCullMode(MTLCullMode::None);
    enc.setArgumentTable_atStages(table, BOTH_STAGES);
    unsafe {
        enc.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferLength_instanceCount(
            MTLPrimitiveType::Triangle,
            w.index_count,
            MTLIndexType::UInt16,
            w.indices.gpuAddress(),
            w.indices.length(),
            WATER_LEVELS,
        );
    }
    enc.popDebugGroup();
}
