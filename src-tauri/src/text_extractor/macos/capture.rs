use super::native;
use crate::text_extractor::{pixels::DesktopFrame, protocol::Crop};
use image::RgbaImage;

/// Must match PulseMonitor in native.h. Origins are desktop points, dimensions
/// are backing pixels. Native placement avoids mixed-DPI global-coordinate bugs.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Monitor {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub width: u32,
    pub height: u32,
    pub display_id: u32,
}

pub fn snapshot(monitor: Monitor) -> Result<DesktopFrame, String> {
    let mut pixels = std::ptr::null_mut();
    native::checked(|error| unsafe { native::pulse_capture_screen(monitor, &mut pixels, error) })?;
    if pixels.is_null() {
        return Err("macOS returned an empty screen capture.".into());
    }
    let bytes = unsafe {
        let bytes = std::slice::from_raw_parts(
            pixels,
            monitor.width as usize * monitor.height as usize * 4,
        )
        .to_vec();
        native::pulse_native_free(pixels.cast());
        bytes
    };
    DesktopFrame::new(monitor.width, monitor.height, bytes)
}

pub fn selection(monitor: Monitor, crop: Crop) -> Result<RgbaImage, String> {
    crop.validate(monitor.width, monitor.height)?;
    snapshot(monitor)?.crop(crop)
}
