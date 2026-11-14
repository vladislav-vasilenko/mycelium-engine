//! Создание рендерера и загрузка ресурсов сцены: ландшафт, материалы, вода.

use std::time::Instant;

use winit::window::Window;

use super::{App, DETAIL_TEXTURE_SIZE};
use crate::hands::Hands;
use crate::renderer::{self, Renderer};
use crate::{platform, scene, terrain_materials, terrain_mesh, water};

impl App {
    /// Устройство, рендерер, ландшафт, вода и руки для окна. Без рук игра
    /// работает: ошибка ассета печатается и руки не рисуются.
    pub(super) fn create_renderer(
        &self,
        window: &Window,
    ) -> Result<
        (
            Renderer,
            water::ShoreField,
            Option<Hands>,
            crate::terrain_clusters::ClusterSet,
        ),
        String,
    > {
        let (device, info) = renderer::probe_device()?;
        eprintln!("{info}");
        let layer = platform::attach_metal_layer(window)?;
        let (vertices, indices) = scene::cube();
        let mut r = Renderer::new(device, info, layer, &vertices, &indices)?;
        let t0 = Instant::now();
        let terrain = terrain_mesh::vertices(&self.world);
        let detail = terrain_mesh::detail_texture(self.world.seed, DETAIL_TEXTURE_SIZE as usize);
        let t_mat = Instant::now();
        let materials = terrain_materials::load(&terrain_materials::source_dir())?;
        let mat_ms = t_mat.elapsed().as_secs_f64() * 1000.0;
        // Маска мокрого песка — тексель на шаг сетки (1 м у генератора, 30 м у острова).
        let empty = crate::world::features::FeatureSet::empty(self.world.size_m().x);
        let features = self.world.features.as_ref().unwrap_or(&empty);
        let wet_size = (self.world.tiles_x as u32) * (crate::world::TILE_SAMPLES as u32 - 1);
        r.set_terrain(
            &terrain,
            &terrain_mesh::tile_indices(),
            self.world.tiles.len(),
            &renderer::TerrainTextures {
                detail_rgba: &detail,
                detail_size: DETAIL_TEXTURE_SIZE,
                materials: &materials,
                wetness_size: wet_size,
                features: renderer::FeatureBuffers {
                    segments: &features.segments,
                    cells: &features.cells,
                    list: &features.list,
                },
            },
        )?;
        if !features.segments.is_empty() {
            eprintln!(
                "объекты карты: {} отрезков, ячеек {}², до {} отрезков в ячейке",
                features.segments.len(),
                features.cells_n,
                features.max_per_cell()
            );
        }
        eprintln!(
            "ландшафт: {} вершин, {} треугольников, материалов {} (JPEG за {mat_ms:.0} мс), загружен за {:.0} мс",
            terrain.len(),
            self.world.tiles.len() * 64 * 64 * 2,
            materials.layers,
            t0.elapsed().as_secs_f64() * 1000.0
        );
        // Кластерный рельеф (02.3): иерархия из поля высот полного разрешения.
        let t_clusters = Instant::now();
        let (clusters, cached) = crate::terrain_clusters::load_or_build(&self.world.heightfield);
        let (layer_size, weights_rgba, material_rgba) = terrain_mesh::attribute_layers(&self.world);
        r.set_terrain_clusters(&renderer::ClusterData {
            heights: &self.world.heightfield.heights,
            clusters: &clusters.gpu,
            vertices: &clusters.vertices,
            triangles: &clusters.triangles,
            layer_size,
            weights_rgba: &weights_rgba,
            material_rgba: &material_rgba,
            cover: self.world.cover.as_ref().map(|c| (&c.rgba[..], c.size)),
        })?;
        eprintln!(
            "кластеры рельефа: {} кластеров, {} групп, {} уровней, {} треугольников (уровень 0 — {}), {:.1} МБ, {} за {:.0} мс",
            clusters.gpu.len(),
            clusters.groups.len(),
            clusters.levels,
            clusters.triangle_count(),
            (self.world.heightfield.nx - 1) * (self.world.heightfield.nz - 1) * 2,
            (clusters.vertices.len() * 4 + clusters.triangles.len() * 4 + clusters.gpu.len() * 16)
                as f64
                / 1e6,
            if cached {
                "из кэша"
            } else {
                "собраны"
            },
            t_clusters.elapsed().as_secs_f64() * 1000.0
        );
        let t_water = Instant::now();
        let shore = water::ShoreField::from_world(&self.world);
        r.set_water(&shore)?;
        eprintln!(
            "вода: поле берега {}×{}, {} уровней сетки, готово за {:.0} мс",
            shore.size,
            shore.size,
            water::WATER_LEVELS,
            t_water.elapsed().as_secs_f64() * 1000.0
        );
        let t_hands = Instant::now();
        let hands = match Hands::load() {
            Ok(h) => {
                let a = &h.asset;
                r.set_hands(&a.vertices, &a.indices, &a.albedo, a.albedo_size)?;
                eprintln!(
                    "руки: {} вершин, {} костей, поз {}, загружены за {:.0} мс",
                    a.vertices.len(),
                    a.skeleton.joints.len(),
                    a.poses.len(),
                    t_hands.elapsed().as_secs_f64() * 1000.0
                );
                Some(h)
            }
            Err(e) => {
                eprintln!("руки не загружены: {e}");
                None
            }
        };
        Ok((r, shore, hands, clusters))
    }
}
