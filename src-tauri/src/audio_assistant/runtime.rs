use super::{
    model::{self, Lifecycle, Preferences},
    transcription,
};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, sync::Mutex, time::Instant};
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

const OVERLAY: &str = "audio-assistant";
pub struct AudioAssistant {
    pub live: Mutex<Lifecycle>,
    preferences: Mutex<Preferences>,
    path: PathBuf,
    error: Mutex<Option<String>>,
    transition: tokio::sync::Mutex<()>,
    stream: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    answer_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    escape_registered: Mutex<bool>,
    registered: Mutex<Vec<Shortcut>>,
}
fn settings_only(window: &WebviewWindow) -> Result<(), String> {
    if window.label() != "settings" {
        return Err("Open Settings to do this.".into());
    }
    Ok(())
}
fn save(state: &AudioAssistant, prefs: &Preferences) -> Result<(), String> {
    if let Some(parent) = state.path.parent() {
        fs::create_dir_all(parent).map_err(|_| "Could not create the settings folder.")?;
    }
    fs::write(
        &state.path,
        serde_json::to_vec(prefs).map_err(|_| "Could not encode audio assistant settings.")?,
    )
    .map_err(|_| "Could not save audio assistant settings.".into())
}
fn notify(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.eval("window.dispatchEvent(new Event('pulse-audio-assistant-changed'))");
    }
}
pub(super) fn publish_error(app: &tauri::AppHandle, generation: u64, message: &str) {
    let state = app.state::<AudioAssistant>();
    if !state.live.lock().unwrap().accepts(generation) {
        return;
    }
    *state.error.lock().unwrap() = (!message.is_empty()).then(|| message.into());
    notify(app);
}
fn report_error(app: &tauri::AppHandle, error: String) {
    *app.state::<AudioAssistant>().error.lock().unwrap() = Some(error);
    notify(app);
    let _ = crate::open_settings(app);
}

pub fn install(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let path = app.path().app_config_dir()?.join("audio-assistant.json");
    let prefs = fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Preferences>(&b).ok())
        .filter(|p| p.shortcuts().is_ok())
        .unwrap_or_default();
    app.manage(AudioAssistant {
        live: Mutex::new(Lifecycle::default()),
        preferences: Mutex::new(prefs.clone()),
        path,
        error: Mutex::new(None),
        transition: tokio::sync::Mutex::new(()),
        stream: Mutex::new(None),
        answer_task: Mutex::new(None),
        escape_registered: Mutex::new(false),
        registered: Mutex::new(Vec::new()),
    });
    let state = app.state::<AudioAssistant>();
    let result = register_shortcuts(app, &prefs);
    if let Err(error) = result {
        *state.error.lock().unwrap() = Some(error);
    }
    if prefs.listening {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = set_listening(&app, Some(true)).await {
                report_error(&app, error);
            }
        });
    }
    Ok(())
}
fn register_shortcuts(app: &tauri::AppHandle, next: &Preferences) -> Result<(), String> {
    let keys = next.shortcuts()?;
    for key in keys {
        if crate::text_extractor::shortcut_reserved(app, key) {
            return Err(
                "This shortcut is used by Text Extractor. Choose another combination.".into(),
            );
        }
    }
    let state = app.state::<AudioAssistant>();
    let mut registered = state.registered.lock().unwrap();
    crate::text_extractor::change_registered_shortcuts(app, &registered, &keys, || {
        save(&state, next)
    })?;
    *registered = keys.to_vec();
    Ok(())
}
pub(crate) fn shortcut_reserved(app: &tauri::AppHandle, key: Shortcut) -> bool {
    app.try_state::<AudioAssistant>().is_some_and(|s| {
        s.preferences
            .lock()
            .unwrap()
            .shortcuts()
            .is_ok_and(|keys| keys.contains(&key))
    })
}

/// Called by the existing global shortcut dispatcher after its recorder gate.
pub(crate) async fn dispatch(app: &tauri::AppHandle, key: Shortcut) -> bool {
    let Some(state) = app.try_state::<AudioAssistant>() else {
        return false;
    };
    if key == "Escape".parse::<Shortcut>().unwrap() && *state.escape_registered.lock().unwrap() {
        dismiss(app);
        return true;
    }
    let keys = state.preferences.lock().unwrap().shortcuts();
    if let Ok(keys) = keys {
        if key == keys[0] {
            if let Err(error) = set_listening(app, None).await {
                report_error(app, error);
            }
            return true;
        }
        if key == keys[1] {
            request_answer(app).await;
            return true;
        }
    }
    false
}

async fn set_listening(app: &tauri::AppHandle, requested: Option<bool>) -> Result<(), String> {
    let state = app.state::<AudioAssistant>();
    let _transition = state.transition.lock().await;
    let enabled = requested.unwrap_or_else(|| !state.live.lock().unwrap().listening);
    if enabled {
        if state.registered.lock().unwrap().len() != 2 {
            return Err(
                "Choose available audio assistant shortcuts in Settings before listening.".into(),
            );
        }
        if !crate::openai_credentials::is_configured()? {
            let error = "Add an OpenAI API key in Settings to turn on audio listening. Codex answers still require an API key for transcription.";
            *state.error.lock().unwrap() = Some(error.into());
            notify(app);
            return Err(error.into());
        }
        let mut next = state.preferences.lock().unwrap().clone();
        next.listening = true;
        save(&state, &next)?;
        *state.preferences.lock().unwrap() = next;
        if state.live.lock().unwrap().listening {
            return Ok(());
        }
        let (generation, cancel) = {
            let mut live = state.live.lock().unwrap();
            let id = live.start();
            (id, live.cancel.clone())
        };
        *state.error.lock().unwrap() = None;
        let app = app.clone();
        *state.stream.lock().unwrap() = Some(tauri::async_runtime::spawn(transcription::run(
            app, generation, cancel,
        )));
    } else {
        // OFF is effective even when persistence fails. Await task destruction so
        // socket and WASAPI resources are gone before the command completes.
        state.live.lock().unwrap().stop();
        dismiss(app);
        let task = state.stream.lock().unwrap().take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        let mut next = state.preferences.lock().unwrap().clone();
        next.listening = false;
        *state.preferences.lock().unwrap() = next.clone();
        let saved = save(&state, &next);
        *state.error.lock().unwrap() = saved.as_ref().err().cloned();
        notify(app);
        saved?;
    }
    notify(app);
    Ok(())
}

#[tauri::command]
pub fn audio_assistant_state(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<Value, String> {
    settings_only(&window)?;
    let state = app.state::<AudioAssistant>();
    let mut value = serde_json::to_value(state.preferences.lock().unwrap().clone()).unwrap();
    value["listening"] = json!(state.live.lock().unwrap().listening);
    value["error"] = json!(*state.error.lock().unwrap());
    value["answerProvider"] = json!(crate::text_extractor::audio_answer_provider(&app));
    Ok(value)
}
#[tauri::command]
pub async fn set_audio_listening(
    app: tauri::AppHandle,
    window: WebviewWindow,
    listening: bool,
) -> Result<(), String> {
    settings_only(&window)?;
    set_listening(&app, Some(listening)).await
}
#[tauri::command]
pub async fn set_audio_shortcuts(
    app: tauri::AppHandle,
    window: WebviewWindow,
    listen_shortcut: String,
    answer_shortcut: String,
) -> Result<(), String> {
    settings_only(&window)?;
    let state = app.state::<AudioAssistant>();
    let _transition = state.transition.lock().await;
    let prefs = state.preferences.lock().unwrap().clone();
    let mut next = prefs.clone();
    next.listen_shortcut = listen_shortcut;
    next.answer_shortcut = answer_shortcut;
    let keys = next.shortcuts()?;
    next.listen_shortcut = keys[0].to_string();
    next.answer_shortcut = keys[1].to_string();
    register_shortcuts(&app, &next)?;
    *state.preferences.lock().unwrap() = next;
    notify(&app);
    Ok(())
}
#[tauri::command]
pub async fn audio_context_file(
    app: tauri::AppHandle,
    window: WebviewWindow,
    clear: bool,
) -> Result<(), String> {
    settings_only(&window)?;
    let path = if clear {
        None
    } else {
        let (send, receive) = tokio::sync::oneshot::channel();
        app.dialog()
            .file()
            .add_filter("Text context", &["txt", "md"])
            .pick_file(move |file| {
                let _ = send.send(file);
            });
        let Some(file) = receive.await.map_err(|_| "File selection was cancelled.")? else {
            return Ok(());
        };
        let path = file.into_path().map_err(|_| "Choose a local text file.")?;
        model::context_text(&path)?;
        Some(path)
    };
    let state = app.state::<AudioAssistant>();
    let _transition = state.transition.lock().await;
    let mut prefs = state.preferences.lock().unwrap();
    let mut next = prefs.clone();
    next.context_file = path;
    save(&state, &next)?;
    *prefs = next;
    drop(prefs);
    notify(&app);
    Ok(())
}

async fn request_answer(app: &tauri::AppHandle) {
    let state = app.state::<AudioAssistant>();
    let _transition = state.transition.lock().await;
    let request = {
        let mut live = state.live.lock().unwrap();
        live.begin_answer().map(|(generation, id, cancel)| {
            (
                generation,
                id,
                cancel,
                live.transcript.snapshot(Instant::now()),
            )
        })
    };
    let Some((generation, id, cancel, transcript)) = request else {
        return;
    };
    hide_overlay(app);
    if let Some(task) = state.answer_task.lock().unwrap().take() {
        task.abort();
    }
    let path = state.preferences.lock().unwrap().context_file.clone();
    let app = app.clone();
    *state.answer_task.lock().unwrap() = Some(tauri::async_runtime::spawn(async move {
        let request = async {
            let context = if let Some(path) = path {
                tokio::task::spawn_blocking(move || model::context_text(&path))
                    .await
                    .map_err(|_| "Could not read context.")??
            } else {
                String::new()
            };
            crate::text_extractor::request_audio_answer(
                &app,
                model::answer_body(&transcript, &context),
                cancel.clone(),
            )
            .await
        };
        let result =
            tokio::select! { biased; _ = cancel.cancelled() => return, result = request => result };
        // Serialize final UI delivery with OFF, replacement, and Escape on the UI thread.
        let ui = app.clone();
        let _ = app.run_on_main_thread(move || {
            let state = ui.state::<AudioAssistant>();
            match result {
                Ok(text) if !text.trim().is_empty() => {
                    if !state
                        .live
                        .lock()
                        .unwrap()
                        .finish_answer(generation, id, text)
                    {
                        return;
                    }
                    if let Err(error) = prepare_overlay(&ui) {
                        report_error(&ui, error);
                    }
                }
                Err(error) => {
                    let current = {
                        let live = state.live.lock().unwrap();
                        live.accepts(generation) && live.answer_id == id
                    };
                    if current {
                        report_error(&ui, error);
                    }
                }
                _ => {}
            }
        });
    }));
}
fn prepare_overlay(app: &tauri::AppHandle) -> Result<(), String> {
    let window = if let Some(window) = app.get_webview_window(OVERLAY) {
        window
    } else {
        WebviewWindowBuilder::new(app, OVERLAY, WebviewUrl::App("audio-assistant.html".into()))
            .title("Pulse")
            .inner_size(420.0, 200.0)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .resizable(false)
            .skip_taskbar(true)
            .always_on_top(true)
            .focused(false)
            .focusable(false)
            .visible(false)
            .build()
            .map_err(|_| "Could not open the answer overlay.")?
    };
    if let Ok(theme) = crate::visual_app_theme(app) {
        let _ = window.set_theme(Some(theme));
    }
    let _ = window.eval("window.dispatchEvent(new Event('pulse-answer'))");
    Ok(())
}
#[tauri::command]
pub fn audio_answer_ready(app: tauri::AppHandle, window: WebviewWindow) -> Result<Value, String> {
    if window.label() != OVERLAY {
        return Err("Invalid answer window.".into());
    }
    let state = app.state::<AudioAssistant>();
    let live = state.live.lock().unwrap();
    Ok(json!({"id":live.answer_id, "text":live.answer}))
}
#[tauri::command]
pub fn show_audio_answer(
    app: tauri::AppHandle,
    window: WebviewWindow,
    id: u64,
    height: f64,
) -> Result<(), String> {
    if window.label() != OVERLAY || !height.is_finite() {
        return Err("Invalid answer window.".into());
    }
    let state = app.state::<AudioAssistant>();
    let live = state.live.lock().unwrap();
    if !live.listening || live.answer_id != id || live.answer.is_none() {
        return Ok(());
    }
    if let Some(monitor) = app.primary_monitor().map_err(|e| e.to_string())? {
        let area = monitor.work_area();
        let scale = monitor.scale_factor();
        let _ = window.set_position(tauri::PhysicalPosition::new(
            area.position.x + (12.0 * scale) as i32,
            area.position.y + (12.0 * scale) as i32,
        ));
    }
    window
        .set_size(tauri::LogicalSize::new(420.0, height.clamp(64.0, 500.0)))
        .map_err(|e| e.to_string())?;
    let mut escape = state.escape_registered.lock().unwrap();
    if !*escape {
        app.global_shortcut()
            .register("Escape")
            .map_err(|_| "Could not register Escape for the answer overlay.")?;
        *escape = true;
    }
    window.show().map_err(|e| e.to_string())
}
fn hide_overlay(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(OVERLAY) {
        let _ = window.hide();
        let _ = window.eval("window.dispatchEvent(new Event('pulse-answer-clear'))");
    }
    let state = app.state::<AudioAssistant>();
    let mut registered = state.escape_registered.lock().unwrap();
    if *registered {
        let _ = app.global_shortcut().unregister("Escape");
        *registered = false;
    }
}
pub(crate) fn dismiss(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AudioAssistant>() else {
        return;
    };
    state.live.lock().unwrap().dismiss();
    if let Some(task) = state.answer_task.lock().unwrap().take() {
        task.abort();
    }
    hide_overlay(app);
}
#[tauri::command]
pub fn dismiss_audio_answer(app: tauri::AppHandle, window: WebviewWindow) -> Result<(), String> {
    if window.label() != OVERLAY {
        return Err("Invalid answer window.".into());
    }
    dismiss(&app);
    Ok(())
}
pub(crate) fn appearance_changed(app: &tauri::AppHandle, theme: tauri::Theme) {
    if let Some(window) = app.get_webview_window(OVERLAY) {
        let _ = window.set_theme(Some(theme));
    }
}
pub(crate) fn shutdown(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AudioAssistant>() else {
        return;
    };
    state.live.lock().unwrap().stop();
    hide_overlay(app);
    let stream = state.stream.lock().unwrap().take();
    let answer = state.answer_task.lock().unwrap().take();
    for task in [stream, answer].into_iter().flatten() {
        task.abort();
        let _ = tauri::async_runtime::block_on(task);
    }
    for key in state.registered.lock().unwrap().drain(..) {
        let _ = app.global_shortcut().unregister(key);
    }
    if let Some(window) = app.get_webview_window(OVERLAY) {
        let _ = window.destroy();
    }
}
pub(crate) async fn credentials_cleared(app: &tauri::AppHandle) -> Result<(), String> {
    set_listening(app, Some(false)).await
}
