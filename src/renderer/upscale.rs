//! Апскейл MetalFX (04.6, сессия 1): сцена в разрешении рендера → выходной
//! HDR временным апскейлером `MTL4FXTemporalScaler` (вместо TAA при масштабе
//! < 1). Вход — HDR сцены с объёмным светом, глубина reversed-Z и векторы
//! движения к прошлому кадру в uv (ядро `upscale_prep`).

use objc2::runtime::ProtocolObject;
use objc2_metal::*;
use objc2_metal_fx::*;

use super::shaders::{self, Obj};
use super::targets::Targets;
use crate::layout::TaaUniforms;

/// Апскейлер под размеры целей; `None` — устройство не поддерживает MetalFX
/// для Metal 4 или создание не удалось.
pub(super) fn new_scaler(
    device: &ProtocolObject<dyn MTLDevice>,
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    t: &Targets,
) -> Option<Obj<dyn MTL4FXTemporalScaler>> {
    unsafe {
        if !MTLFXTemporalScalerDescriptor::supportsMetal4FX(device) {
            return None;
        }
        let d = MTLFXTemporalScalerDescriptor::new();
        d.setColorTextureFormat(shaders::HDR_FORMAT);
        d.setDepthTextureFormat(shaders::DEPTH_FORMAT);
        d.setMotionTextureFormat(MTLPixelFormat::RG16Float);
        d.setOutputTextureFormat(shaders::HDR_FORMAT);
        d.setInputWidth(t.rw as usize);
        d.setInputHeight(t.rh as usize);
        d.setOutputWidth(t.width as usize);
        d.setOutputHeight(t.height as usize);
        // Экспозиция кадра — автоэкспозиция по гистограмме уже в HDR
        // (предэкспозиция); MetalFX оценивает яркость сам.
        d.setAutoExposureEnabled(true);
        d.newTemporalScalerWithDevice_compiler(device, compiler)
    }
}

/// Записывает апскейл кадра. Дрожание — в пикселях рендера, как в FSR: сдвиг
/// картинки проекцией (x вправо, y вниз; `TaaUniforms.jitter.xy` — сдвиг в
/// NDC, y вверх). С обратным знаком (сдвиг сэмпла) текстура вблизи
/// подрагивала: кадры совмещались со сдвигом на доли пикселя.
pub(super) fn encode(
    scaler: &ProtocolObject<dyn MTL4FXTemporalScaler>,
    cmd: &ProtocolObject<dyn MTL4CommandBuffer>,
    t: &Targets,
    taa: &TaaUniforms,
) {
    unsafe {
        scaler.setColorTexture(Some(&t.scene));
        scaler.setDepthTexture(Some(&t.scene_depth));
        scaler.setMotionTexture(Some(&t.motion));
        scaler.setOutputTexture(Some(&t.hdr));
        scaler.setInputContentWidth(t.rw as usize);
        scaler.setInputContentHeight(t.rh as usize);
        scaler.setJitterOffsetX(taa.jitter[0] * t.rw as f32 * 0.5);
        scaler.setJitterOffsetY(-taa.jitter[1] * t.rh as f32 * 0.5);
        // Векторы движения в uv → пиксели рендера.
        scaler.setMotionVectorScaleX(t.rw as f32);
        scaler.setMotionVectorScaleY(t.rh as f32);
        scaler.setDepthReversed(true);
        scaler.setReset(taa.cam_delta[3] < 0.5);
        scaler.encodeToCommandBuffer(cmd);
    }
}
