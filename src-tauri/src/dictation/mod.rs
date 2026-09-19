//! Mac hold-to-dictate. Credentials and microphone bytes never enter the webview.
mod protocol;
mod shortcut;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    ffi::{c_char, c_void, CString},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::Duration,
};
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tokio::{
    sync::mpsc,
    time::{timeout, Instant},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use tokio_util::sync::CancellationToken;

const WINDOW: &str = "dictation";
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
extern "C" {
    fn pulse_dictation_register_shortcut(callback: extern "C" fn(bool, bool, bool, f64)) -> bool;
    fn pulse_dictation_unregister_shortcut();
    fn pulse_dictation_right_option_down() -> bool;
    fn pulse_dictation_mic_allowed() -> bool;
    fn pulse_dictation_ax_allowed() -> bool;
    fn pulse_dictation_request_access(microphone: bool);
    fn pulse_dictation_start(id: u64, callback: extern "C" fn(u64, *const u8, usize, f32)) -> bool;
    fn pulse_dictation_stop();
    fn pulse_dictation_clear_target();
    fn pulse_dictation_target(x: *mut f64, y: *mut f64, width: *mut f64, height: *mut f64) -> bool;
    fn pulse_dictation_deliver(text: *const c_char) -> i32;
    fn pulse_dictation_position(window: *mut c_void, x: *mut f64, y: *mut f64, bottom: *mut f64);
}
struct Session {
    id: u64,
    audio: Option<mpsc::Sender<Vec<u8>>>,
    cancel: CancellationToken,
    error: Option<String>,
    phase: &'static str,
    text: String,
    levels: VecDeque<f32>,
    samples: u64,
    origin: (f64, f64),
    bottom: f64,
    target: Option<Value>,
    message: String,
}
struct Dictation {
    hold_shortcut: Mutex<shortcut::HoldShortcut>,
    enabled: AtomicBool,
    next_id: AtomicU64,
    session: Mutex<Option<Session>>,
    startup_error: Mutex<Option<String>>,
    escape_registered: AtomicBool,
    last_transcript: Mutex<String>,
}
async fn on_main<T: Send + 'static>(
    app: &tauri::AppHandle,
    action: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(action());
    })
    .map_err(|e| e.to_string())?;
    rx.await
        .map_err(|_| "The dictation operation stopped.".to_owned())?
}
fn settings_only(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "settings" {
        Ok(())
    } else {
        Err("Open Pulse Settings to change dictation.".into())
    }
}
fn register(app: &tauri::AppHandle) -> Result<(), String> {
    if !unsafe { pulse_dictation_ax_allowed() } {
        return Err("Allow Accessibility in Dictation settings to use Right Option, then enable Dictation again.".into());
    }
    *app.state::<Dictation>().hold_shortcut.lock().unwrap() =
        shortcut::HoldShortcut::new(unsafe { pulse_dictation_right_option_down() });
    if unsafe { pulse_dictation_register_shortcut(shortcut_event) } {
        Ok(())
    } else {
        Err("Could not listen for Right Option. Check Accessibility access and try again.".into())
    }
}

// AppKit invokes both local and global event monitors on its main thread.
// Only physical key state is forwarded; no typed text is read or retained.
extern "C" fn shortcut_event(down: bool, chord: bool, interrupted: bool, timestamp: f64) {
    let Some(app) = APP.get() else {
        return;
    };
    let state = app.state::<Dictation>();
    let action = {
        let mut key = state.hold_shortcut.lock().unwrap();
        if interrupted {
            key.interrupt()
        } else {
            key.update(down, chord, timestamp)
        }
    };
    match action {
        Some(shortcut::Action::Start) => start(app),
        Some(shortcut::Action::Release) => release(app),
        Some(shortcut::Action::Cancel) => cancel(app),
        None => {}
    }
}
pub fn install(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let enabled = std::fs::read_to_string(app.path().app_config_dir()?.join("dictation-enabled"))
        .is_ok_and(|value| value == "true");
    let _ = APP.set(app.clone());
    app.manage(Dictation {
        hold_shortcut: Mutex::new(shortcut::HoldShortcut::default()),
        enabled: AtomicBool::new(enabled),
        next_id: AtomicU64::new(1),
        session: Mutex::new(None),
        startup_error: Mutex::new(None),
        escape_registered: AtomicBool::new(false),
        last_transcript: Mutex::new(String::new()),
    });
    if enabled {
        if let Err(error) = register(app) {
            app.state::<Dictation>()
                .enabled
                .store(false, Ordering::Release);
            *app.state::<Dictation>().startup_error.lock().unwrap() = Some(error);
        }
    }
    // Warm the local overlay, never the microphone or an API session.
    if let Err(error) = create_window(app) {
        *app.state::<Dictation>().startup_error.lock().unwrap() = Some(error);
    }
    Ok(())
}
fn create_window(app: &tauri::AppHandle) -> Result<WebviewWindow, String> {
    if let Some(window) = app.get_webview_window(WINDOW) {
        return Ok(window);
    }
    WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("dictation.html".into()))
        .title("Pulse Dictation")
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
        .build()
        .map_err(|_| "Could not prepare the dictation overlay.".into())
}
#[tauri::command]
pub fn dictation_settings(app: tauri::AppHandle, window: WebviewWindow) -> Result<Value, String> {
    settings_only(&window)?;
    let state = app.state::<Dictation>();
    Ok(
        json!({"enabled":state.enabled.load(Ordering::Acquire), "shortcut":"Right Option", "microphone":unsafe { pulse_dictation_mic_allowed() }, "accessibility":unsafe { pulse_dictation_ax_allowed() }, "apiKeyConfigured":crate::openai_credentials::is_configured().unwrap_or(false), "error":*state.startup_error.lock().unwrap(), "hasLastTranscript":!state.last_transcript.lock().unwrap().is_empty()}),
    )
}
#[tauri::command]
pub fn copy_last_dictation(app: tauri::AppHandle, window: WebviewWindow) -> Result<(), String> {
    settings_only(&window)?;
    let text = app
        .state::<Dictation>()
        .last_transcript
        .lock()
        .unwrap()
        .clone();
    if text.is_empty() {
        return Err("No transcript is available yet.".into());
    }
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text))
        .map_err(|_| "Could not copy the last transcript. Try again.".into())
}

#[tauri::command]
pub async fn request_dictation_access(
    app: tauri::AppHandle,
    window: WebviewWindow,
    microphone: bool,
) -> Result<(), String> {
    settings_only(&window)?;
    on_main(&app, move || {
        unsafe {
            pulse_dictation_request_access(microphone);
        }
        Ok(())
    })
    .await
}
#[tauri::command]
pub async fn set_dictation_enabled(
    app: tauri::AppHandle,
    window: WebviewWindow,
    enabled: bool,
) -> Result<(), String> {
    settings_only(&window)?;
    let handle = app.clone();
    on_main(&app, move || {
        let state = handle.state::<Dictation>();
        let old = state.enabled.load(Ordering::Acquire);
        if old == enabled {
            return Ok(());
        }
        if enabled {
            register(&handle)?;
        } else {
            unsafe {
                pulse_dictation_unregister_shortcut();
            }
            cancel(&handle);
        }
        let path = handle.path().app_config_dir().map_err(|e| e.to_string())?;
        let saved = std::fs::create_dir_all(&path)
            .and_then(|_| std::fs::write(path.join("dictation-enabled"), enabled.to_string()));
        if saved.is_err() {
            if enabled {
                unsafe {
                    pulse_dictation_unregister_shortcut();
                }
            } else {
                let _ = register(&handle);
            }
            return Err("Could not save Dictation settings.".into());
        }
        state.enabled.store(enabled, Ordering::Release);
        *state.startup_error.lock().unwrap() = None;
        Ok(())
    })
    .await
}
// The webview acknowledges its reset DOM before a reused overlay is shown.
#[tauri::command]
pub fn dictation_overlay_ready(
    app: tauri::AppHandle,
    window: WebviewWindow,
    session_id: u64,
) -> Result<(), String> {
    if window.label() != WINDOW {
        return Err("This is only available to the dictation overlay.".into());
    }
    let state = app.state::<Dictation>();
    let guard = state.session.lock().unwrap();
    if guard.as_ref().is_some_and(|s| {
        s.id == session_id && matches!(s.phase, "listening" | "finalizing" | "error")
    }) {
        window
            .show()
            .map_err(|_| "Could not show the dictation overlay.")?;
    }
    Ok(())
}
#[tauri::command]
pub fn dictation_snapshot(app: tauri::AppHandle, window: WebviewWindow) -> Result<Value, String> {
    if window.label() != WINDOW {
        return Err("This is only available to the dictation overlay.".into());
    }
    let state = app.state::<Dictation>();
    let guard = state.session.lock().unwrap();
    Ok(match guard.as_ref() {
        Some(s) => {
            json!({"id":s.id,"phase":s.phase,"text":s.text,"levels":s.levels,"samples":s.samples,"bottom":s.bottom,"target":s.target,"message":s.message})
        }
        None => json!({"phase":"idle"}),
    })
}
extern "C" fn audio_callback(id: u64, bytes: *const u8, length: usize, level: f32) {
    let Some(app) = APP.get() else {
        return;
    };
    let state = app.state::<Dictation>();
    let mut guard = state.session.lock().unwrap();
    let Some(s) = guard.as_mut().filter(|s| s.id == id && s.audio.is_some()) else {
        return;
    };
    if level < 0.0 || bytes.is_null() {
        s.error = Some("The microphone stopped producing audio. Try again.".into());
        s.cancel.cancel();
        return;
    }
    // The native bridge owns this buffer only until this callback returns.
    let data = unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec();
    match s.audio.as_ref().unwrap().try_send(data) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Closed(_)) => return, // Preserve the network task's original error.
        Err(mpsc::error::TrySendError::Full(_)) => {
            s.error =
                Some("The connection could not keep up with the microphone. Try again.".into());
            s.cancel.cancel();
            return;
        }
    }
    s.samples += (length / 2) as u64;
    s.levels.push_back(level);
    if s.levels.len() > 160 {
        s.levels.pop_front();
    }
}
fn start(app: &tauri::AppHandle) {
    let state = app.state::<Dictation>();
    if !state.enabled.load(Ordering::Acquire) {
        state.hold_shortcut.lock().unwrap().interrupt();
        return;
    }
    {
        let mut guard = state.session.lock().unwrap();
        if guard
            .as_ref()
            .is_some_and(|s| !matches!(s.phase, "idle" | "done" | "error"))
        {
            state.hold_shortcut.lock().unwrap().interrupt();
            return;
        }
        if let Some(previous) = guard.take() {
            previous.cancel.cancel();
        }
    }
    if state.escape_registered.swap(false, Ordering::AcqRel) {
        let _ = app.global_shortcut().unregister("Escape");
    }
    unsafe {
        pulse_dictation_clear_target();
    }
    let window = match create_window(app) {
        Ok(window) => window,
        Err(error) => {
            *state.startup_error.lock().unwrap() = Some(error);
            state.hold_shortcut.lock().unwrap().interrupt();
            let _ = crate::open_settings(app);
            return;
        }
    };
    let mut x = 0.0;
    let mut y = 0.0;
    let mut bottom = 24.0;
    if let Ok(pointer) = window.ns_window() {
        unsafe {
            pulse_dictation_position(pointer, &mut x, &mut y, &mut bottom);
        }
    }
    let (tx, rx) = mpsc::channel(256); // Bounded startup/network backlog, about 10 seconds.
    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let token = CancellationToken::new();
    *state.session.lock().unwrap() = Some(Session {
        id,
        audio: Some(tx),
        cancel: token.clone(),
        error: None,
        phase: "listening",
        text: String::new(),
        levels: VecDeque::new(),
        samples: 0,
        origin: (x, y),
        bottom,
        target: None,
        message: String::new(),
    });
    let _ = window.eval(format!("window.pulseDictationStart?.({id}, {bottom});"));
    let _ = window.set_ignore_cursor_events(true);
    let mic = unsafe { pulse_dictation_mic_allowed() };
    let started = mic && unsafe { pulse_dictation_start(id, audio_callback) };
    let escape_registered = app
        .global_shortcut()
        .on_shortcut("Escape", |app, _, event| {
            if event.state() == ShortcutState::Pressed {
                let app = app.clone();
                let handle = app.clone();
                let _ = handle.run_on_main_thread(move || cancel(&app));
            }
        });
    state
        .escape_registered
        .store(escape_registered.is_ok(), Ordering::Release);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = if !mic {
            Err("Allow Microphone access in Pulse Settings → Advanced → Dictation.".into())
        } else if !started {
            Err("Could not start the microphone. Check your input device and try again.".into())
        } else {
            tokio::select! {
                _ = token.cancelled() => Err("cancelled".into()),
                result = transcribe(&app, id, rx) => result,
            }
        };
        finish(app, id, result).await;
    });
}
fn release(app: &tauri::AppHandle) {
    let state = app.state::<Dictation>();
    let should_stop = state
        .session
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.audio.is_some());
    if !should_stop {
        return;
    }
    // Never hold the session mutex while removing the native audio tap.
    unsafe {
        pulse_dictation_stop();
    }
    let mut x = 0.0;
    let mut y = 0.0;
    let mut width = 0.0;
    let mut height = 0.0;
    let target = unsafe { pulse_dictation_target(&mut x, &mut y, &mut width, &mut height) };
    if let Some(s) = state.session.lock().unwrap().as_mut() {
        s.audio.take(); // All queued PCM is drained before the one final commit.
        s.phase = "finalizing";
        if target {
            s.target =
                Some(json!({"x":x-s.origin.0,"y":y-s.origin.1,"width":width,"height":height}));
        }
    };
}
fn cancel(app: &tauri::AppHandle) {
    app.state::<Dictation>()
        .hold_shortcut
        .lock()
        .unwrap()
        .interrupt();
    unsafe {
        pulse_dictation_stop();
        pulse_dictation_clear_target();
    }
    if let Some(s) = app.state::<Dictation>().session.lock().unwrap().as_mut() {
        s.audio.take();
        s.phase = "idle";
        s.text.clear();
        s.error = None;
        s.cancel.cancel();
    }
}
fn update_text(app: &tauri::AppHandle, id: u64, text: &str) {
    if let Some(s) = app
        .state::<Dictation>()
        .session
        .lock()
        .unwrap()
        .as_mut()
        .filter(|s| s.id == id)
    {
        s.text = text.to_owned();
    }
}
async fn transcribe(
    app: &tauri::AppHandle,
    id: u64,
    audio: mpsc::Receiver<Vec<u8>>,
) -> Result<String, String> {
    let key = tauri::async_runtime::spawn_blocking(crate::openai_credentials::load)
        .await
        .map_err(|_| "Could not read the saved OpenAI key.")??;
    let mut request = "wss://api.openai.com/v1/realtime?intent=transcription"
        .into_client_request()
        .map_err(|_| "Could not prepare transcription.")?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {key}")
            .parse()
            .map_err(|_| "The saved OpenAI key is invalid.")?,
    );
    drop(key);
    let (socket, _) = timeout(Duration::from_secs(12), connect_async(request)).await.map_err(|_| "Connecting to OpenAI timed out.")?
        .map_err(|_| "Could not connect to OpenAI. Check your API key, model access, and internet connection.")?;
    stream_transcription(socket, audio, |text| update_text(app, id, text)).await
}

async fn stream_transcription(
    mut socket: Socket,
    mut audio: mpsc::Receiver<Vec<u8>>,
    mut on_text: impl FnMut(&str),
) -> Result<String, String> {
    // This is a Realtime transcription session, not a GPT-Live voice-agent session.
    send(&mut socket, protocol::configuration()).await?;
    timeout(Duration::from_secs(10), async {
        loop {
            let event = receive(&mut socket).await?;
            protocol::check_error(&event)?;
            if matches!(
                event["type"].as_str(),
                Some("session.updated" | "transcription_session.updated")
            ) {
                return Ok::<_, String>(());
            }
        }
    })
    .await
    .map_err(|_| "OpenAI did not confirm the transcription session.")??;
    let mut transcript = protocol::Transcript::default();
    let mut committed = false;
    let mut samples = 0usize;
    let mut deadline = Instant::now() + Duration::from_secs(300);
    let mut microphone_deadline = Instant::now() + Duration::from_secs(4);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return Err(if committed { "OpenAI did not finish the transcript in time." } else { "Dictation reached its five-minute limit." }.into()),
            _ = tokio::time::sleep_until(microphone_deadline), if !committed => return Err("The microphone stopped producing audio. Check your input device.".into()),
            chunk = audio.recv(), if !committed => {
                if let Some(bytes) = chunk {
                    microphone_deadline = Instant::now() + Duration::from_secs(4);
                    samples += bytes.len()/2;
                    send(&mut socket,json!({"type":"input_audio_buffer.append","audio":BASE64.encode(bytes)})).await?;
                } else {
                    if samples == 0 { return Ok(String::new()); }
                    // Realtime requires at least 100 ms per committed turn.
                    if samples < 2400 { send(&mut socket,json!({"type":"input_audio_buffer.append","audio":BASE64.encode(vec![0u8;(2400-samples)*2])})).await?; }
                    send(&mut socket,json!({"type":"input_audio_buffer.commit"})).await?;
                    committed = true;
                    deadline = Instant::now() + Duration::from_secs(20);
                }
            }
            event = receive(&mut socket) => {
                let event = event?;
                protocol::check_error(&event)?;
                if let Some(done) = transcript.accept(&event)? {
                    on_text(&transcript.text);
                    if done && committed { return Ok(transcript.text); }
                }
            }
        }
    }
}
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
async fn send(socket: &mut Socket, event: Value) -> Result<(), String> {
    timeout(
        Duration::from_secs(5),
        socket.send(Message::Text(event.to_string().into())),
    )
    .await
    .map_err(|_| "Sending microphone audio timed out.")?
    .map_err(|_| "The transcription connection was interrupted.".into())
}
async fn receive(socket: &mut Socket) -> Result<Value, String> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(&text)
                    .map_err(|_| "OpenAI returned an unreadable transcription event.".into())
            }
            Some(Ok(
                Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_),
            )) => {}
            _ => return Err("The transcription connection closed before completion.".into()),
        }
    }
}
async fn finish(app: tauri::AppHandle, id: u64, result: Result<String, String>) {
    let handle = app.clone();
    let prepared = on_main(&app, move || {
        if handle
            .state::<Dictation>()
            .session
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|s| s.id != id)
        {
            return Ok(None);
        }
        unsafe {
            pulse_dictation_stop();
        }
        let state = handle.state::<Dictation>();
        let mut guard = state.session.lock().unwrap();
        let Some(s) = guard.as_mut().filter(|s| s.id == id) else {
            return Ok(None);
        };
        state.hold_shortcut.lock().unwrap().interrupt();
        s.audio.take();
        let result = if let Some(error) = s.error.take() {
            Err(error)
        } else if s.cancel.is_cancelled() {
            Err("cancelled".into())
        } else {
            result
        };
        match result {
            Ok(text) if !text.trim().is_empty() => {
                *state.last_transcript.lock().unwrap() = text.clone();
                s.text = text;
                s.phase = "sending";
                Ok(Some((s.text.clone(), true)))
            }
            Ok(_) => {
                s.phase = "error";
                s.message = "No speech detected".into();
                Ok(None)
            }
            Err(error) if error == "cancelled" => {
                s.phase = "idle";
                s.text.clear();
                Ok(None)
            }
            Err(error) => {
                s.phase = "error";
                s.message = error;
                unsafe {
                    pulse_dictation_clear_target();
                }
                if s.text.is_empty() {
                    Ok(None)
                } else {
                    *state.last_transcript.lock().unwrap() = s.text.clone();
                    s.message.push_str(" Partial transcript copied.");
                    Ok(Some((s.text.clone(), false)))
                }
            }
        }
    })
    .await;
    if let Ok(Some((text, completed))) = prepared {
        let handle = app.clone();
        let _ = on_main(&app, move || {
            // Disabling Dictation before delivery cancels it too.
            let state = handle.state::<Dictation>();
            let mut guard = state.session.lock().unwrap();
            let Some(s) = guard.as_mut().filter(|s| s.id == id) else {
                return Ok(());
            };
            if s.cancel.is_cancelled() && s.phase != "error" {
                s.phase = "idle";
                return Ok(());
            }
            let clean = text.replace('\0', "");
            let text = CString::new(clean).map_err(|_| "Could not deliver the transcript.")?;
            let outcome = unsafe { pulse_dictation_deliver(text.as_ptr()) };
            if outcome == 0 {
                s.phase = "error";
                s.message =
                    "Could not copy the transcript. Use Copy last transcript in Settings to retry."
                        .into();
            } else if completed {
                s.phase = "done";
                s.message = if outcome == 3 {
                    "Sent to input · copied as backup"
                } else {
                    "Copied to clipboard"
                }
                .into();
            }
            Ok(())
        })
        .await;
    }
    let wait = app
        .state::<Dictation>()
        .session
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| if s.phase == "error" { 6000 } else { 220 })
        .unwrap_or(0);
    tokio::time::sleep(Duration::from_millis(wait)).await;
    let handle = app.clone();
    let _ = on_main(&app, move || {
        let state = handle.state::<Dictation>();
        let mut guard = state.session.lock().unwrap();
        if guard.as_ref().is_some_and(|s| s.id == id) {
            *guard = None;
            if state.escape_registered.swap(false, Ordering::AcqRel) {
                let _ = handle.global_shortcut().unregister("Escape");
            }
            unsafe {
                pulse_dictation_clear_target();
            }
            if let Some(window) = handle.get_webview_window(WINDOW) {
                let _ = window.hide();
            }
        }
        Ok(())
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn release_during_connection_drains_audio_before_one_commit_and_final_replaces_partial() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let configuration: Value =
                serde_json::from_str(&socket.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            assert_eq!(configuration, protocol::configuration());
            // Even already-buffered audio must wait for session configuration.
            assert!(timeout(Duration::from_millis(40), socket.next())
                .await
                .is_err());
            socket
                .send(Message::Text(
                    json!({"type":"session.updated"}).to_string().into(),
                ))
                .await
                .unwrap();
            let mut received = Vec::new();
            loop {
                let event: Value = serde_json::from_str(
                    &socket.next().await.unwrap().unwrap().into_text().unwrap(),
                )
                .unwrap();
                match event["type"].as_str().unwrap() {
                    "input_audio_buffer.append" => {
                        received.extend(BASE64.decode(event["audio"].as_str().unwrap()).unwrap())
                    }
                    "input_audio_buffer.commit" => break,
                    other => panic!("Unexpected event: {other}"),
                }
            }
            assert_eq!(&received[..1000], vec![1u8; 1000]);
            assert_eq!(&received[1000..2000], vec![2u8; 1000]);
            assert_eq!(&received[2000..], vec![0u8; 2800]); // Minimum-duration padding.
            for event in [
                json!({"type":"conversation.item.input_audio_transcription.delta","item_id":"one","delta":"Hallo"}),
                json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"one","transcript":"Hello!"}),
            ] {
                socket
                    .send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
            }
            // Client completion must not commit a second time.
            let next = socket.next().await;
            assert!(!matches!(next, Some(Ok(Message::Text(_)))));
        });
        let (tx, rx) = mpsc::channel(8);
        tx.send(vec![1; 1000]).await.unwrap();
        tx.send(vec![2; 1000]).await.unwrap();
        drop(tx); // Key released before the connection was ready.
        let (socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
        let mut shown = Vec::new();
        let result = stream_transcription(socket, rx, |text| shown.push(text.to_owned()))
            .await
            .unwrap();
        assert_eq!(result, "Hello!");
        assert_eq!(shown, ["Hallo", "Hello!"]);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn empty_recording_never_commits_and_failed_session_preserves_partial() {
        for empty in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                socket.next().await.unwrap().unwrap();
                socket
                    .send(Message::Text(
                        json!({"type":"session.updated"}).to_string().into(),
                    ))
                    .await
                    .unwrap();
                if empty {
                    assert!(!matches!(socket.next().await, Some(Ok(Message::Text(_)))));
                } else {
                    for event in [
                        json!({"type":"conversation.item.input_audio_transcription.delta","item_id":"one","delta":"Keep these words"}),
                        json!({"type":"error","error":{"code":"rate_limit_exceeded"}}),
                    ] {
                        socket
                            .send(Message::Text(event.to_string().into()))
                            .await
                            .unwrap();
                    }
                }
            });
            let (tx, rx) = mpsc::channel(8);
            let sender = if empty {
                drop(tx);
                None
            } else {
                Some(tx)
            };
            let (socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
            let mut partial = String::new();
            let result = stream_transcription(socket, rx, |text| partial = text.to_owned()).await;
            if empty {
                assert_eq!(result.unwrap(), "");
            } else {
                assert!(result.is_err());
                assert_eq!(partial, "Keep these words");
            }
            drop(sender);
            server.await.unwrap();
        }
    }
}
