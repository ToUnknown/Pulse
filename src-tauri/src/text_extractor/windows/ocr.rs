use super::ocr_recognizer::Recognizer;
use image::{Rgba, RgbaImage};
use ort::session::{
    builder::{GraphOptimizationLevel, SessionBuilder},
    Session,
};
use paddle_ocr_rs::{
    base_net::BaseNet, db_net::DbNet, ocr_result::TextBox, ocr_utils::OcrUtils,
    scale_param::ScaleParam,
};
use std::path::Path;
use tokio_util::sync::CancellationToken;

/// Reused CPU sessions. Creating these loads weights, but never runs inference.
pub struct Ocr {
    detector: DbNet,
    recognizer: Recognizer,
}

fn session_options(builder: SessionBuilder) -> Result<SessionBuilder, ort::Error> {
    let threads = std::thread::available_parallelism().map_or(2, |count| count.get().min(4));
    builder
        .with_optimization_level(GraphOptimizationLevel::Level2)?
        .with_intra_threads(threads)?
        .with_inter_threads(1)?
        .with_parallel_execution(false)?
        .with_intra_op_spinning(false)?
        .with_inter_op_spinning(false)
}

impl Ocr {
    pub fn load(detector_path: &Path, recognizer_path: &Path) -> Result<Self, String> {
        let mut detector = DbNet::new();
        detector
            .init_model(
                detector_path
                    .to_str()
                    .ok_or("Could not read the model folder.")?,
                1,
                Some(session_options),
            )
            .map_err(|error| format!("Could not load the text detector: {error}"))?;
        let session = Session::builder()
            .and_then(session_options)
            .and_then(|builder| builder.commit_from_file(recognizer_path))
            .map_err(|error| format!("Could not load the text recognizer: {error}"))?;
        let recognizer = Recognizer::new(session)?;
        Ok(Self {
            detector,
            recognizer,
        })
    }

    pub fn recognize(
        &mut self,
        image: RgbaImage,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        if image.width() == 0 || image.height() == 0 {
            return Err("Select a larger area inside this screen.".into());
        }
        if cancel.is_cancelled() {
            return Err("Capture cancelled.".into());
        }
        let image = prepare_image(image, 4096);
        let mut image = image::DynamicImage::ImageRgba8(image).into_rgb8();
        // RapidOCR's ONNX models expect OpenCV's BGR channel order.
        for pixel in image.pixels_mut() {
            pixel.0.swap(0, 2);
        }
        let scale =
            ScaleParam::get_scale_param(&image, image.width().max(image.height()).min(1536));
        let boxes = self
            .detector
            .get_text_boxes(&image, &scale, 0.5, 0.3, 1.6)
            .map_err(|error| format!("Could not find text in this selection: {error}"))?;
        // Form rows first, then order fragments left to right. A fuzzy sort comparator
        // would be non-transitive for boxes with slightly different baselines.
        let mut boxes: Vec<_> = boxes
            .into_iter()
            .filter(|b| {
                if b.points.len() != 4 {
                    return false;
                }
                let (left, top, right, bottom) = bounds(b);
                right > left + 1 && bottom > top + 1
            })
            .collect();
        boxes.sort_by_key(|b| {
            let (x, y, _, _) = bounds(b);
            (y, x)
        });
        let mut rows: Vec<Vec<TextBox>> = Vec::new();
        for text_box in boxes {
            let (_, top, _, bottom) = bounds(&text_box);
            if let Some(row) = rows.last_mut().filter(|row| {
                let (_, row_top, _, row_bottom) = bounds(&row[0]);
                let overlap = bottom.min(row_bottom).saturating_sub(top.max(row_top));
                overlap * 2 >= (bottom - top).min(row_bottom - row_top)
            }) {
                row.push(text_box);
            } else {
                rows.push(vec![text_box]);
            }
        }
        let mut lines = Vec::new();
        for mut row in rows {
            row.sort_by_key(|b| bounds(b).0);
            let mut fragments = Vec::new();
            for text_box in row {
                if cancel.is_cancelled() {
                    return Err("Capture cancelled.".into());
                }
                let crop = OcrUtils::get_rotate_crop_image(&image, &text_box.points);
                if crop.width() == 0 || crop.height() == 0 {
                    continue;
                }
                // Bound very long, thin selections before recognition allocates tensors.
                let crop = if u64::from(crop.width()) * 48 > u64::from(crop.height()) * 4096 {
                    image::imageops::resize(&crop, 4096, 48, image::imageops::FilterType::Triangle)
                } else {
                    crop
                };
                let line = self.recognizer.recognize(&crop)?;
                let text = line.text.trim();
                if !text.is_empty() && line.text_score.is_finite() && line.text_score >= 0.5 {
                    fragments.push(text.to_string());
                }
            }
            if !fragments.is_empty() {
                lines.push(fragments.join(" "));
            }
        }
        Ok(lines.join("\n"))
    }
}

fn bounds(text_box: &TextBox) -> (u32, u32, u32, u32) {
    text_box.points.iter().fold(
        (u32::MAX, u32::MAX, 0, 0),
        |(left, top, right, bottom), p| {
            (left.min(p.x), top.min(p.y), right.max(p.x), bottom.max(p.y))
        },
    )
}

fn prepare_image(image: RgbaImage, limit: u32) -> RgbaImage {
    // Tight crops can leave too little context around characters. Add space using
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
}
