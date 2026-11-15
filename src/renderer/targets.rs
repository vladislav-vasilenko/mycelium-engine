//! Цели рендера: HDR-буфер сцены и глубина reversed-Z.

use objc2::runtime::ProtocolObject;
use objc2_metal::*;

use super::resources::private_texture;
use super::shaders::{self, Obj};

pub(super) struct Targets {
    pub width: u32,
    pub height: u32,
    pub hdr: Obj<dyn MTLTexture>,
    pub depth: Obj<dyn MTLTexture>,
    /// История временного сглаживания (04.5): кадр пишет одну, читает другую.
    pub history: [Obj<dyn MTLTexture>; 2],
    /// Bloom (04.5): цепочка вниз от половины кадра (`BLOOM_LEVELS` уровней)
    /// и цепочка вверх (на уровень меньше); итог — `bloom_up[0]`.
    pub bloom_down: Vec<Obj<dyn MTLTexture>>,
    pub bloom_up: Vec<Obj<dyn MTLTexture>>,
    /// Затенение лучами (11): половина кадра, R32Float, копится во времени.
    pub rt_ao: Obj<dyn MTLTexture>,
}

/// Уровней цепочки bloom: от 1/2 до 1/64 кадра.
pub const BLOOM_LEVELS: usize = 6;

impl Targets {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>, width: u32, height: u32) -> Targets {
        let hdr = private_texture(
            device,
            shaders::HDR_FORMAT,
            width,
            height,
            MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead,
            "HDR scene",
        );
        let depth = private_texture(
            device,
            shaders::DEPTH_FORMAT,
            width,
            height,
            MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead,
            "depth (reversed-Z)",
        );
        let history = [0, 1].map(|k| {
            private_texture(
                device,
                shaders::HDR_FORMAT,
                width,
                height,
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
            (width / 2).max(1),
            (height / 2).max(1),
            MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite,
            "rt ambient occlusion",
        );
        Targets {
            width,
            height,
            hdr,
            depth,
            history,
            bloom_down,
            bloom_up,
            rt_ao,
        }
    }

    /// Все текстуры целей (для резидентности и освобождения).
    pub fn textures(&self) -> Vec<Obj<dyn MTLTexture>> {
        let mut v = vec![self.hdr.clone(), self.depth.clone()];
        v.extend(self.history.iter().cloned());
        v.extend(self.bloom_down.iter().cloned());
        v.extend(self.bloom_up.iter().cloned());
        v.push(self.rt_ao.clone());
        v
    }
}
