use image::RgbImage;
use ort::{inputs, session::Session, value::Tensor};
use paddle_ocr_rs::{ocr_result::TextLine, ocr_utils::OcrUtils};

/// Uses the pinned model's own character indices, including its final space class.
/// The upstream Rust wrapper treats a trailing dictionary newline as an extra
/// character, shifting space one position beyond the model's output vocabulary.
pub struct Recognizer {
    session: Session,
    input_name: String,
    keys: Vec<String>,
}

impl Recognizer {
    pub fn new(session: Session) -> Result<Self, String> {
        let characters = {
            let metadata = session.metadata().map_err(recognition_error)?;
            metadata
                .custom("character")
                .map_err(recognition_error)?
                .ok_or("The offline recognizer has no character dictionary.")?
        };
        let keys = character_keys(&characters)?;
        let output_shape = session
            .outputs
            .first()
            .and_then(|output| output.output_type.tensor_shape())
            .ok_or("The offline recognizer has no text output.")?;
        if output_shape.len() != 3 || output_shape[2] != keys.len() as i64 {
            return Err(
                "The offline recognizer's dictionary does not match its output vocabulary.".into(),
            );
        }
        let input_name = session
            .inputs
            .first()
            .ok_or("The offline recognizer has no image input.")?
            .name
            .clone();
        Ok(Self {
            session,
            input_name,
            keys,
        })
    }

    pub fn recognize(&mut self, image: &RgbImage) -> Result<TextLine, String> {
        if image.width() == 0 || image.height() == 0 {
            return Err("The selected text line is empty.".into());
        }
        let width =
            (u64::from(image.width()) * 48 / u64::from(image.height())).clamp(1, 4096) as u32;
        let resized =
            image::imageops::resize(image, width, 48, image::imageops::FilterType::Triangle);
        let normalized =
            OcrUtils::substract_mean_normalize(&resized, &[127.5; 3], &[1.0 / 127.5; 3]);
        let tensor = Tensor::from_array(normalized).map_err(recognition_error)?;
        let outputs = self
            .session
            .run(inputs![self.input_name.as_str() => tensor])
            .map_err(recognition_error)?;
        let output = outputs
            .iter()
            .next()
            .ok_or("The offline recognizer returned no text output.")?
            .1;
        let (shape, scores) = output
            .try_extract_tensor::<f32>()
            .map_err(recognition_error)?;
        if shape.len() != 3
            || shape[0] != 1
            || shape[1] < 0
            || shape[2] != self.keys.len() as i64
            || (shape[1] as usize).checked_mul(self.keys.len()) != Some(scores.len())
        {
            return Err("The offline recognizer returned an unexpected text shape.".into());
        }
        decode_scores(scores, &self.keys)
    }
}

fn character_keys(metadata: &str) -> Result<Vec<String>, String> {
    // lines() consumes line terminators without inventing a final empty entry.
    // Do not trim each character: a literal whitespace character must survive.
    let mut keys = vec![String::new()]; // CTC blank class, always index zero.
    for character in metadata.lines() {
        if character.is_empty() {
            return Err("The offline character dictionary contains an empty entry.".into());
        }
        keys.push(character.to_string());
    }
    if keys.len() == 1 {
        return Err("The offline character dictionary is empty.".into());
    }
    keys.push(" ".into()); // RapidOCR appends space after the character alphabet.
    Ok(keys)
}

fn decode_scores(scores: &[f32], keys: &[String]) -> Result<TextLine, String> {
    if keys.len() < 2
        || !scores.len().is_multiple_of(keys.len())
        || scores.iter().any(|score| !score.is_finite())
    {
        return Err("The offline recognizer returned invalid character scores.".into());
    }
    let mut text = String::new();
    let mut previous = 0;
    let mut confidence = 0.0;
    let mut count = 0;
    for frame in scores.chunks_exact(keys.len()) {
        let (index, score) = frame
            .iter()
            .copied()
            .enumerate()
            .fold(
                (0, f32::MIN),
                |best, next| {
                    if next.1 > best.1 {
                        next
                    } else {
                        best
                    }
                },
            );
        if index != 0 && index != previous {
            text.push_str(&keys[index]);
            confidence += score;
            count += 1;
        }
        // A blank separates repeated letters; space is a real character, not blank.
        previous = index;
    }
    Ok(TextLine {
        text,
        text_score: if count == 0 {
            0.0
        } else {
            confidence / count as f32
        },
    })
}

fn recognition_error(error: ort::Error) -> String {
    format!("Could not read this selection: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scores(indices: &[usize], classes: usize) -> Vec<f32> {
        indices
            .iter()
            .flat_map(|&index| {
                let mut frame = vec![0.0; classes];
                frame[index] = 1.0;
                frame
            })
            .collect()
    }

    #[test]
    fn trailing_dictionary_newline_does_not_consume_the_space_class() {
        for metadata in ["a\nb", "a\nb\n", "a\r\nb\r\n"] {
            let keys = character_keys(metadata).unwrap();
            assert_eq!(keys, ["", "a", "b", " "]);
            let result = decode_scores(&scores(&[1, 3, 2], 4), &keys).unwrap();
            assert_eq!(result.text, "a b");
        }
    }

    #[test]
    fn ctc_preserves_spaces_between_mixed_script_words_and_repeated_letters() {
        let keys = character_keys("o\nk\nт\nа\nк\n").unwrap();
        // Adjacent duplicate frames collapse; a blank allows a repeated letter.
        let result = decode_scores(&scores(&[1, 1, 0, 1, 2, 6, 6, 3, 4, 5], 7), &keys).unwrap();
        assert_eq!(result.text, "ook так");
        assert_eq!(result.text_score, 1.0);
        assert_eq!(decode_scores(&scores(&[0, 0], 7), &keys).unwrap().text, "");
    }

    #[test]
    fn malformed_dictionaries_and_score_shapes_are_rejected() {
        assert!(character_keys("").is_err());
        assert!(character_keys("a\n\nb\n").is_err());
        let keys = character_keys("a\nb\n").unwrap();
        assert!(decode_scores(&[1.0, 0.0, 0.0], &keys).is_err());
        assert!(decode_scores(&[f32::NAN, 0.0, 0.0, 0.0], &keys).is_err());
    }
}
