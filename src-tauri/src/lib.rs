use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use {
    std::sync::{Arc, Mutex},
    tauri::menu::CheckMenuItem,
    tauri_plugin_autostart::{MacosLauncher, ManagerExt},
    tauri_plugin_updater::{Update, UpdaterExt},
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
    tray_icon_mode: SharedTrayIconMode,
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
                if let Err(error) = refresh_tray_icon(&app, &tray_icon_mode, &status) {
                    eprintln!("update-ready tray icon failed: {error}");
                }
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
    tray_icon_mode: SharedTrayIconMode,
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
                if let Err(error) = refresh_tray_icon(&app, &tray_icon_mode, &status) {
                    eprintln!("default tray icon restore failed: {error}");
                }
                set_update_result(&app, &update_item, "Update Install Failed — Retry", true);
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
const UPDATE_TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-update-white@2x.png");

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

#[cfg(target_os = "windows")]
const RED_TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-red-32.png");

#[cfg(target_os = "windows")]
const UPDATE_RED_TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-update-red-32.png");

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-icon-32.png");

#[cfg(target_os = "windows")]
use {
    chrono::{Local, Timelike},
    std::{fs, path::Path, thread, time::Duration},
    tauri::{menu::Submenu, Manager},
    windows_sys::Win32::{
        System::Registry::{RegNotifyChangeKeyValue, REG_NOTIFY_CHANGE_LAST_SET},
        UI::WindowsAndMessaging::{
            SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
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

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ThemeMode {
    Auto,
    Light,
    Dark,
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

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum TrayIconMode {
    Default,
    Red,
}

#[cfg(any(target_os = "windows", test))]
impl TrayIconMode {
    #[cfg(target_os = "windows")]
    fn parse(value: &str) -> Option<Self> {
        match value {
            "default" | "black" => Some(Self::Default),
            "red" => Some(Self::Red),
            _ => None,
        }
    }

    #[cfg(target_os = "windows")]
    fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Red => "red",
        }
    }

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
}

#[cfg(target_os = "windows")]
type SharedTrayIconMode = Arc<Mutex<TrayIconMode>>;

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct SharedTrayIconMode;

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn update_icon_is_active(status: &SharedUpdateStatus) -> Result<bool, String> {
    let status = status.lock().map_err(|error| error.to_string())?;
    Ok(matches!(
        &*status,
        UpdateStatus::Ready(_) | UpdateStatus::Installing
    ))
}

#[cfg(target_os = "macos")]
fn refresh_tray_icon(
    app: &tauri::AppHandle,
    _mode: &SharedTrayIconMode,
    status: &SharedUpdateStatus,
) -> Result<(), String> {
    let update_ready = update_icon_is_active(status)?;
    let tray = app
        .tray_by_id("pulse-tray")
        .ok_or_else(|| "pulse tray not found".to_string())?;
    let bytes = if update_ready {
        UPDATE_TRAY_ICON_BYTES
    } else {
        TRAY_ICON_BYTES
    };
    let icon = tauri::image::Image::from_bytes(bytes).map_err(|error| error.to_string())?;
    tray.set_icon_with_as_template(Some(icon), !update_ready)
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn load_tray_icon_mode(path: &Path) -> TrayIconMode {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| TrayIconMode::parse(value.trim()))
        .unwrap_or(TrayIconMode::Default)
}

#[cfg(target_os = "windows")]
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
fn scheduled_theme() -> WindowsTheme {
    if (7..19).contains(&Local::now().hour()) {
        WindowsTheme::Light
    } else {
        WindowsTheme::Dark
    }
}

#[cfg(target_os = "windows")]
fn resolve_theme(mode: ThemeMode) -> WindowsTheme {
    match mode {
        ThemeMode::Auto => scheduled_theme(),
        ThemeMode::Light => WindowsTheme::Light,
        ThemeMode::Dark => WindowsTheme::Dark,
    }
}

#[cfg(target_os = "windows")]
fn apply_windows_theme(mode: ThemeMode) -> Result<WindowsTheme, String> {
    let resolved_theme = resolve_theme(mode);
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
) -> Result<(), String> {
    let mut selected_mode = mode.lock().map_err(|error| error.to_string())?;
    apply_windows_theme(next_mode)?;
    save_theme_mode(config_path, next_mode)?;
    *selected_mode = next_mode;
    Ok(())
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
fn refresh_tray_icon(
    app: &tauri::AppHandle,
    mode: &SharedTrayIconMode,
    status: &SharedUpdateStatus,
) -> Result<(), String> {
    let selected_mode = *mode.lock().map_err(|error| error.to_string())?;
    set_tray_icon(
        app,
        selected_mode,
        current_windows_theme()?,
        update_icon_is_active(status)?,
    )
}

#[cfg(target_os = "windows")]
fn select_tray_icon(
    app: &tauri::AppHandle,
    next_mode: TrayIconMode,
    mode: &Arc<Mutex<TrayIconMode>>,
    config_path: &Path,
    update_status: &SharedUpdateStatus,
) -> Result<(), String> {
    let mut selected_mode = mode.lock().map_err(|error| error.to_string())?;
    set_tray_icon(
        app,
        next_mode,
        current_windows_theme()?,
        update_icon_is_active(update_status)?,
    )?;
    save_tray_icon_mode(config_path, next_mode)?;
    *selected_mode = next_mode;
    Ok(())
}

#[cfg(target_os = "windows")]
fn refresh_default_tray_icon(
    app: &tauri::AppHandle,
    mode: &Arc<Mutex<TrayIconMode>>,
    theme: WindowsTheme,
    update_status: &SharedUpdateStatus,
) -> Result<(), String> {
    let selected_mode = *mode.lock().map_err(|error| error.to_string())?;
    if selected_mode == TrayIconMode::Default {
        set_tray_icon(
            app,
            selected_mode,
            theme,
            update_icon_is_active(update_status)?,
        )?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn start_windows_theme_watcher(
    app: tauri::AppHandle,
    tray_icon_mode: Arc<Mutex<TrayIconMode>>,
    update_status: SharedUpdateStatus,
) {
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
            match windows_theme_from_registry(&personalize).and_then(|theme| {
                refresh_default_tray_icon(&app, &tray_icon_mode, theme, &update_status)
            }) {
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
fn start_auto_scheduler(mode: Arc<Mutex<ThemeMode>>) {
    thread::spawn(move || {
        let mut last_applied = None;

        loop {
            let selected_mode = mode.lock().map(|mode| *mode).ok();
            if selected_mode == Some(ThemeMode::Auto) {
                let current_theme = scheduled_theme();
                if last_applied != Some(current_theme) {
                    let explicit_mode = match current_theme {
                        WindowsTheme::Light => ThemeMode::Light,
                        WindowsTheme::Dark => ThemeMode::Dark,
                    };
                    if apply_windows_theme(explicit_mode).is_ok() {
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

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let builder = builder.plugin(tauri_plugin_autostart::init(
        MacosLauncher::LaunchAgent,
        None,
    ));

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());

    builder
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            #[cfg(target_os = "windows")]
            let (initial_theme_mode, theme_config_path) = {
                let config_path = app.path().app_config_dir()?.join("theme-mode");
                (load_theme_mode(&config_path), config_path)
            };
            #[cfg(target_os = "windows")]
            let applied_theme = apply_windows_theme(initial_theme_mode).unwrap_or_else(|error| {
                eprintln!("initial Windows theme update failed: {error}");
                resolve_theme(initial_theme_mode)
            });

            #[cfg(target_os = "windows")]
            let (tray_icon, initial_tray_icon_mode, tray_icon_config_path) = {
                let config_path = app.path().app_config_dir()?.join("tray-icon");
                let selected_mode = load_tray_icon_mode(&config_path);
                let windows_theme = current_windows_theme().unwrap_or(applied_theme);
                (
                    tauri::image::Image::from_bytes(selected_mode.bytes(windows_theme, false))?,
                    selected_mode,
                    config_path,
                )
            };
            #[cfg(not(target_os = "windows"))]
            let tray_icon = tauri::image::Image::from_bytes(TRAY_ICON_BYTES)?;
            let separator = PredefinedMenuItem::separator(app)?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let quit_separator = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Pulse", true, None::<&str>)?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let start_at_login = CheckMenuItem::with_id(
                app,
                "start-at-login",
                "Start at Login",
                true,
                app.autolaunch().is_enabled().unwrap_or(false),
                None::<&str>,
            )?;
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
            #[cfg(target_os = "macos")]
            let tray_icon_mode = SharedTrayIconMode;

            #[cfg(target_os = "windows")]
            let (
                menu,
                auto,
                light,
                dark,
                mode,
                config_path,
                tray_icon_mode,
                tray_icon_config_path,
                default_icon,
                red,
            ) = {
                let config_path = theme_config_path;
                let selected_mode = initial_theme_mode;
                let mode = Arc::new(Mutex::new(selected_mode));
                let auto = CheckMenuItem::with_id(
                    app,
                    "theme-auto",
                    "Auto (07:00–19:00)",
                    true,
                    selected_mode == ThemeMode::Auto,
                    None::<&str>,
                )?;
                let light = CheckMenuItem::with_id(
                    app,
                    "theme-light",
                    "Light",
                    true,
                    selected_mode == ThemeMode::Light,
                    None::<&str>,
                )?;
                let dark = CheckMenuItem::with_id(
                    app,
                    "theme-dark",
                    "Dark",
                    true,
                    selected_mode == ThemeMode::Dark,
                    None::<&str>,
                )?;
                let appearance =
                    Submenu::with_items(app, "Appearance", true, &[&auto, &light, &dark])?;
                let tray_icon_submenu = {
                    let default_icon = CheckMenuItem::with_id(
                        app,
                        "tray-icon-default",
                        "Default",
                        true,
                        initial_tray_icon_mode == TrayIconMode::Default,
                        None::<&str>,
                    )?;
                    let red = CheckMenuItem::with_id(
                        app,
                        "tray-icon-red",
                        "Red",
                        true,
                        initial_tray_icon_mode == TrayIconMode::Red,
                        None::<&str>,
                    )?;
                    let submenu =
                        Submenu::with_items(app, "Tray Icon", true, &[&default_icon, &red])?;
                    (submenu, default_icon, red)
                };
                let tray_icon_mode = Arc::new(Mutex::new(initial_tray_icon_mode));
                let menu = Menu::with_items(
                    app,
                    &[
                        &appearance,
                        &tray_icon_submenu.0,
                        &separator,
                        &start_at_login,
                        &update_item,
                        &quit_separator,
                        &quit,
                    ],
                )?;

                (
                    menu,
                    auto,
                    light,
                    dark,
                    mode,
                    config_path,
                    tray_icon_mode,
                    tray_icon_config_path,
                    tray_icon_submenu.1,
                    tray_icon_submenu.2,
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
                        &start_at_login,
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
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let startup_tray_icon_mode = tray_icon_mode.clone();
            #[cfg(target_os = "windows")]
            let scheduler_mode = mode.clone();
            #[cfg(target_os = "windows")]
            let watcher_tray_icon_mode = tray_icon_mode.clone();

            TrayIconBuilder::with_id("pulse-tray")
                .icon(tray_icon)
                .icon_as_template(true)
                .tooltip("Pulse")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app, event| {
                    #[cfg(any(target_os = "macos", target_os = "windows"))]
                    if event.id().as_ref() == "start-at-login" {
                        let autostart = app.autolaunch();
                        let _ = if autostart.is_enabled().unwrap_or(false) {
                            autostart.disable()
                        } else {
                            autostart.enable()
                        };

                        let _ = start_at_login.set_checked(autostart.is_enabled().unwrap_or(false));
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
                            match select_theme(next_mode, &mode, &config_path) {
                                Ok(()) => {
                                    let _ = auto.set_checked(next_mode == ThemeMode::Auto);
                                    let _ = light.set_checked(next_mode == ThemeMode::Light);
                                    let _ = dark.set_checked(next_mode == ThemeMode::Dark);
                                }
                                Err(error) => eprintln!("theme selection failed: {error}"),
                            }
                        }

                        let next_tray_icon = match event.id().as_ref() {
                            "tray-icon-default" => Some(TrayIconMode::Default),
                            "tray-icon-red" => Some(TrayIconMode::Red),
                            _ => None,
                        };

                        if let Some(next_mode) = next_tray_icon {
                            if let Err(error) = select_tray_icon(
                                app,
                                next_mode,
                                &tray_icon_mode,
                                &tray_icon_config_path,
                                &update_status,
                            ) {
                                eprintln!("tray icon selection failed: {error}");
                            } else {
                                let _ =
                                    default_icon.set_checked(next_mode == TrayIconMode::Default);
                                let _ = red.set_checked(next_mode == TrayIconMode::Red);
                            }
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
                        app.exit(0);
                    }
                })
                .build(app)?;

            #[cfg(target_os = "windows")]
            start_auto_scheduler(scheduler_mode);
            #[cfg(target_os = "windows")]
            start_windows_theme_watcher(
                app.handle().clone(),
                watcher_tray_icon_mode,
                startup_update_status.clone(),
            );

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
        default_tray_icon_variant, DefaultTrayIconVariant, TrayIconAsset, TrayIconMode,
        WindowsTheme,
    };

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
}
