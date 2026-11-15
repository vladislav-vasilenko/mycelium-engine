//! Интерфейс на GPU (05.1): атлас шрифта и буферы вершин слотов.

use objc2_metal::*;

use super::frame::FRAME_SLOTS;
use super::resources::{as_allocation, shared_buffer};
use super::shaders::Obj;
use crate::layout::UiVertex;

/// Вершин интерфейса в кадре не больше.
pub const UI_MAX_VERTS: usize = 1 << 16;
/// В начале буфера слота — размер экрана (float2), вершины — с 16-го байта.
pub const UI_VERTS_OFFSET: usize = 16;

pub(super) struct UiGpu {
    pub font: Obj<dyn MTLTexture>,
    pub buffers: Vec<Obj<dyn MTLBuffer>>,
}

impl super::Renderer {
    pub fn set_ui_font(&mut self, pixels: &[u8], size: u32) -> Result<(), String> {
        let font = super::resources::sampled_texture(
            &self.device,
            MTLPixelFormat::R8Unorm,
            size,
            1,
            false,
            "ui: font SDF atlas",
        )?;
        self.upload_textures(&[super::resources::TextureUpload {
            texture: &font,
            data: pixels,
            pixel_bytes: 1,
            mips: false,
        }])?;
        let buffers: Vec<_> = (0..FRAME_SLOTS)
            .map(|s| {
                shared_buffer(
                    &self.device,
                    UI_VERTS_OFFSET + UI_MAX_VERTS * std::mem::size_of::<UiVertex>(),
                    &format!("ui vertices, slot {s}"),
                )
            })
            .collect();
        self.residency.addAllocation(as_allocation(&*font));
        for b in &buffers {
            self.residency.addAllocation(as_allocation(&**b));
        }
        self.residency.commit();
        if let Some(old) = self.ui.replace(UiGpu { font, buffers }) {
            self.retire(vec![old.font], Some(Box::new(old.buffers)));
        }
        Ok(())
    }
}
