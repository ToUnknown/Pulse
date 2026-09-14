//! Native macOS integration. AppKit stays on the main thread; capture and Vision
//! run on background workers. No downloaded weights or Windows hook are linked.
pub mod capture;
mod native;
mod ocr;
pub use ocr::LocalOcr;
use serde_json::{json, Value};
use tauri::{Manager, WebviewWindow};
use tauri_plugin_global_shortcut::Shortcut;

pub const SHORTCUT_HINT: &str = "Include Control, Option, or Command in the shortcut.";

// macOS uses the shared global-shortcut registrar for all bindings. It needs no
// Windows-style reserved-shortcut helper or Accessibility keyboard hook.
pub fn install_shortcuts(_: &tauri::AppHandle) -> Result<(), String> {
    Ok(())
}
pub fn configure_shortcuts(_: bool, _: [Shortcut; 2]) {}
pub fn recording(_: bool) {}
pub fn capture_closed() {}
pub fn shortcuts_available() -> bool {
    true
}
pub fn owns_shortcut(_: &Shortcut) -> bool {
    false
}

pub fn ensure_supported() -> Result<(), String> {
    if unsafe { native::pulse_capture_supported() } {
        Ok(())
    } else {
        Err("Text Extractor requires macOS 14 or later.".into())
    }
}
pub fn capture_access() -> Value {
    json!({"supported": unsafe { native::pulse_capture_supported() },
        "granted": unsafe { native::pulse_capture_allowed() }})
}
pub fn ensure_capture_access() -> Result<(), String> {
    ensure_supported()?;
    if unsafe { native::pulse_capture_allowed() } {
        Ok(())
    } else {
        Err("Allow Screen Recording for Pulse in Settings → Advanced, then try again.".into())
    }
}
pub async fn request_capture_access(app: &tauri::AppHandle) -> Result<(), String> {
    ensure_supported()?;
    native::on_main(app, || {
        if unsafe { native::pulse_request_capture_access() } { Ok(()) }
        else { Err("Enable Pulse in System Settings → Privacy & Security → Screen Recording. Restart Pulse if macOS asks.".into()) }
    }).await
}
pub async fn monitor_at_pointer(app: &tauri::AppHandle) -> Result<capture::Monitor, String> {
    native::on_main(app, || {
        let mut monitor = capture::Monitor::default();
        native::checked(|error| unsafe { native::pulse_monitor_at_pointer(&mut monitor, error) })?;
        Ok(monitor)
    })
    .await
}
pub async fn configure_selector(window: &WebviewWindow) -> Result<(), String> {
    let target = window.clone();
    native::on_main(window.app_handle(), move || {
        let pointer = target.ns_window().map_err(|error| error.to_string())?;
        unsafe {
            native::pulse_configure_overlay(pointer);
        }
        Ok(())
    })
    .await
}
pub async fn position_selector(
    window: &WebviewWindow,
    monitor: capture::Monitor,
) -> Result<(), String> {
    let target = window.clone();
    native::on_main(window.app_handle(), move || {
        let pointer = target.ns_window().map_err(|error| error.to_string())?;
        native::checked(|error| unsafe {
            native::pulse_position_overlay(pointer, monitor, false, error)
        })
    })
    .await
}
pub fn position_notice(window: &WebviewWindow, monitor: capture::Monitor) -> Result<(), String> {
    // on_page_load may run off the main thread. Queue AppKit positioning before
    // Tauri's subsequent show operation, without blocking its event thread.
    let target = window.clone();
    window
        .run_on_main_thread(move || {
            let result = (|| {
                let pointer = target.ns_window().map_err(|error| error.to_string())?;
                unsafe {
                    native::pulse_configure_overlay(pointer);
                }
                native::checked(|error| unsafe {
                    native::pulse_position_overlay(pointer, monitor, true, error)
                })
            })();
            if let Err(error) = result {
                eprintln!("Quick Copy notice: {error}");
                let _ = target.destroy();
            }
        })
        .map_err(|error| error.to_string())
}
