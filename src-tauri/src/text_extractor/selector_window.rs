use tauri::WebviewWindow;
use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError, HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, STYLESTRUCT, WM_NCDESTROY,
            WM_STYLECHANGING, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
        },
    },
};

// IDs are scoped to the subclass procedure and HWND; no per-window allocation is needed.
const SUBCLASS_ID: usize = 1;

pub async fn configure(window: &WebviewWindow) -> Result<(), String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let target = window.clone();
    // Comctl32 subclasses must be installed on the thread that owns the HWND.
    window
        .run_on_main_thread(move || {
            let result = target
                .hwnd()
                .map_err(|_| "Could not configure the selector window.".to_string())
                .and_then(|hwnd| unsafe { install(hwnd.0) });
            let _ = send.send(result);
        })
        .map_err(|_| "Could not configure the selector window.".to_string())?;
    receive
        .await
        .map_err(|_| "Selector window configuration stopped.".to_string())?
}

unsafe fn install(hwnd: HWND) -> Result<(), String> {
    if SetWindowSubclass(hwnd, Some(selector_proc), SUBCLASS_ID, 0) == 0 {
        return Err("Could not configure the selector window.".into());
    }
    let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    SetLastError(0);
    let previous = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, tool_style(style) as isize);
    if previous == 0 && GetLastError() != 0 {
        RemoveWindowSubclass(hwnd, Some(selector_proc), SUBCLASS_ID);
        return Err("Could not configure the selector window.".into());
    }
    Ok(())
}

fn tool_style(style: u32) -> u32 {
    (style & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW
}

unsafe extern "system" fn selector_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass_id: usize,
    _reference: usize,
) -> LRESULT {
    if message == WM_STYLECHANGING && wparam as i32 == GWL_EXSTYLE && lparam != 0 {
        // Tao rewrites extended styles when showing/hiding the window. Preserve the
        // floating-tool classification before it is visible, so Chromium does not
        // mistake our transparent selector for an opaque window and stop video below it.
        let styles = &mut *(lparam as *mut STYLESTRUCT);
        styles.styleNew = tool_style(styles.styleNew);
    } else if message == WM_NCDESTROY {
        RemoveWindowSubclass(hwnd, Some(selector_proc), subclass_id);
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::null_mut;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WS_EX_ACCEPTFILES, WS_EX_NOACTIVATE, WS_OVERLAPPED,
    };

    #[test]
    fn selector_remains_a_tool_window_when_runtime_rewrites_its_styles() {
        unsafe {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let hwnd = CreateWindowExW(
                WS_EX_APPWINDOW,
                class.as_ptr(),
                null_mut(),
                WS_OVERLAPPED,
                0,
                0,
                100,
                100,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            );
            assert!(!hwnd.is_null());
            let configured = install(hwnd);
            let initial = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            // Exercise the actual WM_STYLECHANGING path used by Tao's visibility updates.
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                (WS_EX_APPWINDOW | WS_EX_ACCEPTFILES | WS_EX_NOACTIVATE) as isize,
            );
            let rewritten = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let destroyed = DestroyWindow(hwnd);
            assert_eq!(configured, Ok(()));
            assert_eq!(initial & WS_EX_TOOLWINDOW, WS_EX_TOOLWINDOW);
            assert_eq!(initial & WS_EX_APPWINDOW, 0);
            assert_eq!(rewritten & WS_EX_TOOLWINDOW, WS_EX_TOOLWINDOW);
            assert_eq!(rewritten & WS_EX_APPWINDOW, 0);
            assert_eq!(rewritten & WS_EX_NOACTIVATE, WS_EX_NOACTIVATE);
            assert_eq!(rewritten & WS_EX_ACCEPTFILES, WS_EX_ACCEPTFILES);
            assert_ne!(destroyed, 0);
        }
    }
}
