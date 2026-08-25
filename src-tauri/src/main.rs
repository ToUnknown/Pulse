#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if pulse_lib::run_audio_router_if_requested() {
        return;
    }
    pulse_lib::run();
}
