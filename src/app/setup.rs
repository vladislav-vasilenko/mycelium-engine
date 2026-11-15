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
    ) -> Result<(Renderer, water::ShoreField, Option<Hands>), String> {
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
        // Маска мокрого песка — окно 1024 × 1024 м вокруг игрока, 1 м на тексель
        // (кольцевая адресация), на карте любого размера.
        let wet_size = 1024;
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
        // Шрифт интерфейса (05.1).
        match crate::ui::Font::load(&crate::ui::assets_dir()) {
            Ok(f) => r.set_ui_font(&f.pixels, f.atlas_size)?,
            Err(e) => eprintln!("шрифт интерфейса не загружен: {e}"),
        }
        // Лес (02.4) — у мира с поясами растительности (Якусима).
        if self.world.cover.is_some() {
            let t = std::time::Instant::now();
            match crate::plants::load(
                &crate::plants::models_dir(),
                &crate::terrain_materials::source_dir(),
            )
            .and_then(|a| r.set_forest(&a, self.world.vegetation.as_ref()).map(|_| a))
            {
                Ok(a) => {
                    eprintln!(
                        "растения: {} видов, {} вершин, {} треугольников, за {} мс",
                        crate::plants::SPECIES.len(),
                        a.vertices.len(),
                        a.indices.len() / 3,
                        t.elapsed().as_millis()
                    );
                }
                Err(e) => eprintln!("растения не загружены: {e}"),
            }
        }
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
        // Звёзды неба (04.1): каталог BSC5; без файла небо остаётся без звёзд.
        let stars_path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/sky/stars.bin");
        match std::fs::read(&stars_path) {
            Ok(bytes) => r.set_stars(&bytes),
            Err(e) => eprintln!("звёзды не загружены: {}: {e}", stars_path.display()),
        }
        let t_hands = Instant::now();
        let hands = match Hands::load() {
            Ok(mut h) => {
                let a = &h.asset;
                r.set_hands(&a.vertices, &a.indices, &a.albedo, a.albedo_size)?;
                // Предметы в руках (этап 10): привязка к кисти в позе grip.
                match crate::hands::load_props(a, "grip") {
                    Ok(props) => {
                        for p in &props {
                            r.add_hand_prop(&p.vertices, &p.indices, &p.albedo, p.albedo_size)?;
                        }
                        eprintln!(
                            "предметы в руках: {}",
                            props
                                .iter()
                                .map(|p| p.spec.label.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                        h.props = props.into_iter().map(|p| p.spec).collect();
                    }
                    Err(e) => eprintln!("предметы в руках не загружены: {e}"),
                }
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
        Ok((r, shore, hands))
    }

    /// Кластеры рельефа (02.3) — в фоновом потоке (02.10): сборка после правки
    /// рельефа идёт около минуты; всё это время остров виден тайлами уровня 0,
    /// а интерфейс показывает ход загрузки. Самопроверка ждёт сразу.
    pub(super) fn start_clusters(&mut self) {
        let world = std::sync::Arc::clone(&self.world);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("terrain clusters".into())
            .spawn(move || {
                let _ = tx.send(crate::terrain_clusters::load_or_build(&world));
            })
            .expect("поток сборки кластеров");
        self.cluster_job = Some(super::ClusterJob {
            rx,
            started: Instant::now(),
        });
        if self.selftest.is_some()
            && let Some(job) = self.cluster_job.take()
            && let Ok((set, cached)) = job.rx.recv()
        {
            self.install_clusters(set, cached, job.started);
        }
    }

    /// Готовые кластеры — забрать из потока, если собрались (раз в кадр).
    pub(super) fn poll_clusters(&mut self) {
        let Some(job) = &self.cluster_job else { return };
        if let Ok((set, cached)) = job.rx.try_recv() {
            let started = job.started;
            self.cluster_job = None;
            self.install_clusters(set, cached, started);
        }
    }

    /// Кластеры на GPU: иерархия, срез, слои мира, зона.
    fn install_clusters(
        &mut self,
        clusters: crate::terrain_clusters::ClusterSet,
        cached: bool,
        t_clusters: Instant,
    ) {
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        let result = (|| -> Result<(), String> {
            // Кластерный рельеф (02.3): иерархия из поля высот полного разрешения.
            let cull = crate::terrain_clusters::cull_data(&clusters);
            let (layer_size, weights_rgba, material_rgba) =
                terrain_mesh::attribute_layers(&self.world);
            r.set_terrain_clusters(&renderer::ClusterData {
                heights: &self.world.heightfield.heights,
                clusters: &clusters.gpu,
                vertices: &clusters.vertices,
                triangles: &clusters.triangles,
                layer_size,
                weights_rgba: &weights_rgba,
                material_rgba: &material_rgba,
                cover: self.world.cover.as_ref().map(|c| (&c.rgba[..], c.size)),
                cull: &cull,
                macro_detail: self
                    .world
                    .macro_detail
                    .as_ref()
                    .map(|c| (&c.rgba[..], c.size)),
                zone: self.world.zone.as_ref().map(|z| {
                    let o = z.origin(&self.world.heightfield);
                    (
                        &z.grid.heights[..],
                        crate::layout::TerrainZone {
                            far_count: (self.world.heightfield.nx * self.world.heightfield.nz)
                                as u32,
                            n: z.grid.nx as u32,
                            step_m: z.grid.step_m,
                            origin_x: o.x,
                            origin_z: o.y,
                            _pad: [0.0; 3],
                        },
                    )
                }),
            })?;
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("кластеры рельефа не загружены: {e}");
            return;
        }
        eprintln!(
            "кластеры рельефа: {} кластеров, {} групп, {} уровней, {} треугольников (уровень 0 — {}), {:.1} МБ, {} за {:.0} мс",
            clusters.gpu.len(),
            clusters.groups.len(),
            clusters.levels,
            clusters.triangle_count(),
            clusters
                .info
                .iter()
                .zip(&clusters.gpu)
                .filter(|(i, _)| i.refined < 0)
                .map(|(_, g)| g.triangle_count as usize)
                .sum::<usize>(),
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
        self.clusters = Some(clusters);
    }
}
