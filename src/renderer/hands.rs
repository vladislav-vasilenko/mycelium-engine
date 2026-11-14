//! Руки на GPU (этап 10): вершины со скином, индексы и текстура кожи.
//! Палитра костей приходит в каждом кадре через upload (`HandsFrame`).

use objc2_metal::*;

use super::Renderer;
use super::resources::{TextureUpload, as_allocation, buffer_with, sampled_texture};
use super::shaders::Obj;
use crate::layout::{HandsUniforms, SkinnedVertex};

/// Сколько костей помещается в палитру upload.
pub const MAX_JOINTS: usize = 64;

pub(super) struct HandsGpu {
    pub vertices: Obj<dyn MTLBuffer>,
    pub indices: Obj<dyn MTLBuffer>,
    pub index_count: usize,
    pub albedo: Obj<dyn MTLTexture>,
}

/// Руки в кадре: параметры прохода и палитра (по столбцам, как `float4x4`).
pub struct HandsFrame {
    pub uniforms: HandsUniforms,
    pub palette: Vec<[[f32; 4]; 4]>,
}

impl Renderer {
    /// Загружает сетку рук и текстуру кожи (RGBA8 sRGB, квадрат `albedo_size`²).
    pub fn set_hands(
        &mut self,
        vertices: &[SkinnedVertex],
        indices: &[u16],
        albedo: &[u8],
        albedo_size: u32,
    ) -> Result<(), String> {
        let vbuf = buffer_with(
            &self.device,
            bytemuck::cast_slice(vertices),
            "hands vertices",
        );
        let ibuf = buffer_with(&self.device, bytemuck::cast_slice(indices), "hands indices");
        for a in [as_allocation(&*vbuf), as_allocation(&*ibuf)] {
            self.residency.addAllocation(a);
        }
        let tex = sampled_texture(
            &self.device,
            MTLPixelFormat::RGBA8Unorm_sRGB,
            albedo_size,
            1,
            true,
            "hands skin albedo",
        )?;
        self.upload_textures(&[TextureUpload {
            texture: &tex,
            data: albedo,
            pixel_bytes: 4,
            mips: true,
        }])?;
        if let Some(old) = self.hands.replace(HandsGpu {
            vertices: vbuf,
            indices: ibuf,
            index_count: indices.len(),
            albedo: tex,
        }) {
            self.retire(
                vec![old.albedo],
                Some(Box::new((old.vertices, old.indices))),
            );
        }
        Ok(())
    }
}
