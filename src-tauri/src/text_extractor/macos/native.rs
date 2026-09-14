use super::capture::Monitor;
use std::ffi::{c_char, c_void, CStr};

extern "C" {
    pub fn pulse_capture_supported() -> bool;
    pub fn pulse_capture_allowed() -> bool;
    pub fn pulse_request_capture_access() -> bool;
    pub fn pulse_monitor_at_pointer(monitor: *mut Monitor, error: *mut *mut c_char) -> bool;
    pub fn pulse_configure_overlay(window: *mut c_void);
    pub fn pulse_position_overlay(
        window: *mut c_void,
        monitor: Monitor,
        notice: bool,
        error: *mut *mut c_char,
    ) -> bool;
    pub fn pulse_capture_screen(
        monitor: Monitor,
        pixels: *mut *mut u8,
        error: *mut *mut c_char,
    ) -> bool;
    pub fn pulse_recognize_text(
        rgba: *const u8,
        width: u32,
        height: u32,
        error: *mut *mut c_char,
    ) -> *mut c_char;
    pub fn pulse_prepare_text_recognition(error: *mut *mut c_char) -> bool;
    pub fn pulse_native_free(pointer: *mut c_void);
}

pub fn checked(action: impl FnOnce(*mut *mut c_char) -> bool) -> Result<(), String> {
    let mut error = std::ptr::null_mut();
    let success = action(&mut error);
    let message = if error.is_null() {
        "The native operation failed.".into()
    } else {
        unsafe { take_string(error) }
    };
    if success {
        Ok(())
    } else {
        Err(message)
    }
}

/// The bridge returns owned, NUL-terminated UTF-8 allocated with malloc.
pub unsafe fn take_string(pointer: *mut c_char) -> String {
    let value = CStr::from_ptr(pointer).to_string_lossy().into_owned();
    pulse_native_free(pointer.cast());
    value
}

pub async fn on_main<T: Send + 'static>(
    app: &tauri::AppHandle,
    action: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = send.send(action());
    })
    .map_err(|error| error.to_string())?;
    receive
        .await
        .map_err(|_| "The native window operation stopped.".to_string())?
}
