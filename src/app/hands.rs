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
        let h = self.hands.as_mut()?;
        if let Some(i) = start_pose {
            h.rig.set_pose(i);
        }
        let palette = if selftest {
            h.control_palette()
        } else {
            if !self.hands_visible || self.sim.mode != MoveMode::Walk {
                return None;
            }
            let (bob_phase, bob_weight) = self.sim.bob();
            let inp = RigInput {
                dt: self.frame_dt,
                yaw: camera.yaw,
                pitch: camera.pitch,
                bob_phase,
                bob_weight,
                view_dy: self.sim.view_dy,
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
        })
    }
}
