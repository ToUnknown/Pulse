//! Cross-platform tap-or-hold dictation. Credentials and microphone bytes never enter the webview.
mod protocol;
mod shortcut;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    ffi::{c_char, c_void, CString},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::Duration,
};
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::{
    sync::{mpsc, oneshot},
    time::{timeout, Instant},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use tokio_util::sync::CancellationToken;

const WINDOW: &str = "dictation";
const API_KEY_REQUIRED: &str = "Add your OpenAI API key in Settings to enable Dictation.";
const ACCESSIBILITY_REQUIRED: &str =
    "Allow Accessibility in Dictation settings to use Right Option, then enable Dictation again.";
const MICROPHONE_REQUIRED: &str =
    "Allow Microphone access in Pulse Settings → Advanced → Dictation.";
const SHORTCUT: &str = if cfg!(target_os = "windows") {
    "Right Alt"
} else {
    "Right Option"
};
fn native_window(window: &WebviewWindow) -> Result<*mut c_void, String> {
    #[cfg(target_os = "macos")]
    {
        window
            .ns_window()
            .map_err(|_| "Could not access the dictation overlay.".into())
    }
    #[cfg(target_os = "windows")]
    {
        window
            .hwnd()
            .map(|h| h.0.cast())
            .map_err(|_| "Could not access the dictation overlay.".into())
    }
}
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
extern "C" {
    fn pulse_dictation_register_shortcut(callback: extern "C" fn(bool, bool, bool, f64)) -> bool;
    fn pulse_dictation_unregister_shortcut();
    fn pulse_dictation_right_option_down() -> bool;
    fn pulse_dictation_mic_allowed() -> bool;
    fn pulse_dictation_ax_allowed() -> bool;
    fn pulse_dictation_request_access(microphone: bool);
    fn pulse_dictation_start_async(
        id: u64,
        callback: extern "C" fn(u64, *const u8, usize, f32),
        ready: extern "C" fn(*mut c_void, bool),
        context: *mut c_void,
    );
    fn pulse_dictation_stop();
    fn pulse_dictation_final_step(text: *const c_char) -> i32;
    fn pulse_dictation_copy(text: *const c_char) -> bool;
    fn pulse_dictation_position(window: *mut c_void, bottom: *mut f64);
    fn pulse_dictation_transcript_blur(
        window: *mut c_void,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        opacity: f64,
        dark_mode: bool,
    );
    fn pulse_dictation_glass(
        window: *mut c_void,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        opacity: f64,
        dark_mode: bool,
    ) -> bool;
}
struct Session {
    id: u64,
    audio: Option<mpsc::Sender<Vec<u8>>>,
    cancel: CancellationToken,
    error: Option<String>,
    phase: &'static str,
    text: String,
    level: f32,
    samples: u64,
    bottom: f64,
    delivery: FinalDelivery,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FinalDelivery {
    Pending,
    // Paste was submitted; apps do not provide a universal receipt.
    Submitted,
    Clipboard,
    Discard,
}
impl Session {
    fn final_delivery(&self, expired: bool) -> FinalDelivery {
        if self.cancel.is_cancelled() {
            FinalDelivery::Discard
        } else if expired && self.delivery == FinalDelivery::Pending {
            FinalDelivery::Clipboard
        } else {
            self.delivery
        }
    }
}

struct Dictation {
    hold_shortcut: Mutex<shortcut::HoldShortcut>,
    enabled: AtomicBool,
    next_id: AtomicU64,
    session: Mutex<Option<Session>>,
    startup_error: Mutex<Option<String>>,
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
    if !crate::openai_credentials::is_configured()? {
        return Err(API_KEY_REQUIRED.into());
    }
    if !unsafe { pulse_dictation_ax_allowed() } {
        return Err(ACCESSIBILITY_REQUIRED.into());
    }
    *app.state::<Dictation>().hold_shortcut.lock().unwrap() =
        shortcut::HoldShortcut::new(unsafe { pulse_dictation_right_option_down() });
    if unsafe { pulse_dictation_register_shortcut(shortcut_event) } {
        Ok(())
    } else {
        Err(format!(
            "Could not listen for {SHORTCUT}. Check access and try again."
        ))
    }
}

// Native monitors forward physical key state, never typed text. Dispatch UI
// actions to the main thread; the Windows hook must return promptly.
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
    if let Some(action) = action {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || match action {
            shortcut::Action::Start => start(&handle),
            shortcut::Action::Release => release(&handle),
            shortcut::Action::Cancel => cancel(&handle),
        });
    }
}
pub fn install(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let enabled = std::fs::read_to_string(app.path().app_config_dir()?.join("dictation-enabled"))
        .is_ok_and(|value| value == "true")
        && crate::openai_credentials::is_configured().unwrap_or(false);
    let _ = APP.set(app.clone());
    app.manage(Dictation {
        hold_shortcut: Mutex::new(shortcut::HoldShortcut::default()),
        enabled: AtomicBool::new(enabled),
        next_id: AtomicU64::new(1),
        session: Mutex::new(None),
        startup_error: Mutex::new(None),
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
pub fn shutdown() {
    unsafe {
        pulse_dictation_unregister_shortcut();
        pulse_dictation_stop();
    }
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
    let microphone = unsafe { pulse_dictation_mic_allowed() };
    let accessibility = unsafe { pulse_dictation_ax_allowed() };
    let api_key = crate::openai_credentials::is_configured().unwrap_or(false);
    let mut error = state.startup_error.lock().unwrap();
    // Permissions can change in System Settings without a new recording or
    // toggle. Retire only the diagnostic whose prerequisite is now satisfied.
    let resolved = match error.as_deref() {
        Some(ACCESSIBILITY_REQUIRED) => accessibility,
        Some(MICROPHONE_REQUIRED) => microphone,
        Some(API_KEY_REQUIRED) => api_key,
        _ => false,
    };
    if resolved {
        *error = None;
    }
    Ok(
        json!({"enabled":state.enabled.load(Ordering::Acquire), "shortcut":SHORTCUT, "microphone":microphone, "accessibility":accessibility, "apiKeyConfigured":api_key, "error":*error}),
    )
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
        if old && enabled && !crate::openai_credentials::is_configured()? {
            return Err(API_KEY_REQUIRED.into());
        }
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
#[derive(serde::Deserialize)]
pub struct GlassFrame {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    opacity: f64,
}
impl GlassFrame {
    fn valid(&self, max_width: f64, max_height: f64) -> bool {
        [self.x, self.y, self.width, self.height, self.opacity]
            .iter()
            .all(|value| value.is_finite())
            && (0.0..=max_width).contains(&self.width)
            && (0.0..=max_height).contains(&self.height)
            && (0.0..=1.0).contains(&self.opacity)
    }
}
#[tauri::command]
pub async fn dictation_glass(
    app: tauri::AppHandle,
    window: WebviewWindow,
    session_id: u64,
    frame: GlassFrame,
    transcript: GlassFrame,
    dark_mode: bool,
) -> Result<bool, String> {
    if window.label() != WINDOW {
        return Err("This is only available to the dictation overlay.".into());
    }
    // Clipboard confirmation expands the pill beyond waveform width.
    if !frame.valid(180.0, 40.0) {
        return Err("Invalid glass frame.".into());
    }
    if !transcript.valid(400.0, 140.0) {
        return Err("Invalid transcript frame.".into());
    }
    let handle = app.clone();
    on_main(&app, move || {
        if handle
            .state::<Dictation>()
            .session
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|s| s.id != session_id)
        {
            return Ok(false);
        }
        let pointer = native_window(&window)?;
        Ok(unsafe {
            pulse_dictation_transcript_blur(
                pointer,
                transcript.x,
                transcript.y,
                transcript.width,
                transcript.height,
                transcript.opacity,
                dark_mode,
            );
            pulse_dictation_glass(
                pointer,
                frame.x,
                frame.y,
                frame.width,
                frame.height,
                frame.opacity,
                dark_mode,
            )
        })
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
            json!({"id":s.id,"phase":s.phase,"text":s.text,"level":s.level,"samples":s.samples,"bottom":s.bottom})
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
    s.level = level;
}
// Resolve the current input only at dispatch, after physical modifiers rise.
// Recording never reads, locks, or edits a field.
fn delivery_tick(app: &tauri::AppHandle, id: u64) {
    let state = app.state::<Dictation>();
    let mut guard = state.session.lock().unwrap();
    let Some(s) = guard.as_mut().filter(|s| {
        s.id == id
            && s.phase == "sending"
            && !s.cancel.is_cancelled()
            && s.delivery == FinalDelivery::Pending
    }) else {
        return;
    };
    let text = CString::new(s.text.replace('\0', "")).unwrap();
    s.delivery = match unsafe { pulse_dictation_final_step(text.as_ptr()) } {
        0 => FinalDelivery::Pending,
        1 => FinalDelivery::Submitted,
        _ => FinalDelivery::Clipboard,
    };
}

extern "C" fn capture_ready(context: *mut c_void, started: bool) {
    // Native code returns this allocation exactly once, even on startup failure.
    let sender = unsafe { Box::from_raw(context.cast::<oneshot::Sender<bool>>()) };
    let _ = sender.send(started);
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
            .is_some_and(|s| !matches!(s.phase, "idle" | "done" | "copied" | "error"))
        {
            state.hold_shortcut.lock().unwrap().interrupt();
            return;
        }
        if let Some(previous) = guard.take() {
            previous.cancel.cancel();
        }
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
    let mut bottom = 24.0;
    if let Ok(pointer) = native_window(&window) {
        unsafe {
            pulse_dictation_position(pointer, &mut bottom);
        }
    }
    let (tx, rx) = mpsc::channel(256); // Bound the microphone's startup/network backlog.
    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let token = CancellationToken::new();
    *state.session.lock().unwrap() = Some(Session {
        id,
        audio: Some(tx),
        cancel: token.clone(),
        error: None,
        phase: "listening",
        text: String::new(),
        level: 0.0,
        samples: 0,
        bottom,
        delivery: FinalDelivery::Pending,
    });
    let _ = window.eval(format!("window.pulseDictationStart?.({id}, {bottom});"));
    let _ = window.set_ignore_cursor_events(true);
    let mic = unsafe { pulse_dictation_mic_allowed() };
    let (ready, capture) = oneshot::channel();
    if mic {
        // Return to AppKit immediately so the reset webview and native glass can
        // appear while AVAudioEngine prepares on its serial lifecycle queue.
        unsafe {
            pulse_dictation_start_async(
                id,
                audio_callback,
                capture_ready,
                Box::into_raw(Box::new(ready)).cast(),
            );
        }
    } else {
        let _ = ready.send(false);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let started = capture.await.unwrap_or(false);
        let result = if !mic {
            Err(MICROPHONE_REQUIRED.into())
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
    if let Some(s) = state.session.lock().unwrap().as_mut() {
        s.audio.take(); // Drain queued PCM before the final commit.
        s.phase = "finalizing";
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
        .filter(|s| s.id == id && !s.cancel.is_cancelled())
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
    let mut deadline = Instant::now();
    let mut microphone_deadline = Instant::now() + Duration::from_secs(4);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline), if committed => return Err("OpenAI did not finish the transcript in time.".into()),
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
        let text = match result {
            Ok(text) => text,
            Err(error) if error == "cancelled" => {
                s.phase = "done";
                s.text.clear();
                return Ok(None);
            }
            Err(error) => {
                // Operational diagnostics belong in Settings, never in the pill.
                *state.startup_error.lock().unwrap() = Some(error);
                s.text.clone()
            }
        };
        if text.trim().is_empty() {
            s.phase = "done";
            return Ok(None);
        }
        s.text = text.clone();
        s.phase = "sending";
        // Resolve the selected input at dispatch, never at recording start.
        s.delivery = FinalDelivery::Pending;
        // Capture failures may cancel the network but should still deliver the
        // partial words. Explicit cancellation never delivers anything.
        s.cancel = CancellationToken::new();
        Ok(Some(text))
    })
    .await;
    if let Ok(Some(_)) = prepared {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let expired = Instant::now() >= deadline;
            let handle = app.clone();
            let complete = on_main(&app, move || {
                // A submitted paste never becomes a second clipboard fallback.
                delivery_tick(&handle, id);
                let state = handle.state::<Dictation>();
                let mut guard = state.session.lock().unwrap();
                let Some(s) = guard.as_mut().filter(|s| s.id == id) else {
                    return Ok(true);
                };
                s.phase = match s.final_delivery(expired) {
                    FinalDelivery::Pending => return Ok(false),
                    FinalDelivery::Clipboard => {
                        let text = CString::new(s.text.replace('\0', "")).unwrap();
                        if unsafe { pulse_dictation_copy(text.as_ptr()) } {
                            "copied"
                        } else {
                            *state.startup_error.lock().unwrap() =
                                Some("Could not copy the transcript to the clipboard.".into());
                            "done"
                        }
                    }
                    FinalDelivery::Submitted | FinalDelivery::Discard => "done",
                };
                Ok(true)
            })
            .await
            .unwrap_or(true);
            if complete {
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }
    // Give the clipboard-only confirmation time to expand and be read. A new
    // recording can replace it immediately; this task must not fade that session.
    let copied = app
        .state::<Dictation>()
        .session
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.id == id && s.phase == "copied");
    if copied {
        tokio::time::sleep(Duration::from_millis(1600)).await;
        let handle = app.clone();
        let _ = on_main(&app, move || {
            let state = handle.state::<Dictation>();
            let mut guard = state.session.lock().unwrap();
            if let Some(s) = guard.as_mut().filter(|s| s.id == id && s.phase == "copied") {
                s.phase = "done";
            }
            Ok(())
        })
        .await;
    }
    // Allow the lens contraction/fade plus one overlay polling interval.
    let wait = app
        .state::<Dictation>()
        .session
        .lock()
        .unwrap()
        .as_ref()
        .map(|_| 400)
        .unwrap_or(0);
    tokio::time::sleep(Duration::from_millis(wait)).await;
    let handle = app.clone();
    let _ = on_main(&app, move || {
        let state = handle.state::<Dictation>();
        let mut guard = state.session.lock().unwrap();
        if guard.as_ref().is_some_and(|s| s.id == id) {
            *guard = None;
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

    fn test_session() -> Session {
        let (tx, _) = mpsc::channel(1);
        Session {
            id: 1,
            audio: Some(tx),
            cancel: CancellationToken::new(),
            error: None,
            phase: "listening",
            text: "Keep these words".into(),
            level: 0.1,
            samples: 10,
            bottom: 28.0,
            delivery: FinalDelivery::Pending,
        }
    }

    #[test]
    fn final_delivery_waits_for_dispatch_or_copies_once() {
        let mut session = test_session();
        assert_eq!(session.final_delivery(false), FinalDelivery::Pending);
        session.delivery = FinalDelivery::Submitted;
        assert_eq!(session.final_delivery(false), FinalDelivery::Submitted);
        session.delivery = FinalDelivery::Clipboard;
        assert_eq!(session.final_delivery(false), FinalDelivery::Clipboard);
        session.delivery = FinalDelivery::Pending;
        assert_eq!(session.final_delivery(true), FinalDelivery::Clipboard);
        session.cancel.cancel();
        assert_eq!(session.final_delivery(false), FinalDelivery::Discard);
    }

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
