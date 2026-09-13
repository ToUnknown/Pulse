use super::{pixels::DesktopFrame, protocol::Crop};
use image::RgbaImage;
use std::{mem::size_of, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, GetMonitorInfoW, MonitorFromPoint, ReleaseDC, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, MONITORINFO,
        MONITOR_DEFAULTTONEAREST, SRCCOPY,
    },
    UI::{
        HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
        WindowsAndMessaging::GetCursorPos,
    },
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

struct GdiCapture {
    screen: HDC,
    memory: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}
impl Drop for GdiCapture {
    fn drop(&mut self) {
        // All handles belong to this capture and are released on the same thread.
        unsafe {
            if !self.previous.is_null() {
                SelectObject(self.memory, self.previous);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            if !self.memory.is_null() {
                DeleteDC(self.memory);
            }
            if !self.screen.is_null() {
                ReleaseDC(null_mut(), self.screen);
            }
        }
    }
}

pub fn monitor_at_pointer() -> Result<Monitor, String> {
    // Use physical desktop coordinates even when monitors have different DPI.
    unsafe {
        let previous_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let result = monitor_info(None);
        if !previous_dpi.is_null() {
            SetThreadDpiAwarenessContext(previous_dpi);
        }
        result
    }
}

unsafe fn monitor_info(point: Option<POINT>) -> Result<Monitor, String> {
    let point = if let Some(point) = point {
        point
    } else {
        let mut point: POINT = std::mem::zeroed();
        if GetCursorPos(&mut point) == 0 {
            return Err("Could not locate the pointer.".into());
        }
        point
    };
    let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = std::mem::zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(monitor, &mut info) == 0 {
        return Err("Could not read this screen.".into());
    }
    let rect = info.rcMonitor;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 || i64::from(width) * i64::from(height) > 40_000_000 {
        return Err(
            "This screen is too large to capture. Reduce its resolution and try again.".into(),
        );
    }
    Ok(Monitor {
        x: rect.left,
        y: rect.top,
        width: width as u32,
        height: height as u32,
    })
}

pub fn snapshot(monitor: Monitor) -> Result<DesktopFrame, String> {
    unsafe {
        let previous_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let result = snapshot_inner(monitor, None);
        if !previous_dpi.is_null() {
            SetThreadDpiAwarenessContext(previous_dpi);
        }
        result
    }
}

pub fn selection(monitor: Monitor, crop: Crop) -> Result<RgbaImage, String> {
    crop.validate(monitor.width, monitor.height)?;
    unsafe {
        let previous_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let result = snapshot_inner(monitor, Some(crop));
        if !previous_dpi.is_null() {
            SetThreadDpiAwarenessContext(previous_dpi);
        }
        result?.crop(Crop {
            x: 0,
            y: 0,
            width: crop.width,
            height: crop.height,
        })
    }
}

unsafe fn snapshot_inner(monitor: Monitor, crop: Option<Crop>) -> Result<DesktopFrame, String> {
    let current = monitor_info(Some(POINT {
        x: monitor.x + monitor.width as i32 / 2,
        y: monitor.y + monitor.height as i32 / 2,
    }))?;
    if current != monitor {
        return Err("This screen changed. Start a new selection.".into());
    }
    let crop = crop.unwrap_or(Crop {
        x: 0,
        y: 0,
        width: monitor.width,
        height: monitor.height,
    });
    let width = crop.width as i32;
    let height = crop.height as i32;
    let mut gdi = GdiCapture {
        screen: GetDC(null_mut()),
        memory: null_mut(),
        bitmap: null_mut(),
        previous: null_mut(),
    };
    if gdi.screen.is_null() {
        return Err("Screen capture is unavailable.".into());
    }
    gdi.memory = CreateCompatibleDC(gdi.screen);
    gdi.bitmap = CreateCompatibleBitmap(gdi.screen, width, height);
    if gdi.memory.is_null() || gdi.bitmap.is_null() {
        return Err("Could not allocate the screen capture.".into());
    }
    gdi.previous = SelectObject(gdi.memory, gdi.bitmap);
    if gdi.previous.is_null() || gdi.previous as isize == -1 {
        gdi.previous = null_mut();
        return Err("Could not prepare the screen capture.".into());
    }
    if BitBlt(
        gdi.memory,
        0,
        0,
        width,
        height,
        gdi.screen,
        monitor.x + crop.x as i32,
        monitor.y + crop.y as i32,
        SRCCOPY | CAPTUREBLT,
    ) == 0
    {
        return Err("Windows could not capture this screen.".into());
    }
    // GetDIBits requires that the bitmap is not selected into a DC.
    SelectObject(gdi.memory, gdi.previous);
    gdi.previous = null_mut();
    let mut bitmap_info: BITMAPINFO = std::mem::zeroed();
    bitmap_info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..std::mem::zeroed()
    };
    let mut pixels = vec![0; width as usize * height as usize * 4];
    if GetDIBits(
        gdi.memory,
        gdi.bitmap,
        0,
        height as u32,
        pixels.as_mut_ptr().cast(),
        &mut bitmap_info,
        DIB_RGB_COLORS,
    ) != height
    {
        return Err("Could not read the captured pixels.".into());
    }
    DesktopFrame::new(width as u32, height as u32, pixels)
}
