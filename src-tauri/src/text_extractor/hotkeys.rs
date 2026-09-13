use super::{
    shortcut_keys::{Action, Decision, Key, ShortcutKeys},
    windows::{self, CaptureMode},
};
use std::{
    cell::RefCell,
    ptr::null_mut,
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc::{sync_channel, SyncSender},
        OnceLock,
    },
};
use tauri::Manager;
use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_NONAME,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
            UnhookWindowsHookEx, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
            WM_SYSKEYDOWN,
        },
    },
};

static FLAGS: AtomicU8 = AtomicU8::new(0);
static EVENTS: OnceLock<SyncSender<Action>> = OnceLock::new();
thread_local! { static KEYS: RefCell<ShortcutKeys> = RefCell::new(ShortcutKeys::default()); }

pub fn configure(enabled: bool, editor_default: bool) {
    FLAGS
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |flags| {
            Some((flags & 4) | u8::from(enabled) | (u8::from(editor_default) << 1))
        })
        .ok();
}
pub fn recording(value: bool) {
    if value {
        FLAGS.fetch_or(4, Ordering::AcqRel);
    } else {
        FLAGS.fetch_and(!4, Ordering::AcqRel);
    }
}

pub fn install(app: &tauri::AppHandle) -> Result<(), String> {
    let (events, receive) = sync_channel::<Action>(8);
    EVENTS
        .set(events)
        .map_err(|_| "Text Extractor shortcuts are already installed.")?;
    let (ready, started) = sync_channel(1);
    std::thread::Builder::new()
        .name("pulse-extractor-keys".into())
        .spawn(move || unsafe {
            let hook = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard),
                GetModuleHandleW(null_mut()),
                0,
            );
            let _ = ready.send(!hook.is_null());
            if hook.is_null() {
                return;
            }
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            UnhookWindowsHookEx(hook);
        })
        .map_err(|_| "Could not start the Text Extractor shortcuts.")?;
    if !started
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or(false)
    {
        return Err("Windows could not install the Text Extractor shortcuts.".into());
    }
    let app = app.clone();
    std::thread::Builder::new().name("pulse-extractor-actions".into()).spawn(move || {
        while let Ok(action) = receive.recv() {
            match action {
                Action::RecordQuick | Action::RecordEditor => {
                    if let Some(window) = app.get_webview_window("settings") {
                        let shortcut = if action == Action::RecordQuick { "Super+Shift+KeyT" } else { "Control+Super+Shift+KeyT" };
                        let _ = window.eval(format!("window.dispatchEvent(new CustomEvent('pulse-extractor-shortcut', {{detail: '{shortcut}'}}))"));
                    }
                }
                Action::QuickCopy | Action::Editor => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let mode = if action == Action::QuickCopy { CaptureMode::Quick } else { CaptureMode::Editor };
                        if let Err(error) = windows::start(&app, mode).await { windows::report_error(&app, error); }
                    });
                }
            }
        }
    }).map_err(|_| "Could not start the Text Extractor shortcut actions.")?;
    Ok(())
}

unsafe extern "system" fn keyboard(code: i32, message: WPARAM, event: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = &*(event as *const KBDLLHOOKSTRUCT);
        if event.flags & LLKHF_INJECTED == 0 {
            let key = match event.vkCode {
                0x5B => Key::WinLeft,
                0x5C => Key::WinRight,
                0xA0 => Key::ShiftLeft,
                0xA1 => Key::ShiftRight,
                0xA2 => Key::ControlLeft,
                0xA3 => Key::ControlRight,
                0xA4 => Key::AltLeft,
                0xA5 => Key::AltRight,
                0x54 => Key::T,
                _ => Key::Other,
            };
            let flags = FLAGS.load(Ordering::Acquire);
            let down = message as u32 == WM_KEYDOWN || message as u32 == WM_SYSKEYDOWN;
            let decision = KEYS.with(|keys| {
                keys.borrow_mut()
                    .update(key, down, flags & 1 != 0, flags & 2 != 0, flags & 4 != 0)
            });
            if let Decision::Suppress(action) = decision {
                if let Some(action) = action {
                    // Mask the Windows-key menu with an unused key. Modifier releases
                    // still reach Windows, so no key is left logically held down.
                    let inputs = [false, true].map(|up| INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VK_NONAME,
                                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                                ..std::mem::zeroed()
                            },
                        },
                    });
                    SendInput(
                        inputs.len() as u32,
                        inputs.as_ptr(),
                        std::mem::size_of::<INPUT>() as i32,
                    );
                    if let Some(events) = EVENTS.get() {
                        let _ = events.try_send(action);
                    }
                }
                // Consume the chord before Windows' Snipping Tool sees it.
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, message, event)
}
