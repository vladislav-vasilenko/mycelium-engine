//! Заголовок окна: частота кадров, состояние MSL, место игрока, вид.

use std::time::Instant;

use super::{App, SceneMode};
use crate::sim::{self, MoveMode};
use crate::terrain_materials::TerrainView;
use crate::water;
use crate::world::Ground;

impl App {
    pub(super) fn update_title(&mut self, force: bool) {
        let elapsed = self.title_timer.elapsed().as_secs_f64();
        if !force && elapsed < 0.5 {
            return;
        }
        let (Some(window), Some(r)) = (&self.window, &self.renderer) else {
            return;
        };
        let fps = self.frames_since_title as f64 / elapsed.max(1e-6);
        let size = r
            .size()
            .map_or("свёрнуто".to_string(), |(w, h)| format!("{w}×{h}"));
        let status = match &self.last_error {
            Some(e) => format!(
                "ОШИБКА MSL {e}, работает поколение {}",
                r.pipeline_generation()
            ),
            None => format!("MSL поколение {}", r.pipeline_generation()),
        };
        let p = self.sim.curr.position;
        // Вода под игроком — CPU-запрос `sample_water` (та же функция волн, что на GPU).
        let water_note = match (&self.shore, self.world.sample_ground(p.x, p.z)) {
            (Some(shore), Some(_)) => {
                let t = (self.sim.tick as f64 * sim::TICK_DT) as f32;
                let w = water::sample_water(
                    shore,
                    &self.sea.params_for(shore),
                    glam::Vec2::new(p.x, p.z),
                    t,
                );
                if w.dry {
                    String::new()
                } else {
                    format!(", вода {:.2} м (поверхность {:+.2} м)", w.depth, w.height)
                }
            }
            _ => String::new(),
        };
        let place = match (self.scene, self.world.sample_ground(p.x, p.z)) {
            (SceneMode::Terrain, Some(g)) => format!(
                " · {} · x {:.0} z {:.0} · тайл ({}, {}) · {:?}{}{} · {:.1} м",
                if self.sim.mode == MoveMode::Walk {
                    "ходьба"
                } else {
                    "полёт"
                },
                // Координаты для воспроизведения снимков (`--camera x,y,z,…`).
                p.x,
                p.z,
                g.tile.x,
                g.tile.z,
                g.substrate,
                if g.on_trail { ", тропа" } else { "" },
                water_note,
                g.height
            ),
            (SceneMode::Terrain, None) => " · за картой".into(),
            (SceneMode::Cubes, _) => " · тестовые кубы".into(),
        };
        let view = match (self.scene, self.view_mode) {
            (SceneMode::Cubes, _) | (_, TerrainView::Lit) => String::new(),
            (_, v) => format!(" · вид: {}", v.name()),
        };
        let terrain = if self.scene == SceneMode::Terrain {
            if self.use_clusters && self.clusters.is_some() {
                match self.renderer.as_ref().and_then(|r| r.cull_stats()) {
                    Some(g) if !self.args.cpu_cull => format!(
                        " · рельеф: {} кл. (GPU), тени {}{}{}",
                        g.visible[0],
                        g.visible[1..].iter().sum::<u32>(),
                        if g.overflow > 0 {
                            format!(", переполнение {}", g.overflow)
                        } else {
                            String::new()
                        },
                        match self.renderer.as_ref().and_then(|r| r.forest_stats()) {
                            Some(f) if self.forest_loaded => format!(
                                " · лес: {} раст., в тенях {}{}",
                                f.visible,
                                f.shadow,
                                if f.overflow {
                                    " (переполнение)"
                                } else {
                                    ""
                                }
                            ),
                            _ => String::new(),
                        }
                    ),
                    _ => {
                        let s = self.cluster_stats;
                        format!(
                            " · рельеф: {} кл., {:.2} млн тр., выбор {:.2} мс",
                            s.clusters,
                            s.triangles as f64 / 1e6,
                            self.cluster_select_ms
                        )
                    }
                }
            } else {
                " · рельеф: тайлы".to_string()
            }
        } else {
            String::new()
        };
        let clock = if self.scene == SceneMode::Terrain {
            // Ближайшая река (03.2): расход и глубина.
            let river = self.world.features.as_ref().and_then(|f| {
                let p = self.sim.curr.position;
                self.hydrology
                    .nearest(f, glam::Vec2::new(p.x, p.z))
                    .filter(|(_, d)| *d < 150.0)
                    .map(|(r, _)| format!(" · река {:.2} м³/с, {:.2} м", r.q, r.depth))
            });
            format!(
                " · {} · {}{}{}",
                self.time_label(),
                self.weather.preset.label(),
                river.unwrap_or_default(),
                if self.grade {
                    ""
                } else {
                    " · без обработки"
                }
            )
        } else {
            String::new()
        };
        let hands = match &self.hands {
            Some(h) if self.hands_visible && self.sim.mode == MoveMode::Walk => {
                format!(" · руки: {}", crate::hands::POSE_NAMES[h.rig.target()])
            }
            _ => String::new(),
        };
        window.set_title(&format!(
            "Эхо мицелия · {} · {size} · {fps:.0} fps · CPU {:.2} мс · {status}{clock}{place}{view}{terrain}{hands}",
            r.info.name, r.stats.cpu_encode_ms
        ));
        self.title_timer = Instant::now();
        self.frames_since_title = 0;
    }
}
