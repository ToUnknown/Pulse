//! Windows-only capture, downloaded OCR, and reserved shortcut handling.
use super::{pixels, protocol, session};
pub mod capture;
mod hotkeys;
mod ocr;
mod ocr_models;
mod ocr_recognizer;
mod selector_window;
mod shortcut_keys;
mod shortcut_worker;
pub use ocr_models::LocalOcr;
pub(crate) use shortcut_worker::run_if_requested as run_shortcut_worker_if_requested;

use serde_json::{json, Value};
use tauri::{PhysicalPosition, PhysicalSize, WebviewWindow};
use tauri_plugin_global_shortcut::Shortcut;

pub const SHORTCUT_HINT: &str = "Include Ctrl, Alt, or Windows in the shortcut.";

pub use hotkeys::{
    available as shortcuts_available, capture_closed, install as install_shortcuts, recording,
};

pub fn configure_shortcuts(enabled: bool, keys: [Shortcut; 2]) {
    use shortcut_keys::{Action, Bindings};
    let action = |key| {
        if keys[0] == key {
            Some(Action::Editor)
        } else if keys[1] == key {
            Some(Action::QuickCopy)
        } else {
            None
        }
    };
    hotkeys::configure(
        enabled,
        Bindings {
            plain: action(protocol::DEFAULT_SHORTCUT.parse::<Shortcut>().unwrap()),
            control: action(protocol::QUICK_SHORTCUT.parse::<Shortcut>().unwrap()),
        },
    );
}

pub fn owns_shortcut(key: &Shortcut) -> bool {
    *key == protocol::DEFAULT_SHORTCUT.parse::<Shortcut>().unwrap()
        || *key == protocol::QUICK_SHORTCUT.parse::<Shortcut>().unwrap()
}

pub fn capture_access() -> Value {
    json!({"supported": true, "granted": true})
}
pub fn ensure_supported() -> Result<(), String> {
    Ok(())
}
pub fn ensure_capture_access() -> Result<(), String> {
    Ok(())
}
pub async fn request_capture_access(_: &tauri::AppHandle) -> Result<(), String> {
    Ok(())
}

pub async fn monitor_at_pointer(_: &tauri::AppHandle) -> Result<capture::Monitor, String> {
    capture::monitor_at_pointer()
}

pub async fn position_selector(
    window: &WebviewWindow,
    monitor: capture::Monitor,
) -> Result<(), String> {
    window
        .set_position(PhysicalPosition::new(monitor.x, monitor.y))
        .and_then(|_| window.set_size(PhysicalSize::new(monitor.width, monitor.height)))
        .map_err(|_| "Could not position Text Extractor.".into())
}

pub fn position_notice(window: &WebviewWindow, monitor: capture::Monitor) -> Result<(), String> {
    let scale = window
        .available_monitors()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|display| display.position().x == monitor.x && display.position().y == monitor.y)
        .map(|display| display.scale_factor())
        .unwrap_or(1.0);
    let width = (240.0 * scale).round() as u32;
    let height = (88.0 * scale).round() as u32;
    window
        .set_position(PhysicalPosition::new(
            monitor.x + (monitor.width as i32 - width as i32) / 2,
            monitor.y + (16.0 * scale).round() as i32,
        ))
        .and_then(|_| window.set_size(PhysicalSize::new(width, height)))
        .map_err(|error| error.to_string())
}

pub async fn configure_selector(window: &WebviewWindow) -> Result<(), String> {
    selector_window::configure(window).await?;
    let hwnd = window
        .hwnd()
        .map_err(|_| "Could not prepare the selector window.")?
        .0;
    unsafe {
        use windows_sys::Win32::{
            Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED},
            UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE},
        };
        let disabled: i32 = 1;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED as u32,
            (&disabled as *const i32).cast(),
            std::mem::size_of::<i32>() as u32,
        );
        if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) == 0 {
            return Err("Windows could not exclude the selector from screen capture.".into());
        }
    }
    Ok(())
}
