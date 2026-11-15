//! Кадр приложения: тики симуляции, камера, параметры кадра, вода, отправка
//! в рендерер и разбор readback.

use crate::world::Ground;
use std::path::PathBuf;
use std::time::Instant;

use glam::Vec3;
use winit::event_loop::ActiveEventLoop;

use super::cameras::{marker_posts, start_camera, terrain_check_camera};
use super::selftest::checks::{report_readback, report_terrain_readback};
use super::shadows;
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
        self.poll_clusters();
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
        // Стволы рядом с игроком (02.4) — из расстановки GPU прошлых кадров.
        self.sim.obstacles.clear();
        if let Some(r) = self.renderer.as_ref() {
            let p = self.sim.curr.position;
            self.sim.obstacles.extend(
                r.forest_obstacles()
                    .iter()
                    .filter(|o| (o[0] - p.x).abs() < 12.0 && (o[1] - p.z).abs() < 12.0)
                    .map(|o| glam::Vec3::from(*o)),
            );
        }
        for _ in 0..ticks {
            // Вода реки у игрока (03.6): уровень и течение — для брода и плавания.
            self.sim.water = self
                .world
                .features
                .as_ref()
                .filter(|_| !self.river_levels.is_empty())
                .and_then(|f| {
                    let p = self.sim.curr.position;
                    let g = self
                        .world
                        .sample_ground(p.x, p.z)
                        .map_or(f32::MIN, |g| g.height);
                    self.hydrology
                        .water_at(f, &self.river_levels, glam::Vec2::new(p.x, p.z), g)
                        .map(|(level, flow)| crate::sim::Water { level, flow })
                });
            self.sim.step(&input, Some(&*self.world));
            if self.sim.tick % 60 == 0 && std::env::var_os("ECHO_SWIM_LOG").is_some() {
                let p = self.sim.curr.position;
                eprintln!(
                    "вода: {:?}, плывёт {}, позиция ({:.1}, {:.2}, {:.1})",
                    self.sim.water.map(|w| (w.level, w.flow)),
                    self.sim.swimming,
                    p.x,
                    p.y,
                    p.z
                );
            }
        }
        self.advance_time(ticks);

        let selftest = self.selftest.is_some();
        let camera = match (selftest, self.scene) {
            (true, SceneMode::Cubes) => start_camera(),
            (true, SceneMode::Terrain) => terrain_check_camera(&self.world),
            (false, _) => self.sim.interpolated(alpha),
        };
        // Макро-осмотр (05.2): камера приседает к грибу.
        let game_on = !selftest && self.scene == SceneMode::Terrain;
        let camera = if game_on {
            self.game.view(camera, &self.world)
        } else {
            camera
        };
        // Демо-сплав (03.2): камера плывёт по реке; симуляция — там же
        // (подгрузка зоны и лес следуют за ней).
        let camera = match self.raft.as_mut() {
            Some(r) if !selftest => {
                let c = r.step(&self.world, self.frame_dt, &self.river_levels);
                self.sim.prev = c;
                self.sim.curr = c;
                c
            }
            _ => camera,
        };
        let water_frame = self.water_frame(camera.position);
        // Глаза под водой: та же функция волн, что рисует море (03.1).
        let underwater_m = match (&water_frame, &self.shore) {
            (Some(wf), Some(shore)) if !selftest => {
                let t = ((self.sim.tick as f64 + alpha as f64) * sim::TICK_DT) as f32;
                let p = glam::Vec2::new(camera.position.x, camera.position.z);
                let w = water::sample_water(shore, &wf.params, p, t);
                if w.dry {
                    0.0
                } else {
                    (w.height - camera.position.y).max(0.0)
                }
            }
            _ => 0.0,
        };
        // Под водой реки (03.2): глаза ниже уровня разлива над точкой.
        let underwater_m = match self.world.features.as_ref() {
            Some(f) if !selftest && !self.river_levels.is_empty() => {
                let p = glam::Vec2::new(camera.position.x, camera.position.z);
                let g = self
                    .world
                    .sample_ground(p.x, p.y)
                    .map_or(f32::MIN, |g| g.height);
                self.hydrology
                    .level_at(f, &self.river_levels, p, g)
                    .map_or(underwater_m, |w| underwater_m.max(w - camera.position.y))
            }
            _ => underwater_m,
        };
        let Some((w, h)) = self.renderer.as_ref().and_then(|r| r.size()) else {
            if let Some(r) = self.renderer.as_mut() {
                r.collect_completed();
            }
            return;
        };
        let hands = self.hands_frame(&camera, w as f32 / h as f32);
        // Срез Сиратани (05.1–05.2): гриб под взглядом, чувство мицелия,
        // сообщения, интерфейс.
        self.game.tick(self.frame_dt);
        if game_on {
            let sensing = (self.args.sense
                || (self.keys.contains(&winit::keyboard::KeyCode::KeyC)
                    && self.sim.mode == sim::MoveMode::Walk))
                && !self.game.closeup.active();
            self.game
                .update_sense(self.frame_dt, camera.position, sensing, &self.world);
            self.game.update_focus(camera.position, camera.forward());
        }
        let props = if game_on {
            let mut p = self.game.instances(camera.position);
            p.extend(self.game.sense_beads(camera.position, &self.world));
            p
        } else {
            Vec::new()
        };
        let ui_verts = match (&self.font, game_on) {
            (Some(font), true) => {
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
                let mut b = crate::ui::UiBuilder::new(font, scale);
                self.game.ui(&mut b, w as f32 / scale, h as f32 / scale);
                if let Some(job) = &self.cluster_job {
                    // Ход загрузки (02.10): кластеры собираются в фоне.
                    let text = format!(
                        "Сборка детального рельефа… {:.0} с — пока видна упрощённая сетка",
                        job.started.elapsed().as_secs_f32()
                    );
                    let tw = b.font.width(&text, 18.0);
                    let x = (w as f32 / scale - tw) * 0.5;
                    b.rect(x - 14.0, 12.0, tw + 28.0, 32.0, [10, 14, 10, 170]);
                    b.text(x, 16.0, 18.0, [230, 235, 215, 255], &text);
                }
                b.verts
            }
            _ => Vec::new(),
        };
        let fov = if game_on {
            self.game.fov_deg()
        } else {
            crate::game::closeup::FOV_DEG
        };
        let proj0 = sim::projection_fov(w as f32 / h as f32, fov);
        let view = camera.view();
        let cam_hi = (camera.position / 256.0).floor() * 256.0;
        let rot = {
            let mut rot = view;
            rot.w_axis = glam::Vec4::W;
            rot
        };
        // Временное сглаживание (04.5): сдвиг субпикселя по Халтону (2, 3),
        // 8 положений; матрицы без сдвига — для перепроекции в проходе `taa`.
        let taa_on = !selftest
            && self.scene == SceneMode::Terrain
            && self.view_mode == TerrainView::Lit
            && !self.args.no_taa;
        // Апскейл MetalFX (04.6) — только с TAA (дрожание, история); в
        // отладочных видах и самопроверке — полное разрешение.
        if let Some(r) = self.renderer.as_mut() {
            r.set_render_scale(if taa_on {
                self.args.render_scale.unwrap_or(0.67)
            } else {
                1.0
            });
        }
        let (rw, rh) = self
            .renderer
            .as_ref()
            .and_then(|r| r.render_size())
            .unwrap_or((w, h));
        let (proj, taa) = if taa_on {
            self.taa_frame = self.taa_frame.wrapping_add(1);
            let k = self.taa_frame % 8 + 1;
            let j = glam::Vec2::new(halton(k, 2) - 0.5, halton(k, 3) - 0.5)
                * glam::Vec2::new(2.0 / rw as f32, 2.0 / rh as f32);
            let mut p = proj0;
            // ndc.x = clip.x / (−z): прибавка к z_axis сдвигает ndc на −Δ.
            p.z_axis.x -= j.x;
            p.z_axis.y -= j.y;
            let vp0 = proj0 * rot;
            let prev = self
                .taa_prev
                .filter(|(_, c, size)| *size == (w, h) && c.distance(camera.position) < 5.0);
            let u = crate::layout::TaaUniforms {
                inv_view_proj_rel: vp0.inverse().to_cols_array_2d(),
                prev_view_proj_rel: prev.map_or(vp0, |(m, ..)| m).to_cols_array_2d(),
                cam_delta: prev
                    .map_or(glam::Vec3::ZERO, |(_, c, _)| camera.position - c)
                    .extend(if prev.is_some() { 1.0 } else { 0.0 })
                    .to_array(),
                jitter: [j.x, j.y, rw as f32, rh as f32],
            };
            self.taa_prev = Some((vp0, camera.position, (w, h)));
            (p, Some(u))
        } else {
            self.taa_prev = None;
            (proj0, None)
        };
        let view_proj_rel = proj * rot;
        let terrain_scene = self.scene == SceneMode::Terrain;
        let mut sky = terrain_scene.then(|| self.sky_frame(view_proj_rel, self.frame_dt, h));
        // Объёмные облака считаются с TAA (матрицы и история) — флаг небу.
        if let Some(s) = sky.as_mut() {
            s.inputs.view[3] = if taa.is_some() { 1.0 } else { 0.0 };
        }
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        let pre_exposure = r.last_exposure();
        // Время мира от тиков симуляции: волны и накат детерминированы.
        let time = ((self.sim.tick as f64 + alpha as f64) * sim::TICK_DT) as f32;
        // Шаг времени мира между кадрами (04.6): ветер прошлого кадра для
        // векторов движения растений — в `cam_lo.w`.
        let world_dt = (time - self.prev_world_time).clamp(0.0, 0.1);
        // Погода идёт по игровым часам (ускорение хода суток).
        if !self.time.paused {
            let dt_game = self.frame_dt * self.time.scale as f32;
            self.weather.update(dt_game);
            // Реки (03.2): сток от дождя → расход → уровень; разлив в рендер
            // дважды в секунду.
            self.hydrology
                .update(dt_game, self.weather.rain, self.weather.ground_wet);
        }
        self.river_push_s -= self.frame_dt;
        if self.river_push_s <= 0.0 {
            self.river_push_s = 0.5;
            if let Some(f) = self.world.features.as_ref() {
                let spread = self.hydrology.spread(f.segments.len());
                r.set_river_spread(&spread);
                // Уровень воды (03.2): плёсы ровные, перепады — водопады.
                self.river_levels = self
                    .hydrology
                    .levels(f.segments.len(), super::raft::LEVEL_SCALE);
                r.set_river_levels(&self.river_levels);
                // Рыба (03.2): точки прыжков на реках в 60 м от камеры —
                // равномерно по найденным отрезкам.
                let cam = self.sim.curr.position;
                let near: Vec<usize> = f
                    .segments
                    .iter()
                    .enumerate()
                    .filter(|(i, s)| {
                        s.kind == crate::world::features::KIND_RIVER
                            && spread[*i][1] > 0.0
                            && glam::Vec2::from(s.a).distance(glam::Vec2::new(cam.x, cam.z)) < 40.0
                    })
                    .map(|(i, _)| i)
                    .collect();
                let stride = (near.len() / crate::renderer::FISH_MAX).max(2);
                let fish: Vec<[f32; 4]> = near
                    .iter()
                    .step_by(stride)
                    .filter_map(|&i| {
                        let s = &f.segments[i];
                        let (a, b) = (glam::Vec2::from(s.a), glam::Vec2::from(s.b));
                        let m = (a + b) * 0.5;
                        let ha = self.world.sample_ground(a.x, a.y)?.height;
                        let hb = self.world.sample_ground(b.x, b.y)?.height;
                        let flow = if hb > ha { a - b } else { b - a };
                        let y = 0.5 * (self.river_levels[i][0] + self.river_levels[i][1]);
                        Some([m.x, y, m.y, flow.y.atan2(flow.x)])
                    })
                    .collect();
                r.set_fish(&fish);
                if std::env::var_os("ECHO_FISH_LOG").is_some() {
                    eprintln!("рыба: {} (рек рядом {})", fish.len(), near.len());
                }
                // С `--gpu-times` — ближайшая река в журнал (проверка паводка).
                if self.args.gpu_times && r.frame_index() % 600 < 30 {
                    let p = self.sim.curr.position;
                    if let Some((rv, d)) = self.hydrology.nearest(f, glam::Vec2::new(p.x, p.z)) {
                        eprintln!(
                            "река в {d:.0} м: {:.3} м³/с, глубина {:.2} м (межень {:.2}), водосбор {:.2} км²",
                            rv.q,
                            rv.depth,
                            rv.base_depth,
                            rv.area_m2 / 1e6
                        );
                    }
                }
            }
        }
        self.prev_world_time = time;
        // Лес (02.4): кластерный рельеф, выбор на GPU, не в самопроверке.
        let forest = self.forest_loaded
            && self.use_clusters
            && self.clusters.is_some()
            && !self.args.cpu_cull
            && !selftest
            && self.scene == SceneMode::Terrain;
        let uniforms = FrameUniforms {
            view_proj: (proj * view).to_cols_array_2d(),
            // w — кинематографичная обработка в тонмаппинге (04.5): 1 — вкл.
            camera_pos: camera
                .position
                .extend(if self.grade { 1.0 } else { 0.0 })
                .to_array(),
            // К главному светилу — со знаком «от источника к сцене».
            light_dir: match &sky {
                Some(s) => (-s.light_to).extend(0.0).to_array(),
                None => LIGHT_DIR.normalize().extend(0.0).to_array(),
            },
            time,
            // Небо: автоэкспозиция (−1) или фиксированная `--exposure`; кубы — 1.
            // Отладочные виды и самопроверка — фиксированная экспозиция 1.
            exposure: if terrain_scene && self.view_mode == TerrainView::Lit && !selftest {
                self.args.exposure.unwrap_or(-1.0)
            } else {
                1.0
            },
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
            grid_nx: self.world.heightfield.nx as u32,
            grid_step_m: self.world.heightfield.step_m,
            underwater_m,
            canopy_eye: self.world.cover.as_ref().map_or(0.0, |c| {
                let w = c.at(camera.position.x, camera.position.z, self.world.size_m().x);
                w[0] + w[1]
            }),
            // Предэкспозиция — экспозиция прошлого замера; кубы и отладочные виды — 1.
            pre_exposure: if terrain_scene && self.view_mode == TerrainView::Lit && !selftest {
                pre_exposure
            } else {
                1.0
            },
            forest: if forest { 1.0 } else { 0.0 },
            // Относительно камеры (AD-49): поворот вида без сдвига, камера =
            // крупная часть (кратна 256 м) + остаток.
            view_proj_rel: view_proj_rel.to_cols_array_2d(),
            // w — масштаб рендера (04.6): шейдеры уменьшают на него производные
            // для выбора мипов — текстуры чёткие, как в выходном разрешении.
            cam_hi: cam_hi.extend(rw as f32 / w as f32).to_array(),
            cam_lo: (camera.position - cam_hi).extend(world_dt).to_array(),
            weather: self.weather.uniforms(),
        };
        // Срез кластерного рельефа по экранной ошибке 1 пиксель (02.3): на GPU
        // (02.5) или на CPU (`--cpu-cull`). Самопроверка считает и CPU-срез —
        // эталон для сверки с GPU.
        let use_clusters =
            self.use_clusters && self.scene == SceneMode::Terrain && self.clusters.is_some();
        let gpu_cull = use_clusters && !self.args.cpu_cull;
        let mut main_view = None;
        if use_clusters {
            let set = self.clusters.as_ref().unwrap();
            // Самопроверка вида Raw сверяет кластеры уровня 0 с CPU точно:
            // отрицательный порог пропускает только исходную сетку (в ровных
            // местах ошибка упрощения почти нулевая, и малый порог её не держит).
            let threshold = if selftest { -1.0 } else { 1.0 };
            let v = crate::terrain_clusters::View::new(
                view,
                proj,
                h as f32,
                sim::NEAR_PLANE_M,
                threshold,
            );
            main_view = Some(v);
            if !gpu_cull || selftest {
                let t = Instant::now();
                self.cluster_stats =
                    crate::terrain_clusters::select(set, &v, true, &mut self.visible_clusters);
                self.cluster_select_ms = t.elapsed().as_secs_f64() * 1000.0;
            }
        }
        // Каскады теней (04.2): на CPU списки каскадов — после основного.
        let cascades = match (&sky, use_clusters) {
            (Some(s), true) => shadows::shadow_frame(
                (!gpu_cull).then(|| self.clusters.as_ref().unwrap()),
                camera.position,
                self.world.size_m().x,
                s.light_to,
                renderer::MAX_VISIBLE_CLUSTERS
                    - self
                        .visible_clusters
                        .len()
                        .min(renderer::MAX_VISIBLE_CLUSTERS),
                &mut self.shadow_clusters,
                &mut self.shadow_scratch,
            ),
            _ => None,
        };
        let cull = main_view.filter(|_| gpu_cull).map(|v| {
            let mut views = [crate::layout::CullView::default(); renderer::CULL_VIEWS];
            views[0] = v.to_gpu();
            if let Some((_, cv)) = &cascades {
                for (k, c) in cv.iter().enumerate() {
                    views[k + 1] = c.to_gpu();
                }
            }
            renderer::CullFrame { views }
        });
        // Поворот диска мягких теней меняется каждый кадр, TAA усредняет шум
        // (04.5); без TAA — 0, диск неподвижен.
        let shadow_seed = if taa.is_some() {
            (self.taa_frame % 64) as f32 + 1.0
        } else {
            0.0
        };
        let shadows = cascades.map(|(mut s, _)| {
            s.uniforms.params[3] = shadow_seed;
            s
        });
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
        if self.args.gpu_times && r.frame_index() % 120 == 0 && r.frame_index() > 0 {
            let mut v: Vec<_> = r.gpu_pass_ms().to_vec();
            let total: f64 = v.iter().map(|x| x.1).sum();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            let line: Vec<String> = v
                .iter()
                .take(8)
                .map(|(n, ms)| format!("{n} {ms:.2}"))
                .collect();
            eprintln!("GPU {total:.2} мс: {}", line.join(", "));
        }
        let outcome = r.render(&FrameInput {
            uniforms,
            instances: &instances,
            draw_terrain: self.scene == SceneMode::Terrain,
            clear_color: match self.scene {
                SceneMode::Cubes => TEST_SCENE_CLEAR,
                // Под водой фон — цвет толщи (как `UW_COLOR` в common.h).
                SceneMode::Terrain if underwater_m > 0.0 => {
                    let k = |c: f32, a: f32| c * (-a * underwater_m).exp();
                    [k(0.020, 0.42), k(0.115, 0.085), k(0.135, 0.055)]
                }
                SceneMode::Terrain => SKY_COLOR,
            },
            readback,
            capture,
            water: water_frame,
            hands,
            clusters: (use_clusters && !gpu_cull).then_some(&self.visible_clusters[..]),
            shadows,
            shadow_clusters: &self.shadow_clusters,
            cull,
            forest,
            props: &props,
            ui: &ui_verts,
            taa,
            rt: !self.args.no_rt,
            bloom: !selftest
                && self.scene == SceneMode::Terrain
                && self.view_mode == TerrainView::Lit,
            sky: sky.as_ref().map(|s| s.inputs),
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

/// Последовательность Халтона: `index`-й член по основанию `base`, в [0, 1).
fn halton(mut index: u32, base: u32) -> f32 {
    let (mut f, mut r) = (1.0f32, 0.0f32);
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}
