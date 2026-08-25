#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "windows")]
    if let Some(result) = pulse_lib::run_windows_uninstall_maintenance_if_requested() {
        if let Err(error) = result {
            eprintln!("Pulse driver uninstall failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if pulse_lib::run_audio_router_if_requested() {
        return;
    }
    pulse_lib::run();
}
