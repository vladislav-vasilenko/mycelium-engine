//! Сценарий самопроверки (`--selftest`): шаги по кадрам, итог и код выхода.
//! Сверки с эталонами — в `checks`.

pub(super) mod checks;

use std::time::Instant;

use glam::{Mat4, Vec3};
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;

use super::{App, SceneMode};
use crate::platform;
use crate::renderer::Readback;
use checks::{verify_readback, verify_skin_probe, verify_terrain_readback, verify_water_probe};

/// Сценарий самопроверки. Шаги идут по очереди, каждый ждёт своё условие.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum TestStep {
    Warmup,
    ReadbackA,
    HotReload,
    AfterBrokenReload {
        frames: u32,
    },
    /// Окно свёрнуто; разворачивает `about_to_wait` по таймеру.
    /// `presented_at` — счётчик кадров в момент, когда система подтвердила сворачивание.
    Minimized {
        since: Instant,
        presented_at: Option<u64>,
    },
    /// Окно развёрнуто, ждём возобновления вывода.
    Restored {
        presented_before: u64,
        presented_while_minimized: u64,
    },
    Resize,
    ReadbackB,
    /// Сцена переключена на ландшафт с контрольной камерой.
    TerrainView,
    TerrainReadback,
    Done,
}

pub(super) struct SelfTest {
    pub step: TestStep,
    pub frames_in_step: u32,
    pub failures: Vec<String>,
    pub log: Vec<String>,
    pub resize_target: PhysicalSize<u32>,
    pub size_before: Option<(u32, u32)>,
}

impl SelfTest {
    pub fn check(&mut self, ok: bool, what: String) {
        let line = format!("[{}] {what}", if ok { "pass" } else { "FAIL" });
        println!("{line}");
        self.log.push(line);
        if !ok {
            self.failures.push(what);
        }
    }
}

impl SelfTest {
    pub fn new() -> SelfTest {
        SelfTest {
            step: TestStep::Warmup,
            frames_in_step: 0,
            failures: Vec::new(),
            log: Vec::new(),
            resize_target: PhysicalSize::new(960, 600),
            size_before: None,
        }
    }
}

impl App {
    pub(super) fn selftest_step(
        &mut self,
        event_loop: &ActiveEventLoop,
        readbacks: Vec<Readback>,
        view: Mat4,
        proj: Mat4,
    ) {
        let mut t = self.selftest.take().unwrap();
        if t.step == TestStep::Done {
            // winit может доставить ещё кадр после exit(): итог уже напечатан.
            self.selftest = Some(t);
            return;
        }
        let water_frame = self.water_frame(Vec3::ZERO);
        let r = self.renderer.as_mut().unwrap();
        t.frames_in_step += 1;
        let total_frames = r.frame_index();
        match t.step {
            TestStep::Warmup => {
                if t.frames_in_step == 1 {
                    println!("{}", r.info);
                    println!("план проходов (с readback):\n{}", r.plan_summary());
                }
                // Захват мыши как по клику: скрытие курсора и Locked-режим.
                if let Some(window) = &self.window {
                    if t.frames_in_step == 2 {
                        platform::grab_cursor(window, true);
                    }
                    if t.frames_in_step == 4 {
                        platform::grab_cursor(window, false);
                        t.check(true, "захват и освобождение курсора".into());
                    }
                }
                if t.frames_in_step >= 5 {
                    self.readback_requested = true;
                    t.step = TestStep::ReadbackA;
                    t.frames_in_step = 0;
                }
            }
            TestStep::ReadbackA | TestStep::ReadbackB => {
                if let Some(rb) = readbacks.first() {
                    verify_readback(&mut t, rb, view, proj);
                    if t.step == TestStep::ReadbackA {
                        t.step = TestStep::HotReload;
                    } else {
                        self.scene = SceneMode::Terrain;
                        t.step = TestStep::TerrainView;
                    }
                    t.frames_in_step = 0;
                }
            }
            TestStep::HotReload => {
                let before = r.pipeline_generation();
                let ok = r.reload_shaders(None);
                t.check(
                    ok.is_ok(),
                    format!(
                        "перезагрузка MSL без изменений: {:?}",
                        ok.as_ref().map_err(|e| e.to_string())
                    ),
                );
                let generation = r.pipeline_generation();
                t.check(
                    generation == before + 1,
                    format!("поколение pipeline {before} → {generation}"),
                );
                let broken = "#include \"common.h\"\nkernel void pattern_main( { syntax error }\n";
                let err = r.reload_shaders(Some(("pattern.metal", broken)));
                match &err {
                    Err(e) => {
                        println!(
                            "       диагностика: {}",
                            e.to_string().lines().next().unwrap_or("")
                        );
                        t.check(
                            e.file == "pattern.metal" && e.stage == "compile MSL",
                            format!(
                                "ошибочная правка отклонена: файл {}, стадия {}",
                                e.file, e.stage
                            ),
                        );
                    }
                    Ok(_) => t.check(false, "ошибочная правка не должна компилироваться".into()),
                }
                t.check(
                    r.pipeline_generation() == generation,
                    "после ошибки остаётся предыдущее поколение pipeline".into(),
                );
                t.step = TestStep::AfterBrokenReload { frames: 0 };
                t.frames_in_step = 0;
            }
            TestStep::AfterBrokenReload { frames } => {
                if frames >= 3 {
                    t.check(
                        true,
                        format!("кадры рисуются после ошибочной правки ({frames} кадра)"),
                    );
                    let ok = r.reload_shaders(None);
                    t.check(
                        ok.is_ok(),
                        "исправленная правка подхватывается в том же запуске".into(),
                    );
                    if let Some(window) = &self.window {
                        window.set_minimized(true);
                    }
                    t.step = TestStep::Minimized {
                        since: Instant::now(),
                        presented_at: None,
                    };
                    t.frames_in_step = 0;
                } else {
                    t.step = TestStep::AfterBrokenReload { frames: frames + 1 };
                }
            }
            // Пока окно свёрнуто, кадры могут не приходить вовсе; разворачивает
            // окно `about_to_wait` через секунду.
            TestStep::Minimized { .. } => {}
            TestStep::Restored {
                presented_before,
                presented_while_minimized,
            } => {
                let presented = r.stats.presented - presented_before;
                if presented >= 5 {
                    let minimized = self.window.as_ref().and_then(|w| w.is_minimized());
                    t.check(
                        presented_while_minimized == 0,
                        format!("в свёрнутом окне вывод пропускается: выведено {presented_while_minimized} кадров"),
                    );
                    t.check(
                        minimized == Some(false),
                        format!(
                            "после разворота вывод возобновился: {presented} кадров, пропусков без drawable {}",
                            r.stats.skipped_no_drawable
                        ),
                    );
                    t.size_before = r.size();
                    if let Some(window) = &self.window {
                        let _ = window.request_inner_size(t.resize_target);
                    }
                    t.step = TestStep::Resize;
                    t.frames_in_step = 0;
                } else if t.frames_in_step > 300 {
                    t.check(false, "вывод не возобновился после разворота окна".into());
                    t.step = TestStep::Done;
                }
            }
            TestStep::Resize => {
                let size = r.size();
                if let Some(new_size) =
                    size.filter(|_| size != t.size_before && t.frames_in_step >= 4)
                {
                    t.check(true, format!("resize: новые цели {new_size:?}"));
                    self.readback_requested = true;
                    t.step = TestStep::ReadbackB;
                    t.frames_in_step = 0;
                } else if t.frames_in_step > 120 {
                    t.check(false, "resize не пришёл за 120 кадров".into());
                    t.step = TestStep::Done;
                }
            }
            TestStep::TerrainView => {
                if t.frames_in_step >= 3 {
                    self.readback_requested = true;
                    t.step = TestStep::TerrainReadback;
                    t.frames_in_step = 0;
                }
            }
            TestStep::TerrainReadback => {
                if let Some(rb) = readbacks.first() {
                    verify_terrain_readback(&mut t, rb, view, proj, &self.world);
                    verify_water_probe(&mut t, rb, water_frame, self.shore.as_ref());
                    verify_skin_probe(&mut t, rb, self.hands.as_ref());
                    t.step = TestStep::Done;
                }
            }
            TestStep::Done => {}
        }
        if t.step != TestStep::Done && total_frames > 1000 {
            t.check(false, format!("самопроверка зависла на шаге {:?}", t.step));
            t.step = TestStep::Done;
        }
        if t.step == TestStep::Done {
            let s = &r.stats;
            println!(
                "кадров {}, пропуски: занят слот {}, нет drawable {}; отложенно освобождено {}; CPU кодирование последнего кадра {:.3} мс",
                s.presented,
                s.skipped_busy,
                s.skipped_no_drawable,
                s.released_deferred,
                s.cpu_encode_ms
            );
            if t.failures.is_empty() {
                println!("SELFTEST PASSED ({} проверок)", t.log.len());
                self.exit_code = 0;
            } else {
                println!("SELFTEST FAILED: {} из {}", t.failures.len(), t.log.len());
                self.exit_code = 1;
            }
            event_loop.exit();
        }
        self.selftest = Some(t);
    }
}
