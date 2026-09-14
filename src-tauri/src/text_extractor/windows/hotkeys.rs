use super::{
    session::{self, CaptureMode},
    shortcut_keys::{Action, Bindings, Decision, Key, ShortcutKeys},
    shortcut_worker::{self, Control},
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
use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
            KEYEVENTF_KEYUP, VK_NONAME,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
            UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
        },
    },
};

static FLAGS: AtomicU8 = AtomicU8::new(0);
const MENU_MASK_TAG: usize = 0x5055_4C53;
static EVENTS: OnceLock<SyncSender<Action>> = OnceLock::new();
thread_local! { static KEYS: RefCell<ShortcutKeys> = RefCell::new(ShortcutKeys::default()); }

pub fn configure(enabled: bool, bindings: Bindings) {
    shortcut_worker::send(Control::Configure { enabled, bindings });
}
pub fn capture_closed() {
    shortcut_worker::send(Control::Resynchronize);
}
pub fn recording(value: bool) {
    shortcut_worker::send(Control::Recording(value));
}
pub fn available() -> bool {
    shortcut_worker::available()
}
pub fn install(app: &tauri::AppHandle) -> Result<(), String> {
    shortcut_worker::install(app)
}

pub(super) fn apply(control: Control) {
    match control {
        Control::Configure { enabled, bindings } => {
            let encode = |action| match action {
                Some(Action::Editor) => 1,
                Some(Action::QuickCopy) => 2,
                _ => 0,
            };
            let routes = (encode(bindings.plain) << 4) | (encode(bindings.control) << 6);
            FLAGS
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |flags| {
                    Some((flags & 12) | u8::from(enabled) | routes)
                })
                .ok();
        }
        Control::Recording(true) => {
            FLAGS.fetch_or(4, Ordering::AcqRel);
        }
        Control::Recording(false) => {
            FLAGS.fetch_and(!4, Ordering::AcqRel);
        }
        Control::Resynchronize => {
            FLAGS.fetch_or(8, Ordering::AcqRel);
        }
    }
}

pub(super) fn dispatch(app: &tauri::AppHandle, action: Action) {
    match action {
        Action::RecordQuick | Action::RecordEditor => {
            let shortcut = if action == Action::RecordQuick {
                "Control+Super+Shift+KeyT"
            } else {
                "Super+Shift+KeyT"
            };
            session::report_recorded_shortcut(app, shortcut);
        }
        Action::QuickCopy | Action::Editor => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let mode = if action == Action::QuickCopy {
                    CaptureMode::Quick
                } else {
                    CaptureMode::Editor
                };
                if let Err(error) = session::start(&app, mode).await {
                    session::report_error(&app, error);
                }
            });
        }
    }
}

// Run the unchanged native interception in a process with no WebView2 windows.
// The Settings webview otherwise bypasses our hook when it has keyboard focus.
pub(super) fn run_worker() -> Result<(), String> {
    let (events, receive) = sync_channel::<Action>(8);
    EVENTS
        .set(events)
        .map_err(|_| "Shortcuts are already installed.")?;
    std::thread::Builder::new()
        .name("pulse-shortcut-input".into())
        .spawn(shortcut_worker::read_controls)
        .map_err(|_| "Could not receive shortcut settings.")?;
    std::thread::Builder::new()
        .name("pulse-shortcut-output".into())
        .spawn(move || {
            while let Ok(action) = receive.recv() {
                mask_windows_menu();
                if shortcut_worker::emit(action).is_err() {
                    std::process::exit(0);
                }
            }
        })
        .map_err(|_| "Could not send shortcut actions.")?;
    unsafe {
        let hook = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard),
            GetModuleHandleW(null_mut()),
            0,
        );
        if hook.is_null() {
            return Err("Windows could not install the Text Extractor shortcuts.".into());
        }
        let ready = shortcut_worker::ready();
        if ready.is_ok() {
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        UnhookWindowsHookEx(hook);
        ready.map_err(|_| "Could not connect the shortcut helper.".into())
    }
}

unsafe extern "system" fn keyboard(code: i32, message: WPARAM, event: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = &*(event as *const KBDLLHOOKSTRUCT);
        // Remapping and accessibility software inject ordinary shortcut keys.
        // Ignore only our own Windows-menu mask, otherwise those shortcuts
        // escape to Snipping Tool instead of reaching Pulse.
        // Match both fields: a modifier/T release must never be discarded
        // solely because its injection tag matches the menu mask.
        if event.vkCode != u32::from(VK_NONAME) || event.dwExtraInfo != MENU_MASK_TAG {
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
            let flags = FLAGS.fetch_and(!8, Ordering::AcqRel);
            let down = message as u32 == WM_KEYDOWN || message as u32 == WM_SYSKEYDOWN;
            let decision = KEYS.with(|keys| {
                let mut keys = keys.borrow_mut();
                if flags & 8 != 0 {
                    // Recover after capture closes if another input hook or
                    // remapper consumed a release before it reached this hook.
                    keys.resynchronize(
                        [
                            (0x5B, Key::WinLeft),
                            (0x5C, Key::WinRight),
                            (0xA0, Key::ShiftLeft),
                            (0xA1, Key::ShiftRight),
                            (0xA2, Key::ControlLeft),
                            (0xA3, Key::ControlRight),
                            (0xA4, Key::AltLeft),
                            (0xA5, Key::AltRight),
                            (0x54, Key::T),
                        ]
                        .into_iter()
                        .filter_map(|(vk, key)| (GetAsyncKeyState(vk) < 0).then_some(key)),
                    );
                }
                let decode = |value| match value & 3 {
                    1 => Some(Action::Editor),
                    2 => Some(Action::QuickCopy),
                    _ => None,
                };
                let bindings = Bindings {
                    plain: decode(flags >> 4),
                    control: decode(flags >> 6),
                };
                keys.update(key, down, flags & 1 != 0, bindings, flags & 4 != 0)
            });
            if let Decision::Suppress(action) = decision {
                if let Some(action) = action {
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

// SendInput must run outside WH_KEYBOARD_LL. It can wait for input dispatch,
// delaying the hook's return and causing Windows to discard the hook on timeout.
fn mask_windows_menu() {
    unsafe {
        let inputs = [false, true].map(|up| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_NONAME,
                    dwExtraInfo: MENU_MASK_TAG,
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
    }
}
