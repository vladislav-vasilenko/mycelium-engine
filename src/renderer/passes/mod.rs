//! Проходы кадра как объекты (паттерн «Стратегия»).
//!
//! Планировщик `frame_graph` выдаёт порядок проходов и барьеры; фасад
//! `Renderer::render` находит объект прохода по имени в реестре и вызывает
//! `encode`. Новый проход — новый тип с `FramePass` и строка в `REGISTRY`
//! плюс объявление в `frame::frame_passes`.

mod pattern;
mod readback;
mod scene;
mod tonemap;
mod wet_sand;

use objc2::runtime::ProtocolObject;
use objc2_metal::*;

use super::Renderer;
use super::frame::{FrameInput, FrameSlot};
use super::targets::Targets;
use super::terrain::TerrainGpu;
use super::water::WaterGpu;

/// Барьер перед проходом: стадии очереди, которых ждать, и стадии, которые ждут.
pub(super) struct PassBarrier {
    pub after: MTLStages,
    pub before: MTLStages,
    /// Есть ли зависимость (план может не требовать барьера).
    pub needed: bool,
}

impl PassBarrier {
    /// Consumer barrier в начале кодировщика, если план его требует.
    pub fn apply<E: MTL4CommandEncoder + objc2::Message>(&self, enc: &E) {
        if self.needed {
            self.apply_always(enc);
        }
    }

    pub fn apply_always<E: MTL4CommandEncoder + objc2::Message>(&self, enc: &E) {
        enc.barrierAfterQueueStages_beforeStages_visibilityOptions(
            self.after,
            self.before,
            MTL4VisibilityOptions::Device,
        );
    }
}

/// Всё, что нужно проходу для кодирования одного кадра.
pub(super) struct PassCtx<'a> {
    /// Ресурсы и pipeline рендерера (только чтение).
    pub r: &'a Renderer,
    pub slot: &'a FrameSlot,
    pub upload_addr: u64,
    pub targets: &'a Targets,
    pub drawable: &'a ProtocolObject<dyn MTLTexture>,
    pub input: &'a FrameInput<'a>,
    /// Сколько экземпляров кубов записано в upload.
    pub instance_count: usize,
    /// Кадр с readback: глубина сохраняется после прохода `scene`.
    pub readback: bool,
    /// Вода и ландшафт, если вода есть в кадре и загружена.
    pub water: Option<(&'a WaterGpu, &'a TerrainGpu)>,
}

/// Проход кадра: кодирует свои команды в командный буфер слота.
pub(super) trait FramePass: Sync {
    fn name(&self) -> &'static str;
    fn encode(&self, ctx: &PassCtx, barrier: &PassBarrier);
}

static REGISTRY: [&dyn FramePass; 5] = [
    &pattern::PatternPass,
    &wet_sand::WetSandPass,
    &scene::ScenePass,
    &readback::ReadbackPass,
    &tonemap::TonemapPass,
];

/// Проход по имени из плана.
pub(super) fn find(name: &str) -> &'static dyn FramePass {
    REGISTRY
        .iter()
        .copied()
        .find(|p| p.name() == name)
        .unwrap_or_else(|| unreachable!("проход {name} без кодировщика"))
}

/// Размер диспетчеризации 2D.
pub(super) fn size2(width: usize, height: usize) -> MTLSize {
    MTLSize {
        width,
        height,
        depth: 1,
    }
}
