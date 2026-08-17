use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

#[cfg(target_os = "macos")]
const TRAY_ICON_BYTES: &[u8] =
    include_bytes!("../icons/tray/pulse-tray-expanded-iconTemplate@2x.png");

#[cfg(target_os = "windows")]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-icon-32.png");

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray/pulse-tray-expanded-icon-32.png");

#[cfg(target_os = "windows")]
use {
    chrono::{Local, Timelike},
    std::{
        fs,
        path::Path,
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    },
    tauri::{
        menu::{CheckMenuItem, Submenu},
        Manager,
    },
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
fn start_auto_scheduler(mode: Arc<Mutex<ThemeMode>>) {
    thread::spawn(move || {
        let mut last_applied = None;

        loop {
            if let Ok(selected_mode) = mode.lock() {
                if *selected_mode == ThemeMode::Auto {
                    let current_theme = scheduled_theme();
                    if last_applied != Some(current_theme) {
                        if apply_windows_theme(current_theme).is_ok() {
                            last_applied = Some(current_theme);
                        }
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
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let tray_icon = tauri::image::Image::from_bytes(TRAY_ICON_BYTES)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Pulse", true, None::<&str>)?;

            #[cfg(target_os = "windows")]
            let (menu, auto, light, dark, mode, config_path) = {
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
                let menu = Menu::with_items(app, &[&appearance, &separator, &quit])?;

                let _ = apply_windows_theme(selected_mode);
                start_auto_scheduler(mode.clone());

                (menu, auto, light, dark, mode, config_path)
            };

            #[cfg(not(target_os = "windows"))]
            let menu = {
                let status =
                    MenuItem::with_id(app, "status", "Pulse is running", false, None::<&str>)?;
                Menu::with_items(app, &[&status, &separator, &quit])?
            };

            TrayIconBuilder::with_id("pulse-tray")
                .icon(tray_icon)
                .icon_as_template(true)
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
                            if select_theme(next_mode, &mode, &config_path).is_ok() {
                                let _ = auto.set_checked(next_mode == ThemeMode::Auto);
                                let _ = light.set_checked(next_mode == ThemeMode::Light);
                                let _ = dark.set_checked(next_mode == ThemeMode::Dark);
                            }
                        }
                    }

                    if event.id().as_ref() == "quit" {
                        app.exit(0);
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Pulse");
}
