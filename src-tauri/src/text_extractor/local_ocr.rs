//! Status shared by the built-in macOS engine and the Windows model installer.
use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub(crate) phase: &'static str,
    pub(crate) model_index: usize,
    pub(crate) downloaded_bytes: u64,
    pub(crate) total_bytes: Option<u64>,
    pub(crate) error: Option<String>,
}
impl ModelStatus {
    pub(crate) fn new(phase: &'static str) -> Self {
        Self {
            phase,
            model_index: 0,
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
        }
    }
}
