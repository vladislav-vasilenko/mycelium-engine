//! Руки от первого лица (этап 10): загрузка `.glb`, скелет и позы, палитра
//! костей и движение.
//!
//! - `asset` — загрузчик glTF: сетка `SkinnedVertex`, скелет, позы, текстура;
//! - `skeleton` — `Trs`, `Joint`, `Skeleton`, `Pose`, палитра и CPU-эталон скиннинга;
//! - `rig` — `HandRig`: смена поз, покачивание, отставание, проседание.
//!
//! Пространство рук — пространство глаза: начало в глазу, взгляд −Z, вверх +Y.

mod asset;
mod rig;
mod skeleton;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use glam::Mat4;

pub use asset::{HandsAsset, load};
pub use rig::{HandRig, POSE_NAMES, RigInput};
pub use skeleton::skin_position;

/// Ассет рук в репозитории.
pub fn asset_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/models/fps-arms.glb")
}

/// Угол обзора рук по вертикали и ближняя плоскость: руки не обрезаются ближней
/// плоскостью мира (5 см) и не зависят от FOV мира.
pub const VIEWMODEL_FOV_DEG: f32 = 60.0;
pub const VIEWMODEL_NEAR_M: f32 = 0.01;

/// Проекция рук, reversed-Z.
pub fn projection(aspect: f32) -> Mat4 {
    Mat4::perspective_infinite_reverse_rh(VIEWMODEL_FOV_DEG.to_radians(), aspect, VIEWMODEL_NEAR_M)
}

/// Контрольная поза самопроверки: `point` без движения.
pub const CONTROL_POSE: &str = "point";

/// Руки в приложении: ассет (сетка и позы) и аниматор.
pub struct Hands {
    pub asset: HandsAsset,
    pub rig: HandRig,
}

impl Hands {
    /// Загружает ассет; позы `POSE_NAMES` обязательны.
    pub fn load() -> Result<Hands, String> {
        let asset = load(&asset_path())?;
        for j in ["hand.L", "hand.R"] {
            asset
                .skeleton
                .joint(j)
                .ok_or(format!("в скелете нет кости {j}"))?;
        }
        let poses = POSE_NAMES
            .iter()
            .map(|n| asset.pose(n).cloned().ok_or(format!("нет позы {n}")))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Hands {
            rig: HandRig::new(poses),
            asset,
        })
    }

    /// Палитра кадра по текущей позе и движению.
    pub fn palette(&self, inp: &RigInput) -> Vec<Mat4> {
        self.asset
            .skeleton
            .palette(&self.rig.pose(), self.rig.root(inp))
    }

    /// Палитра контрольной позы без движения (самопроверка).
    pub fn control_palette(&self) -> Vec<Mat4> {
        let pose = self.asset.pose(CONTROL_POSE).expect("контрольная поза");
        self.asset.skeleton.palette(pose, Mat4::IDENTITY)
    }
}
