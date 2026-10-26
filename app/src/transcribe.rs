//! Whole-recording transcription through AI Gateway.

use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use serde_json::{Value, json};
use ureq::http::Response;

use crate::{audio, gateway};

const UNREACHABLE: &str =
    "Could not reach AI Gateway for transcription. Check your connection and try again.";
// 20 ms Opus frames; Ogg Opus granule positions always count 48 kHz samples.
const FRAME: usize = audio::SAMPLE_RATE as usize / 50;
const GRANULE_RATE: u64 = 48_000 / audio::SAMPLE_RATE as u64;
const OPUS_BITRATE: i32 = 24_000;

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

/// Encodes mono speech as Ogg Opus (RFC 7845): about 0.9 MB for five minutes.
fn ogg_opus(samples: &[f32]) -> Result<Vec<u8>, String> {
    const FAILED: &str = "Could not compress the recording.";
    let mut encoder = opus::Encoder::new(
        audio::SAMPLE_RATE,
        opus::Channels::Mono,
        opus::Application::Voip,
    )
    .map_err(|_| FAILED)?;
    encoder
        .set_bitrate(opus::Bitrate::Bits(OPUS_BITRATE))
        .map_err(|_| FAILED)?;
    let lookahead = encoder.get_lookahead().map_err(|_| FAILED)? as usize;
    let pre_skip = lookahead as u64 * GRANULE_RATE;
    let end = pre_skip + samples.len() as u64 * GRANULE_RATE;

    let mut head = b"OpusHead\x01\x01".to_vec();
    head.extend((pre_skip as u16).to_le_bytes());
    head.extend(audio::SAMPLE_RATE.to_le_bytes());
    head.extend([0, 0, 0]);
    let vendor = b"yapr";
    let mut tags = b"OpusTags".to_vec();
    tags.extend((vendor.len() as u32).to_le_bytes());
    tags.extend(vendor);
    tags.extend(0u32.to_le_bytes());

    let mut writer = PacketWriter::new(Vec::new());
    writer
        .write_packet(head, 1, PacketWriteEndInfo::EndPage, 0)
        .map_err(|_| FAILED)?;
    writer
        .write_packet(tags, 1, PacketWriteEndInfo::EndPage, 0)
        .map_err(|_| FAILED)?;

    let mut pcm: Vec<i16> = samples.iter().map(|s| pcm_sample(*s)).collect();
    let frames = (pcm.len() + lookahead).div_ceil(FRAME);
    pcm.resize(frames * FRAME, 0);
    let mut packet = [0u8; 4000];
    for (index, frame) in pcm.chunks(FRAME).enumerate() {
        let size = encoder.encode(frame, &mut packet).map_err(|_| FAILED)?;
        let last = index + 1 == frames;
        let granule = (((index + 1) * FRAME) as u64 * GRANULE_RATE).min(end);
        let info = if last {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        writer
            .write_packet(packet[..size].to_vec(), 1, info, granule)
            .map_err(|_| FAILED)?;
    }
    Ok(writer.into_inner())
}

fn request(
    url: String,
    model: &str,
    access: &gateway::Access,
) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
    gateway::agent(60)
        .post(url)
        .header("Authorization", format!("Bearer {}", access.token))
        .header("ai-gateway-protocol-version", "0.0.1")
        .header("ai-transcription-model-specification-version", "4")
        .header("ai-model-id", model)
}

/// The relay cannot take audio as a request body, so the recording goes to Vercel Blob first
/// and the relay forwards it from there.
fn through_blob(
    recording: &[u8],
    media_type: &str,
    model: &str,
    access: &gateway::Access,
) -> Result<Response<ureq::Body>, String> {
    let mut response = gateway::agent(60)
        .post(access.url("/uploads"))
        .header("Authorization", format!("Bearer {}", access.token))
        .send_json(json!({"mediaType": media_type}))
        .map_err(|_| UNREACHABLE)?;
    gateway::check(&mut response, access)?;
    let ticket = response
        .body_mut()
        .read_json::<Value>()
        .map_err(|_| "The liatrio.com relay returned an unreadable upload.")?;
    let (Some(url), Some(pathname), Some(headers)) = (
        ticket["url"].as_str(),
        ticket["pathname"].as_str(),
        ticket["headers"].as_object(),
    ) else {
        return Err("The liatrio.com relay returned an unreadable upload.".into());
    };
    let mut upload = gateway::agent(60).put(url);
    for (name, value) in headers {
        if let Some(value) = value.as_str() {
            upload = upload.header(name, value);
        }
    }
    let uploaded = upload
        .send(recording)
        .map_err(|_| "Could not upload the recording. Check your connection and try again.")?;
    if !uploaded.status().is_success() {
        return Err(format!(
            "Could not upload the recording (HTTP {}).",
            uploaded.status().as_u16()
        ));
    }
    request(access.url("/transcriptions"), model, access)
        .send_json(json!({"blob": pathname, "mediaType": media_type}))
        .map_err(|_| UNREACHABLE.into())
}

pub fn transcribe(
    samples: &[f32],
    model: &str,
    access: &gateway::Access,
) -> Result<String, String> {
    if samples.is_empty() {
        return Ok(String::new());
    }
    let mut response = if access.relayed() {
        let response = through_blob(&ogg_opus(samples)?, "audio/ogg", model, access)?;
        // AI Gateway documents Ogg Opus only for Microsoft models; other models may need WAV.
        if response.status().as_u16() == 400 {
            through_blob(&wav(samples)?, "audio/wav", model, access)?
        } else {
            response
        }
    } else {
        request(access.url("/v4/ai/transcription-model"), model, access)
            .send_json(json!({"audio": STANDARD.encode(wav(samples)?), "mediaType": "audio/wav"}))
            .map_err(|_| UNREACHABLE)?
    };
    gateway::check(&mut response, access)?;
    let body = response
        .body_mut()
        .read_json::<Value>()
        .map_err(|_| "AI Gateway returned an unreadable transcript.")?;
    body.get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "AI Gateway returned no transcript.".into())
}
