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
}

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
            MTLTextureUsage::RenderTarget,
            "depth (reversed-Z)",
        );
        Targets {
            width,
            height,
            hdr,
            depth,
        }
    }
}
