use super::native;
use crate::text_extractor::local_ocr::ModelStatus;
use image::RgbaImage;
use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio_util::sync::CancellationToken;

pub struct LocalOcr {
    enabled: AtomicBool,
}
impl LocalOcr {
    pub fn new(_: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
        })
    }
    pub fn set_enabled(self: &Arc<Self>, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }
    pub fn status(&self) -> ModelStatus {
        ModelStatus::new(if self.enabled.load(Ordering::Acquire) {
            "ready"
        } else {
            "idle"
        })
    }
    pub fn retry(self: &Arc<Self>) -> Result<(), String> {
        super::ensure_supported()
    }
    pub fn recognize(
        &self,
        image: RgbaImage,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        if cancel.is_cancelled() || !self.enabled.load(Ordering::Acquire) {
            return Err("Capture cancelled.".into());
        }
        if image.width() == 0 || image.height() == 0 {
            return Err("Select a larger area.".into());
        }
        let mut result = std::ptr::null_mut();
        native::checked(|error| unsafe {
            result =
                native::pulse_recognize_text(image.as_ptr(), image.width(), image.height(), error);
            !result.is_null()
        })?;
        let json = unsafe { native::take_string(result) };
        if cancel.is_cancelled() || !self.enabled.load(Ordering::Acquire) {
            return Err("Capture cancelled.".into());
        }
        let lines: Vec<Line> =
            serde_json::from_str(&json).map_err(|_| "Could not read Apple Vision's result.")?;
        Ok(assemble_lines(lines))
    }
}

#[derive(Deserialize)]
struct Line {
    text: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}
fn assemble_lines(mut lines: Vec<Line>) -> String {
    lines.retain(|line| {
        !line.text.trim().is_empty()
            && line.x.is_finite()
            && line.y.is_finite()
            && line.width.is_finite()
            && line.height.is_finite()
            && line.height > 0.0
    });
    lines.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let mut rows: Vec<Vec<Line>> = Vec::new();
    for line in lines {
        if let Some(row) = rows.last_mut().filter(|row| {
            let first = &row[0];
            let overlap = (line.y + line.height).min(first.y + first.height) - line.y.max(first.y);
            overlap >= line.height.min(first.height) * 0.5
        }) {
            row.push(line);
        } else {
            rows.push(vec![line]);
        }
    }
    let mut output = String::new();
    let mut previous_bottom = None;
    for mut row in rows {
        row.sort_by(|a, b| a.x.total_cmp(&b.x));
        let top = row.iter().map(|line| line.y).fold(f64::INFINITY, f64::min);
        let bottom = row
            .iter()
            .map(|line| line.y + line.height)
            .fold(0.0, f64::max);
        if let Some(previous) = previous_bottom {
            output.push('\n');
            if top - previous > (bottom - top) * 0.9 {
                output.push('\n');
            }
        }
        // Vision returns word spacing within each line. Keep it intact and insert
        // a separator between separately detected fragments on the same row.
        output.push_str(
            &row.into_iter()
                .map(|line| line.text)
                .collect::<Vec<_>>()
                .join(" "),
        );
        previous_bottom = Some(bottom);
    }
    output
}
