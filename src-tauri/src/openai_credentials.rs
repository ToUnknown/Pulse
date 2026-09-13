//! Shared with Live Translate: do not change the service/account identifiers.
//! The secret never leaves the Rust backend after Settings saves it.
const API_KEY_SERVICE: &str = "app.pulse.desktop";
const API_KEY_ACCOUNT: &str = "openai-api-key";
pub const MISSING_KEY: &str =
    "Add an OpenAI API key in Pulse Settings to use Advanced or Translate.";

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(API_KEY_SERVICE, API_KEY_ACCOUNT)
        .map_err(|_| "Secure credential storage is unavailable.".into())
}

pub fn is_configured() -> Result<bool, String> {
    match entry()?.get_password() {
        Ok(key) => Ok(!key.trim().is_empty()),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(_) => Err("Could not access the shared OpenAI key in credential storage.".into()),
    }
}

pub fn load() -> Result<String, String> {
    match entry()?.get_password() {
        Ok(key) if !key.trim().is_empty() => Ok(key),
        Ok(_) | Err(keyring::Error::NoEntry) => Err(MISSING_KEY.into()),
        Err(_) => Err("Could not access the shared OpenAI key in credential storage.".into()),
    }
}

pub async fn validate_and_save(key: &str) -> Result<(), String> {
    validate_and_persist(key, "https://api.openai.com/v1/responses", |key| {
        entry()?
            .set_password(key)
            .map_err(|_| "Could not save the OpenAI key securely.".into())
    })
    .await
}

async fn validate_and_persist(
    key: &str,
    endpoint: &str,
    persist: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || key.len() > 2048 || key.chars().any(char::is_whitespace) {
        return Err("Enter an OpenAI API key without spaces.".into());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not prepare the API key check.")?;
    // Like Live Translate, verify the actual model/transport before replacing the key.
    // This tiny request contains no screenshot, clipboard text, or user content.
    let response = client
        .post(endpoint)
        .bearer_auth(key)
        .json(&serde_json::json!({
            "model": crate::text_extractor::protocol::MODEL,
            "reasoning": {"effort": "none"},
            "input": "Reply with OK.",
            "max_output_tokens": 16,
            "store": false
        }))
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "The API key check timed out. Try again."
            } else {
                "Could not reach OpenAI. Check your connection and try again."
            }
        })?;
    match response.status().as_u16() {
        200 => {}
        401 => return Err("Not a valid OpenAI API key.".into()),
        403 | 404 => return Err("This API key cannot access GPT-5.6 Luna.".into()),
        429 => {
            return Err(
                "OpenAI could not check this key because its usage or billing limit was reached."
                    .into(),
            )
        }
        500..=599 => return Err("OpenAI is temporarily unavailable. Try again.".into()),
        status => {
            return Err(format!(
                "OpenAI rejected the API key check (HTTP {status})."
            ))
        }
    }
    let response = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| "OpenAI returned an unreadable key-check response.")?;
    if !crate::text_extractor::protocol::response_text(&response)
        .is_ok_and(|text| !text.trim().is_empty())
    {
        return Err("OpenAI did not complete the API key check. Try again.".into());
    }
    persist(key)
}

pub fn clear() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err("Could not remove the shared OpenAI key from credential storage.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
    };

    #[tokio::test]
    async fn validation_only_replaces_the_key_after_a_completed_model_response() {
        for (status, body, succeeds) in [
            (
                200,
                r#"{"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"OK"}]}]}"#,
                true,
            ),
            (401, "{}", false),
            (403, "{}", false),
            (429, "{}", false),
            (503, "{}", false),
            (200, r#"{"status":"incomplete"}"#, false),
            (200, r#"{"status":"completed","output":[]}"#, false),
            (200, "not-json", false),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let size = stream.read(&mut chunk).unwrap();
                    assert!(size > 0);
                    request.extend_from_slice(&chunk[..size]);
                    if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .unwrap()
                            .trim()
                            .parse()
                            .unwrap();
                        if request.len() < end + 4 + length {
                            continue;
                        }
                        assert!(headers.starts_with("post /v1/responses "));
                        assert!(headers.contains("authorization: bearer fixture-candidate"));
                        let body: serde_json::Value =
                            serde_json::from_slice(&request[end + 4..]).unwrap();
                        assert_eq!(body["model"], crate::text_extractor::protocol::MODEL);
                        assert_eq!(body["reasoning"]["effort"], "none");
                        assert_eq!(body["max_output_tokens"], 16);
                        assert_eq!(body["store"], false);
                        break;
                    }
                }
                write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let stored = Arc::new(Mutex::new("fixture-previous".to_string()));
            let result = validate_and_persist("fixture-candidate", &endpoint, |key| {
                *stored.lock().unwrap() = key.to_owned();
                Ok(())
            })
            .await;
            server.join().unwrap();
            assert_eq!(result.is_ok(), succeeds);
            assert_eq!(
                *stored.lock().unwrap(),
                if succeeds {
                    "fixture-candidate"
                } else {
                    "fixture-previous"
                }
            );
        }
    }

    #[tokio::test]
    async fn malformed_keys_fail_before_network_or_storage_access() {
        for key in ["", "has a space", "line\nbreak"] {
            assert!(validate_and_persist(key, "http://127.0.0.1:1", |_| panic!(
                "Must not save invalid input"
            ))
            .await
            .is_err());
        }
    }
}
