//! Prototype Google web translation. No account, credentials, or automatic provider fallback.
//! This is an unofficial endpoint: reject unexpected responses instead of losing the draft.
use super::protocol;
use reqwest::Client;
use serde_json::Value;
use std::{sync::OnceLock, time::Duration};
use tokio_util::sync::CancellationToken;

const ENDPOINT: &str = "https://translate.googleapis.com/translate_a/single";
// A conservative prototype limit, not a promised Google quota. The editor applies it too.
const MAX_CHARACTERS: usize = 5_000;
const MAX_RESPONSE_BYTES: usize = 1_000_000;
static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();

fn client() -> Result<Client, String> {
    CLIENT
        .get_or_init(|| {
            // Normal app startup installs this in session::install; standalone tests bypass it.
            #[cfg(test)]
            let _ = rustls::crypto::ring::default_provider().install_default();
            Client::builder()
                .timeout(Duration::from_secs(12))
                .connect_timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "Could not connect to Google Translate.".into())
        })
        .clone()
}

pub async fn request(
    text: &str,
    language: &str,
    cancel: CancellationToken,
) -> Result<String, String> {
    protocol::translation_target(text, language)?;
    if text.chars().count() > MAX_CHARACTERS {
        return Err(
            "Translation supports up to 5,000 characters here. Shorten the text to translate."
                .into(),
        );
    }
    let client = client()?;
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("Capture cancelled.".into()),
        result = translate_at(&client, ENDPOINT, text, language) => result,
    }
}

async fn translate_at(
    client: &Client,
    endpoint: &str,
    text: &str,
    language: &str,
) -> Result<String, String> {
    let mut response = client
        .post(endpoint)
        // Keep selected text out of URLs and never attach OpenAI API credentials.
        .form(&[
            ("client", "gtx"),
            ("sl", "auto"),
            ("tl", if language == "zh" { "zh-CN" } else { language }),
            ("dt", "t"),
            ("dj", "1"),
            ("q", text),
        ])
        .send()
        .await
        // Never surface raw network errors: they can include request details.
        .map_err(|error| {
            if error.is_timeout() {
                "Google Translate timed out. Try again."
            } else {
                "Could not reach Google Translate. Check your connection and try again."
            }
        })?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            403 | 429 => {
                "Google Translate is temporarily blocking requests. Try later or use Advanced."
            }
            _ => "Google Translate is unavailable. Try again or use Advanced.",
        }
        .into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Google's translation was interrupted. Try again.")?
    {
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err("Google returned an oversized translation. Use a shorter passage.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_translation(&bytes)
}

fn parse_translation(bytes: &[u8]) -> Result<String, String> {
    let invalid =
        "Google returned an incomplete or unreadable translation. Try again or use Advanced.";
    let response: Value = serde_json::from_slice(bytes).map_err(|_| invalid)?;
    let sentences = response["sentences"].as_array().ok_or(invalid)?;
    let mut text = String::new();
    for sentence in sentences {
        // No filter_map: a missing segment must not silently produce a partial translation.
        let translated = sentence["trans"].as_str().ok_or(invalid)?;
        if translated.trim().is_empty()
            && sentence["orig"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty())
        {
            return Err(invalid.into());
        }
        // Google includes sentence spacing and paragraph breaks in each segment.
        text.push_str(translated);
    }
    if text.trim().is_empty() {
        return Err(invalid.into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn preserves_sentence_spacing_and_paragraphs() {
        assert_eq!(
            parse_translation(br#"{"sentences":[{"trans":"First. "},{"trans":"Second.\n\n"},{"trans":"1. Third."}]}"#).unwrap(),
            "First. Second.\n\n1. Third."
        );
    }

    #[test]
    fn rejects_empty_malformed_and_partial_responses() {
        for body in [
            "<html>blocked</html>",
            "{}",
            r#"{"sentences":[]}"#,
            r#"{"sentences":[{"trans":""}]}"#,
            r#"{"sentences":[{"trans":"first"},{"orig":"missing"}]}"#,
            r#"{"sentences":[{"trans":"first"},{"trans":"", "orig":"missing"}]}"#,
        ] {
            assert!(parse_translation(body.as_bytes()).is_err(), "{body}");
        }
    }

    #[tokio::test]
    async fn validates_input_and_honors_cancellation_without_network() {
        assert!(request("hello", "invalid", CancellationToken::new())
            .await
            .is_err());
        assert!(request(" ", "en", CancellationToken::new()).await.is_err());
        assert!(request(&"і".repeat(5_001), "en", CancellationToken::new())
            .await
            .unwrap_err()
            .contains("5,000"));
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            request("Привіт", "en", cancel).await.unwrap_err(),
            "Capture cancelled."
        );
    }

    // These tests exercise HTTP without sending user content or making external requests.
    fn server(status: &str, body: String) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/translate", listener.local_addr().unwrap());
        let status = status.to_string();
        let task = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
                if let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length: usize = header
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            // The client deliberately closes oversized responses early.
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            String::from_utf8(request).unwrap()
        });
        (url, task)
    }

    #[tokio::test]
    async fn posts_encoded_text_without_credentials_or_query_string() {
        let (url, task) = server("200 OK", r#"{"sentences":[{"trans":"Привіт!"}]}"#.into());
        let result = translate_at(&client().unwrap(), &url, "Hello & +\nworld?", "zh")
            .await
            .unwrap();
        assert_eq!(result, "Привіт!");
        let sent = task.join().unwrap();
        assert!(sent.starts_with("POST /translate HTTP/1.1\r\n"));
        assert!(sent.contains("q=Hello+%26+%2B%0Aworld%3F"));
        assert!(sent.contains("tl=zh-CN"));
        assert!(sent.contains("client=gtx&sl=auto"));
        assert!(!sent.to_lowercase().contains("authorization:"));
        assert!(!sent.to_lowercase().contains("cookie:"));
    }

    #[tokio::test]
    async fn rejects_rate_limits_redirects_and_oversized_responses() {
        for (status, body) in [
            ("429 Too Many Requests", "private error details".to_string()),
            ("302 Found", "redirect".to_string()),
            ("200 OK", "x".repeat(MAX_RESPONSE_BYTES + 1)),
        ] {
            let (url, task) = server(status, body);
            let error = translate_at(&client().unwrap(), &url, "hello", "uk")
                .await
                .unwrap_err();
            assert!(!error.contains("private error details"));
            task.join().unwrap();
        }
    }

    #[tokio::test]
    #[ignore = "Makes an external request with synthetic text; run explicitly when authorized"]
    async fn live_google_translates_synthetic_text() {
        for (language, text) in [
            (
                "uk",
                "Plan for today\n\n1. Save the original text.\n2. Translate the message.",
            ),
            (
                "en",
                "План на сьогодні\n\n1. Зберегти вихідний текст.\n2. Перекласти повідомлення.",
            ),
        ] {
            let start = std::time::Instant::now();
            let translated = request(text, language, CancellationToken::new())
                .await
                .unwrap();
            assert_ne!(translated, text);
            assert!(translated.contains("1.") && translated.contains("2."));
            assert!(translated.contains('\n'));
            println!("{language}: {:?}\n{translated}", start.elapsed());
        }
    }
}
