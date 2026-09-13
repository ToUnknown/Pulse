#[cfg(target_os = "windows")]
mod capture;
pub mod protocol;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;
