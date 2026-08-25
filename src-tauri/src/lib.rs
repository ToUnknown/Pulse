use tauri::{
    menu::{IconMenuItem, Menu, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod translation;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod virtual_audio;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use {
    std::{
        fs,
        path::Path,
        sync::{Arc, Mutex},
    },
    tauri_plugin_autostart::{MacosLauncher, ManagerExt},
    tauri_plugin_dialog::{DialogExt, MessageDialogKind},
    tauri_plugin_updater::{Update, UpdaterExt},
};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use tauri::{
    menu::{CheckMenuItem, IsMenuItem, Submenu},
    WebviewUrl, WebviewWindowBuilder,
};

#[cfg(target_os = "macos")]
use {
    block2::RcBlock,
    objc2::MainThreadMarker,
    objc2_app_kit::{NSApp, NSAppearanceNameAqua, NSAppearanceNameDarkAqua},
    objc2_foundation::{
        NSArray, NSDistributedNotificationCenter, NSNotification, NSOperationQueue, NSString,
    },
    std::ptr::NonNull,
    tauri::Manager as _,
};

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct DownloadedUpdate {
    update: Update,
    bytes: Vec<u8>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
enum UpdateStatus {
    Idle,
    Checking,
    Downloading,
    Ready(Box<DownloadedUpdate>),
    Installing,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateResult {
    Ready,
    UpToDate,
    ReleaseBuildRequired,
    Failed(&'static str),
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl UpdateResult {
    fn feedback(self) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::UpToDate => Some("Pulse is up to date."),
            Self::ReleaseBuildRequired => Some("Update checks require a release build."),
            Self::Failed(message) => Some(message),
        }
    }

    fn is_error(self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
type SharedUpdateStatus = Arc<Mutex<UpdateStatus>>;

#[cfg(any(target_os = "macos", target_os = "windows"))]
const CHECK_FOR_UPDATES_MENU_ICON_BYTES: &[u8] =
    include_bytes!("../icons/menu/check-for-updates.png");
#[cfg(any(target_os = "macos", target_os = "windows"))]
const RESTART_TO_UPDATE_MENU_ICON_BYTES: &[u8] =
    include_bytes!("../icons/menu/restart-to-update.png");
#[cfg(any(target_os = "macos", target_os = "windows"))]
const SETTINGS_MENU_ICON_BYTES: &[u8] = include_bytes!("../icons/menu/settings.png");
#[cfg(any(target_os = "macos", target_os = "windows"))]
const QUIT_MENU_ICON_BYTES: &[u8] = include_bytes!("../icons/menu/quit.png");
#[cfg(any(target_os = "macos", target_os = "windows"))]
const TRANSLATION_MENU_ICON_BYTES: &[u8] = include_bytes!("../icons/menu/translation.png");

#[cfg(target_os = "windows")]
const APPEARANCE_MENU_ICON_BYTES: &[u8] = include_bytes!("../icons/menu/appearance.png");
#[cfg(target_os = "windows")]
const AUTO_MENU_ICON_BYTES: &[u8] = include_bytes!("../icons/menu/auto.png");
#[cfg(target_os = "windows")]
const AUTO_SELECTED_MENU_ICON_BYTES: &[u8] = include_bytes!("../icons/menu/auto-selected.png");
#[cfg(target_os = "windows")]
const LIGHT_MENU_ICON_BYTES: &[u8] = include_bytes!("../icons/menu/light.png");
#[cfg(target_os = "windows")]
const LIGHT_SELECTED_MENU_ICON_BYTES: &[u8] = include_bytes!("../icons/menu/light-selected.png");
#[cfg(target_os = "windows")]
const DARK_MENU_ICON_BYTES: &[u8] = include_bytes!("../icons/menu/dark.png");
#[cfg(target_os = "windows")]
const DARK_SELECTED_MENU_ICON_BYTES: &[u8] = include_bytes!("../icons/menu/dark-selected.png");

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn set_update_menu(update_item: &IconMenuItem<tauri::Wry>, update_ready: bool, enabled: bool) {
    let text = if update_ready {
        "Restart to Update"
    } else {
        "Check for Updates"
    };
    let icon_bytes = if update_ready {
        RESTART_TO_UPDATE_MENU_ICON_BYTES
    } else {
        CHECK_FOR_UPDATES_MENU_ICON_BYTES
    };
    let icon = tauri::image::Image::from_bytes(icon_bytes).ok();

    let _ = update_item.set_text(text);
    let _ = update_item.set_icon(icon);
    let _ = update_item.set_enabled(enabled);
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn reveal_update_menu(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(tray) = app.tray_by_id("pulse-tray") else {
            return;
        };

        let _ = tray.with_inner_tray_icon(|tray| tray.show_menu());
    });
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn set_update_result(
    app: &tauri::AppHandle,
    update_item: &IconMenuItem<tauri::Wry>,
    result: UpdateResult,
    reveal_result: bool,
) {
    set_update_menu(update_item, result == UpdateResult::Ready, true);
    if !reveal_result {
        return;
    }

    if let Some(message) = result.feedback() {
        let kind = if result.is_error() {
            MessageDialogKind::Error
        } else {
            MessageDialogKind::Info
        };
        app.dialog()
            .message(message)
            .kind(kind)
            .title("Pulse")
            .show(|_| {});
    } else {
        reveal_update_menu(app);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn begin_update_check(status: &SharedUpdateStatus) -> bool {
    let Ok(mut status) = status.lock() else {
        return false;
    };

    match &*status {
        UpdateStatus::Idle => {
            *status = UpdateStatus::Checking;
            true
        }
        UpdateStatus::Checking
        | UpdateStatus::Downloading
        | UpdateStatus::Ready(_)
        | UpdateStatus::Installing => false,
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn reset_update_status(status: &SharedUpdateStatus) {
    if let Ok(mut status) = status.lock() {
        *status = UpdateStatus::Idle;
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
async fn check_for_updates(
    app: tauri::AppHandle,
    update_item: IconMenuItem<tauri::Wry>,
    status: SharedUpdateStatus,
    tray_icon_mode: Arc<Mutex<TrayIconMode>>,
    reveal_result: bool,
) {
    if !begin_update_check(&status) {
        return;
    }

    set_update_menu(&update_item, false, false);

    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(error) => {
            eprintln!("update setup failed: {error}");
            reset_update_status(&status);
            set_update_result(
                &app,
                &update_item,
                UpdateResult::Failed("Pulse couldn't start the update check. Try again."),
                reveal_result,
            );
            return;
        }
    };

    let update = match updater.check().await {
        Ok(update) => update,
        Err(error) => {
            eprintln!("update check failed: {error}");
            reset_update_status(&status);
            set_update_result(
                &app,
                &update_item,
                UpdateResult::Failed(
                    "Pulse couldn't check for updates. Check your internet connection and try again.",
                ),
                reveal_result,
            );
            return;
        }
    };

    let Some(update) = update else {
        reset_update_status(&status);
        set_update_result(&app, &update_item, UpdateResult::UpToDate, reveal_result);
        return;
    };

    if let Ok(mut status) = status.lock() {
        *status = UpdateStatus::Downloading;
    } else {
        set_update_result(
            &app,
            &update_item,
            UpdateResult::Failed("Pulse found an update but couldn't prepare it. Try again."),
            reveal_result,
        );
        return;
    }

    set_update_menu(&update_item, false, false);

    match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => {
            let update_ready = if let Ok(mut status) = status.lock() {
                *status = UpdateStatus::Ready(Box::new(DownloadedUpdate { update, bytes }));
                true
            } else {
                false
            };

            if update_ready {
                if let Err(error) = refresh_tray_icon(&app, &tray_icon_mode, &status) {
                    eprintln!("update-ready tray icon failed: {error}");
                }
                set_update_result(&app, &update_item, UpdateResult::Ready, reveal_result);
            } else {
                set_update_result(
                    &app,
                    &update_item,
                    UpdateResult::Failed(
                        "Pulse found an update but couldn't prepare it. Try again.",
                    ),
                    reveal_result,
                );
            }
        }
        Err(error) => {
            eprintln!("update download failed: {error}");
            reset_update_status(&status);
            set_update_result(
                &app,
                &update_item,
                UpdateResult::Failed("Pulse found an update but couldn't download it. Try again."),
                reveal_result,
            );
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn handle_update_menu(
    app: tauri::AppHandle,
    update_item: IconMenuItem<tauri::Wry>,
    status: SharedUpdateStatus,
    tray_icon_mode: Arc<Mutex<TrayIconMode>>,
) {
    if cfg!(debug_assertions) {
        set_update_result(&app, &update_item, UpdateResult::ReleaseBuildRequired, true);
        return;
    }

    let ready_update = {
        let Ok(mut status) = status.lock() else {
            set_update_result(
                &app,
                &update_item,
                UpdateResult::Failed("Pulse couldn't check for updates. Try again."),
                true,
            );
            return;
        };

        match std::mem::replace(&mut *status, UpdateStatus::Installing) {
            UpdateStatus::Ready(update) => Some(update),
            UpdateStatus::Idle => {
                *status = UpdateStatus::Idle;
                None
            }
            current => {
                *status = current;
                return;
            }
        }
    };

    if let Some(downloaded) = ready_update {
        set_update_menu(&update_item, true, false);
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = downloaded.update.install(&downloaded.bytes) {
                eprintln!("update install failed: {error}");
                reset_update_status(&status);
                if let Err(error) = refresh_tray_icon(&app, &tray_icon_mode, &status) {
                    eprintln!("default tray icon restore failed: {error}");
                }
                set_update_result(
                    &app,
                    &update_item,
                    UpdateResult::Failed("Pulse couldn't install the update. Try again."),
                    true,
                );
                return;
            }

            // Do not return after installing: on macOS the updater has already
            // replaced the running app bundle, so the process must stay in the
            // restart path until Tauri exits and launches the updated binary.
            app.restart();
        });
    } else {
        tauri::async_runtime::spawn(check_for_updates(
            app,
            update_item,
            status,
            tray_icon_mode,
            true,
        ));
    }
}

#[cfg(target_os = "macos")]
const TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-iconTemplate@2x.png");

#[cfg(target_os = "macos")]
// Update icons stay non-template to preserve the blue badge, so Pulse selects
// a contrasting waveform for the current macOS appearance.
const UPDATE_DARK_TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-update-macos@2x.png");

#[cfg(target_os = "macos")]
const UPDATE_LIGHT_TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-update-macos-light@2x.png");

#[cfg(target_os = "windows")]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-icon-32.png");

#[cfg(target_os = "windows")]
const UPDATE_TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-update-icon-32.png");

#[cfg(target_os = "windows")]
const WHITE_TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-white-32.png");

#[cfg(target_os = "windows")]
const UPDATE_WHITE_TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-update-white-32.png");

#[cfg(any(target_os = "macos", target_os = "windows"))]
const RED_TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-red-32.png");

#[cfg(any(target_os = "macos", target_os = "windows"))]
const UPDATE_RED_TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-update-red-32.png");

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-icon-32.png");

#[cfg(target_os = "windows")]
use {
    chrono::{Local, Timelike},
    std::{path::PathBuf, thread, time::Duration},
    tauri::Manager,
    windows_sys::Win32::{
        System::Registry::{RegNotifyChangeKeyValue, REG_NOTIFY_CHANGE_LAST_SET},
        UI::WindowsAndMessaging::{
            MessageBoxW, SendMessageTimeoutW, HWND_BROADCAST, MB_ICONERROR, MB_ICONWARNING, MB_OK,
            SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
        },
    },
    winreg::{
        enums::{HKEY_CURRENT_USER, KEY_NOTIFY, KEY_READ},
        RegKey,
    },
};

#[cfg(target_os = "windows")]
const PERSONALIZE_REGISTRY_PATH: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThemeMode {
    Auto,
    Light,
    Dark,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AutoSchedule {
    light_start: u8,
    dark_start: u8,
}

#[cfg(any(target_os = "windows", test))]
impl AutoSchedule {
    const DEFAULT: Self = Self {
        light_start: 7,
        dark_start: 19,
    };

    fn new(light_start: u8, dark_start: u8) -> Option<Self> {
        (light_start < 24 && dark_start < 24 && light_start != dark_start).then_some(Self {
            light_start,
            dark_start,
        })
    }

    fn parse(value: &str) -> Option<Self> {
        let (light_start, dark_start) = value.trim().split_once('-')?;
        Self::new(light_start.parse().ok()?, dark_start.parse().ok()?)
    }

    fn theme_at_hour(self, hour: u32) -> WindowsTheme {
        let hour = hour as u8;
        let uses_light_theme = if self.light_start < self.dark_start {
            (self.light_start..self.dark_start).contains(&hour)
        } else {
            hour >= self.light_start || hour < self.dark_start
        };

        if uses_light_theme {
            WindowsTheme::Light
        } else {
            WindowsTheme::Dark
        }
    }
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowsTheme {
    Light,
    Dark,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AppearanceSnapshot {
    mode: ThemeMode,
    theme: WindowsTheme,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AppearanceTransition {
    id: u64,
    displayed_before: ThemeMode,
    previous: AppearanceSnapshot,
    next: AppearanceSnapshot,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug)]
struct AppearanceState {
    confirmed: AppearanceSnapshot,
    displayed_mode: ThemeMode,
    visual_theme: WindowsTheme,
    generation: u64,
    pending: Option<u64>,
}

#[cfg(any(target_os = "windows", test))]
impl AppearanceState {
    fn new(mode: ThemeMode, theme: WindowsTheme) -> Self {
        Self {
            confirmed: AppearanceSnapshot { mode, theme },
            displayed_mode: mode,
            visual_theme: theme,
            generation: 0,
            pending: None,
        }
    }

    fn begin(
        &mut self,
        next_mode: ThemeMode,
        next_theme: WindowsTheme,
    ) -> Option<AppearanceTransition> {
        self.begin_transition(next_mode, next_theme, false)
    }

    fn begin_forced(
        &mut self,
        next_mode: ThemeMode,
        next_theme: WindowsTheme,
    ) -> Option<AppearanceTransition> {
        self.begin_transition(next_mode, next_theme, true)
    }

    fn begin_transition(
        &mut self,
        next_mode: ThemeMode,
        next_theme: WindowsTheme,
        forced: bool,
    ) -> Option<AppearanceTransition> {
        if !forced && self.displayed_mode == next_mode && self.visual_theme == next_theme {
            return None;
        }

        self.generation = self.generation.wrapping_add(1);
        let transition = AppearanceTransition {
            id: self.generation,
            displayed_before: self.displayed_mode,
            previous: self.confirmed,
            next: AppearanceSnapshot {
                mode: next_mode,
                theme: next_theme,
            },
        };
        self.displayed_mode = next_mode;
        self.visual_theme = next_theme;
        self.pending = Some(transition.id);
        Some(transition)
    }

    fn is_current(&self, transition_id: u64) -> bool {
        self.pending == Some(transition_id)
    }

    fn commit(&mut self, transition: AppearanceTransition) -> bool {
        if !self.is_current(transition.id) {
            return false;
        }

        self.confirmed = transition.next;
        self.displayed_mode = transition.next.mode;
        self.visual_theme = transition.next.theme;
        self.pending = None;
        true
    }

    fn rollback(&mut self, transition: AppearanceTransition, actual_theme: WindowsTheme) -> bool {
        if !self.is_current(transition.id) {
            return false;
        }

        self.confirmed = AppearanceSnapshot {
            mode: transition.previous.mode,
            theme: actual_theme,
        };
        self.displayed_mode = transition.previous.mode;
        self.visual_theme = actual_theme;
        self.pending = None;
        true
    }

    fn observe_external(&mut self, theme: WindowsTheme) -> Option<(ThemeMode, AppearanceSnapshot)> {
        if self.pending.is_some() || self.confirmed.theme == theme {
            return None;
        }

        let previous_mode = self.displayed_mode;
        let mode = if self.confirmed.mode == ThemeMode::Auto {
            ThemeMode::Auto
        } else {
            match theme {
                WindowsTheme::Light => ThemeMode::Light,
                WindowsTheme::Dark => ThemeMode::Dark,
            }
        };
        let next = AppearanceSnapshot { mode, theme };
        self.confirmed = next;
        self.displayed_mode = mode;
        self.visual_theme = theme;
        self.generation = self.generation.wrapping_add(1);
        Some((previous_mode, next))
    }
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DefaultTrayIconVariant {
    Black,
    White,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrayIconAsset {
    Black,
    White,
    Red,
    UpdateBlack,
    UpdateWhite,
    UpdateRed,
}

#[cfg(any(target_os = "windows", test))]
fn default_tray_icon_variant(theme: WindowsTheme) -> DefaultTrayIconVariant {
    match theme {
        WindowsTheme::Light => DefaultTrayIconVariant::Black,
        WindowsTheme::Dark => DefaultTrayIconVariant::White,
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MacosAppearance {
    Light,
    Dark,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MacosTrayIconAsset {
    DefaultTemplate,
    Red,
    UpdateBlack,
    UpdateWhite,
    UpdateRed,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum TrayIconMode {
    Default,
    Red,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct TrayIconSettings {
    mode: Arc<Mutex<TrayIconMode>>,
    config_path: std::path::PathBuf,
    update_status: SharedUpdateStatus,
    #[cfg(target_os = "macos")]
    appearance: Arc<Mutex<MacosAppearance>>,
}

#[cfg(target_os = "windows")]
#[derive(Clone)]
struct AppearanceMenuItems {
    auto: IconMenuItem<tauri::Wry>,
    light: IconMenuItem<tauri::Wry>,
    dark: IconMenuItem<tauri::Wry>,
}

#[cfg(target_os = "windows")]
#[derive(Clone)]
struct WindowsAppearanceController {
    app: tauri::AppHandle,
    state: Arc<Mutex<AppearanceState>>,
    schedule: Arc<Mutex<AutoSchedule>>,
    mode_config_path: PathBuf,
    schedule_config_path: PathBuf,
    menu_items: AppearanceMenuItems,
    tray_icon_mode: Arc<Mutex<TrayIconMode>>,
    update_status: SharedUpdateStatus,
    transition_lock: Arc<Mutex<()>>,
    apply_lock: Arc<Mutex<()>>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[tauri::command]
fn settings_state(app: tauri::AppHandle) -> serde_json::Value {
    let start_at_login = app.autolaunch().is_enabled().unwrap_or(false);

    let tray_icon = app
        .try_state::<TrayIconSettings>()
        .and_then(|settings| settings.mode.lock().ok().map(|mode| mode.as_str()));

    #[cfg(target_os = "windows")]
    let auto_schedule = app
        .try_state::<WindowsAppearanceController>()
        .and_then(|controller| controller.schedule.lock().ok().map(|schedule| *schedule))
        .map(|schedule| {
            serde_json::json!({
                "lightStart": schedule.light_start,
                "darkStart": schedule.dark_start,
            })
        });
    #[cfg(target_os = "macos")]
    let auto_schedule: Option<serde_json::Value> = None;

    let translation = app
        .try_state::<translation::TranslationManager>()
        .map(|manager| manager.state_json())
        .unwrap_or_else(|| serde_json::json!({}));

    serde_json::json!({
        "platform": std::env::consts::OS,
        "startAtLogin": start_at_login,
        "trayIcon": tray_icon,
        "autoSchedule": auto_schedule,
        "translation": translation,
    })
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[tauri::command]
fn set_openai_api_key(app: tauri::AppHandle, api_key: String) -> Result<(), String> {
    translation::save_api_key(&api_key)?;
    app.state::<translation::TranslationManager>()
        .clear_last_error();
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[tauri::command]
fn clear_openai_api_key(app: tauri::AppHandle) -> Result<(), String> {
    translation::clear_api_key()?;
    app.state::<translation::TranslationManager>()
        .clear_last_error();
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[tauri::command]
async fn set_translation_enabled(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<virtual_audio::LifecycleResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let manager = app.state::<translation::TranslationManager>();
        if enabled {
            let result = virtual_audio::enable(&app)?;
            manager.set_enabled(true)?;
            if !result.restart_required {
                if let Err(error) = manager.start_passthrough() {
                    eprintln!("Pulse microphone passthrough could not start: {error}");
                }
            }
            Ok(result)
        } else {
            manager.set_enabled(false)?;
            manager.stop_and_wait()?;
            let result = virtual_audio::disable(&app)?;
            Ok(result)
        }
    })
    .await
    .map_err(|error| format!("Pulse audio lifecycle task failed: {error}"))?
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[tauri::command]
fn set_translation_input_device(app: tauri::AppHandle, name: Option<String>) -> Result<(), String> {
    app.state::<translation::TranslationManager>()
        .set_input_device(name)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[tauri::command]
fn set_start_at_login(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[tauri::command]
fn set_tray_icon_mode(app: tauri::AppHandle, mode: String) -> Result<(), String> {
    let next_mode =
        TrayIconMode::parse(&mode).ok_or_else(|| "invalid tray icon mode".to_string())?;
    let settings = app.state::<TrayIconSettings>();
    select_tray_icon(
        &app,
        next_mode,
        &settings.mode,
        &settings.config_path,
        &settings.update_status,
    )
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn set_auto_schedule(app: tauri::AppHandle, light_start: u8, dark_start: u8) -> Result<(), String> {
    let next_schedule = AutoSchedule::new(light_start, dark_start)
        .ok_or_else(|| "choose two different hours between 00:00 and 23:00".to_string())?;
    let controller = app.state::<WindowsAppearanceController>();

    save_auto_schedule(&controller.schedule_config_path, next_schedule)?;
    *controller
        .schedule
        .lock()
        .map_err(|error| error.to_string())? = next_schedule;

    if controller.displayed_mode()? == ThemeMode::Auto {
        controller.request_mode(ThemeMode::Auto)?;
    }

    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn open_settings(app: &tauri::AppHandle) -> tauri::Result<()> {
    let window = if let Some(window) = app.get_webview_window("settings") {
        window.show()?;
        window
    } else {
        #[cfg(target_os = "windows")]
        let window_height = 650.0;
        #[cfg(target_os = "macos")]
        let window_height = 590.0;

        WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
            .title("Pulse Settings")
            .inner_size(500.0, window_height)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .build()?
    };

    window.set_focus()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl TrayIconMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "default" | "black" => Some(Self::Default),
            "red" => Some(Self::Red),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Red => "red",
        }
    }

    #[cfg(any(target_os = "macos", test))]
    fn macos_asset(self, appearance: MacosAppearance, update_ready: bool) -> MacosTrayIconAsset {
        match (self, appearance, update_ready) {
            (Self::Default, _, false) => MacosTrayIconAsset::DefaultTemplate,
            (Self::Red, _, false) => MacosTrayIconAsset::Red,
            (Self::Default, MacosAppearance::Light, true) => MacosTrayIconAsset::UpdateBlack,
            (Self::Default, MacosAppearance::Dark, true) => MacosTrayIconAsset::UpdateWhite,
            (Self::Red, _, true) => MacosTrayIconAsset::UpdateRed,
        }
    }

    #[cfg(any(target_os = "windows", test))]
    fn asset(self, theme: WindowsTheme, update_ready: bool) -> TrayIconAsset {
        match (self, default_tray_icon_variant(theme), update_ready) {
            (Self::Default, DefaultTrayIconVariant::Black, false) => TrayIconAsset::Black,
            (Self::Default, DefaultTrayIconVariant::White, false) => TrayIconAsset::White,
            (Self::Red, _, false) => TrayIconAsset::Red,
            (Self::Default, DefaultTrayIconVariant::Black, true) => TrayIconAsset::UpdateBlack,
            (Self::Default, DefaultTrayIconVariant::White, true) => TrayIconAsset::UpdateWhite,
            (Self::Red, _, true) => TrayIconAsset::UpdateRed,
        }
    }

    #[cfg(target_os = "windows")]
    fn bytes(self, theme: WindowsTheme, update_ready: bool) -> &'static [u8] {
        match self.asset(theme, update_ready) {
            TrayIconAsset::Black => TRAY_ICON_BYTES,
            TrayIconAsset::White => WHITE_TRAY_ICON_BYTES,
            TrayIconAsset::Red => RED_TRAY_ICON_BYTES,
            TrayIconAsset::UpdateBlack => UPDATE_TRAY_ICON_BYTES,
            TrayIconAsset::UpdateWhite => UPDATE_WHITE_TRAY_ICON_BYTES,
            TrayIconAsset::UpdateRed => UPDATE_RED_TRAY_ICON_BYTES,
        }
    }

    #[cfg(target_os = "macos")]
    fn bytes(self, appearance: MacosAppearance, update_ready: bool) -> &'static [u8] {
        match self.macos_asset(appearance, update_ready) {
            MacosTrayIconAsset::DefaultTemplate => TRAY_ICON_BYTES,
            MacosTrayIconAsset::Red => RED_TRAY_ICON_BYTES,
            MacosTrayIconAsset::UpdateBlack => UPDATE_LIGHT_TRAY_ICON_BYTES,
            MacosTrayIconAsset::UpdateWhite => UPDATE_DARK_TRAY_ICON_BYTES,
            MacosTrayIconAsset::UpdateRed => UPDATE_RED_TRAY_ICON_BYTES,
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn update_icon_is_active(status: &SharedUpdateStatus) -> Result<bool, String> {
    let status = status.lock().map_err(|error| error.to_string())?;
    Ok(matches!(
        &*status,
        UpdateStatus::Ready(_) | UpdateStatus::Installing
    ))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn load_tray_icon_mode(path: &Path) -> TrayIconMode {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| TrayIconMode::parse(value.trim()))
        .unwrap_or(TrayIconMode::Default)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn save_tray_icon_mode(path: &Path, mode: TrayIconMode) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    fs::write(path, mode.as_str()).map_err(|error| error.to_string())
}

#[cfg(any(target_os = "windows", test))]
impl ThemeMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[cfg(target_os = "windows")]
fn load_theme_mode(path: &Path) -> ThemeMode {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| ThemeMode::parse(value.trim()))
        .unwrap_or(ThemeMode::Auto)
}

#[cfg(target_os = "windows")]
fn save_theme_mode(path: &Path, mode: ThemeMode) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    fs::write(path, mode.as_str()).map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn load_auto_schedule(path: &Path) -> AutoSchedule {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| AutoSchedule::parse(&value))
        .unwrap_or(AutoSchedule::DEFAULT)
}

#[cfg(target_os = "windows")]
fn save_auto_schedule(path: &Path, schedule: AutoSchedule) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    fs::write(
        path,
        format!("{}-{}", schedule.light_start, schedule.dark_start),
    )
    .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn scheduled_theme(schedule: AutoSchedule) -> WindowsTheme {
    schedule.theme_at_hour(Local::now().hour())
}

#[cfg(target_os = "windows")]
fn resolve_theme(mode: ThemeMode, schedule: AutoSchedule) -> WindowsTheme {
    match mode {
        ThemeMode::Auto => scheduled_theme(schedule),
        ThemeMode::Light => WindowsTheme::Light,
        ThemeMode::Dark => WindowsTheme::Dark,
    }
}

#[cfg(target_os = "windows")]
fn apply_windows_theme_value(theme: WindowsTheme) -> Result<(), String> {
    let use_light_theme = u32::from(theme == WindowsTheme::Light);
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (personalize, _) = current_user
        .create_subkey(PERSONALIZE_REGISTRY_PATH)
        .map_err(|error| error.to_string())?;

    personalize
        .set_value("AppsUseLightTheme", &use_light_theme)
        .map_err(|error| error.to_string())?;
    personalize
        .set_value("SystemUsesLightTheme", &use_light_theme)
        .map_err(|error| error.to_string())?;

    for value_name in ["AppsUseLightTheme", "SystemUsesLightTheme"] {
        let saved_value: u32 = personalize
            .get_value(value_name)
            .map_err(|error| error.to_string())?;
        if saved_value != use_light_theme {
            return Err(format!("Windows did not save {value_name}"));
        }
    }

    let setting_name: Vec<u16> = "ImmersiveColorSet\0".encode_utf16().collect();
    let notified = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            setting_name.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5_000,
            std::ptr::null_mut(),
        )
    };
    if notified == 0 {
        eprintln!(
            "Windows appearance notification failed or timed out after registry values were saved"
        );
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_theme_from_registry(personalize: &RegKey) -> Result<WindowsTheme, String> {
    let uses_light_theme: u32 = personalize
        .get_value("SystemUsesLightTheme")
        .map_err(|error| error.to_string())?;
    if uses_light_theme == 0 {
        Ok(WindowsTheme::Dark)
    } else {
        Ok(WindowsTheme::Light)
    }
}

#[cfg(target_os = "windows")]
fn current_windows_theme() -> Result<WindowsTheme, String> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let personalize = current_user
        .open_subkey(PERSONALIZE_REGISTRY_PATH)
        .map_err(|error| error.to_string())?;
    windows_theme_from_registry(&personalize)
}

#[cfg(target_os = "windows")]
fn appearance_menu_icon_bytes(mode: ThemeMode, selected: bool) -> &'static [u8] {
    match (mode, selected) {
        (ThemeMode::Auto, false) => AUTO_MENU_ICON_BYTES,
        (ThemeMode::Auto, true) => AUTO_SELECTED_MENU_ICON_BYTES,
        (ThemeMode::Light, false) => LIGHT_MENU_ICON_BYTES,
        (ThemeMode::Light, true) => LIGHT_SELECTED_MENU_ICON_BYTES,
        (ThemeMode::Dark, false) => DARK_MENU_ICON_BYTES,
        (ThemeMode::Dark, true) => DARK_SELECTED_MENU_ICON_BYTES,
    }
}

#[cfg(target_os = "windows")]
fn appearance_menu_item(items: &AppearanceMenuItems, mode: ThemeMode) -> &IconMenuItem<tauri::Wry> {
    match mode {
        ThemeMode::Auto => &items.auto,
        ThemeMode::Light => &items.light,
        ThemeMode::Dark => &items.dark,
    }
}

#[cfg(target_os = "windows")]
fn set_appearance_menu_item_icon(
    item: &IconMenuItem<tauri::Wry>,
    mode: ThemeMode,
    selected: bool,
) -> Result<(), String> {
    let icon = tauri::image::Image::from_bytes(appearance_menu_icon_bytes(mode, selected))
        .map_err(|error| error.to_string())?;
    item.set_icon(Some(icon)).map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn set_appearance_selection(
    items: &AppearanceMenuItems,
    previous_mode: ThemeMode,
    next_mode: ThemeMode,
) -> Result<(), String> {
    if previous_mode == next_mode {
        return Ok(());
    }

    set_appearance_menu_item_icon(
        appearance_menu_item(items, previous_mode),
        previous_mode,
        false,
    )?;
    set_appearance_menu_item_icon(appearance_menu_item(items, next_mode), next_mode, true)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn show_windows_message(title: &str, message: &str, style: u32) {
    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let message: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | style,
        );
    }
}

#[cfg(target_os = "windows")]
impl WindowsAppearanceController {
    fn displayed_mode(&self) -> Result<ThemeMode, String> {
        self.state
            .lock()
            .map(|state| state.displayed_mode)
            .map_err(|error| error.to_string())
    }

    fn status(&self) -> Result<(ThemeMode, WindowsTheme, bool), String> {
        self.state
            .lock()
            .map(|state| {
                (
                    state.displayed_mode,
                    state.confirmed.theme,
                    state.pending.is_some(),
                )
            })
            .map_err(|error| error.to_string())
    }

    fn visual_theme(&self) -> Result<WindowsTheme, String> {
        self.state
            .lock()
            .map(|state| state.visual_theme)
            .map_err(|error| error.to_string())
    }

    fn request_mode(&self, next_mode: ThemeMode) -> Result<bool, String> {
        let schedule = *self.schedule.lock().map_err(|error| error.to_string())?;
        self.request_mode_with_theme(next_mode, resolve_theme(next_mode, schedule))
    }

    fn force_startup_mode(&self, next_mode: ThemeMode) -> Result<bool, String> {
        let schedule = *self.schedule.lock().map_err(|error| error.to_string())?;
        self.start_transition(next_mode, resolve_theme(next_mode, schedule), true)
    }

    fn request_mode_with_theme(
        &self,
        next_mode: ThemeMode,
        next_theme: WindowsTheme,
    ) -> Result<bool, String> {
        self.start_transition(next_mode, next_theme, false)
    }

    fn start_transition(
        &self,
        next_mode: ThemeMode,
        next_theme: WindowsTheme,
        forced: bool,
    ) -> Result<bool, String> {
        let _transition_guard = self
            .transition_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let transition = {
            let mut state = self.state.lock().map_err(|error| error.to_string())?;
            if forced {
                state.begin_forced(next_mode, next_theme)
            } else {
                state.begin(next_mode, next_theme)
            }
        };
        let Some(transition) = transition else {
            return Ok(false);
        };

        self.update_visuals(
            transition.displayed_before,
            transition.next.mode,
            transition.next.theme,
        );

        let controller = self.clone();
        thread::spawn(move || controller.finish_transition(transition));
        Ok(true)
    }

    fn update_visuals(&self, previous_mode: ThemeMode, next_mode: ThemeMode, theme: WindowsTheme) {
        if let Err(error) = set_appearance_selection(&self.menu_items, previous_mode, next_mode) {
            eprintln!("appearance menu icon update failed: {error}");
        }

        let tray_result = (|| {
            let tray_mode = *self
                .tray_icon_mode
                .lock()
                .map_err(|error| error.to_string())?;
            set_tray_icon(
                &self.app,
                tray_mode,
                theme,
                update_icon_is_active(&self.update_status)?,
            )
        })();
        if let Err(error) = tray_result {
            eprintln!("appearance tray icon update failed: {error}");
        }
    }

    fn is_current(&self, transition_id: u64) -> bool {
        self.state
            .lock()
            .map(|state| state.is_current(transition_id))
            .unwrap_or(false)
    }

    fn finish_transition(&self, transition: AppearanceTransition) {
        let _apply_guard = match self.apply_lock.lock() {
            Ok(guard) => guard,
            Err(error) => {
                eprintln!("Windows appearance transition lock failed: {error}");
                return;
            }
        };
        if !self.is_current(transition.id) {
            return;
        }

        if let Err(error) = apply_windows_theme_value(transition.next.theme) {
            if self.is_current(transition.id) {
                self.rollback_transition(transition, error);
            }
            return;
        }
        if !self.is_current(transition.id) {
            return;
        }

        let committed = self
            .state
            .lock()
            .map(|mut state| state.commit(transition))
            .unwrap_or(false);
        if !committed {
            return;
        }

        let save_result = save_theme_mode(&self.mode_config_path, transition.next.mode);
        drop(_apply_guard);
        if let Err(error) = save_result {
            eprintln!("Windows appearance preference save failed: {error}");
            show_windows_message(
                "Pulse",
                "Appearance changed, but Pulse couldn't save it for the next restart.",
                MB_ICONWARNING,
            );
        }
    }

    fn rollback_transition(&self, transition: AppearanceTransition, apply_error: String) {
        let rollback_error = apply_windows_theme_value(transition.previous.theme).err();
        let actual_theme = current_windows_theme().unwrap_or(transition.previous.theme);
        let _transition_guard = match self.transition_lock.lock() {
            Ok(guard) => guard,
            Err(error) => {
                eprintln!("Windows appearance rollback lock failed: {error}");
                return;
            }
        };
        let rolled_back = self
            .state
            .lock()
            .map(|mut state| state.rollback(transition, actual_theme))
            .unwrap_or(false);
        if !rolled_back {
            return;
        }

        self.update_visuals(transition.next.mode, transition.previous.mode, actual_theme);
        drop(_transition_guard);
        eprintln!("Windows appearance change failed: {apply_error}");

        let message = if let Some(rollback_error) = rollback_error {
            eprintln!("Windows appearance rollback failed: {rollback_error}");
            "Pulse couldn't change Windows appearance or fully restore the previous appearance."
        } else {
            "Pulse couldn't change Windows appearance. The previous appearance was restored."
        };
        show_windows_message("Pulse", message, MB_ICONERROR);
    }

    fn observe_windows_theme(&self, theme: WindowsTheme) {
        let change = {
            let _transition_guard = match self.transition_lock.lock() {
                Ok(guard) => guard,
                Err(error) => {
                    eprintln!("Windows appearance watcher lock failed: {error}");
                    return;
                }
            };
            let change = self
                .state
                .lock()
                .ok()
                .and_then(|mut state| state.observe_external(theme));
            if let Some((previous_mode, next)) = change {
                self.update_visuals(previous_mode, next.mode, next.theme);
            }
            change
        };
        let Some((_, next)) = change else {
            return;
        };

        if next.mode != ThemeMode::Auto {
            let _apply_guard = match self.apply_lock.lock() {
                Ok(guard) => guard,
                Err(error) => {
                    eprintln!("external Windows appearance save lock failed: {error}");
                    return;
                }
            };
            let still_current = self
                .state
                .lock()
                .map(|state| state.confirmed == next && state.pending.is_none())
                .unwrap_or(false);
            if !still_current {
                return;
            }
            let save_result = save_theme_mode(&self.mode_config_path, next.mode);
            drop(_apply_guard);
            if let Err(error) = save_result {
                eprintln!("external Windows appearance preference save failed: {error}");
                show_windows_message(
                    "Pulse",
                    "Appearance changed, but Pulse couldn't save it for the next restart.",
                    MB_ICONWARNING,
                );
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn set_tray_icon(
    app: &tauri::AppHandle,
    mode: TrayIconMode,
    theme: WindowsTheme,
    update_ready: bool,
) -> Result<(), String> {
    let tray = app
        .tray_by_id("pulse-tray")
        .ok_or_else(|| "pulse tray not found".to_string())?;
    let icon = tauri::image::Image::from_bytes(mode.bytes(theme, update_ready))
        .map_err(|error| error.to_string())?;
    tray.set_icon(Some(icon)).map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn visual_windows_theme(app: &tauri::AppHandle) -> Result<WindowsTheme, String> {
    if let Some(controller) = app.try_state::<WindowsAppearanceController>() {
        controller.visual_theme()
    } else {
        current_windows_theme()
    }
}

#[cfg(target_os = "macos")]
fn current_macos_appearance() -> MacosAppearance {
    let main_thread =
        MainThreadMarker::new().expect("macOS appearance must be read on the main thread");
    let appearance = NSApp(main_thread).effectiveAppearance();
    let names = NSArray::from_slice(&[unsafe { NSAppearanceNameAqua }, unsafe {
        NSAppearanceNameDarkAqua
    }]);
    let dark = appearance
        .bestMatchFromAppearancesWithNames(&names)
        .is_some_and(|name| &*name == unsafe { NSAppearanceNameDarkAqua });

    if dark {
        MacosAppearance::Dark
    } else {
        MacosAppearance::Light
    }
}

#[cfg(target_os = "macos")]
fn visual_macos_appearance(app: &tauri::AppHandle) -> Result<MacosAppearance, String> {
    let settings = app.state::<TrayIconSettings>();
    settings
        .appearance
        .lock()
        .map(|appearance| *appearance)
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn set_tray_icon(
    app: &tauri::AppHandle,
    mode: TrayIconMode,
    update_ready: bool,
) -> Result<(), String> {
    let tray = app
        .tray_by_id("pulse-tray")
        .ok_or_else(|| "pulse tray not found".to_string())?;
    let icon =
        tauri::image::Image::from_bytes(mode.bytes(visual_macos_appearance(app)?, update_ready))
            .map_err(|error| error.to_string())?;
    tray.set_icon_with_as_template(Some(icon), mode == TrayIconMode::Default && !update_ready)
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn start_macos_appearance_watcher(app: tauri::AppHandle) {
    let center = NSDistributedNotificationCenter::defaultCenter();
    let name = NSString::from_str("AppleInterfaceThemeChangedNotification");
    let queue = NSOperationQueue::mainQueue();
    let block = RcBlock::new(move |_notification: NonNull<NSNotification>| {
        let next = current_macos_appearance();
        let settings = app.state::<TrayIconSettings>();
        let changed = match settings.appearance.lock() {
            Ok(mut appearance) if *appearance != next => {
                *appearance = next;
                true
            }
            Ok(_) => false,
            Err(error) => {
                eprintln!("macOS appearance state update failed: {error}");
                false
            }
        };

        if changed {
            if let Err(error) = refresh_tray_icon(&app, &settings.mode, &settings.update_status) {
                eprintln!("macOS appearance tray icon update failed: {error}");
            }
        }
    });

    unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(&name), None, Some(&queue), &block);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn refresh_tray_icon(
    app: &tauri::AppHandle,
    mode: &Arc<Mutex<TrayIconMode>>,
    update_status: &SharedUpdateStatus,
) -> Result<(), String> {
    let selected_mode = *mode.lock().map_err(|error| error.to_string())?;
    let update_ready = update_icon_is_active(update_status)?;

    #[cfg(target_os = "windows")]
    set_tray_icon(app, selected_mode, visual_windows_theme(app)?, update_ready)?;
    #[cfg(target_os = "macos")]
    set_tray_icon(app, selected_mode, update_ready)?;

    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn select_tray_icon(
    app: &tauri::AppHandle,
    next_mode: TrayIconMode,
    mode: &Arc<Mutex<TrayIconMode>>,
    config_path: &Path,
    update_status: &SharedUpdateStatus,
) -> Result<(), String> {
    let mut selected_mode = mode.lock().map_err(|error| error.to_string())?;

    #[cfg(target_os = "windows")]
    set_tray_icon(
        app,
        next_mode,
        visual_windows_theme(app)?,
        update_icon_is_active(update_status)?,
    )?;
    #[cfg(target_os = "macos")]
    set_tray_icon(app, next_mode, update_icon_is_active(update_status)?)?;

    save_tray_icon_mode(config_path, next_mode)?;
    *selected_mode = next_mode;
    Ok(())
}

#[cfg(target_os = "windows")]
fn start_windows_theme_watcher(controller: WindowsAppearanceController) {
    thread::spawn(move || loop {
        let current_user = RegKey::predef(HKEY_CURRENT_USER);
        let personalize = match current_user
            .open_subkey_with_flags(PERSONALIZE_REGISTRY_PATH, KEY_READ | KEY_NOTIFY)
        {
            Ok(personalize) => personalize,
            Err(error) => {
                eprintln!("Windows theme watcher setup failed: {error}");
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        loop {
            match windows_theme_from_registry(&personalize) {
                Ok(theme) => controller.observe_windows_theme(theme),
                Err(error) => eprintln!("Windows theme watcher read failed: {error}"),
            }

            let status = unsafe {
                RegNotifyChangeKeyValue(
                    personalize.raw_handle(),
                    0,
                    REG_NOTIFY_CHANGE_LAST_SET,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if status != 0 {
                eprintln!("Windows theme watcher failed with status {status}");
                break;
            }
        }

        thread::sleep(Duration::from_secs(1));
    });
}

#[cfg(target_os = "windows")]
fn start_auto_scheduler(controller: WindowsAppearanceController) {
    thread::spawn(move || {
        let mut active_schedule_target = None;

        loop {
            let status = controller.status();
            let schedule = controller
                .schedule
                .lock()
                .map(|schedule| *schedule)
                .map_err(|error| error.to_string());
            match (status, schedule) {
                (Ok((ThemeMode::Auto, confirmed_theme, pending)), Ok(schedule)) => {
                    let target = scheduled_theme(schedule);
                    match active_schedule_target {
                        None if !pending && confirmed_theme != target => {
                            // Remember automated attempts before they finish so a rollback does
                            // not trigger another modal failure on every scheduler iteration.
                            active_schedule_target = Some(target);
                            let _ = controller.request_mode_with_theme(ThemeMode::Auto, target);
                        }
                        None if !pending => active_schedule_target = Some(target),
                        None => {}
                        Some(previous_target) if previous_target != target => {
                            if confirmed_theme == target && !pending {
                                active_schedule_target = Some(target);
                            } else if !pending {
                                // A new schedule period gets one automatic attempt. Manual mode
                                // changes reset this marker and allow a later Auto selection to try.
                                active_schedule_target = Some(target);
                                let _ = controller.request_mode_with_theme(ThemeMode::Auto, target);
                            }
                        }
                        Some(_) => {}
                    }
                }
                (Ok(_), Ok(_)) => active_schedule_target = None,
                (Err(error), _) | (_, Err(error)) => {
                    eprintln!("Windows appearance scheduler read failed: {error}");
                }
            }

            thread::sleep(Duration::from_secs(30));
        }
    });
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
type TranslationControls = (
    Submenu<tauri::Wry>,
    IconMenuItem<tauri::Wry>,
    PredefinedMenuItem<tauri::Wry>,
    Vec<(String, CheckMenuItem<tauri::Wry>)>,
);

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn build_translation_controls(app: &tauri::App<tauri::Wry>) -> tauri::Result<TranslationControls> {
    let language_items = translation::LANGUAGES
        .iter()
        .map(|(code, label)| {
            CheckMenuItem::with_id(
                app,
                format!("translation-language-{code}"),
                *label,
                true,
                *code == "en",
                None::<&str>,
            )
            .map(|item| ((*code).to_string(), item))
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let language_item_refs = language_items
        .iter()
        .map(|(_, item)| item as &dyn IsMenuItem<tauri::Wry>)
        .collect::<Vec<_>>();
    let language_menu = Submenu::with_items(app, "Translate to", true, &language_item_refs)?;
    language_menu.set_icon(Some(tauri::image::Image::from_bytes(
        TRANSLATION_MENU_ICON_BYTES,
    )?))?;
    let start_item = IconMenuItem::with_id(
        app,
        "translation-start",
        "Start Translation",
        true,
        Some(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/menu/translation-off.png"
        ))?),
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;

    Ok((language_menu, start_item, separator, language_items))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    #[cfg(target_os = "windows")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        settings_state,
        set_start_at_login,
        set_tray_icon_mode,
        set_auto_schedule,
        set_openai_api_key,
        clear_openai_api_key,
        set_translation_enabled,
        set_translation_input_device
    ]);

    #[cfg(target_os = "macos")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        settings_state,
        set_start_at_login,
        set_tray_icon_mode,
        set_openai_api_key,
        clear_openai_api_key,
        set_translation_enabled,
        set_translation_input_device
    ]);

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let builder = builder.plugin(tauri_plugin_autostart::init(
        MacosLauncher::LaunchAgent,
        None,
    ));

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let builder = builder.plugin(tauri_plugin_dialog::init());

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let builder = builder.on_window_event(|window, event| {
        if window.label() == "settings" {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(error) = window.hide() {
                    eprintln!("failed to hide settings: {error}");
                }
            }
        }
    });

    builder
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let (initial_tray_icon_mode, tray_icon_config_path) = {
                let config_path = app.path().app_config_dir()?.join("tray-icon");
                (load_tray_icon_mode(&config_path), config_path)
            };

            #[cfg(target_os = "macos")]
            let initial_macos_appearance = current_macos_appearance();

            #[cfg(target_os = "windows")]
            let (
                initial_theme_mode,
                theme_config_path,
                initial_auto_schedule,
                auto_schedule_config_path,
            ) = {
                let app_config_dir = app.path().app_config_dir()?;
                let theme_config_path = app_config_dir.join("theme-mode");
                let auto_schedule_config_path = app_config_dir.join("auto-schedule");
                (
                    load_theme_mode(&theme_config_path),
                    theme_config_path,
                    load_auto_schedule(&auto_schedule_config_path),
                    auto_schedule_config_path,
                )
            };
            #[cfg(target_os = "windows")]
            let initial_resolved_theme = resolve_theme(initial_theme_mode, initial_auto_schedule);
            #[cfg(target_os = "windows")]
            let initial_windows_theme = current_windows_theme().unwrap_or(initial_resolved_theme);

            #[cfg(target_os = "windows")]
            let tray_icon = tauri::image::Image::from_bytes(
                initial_tray_icon_mode.bytes(initial_windows_theme, false),
            )?;
            #[cfg(target_os = "macos")]
            let tray_icon = tauri::image::Image::from_bytes(
                initial_tray_icon_mode.bytes(initial_macos_appearance, false),
            )?;
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let tray_icon = tauri::image::Image::from_bytes(TRAY_ICON_BYTES)?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let tray_icon_is_template = initial_tray_icon_mode == TrayIconMode::Default;
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let tray_icon_is_template = true;
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let separator = PredefinedMenuItem::separator(app)?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let quit_separator = PredefinedMenuItem::separator(app)?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let quit = IconMenuItem::with_id(
                app,
                "quit",
                "Quit Pulse",
                true,
                Some(tauri::image::Image::from_bytes(QUIT_MENU_ICON_BYTES)?),
                None::<&str>,
            )?;
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let quit =
                tauri::menu::MenuItem::with_id(app, "quit", "Quit Pulse", true, None::<&str>)?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let settings = IconMenuItem::with_id(
                app,
                "settings",
                "Settings",
                true,
                Some(tauri::image::Image::from_bytes(SETTINGS_MENU_ICON_BYTES)?),
                None::<&str>,
            )?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let update_item = IconMenuItem::with_id(
                app,
                "check-for-updates",
                "Check for Updates",
                true,
                Some(tauri::image::Image::from_bytes(
                    CHECK_FOR_UPDATES_MENU_ICON_BYTES,
                )?),
                None::<&str>,
            )?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let update_status = Arc::new(Mutex::new(UpdateStatus::Idle));
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let (
                translation_menu,
                translation_start,
                translation_separator,
                translation_language_items,
            ) = build_translation_controls(app)?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let translation_config_path = app.path().app_config_dir()?.join("live-translate.json");

            #[cfg(target_os = "windows")]
            let (menu, appearance_menu_items, auto_schedule) = {
                let selected_mode = initial_theme_mode;
                let auto_schedule = Arc::new(Mutex::new(initial_auto_schedule));
                let auto = IconMenuItem::with_id(
                    app,
                    "theme-auto",
                    "Auto",
                    true,
                    Some(tauri::image::Image::from_bytes(
                        appearance_menu_icon_bytes(
                            ThemeMode::Auto,
                            selected_mode == ThemeMode::Auto,
                        ),
                    )?),
                    None::<&str>,
                )?;
                let light = IconMenuItem::with_id(
                    app,
                    "theme-light",
                    "Light",
                    true,
                    Some(tauri::image::Image::from_bytes(
                        appearance_menu_icon_bytes(
                            ThemeMode::Light,
                            selected_mode == ThemeMode::Light,
                        ),
                    )?),
                    None::<&str>,
                )?;
                let dark = IconMenuItem::with_id(
                    app,
                    "theme-dark",
                    "Dark",
                    true,
                    Some(tauri::image::Image::from_bytes(
                        appearance_menu_icon_bytes(
                            ThemeMode::Dark,
                            selected_mode == ThemeMode::Dark,
                        ),
                    )?),
                    None::<&str>,
                )?;
                let appearance =
                    Submenu::with_items(app, "Appearance", true, &[&auto, &light, &dark])?;
                appearance.set_icon(Some(tauri::image::Image::from_bytes(
                    APPEARANCE_MENU_ICON_BYTES,
                )?))?;
                let menu = Menu::with_items(
                    app,
                    &[
                        &appearance,
                        &translation_menu,
                        &translation_start,
                        &translation_separator,
                        &settings,
                        &update_item,
                        &quit_separator,
                        &quit,
                    ],
                )?;
                (
                    menu,
                    AppearanceMenuItems { auto, light, dark },
                    auto_schedule,
                )
            };

            #[cfg(target_os = "macos")]
            let menu = Menu::with_items(
                app,
                &[
                    &translation_menu,
                    &translation_start,
                    &translation_separator,
                    &settings,
                    &update_item,
                    &quit_separator,
                    &quit,
                ],
            )?;

            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let menu = {
                let status = tauri::menu::MenuItem::with_id(
                    app,
                    "status",
                    "Pulse is running",
                    false,
                    None::<&str>,
                )?;
                Menu::with_items(app, &[&status, &separator, &quit])?
            };

            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let tray_icon_mode = Arc::new(Mutex::new(initial_tray_icon_mode));
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let startup_update_item = update_item.clone();
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let startup_update_status = update_status.clone();
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let startup_tray_icon_mode = tray_icon_mode.clone();
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            app.manage(TrayIconSettings {
                mode: tray_icon_mode.clone(),
                config_path: tray_icon_config_path,
                update_status: update_status.clone(),
                #[cfg(target_os = "macos")]
                appearance: Arc::new(Mutex::new(initial_macos_appearance)),
            });
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            app.manage(translation::TranslationManager::new(
                translation_config_path,
                menu.clone(),
                translation_menu.clone(),
                translation_start.clone(),
                translation_separator.clone(),
                translation_language_items,
            )?);
            #[cfg(target_os = "windows")]
            let appearance_controller = WindowsAppearanceController {
                app: app.handle().clone(),
                state: Arc::new(Mutex::new(AppearanceState::new(
                    initial_theme_mode,
                    initial_windows_theme,
                ))),
                schedule: auto_schedule,
                mode_config_path: theme_config_path,
                schedule_config_path: auto_schedule_config_path,
                menu_items: appearance_menu_items,
                tray_icon_mode: tray_icon_mode.clone(),
                update_status: update_status.clone(),
                transition_lock: Arc::new(Mutex::new(())),
                apply_lock: Arc::new(Mutex::new(())),
            };
            #[cfg(target_os = "windows")]
            app.manage(appearance_controller.clone());
            #[cfg(target_os = "windows")]
            let menu_appearance_controller = appearance_controller.clone();

            TrayIconBuilder::with_id("pulse-tray")
                .icon(tray_icon)
                .icon_as_template(tray_icon_is_template)
                .tooltip("Pulse")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app, event| {
                    #[cfg(any(target_os = "macos", target_os = "windows"))]
                    if let Some(code) = event.id().as_ref().strip_prefix("translation-language-") {
                        if let Err(error) = app
                            .state::<translation::TranslationManager>()
                            .select_language(code)
                        {
                            eprintln!("translation language selection failed: {error}");
                        }
                    }

                    #[cfg(any(target_os = "macos", target_os = "windows"))]
                    if event.id().as_ref() == "translation-start" {
                        if let Err(error) = app.state::<translation::TranslationManager>().toggle()
                        {
                            eprintln!("translation start failed: {error}");
                            if let Err(window_error) = open_settings(app) {
                                eprintln!("failed to open translation settings: {window_error}");
                            }
                        }
                    }

                    #[cfg(target_os = "windows")]
                    {
                        let next_mode = match event.id().as_ref() {
                            "theme-auto" => Some(ThemeMode::Auto),
                            "theme-light" => Some(ThemeMode::Light),
                            "theme-dark" => Some(ThemeMode::Dark),
                            _ => None,
                        };

                        if let Some(next_mode) = next_mode {
                            if let Err(error) = menu_appearance_controller.request_mode(next_mode) {
                                eprintln!("Windows appearance request failed: {error}");
                            }
                        }
                    }

                    #[cfg(any(target_os = "macos", target_os = "windows"))]
                    if event.id().as_ref() == "settings" {
                        if let Err(error) = open_settings(app) {
                            eprintln!("failed to open settings: {error}");
                        }
                    }

                    #[cfg(any(target_os = "macos", target_os = "windows"))]
                    if event.id().as_ref() == "check-for-updates" {
                        handle_update_menu(
                            app.clone(),
                            update_item.clone(),
                            update_status.clone(),
                            tray_icon_mode.clone(),
                        );
                    }

                    if event.id().as_ref() == "quit" {
                        #[cfg(any(target_os = "macos", target_os = "windows"))]
                        if let Some(manager) = app.try_state::<translation::TranslationManager>() {
                            if let Err(error) = manager.stop() {
                                eprintln!("translation shutdown failed: {error}");
                            }
                        }
                        app.exit(0);
                    }
                })
                .build(app)?;

            #[cfg(target_os = "macos")]
            start_macos_appearance_watcher(app.handle().clone());

            #[cfg(target_os = "windows")]
            if let Err(error) = appearance_controller.force_startup_mode(initial_theme_mode) {
                eprintln!("initial Windows appearance request failed: {error}");
            }
            #[cfg(target_os = "windows")]
            start_auto_scheduler(appearance_controller.clone());
            #[cfg(target_os = "windows")]
            start_windows_theme_watcher(appearance_controller);

            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if !cfg!(debug_assertions) {
                tauri::async_runtime::spawn(check_for_updates(
                    app.handle().clone(),
                    startup_update_item,
                    startup_update_status,
                    startup_tray_icon_mode,
                    false,
                ));
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Pulse");
}

#[cfg(test)]
mod tests {
    use super::{
        default_tray_icon_variant, AppearanceSnapshot, AppearanceState, AutoSchedule,
        DefaultTrayIconVariant, MacosAppearance, MacosTrayIconAsset, ThemeMode, TrayIconAsset,
        TrayIconMode, UpdateResult, WindowsTheme,
    };

    #[test]
    fn manual_update_results_have_distinct_feedback() {
        assert_eq!(
            UpdateResult::ReleaseBuildRequired.feedback(),
            Some("Update checks require a release build.")
        );
        assert_eq!(
            UpdateResult::UpToDate.feedback(),
            Some("Pulse is up to date.")
        );
        assert_eq!(
            UpdateResult::Failed("Pulse couldn't check for updates.").feedback(),
            Some("Pulse couldn't check for updates.")
        );
        assert_eq!(UpdateResult::Ready.feedback(), None);
    }

    #[test]
    fn default_auto_schedule_uses_light_between_seven_and_nineteen() {
        let schedule = AutoSchedule::DEFAULT;

        assert_eq!(schedule.theme_at_hour(7), WindowsTheme::Light);
        assert_eq!(schedule.theme_at_hour(18), WindowsTheme::Light);
        assert_eq!(schedule.theme_at_hour(19), WindowsTheme::Dark);
        assert_eq!(schedule.theme_at_hour(6), WindowsTheme::Dark);
    }

    #[test]
    fn auto_schedule_supports_a_light_period_across_midnight() {
        let schedule = AutoSchedule::new(20, 6).expect("valid schedule");

        assert_eq!(schedule.theme_at_hour(23), WindowsTheme::Light);
        assert_eq!(schedule.theme_at_hour(2), WindowsTheme::Light);
        assert_eq!(schedule.theme_at_hour(12), WindowsTheme::Dark);
    }

    #[test]
    fn auto_schedule_rejects_equal_or_out_of_range_hours() {
        assert_eq!(AutoSchedule::new(7, 7), None);
        assert_eq!(AutoSchedule::new(24, 19), None);
        assert_eq!(AutoSchedule::parse("7-19"), Some(AutoSchedule::DEFAULT));
    }

    #[test]
    fn default_tray_icon_is_white_for_dark_windows_theme() {
        assert_eq!(
            default_tray_icon_variant(WindowsTheme::Dark),
            DefaultTrayIconVariant::White
        );
    }

    #[test]
    fn default_tray_icon_is_black_for_light_windows_theme() {
        assert_eq!(
            default_tray_icon_variant(WindowsTheme::Light),
            DefaultTrayIconVariant::Black
        );
    }

    #[test]
    fn update_default_tray_icon_follows_windows_theme() {
        assert_eq!(
            TrayIconMode::Default.asset(WindowsTheme::Light, true),
            TrayIconAsset::UpdateBlack
        );
        assert_eq!(
            TrayIconMode::Default.asset(WindowsTheme::Dark, true),
            TrayIconAsset::UpdateWhite
        );
    }

    #[test]
    fn update_red_tray_icon_ignores_windows_theme() {
        assert_eq!(
            TrayIconMode::Red.asset(WindowsTheme::Light, true),
            TrayIconAsset::UpdateRed
        );
        assert_eq!(
            TrayIconMode::Red.asset(WindowsTheme::Dark, true),
            TrayIconAsset::UpdateRed
        );
    }

    #[test]
    fn macos_update_default_tray_icon_follows_appearance() {
        assert_eq!(
            TrayIconMode::Default.macos_asset(MacosAppearance::Light, true),
            MacosTrayIconAsset::UpdateBlack
        );
        assert_eq!(
            TrayIconMode::Default.macos_asset(MacosAppearance::Dark, true),
            MacosTrayIconAsset::UpdateWhite
        );
    }

    #[test]
    fn macos_update_red_tray_icon_ignores_appearance() {
        assert_eq!(
            TrayIconMode::Red.macos_asset(MacosAppearance::Light, true),
            MacosTrayIconAsset::UpdateRed
        );
        assert_eq!(
            TrayIconMode::Red.macos_asset(MacosAppearance::Dark, true),
            MacosTrayIconAsset::UpdateRed
        );
    }

    #[test]
    fn a_stale_transition_cannot_override_the_latest_selection() {
        let mut state = AppearanceState::new(ThemeMode::Auto, WindowsTheme::Light);
        let stale = state
            .begin(ThemeMode::Dark, WindowsTheme::Dark)
            .expect("first transition");
        let latest = state
            .begin(ThemeMode::Light, WindowsTheme::Light)
            .expect("newer transition");

        assert!(!state.commit(stale));
        assert!(!state.rollback(stale, WindowsTheme::Light));
        assert!(state.commit(latest));
        assert_eq!(
            state.confirmed,
            AppearanceSnapshot {
                mode: ThemeMode::Light,
                theme: WindowsTheme::Light,
            }
        );
        assert_eq!(state.displayed_mode, ThemeMode::Light);
        assert_eq!(state.visual_theme, WindowsTheme::Light);
    }

    #[test]
    fn forcing_the_current_mode_starts_a_transition() {
        let mut state = AppearanceState::new(ThemeMode::Dark, WindowsTheme::Dark);

        let transition = state.begin_forced(ThemeMode::Dark, WindowsTheme::Dark);

        assert!(transition.is_some());
        assert!(state.pending.is_some());
    }

    #[test]
    fn failed_transition_restores_the_last_confirmed_selection() {
        let mut state = AppearanceState::new(ThemeMode::Auto, WindowsTheme::Light);
        let transition = state
            .begin(ThemeMode::Dark, WindowsTheme::Dark)
            .expect("transition");

        assert!(state.rollback(transition, WindowsTheme::Light));
        assert_eq!(
            state.confirmed,
            AppearanceSnapshot {
                mode: ThemeMode::Auto,
                theme: WindowsTheme::Light,
            }
        );
        assert_eq!(state.displayed_mode, ThemeMode::Auto);
        assert_eq!(state.visual_theme, WindowsTheme::Light);
    }

    #[test]
    fn external_windows_change_updates_an_explicit_selection() {
        let mut state = AppearanceState::new(ThemeMode::Light, WindowsTheme::Light);

        let (previous_mode, next) = state
            .observe_external(WindowsTheme::Dark)
            .expect("external change");

        assert_eq!(previous_mode, ThemeMode::Light);
        assert_eq!(next.mode, ThemeMode::Dark);
        assert_eq!(state.displayed_mode, ThemeMode::Dark);
    }

    #[test]
    fn external_windows_change_keeps_auto_selected() {
        let mut state = AppearanceState::new(ThemeMode::Auto, WindowsTheme::Light);

        let (_, next) = state
            .observe_external(WindowsTheme::Dark)
            .expect("external change");

        assert_eq!(next.mode, ThemeMode::Auto);
        assert_eq!(next.theme, WindowsTheme::Dark);
        assert_eq!(state.displayed_mode, ThemeMode::Auto);
    }

    #[test]
    fn selecting_the_confirmed_mode_and_theme_is_a_noop() {
        let mut state = AppearanceState::new(ThemeMode::Dark, WindowsTheme::Dark);

        assert_eq!(state.begin(ThemeMode::Dark, WindowsTheme::Dark), None);
        assert_eq!(state.generation, 0);
    }

    #[test]
    fn a_new_auto_target_supersedes_an_in_flight_auto_target() {
        let mut state = AppearanceState::new(ThemeMode::Auto, WindowsTheme::Light);
        let stale = state
            .begin(ThemeMode::Auto, WindowsTheme::Dark)
            .expect("first Auto target");
        let latest = state
            .begin(ThemeMode::Auto, WindowsTheme::Light)
            .expect("updated Auto target");

        assert!(!state.commit(stale));
        assert!(state.commit(latest));
        assert_eq!(state.confirmed.theme, WindowsTheme::Light);
    }

    #[test]
    fn theme_modes_round_trip_through_persistence_values() {
        for (value, mode) in [
            ("auto", ThemeMode::Auto),
            ("light", ThemeMode::Light),
            ("dark", ThemeMode::Dark),
        ] {
            assert_eq!(ThemeMode::parse(value), Some(mode));
            assert_eq!(mode.as_str(), value);
        }
        assert_eq!(ThemeMode::parse("system"), None);
    }
}
