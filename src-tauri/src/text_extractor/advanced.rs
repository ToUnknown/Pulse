//! The common editor dispatches only to the explicitly selected cloud provider.
use super::protocol::{self, AdvancedProvider};
use crate::openai_credentials;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{sync::Mutex, time::Duration};
use tauri::Manager;
use tokio_util::sync::CancellationToken;

const RESPONSE_URL: &str = "https://api.openai.com/v1/responses";

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub available: bool,
    pub checking: bool,
    pub message: Option<String>,
}
impl ProviderStatus {
    fn checking() -> Self {
        Self {
            available: false,
            checking: true,
            message: None,
        }
    }
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            available: false,
            checking: false,
            message: Some(message.into()),
        }
    }
}
struct AdvancedModels {
    apple: Mutex<ProviderStatus>,
    refreshing: tokio::sync::Mutex<()>,
}
pub fn install(app: &tauri::AppHandle, provider: AdvancedProvider) {
    app.manage(AdvancedModels {
        apple: Mutex::new(ProviderStatus::unavailable(
            "Apple Intelligence has not been checked.",
        )),
        refreshing: tokio::sync::Mutex::new(()),
    });
    if provider == AdvancedProvider::Apple && cfg!(target_os = "macos") {
        *app.state::<AdvancedModels>().apple.lock().unwrap() = ProviderStatus::checking();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            refresh_apple(&app).await;
        });
    }
}
pub fn apple_status(app: &tauri::AppHandle) -> ProviderStatus {
    app.state::<AdvancedModels>().apple.lock().unwrap().clone()
}
pub fn available(app: &tauri::AppHandle, provider: AdvancedProvider) -> bool {
    match provider {
        AdvancedProvider::Openai => openai_credentials::is_configured().unwrap_or(false),
        AdvancedProvider::Apple => apple_status(app).available,
    }
}
pub async fn refresh_apple(app: &tauri::AppHandle) {
    let state = app.state::<AdvancedModels>();
    let Ok(_refresh) = state.refreshing.try_lock() else {
        return;
    };
    *state.apple.lock().unwrap() = ProviderStatus::checking();
    #[cfg(target_os = "macos")]
    let status = super::macos::apple_intelligence::status().await;
    #[cfg(not(target_os = "macos"))]
    let status = ProviderStatus::unavailable("Apple Intelligence is available only on macOS.");
    *state.apple.lock().unwrap() = status;
}
pub async fn request(
    app: &tauri::AppHandle,
    provider: AdvancedProvider,
    body: Value,
    cancel: CancellationToken,
) -> Result<String, String> {
    if provider == AdvancedProvider::Openai {
        return tokio::select! {
            _ = cancel.cancelled() => Err("Capture cancelled.".into()),
            result = openai_request(body) => result,
        };
    }
    #[cfg(target_os = "macos")]
    {
        let result = super::macos::apple_intelligence::request(body, cancel).await;
        // Model errors are shown by the editor/quick-copy error path. A retry in
        // Settings rechecks access without submitting any user data or test prompt.
        if let Err(error) = &result {
            if error != "Capture cancelled." {
                *app.state::<AdvancedModels>().apple.lock().unwrap() =
                    ProviderStatus::unavailable(error);
            }
        }
        result
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("Apple Intelligence is available only on macOS.".into())
    }
}

async fn openai_request(body: Value) -> Result<String, String> {
    let key = openai_credentials::load()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not start the connection to OpenAI.")?;
    let mut response = client
        .post(RESPONSE_URL)
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "The request timed out. Try again or use less text."
            } else {
                "Could not reach OpenAI. Check your connection and try again."
            }
        })?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 => "The OpenAI API key was rejected. Update the shared key in Settings.",
            403 | 404 => "This API key does not have access to GPT-5.6 Luna.",
            429 => "OpenAI usage or rate limit reached. Check your API billing or try again later.",
            _ => "OpenAI could not complete the request. Try again shortly.",
        }
        .into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "The response was interrupted. Try again.")?
    {
        if bytes.len() + chunk.len() > 2_000_000 {
            return Err("The response was too large. Select a smaller area.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let response = serde_json::from_slice(&bytes)
        .map_err(|_| "OpenAI returned an unreadable response. Try again.")?;
    protocol::response_text(&response)
}
