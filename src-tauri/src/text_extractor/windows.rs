use super::{
    capture, hotkeys, ocr,
    protocol::{self, Crop, Preferences},
};
use crate::openai_credentials;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::{
    codecs::png::{CompressionType, FilterType, PngEncoder},
    ImageEncoder, RgbaImage,
};
use serde_json::{json, Value};
use std::{
    fs,
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
    warm: Mutex<Option<WarmWindow>>,
    preparing: tokio::sync::Mutex<()>,
    starting: AtomicBool,
    hotkeys_ready: AtomicBool,
    recording_shortcut: AtomicBool,
    next_id: AtomicU64,
    error: Mutex<Option<String>>,
}

struct WarmWindow {
    label: String,
    ready: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum CaptureMode {
    Quick,
    Editor,
}

struct Session {
    label: String,
    mode: CaptureMode,
    monitor: capture::Monitor,
    image: Option<Arc<RgbaImage>>,
    crop: Option<Crop>,
    capturing: bool,
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
    if shortcut(&preferences.shortcut).is_ok_and(|key| {
        key == shortcut("Control+Shift+E").unwrap()
            || key == shortcut(protocol::QUICK_SHORTCUT).unwrap()
    }) {
        preferences.shortcut = protocol::DEFAULT_SHORTCUT.into();
        let _ = save(&path, &preferences);
    }
    if shortcut(&preferences.shortcut).is_err() {
        preferences = Preferences::default();
    }
    app.manage(TextExtractor {
        preferences: Mutex::new(preferences.clone()),
        path,
        session: Mutex::new(None),
        warm: Mutex::new(None),
        preparing: tokio::sync::Mutex::new(()),
        starting: AtomicBool::new(false),
        hotkeys_ready: AtomicBool::new(false),
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
                        if let Err(error) = start(&app, CaptureMode::Editor).await {
                            *app.state::<TextExtractor>().error.lock().unwrap() = Some(error);
                            let _ = crate::open_settings(&app);
                        }
                    }
                });
            })
            .build(),
    )?;
    match hotkeys::install(app) {
        Ok(()) => app
            .state::<TextExtractor>()
            .hotkeys_ready
            .store(true, Ordering::Release),
        Err(error) => {
            let state = app.state::<TextExtractor>();
            state.preferences.lock().unwrap().enabled = false;
            *state.error.lock().unwrap() = Some(error);
        }
    }
    if app
        .state::<TextExtractor>()
        .preferences
        .lock()
        .unwrap()
        .enabled
    {
        if let Err(reason) = register_editor(app, shortcut(&preferences.shortcut)?) {
            let state = app.state::<TextExtractor>();
            state.preferences.lock().unwrap().enabled = false;
            *state.error.lock().unwrap() = Some(reason);
        }
    }
    let enabled = app
        .state::<TextExtractor>()
        .preferences
        .lock()
        .unwrap()
        .enabled;
    hotkeys::configure(
        enabled,
        is_editor_override(shortcut(&preferences.shortcut)?),
    );
    queue_prewarm(app);
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
    hotkeys::recording(recording);
    app.state::<TextExtractor>()
        .recording_shortcut
        .store(recording, Ordering::Release);
    Ok(())
}

pub fn settings_blurred(app: &tauri::AppHandle) {
    hotkeys::recording(false);
    app.state::<TextExtractor>()
        .recording_shortcut
        .store(false, Ordering::Release);
}

fn is_editor_override(key: Shortcut) -> bool {
    key == shortcut(protocol::DEFAULT_SHORTCUT).unwrap()
}
fn register_editor(app: &tauri::AppHandle, key: Shortcut) -> Result<(), String> {
    if is_editor_override(key) {
        return Ok(());
    }
    app.global_shortcut()
        .register(key)
        .map_err(|_| "This shortcut is already in use. Choose another combination.".into())
}
fn unregister_editor(app: &tauri::AppHandle, key: Shortcut) -> Result<(), String> {
    if is_editor_override(key) {
        return Ok(());
    }
    app.global_shortcut()
        .unregister(key)
        .map_err(|_| "Could not release the previous shortcut. Try again.".into())
}

pub(super) fn report_error(app: &tauri::AppHandle, error: String) {
    *app.state::<TextExtractor>().error.lock().unwrap() = Some(error);
    let _ = crate::open_settings(app);
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
    if next_shortcut == shortcut(protocol::QUICK_SHORTCUT)? {
        return Err(
            "Win + Shift + T is reserved for quick copy. Choose another editor shortcut.".into(),
        );
    }
    let state = app.state::<TextExtractor>();
    if enabled && !state.hotkeys_ready.load(Ordering::Acquire) {
        return Err(
            "Windows could not install Text Extractor shortcuts. Restart Pulse and try again."
                .into(),
        );
    }
    let mut current = state.preferences.lock().unwrap();
    let old_shortcut = shortcut(&current.shortcut)?;
    let changed = old_shortcut != next_shortcut;
    // Reserve the new shortcut before releasing the working one.
    let added = enabled && (!current.enabled || changed);
    if added {
        register_editor(&app, next_shortcut)?;
    }
    let removed = current.enabled && (!enabled || changed);
    if removed && unregister_editor(&app, old_shortcut).is_err() {
        if added {
            let _ = unregister_editor(&app, next_shortcut);
        }
        return Err("Could not release the previous shortcut. Try again.".into());
    }
    let next = Preferences {
        enabled,
        shortcut: next_shortcut.to_string(),
    };
    if let Err(error) = save(&state.path, &next) {
        if added {
            let _ = unregister_editor(&app, next_shortcut);
        }
        if removed {
            let _ = register_editor(&app, old_shortcut);
        }
        return Err(error);
    }
    *current = next;
    hotkeys::configure(enabled, is_editor_override(next_shortcut));
    *state.error.lock().unwrap() = None;
    drop(current);
    if !enabled {
        close_active(&app);
        let warm = state.warm.lock().unwrap().take();
        if let Some(window) = warm.and_then(|warm| app.get_webview_window(&warm.label)) {
            let _ = window.destroy();
        }
    } else {
        queue_prewarm(&app);
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

fn queue_prewarm(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = prepare_window(&app).await {
            *app.state::<TextExtractor>().error.lock().unwrap() = Some(error);
        }
    });
}

async fn prepare_window(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<TextExtractor>();
    let _preparing = state.preparing.lock().await;
    if !state.preferences.lock().unwrap().enabled || state.warm.lock().unwrap().is_some() {
        return Ok(());
    }
    let label = format!(
        "{WINDOW_PREFIX}{}",
        state.next_id.fetch_add(1, Ordering::Relaxed)
    );
    *state.warm.lock().unwrap() = Some(WarmWindow {
        label: label.clone(),
        ready: false,
    });
    let created = (|| {
        let window =
            WebviewWindowBuilder::new(app, &label, WebviewUrl::App("text-extractor.html".into()))
                .title("Pulse Text Extractor")
                .theme(Some(native_theme(crate::visual_windows_theme(app)?)))
                .visible(false)
                .focused(false)
                .transparent(true)
                .background_color(tauri::window::Color(0, 0, 0, 0))
                .decorations(false)
                .shadow(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .maximizable(false)
                .minimizable(false)
                .content_protected(true)
                .build()
                .map_err(|_| "Could not prepare Text Extractor.")?;
        let hwnd = window
            .hwnd()
            .map_err(|_| "Could not prepare the selector window.")?
            .0;
        unsafe {
            use windows_sys::Win32::{
                Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED},
                UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE},
            };
            let disabled: i32 = 1;
            // No OS window entrance animation: the live desktop remains visible.
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                (&disabled as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            );
            // Capture the desktop beneath our transparent selection border.
            if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) == 0 {
                return Err("Windows could not exclude the selector from screen capture.".into());
            }
        }
        Ok::<(), String>(())
    })();
    if created.is_err() || !state.preferences.lock().unwrap().enabled {
        state.warm.lock().unwrap().take();
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.destroy();
        }
    }
    created
}

#[tauri::command]
pub fn text_extractor_ready(app: tauri::AppHandle, window: WebviewWindow) -> Result<bool, String> {
    let state = app.state::<TextExtractor>();
    // Keep this lock through the session check, matching start's publication order.
    let mut warm = state.warm.lock().unwrap();
    if let Some(warm) = warm.as_mut().filter(|warm| warm.label == window.label()) {
        warm.ready = true;
        return Ok(false);
    }
    ensure_session(&app, &window)?;
    Ok(true)
}

pub(super) async fn start(app: &tauri::AppHandle, mode: CaptureMode) -> Result<(), String> {
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
    // Only monitor geometry is queried on the shortcut path; no pixels or key reads.
    let monitor = capture::monitor_at_pointer()?;
    prepare_window(app).await?;
    let prepared_label = state
        .warm
        .lock()
        .unwrap()
        .as_ref()
        .map(|warm| warm.label.clone())
        .ok_or("Text Extractor is still preparing. Try again.")?;
    let window = app
        .get_webview_window(&prepared_label)
        .ok_or("Could not open Text Extractor.")?;
    // Window dispatch must not hold locks needed by the webview's ready callback.
    window
        .set_position(PhysicalPosition::new(monitor.x, monitor.y))
        .map_err(|_| "Could not position Text Extractor.")?;
    window
        .set_size(PhysicalSize::new(monitor.width, monitor.height))
        .map_err(|_| "Could not size Text Extractor.")?;
    let (label, ready) = {
        let preferences = state.preferences.lock().unwrap();
        if !preferences.enabled {
            return Ok(());
        }
        let mut warm = state.warm.lock().unwrap();
        if !warm
            .as_ref()
            .is_some_and(|warm| warm.label == prepared_label)
        {
            return Err("Text Extractor was disabled. Try again.".into());
        }
        let prepared = warm.take().unwrap();
        *state.session.lock().unwrap() = Some(Session {
            label: prepared.label.clone(),
            mode,
            monitor,
            image: None,
            crop: None,
            capturing: false,
            cancel: CancellationToken::new(),
            request_id: 0,
            request_cancel: CancellationToken::new(),
            shown: false,
        });
        (prepared.label, prepared.ready)
    };
    *state.error.lock().unwrap() = None;
    if ready {
        let window = app
            .get_webview_window(&label)
            .ok_or("Could not open Text Extractor.")?;
        if window
            .eval("window.dispatchEvent(new Event('pulse-capture-start'))")
            .is_err()
        {
            close_active(app);
            return Err("Could not start the selector. Try again.".into());
        }
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        if close_matching(&app, &label, true) {
            queue_prewarm(&app);
            *app.state::<TextExtractor>().error.lock().unwrap() =
                Some("Text Extractor could not open. Try the shortcut again.".into());
            let _ = crate::open_settings(&app);
        }
    });
    Ok(())
}

fn png_url(image: &RgbaImage) -> Result<String, String> {
    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::Adaptive)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|_| "Could not prepare the screenshot.")?;
    Ok(format!("data:image/png;base64,{}", BASE64.encode(bytes)))
}

#[tauri::command]
pub fn text_extractor_capture(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<Value, String> {
    let state = app.state::<TextExtractor>();
    let active = state.session.lock().unwrap();
    let session = active
        .as_ref()
        .filter(|session| session.label == window.label())
        .ok_or("This capture has ended.")?;
    Ok(
        json!({"width": session.monitor.width, "height": session.monitor.height, "mode": if session.mode == CaptureMode::Quick { "quick" } else { "editor" }}),
    )
}

#[tauri::command]
pub async fn text_extractor_capture_selection(
    app: tauri::AppHandle,
    window: WebviewWindow,
    crop: Crop,
) -> Result<Value, String> {
    let (monitor, cancel) = {
        let state = app.state::<TextExtractor>();
        let mut active = state.session.lock().unwrap();
        let session = active
            .as_mut()
            .filter(|s| s.label == window.label())
            .ok_or("This capture has ended.")?;
        if session.mode != CaptureMode::Editor {
            return Err("Use the editor shortcut for this action.".into());
        }
        crop.validate(session.monitor.width, session.monitor.height)?;
        if session.capturing || session.image.is_some() {
            return Err("This selection is already captured.".into());
        }
        session.capturing = true;
        (session.monitor, session.cancel.clone())
    };
    let result = tokio::select! {
        _ = cancel.cancelled() => return Err("Capture cancelled.".into()),
        result = tauri::async_runtime::spawn_blocking(move || {
            let image = capture::snapshot(monitor)?;
            let selection = image::imageops::crop_imm(&image, crop.x, crop.y, crop.width, crop.height).to_image();
            // The backdrop is blurred, so transfer a small thumbnail instead of a full monitor PNG.
            let backdrop = image::imageops::thumbnail(&image, 1200, 800);
            let payload = json!({"imageUrl": png_url(&selection)?, "backdropUrl": png_url(&backdrop)?});
            Ok::<_, String>((Arc::new(image), payload))
        }) => result.map_err(|_| "Screen capture stopped unexpectedly.".to_string()).and_then(|result| result),
    };
    let state = app.state::<TextExtractor>();
    let mut active = state.session.lock().unwrap();
    let session = active
        .as_mut()
        .filter(|s| s.label == window.label() && !s.cancel.is_cancelled())
        .ok_or("Capture cancelled.")?;
    session.capturing = false;
    let (image, payload) = result?;
    session.image = Some(image);
    session.crop = Some(crop);
    Ok(payload)
}

#[tauri::command]
pub async fn text_extractor_quick_copy(
    app: tauri::AppHandle,
    window: WebviewWindow,
    crop: Crop,
) -> Result<(), String> {
    let (monitor, cancel) = {
        let state = app.state::<TextExtractor>();
        let mut active = state.session.lock().unwrap();
        let session = active
            .as_mut()
            .filter(|s| s.label == window.label())
            .ok_or("This capture has ended.")?;
        if session.mode != CaptureMode::Quick || session.capturing {
            return Err("This quick copy has already started.".into());
        }
        crop.validate(session.monitor.width, session.monitor.height)?;
        session.capturing = true;
        (session.monitor, session.cancel.clone())
    };
    // Selection is finished. Return focus immediately; no image or result UI is sent to the webview.
    let _ = window.hide();
    let recognized = tokio::select! {
        _ = cancel.cancelled() => return Err("Capture cancelled.".into()),
        result = tauri::async_runtime::spawn_blocking(move || ocr::recognize(capture::selection(monitor, crop)?)) => result.map_err(|_| "Windows text recognition stopped unexpectedly.".to_string()).and_then(|result| result),
    };
    let result = (|| {
        let state = app.state::<TextExtractor>();
        let active = state.session.lock().unwrap();
        let session = active
            .as_ref()
            .filter(|s| s.label == window.label() && !s.cancel.is_cancelled())
            .ok_or("Capture cancelled.")?;
        let text = recognized?;
        if text.trim().is_empty() {
            return Err("No text was found. Select another area.".into());
        }
        if session.mode != CaptureMode::Quick {
            return Err("This capture has ended.".into());
        }
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(text))
            .map_err(|_| "The clipboard is busy. Try quick copy again.".into())
    })();
    // A cancelled old request must never close a newer capture.
    if close_matching(&app, window.label(), false) {
        queue_prewarm(&app);
        if let Err(error) = &result {
            report_error(&app, format!("Quick copy: {error}"));
        }
    }
    result
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
        if session.crop != Some(crop) {
            return Err("Start a new selection to read a different area.".into());
        }
        let image = session.image.clone().ok_or("Select an area first.")?;
        (image, begin_request(session, request_id)?)
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
    if !close_matching(&app, window.label(), false) {
        return Err("This capture has ended.".into());
    }
    queue_prewarm(&app);
    Ok(())
}

fn dispose_session(app: &tauri::AppHandle, session: Session) {
    session.cancel.cancel();
    session.request_cancel.cancel();
    if let Some(window) = app.get_webview_window(&session.label) {
        let _ = window.destroy();
    }
}

fn take_matching(active: &mut Option<Session>, label: &str, only_unshown: bool) -> Option<Session> {
    if active
        .as_ref()
        .is_some_and(|s| s.label == label && (!only_unshown || !s.shown))
    {
        active.take()
    } else {
        None
    }
}

fn close_matching(app: &tauri::AppHandle, label: &str, only_unshown: bool) -> bool {
    let session = take_matching(
        &mut app.state::<TextExtractor>().session.lock().unwrap(),
        label,
        only_unshown,
    );
    if let Some(session) = session {
        dispose_session(app, session);
        true
    } else {
        false
    }
}

fn close_active(app: &tauri::AppHandle) {
    let session = app.state::<TextExtractor>().session.lock().unwrap().take();
    if let Some(session) = session {
        dispose_session(app, session);
    }
}

pub fn window_destroyed(app: &tauri::AppHandle, label: &str) {
    if !label.starts_with(WINDOW_PREFIX) {
        return;
    }
    let state = app.state::<TextExtractor>();
    let removed = take_matching(&mut state.session.lock().unwrap(), label, false);
    let was_active = removed.is_some();
    if let Some(session) = removed {
        session.cancel.cancel();
        session.request_cancel.cancel();
    }
    let removed_warm = {
        let mut warm = state.warm.lock().unwrap();
        if warm.as_ref().is_some_and(|warm| warm.label == label) {
            warm.take().is_some()
        } else {
            false
        }
    };
    if was_active || removed_warm {
        queue_prewarm(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            label: "text-extractor-test".into(),
            mode: CaptureMode::Editor,
            monitor: capture::Monitor {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            image: Some(Arc::new(RgbaImage::new(4, 4))),
            crop: None,
            capturing: false,
            cancel: CancellationToken::new(),
            request_id: 0,
            request_cancel: CancellationToken::new(),
            shown: true,
        }
    }

    #[test]
    fn obsolete_closes_and_watchdogs_cannot_remove_a_new_or_visible_capture() {
        let mut active = Some(session());
        assert!(take_matching(&mut active, "old-window", false).is_none());
        assert!(take_matching(&mut active, "text-extractor-test", true).is_none());
        assert!(active.is_some());
        active.as_mut().unwrap().shown = false;
        assert!(take_matching(&mut active, "text-extractor-test", true).is_some());
        assert!(active.is_none());
    }

    #[test]
    fn mode_switch_cancels_old_work_and_rejects_out_of_order_requests() {
        let mut session = session();
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
