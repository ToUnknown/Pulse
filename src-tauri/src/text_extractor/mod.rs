#[cfg(target_os = "windows")]
mod capture;
#[cfg(target_os = "windows")]
mod ocr;
pub mod protocol;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;
