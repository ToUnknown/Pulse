//! Explicit, test-only bridge for a simulated UI using the production OCR/translation engines.
//! No desktop capture, clipboard writes, credentials, or app settings are accessed.
use super::{google_translate, platform::LocalOcr};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use tokio_util::sync::CancellationToken;

#[test]
#[ignore = "Opt-in preview worker: runs Apple Vision and sends supplied text to Google"]
fn live_preview_worker() {
    assert_eq!(std::env::var("PULSE_LIVE_PREVIEW").as_deref(), Ok("1"));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let ocr = LocalOcr::new(std::path::PathBuf::new());
    ocr.set_enabled(true);
    println!("PULSE_PREVIEW_READY");
    std::io::stdout().flush().unwrap();
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let started = std::time::Instant::now();
        let request: Value = serde_json::from_str(&line).unwrap();
        let result = match request["command"].as_str() {
            Some("ocr") => (|| {
                let encoded = request["image"].as_str().ok_or("Missing image")?;
                if encoded.len() > 8_000_000 {
                    return Err("Image is too large".into());
                }
                let bytes = STANDARD
                    .decode(encoded)
                    .map_err(|_| "Invalid image encoding")?;
                let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                    .map_err(|_| "Invalid PNG")?
                    .to_rgba8();
                ocr.recognize(image, &CancellationToken::new())
            })(),
            Some("translate") => runtime.block_on(google_translate::request(
                request["text"].as_str().unwrap_or_default(),
                request["language"].as_str().unwrap_or_default(),
                CancellationToken::new(),
            )),
            _ => Err("Unsupported preview command".into()),
        };
        let output = match result {
            Ok(text) => {
                json!({"id":request["id"],"text":text,"elapsedMs":started.elapsed().as_millis()})
            }
            Err(error) => {
                json!({"id":request["id"],"error":error,"elapsedMs":started.elapsed().as_millis()})
            }
        };
        println!("PULSE_PREVIEW_RESULT {output}");
        std::io::stdout().flush().unwrap();
    }
    ocr.set_enabled(false);
}
