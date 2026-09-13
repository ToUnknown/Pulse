//! Shared with Live Translate: do not change the service/account identifiers.
//! The secret never leaves the Rust backend after Settings saves it.
const API_KEY_SERVICE: &str = "app.pulse.desktop";
const API_KEY_ACCOUNT: &str = "openai-api-key";
pub const MISSING_KEY: &str = "Add an OpenAI API key in Settings before enabling Text Extractor.";

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

pub fn save(key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || key.len() > 2048 || key.chars().any(char::is_whitespace) {
        return Err("Enter an OpenAI API key without spaces.".into());
    }
    entry()?
        .set_password(key)
        .map_err(|_| "Could not save the OpenAI key securely.".into())
}
