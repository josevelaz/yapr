//! One blocking WebSocket worker per recording. Closing the chunk channel sends audio-done.

use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Error, Message};

use crate::{gateway, transcribe};

pub struct Session(JoinHandle<Result<String, String>>);

impl Session {
    pub fn start(
        model: String,
        key: String,
        chunks: Receiver<Vec<f32>>,
        on_part: impl FnMut(&str, &str) + Send + 'static,
    ) -> Result<Self, String> {
        std::thread::Builder::new()
            .name("transcription-stream".into())
            .spawn(move || run(&model, &key, chunks, on_part))
            .map(Self)
            .map_err(|_| "Could not start streaming transcription.".into())
    }

    pub fn finish(self) -> Result<String, String> {
        self.0
            .join()
            .unwrap_or_else(|_| Err("Streaming transcription stopped unexpectedly.".into()))
    }
}

fn socket_error(error: Error, key: &str) -> String {
    match error {
        Error::Http(response) => {
            let body = response
                .body()
                .as_ref()
                .and_then(|b| serde_json::from_slice::<Value>(b).ok())
                .unwrap_or(Value::Null);
            gateway::error(response.status().as_u16(), &body, key)
        }
        _ => "The AI Gateway transcription connection failed. Check your connection.".into(),
    }
}

fn run(
    model: &str,
    key: &str,
    chunks: Receiver<Vec<f32>>,
    mut on_part: impl FnMut(&str, &str),
) -> Result<String, String> {
    let mut url = url::Url::parse("wss://ai-gateway.vercel.sh/v4/ai/transcription-model").unwrap();
    url.query_pairs_mut().append_pair("ai-model-id", model);
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| "Invalid transcription model.")?;
    let headers = request.headers_mut();
    headers.insert(
        "Authorization",
        format!("Bearer {key}")
            .parse()
            .map_err(|_| "Invalid AI Gateway key.")?,
    );
    headers.insert(
        "Sec-WebSocket-Protocol",
        format!("ai-gateway-transcription.v1, ai-gateway-auth.{key}")
            .parse()
            .map_err(|_| "Invalid AI Gateway key.")?,
    );
    headers.insert("ai-gateway-protocol-version", "0.0.1".parse().unwrap());
    headers.insert(
        "ai-transcription-model-specification-version",
        "4".parse().unwrap(),
    );
    let addresses = ("ai-gateway.vercel.sh", 443)
        .to_socket_addrs()
        .map_err(|_| "Could not reach AI Gateway for streaming transcription.")?;
    let tcp = addresses
        .filter_map(|address| TcpStream::connect_timeout(&address, Duration::from_secs(5)).ok())
        .next()
        .ok_or("Could not reach AI Gateway for streaming transcription.")?;
    tcp.set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|_| "Could not open transcription connection.")?;
    tcp.set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| "Could not open transcription connection.")?;
    let (mut socket, _) = tungstenite::client_tls_with_config(request, tcp, None, None).map_err(
        |error| match error {
            tungstenite::HandshakeError::Failure(error) => socket_error(error, key),
            _ => "Could not open AI Gateway streaming transcription.".into(),
        },
    )?;
    match socket.get_mut() {
        MaybeTlsStream::Rustls(stream) => stream
            .sock
            .set_read_timeout(Some(Duration::from_millis(20))),
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(Some(Duration::from_millis(20))),
        _ => return Err("Unsupported transcription connection.".into()),
    }
    .map_err(|_| "Could not set the transcription timeout.")?;
    socket.send(Message::Text(json!({"type": "transcription-stream.start", "inputAudioFormat": {"type": "audio/pcm", "rate": 16000}}).to_string().into()))
        .map_err(|e| socket_error(e, key))?;
    let mut done_at = None;
    let mut finals = String::new();
    // Some models (MAI) emit committed deltas plus an uncommitted partial, then one
    // final for the whole utterance. Others (Grok) emit partial/final pairs per segment.
    let mut deltas = String::new();
    let mut partial = String::new();
    loop {
        if done_at.is_none() {
            // Bound each batch so incoming text is read even when replay/capture runs ahead.
            for _ in 0..32 {
                match chunks.try_recv() {
                    Ok(samples) => {
                        for bytes in transcribe::pcm(&samples).chunks(64 * 1024) {
                            socket
                                .send(Message::Binary(bytes.to_vec().into()))
                                .map_err(|e| socket_error(e, key))?;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        socket
                            .send(Message::Text(
                                json!({"type": "transcription-stream.audio-done"})
                                    .to_string()
                                    .into(),
                            ))
                            .map_err(|e| socket_error(e, key))?;
                        done_at = Some(Instant::now());
                        break;
                    }
                }
            }
        }
        if done_at.is_some_and(|start| start.elapsed() > Duration::from_secs(60)) {
            return Err("AI Gateway timed out waiting for the final transcript.".into());
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                let part: Value = serde_json::from_str(&text)
                    .map_err(|_| "AI Gateway returned an unreadable transcription stream.")?;
                let kind = part
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                match kind {
                    "transcript-delta" => {
                        let delta = part
                            .get("delta")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        deltas.push_str(delta);
                        partial = partial.strip_prefix(delta).unwrap_or_default().to_string();
                    }
                    "transcript-partial" => {
                        partial = part
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    }
                    "transcript-final" => {
                        let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
                        if !finals.is_empty()
                            && !finals.ends_with(char::is_whitespace)
                            && !text.starts_with(char::is_whitespace)
                        {
                            finals.push(' ');
                        }
                        finals.push_str(text);
                        deltas.clear();
                        partial.clear();
                    }
                    "finish" => {
                        let text = part
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or("AI Gateway returned no final transcript.")?;
                        on_part(kind, text);
                        let _ = socket.close(None);
                        return Ok(text.to_string());
                    }
                    "error" => {
                        on_part(kind, "");
                        let code = match part.pointer("/error/type").and_then(Value::as_str) {
                            Some("authentication_error") => 401,
                            Some("forbidden") => 403,
                            Some("model_not_found") => 404,
                            Some("invalid_request_error") => 400,
                            Some("rate_limit_exceeded") => 429,
                            _ => 500,
                        };
                        return Err(gateway::error(code, &part, key));
                    }
                    _ => {}
                }
                let current = format!("{deltas}{partial}");
                let separator = if !finals.is_empty()
                    && !current.is_empty()
                    && !finals.ends_with(char::is_whitespace)
                    && !current.starts_with(char::is_whitespace)
                {
                    " "
                } else {
                    ""
                };
                on_part(kind, &format!("{finals}{separator}{current}"));
            }
            Ok(Message::Close(_)) | Err(Error::ConnectionClosed) => {
                return Err(
                    "AI Gateway closed the transcription stream before the final transcript."
                        .into(),
                );
            }
            Err(Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err(socket_error(error, key)),
            _ => {}
        }
    }
}
