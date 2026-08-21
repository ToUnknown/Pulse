use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use {
    std::{
        fs,
        path::Path,
        sync::{Arc, Mutex},
    },
    tauri_plugin_autostart::{MacosLauncher, ManagerExt},
    tauri_plugin_updater::{Update, UpdaterExt},
};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use tauri::{WebviewUrl, WebviewWindowBuilder};

#[cfg(target_os = "macos")]
use tauri::Manager as _;

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
type SharedUpdateStatus = Arc<Mutex<UpdateStatus>>;

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn set_update_menu(update_item: &MenuItem<tauri::Wry>, text: &str, enabled: bool) {
    let _ = update_item.set_text(text);
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
    update_item: &MenuItem<tauri::Wry>,
    text: &str,
    reveal_result: bool,
) {
    set_update_menu(update_item, text, true);
    if reveal_result {
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
    update_item: MenuItem<tauri::Wry>,
    status: SharedUpdateStatus,
    reveal_result: bool,
) {
    if !begin_update_check(&status) {
        return;
    }

    set_update_menu(&update_item, "Checking for Updates…", false);

    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(error) => {
            eprintln!("update setup failed: {error}");
            reset_update_status(&status);
            set_update_result(
                &app,
                &update_item,
                "Update Check Failed — Retry",
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
                "Update Check Failed — Retry",
                reveal_result,
            );
            return;
        }
    };

    let Some(update) = update else {
        reset_update_status(&status);
        set_update_result(
            &app,
            &update_item,
            "Up to Date — Check Again",
            reveal_result,
        );
        return;
    };

    if let Ok(mut status) = status.lock() {
        *status = UpdateStatus::Downloading;
    } else {
        set_update_result(
            &app,
            &update_item,
            "Update Check Failed — Retry",
            reveal_result,
        );
        return;
    }

    let version = update.version.clone();
    set_update_menu(
        &update_item,
        &format!("Downloading Update {version}…"),
        false,
    );

    match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => {
            let update_ready = if let Ok(mut status) = status.lock() {
                *status = UpdateStatus::Ready(Box::new(DownloadedUpdate { update, bytes }));
                true
            } else {
                false
            };

            if update_ready {
                set_update_result(
                    &app,
                    &update_item,
                    &format!("Restart to Update {version}"),
                    reveal_result,
                );
            } else {
                set_update_result(
                    &app,
                    &update_item,
                    "Update Check Failed — Retry",
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
                "Update Download Failed — Retry",
                reveal_result,
            );
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn handle_update_menu(
    app: tauri::AppHandle,
    update_item: MenuItem<tauri::Wry>,
    status: SharedUpdateStatus,
) {
    if cfg!(debug_assertions) {
        set_update_result(&app, &update_item, "Updates Require a Release Build", true);
        return;
    }

    let ready_update = {
        let Ok(mut status) = status.lock() else {
            set_update_result(&app, &update_item, "Update Check Failed — Retry", true);
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
        set_update_menu(&update_item, "Installing Update…", false);
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = downloaded.update.install(&downloaded.bytes) {
                eprintln!("update install failed: {error}");
                reset_update_status(&status);
                set_update_result(&app, &update_item, "Update Install Failed — Retry", true);
                return;
            }

            // Do not return after installing: on macOS the updater has already
            // replaced the running app bundle, so the process must stay in the
            // restart path until Tauri exits and launches the updated binary.
            app.restart();
        });
    } else {
        tauri::async_runtime::spawn(check_for_updates(app, update_item, status, true));
    }
}

#[cfg(target_os = "macos")]
const TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-iconTemplate@2x.png");

#[cfg(target_os = "windows")]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-icon-32.png");

#[cfg(target_os = "windows")]
const WHITE_TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-white-32.png");

#[cfg(any(target_os = "macos", target_os = "windows"))]
const RED_TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-red-32.png");

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-icon-32.png");

#[cfg(target_os = "windows")]
use {
    chrono::{Local, Timelike},
    std::{thread, time::Duration},
    tauri::{
        menu::{ContextMenu, Submenu},
        Manager,
    },
    windows_sys::Win32::{
        System::Registry::{RegNotifyChangeKeyValue, REG_NOTIFY_CHANGE_LAST_SET},
        UI::WindowsAndMessaging::{
            CheckMenuItem, SendMessageTimeoutW, HWND_BROADCAST, MF_BYPOSITION, MF_CHECKED,
            MF_UNCHECKED, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
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
fn appearance_menu_position(mode: ThemeMode) -> u32 {
    match mode {
        ThemeMode::Auto => 0,
        ThemeMode::Light => 1,
        ThemeMode::Dark => 2,
    }
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
enum DefaultTrayIconVariant {
    Black,
    White,
}

#[cfg(any(target_os = "windows", test))]
fn default_tray_icon_variant(theme: WindowsTheme) -> DefaultTrayIconVariant {
    match theme {
        WindowsTheme::Light => DefaultTrayIconVariant::Black,
        WindowsTheme::Dark => DefaultTrayIconVariant::White,
    }
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
}

#[cfg(target_os = "windows")]
struct AutoScheduleSettings {
    mode: Arc<Mutex<ThemeMode>>,
    schedule: Arc<Mutex<AutoSchedule>>,
    config_path: std::path::PathBuf,
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
        .try_state::<AutoScheduleSettings>()
        .and_then(|settings| settings.schedule.lock().ok().map(|schedule| *schedule))
        .map(|schedule| {
            serde_json::json!({
                "lightStart": schedule.light_start,
                "darkStart": schedule.dark_start,
            })
        });
    #[cfg(target_os = "macos")]
    let auto_schedule: Option<serde_json::Value> = None;

    serde_json::json!({
        "platform": std::env::consts::OS,
        "startAtLogin": start_at_login,
        "trayIcon": tray_icon,
        "autoSchedule": auto_schedule,
    })
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
    select_tray_icon(&app, next_mode, &settings.mode, &settings.config_path)
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn set_auto_schedule(app: tauri::AppHandle, light_start: u8, dark_start: u8) -> Result<(), String> {
    let next_schedule = AutoSchedule::new(light_start, dark_start)
        .ok_or_else(|| "choose two different hours between 00:00 and 23:00".to_string())?;
    let settings = app.state::<AutoScheduleSettings>();

    save_auto_schedule(&settings.config_path, next_schedule)?;
    *settings
        .schedule
        .lock()
        .map_err(|error| error.to_string())? = next_schedule;

    let selected_mode = *settings.mode.lock().map_err(|error| error.to_string())?;
    if selected_mode == ThemeMode::Auto {
        apply_windows_theme(selected_mode, next_schedule)?;
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
        let window_height = 440.0;
        #[cfg(target_os = "macos")]
        let window_height = 288.0;

        WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
            .title("Pulse Settings")
            .inner_size(420.0, window_height)
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

    #[cfg(target_os = "windows")]
    fn bytes(self, theme: WindowsTheme) -> &'static [u8] {
        match self {
            Self::Default => match default_tray_icon_variant(theme) {
                DefaultTrayIconVariant::Black => TRAY_ICON_BYTES,
                DefaultTrayIconVariant::White => WHITE_TRAY_ICON_BYTES,
            },
            Self::Red => RED_TRAY_ICON_BYTES,
        }
    }

    #[cfg(target_os = "macos")]
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Default => TRAY_ICON_BYTES,
            Self::Red => RED_TRAY_ICON_BYTES,
        }
    }
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

#[cfg(target_os = "windows")]
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
fn apply_windows_theme(mode: ThemeMode, schedule: AutoSchedule) -> Result<WindowsTheme, String> {
    let resolved_theme = resolve_theme(mode, schedule);
    let use_light_theme = u32::from(resolved_theme == WindowsTheme::Light);
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

    let setting_name: Vec<u16> = "ImmersiveColorSet\0".encode_utf16().collect();
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            setting_name.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5_000,
            std::ptr::null_mut(),
        );
    }

    Ok(resolved_theme)
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
fn select_theme(
    next_mode: ThemeMode,
    mode: &Arc<Mutex<ThemeMode>>,
    config_path: &Path,
    schedule: &Arc<Mutex<AutoSchedule>>,
) -> Result<(), String> {
    let mut selected_mode = mode.lock().map_err(|error| error.to_string())?;
    let schedule = *schedule.lock().map_err(|error| error.to_string())?;
    apply_windows_theme(next_mode, schedule)?;
    save_theme_mode(config_path, next_mode)?;
    *selected_mode = next_mode;
    Ok(())
}

#[cfg(target_os = "windows")]
fn sync_appearance_menu(
    selected_mode: ThemeMode,
    appearance: &Submenu<tauri::Wry>,
) -> Result<(), String> {
    let menu = appearance.hpopupmenu().map_err(|error| error.to_string())?;
    let selected_position = appearance_menu_position(selected_mode);

    for position in 0..3 {
        let checked = if position == selected_position {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        let previous_state = unsafe { CheckMenuItem(menu as _, position, MF_BYPOSITION | checked) };
        if previous_state == u32::MAX {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn set_tray_icon(
    app: &tauri::AppHandle,
    mode: TrayIconMode,
    theme: WindowsTheme,
) -> Result<(), String> {
    let tray = app
        .tray_by_id("pulse-tray")
        .ok_or_else(|| "pulse tray not found".to_string())?;
    let icon =
        tauri::image::Image::from_bytes(mode.bytes(theme)).map_err(|error| error.to_string())?;
    tray.set_icon(Some(icon)).map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn set_tray_icon(app: &tauri::AppHandle, mode: TrayIconMode) -> Result<(), String> {
    let tray = app
        .tray_by_id("pulse-tray")
        .ok_or_else(|| "pulse tray not found".to_string())?;
    let icon = tauri::image::Image::from_bytes(mode.bytes()).map_err(|error| error.to_string())?;
    tray.set_icon_with_as_template(Some(icon), mode == TrayIconMode::Default)
        .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn select_tray_icon(
    app: &tauri::AppHandle,
    next_mode: TrayIconMode,
    mode: &Arc<Mutex<TrayIconMode>>,
    config_path: &Path,
) -> Result<(), String> {
    let mut selected_mode = mode.lock().map_err(|error| error.to_string())?;

    #[cfg(target_os = "windows")]
    set_tray_icon(app, next_mode, current_windows_theme()?)?;
    #[cfg(target_os = "macos")]
    set_tray_icon(app, next_mode)?;

    save_tray_icon_mode(config_path, next_mode)?;
    *selected_mode = next_mode;
    Ok(())
}

#[cfg(target_os = "windows")]
fn refresh_default_tray_icon(
    app: &tauri::AppHandle,
    mode: &Arc<Mutex<TrayIconMode>>,
    theme: WindowsTheme,
) -> Result<(), String> {
    let selected_mode = *mode.lock().map_err(|error| error.to_string())?;
    if selected_mode == TrayIconMode::Default {
        set_tray_icon(app, selected_mode, theme)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn start_windows_theme_watcher(app: tauri::AppHandle, tray_icon_mode: Arc<Mutex<TrayIconMode>>) {
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
            match windows_theme_from_registry(&personalize)
                .and_then(|theme| refresh_default_tray_icon(&app, &tray_icon_mode, theme))
            {
                Ok(()) => {}
                Err(error) => eprintln!("Windows theme tray icon update failed: {error}"),
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
fn start_auto_scheduler(mode: Arc<Mutex<ThemeMode>>, schedule: Arc<Mutex<AutoSchedule>>) {
    thread::spawn(move || {
        let mut last_applied = None;

        loop {
            let selected_mode = mode.lock().map(|mode| *mode).ok();
            let auto_schedule = schedule.lock().map(|schedule| *schedule).ok();
            if let (Some(ThemeMode::Auto), Some(auto_schedule)) = (selected_mode, auto_schedule) {
                let current_theme = scheduled_theme(auto_schedule);
                if last_applied != Some(current_theme) {
                    let explicit_mode = match current_theme {
                        WindowsTheme::Light => ThemeMode::Light,
                        WindowsTheme::Dark => ThemeMode::Dark,
                    };
                    if apply_windows_theme(explicit_mode, auto_schedule).is_ok() {
                        last_applied = Some(current_theme);
                    }
                }
            } else {
                last_applied = None;
            }

            thread::sleep(Duration::from_secs(30));
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    #[cfg(target_os = "windows")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        settings_state,
        set_start_at_login,
        set_tray_icon_mode,
        set_auto_schedule
    ]);

    #[cfg(target_os = "macos")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        settings_state,
        set_start_at_login,
        set_tray_icon_mode
    ]);

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let builder = builder.plugin(tauri_plugin_autostart::init(
        MacosLauncher::LaunchAgent,
        None,
    ));

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());

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
            let applied_theme = apply_windows_theme(initial_theme_mode, initial_auto_schedule)
                .unwrap_or_else(|error| {
                    eprintln!("initial Windows theme update failed: {error}");
                    resolve_theme(initial_theme_mode, initial_auto_schedule)
                });

            #[cfg(target_os = "windows")]
            let tray_icon = {
                let windows_theme = current_windows_theme().unwrap_or(applied_theme);
                tauri::image::Image::from_bytes(initial_tray_icon_mode.bytes(windows_theme))?
            };
            #[cfg(target_os = "macos")]
            let tray_icon = tauri::image::Image::from_bytes(initial_tray_icon_mode.bytes())?;
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let tray_icon = tauri::image::Image::from_bytes(TRAY_ICON_BYTES)?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let tray_icon_is_template = initial_tray_icon_mode == TrayIconMode::Default;
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let tray_icon_is_template = true;
            let separator = PredefinedMenuItem::separator(app)?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let quit_separator = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Pulse", true, None::<&str>)?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let update_item = MenuItem::with_id(
                app,
                "check-for-updates",
                "Check for Updates…",
                true,
                None::<&str>,
            )?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let update_status = Arc::new(Mutex::new(UpdateStatus::Idle));

            #[cfg(target_os = "windows")]
            let (menu, appearance, mode, config_path, auto_schedule, auto_schedule_config_path) = {
                let config_path = theme_config_path;
                let selected_mode = initial_theme_mode;
                let mode = Arc::new(Mutex::new(selected_mode));
                let auto_schedule = Arc::new(Mutex::new(initial_auto_schedule));
                let auto = MenuItem::with_id(app, "theme-auto", "Auto", true, None::<&str>)?;
                let light = MenuItem::with_id(app, "theme-light", "Light", true, None::<&str>)?;
                let dark = MenuItem::with_id(app, "theme-dark", "Dark", true, None::<&str>)?;
                let appearance =
                    Submenu::with_items(app, "Appearance", true, &[&auto, &light, &dark])?;
                sync_appearance_menu(selected_mode, &appearance)?;
                let menu = Menu::with_items(
                    app,
                    &[
                        &appearance,
                        &separator,
                        &settings,
                        &update_item,
                        &quit_separator,
                        &quit,
                    ],
                )?;

                (
                    menu,
                    appearance,
                    mode,
                    config_path,
                    auto_schedule,
                    auto_schedule_config_path,
                )
            };

            #[cfg(target_os = "macos")]
            let menu = {
                let status =
                    MenuItem::with_id(app, "status", "Pulse is running", false, None::<&str>)?;
                Menu::with_items(
                    app,
                    &[
                        &status,
                        &separator,
                        &settings,
                        &update_item,
                        &quit_separator,
                        &quit,
                    ],
                )?
            };

            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let menu = {
                let status =
                    MenuItem::with_id(app, "status", "Pulse is running", false, None::<&str>)?;
                Menu::with_items(app, &[&status, &separator, &quit])?
            };

            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let startup_update_item = update_item.clone();
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let startup_update_status = update_status.clone();
            #[cfg(target_os = "windows")]
            let scheduler_mode = mode.clone();
            #[cfg(target_os = "windows")]
            let scheduler_auto_schedule = auto_schedule.clone();
            #[cfg(target_os = "windows")]
            app.manage(AutoScheduleSettings {
                mode: mode.clone(),
                schedule: auto_schedule.clone(),
                config_path: auto_schedule_config_path,
            });
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let tray_icon_mode = Arc::new(Mutex::new(initial_tray_icon_mode));
            #[cfg(target_os = "windows")]
            let watcher_tray_icon_mode = tray_icon_mode.clone();
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            app.manage(TrayIconSettings {
                mode: tray_icon_mode.clone(),
                config_path: tray_icon_config_path,
            });

            TrayIconBuilder::with_id("pulse-tray")
                .icon(tray_icon)
                .icon_as_template(tray_icon_is_template)
                .tooltip("Pulse")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app, event| {
                    #[cfg(target_os = "windows")]
                    {
                        let next_mode = match event.id().as_ref() {
                            "theme-auto" => Some(ThemeMode::Auto),
                            "theme-light" => Some(ThemeMode::Light),
                            "theme-dark" => Some(ThemeMode::Dark),
                            _ => None,
                        };

                        if let Some(next_mode) = next_mode {
                            let previous_mode = mode
                                .lock()
                                .map(|selected_mode| *selected_mode)
                                .unwrap_or(initial_theme_mode);
                            let selected_mode = match select_theme(
                                next_mode,
                                &mode,
                                &config_path,
                                &auto_schedule,
                            ) {
                                Ok(()) => next_mode,
                                Err(error) => {
                                    eprintln!("theme selection failed: {error}");
                                    previous_mode
                                }
                            };

                            if let Err(error) = sync_appearance_menu(selected_mode, &appearance) {
                                eprintln!("appearance menu sync failed: {error}");
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
                        handle_update_menu(app.clone(), update_item.clone(), update_status.clone());
                    }

                    if event.id().as_ref() == "quit" {
                        app.exit(0);
                    }
                })
                .build(app)?;

            #[cfg(target_os = "windows")]
            start_auto_scheduler(scheduler_mode, scheduler_auto_schedule);
            #[cfg(target_os = "windows")]
            start_windows_theme_watcher(app.handle().clone(), watcher_tray_icon_mode);

            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if !cfg!(debug_assertions) {
                tauri::async_runtime::spawn(check_for_updates(
                    app.handle().clone(),
                    startup_update_item,
                    startup_update_status,
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
        appearance_menu_position, default_tray_icon_variant, AutoSchedule, DefaultTrayIconVariant,
        ThemeMode, WindowsTheme,
    };

    #[test]
    fn appearance_modes_map_to_distinct_menu_positions() {
        assert_eq!(appearance_menu_position(ThemeMode::Auto), 0);
        assert_eq!(appearance_menu_position(ThemeMode::Light), 1);
        assert_eq!(appearance_menu_position(ThemeMode::Dark), 2);
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
}
