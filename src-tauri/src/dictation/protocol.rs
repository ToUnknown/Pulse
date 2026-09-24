// Schema: https://developers.openai.com/api/docs/guides/realtime-transcription
use serde_json::{json, Value};

pub fn configuration(model: &str) -> Value {
    let transcription = if model == "gpt-live-transcribe" {
        json!({"model":model,"delay":"high"})
    } else {
        json!({"model":model})
    };
    json!({"type":"session.update","session":{"type":"transcription","audio":{"input":{
        "format":{"type":"audio/pcm","rate":24000},
        "transcription":transcription,
        "turn_detection":null
    }}}})
}
pub fn check_error(event: &Value, model: &str) -> Result<(), String> {
    if matches!(
        event["type"].as_str(),
        Some("error" | "conversation.item.input_audio_transcription.failed")
    ) {
        // Don't echo server payloads: they may contain user audio or credentials.
        return Err(match event["error"]["code"].as_str().unwrap_or("") {
            "invalid_api_key" => "The saved OpenAI API key is invalid.",
            "insufficient_quota" | "rate_limit_exceeded" => {
                "OpenAI's usage or billing limit was reached."
            }
            "model_not_found" if model == "gpt-live-transcribe" => {
                "This API key cannot access GPT Live Transcribe."
            }
            "model_not_found" => "This API key cannot access GPT Transcribe.",
            _ => "OpenAI could not transcribe this recording. Check model access and try again.",
        }
        .into());
    }
    Ok(())
}
#[derive(Default)]
pub struct Transcript {
    pub text: String,
    item: Option<String>,
}
impl Transcript {
    pub fn accept(&mut self, event: &Value) -> Result<Option<bool>, String> {
        let kind = event["type"].as_str().unwrap_or("");
        let done = kind == "conversation.item.input_audio_transcription.completed";
        if !done && kind != "conversation.item.input_audio_transcription.delta" {
            return Ok(None);
        }
        let item = event["item_id"]
            .as_str()
            .ok_or("OpenAI returned a transcript without an item ID.")?;
        if self
            .item
            .as_deref()
            .is_some_and(|previous| previous != item)
        {
            return Err("OpenAI returned an unexpected transcription turn.".into());
        }
        self.item = Some(item.to_owned());
        let content = event[if done { "transcript" } else { "delta" }]
            .as_str()
            .ok_or("OpenAI returned an incomplete transcript event.")?;
        // Some initial deltas contain only whitespace. Ignore it until the
        // first spoken text, without trimming later word separators or lines.
        if done {
            self.text = content.trim_start().to_owned();
        } else if self.text.is_empty() {
            self.text.push_str(content.trim_start());
        } else {
            self.text.push_str(content);
        }
        if self.text.len() > 100_000 {
            return Err("This dictation is too long. Try a shorter recording.".into());
        }
        Ok(Some(done))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn final_replaces_partials_and_foreign_turns_cannot_mix() {
        let mut transcript = Transcript::default();
        for delta in ["Hello", ", wor"] {
            assert_eq!(transcript.accept(&json!({"type":"conversation.item.input_audio_transcription.delta","item_id":"one","delta":delta})).unwrap(),Some(false));
        }
        assert_eq!(transcript.text, "Hello, wor");
        assert_eq!(transcript.accept(&json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"one","transcript":"Hello, world!"})).unwrap(),Some(true));
        assert_eq!(transcript.text, "Hello, world!");
        assert!(transcript.accept(&json!({"type":"conversation.item.input_audio_transcription.delta","item_id":"two","delta":"bad"})).is_err());
    }
    #[test]
    fn leading_whitespace_is_removed_before_live_and_final_output() {
        let mut transcript = Transcript::default();
        for (delta, expected) in [
            (" ", ""),
            ("\t\u{2003}", ""),
            (" Hello", "Hello"),
            (" ", "Hello "),
            ("world", "Hello world"),
            ("!\nNext line", "Hello world!\nNext line"),
        ] {
            transcript.accept(&json!({"type":"conversation.item.input_audio_transcription.delta","item_id":"one","delta":delta})).unwrap();
            assert_eq!(transcript.text, expected);
        }
        transcript.accept(&json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"one","transcript":"  Hello world!\nNext line"})).unwrap();
        assert_eq!(transcript.text, "Hello world!\nNext line");
    }
    #[test]
    fn both_models_use_single_manual_turn() {
        for model in ["gpt-transcribe", "gpt-live-transcribe"] {
            let config = configuration(model);
            let input = &config["session"]["audio"]["input"];
            assert_eq!(input["format"]["rate"], 24000);
            assert_eq!(input["transcription"]["model"], model);
            if model == "gpt-live-transcribe" {
                assert_eq!(input["transcription"]["delay"], "high");
            } else {
                assert!(input["transcription"].get("delay").is_none());
            }
            assert!(input["turn_detection"].is_null());
            assert!(input["transcription"].get("language").is_none());
        }
    }
    #[test]
    fn server_error_never_echoes_untrusted_details() {
        let error = check_error(
            &json!({"type":"error","error":{"code":"invalid_api_key","message":"SECRET"}}),
            "gpt-transcribe",
        )
        .unwrap_err();
        assert!(!error.contains("SECRET"));
        assert!(check_error(
            &json!({"type":"conversation.item.input_audio_transcription.failed"}),
            "gpt-transcribe"
        )
        .is_err());
    }
}
