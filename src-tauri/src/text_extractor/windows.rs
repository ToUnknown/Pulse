use super::{
    capture, ocr,
    protocol::{self, Crop, Preferences},
};
use crate::openai_credentials;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::{ImageFormat, RgbaImage};
use serde_json::{json, Value};
use std::{
    fs,
    io::Cursor,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{
    Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tokio_util::sync::CancellationToken;

const WINDOW_PREFIX: &str = "text-extractor-";
const RESPONSE_URL: &str = "https://api.openai.com/v1/responses";

pub struct TextExtractor {
    preferences: Mutex<Preferences>,
    path: PathBuf,
    session: Mutex<Option<Session>>,
    starting: AtomicBool,
    recording_shortcut: AtomicBool,
    next_id: AtomicU64,
    error: Mutex<Option<String>>,
}

struct Session {
    label: String,
    image: Arc<RgbaImage>,
    cancel: CancellationToken,
    request_id: u32,
    request_cancel: CancellationToken,
    shown: bool,
}

fn settings_only(window: &WebviewWindow) -> Result<(), String> {
    if window.label() != "settings" {
        return Err("Open Text Extractor settings to do this.".into());
    }
    Ok(())
}

fn shortcut(value: &str) -> Result<Shortcut, String> {
    let parsed = value
        .parse::<Shortcut>()
        .map_err(|_| "Choose a valid keyboard shortcut.".to_string())?;
    if !parsed
        .mods
        .intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER)
    {
        return Err("Include Ctrl, Alt, or Windows in the shortcut.".into());
    }
    Ok(parsed)
}

fn save(path: &PathBuf, preferences: &Preferences) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| "Could not create the settings folder.")?;
    }
    fs::write(
        path,
        serde_json::to_vec(preferences).map_err(|_| "Could not encode settings.")?,
    )
    .map_err(|_| "Could not save Text Extractor settings.".into())
}

pub fn install(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let path = app.path().app_config_dir()?.join("text-extractor.json");
    let mut preferences = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Preferences>(&bytes).unwrap_or_default(),
        Err(_) => Preferences::default(),
    };
    if shortcut(&preferences.shortcut).is_err() {
        preferences = Preferences::default();
    }
    app.manage(TextExtractor {
        preferences: Mutex::new(preferences.clone()),
        path,
        session: Mutex::new(None),
        starting: AtomicBool::new(false),
        recording_shortcut: AtomicBool::new(false),
        next_id: AtomicU64::new(1),
        error: Mutex::new(None),
    });
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, key, event| {
                if event.state() != ShortcutState::Pressed {
                    return;
                }
                let app = app.clone();
                let key = *key;
                // Do not lock preferences on the event thread while registration is in flight.
                tauri::async_runtime::spawn(async move {
                    let matches = {
                        let state = app.state::<TextExtractor>();
                        let prefs = state.preferences.lock().unwrap();
                        !state.recording_shortcut.load(Ordering::Acquire)
                            && prefs.enabled
                            && shortcut(&prefs.shortcut).is_ok_and(|registered| registered == key)
                    };
                    if matches {
                        if let Err(error) = start(&app).await {
                            *app.state::<TextExtractor>().error.lock().unwrap() = Some(error);
                            let _ = crate::open_settings(&app);
                        }
                    }
                });
            })
            .build(),
    )?;
    if preferences.enabled {
        if let Err(reason) = app
            .global_shortcut()
            .register(shortcut(&preferences.shortcut)?)
        {
            let state = app.state::<TextExtractor>();
            state.preferences.lock().unwrap().enabled = false;
            *state.error.lock().unwrap() = Some(format!(
                "Shortcut unavailable. Choose another combination in Settings. ({reason})"
            ));
        }
    }
    Ok(())
}

#[tauri::command]
pub fn text_extractor_state(app: tauri::AppHandle, window: WebviewWindow) -> Result<Value, String> {
    settings_only(&window)?;
    let state = app.state::<TextExtractor>();
    let credential_state = openai_credentials::is_configured();
    let configured = credential_state.as_ref().copied().unwrap_or(false);
    let preferences = state.preferences.lock().unwrap().clone();
    let error = state
        .error
        .lock()
        .unwrap()
        .clone()
        .or_else(|| credential_state.err());
    Ok(
        json!({"enabled": preferences.enabled, "shortcut": preferences.shortcut,
        "apiKeyConfigured": configured, "error": error}),
    )
}

#[tauri::command]
pub fn save_openai_api_key(window: WebviewWindow, api_key: String) -> Result<(), String> {
    settings_only(&window)?;
    openai_credentials::save(&api_key)
}

#[tauri::command]
pub fn record_text_extractor_shortcut(
    app: tauri::AppHandle,
    window: WebviewWindow,
    recording: bool,
) -> Result<(), String> {
    settings_only(&window)?;
    app.state::<TextExtractor>()
        .recording_shortcut
        .store(recording, Ordering::Release);
    Ok(())
}

pub fn settings_blurred(app: &tauri::AppHandle) {
    app.state::<TextExtractor>()
        .recording_shortcut
        .store(false, Ordering::Release);
}

#[tauri::command]
pub async fn set_text_extractor(
    app: tauri::AppHandle,
    window: WebviewWindow,
    enabled: bool,
    shortcut_value: String,
) -> Result<(), String> {
    settings_only(&window)?;
    let next_shortcut = shortcut(&shortcut_value)?;
    let state = app.state::<TextExtractor>();
    let mut current = state.preferences.lock().unwrap();
    let old_shortcut = shortcut(&current.shortcut)?;
    let changed = old_shortcut != next_shortcut;
    // Reserve the new shortcut before releasing the working one.
    let added = enabled && (!current.enabled || changed);
    if added {
        app.global_shortcut()
            .register(next_shortcut)
            .map_err(|_| "This shortcut is already in use. Choose another combination.")?;
    }
    let removed = current.enabled && (!enabled || changed);
    if removed && app.global_shortcut().unregister(old_shortcut).is_err() {
        if added {
            let _ = app.global_shortcut().unregister(next_shortcut);
        }
        return Err("Could not release the previous shortcut. Try again.".into());
    }
    let next = Preferences {
        enabled,
        shortcut: next_shortcut.to_string(),
    };
    if let Err(error) = save(&state.path, &next) {
        if added {
            let _ = app.global_shortcut().unregister(next_shortcut);
        }
        if removed {
            let _ = app.global_shortcut().register(old_shortcut);
        }
        return Err(error);
    }
    *current = next;
    *state.error.lock().unwrap() = None;
    drop(current);
    if !enabled {
        close_active(&app);
    }
    Ok(())
}

struct StartingGuard<'a>(&'a AtomicBool);
impl Drop for StartingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn native_theme(theme: crate::WindowsTheme) -> tauri::Theme {
    match theme {
        crate::WindowsTheme::Light => tauri::Theme::Light,
        crate::WindowsTheme::Dark => tauri::Theme::Dark,
    }
}

pub fn appearance_changed(app: &tauri::AppHandle, theme: crate::WindowsTheme) {
    let state = app.state::<TextExtractor>();
    let label = state
        .session
        .lock()
        .unwrap()
        .as_ref()
        .map(|session| session.label.clone());
    if let Some(window) = label.and_then(|label| app.get_webview_window(&label)) {
        if let Err(error) = window.set_theme(Some(native_theme(theme))) {
            eprintln!("Text Extractor appearance update failed: {error}");
        }
    }
}

async fn start(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<TextExtractor>();
    if state.starting.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let _starting = StartingGuard(&state.starting);
    let active_label = state
        .session
        .lock()
        .unwrap()
        .as_ref()
        .map(|session| session.label.clone());
    if let Some(label) = active_label {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.set_focus();
        }
        return Ok(());
    }
    if !state.preferences.lock().unwrap().enabled {
        return Ok(());
    }
    let capture = tauri::async_runtime::spawn_blocking(capture::monitor_at_pointer)
        .await
        .map_err(|_| "Screen capture stopped unexpectedly.")??;
    let label = format!(
        "{WINDOW_PREFIX}{}",
        state.next_id.fetch_add(1, Ordering::Relaxed)
    );
    let size = PhysicalSize::new(capture.image.width(), capture.image.height());
    let position = PhysicalPosition::new(capture.x, capture.y);
    {
        // Publish the session under the same lock used to disable the feature.
        let preferences = state.preferences.lock().unwrap();
        if !preferences.enabled {
            return Ok(());
        }
        *state.session.lock().unwrap() = Some(Session {
            label: label.clone(),
            image: Arc::new(capture.image),
            cancel: CancellationToken::new(),
            request_id: 0,
            request_cancel: CancellationToken::new(),
            shown: false,
        });
    }
    let result = (|| {
        let window =
            WebviewWindowBuilder::new(app, &label, WebviewUrl::App("text-extractor.html".into()))
                .title("Pulse Text Extractor")
                .theme(Some(native_theme(crate::visual_windows_theme(app)?)))
                .visible(false)
                .decorations(false)
                .shadow(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .maximizable(false)
                .minimizable(false)
                .content_protected(true)
                .build()
                .map_err(|_| "Could not open Text Extractor.")?;
        window
            .set_position(position)
            .map_err(|_| "Could not position Text Extractor.")?;
        window
            .set_size(size)
            .map_err(|_| "Could not size Text Extractor.")?;
        Ok::<(), String>(())
    })();
    let active = state
        .session
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.label == label);
    if result.is_err() || !active {
        close_active(app);
        // Disabling can race window creation after the session was published.
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.destroy();
        }
    } else {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(20)).await;
            let state = app.state::<TextExtractor>();
            let stalled = {
                let mut active = state.session.lock().unwrap();
                if active
                    .as_ref()
                    .is_some_and(|s| s.label == label && !s.shown)
                {
                    active.take()
                } else {
                    None
                }
            };
            if let Some(session) = stalled {
                session.cancel.cancel();
                session.request_cancel.cancel();
                if let Some(window) = app.get_webview_window(&session.label) {
                    let _ = window.destroy();
                }
                *state.error.lock().unwrap() =
                    Some("Text Extractor could not open. Try the shortcut again.".into());
                let _ = crate::open_settings(&app);
            }
        });
    }
    result
}

fn png_url(image: &RgbaImage) -> Result<String, String> {
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|_| "Could not prepare the screenshot.")?;
    Ok(format!(
        "data:image/png;base64,{}",
        BASE64.encode(bytes.into_inner())
    ))
}

#[tauri::command]
pub async fn text_extractor_capture(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<Value, String> {
    let image = {
        let state = app.state::<TextExtractor>();
        let session = state.session.lock().unwrap();
        let session = session
            .as_ref()
            .filter(|s| s.label == window.label())
            .ok_or("This capture has ended.")?;
        session.image.clone()
    };
    tauri::async_runtime::spawn_blocking(move || {
        Ok(
            json!({"width": image.width(), "height": image.height(), "imageUrl": png_url(&image)?,
            "advancedAvailable": openai_credentials::is_configured().unwrap_or(false)}),
        )
    })
    .await
    .map_err(|_| "Could not load this capture.")?
}

#[tauri::command]
pub fn text_extractor_show(app: tauri::AppHandle, window: WebviewWindow) -> Result<(), String> {
    ensure_session(&app, &window)?;
    // Re-read Pulse's visual theme in case it changed while the capture loaded.
    window
        .set_theme(Some(native_theme(crate::visual_windows_theme(&app)?)))
        .map_err(|_| "Could not apply Pulse appearance.".to_string())?;
    window
        .show()
        .and_then(|_| window.set_focus())
        .map_err(|_| "Could not show Text Extractor.".to_string())?;
    if let Some(session) = app
        .state::<TextExtractor>()
        .session
        .lock()
        .unwrap()
        .as_mut()
        .filter(|session| session.label == window.label())
    {
        session.shown = true;
    }
    Ok(())
}

fn ensure_session(app: &tauri::AppHandle, window: &WebviewWindow) -> Result<(), String> {
    let state = app.state::<TextExtractor>();
    let active = state
        .session
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.label == window.label());
    if active {
        Ok(())
    } else {
        Err("This capture has ended.".into())
    }
}

#[tauri::command]
pub fn text_extractor_capabilities(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<bool, String> {
    ensure_session(&app, &window)?;
    // Credential storage failure must not prevent local OCR.
    Ok(openai_credentials::is_configured().unwrap_or(false))
}

/// Request IDs also order cancellation IPC, so a late request can never replace a
/// newer mode's work or clear its cancellation token.
fn begin_request(session: &mut Session, request_id: u32) -> Result<CancellationToken, String> {
    if request_id <= session.request_id {
        return Err("Capture cancelled.".into());
    }
    session.request_cancel.cancel();
    session.request_id = request_id;
    session.request_cancel = session.cancel.child_token();
    Ok(session.request_cancel.clone())
}

#[tauri::command]
pub fn cancel_text_extraction(
    app: tauri::AppHandle,
    window: WebviewWindow,
    request_id: u32,
) -> Result<(), String> {
    let state = app.state::<TextExtractor>();
    let mut active = state.session.lock().unwrap();
    let session = active
        .as_mut()
        .filter(|s| s.label == window.label())
        .ok_or("This capture has ended.")?;
    begin_request(session, request_id)?.cancel();
    Ok(())
}

#[tauri::command]
pub async fn extract_screen_text(
    app: tauri::AppHandle,
    window: WebviewWindow,
    crop: Crop,
    mode: String,
    request_id: u32,
) -> Result<String, String> {
    if mode != "basic" && mode != "advanced" {
        return Err("Choose Basic or Advanced.".into());
    }
    let (image, cancel) = {
        let state = app.state::<TextExtractor>();
        let mut active = state.session.lock().unwrap();
        let session = active
            .as_mut()
            .filter(|s| s.label == window.label())
            .ok_or("This capture has ended.")?;
        crop.validate(session.image.width(), session.image.height())?;
        (session.image.clone(), begin_request(session, request_id)?)
    };
    let result = tokio::select! {
        _ = cancel.cancelled() => Err("Capture cancelled.".into()),
        result = async {
            if mode == "basic" {
                tauri::async_runtime::spawn_blocking(move || {
                    let crop = image::imageops::crop_imm(image.as_ref(), crop.x, crop.y, crop.width, crop.height).to_image();
                    ocr::recognize(crop)
                }).await.map_err(|_| "Windows text recognition stopped unexpectedly.".to_string())?
            } else {
                extract(image, crop).await
            }
        } => result,
    };
    finish_request(&app, &window, request_id, result)
}

#[tauri::command]
pub async fn translate_extracted_text(
    app: tauri::AppHandle,
    window: WebviewWindow,
    text: String,
    language: String,
    request_id: u32,
) -> Result<String, String> {
    let body = protocol::translation_body(&text, &language)?;
    let cancel = {
        let state = app.state::<TextExtractor>();
        let mut active = state.session.lock().unwrap();
        let session = active
            .as_mut()
            .filter(|s| s.label == window.label())
            .ok_or("This capture has ended.")?;
        begin_request(session, request_id)?
    };
    let result = tokio::select! {
        _ = cancel.cancelled() => Err("Capture cancelled.".into()),
        result = request_text(body) => result,
    };
    finish_request(&app, &window, request_id, result)
}

fn finish_request(
    app: &tauri::AppHandle,
    window: &WebviewWindow,
    request_id: u32,
    result: Result<String, String>,
) -> Result<String, String> {
    let state = app.state::<TextExtractor>();
    let active = state.session.lock().unwrap();
    if !active.as_ref().is_some_and(|s| {
        s.label == window.label() && s.request_id == request_id && !s.request_cancel.is_cancelled()
    }) {
        return Err("Capture cancelled.".into());
    }
    result
}

async fn extract(image: Arc<RgbaImage>, crop: Crop) -> Result<String, String> {
    let image_url = tauri::async_runtime::spawn_blocking(move || {
        let crop =
            image::imageops::crop_imm(image.as_ref(), crop.x, crop.y, crop.width, crop.height)
                .to_image();
        png_url(&crop)
    })
    .await
    .map_err(|_| "Could not prepare this selection.")??;
    request_text(protocol::request_body(&image_url)).await
}

async fn request_text(body: Value) -> Result<String, String> {
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

#[tauri::command]
pub fn copy_extracted_text(
    app: tauri::AppHandle,
    window: WebviewWindow,
    text: String,
) -> Result<(), String> {
    ensure_session(&app, &window)?;
    if text.len() > 1_000_000 {
        return Err("The text is too large to copy.".into());
    }
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text))
        .map_err(|_| "The clipboard is busy. Try copying again.".into())
}

#[tauri::command]
pub fn close_text_extractor(app: tauri::AppHandle, window: WebviewWindow) -> Result<(), String> {
    ensure_session(&app, &window)?;
    close_active(&app);
    Ok(())
}

fn close_active(app: &tauri::AppHandle) {
    let session = app.state::<TextExtractor>().session.lock().unwrap().take();
    if let Some(session) = session {
        session.cancel.cancel();
        session.request_cancel.cancel();
        if let Some(window) = app.get_webview_window(&session.label) {
            let _ = window.destroy();
        }
    }
}

pub fn window_destroyed(app: &tauri::AppHandle, label: &str) {
    if !label.starts_with(WINDOW_PREFIX) {
        return;
    }
    let state = app.state::<TextExtractor>();
    let mut active = state.session.lock().unwrap();
    if active.as_ref().is_some_and(|s| s.label == label) {
        if let Some(session) = active.take() {
            session.cancel.cancel();
            session.request_cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_switch_cancels_old_work_and_rejects_out_of_order_requests() {
        let mut session = Session {
            label: "text-extractor-test".into(),
            image: Arc::new(RgbaImage::new(4, 4)),
            cancel: CancellationToken::new(),
            request_id: 0,
            request_cancel: CancellationToken::new(),
            shown: true,
        };
        let basic = begin_request(&mut session, 1).unwrap();
        let advanced = begin_request(&mut session, 3).unwrap();
        assert!(basic.is_cancelled());
        assert!(!advanced.is_cancelled());
        assert!(begin_request(&mut session, 2).is_err());
        assert!(begin_request(&mut session, 3).is_err());
        assert!(!advanced.is_cancelled());
        session.cancel.cancel();
        assert!(advanced.is_cancelled());
    }
}
