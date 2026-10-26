use std::time::Duration;

use serde_json::Value;

pub const AI_GATEWAY: &str = "https://ai-gateway.vercel.sh";

/// Where gateway requests go and the bearer token they carry: AI Gateway with the user's
/// key, or the liatrio.com relay with a Google ID token.
#[derive(Clone)]
pub struct Access {
    pub base: String,
    pub token: String,
}

impl Access {
    pub fn direct(key: String) -> Self {
        Self {
            base: AI_GATEWAY.into(),
            token: key,
        }
    }

    pub fn relayed(&self) -> bool {
        self.base != AI_GATEWAY
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

pub fn agent(seconds: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(seconds)))
        .build()
        .new_agent()
}

pub fn message(message: &str, key: &str) -> String {
    let key: Vec<char> = key.chars().collect();
    let fragments: std::collections::HashSet<&[char]> = key.windows(8).collect();
    let text: Vec<char> = message.chars().collect();
    let mut hidden = vec![false; text.len()];
    for (start, window) in text.windows(8).enumerate() {
        if fragments.contains(window) {
            hidden[start..start + 8].fill(true);
        }
    }
    let mut output = String::new();
    let mut redacting = false;
    for (character, hide) in text.into_iter().zip(hidden) {
        if hide {
            if !redacting {
                output.push_str("[redacted]");
            }
        } else {
            output.push(character);
        }
        redacting = hide;
    }
    output.chars().take(400).collect()
}

pub fn error(status: u16, body: &Value, access: &Access) -> String {
    let key = &access.token;
    let detail = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("No details from the gateway.");
    match status {
        401 if access.relayed() => {
            "The liatrio.com relay rejected your Google sign-in. Sign in with Google again.".into()
        }
        401 | 403 => "AI Gateway rejected the key. Choose Set API Key in the Yapr menu.".into(),
        413 => "The recording is too long to send. Try a shorter dictation.".into(),
        402 => "AI Gateway is out of credits. Add credits and try again.".into(),
        429 => "AI Gateway rate limited this request. Try again shortly.".into(),
        400 | 404 => format!(
            "Unknown or unsupported AI Gateway model: {}",
            message(detail, key)
        ),
        _ => format!(
            "AI Gateway failed (HTTP {status}): {}",
            message(detail, key)
        ),
    }
}

pub fn check(
    response: &mut ureq::http::Response<ureq::Body>,
    access: &Access,
) -> Result<(), String> {
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response
            .body_mut()
            .read_json::<Value>()
            .unwrap_or(Value::Null);
        return Err(error(status, &body, access));
    }
    Ok(())
}
