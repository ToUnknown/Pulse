use crate::text_extractor::advanced::ProviderStatus;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub async fn status() -> ProviderStatus {
    match call(
        serde_json::json!({"operation": "status"}),
        CancellationToken::new(),
    )
    .await
    {
        Ok(value) => serde_json::from_str(&value).unwrap_or_else(|_| {
            ProviderStatus::unavailable("Could not read Apple Intelligence availability. Retry.")
        }),
        Err(error) => ProviderStatus::unavailable(error),
    }
}
pub async fn request(body: Value, cancel: CancellationToken) -> Result<String, String> {
    let content = body["input"][0]["content"]
        .as_array()
        .ok_or("Invalid extraction request.")?;
    let text = content
        .iter()
        .find(|item| item["type"] == "input_text")
        .and_then(|item| item["text"].as_str())
        .unwrap_or("");
    let image = content
        .iter()
        .find(|item| item["type"] == "input_image")
        .and_then(|item| item["image_url"].as_str())
        .and_then(|url| url.strip_prefix("data:image/png;base64,"));
    call(serde_json::json!({"operation": "generate", "instructions": body["instructions"], "text": text, "imageBase64": image}), cancel).await
}

#[cfg(not(pulse_apple_pcc))]
async fn call(_: Value, _: CancellationToken) -> Result<String, String> {
    Err("Apple Intelligence is unavailable in this build. Build Pulse with Xcode 27, or select OpenAI.".into())
}

#[cfg(pulse_apple_pcc)]
async fn call(body: Value, cancel: CancellationToken) -> Result<String, String> {
    bridge::call(body, cancel).await
}

#[cfg(pulse_apple_pcc)]
mod bridge {
    use super::*;
    use std::{
        collections::HashMap,
        ffi::{c_char, CStr, CString},
        sync::{
            atomic::{AtomicU64, Ordering},
            Mutex, OnceLock,
        },
        time::Duration,
    };
    use tokio::sync::oneshot;
    type Response = Result<String, String>;
    static NEXT: AtomicU64 = AtomicU64::new(1);
    static PENDING: OnceLock<Mutex<HashMap<u64, oneshot::Sender<Response>>>> = OnceLock::new();
    extern "C" {
        fn pulse_apple_start(
            id: u64,
            json: *const c_char,
            callback: extern "C" fn(u64, i32, *const c_char),
        );
        fn pulse_apple_cancel(id: u64);
    }
    fn pending() -> &'static Mutex<HashMap<u64, oneshot::Sender<Response>>> {
        PENDING.get_or_init(Mutex::default)
    }
    extern "C" fn replied(id: u64, success: i32, text: *const c_char) {
        let Some(sender) = pending().lock().unwrap().remove(&id) else {
            return;
        };
        let value = if text.is_null() {
            "Apple Intelligence returned no response.".into()
        } else {
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        };
        let _ = sender.send(if success == 1 && !text.is_null() {
            Ok(value)
        } else {
            Err(value)
        });
    }
    struct Request(u64);
    impl Drop for Request {
        fn drop(&mut self) {
            pending().lock().unwrap().remove(&self.0);
            unsafe {
                pulse_apple_cancel(self.0);
            }
        }
    }
    pub async fn call(body: Value, cancel: CancellationToken) -> Response {
        if cancel.is_cancelled() {
            return Err("Capture cancelled.".into());
        }
        let timeout = if body["operation"] == "status" {
            15
        } else {
            120
        };
        let payload = CString::new(body.to_string())
            .map_err(|_| "Could not encode the Apple Intelligence request.")?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let (send, receive) = oneshot::channel();
        pending().lock().unwrap().insert(id, send);
        let _request = Request(id);
        // Swift copies the JSON before returning; neither side shares a borrowed
        // buffer across an await, and late callbacks cannot reach cancelled IPC.
        unsafe {
            pulse_apple_start(id, payload.as_ptr(), replied);
        }
        tokio::select! {
            _ = cancel.cancelled() => Err("Capture cancelled.".into()),
            _ = tokio::time::sleep(Duration::from_secs(timeout)) => Err("Apple Intelligence timed out. Retry or select OpenAI.".into()),
            result = receive => result.map_err(|_| "Apple Intelligence stopped unexpectedly.".to_string())?,
        }
    }
}
