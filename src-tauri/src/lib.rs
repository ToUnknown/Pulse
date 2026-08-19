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
            set_update_menu(&update_item, "Update Check Failed — Retry", true);
            return;
        }
    };

    let update = match updater.check().await {
        Ok(update) => update,
        Err(error) => {
            eprintln!("update check failed: {error}");
            reset_update_status(&status);
            set_update_menu(&update_item, "Update Check Failed — Retry", true);
            return;
        }
    };

    let Some(update) = update else {
        reset_update_status(&status);
        set_update_menu(&update_item, "Up to Date — Check Again", true);
        return;
    };

    if let Ok(mut status) = status.lock() {
        *status = UpdateStatus::Downloading;
    } else {
        set_update_menu(&update_item, "Update Check Failed — Retry", true);
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
                set_update_menu(&update_item, &format!("Restart to Update {version}"), true);
            } else {
                set_update_menu(&update_item, "Update Check Failed — Retry", true);
            }
        }
        Err(error) => {
            eprintln!("update download failed: {error}");
            reset_update_status(&status);
            set_update_menu(&update_item, "Update Download Failed — Retry", true);
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
        set_update_menu(&update_item, "Updates Require a Release Build", true);
        return;
    }

    let ready_update = {
        let Ok(mut status) = status.lock() else {
            set_update_menu(&update_item, "Update Check Failed — Retry", true);
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
                set_update_menu(&update_item, "Update Install Failed — Retry", true);
                return;
            }

            // Do not return after installing: on macOS the updater has already
            // replaced the running app bundle, so the process must stay in the
            // restart path until Tauri exits and launches the updated binary.
            app.restart();
        });
    } else {
        tauri::async_runtime::spawn(check_for_updates(app, update_item, status));
    }
}

#[cfg(target_os = "macos")]
const TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-iconTemplate@2x.png");

#[cfg(target_os = "windows")]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-icon-32.png");

#[cfg(target_os = "windows")]
const RED_TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-red-32.png");

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-icon-32.png");

#[cfg(target_os = "windows")]
use {
    chrono::{Local, Timelike},
    std::{fs, path::Path, thread, time::Duration},
    tauri::{menu::Submenu, Manager},
    windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    },
    winreg::{enums::HKEY_CURRENT_USER, RegKey},
};

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ThemeMode {
    Auto,
    Light,
    Dark,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum TrayIconMode {
    Black,
    Red,
}

#[cfg(target_os = "windows")]
impl TrayIconMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "black" => Some(Self::Black),
            "red" => Some(Self::Red),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::Red => "red",
        }
    }

    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Black => TRAY_ICON_BYTES,
            Self::Red => RED_TRAY_ICON_BYTES,
        }
    }
}

#[cfg(target_os = "windows")]
fn load_tray_icon_mode(path: &Path) -> TrayIconMode {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| TrayIconMode::parse(value.trim()))
        .unwrap_or(TrayIconMode::Red)
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
fn scheduled_theme() -> ThemeMode {
    if (7..19).contains(&Local::now().hour()) {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    }
}

#[cfg(target_os = "windows")]
fn apply_windows_theme(mode: ThemeMode) -> Result<(), String> {
    let resolved_mode = if mode == ThemeMode::Auto {
        scheduled_theme()
    } else {
        mode
    };
    let use_light_theme = u32::from(resolved_mode == ThemeMode::Light);
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (personalize, _) = current_user
        .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
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

    Ok(())
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
fn select_tray_icon(
    app: &tauri::AppHandle,
    next_mode: TrayIconMode,
    mode: &Arc<Mutex<TrayIconMode>>,
    config_path: &Path,
) -> Result<(), String> {
    let mut selected_mode = mode.lock().map_err(|error| error.to_string())?;
    let tray = app
        .tray_by_id("pulse-tray")
        .ok_or_else(|| "pulse tray not found".to_string())?;
    let icon =
        tauri::image::Image::from_bytes(next_mode.bytes()).map_err(|error| error.to_string())?;
    tray.set_icon(Some(icon))
        .map_err(|error| error.to_string())?;
    save_tray_icon_mode(config_path, next_mode)?;
    *selected_mode = next_mode;
    Ok(())
}

#[cfg(target_os = "windows")]
fn start_auto_scheduler(mode: Arc<Mutex<ThemeMode>>) {
    thread::spawn(move || {
        let mut last_applied = None;

        loop {
            if let Ok(selected_mode) = mode.lock() {
                if *selected_mode == ThemeMode::Auto {
                    let current_theme = scheduled_theme();
                    if last_applied != Some(current_theme)
                        && apply_windows_theme(current_theme).is_ok()
                    {
                        last_applied = Some(current_theme);
                    }
                } else {
                    last_applied = None;
                }
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
            let (tray_icon, initial_tray_icon_mode, tray_icon_config_path) = {
                let config_path = app.path().app_config_dir()?.join("tray-icon");
                let selected_mode = load_tray_icon_mode(&config_path);
                (
                    tauri::image::Image::from_bytes(selected_mode.bytes())?,
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
                black,
                red,
            ) = {
                let config_path = app.path().app_config_dir()?.join("theme-mode");
                let selected_mode = load_theme_mode(&config_path);
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
                    let black = CheckMenuItem::with_id(
                        app,
                        "tray-icon-black",
                        "Black",
                        true,
                        initial_tray_icon_mode == TrayIconMode::Black,
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
                    let submenu = Submenu::with_items(app, "Tray Icon", true, &[&black, &red])?;
                    (submenu, black, red)
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

                let _ = apply_windows_theme(selected_mode);
                start_auto_scheduler(mode.clone());

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
                            if select_theme(next_mode, &mode, &config_path).is_ok() {
                                let _ = auto.set_checked(next_mode == ThemeMode::Auto);
                                let _ = light.set_checked(next_mode == ThemeMode::Light);
                                let _ = dark.set_checked(next_mode == ThemeMode::Dark);
                            }
                        }

                        let next_tray_icon = match event.id().as_ref() {
                            "tray-icon-black" => Some(TrayIconMode::Black),
                            "tray-icon-red" => Some(TrayIconMode::Red),
                            _ => None,
                        };

                        if let Some(next_mode) = next_tray_icon {
                            if let Err(error) = select_tray_icon(
                                app,
                                next_mode,
                                &tray_icon_mode,
                                &tray_icon_config_path,
                            ) {
                                eprintln!("tray icon selection failed: {error}");
                            } else {
                                let _ = black.set_checked(next_mode == TrayIconMode::Black);
                                let _ = red.set_checked(next_mode == TrayIconMode::Red);
                            }
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

            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if !cfg!(debug_assertions) {
                tauri::async_runtime::spawn(check_for_updates(
                    app.handle().clone(),
                    startup_update_item,
                    startup_update_status,
                ));
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Pulse");
}
