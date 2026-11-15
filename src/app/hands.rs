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
        let h = self.hands.as_mut()?;
        if let Some(i) = start_pose {
            h.rig.set_pose(i);
        }
        if let Some(i) = start_prop.filter(|&i| i < h.props.len()) {
            h.prop = Some(i);
        }
        let palette = if selftest {
            h.control_palette()
        } else {
            if !self.hands_visible || self.sim.mode != MoveMode::Walk {
                return None;
            }
            // Поза рук от игры (05.2): ладони к земле в чувстве мицелия,
            // указание при макро-осмотре; после — обратно «relaxed».
            let (pose, offset) = self.game.hands_hint();
            match pose {
                Some(p) => {
                    h.rig.set_pose(p);
                    self.game.hands_override = true;
                }
                None => {
                    // С предметом — хват (grip), без — расслабленно.
                    let want = if h.prop.is_some() { 1 } else { 0 };
                    if self.game.hands_override || h.rig.target() != want {
                        h.rig.set_pose(want);
                    }
                    self.game.hands_override = false;
                }
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
            prop: if selftest { None } else { h.prop },
        })
    }
}
