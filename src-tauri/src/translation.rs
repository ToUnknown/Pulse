use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
#[cfg(target_os = "windows")]
use cpal::SupportedStreamConfig;
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, SampleFormat, Stream, StreamConfig,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::menu::{CheckMenuItem, IconMenuItem, Menu, Submenu};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{header::AUTHORIZATION, HeaderValue},
        Message,
    },
};

const API_KEY_SERVICE: &str = "app.pulse.desktop";
const API_KEY_ACCOUNT: &str = "openai-api-key";
const TRANSLATION_URL: &str =
    "wss://api.openai.com/v1/realtime/translations?model=gpt-realtime-translate";
const REALTIME_SAMPLE_RATE: u32 = 24_000;
const REALTIME_FRAME_SAMPLES: usize = 4_800;
const INPUT_QUEUE_FRAMES: usize = 10;
const OUTPUT_PREBUFFER_MS: usize = 200;
const PASSTHROUGH_PREBUFFER_MS: usize = 20;
const MAX_OUTPUT_BUFFER_SECONDS: usize = 10;
const MAX_PASSTHROUGH_BUFFER_SECONDS: usize = 1;
const RECONNECT_DELAYS_MS: [u64; 3] = [250, 1_000, 3_000];
const RESAMPLER_HALF_TAPS: usize = 24;
#[cfg(target_os = "macos")]
const TRANSLATION_MENU_INDEX: usize = 0;
#[cfg(target_os = "windows")]
const TRANSLATION_MENU_INDEX: usize = 1;
#[cfg(target_os = "macos")]
const PULSE_DRIVER_SAMPLE_RATE: u32 = 48_000;
#[cfg(target_os = "macos")]
const PULSE_DRIVER_PACKET_SAMPLES: usize = 480;
#[cfg(target_os = "macos")]
const PULSE_DRIVER_ADDRESS: &str = "127.0.0.1:41873";
#[cfg(target_os = "macos")]
const INSTALLED_PULSE_DRIVER: &str = "/Library/Audio/Plug-Ins/HAL/Pulse.driver";
const TRANSLATION_OFF_MENU_ICON_BYTES: &[u8] = include_bytes!("../icons/menu/translation-off.png");
const TRANSLATION_BOOTING_MENU_ICON_BYTES: &[u8] =
    include_bytes!("../icons/menu/translation-booting.png");
const TRANSLATION_READY_MENU_ICON_BYTES: &[u8] =
    include_bytes!("../icons/menu/translation-ready.png");

#[derive(Clone, Copy)]
enum TranslationMenuState {
    Off,
    Booting,
    Ready,
}

fn set_translation_menu_state(
    item: &IconMenuItem<tauri::Wry>,
    text: &str,
    state: TranslationMenuState,
) {
    let icon_bytes = match state {
        TranslationMenuState::Off => TRANSLATION_OFF_MENU_ICON_BYTES,
        TranslationMenuState::Booting => TRANSLATION_BOOTING_MENU_ICON_BYTES,
        TranslationMenuState::Ready => TRANSLATION_READY_MENU_ICON_BYTES,
    };
    let icon = tauri::image::Image::from_bytes(icon_bytes).ok();

    let _ = item.set_text(text);
    let _ = item.set_icon(icon);
}

pub(crate) const LANGUAGES: [(&str, &str); 13] = [
    ("en", "English"),
    ("es", "Spanish"),
    ("pt", "Portuguese"),
    ("fr", "French"),
    ("ja", "Japanese"),
    ("ru", "Russian"),
    ("zh", "Chinese"),
    ("de", "German"),
    ("ko", "Korean"),
    ("hi", "Hindi"),
    ("id", "Indonesian"),
    ("vi", "Vietnamese"),
    ("it", "Italian"),
];

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranslationConfig {
    #[serde(default)]
    enabled: bool,
    target_language: String,
    input_device: Option<String>,
}

impl Default for TranslationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            target_language: "en".to_string(),
            input_device: None,
        }
    }
}

impl TranslationConfig {
    fn load(path: &Path) -> Self {
        let mut config: Self = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();

        if language_label(&config.target_language).is_none() {
            config.target_language = Self::default().target_language;
        }
        config.input_device = normalize_device_name(config.input_device);
        config
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        fs::write(path, bytes).map_err(|error| error.to_string())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TranslationStatus {
    Idle,
    Starting,
    Running,
    Stopping,
}

impl TranslationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
        }
    }

    fn is_active(self) -> bool {
        self != Self::Idle
    }
}

pub(crate) struct TranslationManager {
    config_path: PathBuf,
    config: Arc<Mutex<TranslationConfig>>,
    status: Arc<Mutex<TranslationStatus>>,
    stop_signal: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    passthrough: Arc<Mutex<Option<PassthroughTask>>>,
    last_error: Arc<Mutex<Option<String>>>,
    session_id: Arc<Mutex<Option<String>>>,
    session_number: Arc<AtomicUsize>,
    dropped_input_frames: Arc<AtomicUsize>,
    menu: Menu<tauri::Wry>,
    language_menu: Submenu<tauri::Wry>,
    menu_items_visible: Mutex<bool>,
    start_item: IconMenuItem<tauri::Wry>,
    language_items: Vec<(String, CheckMenuItem<tauri::Wry>)>,
}

impl TranslationManager {
    pub(crate) fn new(
        config_path: PathBuf,
        menu: Menu<tauri::Wry>,
        language_menu: Submenu<tauri::Wry>,
        start_item: IconMenuItem<tauri::Wry>,
        language_items: Vec<(String, CheckMenuItem<tauri::Wry>)>,
    ) -> tauri::Result<Self> {
        let config = TranslationConfig::load(&config_path);
        let manager = Self {
            config_path,
            config: Arc::new(Mutex::new(config)),
            status: Arc::new(Mutex::new(TranslationStatus::Idle)),
            stop_signal: Arc::new(Mutex::new(None)),
            passthrough: Arc::new(Mutex::new(None)),
            last_error: Arc::new(Mutex::new(None)),
            session_id: Arc::new(Mutex::new(None)),
            session_number: Arc::new(AtomicUsize::new(0)),
            dropped_input_frames: Arc::new(AtomicUsize::new(0)),
            menu,
            language_menu,
            menu_items_visible: Mutex::new(true),
            start_item,
            language_items,
        };
        manager.sync_language_menu();
        manager.set_menu_items_visible(manager.is_enabled())?;
        if manager.is_enabled() {
            if let Err(error) = manager.start_passthrough() {
                manager.set_last_error(error);
            }
        }
        Ok(manager)
    }

    pub(crate) fn state_json(&self) -> serde_json::Value {
        let config = self
            .config
            .lock()
            .map(|config| config.clone())
            .unwrap_or_default();
        let status = self
            .status
            .lock()
            .map(|status| *status)
            .unwrap_or(TranslationStatus::Idle);
        let last_error = self.last_error.lock().ok().and_then(|error| error.clone());
        let session_id = self
            .session_id
            .lock()
            .ok()
            .and_then(|session_id| session_id.clone());
        let session_number = self.session_number.load(Ordering::Acquire);
        let dropped_input_frames = self.dropped_input_frames.load(Ordering::Acquire);
        let (input_devices, mut audio_device_error) = match input_devices() {
            Ok(input) => (input, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        if audio_device_error.is_none() {
            audio_device_error = self.passthrough_error();
        }
        let virtual_output_available = match virtual_microphone_available() {
            Ok(available) => available,
            Err(error) => {
                if audio_device_error.is_none() {
                    audio_device_error = Some(error);
                }
                false
            }
        };

        json!({
            "enabled": config.enabled,
            "apiKeyConfigured": api_key_is_configured().unwrap_or(false),
            "targetLanguage": config.target_language,
            "inputDevice": config.input_device,
            "inputDevices": input_devices,
            "virtualOutputAvailable": virtual_output_available,
            "status": status.as_str(),
            "sessionId": session_id,
            "sessionNumber": session_number,
            "droppedInputFrames": dropped_input_frames,
            "lastError": last_error,
            "audioDeviceError": audio_device_error,
        })
    }

    pub(crate) fn select_language(&self, code: &str) -> Result<(), String> {
        if language_label(code).is_none() {
            return Err("unsupported translation language".to_string());
        }
        if self.current_status()?.is_active() {
            return Err("stop translation before changing the language".to_string());
        }

        self.update_config(|config| config.target_language = code.to_string())?;
        self.sync_language_menu();
        Ok(())
    }

    pub(crate) fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut next = self
            .config
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        next.enabled = enabled;
        next.save(&self.config_path)?;
        *self.config.lock().map_err(|error| error.to_string())? = next;
        if !enabled {
            self.stop()?;
            self.stop_passthrough()?;
            if let Ok(mut session_id) = self.session_id.lock() {
                *session_id = None;
            }
            self.session_number.store(0, Ordering::Release);
            self.dropped_input_frames.store(0, Ordering::Release);
        }
        self.set_menu_items_visible(enabled)
            .map_err(|error| error.to_string())?;
        self.clear_last_error();
        Ok(())
    }

    pub(crate) fn set_input_device(&self, name: Option<String>) -> Result<(), String> {
        let name = normalize_device_name(name);
        if let Some(name) = name.as_deref() {
            resolve_input_device(Some(name))?;
        }
        self.stop_passthrough()?;
        if let Err(error) = self.update_config(|config| config.input_device = name) {
            if !self.current_status()?.is_active() && self.is_enabled() {
                let _ = self.start_passthrough();
            }
            return Err(error);
        }
        self.start_passthrough()
    }

    pub(crate) fn toggle(&self) -> Result<(), String> {
        match self.current_status()? {
            TranslationStatus::Idle => {
                let result = self.start();
                if let Err(error) = &result {
                    if let Ok(mut last_error) = self.last_error.lock() {
                        *last_error = Some(error.clone());
                    }
                    set_translation_menu_state(
                        &self.start_item,
                        "Translation Failed — Retry",
                        TranslationMenuState::Off,
                    );
                }
                result
            }
            TranslationStatus::Starting | TranslationStatus::Running => self.stop(),
            TranslationStatus::Stopping => Ok(()),
        }
    }

    pub(crate) fn stop(&self) -> Result<(), String> {
        let status = self.current_status()?;
        if !status.is_active() {
            return Ok(());
        }

        if let Some(signal) = self
            .stop_signal
            .lock()
            .map_err(|error| error.to_string())?
            .as_ref()
        {
            signal.store(true, Ordering::Release);
        }
        *self.status.lock().map_err(|error| error.to_string())? = TranslationStatus::Stopping;
        set_translation_menu_state(
            &self.start_item,
            "Stopping Translation…",
            TranslationMenuState::Booting,
        );
        Ok(())
    }

    pub(crate) fn stop_and_wait(&self) -> Result<(), String> {
        self.stop()?;
        for _ in 0..100 {
            if !self.current_status()?.is_active() {
                return self.stop_passthrough();
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err("translation did not stop before removing the Pulse virtual input".to_string())
    }

    pub(crate) fn clear_last_error(&self) {
        if let Ok(mut last_error) = self.last_error.lock() {
            *last_error = None;
        }
        if matches!(self.current_status(), Ok(TranslationStatus::Idle)) {
            set_translation_menu_state(
                &self.start_item,
                "Start Translation",
                TranslationMenuState::Off,
            );
        }
    }

    fn start(&self) -> Result<(), String> {
        let api_key = load_api_key()?;
        let config = self
            .config
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        if !config.enabled {
            return Err("turn on Live Translate in Settings before starting".to_string());
        }
        // Resolve devices before changing menu state so configuration mistakes
        // remain a synchronous, actionable error.
        resolve_input_device(config.input_device.as_deref())?;
        ensure_virtual_microphone_available()?;
        self.stop_passthrough()?;

        let stop_signal = Arc::new(AtomicBool::new(false));
        *self.status.lock().map_err(|error| error.to_string())? = TranslationStatus::Starting;
        *self.stop_signal.lock().map_err(|error| error.to_string())? = Some(stop_signal.clone());
        if let Ok(mut last_error) = self.last_error.lock() {
            *last_error = None;
        }
        *self.session_id.lock().map_err(|error| error.to_string())? = None;
        self.session_number.store(0, Ordering::Release);
        self.dropped_input_frames.store(0, Ordering::Release);
        self.set_language_menu_enabled(false);
        set_translation_menu_state(
            &self.start_item,
            "Starting Translation…",
            TranslationMenuState::Booting,
        );

        let status = self.status.clone();
        let active_stop_signal = self.stop_signal.clone();
        let shared_config = self.config.clone();
        let passthrough = self.passthrough.clone();
        let last_error = self.last_error.clone();
        let session_id = self.session_id.clone();
        let session_number = self.session_number.clone();
        let dropped_input_frames = self.dropped_input_frames.clone();
        let start_item = self.start_item.clone();
        let language_items = self.language_items.clone();

        thread::spawn(move || {
            let session_context = TranslationSessionContext {
                stop_signal: stop_signal.clone(),
                status: status.clone(),
                start_item: start_item.clone(),
                session_id,
                session_number,
                dropped_input_frames,
            };
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let result = match runtime {
                Ok(runtime) => runtime.block_on(run_translation_with_reconnect(
                    api_key,
                    config,
                    session_context,
                )),
                Err(error) => Err(format!("translation runtime setup failed: {error}")),
            };

            let passthrough_result = shared_config
                .lock()
                .map_err(|error| error.to_string())
                .and_then(|config| {
                    if config.enabled {
                        start_passthrough_task(&config, &passthrough)
                    } else {
                        Ok(())
                    }
                });

            if let Ok(mut current_status) = status.lock() {
                *current_status = TranslationStatus::Idle;
            }
            if let Ok(mut current_signal) = active_stop_signal.lock() {
                if current_signal
                    .as_ref()
                    .is_some_and(|signal| Arc::ptr_eq(signal, &stop_signal))
                {
                    *current_signal = None;
                }
            }
            for (_, item) in &language_items {
                let _ = item.set_enabled(true);
            }

            match (result, passthrough_result) {
                (Ok(()), Ok(())) => {
                    if let Ok(mut error) = last_error.lock() {
                        *error = None;
                    }
                    set_translation_menu_state(
                        &start_item,
                        "Start Translation",
                        TranslationMenuState::Off,
                    );
                }
                (Ok(()), Err(error)) => {
                    eprintln!("Pulse microphone passthrough stopped: {error}");
                    if let Ok(mut last_error) = last_error.lock() {
                        *last_error = Some(error);
                    }
                    set_translation_menu_state(
                        &start_item,
                        "Start Translation",
                        TranslationMenuState::Off,
                    );
                }
                (Err(error), _) => {
                    eprintln!("live translation stopped: {error}");
                    if let Ok(mut last_error) = last_error.lock() {
                        *last_error = Some(error);
                    }
                    set_translation_menu_state(
                        &start_item,
                        "Translation Failed — Retry",
                        TranslationMenuState::Off,
                    );
                }
            }
        });

        Ok(())
    }

    pub(crate) fn start_passthrough(&self) -> Result<(), String> {
        if self.current_status()?.is_active() || !self.is_enabled() {
            return Ok(());
        }
        let config = self
            .config
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        match start_passthrough_task(&config, &self.passthrough) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.set_last_error(error.clone());
                Err(error)
            }
        }
    }

    fn stop_passthrough(&self) -> Result<(), String> {
        self.passthrough
            .lock()
            .map_err(|error| error.to_string())?
            .take();
        Ok(())
    }

    fn passthrough_error(&self) -> Option<String> {
        self.passthrough
            .lock()
            .ok()
            .and_then(|task| task.as_ref().and_then(PassthroughTask::error))
    }

    fn set_last_error(&self, error: String) {
        if let Ok(mut last_error) = self.last_error.lock() {
            *last_error = Some(error);
        }
    }

    fn update_config(&self, update: impl FnOnce(&mut TranslationConfig)) -> Result<(), String> {
        if self.current_status()?.is_active() {
            return Err("stop translation before changing translation settings".to_string());
        }

        let mut next = self
            .config
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        if !next.enabled {
            return Err("turn on Live Translate before changing translation settings".to_string());
        }
        update(&mut next);
        next.save(&self.config_path)?;
        *self.config.lock().map_err(|error| error.to_string())? = next;
        self.clear_last_error();
        Ok(())
    }

    fn current_status(&self) -> Result<TranslationStatus, String> {
        self.status
            .lock()
            .map(|status| *status)
            .map_err(|error| error.to_string())
    }

    fn sync_language_menu(&self) {
        let selected = self
            .config
            .lock()
            .map(|config| config.target_language.clone())
            .unwrap_or_else(|_| "en".to_string());
        for (code, item) in &self.language_items {
            let _ = item.set_checked(code == &selected);
        }
    }

    fn set_language_menu_enabled(&self, enabled: bool) {
        for (_, item) in &self.language_items {
            let _ = item.set_enabled(enabled);
        }
    }

    fn is_enabled(&self) -> bool {
        self.config
            .lock()
            .map(|config| config.enabled)
            .unwrap_or(false)
    }

    fn set_menu_items_visible(&self, visible: bool) -> tauri::Result<()> {
        let mut current = self
            .menu_items_visible
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        if *current == visible {
            return Ok(());
        }

        if visible {
            self.menu.insert_items(
                &[&self.language_menu, &self.start_item],
                TRANSLATION_MENU_INDEX,
            )?;
        } else {
            self.menu.remove(&self.language_menu)?;
            self.menu.remove(&self.start_item)?;
        }
        *current = visible;
        Ok(())
    }
}

pub(crate) fn save_api_key(api_key: &str) -> Result<(), String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("enter an OpenAI API key".to_string());
    }
    api_key_entry()?
        .set_password(api_key)
        .map_err(|error| format!("secure credential storage failed: {error}"))
}

pub(crate) fn clear_api_key() -> Result<(), String> {
    match api_key_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("secure credential removal failed: {error}")),
    }
}

fn api_key_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(API_KEY_SERVICE, API_KEY_ACCOUNT)
        .map_err(|error| format!("secure credential storage is unavailable: {error}"))
}

fn api_key_is_configured() -> Result<bool, String> {
    match api_key_entry()?.get_password() {
        Ok(api_key) => Ok(!api_key.trim().is_empty()),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(format!("secure credential lookup failed: {error}")),
    }
}

fn load_api_key() -> Result<String, String> {
    match api_key_entry()?.get_password() {
        Ok(api_key) if !api_key.trim().is_empty() => Ok(api_key),
        Ok(_) | Err(keyring::Error::NoEntry) => {
            Err("add an OpenAI API key in Settings before starting".to_string())
        }
        Err(error) => Err(format!("secure credential lookup failed: {error}")),
    }
}

fn language_label(code: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find_map(|(candidate, label)| (*candidate == code).then_some(*label))
}

fn normalize_device_name(name: Option<String>) -> Option<String> {
    name.map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn input_devices() -> Result<Vec<String>, String> {
    let host = cpal::default_host();
    let mut input = host
        .input_devices()
        .map_err(|error| format!("could not list microphones: {error}"))?
        .filter_map(|device| device_name(&device).ok())
        .filter(|name| !is_virtual_audio_name(name))
        .collect::<Vec<_>>();
    input.sort_unstable();
    input.dedup();
    Ok(input)
}

fn device_name(device: &Device) -> Result<String, String> {
    device
        .description()
        .map(|description| description.name().to_string())
        .map_err(|error| error.to_string())
}

fn resolve_input_device(name: Option<&str>) -> Result<Device, String> {
    let device = match name {
        Some(name) => find_named_input_device(name),
        None => cpal::default_host()
            .default_input_device()
            .ok_or_else(|| "no default microphone is available".to_string()),
    }?;
    let name = device_name(&device)?;
    if is_virtual_audio_name(&name) {
        return Err(
            "the selected microphone is virtual; choose the physical microphone to prevent an audio loop"
                .to_string(),
        );
    }
    Ok(device)
}

fn find_named_input_device(name: &str) -> Result<Device, String> {
    let device = cpal::default_host()
        .input_devices()
        .map_err(|error| format!("could not list microphones: {error}"))?
        .find(|device| device_name(device).is_ok_and(|candidate| candidate == name));
    device.ok_or_else(|| format!("audio device is unavailable: {name}"))
}

#[cfg(target_os = "windows")]
fn resolve_virtual_output_device() -> Result<Device, String> {
    find_virtual_output_device()?.ok_or_else(|| {
        "the Pulse virtual microphone is unavailable; finish its installation or restart the computer"
            .to_string()
    })
}

#[cfg(target_os = "macos")]
fn virtual_microphone_available() -> Result<bool, String> {
    if !Path::new(INSTALLED_PULSE_DRIVER).is_dir() {
        return Ok(false);
    }
    cpal::default_host()
        .input_devices()
        .map_err(|error| format!("could not list audio inputs: {error}"))
        .map(|mut devices| {
            devices.any(|device| {
                device_name(&device).is_ok_and(|name| name.eq_ignore_ascii_case("Pulse"))
            })
        })
}

#[cfg(target_os = "windows")]
fn virtual_microphone_available() -> Result<bool, String> {
    find_virtual_output_device().map(|device| device.is_some())
}

fn ensure_virtual_microphone_available() -> Result<(), String> {
    virtual_microphone_available()?.then_some(()).ok_or_else(|| {
        #[cfg(target_os = "macos")]
        let message = "the Pulse virtual microphone is unavailable; turn Live Translate off and back on once to install or migrate it";
        #[cfg(target_os = "windows")]
        let message = "the Pulse virtual microphone is unavailable; finish its installation or restart the computer";
        message.to_string()
    })
}

#[cfg(any(target_os = "windows", test))]
fn virtual_output_priority(name: &str) -> Option<u8> {
    let name = name.to_ascii_lowercase();
    if name == "pulse" {
        return Some(0);
    }
    if name.contains("cable input") {
        return Some(1);
    }
    if name.contains("vb-cable") || name.contains("vb cable") {
        return Some(2);
    }
    [
        "blackhole",
        "virtual audio cable",
        "voicemeeter input",
        "loopback audio",
        "soundflower",
    ]
    .iter()
    .position(|candidate| name.contains(candidate))
    .map(|position| position as u8 + 3)
}

#[cfg(target_os = "windows")]
fn find_virtual_output_device() -> Result<Option<Device>, String> {
    let devices = cpal::default_host()
        .output_devices()
        .map_err(|error| format!("could not list audio outputs: {error}"))?;
    let mut candidates = devices
        .filter_map(|device| {
            let name = device_name(&device).ok()?;
            let priority = virtual_output_priority(&name)?;
            Some((priority, name.to_ascii_lowercase(), device))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    Ok(candidates.into_iter().next().map(|(_, _, device)| device))
}

fn is_virtual_audio_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if name == "pulse" {
        return true;
    }
    [
        "blackhole",
        "vb-cable",
        "vb cable",
        "cable input",
        "cable output",
        "virtual audio cable",
        "voicemeeter",
        "loopback audio",
        "soundflower",
    ]
    .iter()
    .any(|candidate| name.contains(candidate))
}

struct PassthroughTask {
    _input_stream: Stream,
    #[cfg(target_os = "macos")]
    _output_stream: MacosPulseOutput,
    #[cfg(target_os = "windows")]
    _output_stream: Stream,
    audio_error: Arc<Mutex<Option<String>>>,
}

impl PassthroughTask {
    fn error(&self) -> Option<String> {
        self.audio_error.lock().ok().and_then(|error| error.clone())
    }
}

fn start_passthrough_task(
    config: &TranslationConfig,
    task: &Arc<Mutex<Option<PassthroughTask>>>,
) -> Result<(), String> {
    let mut task = task.lock().map_err(|error| error.to_string())?;
    if task.as_ref().is_some_and(|task| task.error().is_none()) {
        return Ok(());
    }
    task.take();

    let input_device = resolve_input_device(config.input_device.as_deref())?;
    ensure_virtual_microphone_available()?;
    let input_config = input_device
        .default_input_config()
        .map_err(|error| format!("microphone format is unavailable: {error}"))?;

    #[cfg(target_os = "macos")]
    let output_sample_rate = PULSE_DRIVER_SAMPLE_RATE;
    #[cfg(target_os = "windows")]
    let (output_device, output_config) = {
        let device = resolve_virtual_output_device()?;
        let config = preferred_output_config(&device)?;
        (device, config)
    };
    #[cfg(target_os = "windows")]
    let output_sample_rate = output_config.sample_rate();

    let max_output_samples = output_sample_rate as usize * MAX_PASSTHROUGH_BUFFER_SECONDS;
    let output_queue = Arc::new(Mutex::new(OutputBuffer::with_prebuffer(
        output_sample_rate,
        max_output_samples,
        PASSTHROUGH_PREBUFFER_MS,
    )));
    let audio_error = Arc::new(Mutex::new(None::<String>));

    #[cfg(target_os = "macos")]
    let output_stream = MacosPulseOutput::new(output_queue.clone(), audio_error.clone())?;
    #[cfg(target_os = "windows")]
    let output_stream = build_output_stream(
        &output_device,
        output_config.sample_format(),
        output_config.into(),
        output_queue.clone(),
        audio_error.clone(),
    )?;

    let input_processor = PassthroughChunker::new(
        input_config.sample_rate(),
        input_config.channels() as usize,
        output_sample_rate,
        output_queue,
    );
    let input_stream = build_input_stream(
        &input_device,
        input_config.sample_format(),
        input_config.into(),
        input_processor,
        audio_error.clone(),
    )?;

    #[cfg(target_os = "macos")]
    output_stream.play()?;
    #[cfg(target_os = "windows")]
    output_stream
        .play()
        .map_err(|error| format!("could not start virtual microphone output: {error}"))?;
    input_stream
        .play()
        .map_err(|error| format!("could not start microphone passthrough: {error}"))?;

    *task = Some(PassthroughTask {
        _input_stream: input_stream,
        _output_stream: output_stream,
        audio_error,
    });
    Ok(())
}

#[derive(Clone)]
struct TranslationSessionContext {
    stop_signal: Arc<AtomicBool>,
    status: Arc<Mutex<TranslationStatus>>,
    start_item: IconMenuItem<tauri::Wry>,
    session_id: Arc<Mutex<Option<String>>>,
    session_number: Arc<AtomicUsize>,
    dropped_input_frames: Arc<AtomicUsize>,
}

async fn run_translation_with_reconnect(
    api_key: String,
    config: TranslationConfig,
    context: TranslationSessionContext,
) -> Result<(), String> {
    let mut reconnect_attempt = 0;
    loop {
        let connection_started_at = tokio::time::Instant::now();
        let result =
            run_translation_session(api_key.clone(), config.clone(), context.clone()).await;

        let error = match result {
            Ok(()) => return Ok(()),
            Err(_) if context.stop_signal.load(Ordering::Acquire) => return Ok(()),
            Err(error) => error,
        };
        if connection_started_at.elapsed() >= Duration::from_secs(30) {
            reconnect_attempt = 0;
        }
        if !is_reconnectable_session_error(&error) || reconnect_attempt >= RECONNECT_DELAYS_MS.len()
        {
            return Err(error);
        }

        if let Ok(mut current_status) = context.status.lock() {
            *current_status = TranslationStatus::Starting;
        }
        set_translation_menu_state(
            &context.start_item,
            "Reconnecting Translation…",
            TranslationMenuState::Booting,
        );
        eprintln!("OpenAI translation session reconnecting after: {error}");
        let delay = Duration::from_millis(RECONNECT_DELAYS_MS[reconnect_attempt]);
        reconnect_attempt += 1;
        tokio::select! {
            _ = wait_for_stop(context.stop_signal.clone()) => return Ok(()),
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

fn is_reconnectable_session_error(error: &str) -> bool {
    [
        "could not connect to OpenAI Live Translate",
        "OpenAI Live Translate connection timed out",
        "OpenAI closed the translation connection",
        "translation setup failed",
        "translation connection failed",
        "could not configure translation",
        "could not stream microphone audio",
        "OpenAI did not confirm the translation configuration",
    ]
    .iter()
    .any(|prefix| error.starts_with(prefix))
}

fn session_closed_result(closing: bool) -> Result<(), String> {
    if closing {
        Ok(())
    } else {
        Err("OpenAI closed the translation connection".to_string())
    }
}

async fn run_translation_session(
    api_key: String,
    config: TranslationConfig,
    context: TranslationSessionContext,
) -> Result<(), String> {
    let TranslationSessionContext {
        stop_signal,
        status,
        start_item,
        session_id,
        session_number,
        dropped_input_frames,
    } = context;
    let input_device = resolve_input_device(config.input_device.as_deref())?;
    ensure_virtual_microphone_available()?;

    let input_config = input_device
        .default_input_config()
        .map_err(|error| format!("microphone format is unavailable: {error}"))?;
    #[cfg(target_os = "macos")]
    let output_sample_rate = PULSE_DRIVER_SAMPLE_RATE;
    #[cfg(target_os = "windows")]
    let (output_device, output_config) = {
        let device = resolve_virtual_output_device()?;
        let config = preferred_output_config(&device)?;
        (device, config)
    };
    #[cfg(target_os = "windows")]
    let output_sample_rate = output_config.sample_rate();
    let max_output_samples = output_sample_rate as usize * MAX_OUTPUT_BUFFER_SECONDS;
    let output_queue = Arc::new(Mutex::new(OutputBuffer::new(
        output_sample_rate,
        max_output_samples,
    )));
    let audio_error = Arc::new(Mutex::new(None::<String>));
    #[cfg(target_os = "macos")]
    let output_stream = MacosPulseOutput::new(output_queue.clone(), audio_error.clone())?;
    #[cfg(target_os = "windows")]
    let output_stream = build_output_stream(
        &output_device,
        output_config.sample_format(),
        output_config.into(),
        output_queue.clone(),
        audio_error.clone(),
    )?;

    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(INPUT_QUEUE_FRAMES);
    let input_processor = InputChunker::new(
        input_config.sample_rate(),
        input_config.channels() as usize,
        input_tx,
        audio_error.clone(),
        dropped_input_frames,
    );
    let input_stream = build_input_stream(
        &input_device,
        input_config.sample_format(),
        input_config.into(),
        input_processor,
        audio_error.clone(),
    )?;

    let mut request = TRANSLATION_URL
        .into_client_request()
        .map_err(|error| format!("translation request setup failed: {error}"))?;
    let authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
        .map_err(|_| "the stored OpenAI API key is invalid".to_string())?;
    request.headers_mut().insert(AUTHORIZATION, authorization);

    let connection = tokio::select! {
        result = connect_async(request) => result
            .map_err(|error| format!("could not connect to OpenAI Live Translate: {error}"))?,
        _ = wait_for_stop(stop_signal.clone()) => return Ok(()),
        _ = tokio::time::sleep(Duration::from_secs(15)) => {
            return Err("OpenAI Live Translate connection timed out".to_string());
        }
    };
    let (socket, _) = connection;
    let (mut writer, mut reader) = socket.split();

    writer
        .send(Message::Text(
            translation_session_update(&config).to_string().into(),
        ))
        .await
        .map_err(|error| format!("could not configure translation: {error}"))?;

    let configured = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = wait_for_stop(stop_signal.clone()) => return Ok(false),
                message = reader.next() => {
                    let Some(message) = message else {
                        return Err("OpenAI closed the translation connection during setup".to_string());
                    };
                    let message = message
                        .map_err(|error| format!("translation setup failed: {error}"))?;
                    match message {
                        Message::Text(text) => {
                            let event: serde_json::Value = serde_json::from_str(text.as_ref())
                                .map_err(|error| format!("invalid translation setup event: {error}"))?;
                            match event.get("type").and_then(|value| value.as_str()) {
                                Some("session.created") => {
                                    record_translation_session(
                                        &event,
                                        &session_id,
                                        &session_number,
                                    );
                                }
                                Some("session.updated") => return Ok(true),
                                Some("error") => return Err(translation_error_message(&event)),
                                _ => {}
                            }
                        }
                        Message::Close(_) => {
                            return Err("OpenAI closed the translation connection during setup".to_string());
                        }
                        _ => {}
                    }
                }
            }
        }
    })
    .await
    .map_err(|_| "OpenAI did not confirm the translation configuration".to_string())??;
    if !configured {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    output_stream.play()?;
    #[cfg(target_os = "windows")]
    output_stream
        .play()
        .map_err(|error| format!("could not start virtual microphone output: {error}"))?;
    input_stream
        .play()
        .map_err(|error| format!("could not start microphone capture: {error}"))?;

    if let Ok(mut current_status) = status.lock() {
        *current_status = TranslationStatus::Running;
    }
    set_translation_menu_state(&start_item, "Stop Translation", TranslationMenuState::Ready);

    let mut output_source_rate = REALTIME_SAMPLE_RATE;
    let mut output_resampler = WindowedSincResampler::new(output_source_rate, output_sample_rate);
    let mut closing = false;
    let mut close_deadline = None;

    loop {
        if let Some(error) = audio_error.lock().ok().and_then(|mut error| error.take()) {
            return Err(error);
        }

        if stop_signal.load(Ordering::Acquire) && !closing {
            closing = true;
            let _ = input_stream.pause();
            writer
                .send(Message::Text(
                    json!({ "type": "session.close" }).to_string().into(),
                ))
                .await
                .map_err(|error| format!("could not close translation cleanly: {error}"))?;
            close_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(3));
        }

        if close_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            return Ok(());
        }

        tokio::select! {
            frame = input_rx.recv(), if !closing => {
                let Some(frame) = frame else {
                    return Err("microphone capture stopped unexpectedly".to_string());
                };
                writer
                    .send(Message::Text(json!({
                        "type": "session.input_audio_buffer.append",
                        "audio": BASE64.encode(frame),
                    }).to_string().into()))
                    .await
                    .map_err(|error| format!("could not stream microphone audio: {error}"))?;
            }
            message = reader.next() => {
                let Some(message) = message else {
                    return if closing {
                        Ok(())
                    } else {
                        Err("OpenAI closed the translation connection".to_string())
                    };
                };
                let message = message.map_err(|error| format!("translation connection failed: {error}"))?;
                match message {
                    Message::Text(text) => {
                        let event: serde_json::Value = serde_json::from_str(text.as_ref())
                            .map_err(|error| format!("invalid translation event: {error}"))?;
                        match event.get("type").and_then(|value| value.as_str()) {
                            Some("session.output_audio.delta") => {
                                let format = event
                                    .get("format")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("pcm16");
                                if format != "pcm16" {
                                    return Err(format!("unsupported translated audio format: {format}"));
                                }
                                let channels = event
                                    .get("channels")
                                    .and_then(|value| value.as_u64())
                                    .unwrap_or(1);
                                let channels = usize::try_from(channels)
                                    .ok()
                                    .filter(|channels| (1..=8).contains(channels))
                                    .ok_or_else(|| {
                                        "translated audio reported an invalid channel count"
                                            .to_string()
                                    })?;
                                let sample_rate = event
                                    .get("sample_rate")
                                    .and_then(|value| value.as_u64())
                                    .unwrap_or(REALTIME_SAMPLE_RATE as u64);
                                let sample_rate = u32::try_from(sample_rate)
                                    .ok()
                                    .filter(|rate| (8_000..=384_000).contains(rate))
                                    .ok_or_else(|| "translated audio reported an invalid sample rate".to_string())?;
                                let delta = event
                                    .get("delta")
                                    .and_then(|value| value.as_str())
                                    .ok_or_else(|| "translation audio event had no data".to_string())?;
                                let bytes = BASE64
                                    .decode(delta)
                                    .map_err(|error| format!("invalid translated audio: {error}"))?;
                                if bytes.len() % (channels * 2) != 0 {
                                    return Err("translated audio delta was not a complete PCM frame".to_string());
                                }
                                let pcm = bytes
                                    .chunks_exact(channels * 2)
                                    .map(|frame| {
                                        frame
                                            .chunks_exact(2)
                                            .map(|sample| {
                                                i16::from_le_bytes([sample[0], sample[1]]) as f32
                                                    / i16::MAX as f32
                                            })
                                            .sum::<f32>()
                                            / channels as f32
                                    })
                                    .collect::<Vec<_>>();
                                if sample_rate != output_source_rate {
                                    output_source_rate = sample_rate;
                                    output_resampler = WindowedSincResampler::new(
                                        output_source_rate,
                                        output_sample_rate,
                                    );
                                }
                                let resampled = output_resampler.process(&pcm);
                                if let Ok(mut queue) = output_queue.lock() {
                                    queue.push(resampled);
                                }
                            }
                            Some("session.closed") => return session_closed_result(closing),
                            Some("error") => {
                                eprintln!(
                                    "OpenAI translation warning: {}",
                                    translation_error_message(&event)
                                );
                            }
                            _ => {}
                        }
                    }
                    Message::Close(_) => {
                        return if closing {
                            Ok(())
                        } else {
                            Err("OpenAI closed the translation connection".to_string())
                        };
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
    }
}

fn translation_session_update(config: &TranslationConfig) -> serde_json::Value {
    json!({
        "type": "session.update",
        "session": {
            "audio": {
                "input": {
                    "noise_reduction": {
                        "type": "near_field"
                    },
                    "transcription": null
                },
                "output": {
                    "language": config.target_language
                }
            }
        }
    })
}

fn record_translation_session(
    event: &serde_json::Value,
    session_id: &Arc<Mutex<Option<String>>>,
    session_number: &Arc<AtomicUsize>,
) {
    let Some(id) = event
        .pointer("/session/id")
        .and_then(|value| value.as_str())
    else {
        return;
    };
    let number = session_number.fetch_add(1, Ordering::AcqRel) + 1;
    if let Ok(mut current_session_id) = session_id.lock() {
        *current_session_id = Some(id.to_string());
    }
    eprintln!("OpenAI translation session {number} started: {id}");
}

async fn wait_for_stop(stop_signal: Arc<AtomicBool>) {
    while !stop_signal.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn translation_error_message(event: &serde_json::Value) -> String {
    event
        .pointer("/error/message")
        .and_then(|value| value.as_str())
        .unwrap_or("OpenAI rejected the translation session")
        .to_string()
}

#[cfg(target_os = "macos")]
struct MacosPulseOutput {
    started: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(target_os = "macos")]
impl MacosPulseOutput {
    fn new(
        queue: Arc<Mutex<OutputBuffer>>,
        audio_error: Arc<Mutex<Option<String>>>,
    ) -> Result<Self, String> {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0")
            .map_err(|error| format!("could not open the Pulse audio channel: {error}"))?;
        socket
            .connect(PULSE_DRIVER_ADDRESS)
            .map_err(|error| format!("could not connect to the Pulse audio driver: {error}"))?;

        let started = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_started = started.clone();
        let thread_stop = stop.clone();
        let handle = thread::Builder::new()
            .name("pulse-virtual-microphone".to_string())
            .spawn(move || {
                let _ = socket.send(&[0]);
                let period = Duration::from_millis(10);
                let mut next_packet = std::time::Instant::now();
                let mut packet = [0.0_f32; PULSE_DRIVER_PACKET_SAMPLES];

                while !thread_stop.load(Ordering::Acquire) {
                    if !thread_started.load(Ordering::Acquire) {
                        thread::sleep(period);
                        next_packet = std::time::Instant::now();
                        continue;
                    }

                    if let Ok(mut queue) = queue.lock() {
                        for sample in &mut packet {
                            *sample = queue.next_sample();
                        }
                    } else {
                        packet.fill(0.0);
                    }

                    let bytes = unsafe {
                        std::slice::from_raw_parts(
                            packet.as_ptr().cast::<u8>(),
                            std::mem::size_of_val(&packet),
                        )
                    };
                    if let Err(error) = socket.send(bytes) {
                        if let Ok(mut stored_error) = audio_error.lock() {
                            *stored_error = Some(format!(
                                "Pulse virtual microphone transport failed: {error}"
                            ));
                        }
                        break;
                    }

                    next_packet += period;
                    let now = std::time::Instant::now();
                    if next_packet > now {
                        thread::sleep(next_packet - now);
                    } else if now.duration_since(next_packet) > Duration::from_millis(100) {
                        next_packet = now;
                    }
                }
                let _ = socket.send(&[0]);
            })
            .map_err(|error| format!("could not start the Pulse audio channel: {error}"))?;

        Ok(Self {
            started,
            stop,
            thread: Some(handle),
        })
    }

    fn play(&self) -> Result<(), String> {
        if self
            .thread
            .as_ref()
            .is_some_and(thread::JoinHandle::is_finished)
        {
            return Err("the Pulse audio channel stopped unexpectedly".to_string());
        }
        self.started.store(true, Ordering::Release);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl Drop for MacosPulseOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(target_os = "windows")]
fn preferred_output_config(device: &Device) -> Result<SupportedStreamConfig, String> {
    let native_model_rate = device.supported_output_configs().ok().and_then(|configs| {
        configs
            .filter(|config| {
                matches!(
                    config.sample_format(),
                    SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
                )
            })
            .filter_map(|config| config.try_with_sample_rate(REALTIME_SAMPLE_RATE))
            .min_by_key(|config| {
                let channels = config.channels();
                let channel_rank = if channels == 1 { 0 } else { channels };
                let format_rank = match config.sample_format() {
                    SampleFormat::F32 => 0,
                    SampleFormat::I16 => 1,
                    SampleFormat::U16 => 2,
                    _ => 3,
                };
                (channel_rank, format_rank)
            })
    });

    match native_model_rate {
        Some(config) => Ok(config),
        None => device
            .default_output_config()
            .map_err(|error| format!("virtual output format is unavailable: {error}")),
    }
}

trait MicrophoneProcessor: Send + 'static {
    fn push_f32(&mut self, data: &[f32]);
    fn push_i16(&mut self, data: &[i16]);
    fn push_u16(&mut self, data: &[u16]);
}

fn build_input_stream<Processor: MicrophoneProcessor>(
    device: &Device,
    sample_format: SampleFormat,
    config: StreamConfig,
    mut processor: Processor,
    audio_error: Arc<Mutex<Option<String>>>,
) -> Result<Stream, String> {
    let stream_error = audio_error.clone();
    let error_callback = move |error| {
        if let Ok(mut stored_error) = stream_error.lock() {
            *stored_error = Some(format!("microphone capture failed: {error}"));
        }
    };

    match sample_format {
        SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| processor.push_f32(data),
                error_callback,
                None,
            )
            .map_err(|error| format!("could not open microphone: {error}")),
        SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _| processor.push_i16(data),
                error_callback,
                None,
            )
            .map_err(|error| format!("could not open microphone: {error}")),
        SampleFormat::U16 => device
            .build_input_stream(
                config,
                move |data: &[u16], _| processor.push_u16(data),
                error_callback,
                None,
            )
            .map_err(|error| format!("could not open microphone: {error}")),
        _ => Err(format!(
            "the microphone uses an unsupported sample format: {sample_format}"
        )),
    }
}

#[cfg(target_os = "windows")]
fn build_output_stream(
    device: &Device,
    sample_format: SampleFormat,
    config: StreamConfig,
    queue: Arc<Mutex<OutputBuffer>>,
    audio_error: Arc<Mutex<Option<String>>>,
) -> Result<Stream, String> {
    let channels = config.channels as usize;
    let error_callback = move |error| {
        if let Ok(mut stored_error) = audio_error.lock() {
            *stored_error = Some(format!("virtual microphone output failed: {error}"));
        }
    };

    match sample_format {
        SampleFormat::F32 => device
            .build_output_stream(
                config,
                move |data: &mut [f32], _| fill_output_f32(data, channels, &queue),
                error_callback,
                None,
            )
            .map_err(|error| format!("could not open virtual microphone output: {error}")),
        SampleFormat::I16 => device
            .build_output_stream(
                config,
                move |data: &mut [i16], _| fill_output_i16(data, channels, &queue),
                error_callback,
                None,
            )
            .map_err(|error| format!("could not open virtual microphone output: {error}")),
        SampleFormat::U16 => device
            .build_output_stream(
                config,
                move |data: &mut [u16], _| fill_output_u16(data, channels, &queue),
                error_callback,
                None,
            )
            .map_err(|error| format!("could not open virtual microphone output: {error}")),
        _ => Err(format!(
            "the virtual microphone uses an unsupported sample format: {sample_format}"
        )),
    }
}

struct InputChunker {
    channels: usize,
    resampler: WindowedSincResampler,
    pending: Vec<f32>,
    sender: tokio::sync::mpsc::Sender<Vec<u8>>,
    audio_error: Arc<Mutex<Option<String>>>,
    dropped_frames: Arc<AtomicUsize>,
}

impl InputChunker {
    fn new(
        sample_rate: u32,
        channels: usize,
        sender: tokio::sync::mpsc::Sender<Vec<u8>>,
        audio_error: Arc<Mutex<Option<String>>>,
        dropped_frames: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            channels,
            resampler: WindowedSincResampler::new(sample_rate, REALTIME_SAMPLE_RATE),
            pending: Vec::with_capacity(REALTIME_FRAME_SAMPLES * 2),
            sender,
            audio_error,
            dropped_frames,
        }
    }

    fn push_f32(&mut self, data: &[f32]) {
        self.push_mono(default_microphone_channel(data, self.channels));
    }

    fn push_i16(&mut self, data: &[i16]) {
        let normalized = data
            .iter()
            .map(|sample| *sample as f32 / i16::MAX as f32)
            .collect::<Vec<_>>();
        self.push_mono(default_microphone_channel(&normalized, self.channels));
    }

    fn push_u16(&mut self, data: &[u16]) {
        let normalized = data
            .iter()
            .map(|sample| (*sample as f32 - 32_768.0) / 32_768.0)
            .collect::<Vec<_>>();
        self.push_mono(default_microphone_channel(&normalized, self.channels));
    }

    fn push_mono(&mut self, mono: Vec<f32>) {
        self.pending.extend(self.resampler.process(&mono));

        while self.pending.len() >= REALTIME_FRAME_SAMPLES {
            let remaining = self.pending.split_off(REALTIME_FRAME_SAMPLES);
            let frame = std::mem::replace(&mut self.pending, remaining);
            let mut bytes = Vec::with_capacity(REALTIME_FRAME_SAMPLES * 2);
            for sample in frame {
                let sample = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            if let Err(error) = self.sender.try_send(bytes) {
                match error {
                    tokio::sync::mpsc::error::TrySendError::Full(_) => {
                        self.dropped_frames.fetch_add(1, Ordering::Relaxed);
                    }
                    tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                        if let Ok(mut stored_error) = self.audio_error.lock() {
                            *stored_error = Some(
                                "the translation connection stopped accepting microphone audio"
                                    .to_string(),
                            );
                        }
                        return;
                    }
                }
            }
        }
    }
}

impl MicrophoneProcessor for InputChunker {
    fn push_f32(&mut self, data: &[f32]) {
        InputChunker::push_f32(self, data);
    }

    fn push_i16(&mut self, data: &[i16]) {
        InputChunker::push_i16(self, data);
    }

    fn push_u16(&mut self, data: &[u16]) {
        InputChunker::push_u16(self, data);
    }
}

struct PassthroughChunker {
    channels: usize,
    resampler: WindowedSincResampler,
    output_queue: Arc<Mutex<OutputBuffer>>,
}

impl PassthroughChunker {
    fn new(
        input_sample_rate: u32,
        channels: usize,
        output_sample_rate: u32,
        output_queue: Arc<Mutex<OutputBuffer>>,
    ) -> Self {
        Self {
            channels,
            resampler: WindowedSincResampler::new(input_sample_rate, output_sample_rate),
            output_queue,
        }
    }

    fn push_mono(&mut self, mono: Vec<f32>) {
        let samples = self.resampler.process(&mono);
        if let Ok(mut queue) = self.output_queue.lock() {
            queue.push(samples);
        }
    }
}

impl MicrophoneProcessor for PassthroughChunker {
    fn push_f32(&mut self, data: &[f32]) {
        self.push_mono(default_microphone_channel(data, self.channels));
    }

    fn push_i16(&mut self, data: &[i16]) {
        let normalized = data
            .iter()
            .map(|sample| *sample as f32 / i16::MAX as f32)
            .collect::<Vec<_>>();
        self.push_mono(default_microphone_channel(&normalized, self.channels));
    }

    fn push_u16(&mut self, data: &[u16]) {
        let normalized = data
            .iter()
            .map(|sample| (*sample as f32 - 32_768.0) / 32_768.0)
            .collect::<Vec<_>>();
        self.push_mono(default_microphone_channel(&normalized, self.channels));
    }
}

fn default_microphone_channel(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }

    interleaved
        .chunks_exact(channels)
        .map(|frame| frame[0])
        .collect()
}

struct WindowedSincResampler {
    step: f64,
    position: f64,
    buffer: Vec<f32>,
    cutoff: f64,
    passthrough: bool,
}

impl WindowedSincResampler {
    fn new(input_rate: u32, output_rate: u32) -> Self {
        let passthrough = input_rate == output_rate;
        Self {
            step: input_rate as f64 / output_rate as f64,
            position: RESAMPLER_HALF_TAPS as f64,
            buffer: if passthrough {
                Vec::new()
            } else {
                vec![0.0; RESAMPLER_HALF_TAPS]
            },
            cutoff: 0.47 * (output_rate as f64 / input_rate as f64).min(1.0),
            passthrough,
        }
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.passthrough {
            return input.to_vec();
        }

        self.buffer.extend_from_slice(input);
        let mut output = Vec::with_capacity(
            ((input.len() as f64 / self.step).ceil() as usize).saturating_add(1),
        );

        while self.position + RESAMPLER_HALF_TAPS as f64 <= self.buffer.len() as f64 {
            let center = self.position.floor() as isize;
            let fraction = self.position - center as f64;
            let mut sample = 0.0_f64;
            let mut weight_sum = 0.0_f64;

            for tap in -(RESAMPLER_HALF_TAPS as isize)..RESAMPLER_HALF_TAPS as isize {
                let distance = tap as f64 - fraction;
                let sinc = if distance.abs() < f64::EPSILON {
                    2.0 * self.cutoff
                } else {
                    (2.0 * std::f64::consts::PI * self.cutoff * distance).sin()
                        / (std::f64::consts::PI * distance)
                };
                let window = 0.5
                    + 0.5 * (std::f64::consts::PI * distance / RESAMPLER_HALF_TAPS as f64).cos();
                let weight = sinc * window;
                sample += self.buffer[(center + tap) as usize] as f64 * weight;
                weight_sum += weight;
            }

            output.push(if weight_sum.abs() > f64::EPSILON {
                (sample / weight_sum) as f32
            } else {
                0.0
            });
            self.position += self.step;
        }

        let consumed = (self.position.floor() as usize).saturating_sub(RESAMPLER_HALF_TAPS);
        if consumed > 0 {
            self.buffer.drain(..consumed.min(self.buffer.len()));
            self.position -= consumed as f64;
        }
        output
    }
}

struct OutputBuffer {
    samples: VecDeque<f32>,
    primed: bool,
    prebuffer_samples: usize,
    max_samples: usize,
}

impl OutputBuffer {
    fn new(sample_rate: u32, max_samples: usize) -> Self {
        Self::with_prebuffer(sample_rate, max_samples, OUTPUT_PREBUFFER_MS)
    }

    fn with_prebuffer(sample_rate: u32, max_samples: usize, prebuffer_ms: usize) -> Self {
        Self {
            samples: VecDeque::new(),
            primed: false,
            prebuffer_samples: sample_rate as usize * prebuffer_ms / 1_000,
            max_samples,
        }
    }

    fn push(&mut self, samples: Vec<f32>) {
        self.samples.extend(samples);
        if self.samples.len() > self.max_samples {
            let overflow = self.samples.len() - self.max_samples;
            self.samples.drain(..overflow);
        }
    }

    fn next_sample(&mut self) -> f32 {
        if !self.primed {
            if self.samples.len() < self.prebuffer_samples {
                return 0.0;
            }
            self.primed = true;
        }

        match self.samples.pop_front() {
            Some(sample) => sample,
            None => {
                self.primed = false;
                0.0
            }
        }
    }
}

#[cfg(any(target_os = "windows", test))]
fn fill_output_f32(data: &mut [f32], channels: usize, queue: &Arc<Mutex<OutputBuffer>>) {
    let Ok(mut queue) = queue.lock() else {
        data.fill(0.0);
        return;
    };
    for frame in data.chunks_mut(channels) {
        let sample = queue.next_sample();
        frame.fill(sample);
    }
}

#[cfg(target_os = "windows")]
fn fill_output_i16(data: &mut [i16], channels: usize, queue: &Arc<Mutex<OutputBuffer>>) {
    let Ok(mut queue) = queue.lock() else {
        data.fill(0);
        return;
    };
    for frame in data.chunks_mut(channels) {
        let sample = (queue.next_sample().clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        frame.fill(sample);
    }
}

#[cfg(target_os = "windows")]
fn fill_output_u16(data: &mut [u16], channels: usize, queue: &Arc<Mutex<OutputBuffer>>) {
    let Ok(mut queue) = queue.lock() else {
        data.fill(32_768);
        return;
    };
    for frame in data.chunks_mut(channels) {
        let normalized = queue.next_sample().clamp(-1.0, 1.0);
        let sample = ((normalized + 1.0) * 32_767.5) as u16;
        frame.fill(sample);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn received_rms(receiver: &mut tokio::sync::mpsc::Receiver<Vec<u8>>) -> f64 {
        let mut sum_squares = 0.0_f64;
        let mut sample_count = 0_usize;
        while let Ok(frame) = receiver.try_recv() {
            for sample in frame.chunks_exact(2) {
                let sample = i16::from_le_bytes([sample[0], sample[1]]) as f64 / i16::MAX as f64;
                sum_squares += sample * sample;
                sample_count += 1;
            }
        }
        (sum_squares / sample_count.max(1) as f64).sqrt()
    }

    #[test]
    fn translation_session_enables_near_field_noise_reduction() {
        let config = TranslationConfig {
            target_language: "es".to_string(),
            ..TranslationConfig::default()
        };

        assert_eq!(
            translation_session_update(&config),
            json!({
                "type": "session.update",
                "session": {
                    "audio": {
                        "input": {
                            "noise_reduction": {
                                "type": "near_field"
                            },
                            "transcription": null
                        },
                        "output": {
                            "language": "es"
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn session_created_records_the_openai_session_identity() {
        let session_id = Arc::new(Mutex::new(None));
        let session_number = Arc::new(AtomicUsize::new(0));

        record_translation_session(
            &json!({
                "type": "session.created",
                "session": { "id": "sess_test" }
            }),
            &session_id,
            &session_number,
        );

        assert_eq!(
            session_id.lock().expect("session id lock").as_deref(),
            Some("sess_test")
        );
        assert_eq!(session_number.load(Ordering::Acquire), 1);
    }

    #[test]
    fn stream_timing_backpressure_does_not_end_microphone_capture() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let audio_error = Arc::new(Mutex::new(None));
        let dropped_frames = Arc::new(AtomicUsize::new(0));
        let mut chunker = InputChunker::new(
            REALTIME_SAMPLE_RATE,
            1,
            sender,
            audio_error.clone(),
            dropped_frames.clone(),
        );

        chunker.push_f32(&vec![0.25; REALTIME_FRAME_SAMPLES * 2]);

        assert_eq!(
            audio_error.lock().expect("audio error lock").as_deref(),
            None,
            "temporary WebSocket backpressure must not stop the translation session"
        );
        assert_eq!(dropped_frames.load(Ordering::Acquire), 1);
    }

    #[test]
    fn stream_timing_output_waits_for_a_complete_start_buffer() {
        let queued_samples = REALTIME_SAMPLE_RATE as usize / 10;
        let mut output_buffer = OutputBuffer::new(
            REALTIME_SAMPLE_RATE,
            REALTIME_SAMPLE_RATE as usize * MAX_OUTPUT_BUFFER_SECONDS,
        );
        output_buffer.push(vec![0.5; queued_samples]);
        let queue = Arc::new(Mutex::new(output_buffer));
        let mut output = vec![1.0; REALTIME_SAMPLE_RATE as usize / 50];

        fill_output_f32(&mut output, 1, &queue);

        assert!(
            output.iter().all(|sample| *sample == 0.0),
            "a partial first delta must not play before the output buffer is ready"
        );
        assert_eq!(
            queue.lock().expect("output queue lock").samples.len(),
            queued_samples,
            "prebuffering must retain the first audio delta"
        );
    }

    #[test]
    fn stream_timing_output_reprimes_after_an_underrun() {
        let prebuffer_samples = REALTIME_SAMPLE_RATE as usize * OUTPUT_PREBUFFER_MS / 1_000;
        let mut output_buffer = OutputBuffer::new(
            REALTIME_SAMPLE_RATE,
            REALTIME_SAMPLE_RATE as usize * MAX_OUTPUT_BUFFER_SECONDS,
        );
        output_buffer.push(vec![0.5; prebuffer_samples]);
        let queue = Arc::new(Mutex::new(output_buffer));
        let mut first_output = vec![1.0; prebuffer_samples + 1];

        fill_output_f32(&mut first_output, 1, &queue);
        assert!(first_output[..prebuffer_samples]
            .iter()
            .all(|sample| *sample == 0.5));
        assert_eq!(first_output[prebuffer_samples], 0.0);

        let partial_samples = prebuffer_samples / 2;
        queue
            .lock()
            .expect("output queue lock")
            .push(vec![0.25; partial_samples]);
        let mut second_output = vec![1.0; partial_samples];
        fill_output_f32(&mut second_output, 1, &queue);

        assert!(second_output.iter().all(|sample| *sample == 0.0));
        assert_eq!(
            queue.lock().expect("output queue lock").samples.len(),
            partial_samples
        );
    }

    #[test]
    fn documented_session_endings_are_reconnectable() {
        assert!(is_reconnectable_session_error(
            "OpenAI closed the translation connection"
        ));
        assert!(is_reconnectable_session_error(
            "translation connection failed: connection reset"
        ));
        assert!(!is_reconnectable_session_error(
            "unsupported translated audio format: mp3"
        ));
    }

    #[test]
    fn pulse_output_routing_is_fixed_and_deterministic() {
        assert_eq!(virtual_output_priority("Pulse"), Some(0));
        assert_eq!(
            virtual_output_priority("CABLE Input (VB-Audio Virtual Cable)"),
            Some(1)
        );
        assert_eq!(virtual_output_priority("VB-Cable"), Some(2));
        assert_eq!(virtual_output_priority("MacBook Pro Speakers"), None);
    }

    #[test]
    fn unexpected_session_closed_event_requests_reconnect() {
        assert!(session_closed_result(true).is_ok());
        assert_eq!(
            session_closed_result(false),
            Err("OpenAI closed the translation connection".to_string())
        );
    }

    #[test]
    fn multichannel_microphone_uses_the_default_channel_for_every_callback() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
        let audio_error = Arc::new(Mutex::new(None));
        let mut chunker = InputChunker::new(
            48_000,
            2,
            sender,
            audio_error,
            Arc::new(AtomicUsize::new(0)),
        );
        let frames = 48_000;
        let mut input = Vec::with_capacity(frames * 2);

        for index in 0..frames {
            let sample = (2.0 * std::f32::consts::PI * 440.0 * index as f32 / 48_000.0).sin() * 0.5;
            input.extend_from_slice(&[sample, -sample]);
        }

        for callback in input.chunks(960) {
            chunker.push_f32(callback);
        }

        let rms = received_rms(&mut receiver);

        assert!(
            rms > 0.1,
            "the default microphone channel lost its signal (RMS {rms:.4})"
        );

        assert_eq!(
            default_microphone_channel(&[0.25, 0.9, 0.5, -0.9], 2),
            vec![0.25, 0.5]
        );
    }

    #[test]
    fn integer_microphone_formats_preserve_signal_level() {
        let frames = 48_000;
        let source = (0..frames)
            .map(|index| (2.0 * std::f32::consts::PI * 440.0 * index as f32 / 48_000.0).sin() * 0.5)
            .collect::<Vec<_>>();

        let (i16_sender, mut i16_receiver) = tokio::sync::mpsc::channel(16);
        let mut i16_chunker = InputChunker::new(
            48_000,
            2,
            i16_sender,
            Arc::new(Mutex::new(None)),
            Arc::new(AtomicUsize::new(0)),
        );
        let i16_input = source
            .iter()
            .flat_map(|sample| {
                let sample = (*sample * i16::MAX as f32) as i16;
                [sample, -sample]
            })
            .collect::<Vec<_>>();
        for callback in i16_input.chunks(960) {
            i16_chunker.push_i16(callback);
        }

        let (u16_sender, mut u16_receiver) = tokio::sync::mpsc::channel(16);
        let mut u16_chunker = InputChunker::new(
            48_000,
            2,
            u16_sender,
            Arc::new(Mutex::new(None)),
            Arc::new(AtomicUsize::new(0)),
        );
        let u16_input = source
            .iter()
            .flat_map(|sample| {
                let positive = ((*sample + 1.0) * 32_767.5) as u16;
                let negative = ((-*sample + 1.0) * 32_767.5) as u16;
                [positive, negative]
            })
            .collect::<Vec<_>>();
        for callback in u16_input.chunks(960) {
            u16_chunker.push_u16(callback);
        }

        assert!(received_rms(&mut i16_receiver) > 0.3);
        assert!(received_rms(&mut u16_receiver) > 0.3);
    }

    #[test]
    fn resampler_preserves_speech_band_frequency_and_level() {
        let mut resampler = WindowedSincResampler::new(44_100, REALTIME_SAMPLE_RATE);
        let input = (0..44_100)
            .map(|index| {
                (2.0 * std::f32::consts::PI * 1_000.0 * index as f32 / 44_100.0).sin() * 0.5
            })
            .collect::<Vec<_>>();
        let mut output = Vec::new();
        for callback in input.chunks(257) {
            output.extend(resampler.process(callback));
        }

        let analyzed = &output[500..];
        let rms = (analyzed
            .iter()
            .map(|sample| (*sample as f64).powi(2))
            .sum::<f64>()
            / analyzed.len() as f64)
            .sqrt();
        let positive_crossings = analyzed
            .windows(2)
            .filter(|samples| samples[0] <= 0.0 && samples[1] > 0.0)
            .count();
        let duration = analyzed.len() as f64 / REALTIME_SAMPLE_RATE as f64;
        let measured_frequency = positive_crossings as f64 / duration;

        assert!((0.33..=0.37).contains(&rms), "unexpected RMS {rms:.4}");
        assert!(
            (measured_frequency - 1_000.0).abs() < 5.0,
            "unexpected frequency {measured_frequency:.2} Hz"
        );
    }
}
