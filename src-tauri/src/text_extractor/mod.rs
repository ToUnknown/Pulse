#[cfg(target_os = "windows")]
mod capture;
#[cfg(target_os = "windows")]
mod ocr;
#[cfg(target_os = "windows")]
mod ocr_models;
#[cfg(target_os = "windows")]
mod pixels;
pub mod protocol;
#[cfg(target_os = "windows")]
mod selector_window;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "windows")]
mod hotkeys;
#[cfg(any(target_os = "windows", test))]
mod shortcut_keys;
