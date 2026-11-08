//! События winit: окно, клавиатура, мышь, цикл ожидания.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

use super::App;
use super::selftest::TestStep;

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Эхо мицелия")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("не удалось создать окно: {e}");
                self.exit_code = 2;
                event_loop.exit();
                return;
            }
        };
        let result = self.create_renderer(&window);
        match result {
            Ok((mut r, shore)) => {
                self.shore = Some(shore);
                let size = window.inner_size();
                r.resize(size.width, size.height, window.scale_factor());
                eprintln!(
                    "кадр {}×{} px, шейдеры из {}",
                    size.width,
                    size.height,
                    r.shader_dir().display()
                );
                self.renderer = Some(r);
            }
            Err(e) => {
                eprintln!("ОШИБКА: {e}");
                self.exit_code = 2;
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
        self.update_title(true);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width, size.height, scale);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.set_grab(false);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !self.grabbed {
                    self.set_grab(true);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat,
                        ..
                    },
                ..
            } => match state {
                ElementState::Pressed => self.on_key_pressed(code, repeat),
                ElementState::Released => {
                    self.keys.remove(&code);
                }
            },
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.grabbed
        {
            self.look.0 += delta.0 as f32;
            self.look.1 += delta.1 as f32;
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Самопроверка: сворачивание асинхронное (анимация macOS). Счёт кадров
        // начинается, когда окно действительно свёрнуто; через секунду — разворот.
        let minimized_now = self.is_minimized();
        if let (Some(t), Some(w), Some(r)) = (self.selftest.as_mut(), &self.window, &self.renderer)
            && let TestStep::Minimized {
                since,
                presented_at,
            } = &mut t.step
        {
            if presented_at.is_none() && minimized_now {
                *presented_at = Some(r.stats.presented);
                *since = Instant::now();
            }
            let elapsed = since.elapsed().as_secs_f64();
            match *presented_at {
                Some(at) if elapsed >= 1.0 => {
                    w.set_minimized(false);
                    w.focus_window();
                    t.step = TestStep::Restored {
                        presented_before: r.stats.presented,
                        presented_while_minimized: r.stats.presented - at,
                    };
                    t.frames_in_step = 0;
                }
                None if elapsed >= 5.0 => {
                    t.check(false, "окно не свернулось за 5 с".into());
                    w.set_minimized(false);
                    t.step = TestStep::Done;
                }
                _ => {}
            }
        }
        if self.is_minimized() {
            // Не крутить цикл вхолостую: проверять состояние 10 раз в секунду.
            if let Some(r) = self.renderer.as_mut() {
                r.collect_completed();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + std::time::Duration::from_millis(100),
            ));
            // Пауза не должна превратиться в догоняющие тики после разворота.
            self.last_frame = Instant::now();
        } else {
            event_loop.set_control_flow(ControlFlow::Poll);
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Дождаться GPU до освобождения ресурсов.
        self.renderer.take();
    }
}
