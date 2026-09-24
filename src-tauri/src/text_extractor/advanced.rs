//! OpenAI requests shared by Advanced extraction and Translate on both platforms.
use super::protocol;
use crate::openai_credentials;
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const RESPONSE_URL: &str = "https://api.openai.com/v1/responses";

pub async fn request(body: Value, cancel: CancellationToken) -> Result<String, String> {
    tokio::select! {
        _ = cancel.cancelled() => Err("Capture cancelled.".into()),
        result = openai_request(body) => result,
    }
}

async fn openai_request(body: Value) -> Result<String, String> {
    let key = openai_credentials::load()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not start the connection to OpenAI.")?;
    let mut response = client
        .post(RESPONSE_URL)
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "The request timed out. Try again or use less text."
            } else {
                "Could not reach OpenAI. Check your connection and try again."
            }
        })?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 => "The OpenAI API key was rejected. Update the shared key in Settings.",
            403 | 404 => "This API key does not have access to GPT-6 Luna.",
            429 => "OpenAI usage or rate limit reached. Check your API billing or try again later.",
            _ => "OpenAI could not complete the request. Try again shortly.",
        }
        .into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "The response was interrupted. Try again.")?
    {
        if bytes.len() + chunk.len() > 2_000_000 {
            return Err("The response was too large. Select a smaller area.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let response = serde_json::from_slice(&bytes)
        .map_err(|_| "OpenAI returned an unreadable response. Try again.")?;
    protocol::response_text(&response)
}
