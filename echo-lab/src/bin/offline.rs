//! Deterministic end-to-end experiment; generated audio and metrics are artifacts.
use anyhow::{Result, ensure};
use echo_lab::{EchoCanceller, FRAME, RATE, audio::*};
use std::{f64::consts::TAU, fs};

fn noise(state: &mut u32) -> f64 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    *state as f64 / u32::MAX as f64 * 2. - 1.
}
fn interference(len: usize, speech_like: bool) -> Vec<f32> {
    let mut seed = 42;
    let mut phase = 0.;
    (0..len)
        .map(|i| {
            let t = i as f64 / RATE as f64;
            let n = noise(&mut seed);
            if speech_like {
                // Continuous changing voiced vowels + breath/consonant bursts.
                let syllable = (t * 4.).fract();
                let envelope = (std::f64::consts::PI * syllable).sin().powi(2);
                let f0 = 125. + 35. * (t * 1.7).sin();
                phase += TAU * f0 / RATE as f64;
                let vowels = [(700., 1200.), (300., 2300.), (500., 1700.), (350., 800.)];
                let (f1, f2) = vowels[(t * 4.) as usize % vowels.len()];
                let voiced = (1..40)
                    .map(|h| {
                        let f = f0 * h as f64;
                        let weight = 0.5 * (-(f - f1).powi(2) / 30_000.).exp()
                            + 0.3 * (-(f - f2).powi(2) / 80_000.).exp()
                            + 0.06 / h as f64;
                        weight * (phase * h as f64).sin()
                    })
                    .sum::<f64>();
                (0.24 * voiced * envelope + 0.09 * n * (-syllable * 30.).exp()) as f32
            } else {
                let root = [130.81, 174.61, 146.83, 196.][(t / 2.) as usize % 4];
                let chord = [1., 1.259921, 1.498307]
                    .iter()
                    .map(|ratio| {
                        (1..5)
                            .map(|h| (TAU * root * ratio * h as f64 * t).sin() / h as f64)
                            .sum::<f64>()
                    })
                    .sum::<f64>()
                    * 0.06;
                let beat = (t * 2.).fract();
                let kick = (TAU * (55. * t - 0.05 * (-beat * 25.).exp())).sin()
                    * (-beat * 16.).exp()
                    * 0.16;
                let snare = n * (-((t * 2. + 0.5).fract()) * 24.).exp() * 0.16;
                let hat = n * (-(t * 8.).fract() * 35.).exp() * 0.04;
                (chord + kick + snare + hat) as f32
            }
        })
        .collect()
}
fn region(signal: &[f32], mask: &[bool], active: bool) -> Vec<f32> {
    signal
        .iter()
        .zip(mask)
        .filter_map(|(&x, &m)| (m == active).then_some(x))
        .collect()
}
fn metrics(target: &[f32], signal: &[f32], mask: &[bool], lag: usize) -> (f64, f64, f64) {
    let (mut ss, mut yy, mut sy, mut err) = (0., 0., 0., 0.);
    for i in 0..target.len() - lag {
        if !mask[i] {
            continue;
        }
        let s = target[i] as f64;
        let y = signal[i + lag] as f64;
        ss += s * s;
        yy += y * y;
        sy += s * y;
        err += (s - y).powi(2);
    }
    (
        sy / (ss * yy).sqrt().max(1e-20),
        10. * (ss / err.max(1e-20)).log10(),
        sy / ss,
    )
}
fn run(name: &str, speech: &[f32], speech_like: bool, delay: Option<u16>) -> Result<()> {
    let len = 6 * RATE as usize + speech.len() + 4 * RATE as usize;
    let mut target = vec![0.; len];
    target[6 * RATE as usize..6 * RATE as usize + speech.len()].copy_from_slice(speech);
    // Classify entire 10 ms windows, exclude 200 ms edges from echo-only mask.
    let mut active = vec![false; len];
    for (i, frame) in target.chunks(FRAME).enumerate() {
        if rms(frame) > 0.003 {
            active[i * FRAME..(i * FRAME + frame.len())].fill(true);
        }
    }
    let active_rms = rms(&region(&target, &active, true));
    let gain = 0.045 / active_rms;
    for x in &mut target {
        *x *= gain as f32;
    }
    let reference = interference(len, speech_like);
    // 50 ms direct path + reflections at 63, 79 and 97 ms, alternating polarity.
    let ir = [(800, 0.65), (1008, 0.26), (1264, -0.16), (1552, 0.09)];
    let mut echo: Vec<f32> = (0..len)
        .map(|i| {
            ir.iter()
                .map(|&(d, g)| if i >= d { reference[i - d] * g } else { 0. })
                .sum()
        })
        .collect();
    let echo_gain = 10f64.powf(6. / 20.) * rms(&region(&target, &active, true))
        / rms(&region(&echo, &active, true));
    for x in &mut echo {
        *x *= echo_gain as f32;
    }
    let mut seed = 123;
    let mic: Vec<f32> = target
        .iter()
        .zip(&echo)
        .map(|(&s, &e)| s + e + noise(&mut seed) as f32 * 0.0005)
        .collect();
    ensure!(mic.iter().all(|x| x.abs() < 1.), "synthetic input clips");
    let mut aec = EchoCanceller::with_delay(delay)?;
    let mut clean = Vec::new();
    let mut cpu = 0.;
    // Non-frame-sized chunks exercise residual buffering and flush.
    let mut cursor = 0;
    while cursor < len {
        let end = (cursor + [113, 479, 257][cursor % 3]).min(len);
        let started = thread_cpu_seconds();
        clean.extend(aec.process(&mic[cursor..end], &reference[cursor..end])?);
        cpu += thread_cpu_seconds() - started;
        cursor = end;
    }
    let started = thread_cpu_seconds();
    clean.extend(aec.flush()?);
    cpu += thread_cpu_seconds() - started;
    ensure!(
        clean.len() == len && clean.iter().all(|x| x.is_finite()),
        "invalid AEC output"
    );
    let best_lag = (0..=1600)
        .max_by(|&a, &b| {
            metrics(&target, &clean, &active, a)
                .0
                .total_cmp(&metrics(&target, &clean, &active, b).0)
        })
        .unwrap();
    let (before_corr, before_snr, _) = metrics(&target, &mic, &active, 0);
    let (corr, snr, gain) = metrics(&target, &clean, &active, best_lag);
    let mut echo_only = vec![true; len];
    echo_only[..3 * RATE as usize].fill(false); // exclude cold adaptation
    for (i, &is_active) in active.iter().enumerate() {
        if is_active {
            echo_only[i.saturating_sub(3200)..(i + 3200).min(len)].fill(false);
        }
    }
    let echo_in = rms(&region(&echo, &echo_only, true));
    let mic_only = rms(&region(&mic, &echo_only, true));
    let clean_only = rms(&region(&clean, &echo_only, true));
    let erle = db(echo_in / clean_only);
    println!(
        "{name}: duration={:.3}s room_gain={echo_gain:.3} echo/speech=+6.00dB (active regions), delay={delay:?}",
        len as f64 / RATE as f64
    );
    println!(
        "  echo-only: ERLE={erle:.2}dB (echo / total residual incl. noise); RMS mic={mic_only:.6} ({:.2}dBFS) clean={clean_only:.6} ({:.2}dBFS), measured {:.2}s after 3s warmup",
        db(mic_only),
        db(clean_only),
        echo_only.iter().filter(|&&x| x).count() as f64 / RATE as f64
    );
    println!(
        "  speech-active: RMS target={:.6} mic={:.6} clean={:.6}",
        rms(&region(&target, &active, true)),
        rms(&region(&mic, &active, true)),
        rms(&region(&clean, &active, true))
    );
    println!(
        "  preservation: correlation {before_corr:.4} -> {corr:.4}; unscaled SNR {before_snr:.2} -> {snr:.2}dB; least-squares speech gain={gain:.4}; output lag={best_lag} samples ({:.2}ms)",
        best_lag as f64 / 16.
    );
    println!(
        "  AEC CPU={:.3}ms/audio-second ({:.3}% of one core), total={cpu:.6}s",
        cpu / (len as f64 / RATE as f64) * 1000.,
        cpu / (len as f64 / RATE as f64) * 100.
    );
    for (suffix, audio) in [
        ("target", &target),
        ("ref", &reference),
        ("echo", &echo),
        ("mic", &mic),
        ("clean", &clean),
    ] {
        write_wav(format!("out/{name}-{suffix}.wav"), audio)?;
    }
    if name == "music" {
        let peak = reference.iter().map(|x| x.abs()).fold(0f32, f32::max);
        let playback: Vec<_> = reference.iter().map(|x| x * 0.65 / peak).collect();
        write_wav("out/play-music.wav", &playback)?;
    }
    ensure!(
        erle > 15.,
        "{name}: ERLE below 15dB; investigate delay/configuration"
    );
    // Report waveform preservation separately from echo reduction.
    Ok(())
}

fn clock_smoke() -> Result<()> {
    for ppm in [-100., 150.] {
        let actual_rate = RATE as f64 * (1. + ppm * 1e-6);
        let mut timeline = Timeline::default();
        let mut cursor = 0;
        let mut packet = 0;
        while cursor < 10 * RATE as usize {
            let len = [117, 483, 231][packet % 3];
            let samples = (cursor..cursor + len)
                .map(|i| (TAU * 440. * i as f64 / actual_rate).sin() as f32)
                .collect();
            timeline.push(TimedChunk {
                start_seconds: cursor as f64 / actual_rate,
                samples,
            })?;
            cursor += len;
            packet += 1;
        }
        // Timeline has bounded retention; inspect its retained tail.
        let start = timeline.first_time().unwrap() + 0.01;
        let mut max_error = 0f32;
        for frame in 0..100 {
            let t = start + frame as f64 * 0.01;
            for (i, sample) in timeline.frame(t).iter().enumerate() {
                let expected = (TAU * 440. * (t + i as f64 / RATE as f64)).sin() as f32;
                max_error = max_error.max((sample - expected).abs());
            }
        }
        ensure!(
            max_error < 0.01 && timeline.missing_samples == 0,
            "clock alignment failed: error={max_error}, missing={}",
            timeline.missing_samples
        );
        println!(
            "timestamp alignment smoke: drift={ppm:+.0}ppm, irregular packets, max error={max_error:.6}, missing={}",
            timeline.missing_samples
        );
    }
    let mut gap = Timeline::default();
    gap.push(TimedChunk {
        start_seconds: 0.,
        samples: vec![1.; FRAME],
    })?;
    gap.push(TimedChunk {
        start_seconds: 0.1,
        samples: vec![1.; FRAME],
    })?;
    ensure!(
        gap.frame(0.05).iter().all(|&x| x == 0.) && gap.missing_samples == FRAME,
        "gap was concatenated away"
    );
    println!("timestamp gap smoke: missing 10ms zero-filled and counted");
    Ok(())
}
fn main() -> Result<()> {
    fs::create_dir_all("out")?;
    clock_smoke()?;
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "../e2e/fixtures/pauses.wav".into());
    let speech = read_wav(path)?;
    let mut control = EchoCanceller::new()?;
    let mut output = control.process(&speech, &vec![0.; speech.len()])?;
    output.extend(control.flush()?);
    let mask = vec![true; speech.len()];
    let lag = (0..=320)
        .max_by(|&a, &b| {
            metrics(&speech, &output, &mask, a)
                .0
                .total_cmp(&metrics(&speech, &output, &mask, b).0)
        })
        .unwrap();
    println!(
        "speech-only control: corr/SNR/gain={:?}, lag={lag}",
        metrics(&speech, &output, &mask, lag)
    );
    write_wav("out/speech-only-control.wav", &output)?;
    run("music", &speech, false, Some(50))?;
    run("speech-like", &speech, true, Some(50))?;
    run("music-adaptive", &speech, false, None)?;
    run("speech-like-adaptive", &speech, true, None)?;
    Ok(())
}
