//! Устройство Metal: проверка возможностей и запись GPU capture.

use std::path::Path;

use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_foundation::{NSProcessInfo, NSString, NSURL};
use objc2_metal::*;

use super::shaders::Obj;

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub os: String,
    pub metal4: bool,
    pub apple_family: Option<u32>,
    /// `None` — не запрашивалось (режим GPU capture).
    pub raytracing: Option<bool>,
    pub raytracing_from_render: Option<bool>,
}

fn flag(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "да",
        Some(false) => "нет",
        None => "не запрошено (capture)",
    }
}

impl std::fmt::Display for DeviceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GPU: {} | ОС: {} | Metal 4: {} | семейство Apple{} | raytracing: {} | raytracing from render: {}",
            self.name,
            self.os,
            if self.metal4 { "да" } else { "нет" },
            self.apple_family.map_or("?".to_string(), |v| v.to_string()),
            flag(self.raytracing),
            flag(self.raytracing_from_render)
        )
    }
}

fn capture_enabled() -> bool {
    std::env::var_os("MTL_CAPTURE_ENABLED").is_some_and(|v| v == "1")
}

pub fn probe_device() -> Result<(Obj<dyn MTLDevice>, DeviceInfo), String> {
    let device = MTLCreateSystemDefaultDevice().ok_or("Metal-устройство не найдено")?;
    let families = [
        (10u32, MTLGPUFamily::Apple10),
        (9, MTLGPUFamily::Apple9),
        (8, MTLGPUFamily::Apple8),
        (7, MTLGPUFamily::Apple7),
    ];
    let apple_family = families
        .iter()
        .find(|(_, f)| device.supportsFamily(*f))
        .map(|(v, _)| *v);
    let info = DeviceInfo {
        name: device.name().to_string(),
        os: NSProcessInfo::processInfo()
            .operatingSystemVersionString()
            .to_string(),
        metal4: device.supportsFamily(MTLGPUFamily::Metal4),
        apple_family,
        // При GPU capture устройство обёрнуто в CaptureMTLDevice, который только
        // пересылает эти селекторы; проверка objc2 в Debug считает это ошибкой.
        // В режиме capture признаки трассировки не запрашиваем.
        raytracing: (!capture_enabled()).then(|| device.supportsRaytracing()),
        raytracing_from_render: (!capture_enabled()).then(|| device.supportsRaytracingFromRender()),
    };
    Ok((device, info))
}

/// Начинает запись `.gputrace` в `path`. `false` — запись недоступна или не началась.
pub(super) fn start_capture(device: &ProtocolObject<dyn MTLDevice>, path: &Path) -> bool {
    let manager = unsafe { MTLCaptureManager::sharedCaptureManager() };
    if !manager.supportsDestination(MTLCaptureDestination::GPUTraceDocument) {
        eprintln!("[capture] запись в .gputrace недоступна: запусти с MTL_CAPTURE_ENABLED=1");
        return false;
    }
    let desc = MTLCaptureDescriptor::new();
    let object: &AnyObject = device.as_ref();
    unsafe { desc.setCaptureObject(Some(object)) };
    desc.setDestination(MTLCaptureDestination::GPUTraceDocument);
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    desc.setOutputURL(Some(&url));
    match manager.startCaptureWithDescriptor_error(&desc) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("[capture] ошибка: {}", e.localizedDescription());
            false
        }
    }
}

/// Заканчивает запись, начатую `start_capture`.
pub(super) fn stop_capture(frame: u64) {
    unsafe { MTLCaptureManager::sharedCaptureManager() }.stopCapture();
    eprintln!("[capture] кадр {frame} записан");
}
