use super::{
    advanced,
    codex::Codex,
    google_translate,
    local_ocr::ModelStatus,
    pixels::DesktopFrame,
    platform::{self, capture, LocalOcr},
    protocol::{self, AdvancedProvider, Crop, ExtractionMode, Preferences, QuickCopyOutcome},
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
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tokio_util::sync::CancellationToken;

const WINDOW_PREFIX: &str = "text-extractor-";
const NOTICE_PREFIX: &str = "quick-copy-notice-";

pub struct TextExtractor {
    preferences: Mutex<Preferences>,
    local_ocr: Arc<LocalOcr>,
    codex: Arc<Codex>,
    path: PathBuf,
    session: Mutex<Option<Session>>,
    warm: Mutex<Option<WarmWindow>>,
    preparing: tokio::sync::Mutex<()>,
    starting: AtomicBool,
    recording_shortcut: AtomicBool,
    next_id: AtomicU64,
    error: Mutex<Option<String>>,
}

struct WarmWindow {
    label: String,
    ready: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum CaptureMode {
    Quick,
    Editor,
}

struct Session {
    label: String,
    mode: CaptureMode,
    default_mode: ExtractionMode,
    monitor: capture::Monitor,
    image: Option<Arc<DesktopFrame>>,
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
        return Err(platform::SHORTCUT_HINT.into());
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
    let preferences = fs::read(&path)
        .ok()
        .and_then(|bytes| load_preferences(&bytes).ok())
        .unwrap_or_default();
    let _ = save(&path, &preferences);
    app.manage(TextExtractor {
        preferences: Mutex::new(preferences.clone()),
        local_ocr: LocalOcr::new(app.path().app_local_data_dir()?),
        codex: Codex::new(app.path().app_local_data_dir()?),
        path,
        session: Mutex::new(None),
        warm: Mutex::new(None),
        preparing: tokio::sync::Mutex::new(()),
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
                    let state = app.state::<TextExtractor>();
                    if state.recording_shortcut.load(Ordering::Acquire) {
                        report_recorded_shortcut(&app, &key.to_string());
                        return;
                    }
                    let mode = {
                        let prefs = state.preferences.lock().unwrap();
                        if !prefs.enabled {
                            None
                        } else if shortcut(&prefs.shortcut)
                            .is_ok_and(|registered| registered == key)
                        {
                            Some(CaptureMode::Editor)
                        } else if shortcut(&prefs.quick_shortcut)
                            .is_ok_and(|registered| registered == key)
                        {
                            Some(CaptureMode::Quick)
                        } else {
                            None
                        }
                    };
                    if let Some(mode) = mode {
                        if let Err(error) = start(&app, mode).await {
                            *app.state::<TextExtractor>().error.lock().unwrap() = Some(error);
                            let _ = crate::open_settings(&app);
                        }
                    }
                });
            })
            .build(),
    )?;
    if let Err(error) = platform::install_shortcuts(app) {
        let state = app.state::<TextExtractor>();
        state.preferences.lock().unwrap().enabled = false;
        *state.error.lock().unwrap() = Some(error);
    }
    if app
        .state::<TextExtractor>()
        .preferences
        .lock()
        .unwrap()
        .enabled
    {
        let keys = registered_shortcuts(&preferences)?;
        if let Err(reason) = change_registered_shortcuts(app, &[], &keys, || Ok(())) {
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
    platform::configure_shortcuts(enabled, shortcut_pair(&preferences)?);
    app.state::<TextExtractor>().local_ocr.set_enabled(enabled);
    app.state::<TextExtractor>().codex.refresh(false);
    queue_prewarm(app);
    Ok(())
}

#[tauri::command]
pub fn text_extractor_state(app: tauri::AppHandle, window: WebviewWindow) -> Result<Value, String> {
    settings_only(&window)?;
    let state = app.state::<TextExtractor>();
    state.codex.refresh(false);
    let credential_state = openai_credentials::is_configured();
    let configured = credential_state.as_ref().copied().unwrap_or(false);
    let preferences = state.preferences.lock().unwrap().clone();
    let provider = active_provider(preferences.advanced_provider, &state.codex.status());
    let error = state.error.lock().unwrap().clone().or_else(|| {
        if provider == AdvancedProvider::Api {
            credential_state.err()
        } else {
            None
        }
    });
    let mut result = advanced_access(&state, &preferences, configured);
    result.as_object_mut().unwrap().extend(
        json!({"enabled": preferences.enabled, "shortcut": preferences.shortcut, "quickShortcut": preferences.quick_shortcut,
        "editorMode": preferences.editor_mode, "quickMode": preferences.quick_mode,
        "apiKeyConfigured": configured, "error": error, "localOcr": state.local_ocr.status(),
        "captureAccess": platform::capture_access()}).as_object().unwrap().clone(),
    );
    Ok(result)
}

fn active_provider(preferred: AdvancedProvider, status: &super::codex::Status) -> AdvancedProvider {
    // Wait for discovery before falling back, so startup never sends a request
    // to the paid API while the preferred local Codex is still being located.
    if preferred == AdvancedProvider::Codex && (status.installed || status.checking) {
        AdvancedProvider::Codex
    } else {
        AdvancedProvider::Api
    }
}

fn advanced_access(state: &TextExtractor, preferences: &Preferences, api_key: bool) -> Value {
    let codex = state.codex.status();
    let provider = active_provider(preferences.advanced_provider, &codex);
    let available = match provider {
        AdvancedProvider::Codex => codex.available,
        AdvancedProvider::Api => api_key,
    };
    json!({"advancedProvider": preferences.advanced_provider, "activeAdvancedProvider": provider,
        "advancedAvailable": available, "codex": codex})
}

fn advanced_available(state: &TextExtractor) -> bool {
    let preferred = state.preferences.lock().unwrap().advanced_provider;
    let status = state.codex.status();
    match active_provider(preferred, &status) {
        AdvancedProvider::Codex => status.available,
        AdvancedProvider::Api => openai_credentials::is_configured().unwrap_or(false),
    }
}

#[tauri::command]
pub fn text_extractor_advanced_access(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<Value, String> {
    settings_only(&window)?;
    let state = app.state::<TextExtractor>();
    let preferences = state.preferences.lock().unwrap().clone();
    // Poll only public Codex metadata; do not reopen the system key store on a timer.
    let codex = state.codex.status();
    Ok(json!({"advancedProvider": preferences.advanced_provider,
        "activeAdvancedProvider": active_provider(preferences.advanced_provider, &codex),
        "codex": codex}))
}

#[tauri::command]
pub fn refresh_text_extractor_codex(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<(), String> {
    settings_only(&window)?;
    app.state::<TextExtractor>().codex.refresh(true);
    Ok(())
}

#[tauri::command]
pub fn set_text_extractor_provider(
    app: tauri::AppHandle,
    window: WebviewWindow,
    provider: AdvancedProvider,
) -> Result<(), String> {
    settings_only(&window)?;
    let state = app.state::<TextExtractor>();
    if provider == AdvancedProvider::Codex && !state.codex.status().installed {
        return Err("Codex is not installed on this device.".into());
    }
    let mut current = state.preferences.lock().unwrap();
    let mut next = current.clone();
    next.advanced_provider = provider;
    save(&state.path, &next)?;
    *current = next;
    Ok(())
}

#[tauri::command]
pub async fn request_text_extractor_access(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<(), String> {
    settings_only(&window)?;
    platform::request_capture_access(&app).await
}

#[tauri::command]
pub fn local_ocr_state(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<ModelStatus, String> {
    settings_only(&window)?;
    Ok(app.state::<TextExtractor>().local_ocr.status())
}

#[tauri::command]
pub fn retry_local_ocr_setup(app: tauri::AppHandle, window: WebviewWindow) -> Result<(), String> {
    settings_only(&window)?;
    app.state::<TextExtractor>().local_ocr.retry()
}

#[tauri::command]
pub async fn save_openai_api_key(window: WebviewWindow, api_key: String) -> Result<(), String> {
    settings_only(&window)?;
    openai_credentials::validate_and_save(&api_key).await
}

#[tauri::command]
pub async fn clear_openai_api_key(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<(), String> {
    settings_only(&window)?;
    openai_credentials::clear()?;
    #[cfg(target_os = "macos")]
    crate::dictation::set_dictation_enabled(app, window, false).await?;
    #[cfg(not(target_os = "macos"))]
    let _ = app;
    Ok(())
}

#[tauri::command]
pub fn record_text_extractor_shortcut(
    app: tauri::AppHandle,
    window: WebviewWindow,
    recording: bool,
) -> Result<(), String> {
    settings_only(&window)?;
    platform::recording(recording);
    app.state::<TextExtractor>()
        .recording_shortcut
        .store(recording, Ordering::Release);
    Ok(())
}

pub fn settings_blurred(app: &tauri::AppHandle) {
    platform::recording(false);
    app.state::<TextExtractor>()
        .recording_shortcut
        .store(false, Ordering::Release);
}

#[cfg(target_os = "windows")]
pub(crate) fn shortcuts_stopped(app: &tauri::AppHandle) {
    let state = app.state::<TextExtractor>();
    *state.error.lock().unwrap() =
        Some("Text Extractor shortcuts stopped. Restart Pulse to enable them again.".into());
}

fn shortcut_pair(preferences: &Preferences) -> Result<[Shortcut; 2], String> {
    let keys = [
        shortcut(&preferences.shortcut)?,
        shortcut(&preferences.quick_shortcut)?,
    ];
    if keys[0] == keys[1] {
        return Err("Choose different shortcuts for Quick copy and Open editor.".into());
    }
    Ok(keys)
}

fn load_preferences(bytes: &[u8]) -> Result<Preferences, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| "Could not read Text Extractor settings.")?;
    let legacy = value.get("quickShortcut").is_none();
    let mut preferences: Preferences =
        serde_json::from_value(value).map_err(|_| "Could not read Text Extractor settings.")?;
    // Only legacy files need migration. New custom assignments must survive restarts.
    if legacy
        && shortcut(&preferences.shortcut).is_ok_and(|key| {
            key == shortcut("Control+Shift+E").unwrap()
                || key == shortcut(protocol::QUICK_SHORTCUT).unwrap()
        })
    {
        preferences.shortcut = protocol::DEFAULT_SHORTCUT.into();
    }
    shortcut_pair(&preferences)?;
    Ok(preferences)
}

fn registered_shortcuts(preferences: &Preferences) -> Result<Vec<Shortcut>, String> {
    let keys = shortcut_pair(preferences)?;
    Ok(keys
        .into_iter()
        .filter(|key| preferences.enabled && !platform::owns_shortcut(key))
        .collect())
}

fn change_registered_shortcuts(
    app: &tauri::AppHandle,
    old: &[Shortcut],
    next: &[Shortcut],
    persist: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    transition_shortcuts(
        old,
        next,
        |key| {
            app.global_shortcut()
                .register(*key)
                .map_err(|_| "This shortcut is already in use. Choose another combination.".into())
        },
        |key| {
            app.global_shortcut()
                .unregister(*key)
                .map_err(|_| "Could not release the previous shortcut. Try again.".into())
        },
        persist,
    )
}

fn transition_shortcuts(
    old: &[Shortcut],
    next: &[Shortcut],
    mut register: impl FnMut(&Shortcut) -> Result<(), String>,
    mut unregister: impl FnMut(&Shortcut) -> Result<(), String>,
    persist: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let result = (|| {
        // Reserve every new combination before releasing any working combination.
        for key in next.iter().filter(|key| !old.contains(key)) {
            register(key)?;
            added.push(key);
        }
        for key in old.iter().filter(|key| !next.contains(key)) {
            unregister(key)?;
            removed.push(key);
        }
        persist()
    })();
    if result.is_err() {
        for key in added {
            let _ = unregister(key);
        }
        for key in removed {
            let _ = register(key);
        }
    }
    result
}

pub(crate) fn report_recorded_shortcut(app: &tauri::AppHandle, shortcut: &str) {
    if let Some(window) = app.get_webview_window("settings") {
        let detail = json!(shortcut);
        let _ = window.eval(format!("window.dispatchEvent(new CustomEvent('pulse-extractor-shortcut', {{detail: {detail}}}))"));
    }
}

pub(crate) fn report_error(app: &tauri::AppHandle, error: String) {
    *app.state::<TextExtractor>().error.lock().unwrap() = Some(error);
    let _ = crate::open_settings(app);
}

#[tauri::command]
pub async fn set_text_extractor(
    app: tauri::AppHandle,
    window: WebviewWindow,
    enabled: bool,
    shortcut_value: String,
    quick_shortcut_value: String,
    editor_mode: ExtractionMode,
    quick_mode: ExtractionMode,
) -> Result<(), String> {
    settings_only(&window)?;
    if enabled {
        platform::ensure_supported()?;
    }
    let state = app.state::<TextExtractor>();
    let mut current = state.preferences.lock().unwrap();
    let mut next = Preferences {
        enabled,
        shortcut: shortcut_value,
        quick_shortcut: quick_shortcut_value,
        editor_mode,
        quick_mode,
        advanced_provider: current.advanced_provider,
    };
    let keys = shortcut_pair(&next)?;
    next.shortcut = keys[0].to_string();
    next.quick_shortcut = keys[1].to_string();
    if enabled && !platform::shortcuts_available() {
        return Err(
            "Could not install Text Extractor shortcuts. Restart Pulse and try again.".into(),
        );
    }
    change_registered_shortcuts(
        &app,
        &registered_shortcuts(&current)?,
        &registered_shortcuts(&next)?,
        || save(&state.path, &next),
    )?;
    *current = next;
    platform::configure_shortcuts(enabled, keys);
    *state.error.lock().unwrap() = None;
    drop(current);
    state.local_ocr.set_enabled(enabled);
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

pub fn appearance_changed(app: &tauri::AppHandle, theme: tauri::Theme) {
    let state = app.state::<TextExtractor>();
    let label = state
        .session
        .lock()
        .unwrap()
        .as_ref()
        .map(|session| session.label.clone());
    if let Some(window) = label.and_then(|label| app.get_webview_window(&label)) {
        if let Err(error) = window.set_theme(crate::native_window_theme(theme)) {
            eprintln!("Text Extractor appearance update failed: {error}");
        }
    }
    for window in app.webview_windows().into_values() {
        if window.label().starts_with(NOTICE_PREFIX) || window.label() == "settings" {
            let _ = window.set_theme(crate::native_window_theme(theme));
        }
    }
}

fn dismiss_notices(app: &tauri::AppHandle) {
    for window in app.webview_windows().into_values() {
        if window.label().starts_with(NOTICE_PREFIX) {
            let _ = window.destroy();
        }
    }
}

fn show_quick_copy_notice(
    app: &tauri::AppHandle,
    monitor: capture::Monitor,
    outcome: &QuickCopyOutcome,
) -> Result<(), String> {
    dismiss_notices(app);
    let label = format!(
        "{NOTICE_PREFIX}{}",
        app.state::<TextExtractor>()
            .next_id
            .fetch_add(1, Ordering::Relaxed)
    );
    WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::App("quick-copy-notice.html".into()),
    )
    .title("Pulse Quick Copy Notice")
    .initialization_script(if matches!(outcome, QuickCopyOutcome::Copied) {
        "window.pulseQuickCopySucceeded = true;"
    } else {
        "window.pulseQuickCopySucceeded = false;"
    })
    .theme(crate::native_window_theme(crate::visual_app_theme(app)?))
    .visible(false)
    .focused(false)
    .focusable(false)
    .transparent(true)
    .background_color(tauri::window::Color(0, 0, 0, 0))
    .decorations(false)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .on_page_load(move |window, payload| {
        if payload.event() != tauri::webview::PageLoadEvent::Finished {
            return;
        }
        // A new selection may have started while this webview was loading.
        if window
            .state::<TextExtractor>()
            .session
            .lock()
            .unwrap()
            .is_some()
        {
            let _ = window.destroy();
            return;
        }
        let positioned = platform::position_notice(&window, monitor);
        if let Err(error) = positioned {
            eprintln!("Quick Copy notice: {error}");
            let _ = window.destroy();
            return;
        }
        let _ = window.show();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let _ = window.destroy();
        });
    })
    .build()
    .map_err(|_| "Could not show the Quick Copy notice.")?;
    Ok(())
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
    let created = async {
        let window =
            WebviewWindowBuilder::new(app, &label, WebviewUrl::App("text-extractor.html".into()))
                .title("Pulse Text Extractor")
                .theme(crate::native_window_theme(crate::visual_app_theme(app)?))
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
        platform::configure_selector(&window).await?;
        Ok::<(), String>(())
    }
    .await;
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

pub(crate) async fn start(app: &tauri::AppHandle, mode: CaptureMode) -> Result<(), String> {
    let state = app.state::<TextExtractor>();
    if state.starting.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let _starting = StartingGuard(&state.starting);
    let active_label = state.session.lock().unwrap().as_ref().map(|session| {
        (
            session.label.clone(),
            session.mode == CaptureMode::Quick && session.capturing,
        )
    });
    if let Some((label, copying)) = active_label {
        if copying {
            // A fresh shortcut supersedes pending Quick Copy, including a slow network request.
            close_matching(app, &label, false);
        } else {
            if let Some(window) = app.get_webview_window(&label) {
                let _ = window.set_focus();
            }
            return Ok(());
        }
    }
    if !state.preferences.lock().unwrap().enabled {
        return Ok(());
    }
    dismiss_notices(app);
    // Only monitor geometry is queried on the shortcut path; no pixels or key reads.
    platform::ensure_capture_access()?;
    let monitor = platform::monitor_at_pointer(app).await?;
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
    platform::position_selector(&window, monitor).await?;
    let (label, ready) = {
        let preferences = state.preferences.lock().unwrap();
        if !preferences.enabled {
            return Ok(());
        }
        let mut warm = state.warm.lock().unwrap();
        if warm
            .as_ref()
            .is_none_or(|warm| warm.label != prepared_label)
        {
            return Err("Text Extractor was disabled. Try again.".into());
        }
        let prepared = warm.take().unwrap();
        *state.session.lock().unwrap() = Some(Session {
            label: prepared.label.clone(),
            mode,
            default_mode: match mode {
                CaptureMode::Quick => preferences.quick_mode,
                CaptureMode::Editor => preferences.editor_mode,
            },
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
        json!({"width": session.monitor.width, "height": session.monitor.height, "defaultMode": session.default_mode, "mode": if session.mode == CaptureMode::Quick { "quick" } else { "editor" }}),
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
            let selection = image.crop(crop)?;
            let payload = json!({"imageUrl": png_url(&selection)?});
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
pub async fn text_extractor_backdrop(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<String, String> {
    let (image, cancel) = {
        let state = app.state::<TextExtractor>();
        let active = state.session.lock().unwrap();
        let session = active
            .as_ref()
            .filter(|s| s.label == window.label())
            .ok_or("This capture has ended.")?;
        (
            session.image.clone().ok_or("Select an area first.")?,
            session.cancel.clone(),
        )
    };
    // Decoration must not delay the selected pixels reaching their first animation frame.
    tokio::select! {
        _ = cancel.cancelled() => Err("Capture cancelled.".into()),
        result = tauri::async_runtime::spawn_blocking(move || {
            png_url(&image.backdrop())
        }) => result.map_err(|_| "Could not prepare the backdrop.".to_string())?,
    }
}

#[tauri::command]
pub async fn text_extractor_quick_copy(
    app: tauri::AppHandle,
    window: WebviewWindow,
    crop: Crop,
) -> Result<(), String> {
    let (monitor, default_mode, cancel) = {
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
        (
            session.monitor,
            session.default_mode,
            session.cancel.clone(),
        )
    };
    // Selection is finished. Return focus immediately; no image or result UI is sent to the webview.
    let _ = window.hide();
    let recognized = tokio::select! {
        _ = cancel.cancelled() => return Err("Capture cancelled.".into()),
        result = async {
            let image = tauri::async_runtime::spawn_blocking(move || capture::selection(monitor, crop))
                .await.map_err(|_| "Screen capture stopped unexpectedly.".to_string())??;
            // Keep provider checks off the shortcut-to-selector path. Basic
            // remains usable when the selected Advanced provider is unavailable.
            let mode = default_mode.with_advanced_available(advanced_available(&app.state::<TextExtractor>()));
            recognize_selection(&app, image, mode, cancel.clone()).await
        } => result,
    };
    let result = (|| {
        let state = app.state::<TextExtractor>();
        let active = state.session.lock().unwrap();
        let session = active
            .as_ref()
            .filter(|s| s.label == window.label() && !s.cancel.is_cancelled())
            .ok_or("Capture cancelled.")?;
        if session.mode != CaptureMode::Quick {
            return Err("This capture has ended.".into());
        }
        protocol::copy_recognized_text(recognized?, |text| {
            arboard::Clipboard::new()
                .and_then(|mut clipboard| clipboard.set_text(text))
                .map_err(|_| "The clipboard is busy. Try quick copy again.".into())
        })
    })();
    // A cancelled old request must never close a newer capture.
    if close_matching(&app, window.label(), false) {
        queue_prewarm(&app);
        match &result {
            Ok(outcome) => {
                if let Err(error) = show_quick_copy_notice(&app, monitor, outcome) {
                    eprintln!("Quick Copy notice: {error}");
                }
            }
            Err(error) => report_error(&app, format!("Quick copy: {error}")),
        }
    }
    result.map(|_| ())
}

#[tauri::command]
pub fn text_extractor_show(app: tauri::AppHandle, window: WebviewWindow) -> Result<(), String> {
    ensure_session(&app, &window)?;
    // Re-read Pulse's visual theme in case it changed while the capture loaded.
    window
        .set_theme(crate::native_window_theme(crate::visual_app_theme(&app)?))
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
    Ok(advanced_available(&app.state::<TextExtractor>()))
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
            let selection = tauri::async_runtime::spawn_blocking(move || image.crop(crop))
                .await.map_err(|_| "Could not prepare this selection.")??;
            let mode = if mode == "basic" { ExtractionMode::Basic } else { ExtractionMode::Advanced };
            recognize_selection(&app, selection, mode, cancel.clone()).await
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
    use_advanced: Option<bool>,
) -> Result<String, String> {
    protocol::translation_target(&text, &language)?;
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
        result = async {
            if use_advanced.unwrap_or(false) {
                // Switching providers is an explicit user action, never an automatic paid retry.
                request_advanced(&app, protocol::translation_body(&text, &language)?, cancel.clone()).await
            } else {
                google_translate::request(&text, &language, cancel.clone()).await
            }
        } => result,
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

async fn recognize_selection(
    app: &tauri::AppHandle,
    image: RgbaImage,
    mode: ExtractionMode,
    cancel: CancellationToken,
) -> Result<String, String> {
    match mode {
        ExtractionMode::Basic => {
            let engine = app.state::<TextExtractor>().local_ocr.clone();
            tauri::async_runtime::spawn_blocking(move || engine.recognize(image, &cancel))
                .await
                .map_err(|_| "Offline text recognition stopped unexpectedly.".to_string())?
        }
        ExtractionMode::Advanced => {
            let image_url = tauri::async_runtime::spawn_blocking(move || png_url(&image))
                .await
                .map_err(|_| "Could not prepare this selection.")??;
            request_advanced(app, protocol::request_body(&image_url), cancel).await
        }
    }
}

async fn request_advanced(
    app: &tauri::AppHandle,
    body: Value,
    cancel: CancellationToken,
) -> Result<String, String> {
    let (provider, codex) = {
        let state = app.state::<TextExtractor>();
        let preferred = state.preferences.lock().unwrap().advanced_provider;
        (
            active_provider(preferred, &state.codex.status()),
            state.codex.clone(),
        )
    };
    // Never silently switch to API billing after a Codex authentication, model,
    // or usage-limit error. The user can explicitly select API key in Settings.
    match provider {
        AdvancedProvider::Codex => codex.request(body, cancel).await,
        AdvancedProvider::Api => advanced::request(body, cancel).await,
    }
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
    platform::capture_closed();
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
        platform::capture_closed();
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
            default_mode: ExtractionMode::Basic,
            #[cfg(target_os = "macos")]
            monitor: capture::Monitor {
                width: 4,
                height: 4,
                scale: 1.0,
                ..Default::default()
            },
            #[cfg(target_os = "windows")]
            monitor: capture::Monitor {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            image: Some(Arc::new(DesktopFrame::new(4, 4, vec![0; 64]).unwrap())),
            crop: None,
            capturing: false,
            cancel: CancellationToken::new(),
            request_id: 0,
            request_cancel: CancellationToken::new(),
            shown: true,
        }
    }

    #[test]
    fn legacy_preferences_migrate_once_and_custom_assignments_survive() {
        for editor in ["Control+Shift+E", protocol::QUICK_SHORTCUT] {
            let bytes = serde_json::to_vec(&json!({"enabled": true, "shortcut": editor})).unwrap();
            let prefs = load_preferences(&bytes).unwrap();
            assert!(prefs.enabled);
            assert_eq!(prefs.shortcut, protocol::DEFAULT_SHORTCUT);
            assert_eq!(prefs.quick_shortcut, protocol::QUICK_SHORTCUT);
        }
        let prefs = Preferences {
            enabled: true,
            shortcut: protocol::QUICK_SHORTCUT.into(),
            quick_shortcut: "Control+Alt+Q".into(),
            ..Preferences::default()
        };
        let restored = load_preferences(&serde_json::to_vec(&prefs).unwrap()).unwrap();
        assert_eq!(restored.shortcut, prefs.shortcut);
        assert_eq!(restored.quick_shortcut, prefs.quick_shortcut);
        assert_eq!(
            shortcut_pair(&restored).unwrap()[0],
            shortcut(protocol::QUICK_SHORTCUT).unwrap()
        );
    }

    #[test]
    fn duplicate_shortcuts_are_rejected_regardless_of_modifier_order() {
        let prefs = Preferences {
            enabled: true,
            shortcut: "Shift+Super+KeyT".into(),
            quick_shortcut: "Super+Shift+T".into(),
            ..Preferences::default()
        };
        assert!(shortcut_pair(&prefs).is_err());
    }

    #[test]
    fn shortcut_registration_conflict_preserves_both_previous_assignments() {
        use std::cell::RefCell;
        let old = [
            shortcut("Control+Alt+E").unwrap(),
            shortcut("Control+Alt+Q").unwrap(),
        ];
        let next = [
            shortcut("Control+Alt+R").unwrap(),
            shortcut("Control+Alt+W").unwrap(),
        ];
        let active = RefCell::new(old.to_vec());
        let result = transition_shortcuts(
            &old,
            &next,
            |key| {
                if *key == next[1] {
                    return Err("Occupied".into());
                }
                active.borrow_mut().push(*key);
                Ok(())
            },
            |key| {
                active.borrow_mut().retain(|k| k != key);
                Ok(())
            },
            || panic!("Conflicting shortcuts must never be persisted"),
        );
        assert!(result.is_err());
        assert_eq!(*active.borrow(), old);
    }

    #[test]
    fn failed_settings_write_restores_previous_shortcuts() {
        use std::cell::RefCell;
        let old = [
            shortcut("Control+Alt+E").unwrap(),
            shortcut("Control+Alt+Q").unwrap(),
        ];
        let next = [old[0], shortcut("Control+Alt+W").unwrap()];
        let active = RefCell::new(old.to_vec());
        let result = transition_shortcuts(
            &old,
            &next,
            |key| {
                active.borrow_mut().push(*key);
                Ok(())
            },
            |key| {
                active.borrow_mut().retain(|k| k != key);
                Ok(())
            },
            || Err("Settings folder is not writable".into()),
        );
        assert!(result.is_err());
        assert_eq!(*active.borrow(), old);
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

    #[test]
    fn codex_selection_never_falls_back_to_api_while_installed_or_discovering() {
        let mut status = super::super::codex::Status {
            installed: false,
            available: false,
            checking: true,
            message: String::new(),
        };
        assert_eq!(
            active_provider(AdvancedProvider::Codex, &status),
            AdvancedProvider::Codex
        );
        status.checking = false;
        assert_eq!(
            active_provider(AdvancedProvider::Codex, &status),
            AdvancedProvider::Api
        );
        status.installed = true;
        assert_eq!(
            active_provider(AdvancedProvider::Codex, &status),
            AdvancedProvider::Codex
        );
        assert_eq!(
            active_provider(AdvancedProvider::Api, &status),
            AdvancedProvider::Api
        );
    }

    #[test]
    fn preferences_default_to_api_and_preserve_explicit_codex() {
        let preferences: Preferences =
            serde_json::from_value(json!({"enabled": true, "quickMode": "advanced"})).unwrap();
        assert_eq!(preferences.advanced_provider, AdvancedProvider::Api);
        assert_eq!(preferences.quick_mode, ExtractionMode::Advanced);
        assert_eq!(preferences.editor_mode, ExtractionMode::Basic);
        let mut explicit = preferences;
        explicit.advanced_provider = AdvancedProvider::Codex;
        let restored: Preferences =
            serde_json::from_slice(&serde_json::to_vec(&explicit).unwrap()).unwrap();
        assert_eq!(restored.advanced_provider, AdvancedProvider::Codex);
    }
}
