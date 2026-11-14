//! Кластерный рельеф на GPU (02.3): поле высот, кластеры, их вершины и
//! треугольники, атрибуты биомов и материалов слоями мира. Список видимых
//! кластеров приходит в каждом кадре (`FrameInput::clusters`) в буфер слота.

use objc2_metal::*;

use super::Renderer;
use super::resources::{TextureUpload, as_allocation, buffer_with, sampled_texture};
use super::shaders::Obj;
use crate::layout::TerrainCluster;

/// Видимых кластеров в кадре не больше (буфер слота по 4 байта).
pub const MAX_VISIBLE_CLUSTERS: usize = 1 << 18;
/// Вершин на экземпляр-кластер: 128 треугольников × 3.
pub const CLUSTER_VERTEX_COUNT: usize = 128 * 3;

pub(super) struct ClustersGpu {
    pub heights: Obj<dyn MTLBuffer>,
    pub clusters: Obj<dyn MTLBuffer>,
    pub vertices: Obj<dyn MTLBuffer>,
    pub triangles: Obj<dyn MTLBuffer>,
    /// Слои мира на сетке биомов: веса (лес, торф, берег, тропа) и материал
    /// (субстрат, русло, влажность, тип берега), RGBA8.
    pub weights: Obj<dyn MTLTexture>,
    pub material: Obj<dyn MTLTexture>,
    /// Пояса растительности RGBA8 (02.7); без слоя — 1 × 1 нулей.
    pub cover: Obj<dyn MTLTexture>,
    /// Игровая зона: высоты и параметры (`TerrainZone`); без зоны — заглушки.
    pub zone_heights: Obj<dyn MTLBuffer>,
    pub zone_info: Obj<dyn MTLBuffer>,
}

/// Данные кластерного рельефа для `Renderer::set_terrain_clusters`.
pub struct ClusterData<'a> {
    pub heights: &'a [f32],
    pub clusters: &'a [TerrainCluster],
    pub vertices: &'a [u32],
    pub triangles: &'a [u32],
    /// Сторона квадратной сетки биомов и её RGBA8-слои.
    pub layer_size: u32,
    pub weights_rgba: &'a [u8],
    pub material_rgba: &'a [u8],
    /// Пояса растительности: RGBA8 и сторона, если есть.
    pub cover: Option<(&'a [u8], u32)>,
    /// Игровая зона: высоты (n²) и параметры, если есть.
    pub zone: Option<(&'a [f32], crate::layout::TerrainZone)>,
}

impl Renderer {
    pub fn set_terrain_clusters(&mut self, d: &ClusterData) -> Result<(), String> {
        let buf = |bytes: &[u8], name: &str| buffer_with(&self.device, bytes, name);
        let heights = buf(bytemuck::cast_slice(d.heights), "terrain heightfield");
        let clusters = buf(bytemuck::cast_slice(d.clusters), "terrain clusters");
        let vertices = buf(bytemuck::cast_slice(d.vertices), "terrain cluster vertices");
        let triangles = buf(
            bytemuck::cast_slice(d.triangles),
            "terrain cluster triangles",
        );
        let no_zone = crate::layout::TerrainZone {
            far_count: u32::MAX,
            n: 1,
            step_m: 1.0,
            origin_x: 0.0,
            origin_z: 0.0,
            _pad: [0.0; 3],
        };
        let (zh, zinfo) = d.zone.unwrap_or((&[0.0], no_zone));
        let zone_heights = buf(bytemuck::cast_slice(zh), "terrain zone heights");
        let zone_info = buf(bytemuck::bytes_of(&zinfo), "terrain zone info");
        for b in [
            &heights,
            &clusters,
            &vertices,
            &triangles,
            &zone_heights,
            &zone_info,
        ] {
            self.residency.addAllocation(as_allocation(&**b));
        }
        let layer = |name: &str| {
            sampled_texture(
                &self.device,
                MTLPixelFormat::RGBA8Unorm,
                d.layer_size,
                1,
                false,
                name,
            )
        };
        let none = [0u8; 4];
        let (cover_rgba, cover_size) = d.cover.unwrap_or((&none, 1));
        let cover = sampled_texture(
            &self.device,
            MTLPixelFormat::RGBA8Unorm,
            cover_size,
            1,
            false,
            "terrain layer: vegetation belts",
        )?;
        let weights = layer("terrain layer: biome weights")?;
        let material = layer("terrain layer: material")?;
        self.upload_textures(&[
            TextureUpload {
                texture: &weights,
                data: d.weights_rgba,
                pixel_bytes: 4,
                mips: false,
            },
            TextureUpload {
                texture: &material,
                data: d.material_rgba,
                pixel_bytes: 4,
                mips: false,
            },
            TextureUpload {
                texture: &cover,
                data: cover_rgba,
                pixel_bytes: 4,
                mips: false,
            },
        ])?;
        if let Some(old) = self.clusters.replace(ClustersGpu {
            heights,
            clusters,
            vertices,
            triangles,
            weights,
            material,
            cover,
            zone_heights,
            zone_info,
        }) {
            self.retire(
                vec![old.weights, old.material, old.cover],
                Some(Box::new((
                    old.heights,
                    old.clusters,
                    old.vertices,
                    old.triangles,
                    old.zone_heights,
                    old.zone_info,
                ))),
            );
        }
        Ok(())
    }
}
