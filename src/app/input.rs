//! Ввод: оси движения из нажатых клавиш и команды по клавишам.

use winit::keyboard::KeyCode;

use super::{App, SceneMode};
use crate::sim::{MoveMode, TickInput};

impl App {
    pub(super) fn tick_input(&self) -> TickInput {
        let axis = |pos: KeyCode, neg: KeyCode| {
            (self.keys.contains(&pos) as i32 - self.keys.contains(&neg) as i32) as f32
        };
        TickInput {
            forward: axis(KeyCode::KeyW, KeyCode::KeyS),
            right: axis(KeyCode::KeyD, KeyCode::KeyA),
            up: (axis(KeyCode::KeyE, KeyCode::KeyQ)
                + self.keys.contains(&KeyCode::Space) as i32 as f32)
                .clamp(-1.0, 1.0),
            look_dx: 0.0,
            look_dy: 0.0,
            fast: self.keys.contains(&KeyCode::ShiftLeft)
                || self.keys.contains(&KeyCode::ShiftRight),
        }
    }

    /// Нажатие клавиши: оси движения копятся в `keys`, команды — без автоповтора.
    pub(super) fn on_key_pressed(&mut self, code: KeyCode, repeat: bool) {
        self.keys.insert(code);
        if repeat {
            return;
        }
        match code {
            KeyCode::Escape => self.set_grab(false),
            KeyCode::F5 => self.reload_shaders(),
            KeyCode::F9 => self.readback_requested = true,
            KeyCode::F12 => self.capture_requested = true,
            KeyCode::KeyG => {
                match self.sim.mode {
                    MoveMode::Walk => self.sim.set_mode(MoveMode::Fly, &self.world),
                    MoveMode::Fly => self.land(false),
                }
                self.update_title(true);
            }
            KeyCode::KeyR => {
                self.land(true);
                self.update_title(true);
            }
            KeyCode::KeyV => {
                self.view_mode = self.view_mode.next(self.modifiers.shift_key());
                self.update_title(true);
            }
            KeyCode::KeyB => {
                self.sim.head_bob = !self.sim.head_bob;
            }
            KeyCode::KeyH => {
                self.hands_visible = !self.hands_visible;
            }
            KeyCode::Digit1 | KeyCode::Digit2 | KeyCode::Digit3 | KeyCode::Digit4 => {
                let i = match code {
                    KeyCode::Digit1 => 0,
                    KeyCode::Digit2 => 1,
                    KeyCode::Digit3 => 2,
                    _ => 3,
                };
                if let Some(h) = self.hands.as_mut() {
                    h.rig.set_pose(i);
                }
            }
            KeyCode::KeyT => {
                self.scene = match self.scene {
                    SceneMode::Terrain => SceneMode::Cubes,
                    SceneMode::Cubes => SceneMode::Terrain,
                };
                self.update_title(true);
            }
            // Меню winit на macOS не содержит пункта Window → Minimize,
            // поэтому ⌘M обрабатываем сами.
            KeyCode::KeyM if self.modifiers.super_key() => {
                self.keys.clear();
                self.set_grab(false);
                if let Some(w) = &self.window {
                    w.set_minimized(true);
                }
            }
            _ => {}
        }
    }
}
