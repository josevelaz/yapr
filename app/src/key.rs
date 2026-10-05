use std::sync::atomic::{AtomicBool, Ordering};

use security_framework::passwords::{get_generic_password, set_generic_password};

const SERVICE: &str = "Yapr AI Gateway";
const ACCOUNT: &str = "api-key";
const NOT_FOUND: i32 = -25300;
const CREDITS_URL: &str = "https://ai-gateway.vercel.sh/v1/credits";

pub const MISSING: &str = "No AI Gateway API key. Choose Set API Key in the Yapr menu.";

static PRESENT: AtomicBool = AtomicBool::new(false);

pub fn known_present() -> bool {
    PRESENT.load(Ordering::Relaxed)
}

pub fn load() -> Result<Option<String>, String> {
    let key = read();
    PRESENT.store(matches!(key, Ok(Some(_))), Ordering::Relaxed);
    key
}

fn read() -> Result<Option<String>, String> {
    match get_generic_password(SERVICE, ACCOUNT) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| "The saved AI Gateway API key is unreadable. Set it again.".to_string()),
        Err(e) if e.code() == NOT_FOUND => Ok(None),
        Err(e) => Err(format!(
            "Could not read the AI Gateway API key from the Keychain: {e}"
        )),
    }
}

pub fn require() -> Result<String, String> {
    load()?.ok_or_else(|| MISSING.to_string())
}

pub fn set(key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || key.len() > 512 || key.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("That does not look like an AI Gateway API key.".into());
    }
    validate(key)?;
    set_generic_password(SERVICE, ACCOUNT, key.as_bytes())
        .map_err(|e| format!("Could not save the key to the Keychain: {e}"))?;
    PRESENT.store(true, Ordering::Relaxed);
    Ok(())
}

fn validate(key: &str) -> Result<(), String> {
    let response = crate::gateway::agent(15)
        .get(CREDITS_URL)
        .header("Authorization", format!("Bearer {key}"))
        .call()
        .map_err(|e| format!("Could not reach AI Gateway to check the key: {e}"))?;
    match response.status().as_u16() {
        200 => Ok(()),
        401 | 403 => Err("AI Gateway rejected this API key.".into()),
        status => Err(format!(
            "AI Gateway could not check the key (HTTP {status})."
        )),
    }
}
