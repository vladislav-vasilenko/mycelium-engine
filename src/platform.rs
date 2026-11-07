//! Платформенная граница: окно winit и поверхность вывода `CAMetalLayer`.
//! Нативный дескриптор окна используется только здесь, при создании бэкенда.

use objc2::rc::Retained;
use std::sync::atomic::{AtomicBool, Ordering};

use objc2_app_kit::{NSCursor, NSView};
use objc2_quartz_core::CAMetalLayer;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{CursorGrabMode, Window};

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

/// Курсор скрыт через `NSCursor hide`. Флаг держит вызовы `hide`/`unhide`
/// сбалансированными: AppKit считает их счётчиком.
static CURSOR_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Захват мыши для камеры от первого лица: курсор привязан к окну и скрыт.
///
/// Курсор скрывается через `NSCursor hide`, а не `Window::set_cursor_visible`.
/// winit 0.30 для невидимого курсора декодирует встроенную GIF-картинку, и на
/// macOS 27.0.1 этот разбор в ImageIO падал с `EXC_ARM_DA_ALIGN` (SIGBUS)
/// при клике в окно. См. AD-25 в architecture/00-arch.md.
pub fn grab_cursor(window: &Window, grab: bool) {
    if grab {
        if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
            let _ = window.set_cursor_grab(CursorGrabMode::Confined);
        }
        if !CURSOR_HIDDEN.swap(true, Ordering::Relaxed) {
            NSCursor::hide();
        }
    } else {
        let _ = window.set_cursor_grab(CursorGrabMode::None);
        if CURSOR_HIDDEN.swap(false, Ordering::Relaxed) {
            NSCursor::unhide();
        }
    }
}
