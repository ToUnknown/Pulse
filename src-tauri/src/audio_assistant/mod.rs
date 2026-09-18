//! Windows system-audio assistant. No capture state belongs to Text Extractor.
#[cfg(target_os = "windows")]
mod capture;
mod model;
#[cfg(target_os = "windows")]
mod runtime;
#[cfg(target_os = "windows")]
mod transcription;
#[cfg(target_os = "windows")]
pub use runtime::*;

#[cfg(test)]
pub(crate) use model::answer_body;
