//! Whole-recording transcription through AI Gateway.

use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::{audio, gateway};

pub fn pcm(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|s| pcm_sample(*s).to_le_bytes())
        .collect()
}

fn pcm_sample(sample: f32) -> i16 {
    (sample * 32768.0).clamp(-32768.0, 32767.0) as i16
}

fn wav(samples: &[f32]) -> Result<Vec<u8>, String> {
    let mut output = Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: audio::SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::new(&mut output, spec).map_err(|_| "Could not encode the recording.")?;
    for sample in samples {
        writer
            .write_sample(pcm_sample(*sample))
            .map_err(|_| "Could not encode the recording.")?;
    }
    writer
        .finalize()
        .map_err(|_| "Could not encode the recording.")?;
    Ok(output.into_inner())
}

pub fn transcribe(samples: &[f32], model: &str, key: &str) -> Result<String, String> {
    if samples.is_empty() {
        return Ok(String::new());
    }
    let mut response = gateway::agent(60)
        .post("https://ai-gateway.vercel.sh/v4/ai/transcription-model")
        .header("Authorization", format!("Bearer {key}"))
        .header("ai-gateway-protocol-version", "0.0.1")
        .header("ai-transcription-model-specification-version", "4")
        .header("ai-model-id", model)
        .send_json(json!({"audio": STANDARD.encode(wav(samples)?), "mediaType": "audio/wav"}))
        .map_err(|_| "Could not reach AI Gateway for transcription. Check your connection and try again.")?;
    gateway::check(&mut response, key)?;
    let body = response
        .body_mut()
        .read_json::<Value>()
        .map_err(|_| "AI Gateway returned an unreadable transcript.")?;
    body.get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "AI Gateway returned no transcript.".into())
}
