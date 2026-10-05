//! ScreenCaptureKit reference tap. Callbacks retain buffers and try_send only.
//! Setup and shutdown run separately from microphone capture and AEC.

use anyhow::{Context, Result, ensure};
use crossbeam_channel::{Receiver, Sender, bounded};
use screencapturekit::{cm::CMSampleBuffer, prelude::*};
#[cfg(feature = "e2e")]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;

use crate::audio_timing::{Decimator, TimedChunk};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}
const PERMISSION: &str =
    "Computer audio not removed: allow Screen & System Audio Recording for Yapr";
#[cfg(feature = "e2e")]
static TEST_DENIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Explicit signed-app E2E mode only. Never changes TCC or the user's settings.
#[cfg(feature = "e2e")]
pub fn simulate_permission_denial() {
    TEST_DENIED.store(true, Ordering::Relaxed);
}

pub struct SystemAudioSource {
    pub chunks: Receiver<Result<TimedChunk>>,
    #[cfg(feature = "e2e")]
    pub dropped: Arc<AtomicUsize>,
    stop: Sender<()>,
}

impl SystemAudioSource {
    pub fn start() -> Self {
        let (tx, chunks) = bounded(128);
        let (stop, stop_rx) = bounded(1);
        #[cfg(feature = "e2e")]
        let dropped = Arc::new(AtomicUsize::new(0));
        #[cfg(feature = "e2e")]
        let counter = dropped.clone();
        #[cfg(feature = "e2e")]
        if TEST_DENIED.load(Ordering::Relaxed) {
            let _ = tx.try_send(Err(anyhow::anyhow!(PERMISSION)));
            return Self {
                chunks,
                dropped,
                stop,
            };
        }
        // No permission prompt and no synchronous SCK operation on the mic worker.
        let worker_tx = tx.clone();
        if thread::Builder::new()
            .name("system-audio".into())
            .spawn(move || {
                if let Err(error) = capture(
                    &worker_tx,
                    stop_rx,
                    #[cfg(feature = "e2e")]
                    counter,
                ) {
                    let _ = worker_tx.try_send(Err(error));
                }
            })
            .is_err()
        {
            let _ = tx.try_send(Err(anyhow::anyhow!(
                "Computer audio not removed: could not start capture thread"
            )));
        }
        Self {
            chunks,
            #[cfg(feature = "e2e")]
            dropped,
            stop,
        }
    }
}

impl Drop for SystemAudioSource {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
    }
}

fn capture(
    tx: &Sender<Result<TimedChunk>>,
    stop: Receiver<()>,
    #[cfg(feature = "e2e")] dropped: Arc<AtomicUsize>,
) -> Result<()> {
    // SAFETY: this CoreGraphics preflight has no pointer arguments or side effects.
    ensure!(unsafe { CGPreflightScreenCaptureAccess() }, PERMISSION);
    let content = SCShareableContent::get()
        .context("Computer audio not removed: ScreenCaptureKit could not list displays")?;
    if stop.try_recv().is_ok() {
        return Ok(());
    }
    let displays = content.displays();
    let display = displays
        .first()
        .context("Computer audio not removed: no display")?;
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build()?;
    let config = SCStreamConfiguration::new()
        .with_width(2)
        .with_height(2)
        .with_fps(1)
        .with_captures_audio(true)
        .with_excludes_current_process_audio(true)
        .with_sample_rate(48_000)
        .with_channel_count(2);
    let mut stream = SCStream::new(&filter, &config)?;
    let (raw_tx, raw_rx) = bounded(64);
    #[cfg(feature = "e2e")]
    let counter = dropped.clone();
    stream.add_output_handler(
        move |sample: CMSampleBuffer, kind| {
            if kind == SCStreamOutputType::Audio {
                let result = raw_tx.try_send(sample);
                #[cfg(feature = "e2e")]
                if result.is_err() {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
                #[cfg(not(feature = "e2e"))]
                let _ = result;
            }
        },
        SCStreamOutputType::Audio,
    )?;
    stream
        .start_capture()
        .context("Computer audio not removed: ScreenCaptureKit could not start")?;
    let mut decimator = Decimator::default();
    let mut expected: Option<f64> = None;
    let result = loop {
        crossbeam_channel::select! {
            recv(stop) -> _ => break Ok(()),
            recv(raw_rx) -> sample => {
                let Ok(sample) = sample else { break Err(anyhow::anyhow!("Computer audio not removed: capture ended")); };
                match decode(&sample) {
                    Ok((time, mono)) => {
                        if expected.is_some_and(|e| (e - time).abs() > 0.003) { decimator = Decimator::default(); }
                        expected = Some(time + mono.len() as f64 / 48_000.);
                        let result = tx.try_send(Ok(decimator.process(&mono, time)));
                        #[cfg(feature = "e2e")]
                        if result.is_err() { dropped.fetch_add(1, Ordering::Relaxed); }
                        #[cfg(not(feature = "e2e"))]
                        let _ = result;
                    }
                    Err(error) => break Err(error),
                }
            }
        }
    };
    let _ = stream.stop_capture();
    result
}

fn decode(sample: &CMSampleBuffer) -> Result<(f64, Vec<f32>)> {
    let format = sample
        .format_description()
        .context("missing system audio format")?;
    ensure!(
        format.audio_sample_rate() == Some(48_000.)
            && format.audio_channel_count() == Some(2)
            && format.audio_is_float()
            && !format.audio_is_big_endian()
            && format.audio_bits_per_channel() == Some(32),
        "unsupported ScreenCaptureKit audio format"
    );
    let pts = sample.presentation_timestamp();
    ensure!(pts.timescale > 0, "invalid system audio PTS");
    let buffers = sample
        .audio_buffer_list()
        .map_err(|e| anyhow::anyhow!("system audio buffer error {e}"))?;
    let mut mono = Vec::new();
    for buffer in buffers.iter() {
        let channels = buffer.number_channels as usize;
        ensure!(
            channels > 0 && buffer.data().len() % (4 * channels) == 0,
            "malformed system audio buffer"
        );
        let values: Vec<f32> = buffer
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_ne_bytes(*b))
            .collect();
        let frames = values.len() / channels;
        if mono.is_empty() {
            mono.resize(frames, 0.);
        }
        ensure!(mono.len() == frames, "mismatched stereo planes");
        for (i, frame) in values.chunks_exact(channels).enumerate() {
            mono[i] += frame.iter().sum::<f32>() / 2.;
        }
    }
    Ok((pts.value as f64 / pts.timescale as f64, mono))
}
