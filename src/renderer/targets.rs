//! Цели рендера: HDR-буфер сцены и глубина reversed-Z.
//!
//! Апскейл (04.6): сцена (проход `scene`, лучевое затенение, объёмный свет)
//! рендерится в `rw × rh` — `scene`, `scene_depth`; MetalFX восстанавливает
//! кадр в выходной `hdr` (`width × height`), где рисуются руки, bloom и
//! тонмаппинг. При масштабе 1 `scene` и `scene_depth` — те же текстуры, что
//! `hdr` и `depth`.

use objc2::runtime::ProtocolObject;
use objc2_metal::*;

use super::resources::private_texture;
use super::shaders::{self, Obj};

pub(super) struct Targets {
    /// Выходной размер (drawable).
    pub width: u32,
    pub height: u32,
    /// Размер рендера сцены (04.6): `width × масштаб`.
    pub rw: u32,
    pub rh: u32,
    /// Выходной HDR и глубина (руки, bloom, тонмаппинг).
    pub hdr: Obj<dyn MTLTexture>,
    pub depth: Obj<dyn MTLTexture>,
    /// HDR и глубина сцены в разрешении рендера (при масштабе 1 — `hdr`, `depth`).
    pub scene: Obj<dyn MTLTexture>,
    pub scene_depth: Obj<dyn MTLTexture>,
    /// Векторы движения к прошлому кадру, uv (RG16Float, разрешение рендера).
    pub motion: Obj<dyn MTLTexture>,
    /// История временного сглаживания (04.5): кадр пишет одну, читает другую.
    pub history: [Obj<dyn MTLTexture>; 2],
    /// Bloom (04.5): цепочка вниз от половины кадра (`BLOOM_LEVELS` уровней)
    /// и цепочка вверх (на уровень меньше); итог — `bloom_up[0]`.
    pub bloom_down: Vec<Obj<dyn MTLTexture>>,
    pub bloom_up: Vec<Obj<dyn MTLTexture>>,
    /// Затенение лучами (11): половина кадра, R32Float, копится во времени.
    pub rt_ao: Obj<dyn MTLTexture>,
    /// Объёмный свет (11): четверть кадра, добавляется в проходе `taa`.
    pub vol: Obj<dyn MTLTexture>,
}

/// Уровней цепочки bloom: от 1/2 до 1/64 кадра.
pub const BLOOM_LEVELS: usize = 6;

impl Targets {
    pub fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Targets {
        let scaled = scale < 0.999;
        let rw = if scaled {
            ((width as f32 * scale).round() as u32).max(1)
        } else {
            width
        };
        let rh = if scaled {
            ((height as f32 * scale).round() as u32).max(1)
        } else {
            height
        };
        // Выход MetalFX пишется шейдером: ShaderWrite — только с апскейлом
        // (у цели рендера он может выключать сжатие кадра на GPU Apple).
        let out_usage = if scaled {
            MTLTextureUsage::RenderTarget
                | MTLTextureUsage::ShaderRead
                | MTLTextureUsage::ShaderWrite
        } else {
            MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead
        };
        let hdr = private_texture(
            device,
            shaders::HDR_FORMAT,
            width,
            height,
            out_usage,
            "HDR output",
        );
        let depth = private_texture(
            device,
            shaders::DEPTH_FORMAT,
            width,
            height,
            MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead,
            "depth (reversed-Z)",
        );
        let (scene, scene_depth) = if scaled {
            (
                private_texture(
                    device,
                    shaders::HDR_FORMAT,
                    rw,
                    rh,
                    MTLTextureUsage::RenderTarget
                        | MTLTextureUsage::ShaderRead
                        | MTLTextureUsage::ShaderWrite,
                    "HDR scene (render scale)",
                ),
                private_texture(
                    device,
                    shaders::DEPTH_FORMAT,
                    rw,
                    rh,
                    MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead,
                    "depth scene (render scale)",
                ),
            )
        } else {
            (hdr.clone(), depth.clone())
        };
        let motion = private_texture(
            device,
            MTLPixelFormat::RG16Float,
            rw,
            rh,
            MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite,
            "motion vectors",
        );
        let history = [0, 1].map(|k| {
            private_texture(
                device,
                shaders::HDR_FORMAT,
                rw,
                rh,
                MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite,
                &format!("TAA history {k}"),
            )
        });
        let level = |k: usize, kind: &str| {
            private_texture(
                device,
                shaders::HDR_FORMAT,
                (width >> (k + 1)).max(1),
                (height >> (k + 1)).max(1),
                MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite,
                &format!("bloom {kind} {k}"),
            )
        };
        let bloom_down = (0..BLOOM_LEVELS).map(|k| level(k, "down")).collect();
        let bloom_up = (0..BLOOM_LEVELS - 1).map(|k| level(k, "up")).collect();
        let rt_ao = private_texture(
            device,
            MTLPixelFormat::R32Float,
            (rw / 2).max(1),
            (rh / 2).max(1),
            MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite,
            "rt ambient occlusion",
        );
        let vol = private_texture(
            device,
            shaders::HDR_FORMAT,
            (rw / 4).max(1),
            (rh / 4).max(1),
            MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite,
            "volumetric light",
        );
        Targets {
            width,
            height,
            rw,
            rh,
            hdr,
            depth,
            scene,
            scene_depth,
            motion,
            history,
            bloom_down,
            bloom_up,
            rt_ao,
            vol,
        }
    }

    /// Сцена рендерится в уменьшенном разрешении (нужен апскейл).
    pub fn scaled(&self) -> bool {
        self.rw != self.width || self.rh != self.height
    }

    /// Все текстуры целей (для резидентности и освобождения).
    pub fn textures(&self) -> Vec<Obj<dyn MTLTexture>> {
        let mut v = vec![self.hdr.clone(), self.depth.clone(), self.motion.clone()];
        if self.scaled() {
            v.push(self.scene.clone());
            v.push(self.scene_depth.clone());
        }
        v.extend(self.history.iter().cloned());
        v.extend(self.bloom_down.iter().cloned());
        v.extend(self.bloom_up.iter().cloned());
        v.push(self.rt_ao.clone());
        v.push(self.vol.clone());
        v
    }
}
