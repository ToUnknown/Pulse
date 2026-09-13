use image::RgbaImage;
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
pub fn recognize(mut image: RgbaImage) -> Result<String, String> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
        .map_err(|_| "Windows text recognition could not start. Try again.")?;
    let _runtime = Runtime;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .or_else(|_| {
            let languages = OcrEngine::AvailableRecognizerLanguages()?;
            OcrEngine::TryCreateFromLanguage(&languages.GetAt(0)?)
        })
        .map_err(|_| "Install an OCR language in Windows Settings > Time & language > Language & region, then try again. You can also use Advanced.")?;
    let limit = OcrEngine::MaxImageDimension().map_err(ocr_error)?;
    if limit == 0 {
        return Err("Windows text recognition is unavailable.".into());
    }
    if image.width().max(image.height()) > limit {
        let scale = f64::from(limit) / f64::from(image.width().max(image.height()));
        image = image::imageops::resize(
            &image,
            (f64::from(image.width()) * scale).floor().max(1.0) as u32,
            (f64::from(image.height()) * scale).floor().max(1.0) as u32,
            image::imageops::FilterType::Lanczos3,
        );
    }
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

fn ocr_error(_: windows::core::Error) -> String {
    "Windows could not read this selection. Try again or switch to Advanced.".into()
}
