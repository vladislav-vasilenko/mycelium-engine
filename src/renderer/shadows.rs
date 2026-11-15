//! Каскадные тени на GPU (04.2): массив карт глубины и состояние глубины.

use objc2::runtime::ProtocolObject;
use objc2_metal::*;

use super::resources::{as_allocation, label};
use super::shaders::Obj;

/// Каскадов: три вокруг камеры и один на весь остров.
pub const CASCADES: usize = 4;
/// Сторона карты каскада, тексели.
pub const SHADOW_SIZE: u32 = 2048;

/// Карта теней кадра и диапазоны списков кластеров каскадов.
pub struct ShadowFrame {
    pub uniforms: crate::layout::ShadowUniforms,
    /// (начало, число) в общем списке видимых кластеров кадра.
    pub ranges: [(u32, u32); CASCADES],
}

pub(super) struct ShadowGpu {
    /// Depth32Float, массив из `CASCADES` слоёв.
    pub map: Obj<dyn MTLTexture>,
    /// Глубина [0, 1], ближе к свету — меньше: проверка «меньше или равно», запись.
    pub depth_state: Obj<dyn MTLDepthStencilState>,
    /// Карта укрытия от дождя (04.3): Depth32Float `RAIN_SIZE`², кроны сверху.
    pub rain: Obj<dyn MTLTexture>,
}

/// Сторона карты укрытия от дождя, текселей (128 м → 12,5 см на тексель).
pub const RAIN_SIZE: u32 = 1024;

impl ShadowGpu {
    pub fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        residency: &ProtocolObject<dyn MTLResidencySet>,
    ) -> Result<ShadowGpu, String> {
        let desc = MTLTextureDescriptor::new();
        desc.setTextureType(MTLTextureType::Type2DArray);
        desc.setPixelFormat(MTLPixelFormat::Depth32Float);
        unsafe {
            desc.setWidth(SHADOW_SIZE as usize);
            desc.setHeight(SHADOW_SIZE as usize);
            desc.setArrayLength(CASCADES);
        }
        desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        desc.setStorageMode(MTLStorageMode::Private);
        let map = device
            .newTextureWithDescriptor(&desc)
            .ok_or("карта теней")?;
        map.setLabel(Some(&label("shadow cascades (Depth32Float × 4)")));
        residency.addAllocation(as_allocation(&*map));
        let d = MTLDepthStencilDescriptor::new();
        d.setDepthCompareFunction(MTLCompareFunction::LessEqual);
        d.setDepthWriteEnabled(true);
        d.setLabel(Some(&label("shadow depth: less-equal, write")));
        let depth_state = device
            .newDepthStencilStateWithDescriptor(&d)
            .ok_or("состояние глубины теней")?;
        let rd = MTLTextureDescriptor::new();
        rd.setPixelFormat(MTLPixelFormat::Depth32Float);
        unsafe {
            rd.setWidth(RAIN_SIZE as usize);
            rd.setHeight(RAIN_SIZE as usize);
        }
        rd.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        rd.setStorageMode(MTLStorageMode::Private);
        let rain = device
            .newTextureWithDescriptor(&rd)
            .ok_or("карта укрытия от дождя")?;
        rain.setLabel(Some(&label("rain occlusion (Depth32Float)")));
        residency.addAllocation(as_allocation(&*rain));
        Ok(ShadowGpu {
            map,
            depth_state,
            rain,
        })
    }
}
