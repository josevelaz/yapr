//! Signed-helper near-end E2E and offline AEC comparisons. Artifacts contain audio,
//! never credentials. Only run() makes gateway calls (two per captured scene).

use anyhow::{Context, Result, ensure};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat};
use serde_json::json;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use crate::audio::{Recorder, Resampler, load_wav};
use crate::audio_timing::{FRAME, RATE, db, rms, thread_cpu_seconds};
use crate::echo::{Cleaner, EchoCanceller, Noise, Trace, estimate_delay};
use crate::{key, transcribe};

const SCRIPT: &str = "Okay so here is the plan for tomorrow. First we ship the dictation helper to the team. Then, um, we write the README and check the numbers. After that we take the rest of the afternoon off.";

fn output_dir(name: &str) -> Result<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("missing project root")?;
    let out = root.join("e2e/echo-results").join(name);
    std::fs::create_dir_all(&out)?;
    Ok(out)
}

fn write_wav(path: impl AsRef<Path>, samples: &[f32]) -> Result<()> {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )?;
    for &sample in samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    Ok(())
}

struct Music(Option<Child>);
impl Drop for Music {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct Playback {
    _stream: cpal::Stream,
    first_time: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
}

impl Playback {
    fn start(speech: &[f32]) -> Result<Self> {
        let device = cpal::default_host()
            .default_output_device()
            .context("No audio output device")?;
        let supported = device.default_output_config()?;
        let rate = supported.sample_rate();
        let config: cpal::StreamConfig = supported.into();
        let mut samples = Vec::new();
        Resampler::new(RATE, rate).process(speech, &mut samples);
        let first_time = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let stream = match supported.sample_format() {
            SampleFormat::F32 => {
                output::<f32>(&device, config, samples, first_time.clone(), done.clone())
            }
            SampleFormat::I16 => {
                output::<i16>(&device, config, samples, first_time.clone(), done.clone())
            }
            SampleFormat::I32 => {
                output::<i32>(&device, config, samples, first_time.clone(), done.clone())
            }
            other => anyhow::bail!("Unsupported output format {other}"),
        }?;
        stream.play()?;
        Ok(Self {
            _stream: stream,
            first_time,
            done,
        })
    }
}

fn output<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    samples: Vec<f32>,
    first: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut index = 0;
    device.build_output_stream(
        config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            let time = info
                .timestamp()
                .playback
                .duration_since(cpal::StreamInstant::ZERO)
                .as_secs_f64();
            let _ = first.compare_exchange(0, time.to_bits(), Ordering::Relaxed, Ordering::Relaxed);
            for frame in data.chunks_mut(channels) {
                // Keep the near-end quieter than music, without changing system volume.
                let value = samples.get(index).copied().unwrap_or(0.);
                index += 1;
                for sample in frame {
                    *sample = T::from_sample(value);
                }
            }
            if index >= samples.len() {
                done.store(true, Ordering::Relaxed);
            }
        },
        |_| {},
        None,
    )
}

pub fn run(music: &str, speech: &str, model: &str) -> Result<()> {
    ensure!(
        Path::new(speech)
            .file_name()
            .is_some_and(|name| name == "pauses.wav"),
        "WER mode requires the known pauses.wav script"
    );
    let speech = load_wav(Path::new(speech)).map_err(anyhow::Error::msg)?;
    let trace = Arc::new(Mutex::new(Trace::default()));
    let mut recorder = Recorder::start_measurement(trace.clone()).map_err(anyhow::Error::msg)?;
    let feed = recorder.take_chunks().context("missing live chunk feed")?;
    let feed_worker = std::thread::spawn(move || feed.into_iter().flatten().collect::<Vec<f32>>());
    let status = recorder.echo_status();
    let startup = Instant::now();
    // Refuse a contaminated scene before playing audio or spending gateway credits.
    std::thread::sleep(Duration::from_secs(2));
    let baseline_reference_rms = rms(&trace.lock().unwrap().reference);
    let preflight_status = status.lock().unwrap().clone();
    ensure!(
        preflight_status.reason.is_none(),
        "{}",
        preflight_status.reason.unwrap_or_default()
    );
    ensure!(
        baseline_reference_rms < 0.0001,
        "Other system audio is active (reference RMS {baseline_reference_rms:.6}). Pause other players before --echo-test; no gateway calls were made."
    );
    let music_player = Music(if music == "none" {
        None
    } else {
        Some(Command::new("/usr/bin/afplay").arg(music).spawn()?)
    });
    // Music-only warmup provides a delay estimate without access to clean speech.
    std::thread::sleep(Duration::from_secs(6));
    if music != "none" {
        let warm_status = status.lock().unwrap().clone();
        ensure!(
            warm_status.active,
            "{}",
            warm_status.reason.unwrap_or_else(|| "No confident speaker-to-microphone delay was measured; plain mic kept. No gateway calls were made.".into())
        );
    }
    let playback = Playback::start(&speech)?;
    let deadline = Instant::now() + Duration::from_secs_f64(speech.len() as f64 / RATE as f64 + 3.);
    while !playback.done.load(Ordering::Relaxed) {
        ensure!(Instant::now() < deadline, "speech playback stalled");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_secs(4));
    let clean = recorder.finish();
    let live = feed_worker
        .join()
        .map_err(|_| anyhow::anyhow!("live feed collector stopped"))?;
    ensure!(live == clean, "live chunks differ from the full recording");
    drop(music_player);
    let trace = trace.lock().unwrap();
    ensure!(
        trace.mic.len() == clean.len() && clean.iter().all(|v| v.is_finite()),
        "invalid recorded sample counts/output"
    );
    let first = trace.first_host_time.context("no microphone samples")?;
    let speech_time = f64::from_bits(playback.first_time.load(Ordering::Relaxed));
    let offset = ((speech_time - first).max(0.) * RATE as f64).round() as usize;
    let end = (offset + speech.len() + RATE as usize / 5).min(clean.len());
    ensure!(
        end > offset && offset > 3 * RATE as usize,
        "invalid speech timing"
    );
    let out = output_dir(if music == "none" { "no-music" } else { "music" })?;
    write_wav(out.join("off.wav"), &trace.mic)?;
    write_wav(out.join("on.wav"), &clean)?;
    write_wav(out.join("reference.wav"), &trace.reference)?;
    let music_only = 3 * RATE as usize..offset.saturating_sub(RATE as usize / 5);
    let reduction = db(rms(&trace.mic[music_only.clone()]) / rms(&clean[music_only.clone()]));
    let speech_change = db(rms(&clean[offset..end]) / rms(&trace.mic[offset..end]));
    let duration = clean.len() as f64 / RATE as f64;
    let mut report = json!({
        "music": music, "model": model, "duration_s": duration, "capture_wall_ms": startup.elapsed().as_millis(),
        "speech_playback_gain": 1.0, "speech_offset_samples": offset,
        "baseline_reference_rms": baseline_reference_rms,
        "live_chunks_equal_full_recording": true,
        "echo": *status.lock().unwrap(), "delay_ms": trace.delay_ms, "delay_correlation": trace.delay_correlation,
        "music_reduction_db": reduction, "speech_region_level_change_db": speech_change,
        "aec_cpu_ms_per_audio_second": trace.cpu_seconds / duration * 1000.,
        "max_reference_age_ms": trace.max_reference_age_ms,
        "mic_missing": trace.mic_missing, "reference_missing": trace.reference_missing,
        "mic_dropped": trace.mic_dropped, "reference_dropped": trace.reference_dropped,
        "mean_worker_buffer_ms": trace.mean_worker_buffer_ms, "max_worker_buffer_ms": trace.max_worker_buffer_ms,
        "aec_algorithm_latency_ms": 14,
        "script": SCRIPT
    });
    std::fs::write(
        out.join("measurement.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!("capture: {}", serde_json::to_string(&report)?);
    if music == "none" {
        ensure!(
            rms(&trace.reference[offset..end]) < 0.0001,
            "The no-music reference is not quiet during helper playback. Another player became active or current-process exclusion failed; no gateway calls were made."
        );
    }
    let key = key::require().map_err(anyhow::Error::msg)?;
    let before = Instant::now();
    let off = transcribe::transcribe(&trace.mic, model, &key).map_err(anyhow::Error::msg)?;
    let off_ms = before.elapsed().as_millis();
    let before = Instant::now();
    let on = transcribe::transcribe(&clean, model, &key).map_err(anyhow::Error::msg)?;
    report["off"] = json!({"text": off, "wer": wer(SCRIPT, &off), "transcribe_ms": off_ms});
    report["on"] =
        json!({"text": on, "wer": wer(SCRIPT, &on), "transcribe_ms": before.elapsed().as_millis()});
    std::fs::write(
        out.join("measurement.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn wer(expected: &str, actual: &str) -> f64 {
    let words = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    };
    let expected = words(expected);
    let actual = words(actual);
    let mut row: Vec<usize> = (0..=actual.len()).collect();
    for (i, word) in expected.iter().enumerate() {
        let mut next = vec![i + 1; actual.len() + 1];
        for (j, got) in actual.iter().enumerate() {
            next[j + 1] = (row[j] + usize::from(word != got))
                .min(row[j + 1] + 1)
                .min(next[j] + 1);
        }
        row = next;
    }
    row[actual.len()] as f64 / expected.len().max(1) as f64
}

fn replay(
    mic: &[f32],
    reference: &[f32],
    delay: Option<u16>,
    noise: Noise,
) -> Result<(Vec<f32>, f64)> {
    let mut aec = EchoCanceller::new(delay, noise)?;
    let mut output = Vec::with_capacity(mic.len());
    let started = thread_cpu_seconds();
    for (capture, render) in mic.chunks(FRAME).zip(reference.chunks(FRAME)) {
        let mut c = [0.; FRAME];
        let mut r = [0.; FRAME];
        c[..capture.len()].copy_from_slice(capture);
        r[..render.len()].copy_from_slice(render);
        output.extend_from_slice(&aec.process(&c, &r)?[..capture.len()]);
    }
    Ok((output, thread_cpu_seconds() - started))
}

fn speech_metrics(target: &[f32], signal: &[f32], lag: usize) -> (f64, f64) {
    let (mut ss, mut yy, mut sy) = (0., 0., 0.);
    for i in (0..target.len().saturating_sub(lag)).step_by(4) {
        if target[i].abs() < 0.003 {
            continue;
        }
        let s = target[i] as f64;
        let y = signal[i + lag] as f64;
        ss += s * s;
        yy += y * y;
        sy += s * y;
    }
    (sy / (ss * yy).sqrt().max(1e-20), sy / ss.max(1e-20))
}

pub fn offline(mic: &str, reference: &str, target: &str) -> Result<()> {
    let mic = load_wav(Path::new(mic)).map_err(anyhow::Error::msg)?;
    let reference = load_wav(Path::new(reference)).map_err(anyhow::Error::msg)?;
    let target = load_wav(Path::new(target)).map_err(anyhow::Error::msg)?;
    ensure!(
        mic.len() == reference.len() && mic.len() == target.len(),
        "offline sample counts differ"
    );
    let calibration_len = (2 * RATE as usize).min(mic.len());
    let (delay, correlation) =
        estimate_delay(&mic[..calibration_len], &reference[..calibration_len])
            .context("no confident acoustic delay estimate")?;
    println!("measured delay={delay}ms, correlation={correlation:.4}");
    let out = output_dir("offline")?;
    let mut reports = Vec::new();
    for (name, hint, noise) in [
        ("external-moderate", Some(delay), Noise::Moderate),
        ("external-low", Some(delay), Noise::Low),
        ("external-off", Some(delay), Noise::Off),
        ("automatic-moderate", None, Noise::Moderate),
    ] {
        let (clean, cpu) = replay(&mic, &reference, hint, noise)?;
        let lag = (0..=1600)
            .step_by(4)
            .max_by(|&a, &b| {
                speech_metrics(&target, &clean, a)
                    .0
                    .total_cmp(&speech_metrics(&target, &clean, b).0)
            })
            .unwrap();
        let (corr, gain) = speech_metrics(&target, &clean, lag);
        // Known synthetic scene has six seconds of initial music without speech.
        let region = 3 * RATE as usize..6 * RATE as usize;
        let report = json!({"name": name, "delay_ms": hint, "erle_db": db(rms(&mic[region.clone()]) / rms(&clean[region])), "speech_correlation": corr, "speech_gain_db": db(gain.abs()), "lag_ms": lag as f64 / 16., "cpu_ms_per_audio_second": cpu / (mic.len() as f64 / RATE as f64) * 1000.});
        println!("{report}");
        write_wav(out.join(format!("{name}.wav")), &clean)?;
        reports.push(report);
    }
    let mut cleaner = Cleaner::new();
    let mut clean = Vec::new();
    let started = thread_cpu_seconds();
    for (capture, render) in mic.chunks(FRAME).zip(reference.chunks(FRAME)) {
        let mut c = [0.; FRAME];
        let mut r = [0.; FRAME];
        c[..capture.len()].copy_from_slice(capture);
        r[..render.len()].copy_from_slice(render);
        clean.extend_from_slice(&cleaner.process(&c, &r)?[..capture.len()]);
    }
    let cpu = thread_cpu_seconds() - started;
    ensure!(
        clean.len() == mic.len() && clean.iter().all(|v| v.is_finite()),
        "live pipeline output is invalid"
    );
    let lag = (0..=1600)
        .step_by(4)
        .max_by(|&a, &b| {
            speech_metrics(&target, &clean, a)
                .0
                .total_cmp(&speech_metrics(&target, &clean, b).0)
        })
        .unwrap();
    let (corr, gain) = speech_metrics(&target, &clean, lag);
    let region = 3 * RATE as usize..6 * RATE as usize;
    let erle = db(rms(&mic[region.clone()]) / rms(&clean[region]));
    let report = json!({"name": "live-estimator", "delay_ms": cleaner.delay, "erle_db": erle, "speech_correlation": corr, "speech_gain_db": db(gain.abs()), "lag_ms": lag as f64 / 16., "cpu_ms_per_audio_second": cpu / (mic.len() as f64 / RATE as f64) * 1000.});
    println!("{report}");
    reports.push(report);
    println!(
        "live estimator delay={:?} correlation={:.4}",
        cleaner.delay, cleaner.correlation
    );
    write_wav(out.join("live-estimator.wav"), &clean)?;
    std::fs::write(
        out.join("comparison.json"),
        serde_json::to_string_pretty(&reports)?,
    )?;
    ensure!(
        corr > 0.75 && erle > 15.,
        "speech preservation or echo reduction failed in the synthetic-room E2E"
    );
    Ok(())
}
