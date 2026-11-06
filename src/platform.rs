//! Платформенная граница: окно winit и поверхность вывода `CAMetalLayer`.
//! Нативный дескриптор окна используется только здесь, при создании бэкенда.

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_quartz_core::CAMetalLayer;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// Привязывает новый `CAMetalLayer` к `NSView` окна. Вызывается на главном потоке.
pub fn attach_metal_layer(window: &Window) -> Result<Retained<CAMetalLayer>, String> {
    let handle = window
        .window_handle()
        .map_err(|e| format!("window handle: {e}"))?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return Err("ожидалось окно AppKit".into());
    };
    // SAFETY: winit отдаёт живой NSView окна. Вызов идёт из обработчика
    // событий winit, то есть на главном потоке, как требует AppKit.
    let view: &NSView = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
    let layer = CAMetalLayer::new();
    view.setWantsLayer(true);
    view.setLayer(Some(&layer));
    layer.setContentsScale(window.scale_factor());
    Ok(layer)
}
