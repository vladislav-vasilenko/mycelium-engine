//! Вода на GPU (03.1): поле берега и индексы уровня кольцевой сетки.

use objc2_metal::*;

use super::Renderer;
use super::resources::{TextureUpload, as_allocation, buffer_with, sampled_texture};
use super::shaders::Obj;
use crate::water::ShoreField;

/// Вода на GPU: поле берега и индексы одного уровня кольцевой сетки.
pub(super) struct WaterGpu {
    /// RGBA32Float: высота дна, расстояние до кромки, тип берега.
    pub field: Obj<dyn MTLTexture>,
    pub indices: Obj<dyn MTLBuffer>,
    pub index_count: usize,
}

impl Renderer {
    /// Загружает воду: поле берега (RGBA32Float) и индексы уровня сетки.
    pub fn set_water(&mut self, field: &ShoreField) -> Result<(), String> {
        let tex = sampled_texture(
            &self.device,
            MTLPixelFormat::RGBA32Float,
            field.size as u32,
            1,
            false,
            "shore field (bed, coast distance, rocky)",
        )?;
        self.upload_textures(&[TextureUpload {
            texture: &tex,
            data: bytemuck::cast_slice(&field.data),
            pixel_bytes: 16,
            mips: false,
        }])?;
        let indices = crate::water::grid_indices();
        let ibuf = buffer_with(
            &self.device,
            bytemuck::cast_slice(&indices),
            "water grid indices",
        );
        self.residency.addAllocation(as_allocation(&*ibuf));
        self.residency.commit();
        if let Some(old) = self.water.replace(WaterGpu {
            field: tex,
            indices: ibuf,
            index_count: indices.len(),
        }) {
            self.retire(vec![old.field], Some(Box::new(old.indices)));
        }
        Ok(())
    }
}
