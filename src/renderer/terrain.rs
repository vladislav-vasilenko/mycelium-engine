//! Ландшафт на GPU: вершины всех тайлов, индексы, текстуры материалов и
//! маска мокрого песка.

use objc2_metal::*;

use super::Renderer;
use super::resources::{
    TextureUpload, as_allocation, buffer_with, sampled_texture, sampled_texture_usage,
};
use super::shaders::Obj;
use crate::layout::TerrainVertex;

/// Ландшафт на GPU: вершины всех тайлов подряд, общие индексы одного тайла,
/// детальная текстура с mip-уровнями.
pub(super) struct TerrainGpu {
    pub vertices: Obj<dyn MTLBuffer>,
    pub indices: Obj<dyn MTLBuffer>,
    pub index_count: usize,
    pub tile_count: usize,
    pub detail: Obj<dyn MTLTexture>,
    /// Массив материалов: RGB — альбедо (sRGB), A — высота для смешивания.
    pub albedo: Obj<dyn MTLTexture>,
    /// Массив материалов: RG — нормаль, B — шероховатость.
    pub normal: Obj<dyn MTLTexture>,
    /// Маска мокрого песка R16Float на всю карту, 1 тексель на метр;
    /// пишет ядро `wet_sand_update`, читает шейдер ландшафта.
    pub wetness: Obj<dyn MTLTexture>,
}

/// Текстуры ландшафта для `Renderer::set_terrain`.
pub struct TerrainTextures<'a> {
    pub detail_rgba: &'a [u8],
    pub detail_size: u32,
    pub materials: &'a crate::terrain_materials::MaterialArrays,
    /// Сторона маски мокрого песка, тексели (1 на метр).
    pub wetness_size: u32,
}

impl Renderer {
    /// Загружает ландшафт: вершины всех тайлов, общие индексы, детальную текстуру,
    /// массивы материалов и маску влажности. Mip строит GPU в отдельном командном
    /// буфере; CPU ждёт его завершения, поэтому кадры, отправленные позже, видят
    /// готовые текстуры без межкадрового барьера.
    pub fn set_terrain(
        &mut self,
        vertices: &[TerrainVertex],
        indices: &[u16],
        tile_count: usize,
        tex: &TerrainTextures,
    ) -> Result<(), String> {
        let vbuf = buffer_with(
            &self.device,
            bytemuck::cast_slice(vertices),
            "terrain vertices (all tiles)",
        );
        let ibuf = buffer_with(
            &self.device,
            bytemuck::cast_slice(indices),
            "terrain tile indices",
        );
        for a in [as_allocation(&*vbuf), as_allocation(&*ibuf)] {
            self.residency.addAllocation(a);
        }

        let m = tex.materials;
        let detail = sampled_texture(
            &self.device,
            MTLPixelFormat::RGBA8Unorm,
            tex.detail_size,
            1,
            true,
            "terrain detail (mipmapped)",
        )?;
        let albedo = sampled_texture(
            &self.device,
            MTLPixelFormat::RGBA8Unorm_sRGB,
            m.size as u32,
            m.layers,
            true,
            "terrain materials: albedo + height",
        )?;
        let normal = sampled_texture(
            &self.device,
            MTLPixelFormat::RGBA8Unorm,
            m.size as u32,
            m.layers,
            true,
            "terrain materials: normal + roughness",
        )?;
        let wetness = sampled_texture_usage(
            &self.device,
            MTLPixelFormat::R16Float,
            tex.wetness_size,
            1,
            false,
            MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite,
            "wet sand mask",
        )?;
        // Сухо: половинная точность, ноль — нулевые байты.
        let dry = vec![0u8; (tex.wetness_size * tex.wetness_size * 2) as usize];
        self.upload_textures(&[
            TextureUpload {
                texture: &detail,
                data: tex.detail_rgba,
                pixel_bytes: 4,
                mips: true,
            },
            TextureUpload {
                texture: &albedo,
                data: &m.albedo_height,
                pixel_bytes: 4,
                mips: true,
            },
            TextureUpload {
                texture: &normal,
                data: &m.normal_rough,
                pixel_bytes: 4,
                mips: true,
            },
            TextureUpload {
                texture: &wetness,
                data: &dry,
                pixel_bytes: 2,
                mips: false,
            },
        ])?;

        if let Some(old) = self.terrain.replace(TerrainGpu {
            vertices: vbuf,
            indices: ibuf,
            index_count: indices.len(),
            tile_count,
            detail,
            albedo,
            normal,
            wetness,
        }) {
            self.retire(
                vec![old.detail, old.albedo, old.normal, old.wetness],
                Some(Box::new((old.vertices, old.indices))),
            );
        }
        Ok(())
    }
}
