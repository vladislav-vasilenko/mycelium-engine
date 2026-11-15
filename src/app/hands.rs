//! Руки в кадре: аниматор по камере и шагу, палитра костей и параметры прохода
//! `viewmodel`. В самопроверке — контрольная поза без движения.

use glam::{Mat4, Vec4};

use super::{App, SceneMode};
use crate::hands::{self, RigInput};
use crate::layout::HandsUniforms;
use crate::renderer::HandsFrame;
use crate::sim::{CameraState, MoveMode};

impl App {
    /// Руки видны в ходьбе по ландшафту (клавиша H прячет их).
    pub(super) fn hands_frame(&mut self, camera: &CameraState, aspect: f32) -> Option<HandsFrame> {
        if self.scene != SceneMode::Terrain {
            return None;
        }
        let selftest = self.selftest.is_some();
        let start_pose = self.args.pose.take();
        let start_prop = self.args.prop.take();
        // Проверка: действие повторяется всё время (копание — по кругу).
        let start_act = self.args.act.clone();
        let h = self.hands.as_mut()?;
        if let Some(i) = start_pose {
            h.rig.set_pose(i);
        }
        if let Some(i) = start_prop.filter(|&i| i < h.props.len()) {
            h.prop = Some(i);
            h.shown = Some(i);
        }
        match start_act.as_deref() {
            Some("talk") => h.rig.act(hands::Action::Talk),
            Some("dig") => h.rig.act(hands::Action::Dig),
            Some("aim") => h.rig.act(hands::Action::Aim),
            _ => {}
        }
        let palette = if selftest {
            h.control_palette()
        } else {
            if !self.hands_visible || self.sim.mode != MoveMode::Walk {
                return None;
            }
            // Поза рук от игры (05.2): ладони к земле в чувстве мицелия,
            // указание при макро-осмотре; после — обратно «relaxed».
            let (mut pose, mut offset) = self.game.hands_hint();
            // Соединение с мицелием (05.6): рука тянется к шляпке гриба.
            if self.escape.as_ref().is_some_and(|e| e.reaching()) {
                pose = Some(3);
                offset = glam::Vec3::new(0.0, -0.12, -0.05);
            }
            match pose {
                Some(p) => {
                    h.rig.set_pose(p);
                    self.game.hands_override = true;
                }
                None => {
                    // С предметом — хват (grip), без — расслабленно.
                    let want = if h.shown.is_some() { 1 } else { 0 };
                    if self.game.hands_override || h.rig.target() != want {
                        h.rig.set_pose(want);
                    }
                    self.game.hands_override = false;
                }
            }
            // Смена предмета: руки уходят вниз, предмет меняется вне кадра.
            if h.prop != h.shown && !h.rig.swapping() {
                h.rig.start_swap();
            }
            if h.rig.take_swap_mid() {
                h.shown = h.prop;
            }
            let (bob_phase, bob_weight) = self.sim.bob();
            let inp = RigInput {
                dt: self.frame_dt,
                yaw: camera.yaw,
                pitch: camera.pitch,
                bob_phase,
                bob_weight,
                view_dy: self.sim.view_dy,
                offset,
            };
            h.rig.update(&inp);
            h.palette(&inp)
        };
        // Поворот глаз → мир: обратная матрица вида без сдвига.
        let mut view_to_world = camera.view().inverse();
        view_to_world.w_axis = Vec4::W;
        Some(HandsFrame {
            uniforms: HandsUniforms {
                proj: hands::projection(aspect).to_cols_array_2d(),
                view_to_world: view_to_world.to_cols_array_2d(),
                vertex_count: h.asset.vertices.len() as u32,
                joint_count: palette.len() as u32,
                _pad: [0; 2],
            },
            palette: palette.iter().map(Mat4::to_cols_array_2d).collect(),
            prop: if selftest { None } else { h.shown },
        })
    }

    /// Левая кнопка: действие предметом в правой руке (10.1). Отпускание
    /// возвращает удерживаемое (рация, фонарь).
    pub(super) fn use_item(&mut self, pressed: bool) {
        let Some(h) = self.hands.as_mut() else { return };
        if !pressed {
            h.rig.release();
            return;
        }
        let kind = match h
            .shown
            .and_then(|i| h.props.get(i))
            .map(|p| p.name.as_str())
        {
            Some("trowel_01") => hands::Action::Dig,
            Some("vintage_radio_transceiver") => hands::Action::Talk,
            Some("vintage_flashlight") => hands::Action::Aim,
            _ => return,
        };
        h.rig.act(kind);
    }
}
