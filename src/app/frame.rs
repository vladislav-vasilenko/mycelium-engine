//! Кадр приложения: тики симуляции, камера, параметры кадра, вода, отправка
//! в рендерер и разбор readback.

use std::path::PathBuf;
use std::time::Instant;

use glam::Vec3;
use winit::event_loop::ActiveEventLoop;

use super::cameras::{marker_posts, start_camera, terrain_check_camera};
use super::selftest::checks::{report_readback, report_terrain_readback};
use super::{App, LIGHT_DIR, SKY_COLOR, SceneMode, TEST_SCENE_CLEAR};
use crate::layout::FrameUniforms;
use crate::renderer::{self, FrameInput, FrameOutcome};
use crate::terrain_materials::TerrainView;
use crate::{reference, scene, sim, water};

impl App {
    /// Вода кадра: параметры моря и кольца сетки вокруг камеры.
    pub(super) fn water_frame(&self, camera: Vec3) -> Option<renderer::WaterFrame> {
        let shore = self.shore.as_ref()?;
        if self.scene != SceneMode::Terrain {
            return None;
        }
        let mut params = self.sea.params_for(shore);
        params.dt = self.frame_dt;
        params.probe = water::probe_points(&self.world);
        Some(renderer::WaterFrame {
            params,
            levels: water::water_levels(glam::Vec2::new(camera.x, camera.z)),
        })
    }

    pub(super) fn frame(&mut self, event_loop: &ActiveEventLoop) {
        if self.is_minimized() {
            // Свёрнутое окно не видно: кадр не кодируем, ресурсы освобождаем.
            if let Some(r) = self.renderer.as_mut() {
                r.collect_completed();
            }
            return;
        }
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f64();
        self.last_frame = now;
        self.frame_dt = dt.min(sim::MAX_FRAME_DT) as f32;

        // Поворот мыши применяется в каждом кадре, без ожидания тика (AD-32).
        self.sim.look(self.look.0, self.look.1);
        self.look = (0.0, 0.0);
        let (ticks, alpha) = self.clock.advance(dt);
        let input = self.tick_input();
        for _ in 0..ticks {
            self.sim.step(&input, Some(&self.world));
        }

        let selftest = self.selftest.is_some();
        let camera = match (selftest, self.scene) {
            (true, SceneMode::Cubes) => start_camera(),
            (true, SceneMode::Terrain) => terrain_check_camera(&self.world),
            (false, _) => self.sim.interpolated(alpha),
        };
        let water_frame = self.water_frame(camera.position);
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        let Some((w, h)) = r.size() else {
            r.collect_completed();
            return;
        };
        let proj = sim::projection(w as f32 / h as f32);
        let view = camera.view();
        // Время мира от тиков симуляции: волны и накат детерминированы.
        let time = ((self.sim.tick as f64 + alpha as f64) * sim::TICK_DT) as f32;
        let uniforms = FrameUniforms {
            view_proj: (proj * view).to_cols_array_2d(),
            camera_pos: camera.position.extend(1.0).to_array(),
            light_dir: LIGHT_DIR.normalize().extend(0.0).to_array(),
            time,
            exposure: 1.0,
            pattern_time: if selftest {
                reference::SELFTEST_PATTERN_TIME
            } else {
                time
            },
            frame_index: 0,
            // Самопроверка ландшафта читает числа из HDR-буфера (вид Raw).
            view_mode: if selftest && self.scene == SceneMode::Terrain {
                TerrainView::Raw as u32
            } else {
                self.view_mode as u32
            },
            map_size_m: self.world.size_m().x,
            _pad: [0.0; 2],
        };
        let instances = match self.scene {
            SceneMode::Cubes => scene::instances().to_vec(),
            SceneMode::Terrain => marker_posts(&self.world),
        };
        let capture = (self.capture_requested || self.args.capture_frame == Some(r.frame_index()))
            .then(|| {
                let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("captures");
                let _ = std::fs::create_dir_all(&dir);
                dir.join(format!("frame-{}.gputrace", r.frame_index()))
            });
        self.capture_requested = false;
        let readback = std::mem::take(&mut self.readback_requested);
        let outcome = r.render(&FrameInput {
            uniforms,
            instances: &instances,
            draw_terrain: self.scene == SceneMode::Terrain,
            clear_color: match self.scene {
                SceneMode::Cubes => TEST_SCENE_CLEAR,
                SceneMode::Terrain => SKY_COLOR,
            },
            readback,
            capture,
            water: water_frame,
        });
        if readback && outcome != FrameOutcome::Presented {
            self.readback_requested = true;
        }
        if outcome == FrameOutcome::Presented {
            self.frames_since_title += 1;
        }

        let readbacks = r.take_readbacks();
        for rb in &readbacks {
            if !selftest {
                match self.scene {
                    SceneMode::Cubes => report_readback(rb, view, proj),
                    SceneMode::Terrain => report_terrain_readback(rb, view, proj, &self.world),
                }
            }
        }
        if selftest {
            self.selftest_step(event_loop, readbacks, view, proj);
        }
        self.update_title(false);
    }
}
