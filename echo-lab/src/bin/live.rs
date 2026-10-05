use anyhow::{Context, Result, bail, ensure};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::bounded;
use echo_lab::{EchoCanceller, RATE, SystemAudioTap, audio::*};
use std::{
    fs,
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let seconds: f64 = args
        .first()
        .context("usage: live SECONDS [--request-permission] [--delay-ms MS]")?
        .parse()?;
    ensure!((1.0..=300.).contains(&seconds), "SECONDS must be 1..300");
    let delay = args
        .iter()
        .position(|a| a == "--delay-ms")
        .map(|i| {
            args.get(i + 1)
                .context("missing delay value")?
                .parse::<u16>()
                .map_err(anyhow::Error::from)
        })
        .transpose()?;
    if !SystemAudioTap::permission_granted() && args.iter().any(|a| a == "--request-permission") {
        eprintln!(
            "Requesting macOS Screen & System Audio Recording access (CGRequestScreenCaptureAccess). The launching app/terminal or live binary needs approval. This run will stop rather than wait for a prompt."
        );
        SystemAudioTap::request_permission()?;
    }
    let mut tap = SystemAudioTap::start()?;
    let host = cpal::default_host();
    println!(
        "output={}",
        host.default_output_device()
            .context("no default output")?
            .name()?
    );
    let device = host
        .default_input_device()
        .context("no default microphone")?;
    let config = device.default_input_config()?;
    ensure!(
        config.sample_rate().0 == 48_000 && config.sample_format() == cpal::SampleFormat::F32,
        "prototype needs a 48 kHz f32 microphone, got {config:?}"
    );
    println!(
        "mic={} config={config:?}; system=48kHz stereo; delay hint={delay:?}",
        device.name()?
    );
    let channels = config.channels() as usize;
    let (tx, rx) = bounded::<TimedChunk>(64);
    let (err_tx, err_rx) = bounded(1);
    let dropped = Arc::new(AtomicUsize::new(0));
    let counter = dropped.clone();
    let stream = device
        .build_input_stream(
            &config.into(),
            move |data: &[f32], info: &cpal::InputCallbackInfo| {
                let time = info
                    .timestamp()
                    .capture
                    .duration_since(&cpal::StreamInstant::new(0, 0))
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(f64::NAN);
                // Copy only: downmix/resample/AEC happen on this binary's worker thread.
                if tx
                    .try_send(TimedChunk {
                        start_seconds: time,
                        samples: data.to_vec(),
                    })
                    .is_err()
                {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            },
            move |error| {
                let _ = err_tx.try_send(error.to_string());
            },
            Some(Duration::from_secs(3)),
        )
        .context("microphone start failed; check Privacy & Security > Microphone permission")?;
    stream.play()?;
    println!("READY");
    io::stdout().flush()?;
    fs::create_dir_all("out")?;
    let mut aec = EchoCanceller::with_delay(delay)?;
    let mut decimator = Decimator::default();
    let mut expected = None::<f64>;
    let mut mic_timeline = Timeline::default();
    let mut ref_timeline = Timeline::default();
    let (mut mic, mut reference, mut clean) = (Vec::new(), Vec::new(), Vec::new());
    let mut next_time = None;
    let mut cpu = 0.;
    let started = Instant::now();
    let target_samples = (seconds * RATE as f64) as usize;
    let mut got_reference = false;
    let mut ref_expected = None;
    let mut ref_packets = 0;
    let mut ref_samples = 0;
    let mut ref_gaps = 0;
    while clean.len() < target_samples {
        if let Ok(err) = err_rx.try_recv() {
            bail!("microphone stream failed: {err}");
        }
        for chunk in tap.chunks.try_iter() {
            let chunk = chunk?;
            if ref_packets < 3 {
                println!(
                    "ref packet: frames={} age={:.3}ms",
                    chunk.samples.len(),
                    (host_seconds() - chunk.start_seconds) * 1000.
                );
            }
            if let Some(expected) = ref_expected {
                let gap: f64 = chunk.start_seconds - expected;
                if gap.abs() > 0.003 {
                    if ref_gaps < 3 {
                        println!("ref discontinuity: {gap:.6}s");
                    }
                    ref_gaps += 1;
                }
            }
            ref_expected = Some(chunk.start_seconds + chunk.samples.len() as f64 / RATE as f64);
            ref_packets += 1;
            ref_samples += chunk.samples.len();
            ref_timeline.push(chunk)?;
            got_reference = true;
        }
        for raw in rx.try_iter() {
            let mono: Vec<_> = raw
                .samples
                .chunks_exact(channels)
                .map(|c| c.iter().sum::<f32>() / channels as f32)
                .collect();
            if expected.is_some_and(|t| (t - raw.start_seconds).abs() > 0.003) {
                decimator = Decimator::default();
            }
            expected = Some(raw.start_seconds + mono.len() as f64 / 48_000.);
            mic_timeline.push(decimator.process(&mono, raw.start_seconds))?;
        }
        if next_time.is_none() {
            next_time = mic_timeline.first_time();
        }
        if started.elapsed().as_secs_f64() > seconds + 5. {
            bail!("live recording deadline exceeded (microphone permission/callback stall)");
        }
        if started.elapsed() > Duration::from_secs(3) {
            ensure!(
                next_time.is_some(),
                "no microphone samples in 3s: check Microphone permission"
            );
            ensure!(
                got_reference,
                "no system audio samples in 3s: play audio; check Screen & System Audio Recording permission"
            );
        }
        while let Some(t) = next_time {
            // SCK delivers 20 ms packets in bursts, up to ~105 ms late here.
            // 150 ms jitter allowance + a full 10 ms frame, off callbacks.
            if host_seconds() < t + 0.160 || clean.len() >= target_samples {
                break;
            }
            let capture = mic_timeline.frame(t);
            let render = ref_timeline.frame(t);
            let cpu_start = thread_cpu_seconds();
            clean.extend(aec.process(&capture, &render)?);
            cpu += thread_cpu_seconds() - cpu_start;
            mic.extend(capture);
            reference.extend(render);
            next_time = Some(t + 0.010);
        }
        thread::sleep(Duration::from_millis(2));
    }
    stream.pause()?;
    tap.stop()?;
    mic.truncate(target_samples);
    reference.truncate(target_samples);
    clean.truncate(target_samples);
    write_wav("out/live-mic.wav", &mic)?;
    write_wav("out/live-ref.wav", &reference)?;
    write_wav("out/live-clean.wav", &clean)?;
    println!(
        "RMS mic={:.6} ({:.2}dBFS) ref={:.6} ({:.2}dBFS) clean={:.6} ({:.2}dBFS)",
        rms(&mic),
        db(rms(&mic)),
        rms(&reference),
        db(rms(&reference)),
        rms(&clean),
        db(rms(&clean))
    );
    let warmup = (3 * RATE as usize).min(target_samples - 1);
    println!(
        "after 3s warmup: apparent ERLE/reduction={:.2}dB (valid ERLE only if all sound is far-end); AEC CPU={:.3}ms/audio-second",
        db(rms(&mic[warmup..]) / rms(&clean[warmup..])),
        cpu / seconds * 1000.
    );
    println!(
        "dropped callback/chunks: mic={} ref={}; zero-filled samples: mic={} ref={}",
        dropped.load(Ordering::Relaxed),
        tap.dropped_callbacks.load(Ordering::Relaxed),
        mic_timeline.missing_samples,
        ref_timeline.missing_samples
    );
    println!(
        "ref input: packets={ref_packets}, samples={ref_samples}, timestamp discontinuities={ref_gaps}"
    );
    println!(
        "WARNING: afplay speech is ALSO far-end audio and should cancel; this harness cannot measure real near-end dictation preservation."
    );
    Ok(())
}
