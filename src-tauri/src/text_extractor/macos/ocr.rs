use super::native;
use crate::text_extractor::local_ocr::ModelStatus;
use image::RgbaImage;
use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, TryLockError,
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub struct LocalOcr {
    enabled: AtomicBool,
    setup: Mutex<ModelStatus>,
    changed: Condvar,
    recognition: Mutex<()>,
}
impl LocalOcr {
    pub fn new(_: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
            setup: Mutex::new(ModelStatus::new("idle")),
            changed: Condvar::new(),
            recognition: Mutex::new(()),
        })
    }
    pub fn set_enabled(self: &Arc<Self>, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        self.changed.notify_all();
        if enabled {
            self.prepare_with(prepare_native);
        }
    }
    pub fn status(&self) -> ModelStatus {
        if self.enabled.load(Ordering::Acquire) {
            self.setup.lock().unwrap().clone()
        } else {
            ModelStatus::new("idle")
        }
    }
    pub fn retry(self: &Arc<Self>) -> Result<(), String> {
        super::ensure_supported()?;
        if self.enabled.load(Ordering::Acquire) {
            self.prepare_with(prepare_native);
        }
        Ok(())
    }
    fn prepare_with(
        self: &Arc<Self>,
        prepare: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) {
        let mut setup = self.setup.lock().unwrap();
        if matches!(setup.phase, "preparing" | "ready") {
            return;
        }
        *setup = ModelStatus::new("preparing");
        drop(setup);
        let engine = self.clone();
        // A real recognition pass warms both detection and recognition. Merely
        // creating a Vision request leaves compilation on the first capture.
        let spawned = std::thread::Builder::new()
            .name("pulse-ocr-setup".into())
            .spawn(move || engine.finish_preparation(prepare()));
        if spawned.is_err() {
            self.finish_preparation(Err("Could not start on-device OCR. Try again.".into()));
        }
    }
    fn finish_preparation(&self, result: Result<(), String>) {
        let mut setup = self.setup.lock().unwrap();
        *setup = ModelStatus::new(if result.is_ok() { "ready" } else { "error" });
        setup.error = result.err();
        self.changed.notify_all();
    }
    fn wait_ready(&self, cancel: &CancellationToken) -> Result<(), String> {
        let mut setup = self.setup.lock().unwrap();
        loop {
            if cancel.is_cancelled() || !self.enabled.load(Ordering::Acquire) {
                return Err("Capture cancelled.".into());
            }
            match setup.phase {
                "ready" => return Ok(()),
                "preparing" => {
                    setup = self
                        .changed
                        .wait_timeout(setup, Duration::from_millis(50))
                        .unwrap()
                        .0;
                }
                _ => {
                    return Err(setup.error.clone().unwrap_or_else(|| {
                        "On-device OCR is not ready. Check Settings → Advanced.".into()
                    }))
                }
            }
        }
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
        self.wait_ready(cancel)?;
        // A cancelled Vision call can still be finishing inside macOS. Do not
        // pile more native requests on top of it, or keep cancelled captures queued.
        let _recognition = loop {
            if cancel.is_cancelled() || !self.enabled.load(Ordering::Acquire) {
                return Err("Capture cancelled.".into());
            }
            match self.recognition.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(50)),
                Err(TryLockError::Poisoned(_)) => {
                    return Err("Restart Pulse to use on-device OCR.".into())
                }
            }
        };
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

fn prepare_native() -> Result<(), String> {
    super::ensure_supported()?;
    native::checked(|error| unsafe { native::pulse_prepare_text_recognition(error) })
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

#[cfg(test)]
mod readiness_tests {
    use super::*;
    use std::sync::mpsc;

    fn preparing() -> (Arc<LocalOcr>, mpsc::Sender<Result<(), String>>) {
        let ocr = LocalOcr::new(PathBuf::new());
        // Use the real preparation lifecycle with a controlled native boundary.
        ocr.enabled.store(true, Ordering::Release);
        let (send, receive) = mpsc::channel();
        ocr.prepare_with(move || receive.recv_timeout(Duration::from_secs(5)).unwrap());
        (ocr, send)
    }

    #[test]
    fn preparation_is_visible_and_only_runs_once_until_ready() {
        let (ocr, complete) = preparing();
        assert_eq!(ocr.status().phase, "preparing");
        ocr.prepare_with(|| panic!("Started duplicate native preparation"));
        complete.send(Ok(())).unwrap();
        ocr.wait_ready(&CancellationToken::new()).unwrap();
        assert_eq!(ocr.status().phase, "ready");
        ocr.prepare_with(|| panic!("Recompiled an already prepared engine"));
    }

    #[test]
    fn cancellation_does_not_wait_for_native_compilation() {
        let (ocr, complete) = preparing();
        let cancel = CancellationToken::new();
        let worker = ocr.clone();
        let request = cancel.clone();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || send.send(worker.wait_ready(&request)).unwrap());
        assert!(receive.recv_timeout(Duration::from_millis(30)).is_err());
        cancel.cancel();
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap_err(),
            "Capture cancelled."
        );
        complete.send(Ok(())).unwrap();
        ocr.wait_ready(&CancellationToken::new()).unwrap();
    }

    #[test]
    fn disabling_hides_preparation_and_reenabling_reuses_completion() {
        let (ocr, complete) = preparing();
        ocr.set_enabled(false);
        assert_eq!(ocr.status().phase, "idle");
        assert!(ocr.wait_ready(&CancellationToken::new()).is_err());
        // Reenable while the existing preparation is still running.
        ocr.set_enabled(true);
        assert_eq!(ocr.status().phase, "preparing");
        complete.send(Ok(())).unwrap();
        ocr.wait_ready(&CancellationToken::new()).unwrap();
        ocr.set_enabled(false);
        ocr.set_enabled(true);
        assert_eq!(ocr.status().phase, "ready");
    }

    #[test]
    fn preparation_errors_are_visible_and_can_be_retried() {
        let (ocr, complete) = preparing();
        complete
            .send(Err("Native preparation failed".into()))
            .unwrap();
        assert_eq!(
            ocr.wait_ready(&CancellationToken::new()).unwrap_err(),
            "Native preparation failed"
        );
        assert_eq!(ocr.status().phase, "error");
        ocr.prepare_with(|| Ok(()));
        ocr.wait_ready(&CancellationToken::new()).unwrap();
        assert_eq!(ocr.status().phase, "ready");
        assert!(ocr.status().error.is_none());
    }
}
