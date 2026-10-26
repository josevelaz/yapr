use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use serde_json::Value;

use crate::id_token::{self, Jwks};

pub const DOMAIN: &str = "liatrio.com";

const CLIENT_ID: Option<&str> = option_env!("YAPR_EMBEDDED_GOOGLE_CLIENT_ID");
const CLIENT_SECRET: Option<&str> = option_env!("YAPR_EMBEDDED_GOOGLE_CLIENT_SECRET");
const SHARED_KEY: Option<&str> = option_env!("YAPR_EMBEDDED_LIATRIO_GATEWAY_KEY");

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const SERVICE: &str = "Yapr Google Sign-In";
const ACCOUNT: &str = "account-email";
const NOT_FOUND: i32 = -25300;
const WAIT: Duration = Duration::from_secs(180);

static EMAIL: Mutex<Option<String>> = Mutex::new(None);
static SIGNING_IN: AtomicBool = AtomicBool::new(false);

pub fn available() -> bool {
    CLIENT_ID.is_some() && SHARED_KEY.is_some()
}

pub fn known_email() -> Option<String> {
    EMAIL.lock().unwrap().clone()
}

pub fn shared_key() -> Option<&'static str> {
    SHARED_KEY.filter(|_| known_email().is_some())
}

pub fn load() {
    let email = available()
        .then(|| get_generic_password(SERVICE, ACCOUNT).ok())
        .flatten()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|email| in_domain(email));
    *EMAIL.lock().unwrap() = email;
}

pub fn sign_out() -> Result<String, String> {
    match delete_generic_password(SERVICE, ACCOUNT) {
        Ok(()) => {}
        Err(e) if e.code() == NOT_FOUND => {}
        Err(e) => return Err(format!("Could not remove the Google sign-in: {e}")),
    }
    *EMAIL.lock().unwrap() = None;
    Ok("Signed out of Google.".into())
}

pub fn sign_in() -> Result<String, String> {
    let Some(client_id) = CLIENT_ID.filter(|_| available()) else {
        return Err("This build of Yapr has no Google sign-in.".into());
    };
    if SIGNING_IN.swap(true, Ordering::SeqCst) {
        return Err("Google sign-in is already open in your browser.".into());
    }
    let result = run_flow(client_id);
    SIGNING_IN.store(false, Ordering::SeqCst);
    let email = result?;
    set_generic_password(SERVICE, ACCOUNT, email.as_bytes())
        .map_err(|e| format!("Could not save the Google sign-in to the Keychain: {e}"))?;
    *EMAIL.lock().unwrap() = Some(email.clone());
    Ok(format!("Signed in as {email}."))
}

fn in_domain(email: &str) -> bool {
    email.to_ascii_lowercase().ends_with(&format!("@{DOMAIN}"))
}

fn run_flow(client_id: &str) -> Result<String, String> {
    let rng = SystemRandom::new();
    let verifier = random(&rng, 32)?;
    let state = random(&rng, 16)?;
    let challenge = URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()));

    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Could not start Google sign-in: {e}"))?;
    let redirect = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().map_err(|e| e.to_string())?.port()
    );
    let mut url = url::Url::parse(AUTH_URL).unwrap();
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", &redirect)
        .append_pair("response_type", "code")
        .append_pair("scope", "openid email")
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("hd", DOMAIN)
        .append_pair("prompt", "select_account");
    let opened = std::process::Command::new("/usr/bin/open")
        .arg(url.as_str())
        .status()
        .map_err(|e| format!("Could not open the browser: {e}"))?;
    if !opened.success() {
        return Err("Could not open the browser for Google sign-in.".into());
    }

    let code = wait_for_code(&listener, &state)?;
    drop(listener);
    let token = exchange(client_id, &code, &verifier, &redirect)?;
    let mut response = crate::gateway::agent(15)
        .get(JWKS_URL)
        .call()
        .map_err(|e| format!("Could not reach Google to check the sign-in: {e}"))?;
    let jwks: Jwks = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("Could not read Google's signing keys: {e}"))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    id_token::verify(&token, &jwks, client_id, DOMAIN, now)
}

fn random(rng: &SystemRandom, bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0; bytes];
    rng.fill(&mut buffer)
        .map_err(|_| "Could not create a random value for Google sign-in.")?;
    Ok(URL_SAFE_NO_PAD.encode(buffer))
}

fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(result) = handle(stream, state) {
                    return result;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("Google sign-in stopped: {e}")),
        }
    }
    Err("Google sign-in timed out.".into())
}

fn handle(mut stream: TcpStream, state: &str) -> Option<Result<String, String>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 16 * 1024 {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
    }
    let request = String::from_utf8_lossy(&request);
    let target = request
        .lines()
        .next()?
        .strip_prefix("GET ")?
        .split(' ')
        .next()?;
    let url = url::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if url.path() != "/" {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return None;
    }
    let query = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let result = if query("state").as_deref() != Some(state) {
        Err("Google sign-in returned an unexpected response.".to_string())
    } else if let Some(code) = query("code") {
        Ok(code)
    } else {
        Err(format!(
            "Google sign-in did not finish ({}).",
            query("error").unwrap_or_else(|| "no code".into())
        ))
    };
    let body =
        "<!doctype html><title>Yapr</title><p>You can close this tab and return to Yapr.</p>";
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    Some(result)
}

fn exchange(client_id: &str, code: &str, verifier: &str, redirect: &str) -> Result<String, String> {
    let mut form = vec![
        ("client_id", client_id),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect),
    ];
    if let Some(secret) = CLIENT_SECRET {
        form.push(("client_secret", secret));
    }
    let mut response = crate::gateway::agent(15)
        .post(TOKEN_URL)
        .send_form(form)
        .map_err(|e| format!("Could not reach Google to finish sign-in: {e}"))?;
    let status = response.status().as_u16();
    let body: Value = response.body_mut().read_json().unwrap_or(Value::Null);
    if status != 200 {
        let reason = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!("Google sign-in failed (HTTP {status}: {reason})."));
    }
    body.get("id_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "Google sign-in returned no ID token.".into())
}
