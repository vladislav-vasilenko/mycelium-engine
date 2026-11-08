//! Ресурсы Metal: создание буферов и текстур, запись в shared-память,
//! одноразовая загрузка текстур и отложенное освобождение.

use std::any::Any;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::*;

use super::shaders::Obj;
use crate::frame_graph::Stages;

pub(super) fn label(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

pub(super) fn to_mtl_stages(s: Stages) -> MTLStages {
    let mut out = MTLStages(0);
    if s.contains(Stages::VERTEX) {
        out |= MTLStages::Vertex;
    }
    if s.contains(Stages::FRAGMENT) {
        out |= MTLStages::Fragment;
    }
    if s.contains(Stages::DISPATCH) {
        out |= MTLStages::Dispatch;
    }
    if s.contains(Stages::BLIT) {
        out |= MTLStages::Blit;
    }
    out
}

pub(super) fn as_allocation<T: ?Sized + MTLAllocation>(
    obj: &ProtocolObject<T>,
) -> &ProtocolObject<dyn MTLAllocation>
where
    ProtocolObject<T>: MTLAllocation,
{
    ProtocolObject::from_ref(obj)
}

/// IEEE 754 half → f32 (формат HDR-буфера `RGBA16Float`).
pub(super) fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let frac = (h & 0x3ff) as f32;
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 => {
            if frac == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

pub(super) fn shared_buffer(
    device: &ProtocolObject<dyn MTLDevice>,
    len: usize,
    name: &str,
) -> Obj<dyn MTLBuffer> {
    let buf = device
        .newBufferWithLength_options(len, MTLResourceOptions::StorageModeShared)
        .expect("не удалось создать буфер");
    buf.setLabel(Some(&label(name)));
    buf
}

/// Shared-буфер с содержимым `bytes`.
pub(super) fn buffer_with(
    device: &ProtocolObject<dyn MTLDevice>,
    bytes: &[u8],
    name: &str,
) -> Obj<dyn MTLBuffer> {
    let buf = shared_buffer(device, bytes.len(), name);
    upload_bytes(&buf, 0, bytes);
    buf
}

pub(super) fn upload_bytes(buf: &ProtocolObject<dyn MTLBuffer>, offset: usize, bytes: &[u8]) {
    assert!(
        offset + bytes.len() <= buf.length(),
        "запись за пределы буфера"
    );
    // SAFETY: буфер в shared-памяти, диапазон проверен выше. Слот кадра
    // переиспользуется только после сигнала завершения его кадра на GPU.
    unsafe {
        let dst = buf.contents().as_ptr().cast::<u8>().add(offset);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
    }
}

/// Квадратная текстура (или массив при `layers > 1`) для чтения в шейдере.
pub(super) fn sampled_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    format: MTLPixelFormat,
    size: u32,
    layers: usize,
    mips: bool,
    name: &str,
) -> Result<Obj<dyn MTLTexture>, String> {
    sampled_texture_usage(
        device,
        format,
        size,
        layers,
        mips,
        MTLTextureUsage::ShaderRead,
        name,
    )
}

pub(super) fn sampled_texture_usage(
    device: &ProtocolObject<dyn MTLDevice>,
    format: MTLPixelFormat,
    size: u32,
    layers: usize,
    mips: bool,
    usage: MTLTextureUsage,
    name: &str,
) -> Result<Obj<dyn MTLTexture>, String> {
    let desc = MTLTextureDescriptor::new();
    desc.setTextureType(if layers > 1 {
        MTLTextureType::Type2DArray
    } else {
        MTLTextureType::Type2D
    });
    desc.setPixelFormat(format);
    unsafe {
        desc.setWidth(size as usize);
        desc.setHeight(size as usize);
        desc.setArrayLength(layers);
        desc.setMipmapLevelCount(if mips {
            (size.max(1).ilog2() + 1) as usize
        } else {
            1
        });
    }
    desc.setUsage(usage);
    desc.setStorageMode(MTLStorageMode::Private);
    let tex = device
        .newTextureWithDescriptor(&desc)
        .ok_or_else(|| format!("текстура {name}"))?;
    tex.setLabel(Some(&label(name)));
    Ok(tex)
}

pub(super) fn private_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    format: MTLPixelFormat,
    width: u32,
    height: u32,
    usage: MTLTextureUsage,
    name: &str,
) -> Obj<dyn MTLTexture> {
    let desc = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format,
            width as usize,
            height as usize,
            false,
        )
    };
    desc.setUsage(usage);
    desc.setStorageMode(MTLStorageMode::Private);
    let tex = device
        .newTextureWithDescriptor(&desc)
        .expect("не удалось создать текстуру");
    tex.setLabel(Some(&label(name)));
    tex
}

/// Элемент одноразовой загрузки текстуры.
pub(super) struct TextureUpload<'a> {
    pub texture: &'a Obj<dyn MTLTexture>,
    /// Слой за слоем, строки без выравнивания.
    pub data: &'a [u8],
    pub pixel_bytes: usize,
    pub mips: bool,
}

/// Одноразовая загрузка текстур: общий staging-буфер, копия каждого слоя
/// массива в mip 0, барьер blit → blit, генерация mip, ожидание завершения.
/// Текстуры добавляются в набор резидентности.
pub(super) fn upload_textures(
    device: &ProtocolObject<dyn MTLDevice>,
    queue: &ProtocolObject<dyn MTL4CommandQueue>,
    residency: &ProtocolObject<dyn MTLResidencySet>,
    uploads: &[TextureUpload],
) -> Result<(), String> {
    let total: usize = uploads.iter().map(|u| u.data.len()).sum();
    let staging = shared_buffer(device, total, "texture upload staging");
    let mut offsets = Vec::with_capacity(uploads.len());
    let mut at = 0;
    for u in uploads {
        upload_bytes(&staging, at, u.data);
        offsets.push(at);
        at += u.data.len();
        residency.addAllocation(as_allocation(&**u.texture));
    }
    residency.addAllocation(as_allocation(&*staging));
    residency.commit();

    let allocator = device.newCommandAllocator().ok_or("upload allocator")?;
    let cmd = device.newCommandBuffer().ok_or("upload command buffer")?;
    cmd.setLabel(Some(&label("texture upload")));
    cmd.beginCommandBufferWithAllocator(&allocator);
    let enc = cmd.computeCommandEncoder().ok_or("upload encoder")?;
    enc.setLabel(Some(&label("texture upload + mips")));
    for (u, &offset) in uploads.iter().zip(&offsets) {
        let (w, h) = (u.texture.width(), u.texture.height());
        let slices = u.texture.arrayLength();
        let row = w * u.pixel_bytes;
        let image = row * h;
        assert_eq!(u.data.len(), image * slices, "размер данных текстуры");
        for slice in 0..slices {
            unsafe {
                enc.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    &staging,
                    offset + slice * image,
                    row,
                    image,
                    MTLSize { width: w, height: h, depth: 1 },
                    u.texture,
                    slice,
                    0,
                    MTLOrigin { x: 0, y: 0, z: 0 },
                );
            }
        }
    }
    enc.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
        MTLStages::Blit,
        MTLStages::Blit,
        MTL4VisibilityOptions::Device,
    );
    for u in uploads.iter().filter(|u| u.mips) {
        unsafe { enc.generateMipmapsForTexture(u.texture) };
    }
    enc.endEncoding();
    cmd.endCommandBuffer();
    let done = device.newSharedEvent().ok_or("upload event")?;
    let mut cmd_ptr = NonNull::from(&*cmd);
    unsafe { queue.commit_count(NonNull::from(&mut cmd_ptr), 1) };
    let done_event: &ProtocolObject<dyn MTLEvent> = ProtocolObject::from_ref(&*done);
    queue.signalEvent_value(done_event, 1);
    if !done.waitUntilSignaledValue_timeoutMS(1, 10_000) {
        return Err("загрузка текстур не завершилась за 10 с".into());
    }
    residency.removeAllocation(as_allocation(&*staging));
    residency.commit();
    Ok(())
}

/// Ресурс, который нельзя освобождать до завершения кадров, где он использовался.
struct Retired {
    after_value: u64,
    textures: Vec<Obj<dyn MTLTexture>>,
    /// Держит объект (например, старые pipeline) живым до завершения кадров.
    _keep_alive: Option<Box<dyn Any>>,
}

/// Очередь отложенного освобождения: ресурс живёт, пока GPU не закончит все
/// кадры, отправленные до его замены.
#[derive(Default)]
pub(super) struct DeferredRelease {
    pending: Vec<Retired>,
}

impl DeferredRelease {
    /// Ставит ресурсы в очередь до сигнала кадра `after_value`.
    pub fn push(
        &mut self,
        after_value: u64,
        textures: Vec<Obj<dyn MTLTexture>>,
        other: Option<Box<dyn Any>>,
    ) {
        self.pending.push(Retired {
            after_value,
            textures,
            _keep_alive: other,
        });
    }

    /// Освобождает всё, что завершено к сигналу `done`; текстуры убираются из
    /// набора резидентности. Возвращает число освобождённых записей.
    pub fn collect(&mut self, done: u64, residency: &ProtocolObject<dyn MTLResidencySet>) -> u64 {
        let mut released = 0;
        self.pending.retain(|r| {
            if r.after_value > done {
                return true;
            }
            for t in &r.textures {
                residency.removeAllocation(as_allocation(&**t));
            }
            if !r.textures.is_empty() {
                residency.commit();
            }
            released += 1;
            false
        });
        released
    }
}
