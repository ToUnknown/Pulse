use image::{Rgba, RgbaImage};
use windows::{
    Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap},
    Media::Ocr::OcrEngine,
    Storage::Streams::DataWriter,
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};

struct Runtime;
impl Drop for Runtime {
    fn drop(&mut self) {
        // Balanced on the same blocking worker that successfully initialized WinRT.
        unsafe { RoUninitialize() };
    }
}

/// Uses only installed Windows OCR languages. The selection never leaves this PC.
pub fn recognize(image: RgbaImage) -> Result<String, String> {
    if image.width() == 0 || image.height() == 0 {
        return Err("Select a larger area inside this screen.".into());
    }
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
        .map_err(|_| "Windows text recognition could not start. Try again.")?;
    let _runtime = Runtime;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .or_else(|_| {
            let languages = OcrEngine::AvailableRecognizerLanguages()?;
            OcrEngine::TryCreateFromLanguage(&languages.GetAt(0)?)
        })
        .map_err(|_| "Install an OCR language in Windows Settings > Time & language > Language & region, then try again.")?;
    let limit = OcrEngine::MaxImageDimension().map_err(ocr_error)?;
    if limit == 0 {
        return Err("Windows text recognition is unavailable.".into());
    }
    let mut image = prepare_image(image, limit);
    let (width, height) = image.dimensions();
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
        pixel[3] = 255;
    }
    let pixels = image.into_raw();
    let writer = DataWriter::new().map_err(ocr_error)?;
    writer.WriteBytes(&pixels).map_err(ocr_error)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &writer.DetachBuffer().map_err(ocr_error)?,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )
    .map_err(ocr_error)?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(ocr_error)?
        .get()
        .map_err(ocr_error)?;
    let lines = result.Lines().map_err(ocr_error)?;
    let mut text = Vec::new();
    for index in 0..lines.Size().map_err(ocr_error)? {
        text.push(
            lines
                .GetAt(index)
                .map_err(ocr_error)?
                .Text()
                .map_err(ocr_error)?
                .to_string(),
        );
    }
    Ok(text.join("\n"))
}

fn prepare_image(image: RgbaImage, limit: u32) -> RgbaImage {
    // Tight crops can make Windows OCR return no lines at all. Add space using
    // the crop's own background, without capturing text outside the selection.
    let padding = 16.min((limit - 1) / 2);
    let available = limit - padding * 2;
    let background = border_background(&image);
    // Single-line UI text needs enough pixels for reliable character detection.
    // Bound enlargement and reserve room for padding within the engine's limit.
    let preferred_scale = (32.0 / f64::from(image.height())).clamp(1.0, 3.0);
    let scale =
        preferred_scale.min(f64::from(available) / f64::from(image.width().max(image.height())));
    let width = (f64::from(image.width()) * scale).floor().max(1.0) as u32;
    let height = (f64::from(image.height()) * scale).floor().max(1.0) as u32;
    let image = if image.dimensions() == (width, height) {
        image
    } else {
        image::imageops::resize(&image, width, height, image::imageops::FilterType::Lanczos3)
    };
    let mut padded = RgbaImage::from_pixel(width + padding * 2, height + padding * 2, background);
    image::imageops::replace(&mut padded, &image, i64::from(padding), i64::from(padding));
    padded
}

fn border_background(image: &RgbaImage) -> Rgba<u8> {
    // Median edge color tolerates glyphs touching the crop boundary and handles
    // both light and dark UI without adding a contrasting frame around the text.
    let mut channels = [Vec::new(), Vec::new(), Vec::new()];
    let mut sample = |pixel: &Rgba<u8>| {
        for channel in 0..3 {
            channels[channel].push(pixel[channel]);
        }
    };
    for x in 0..image.width() {
        sample(image.get_pixel(x, 0));
        sample(image.get_pixel(x, image.height() - 1));
    }
    for y in 0..image.height() {
        sample(image.get_pixel(0, y));
        sample(image.get_pixel(image.width() - 1, y));
    }
    let mut background = Rgba([0, 0, 0, 255]);
    for (index, values) in channels.iter_mut().enumerate() {
        let middle = values.len() / 2;
        background[index] = *values.select_nth_unstable(middle).1;
    }
    background
}

fn ocr_error(_: windows::core::Error) -> String {
    "Windows could not read this selection. Try again.".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_matches_light_and_dark_backgrounds_despite_edge_glyphs() {
        for background in [Rgba([255, 255, 255, 255]), Rgba([30, 30, 30, 255])] {
            let mut image = RgbaImage::from_pixel(120, 12, background);
            image.put_pixel(0, 0, Rgba([128, 128, 128, 255]));
            image.put_pixel(119, 11, Rgba([128, 128, 128, 255]));
            let padded = prepare_image(image, 2600);
            assert!(padded.height() > 12 + 32);
            assert_eq!(*padded.get_pixel(0, 0), background);
            assert_eq!(
                *padded.get_pixel(padded.width() - 1, padded.height() - 1),
                background
            );
        }
    }

    #[test]
    fn blank_crops_remain_blank_and_prepared_images_respect_engine_limits() {
        for (width, height, limit) in [(4, 4, 2600), (3000, 12, 2600), (12, 3000, 2600), (4, 4, 1)]
        {
            let background = Rgba([42, 42, 42, 255]);
            let padded = prepare_image(RgbaImage::from_pixel(width, height, background), limit);
            assert!(padded.width() <= limit && padded.height() <= limit);
            assert!(padded.width() > 0 && padded.height() > 0);
            assert!(padded.pixels().all(|pixel| *pixel == background));
        }
    }

    #[test]
    #[ignore = "requires an installed Windows OCR language that recognizes English"]
    fn native_ocr_reads_tightly_cropped_small_lines() {
        for fixture in [
            include_bytes!("testdata/small-line-light.png").as_slice(),
            include_bytes!("testdata/small-line-dark.png").as_slice(),
        ] {
            let image = image::load_from_memory(fixture).unwrap().into_rgba8();
            assert_eq!(recognize(image).unwrap(), "Small text should still copy");
        }
    }
}
