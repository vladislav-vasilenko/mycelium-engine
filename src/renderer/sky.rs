//! Небо на GPU (04.1): LUT атмосферы (пропускание, многократное рассеяние,
//! вид неба), свет неба для освещения и состояние экспозиции.

use std::sync::atomic::AtomicBool;

use objc2::runtime::ProtocolObject;
use objc2_metal::*;

use super::resources::{as_allocation, private_texture, shared_buffer};
use super::shaders::Obj;

pub const TRANSMITTANCE_SIZE: (u32, u32) = (256, 64);
pub const MULTISCATTER_SIZE: (u32, u32) = (32, 32);
pub const SKYVIEW_SIZE: (u32, u32) = (192, 108);
/// `SkyFrame` в `sky.h`: 12 × float4.
pub const SKY_FRAME_BYTES: usize = 12 * 16;
/// Потоков в группе ядра `sky_ambient` (`AMBIENT_THREADS` в `sky.metal`).
pub const AMBIENT_THREADS: usize = 64;

pub(super) struct SkyGpu {
    pub transmittance: Obj<dyn MTLTexture>,
    pub multiscatter: Obj<dyn MTLTexture>,
    pub skyview: Obj<dyn MTLTexture>,
    /// Вид неба, освещённого луной (та же атмосфера, другое светило).
    pub skyview_moon: Obj<dyn MTLTexture>,
    /// Свет неба (SH), главное светило и экспозиция кадра — пишет `sky_ambient`.
    pub frame: Obj<dyn MTLBuffer>,
    /// Экспозиция между кадрами (адаптация глаза).
    pub state: Obj<dyn MTLBuffer>,
    /// LUT пропускания и многократного рассеяния посчитаны (они не зависят от кадра).
    pub static_ready: AtomicBool,
    /// Звёзды каталога BSC5 (по 20 байт) и их число; без каталога — нет звёзд.
    pub stars: Option<(Obj<dyn MTLBuffer>, usize)>,
}

impl SkyGpu {
    pub fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        residency: &ProtocolObject<dyn MTLResidencySet>,
    ) -> SkyGpu {
        let rw = MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite;
        let tex = |(w, h): (u32, u32), name: &str| {
            private_texture(device, MTLPixelFormat::RGBA16Float, w, h, rw, name)
        };
        let s = SkyGpu {
            transmittance: tex(TRANSMITTANCE_SIZE, "sky: transmittance LUT"),
            multiscatter: tex(MULTISCATTER_SIZE, "sky: multiple scattering LUT"),
            skyview: tex(SKYVIEW_SIZE, "sky: sky-view LUT"),
            skyview_moon: tex(SKYVIEW_SIZE, "sky: sky-view LUT (moon)"),
            frame: shared_buffer(device, SKY_FRAME_BYTES, "sky: frame (SH, light, exposure)"),
            state: shared_buffer(device, 16, "sky: exposure state"),
            static_ready: AtomicBool::new(false),
            stars: None,
        };
        residency.addAllocation(as_allocation(&*s.transmittance));
        residency.addAllocation(as_allocation(&*s.multiscatter));
        residency.addAllocation(as_allocation(&*s.skyview));
        residency.addAllocation(as_allocation(&*s.skyview_moon));
        residency.addAllocation(as_allocation(&*s.frame));
        residency.addAllocation(as_allocation(&*s.state));
        s
    }
}

impl super::Renderer {
    /// Звёзды неба: little-endian f32 × 5 на звезду (`assets/sky/stars.bin`).
    pub fn set_stars(&mut self, bytes: &[u8]) {
        let n = bytes.len() / 20;
        if n == 0 {
            return;
        }
        let buf =
            super::resources::buffer_with(&self.device, &bytes[..n * 20], "sky: stars (BSC5)");
        self.residency.addAllocation(as_allocation(&*buf));
        self.residency.commit();
        if let Some((old, _)) = self.sky.stars.replace((buf, n)) {
            self.retire(Vec::new(), Some(Box::new(old)));
        }
    }
}

impl super::Renderer {
    /// Экспозиция по последнему замеру GPU (общий буфер, без ожидания): основа
    /// предэкспозиции следующего кадра. 1 — ещё не мерили.
    pub fn last_exposure(&self) -> f32 {
        // SAFETY: буфер StorageModeShared на 16 байт; гонка с GPU даёт
        // значение прошлого или текущего кадра — оба подходят для предэкспозиции.
        let v = unsafe { *self.sky.state.contents().as_ptr().cast::<f32>() };
        if v.is_finite() && v > 0.0 { v } else { 1.0 }
    }
}
