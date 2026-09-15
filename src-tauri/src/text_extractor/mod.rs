//! Shared extraction lifecycle with compile-time native platform adapters.
mod advanced;
mod codex;
mod google_translate;
mod local_ocr;
mod pixels;
pub mod protocol;
mod session;
pub use session::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;
#[cfg(target_os = "windows")]
pub(crate) use windows::run_shortcut_worker_if_requested;
