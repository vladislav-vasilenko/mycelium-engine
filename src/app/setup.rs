//! Создание рендерера и загрузка ресурсов сцены: ландшафт, материалы, вода.

use std::time::Instant;

use winit::window::Window;

use super::{App, DETAIL_TEXTURE_SIZE};
use crate::renderer::{self, Renderer};
use crate::{platform, scene, terrain_materials, terrain_mesh, water};

impl App {
    /// Устройство, рендерер, ландшафт и вода для окна.
    pub(super) fn create_renderer(
        &self,
        window: &Window,
    ) -> Result<(Renderer, water::ShoreField), String> {
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
            },
        )?;
        eprintln!(
            "ландшафт: {} вершин, {} треугольников, материалов {} (JPEG за {mat_ms:.0} мс), загружен за {:.0} мс",
            terrain.len(),
            self.world.tiles.len() * 64 * 64 * 2,
            materials.layers,
            t0.elapsed().as_secs_f64() * 1000.0
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
        Ok((r, shore))
    }
}
