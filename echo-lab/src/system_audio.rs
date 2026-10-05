use crate::audio::{Decimator, TimedChunk};
use anyhow::{Context, Result, bail, ensure};
use crossbeam_channel::{Receiver, bounded};
use screencapturekit::{cm::CMSampleBuffer, prelude::*};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}
const PERMISSION: &str = "Screen & System Audio Recording permission is not granted. macOS System Settings > Privacy & Security > Screen & System Audio Recording: allow the launching terminal/app (or live binary), then quit and relaunch it. Plain microphone recording is the safe fallback.";

pub struct SystemAudioTap {
    stream: SCStream,
    pub chunks: Receiver<Result<TimedChunk>>,
    pub dropped_callbacks: Arc<AtomicUsize>,
    stop: crossbeam_channel::Sender<()>,
    worker: Option<JoinHandle<()>>,
}
impl SystemAudioTap {
    pub fn permission_granted() -> bool {
        // SAFETY: CoreGraphics has no pointer arguments or ownership requirements.
        unsafe { CGPreflightScreenCaptureAccess() }
    }
    /// Requests only on explicit caller action; never waits for a human choice.
    pub fn request_permission() -> Result<()> {
        let (tx, rx) = bounded(1);
        thread::spawn(move || {
            let _ = tx.try_send(unsafe { CGRequestScreenCaptureAccess() });
        });
        if !rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false) {
            bail!(PERMISSION);
        }
        Ok(())
    }
    /// No prompt by default. ScreenCaptureKit v11 synchronous operations have
    /// a built-in 30 s timeout; the live harness adds a shorter process deadline.
    pub fn start() -> Result<Self> {
        ensure!(Self::permission_granted(), PERMISSION);
        let content = SCShareableContent::get().context(
            "ScreenCaptureKit shareable content (check Screen & System Audio Recording permission)",
        )?;
        let displays = content.displays();
        let display = displays.first().context("ScreenCaptureKit: no display")?;
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
        let dropped = Arc::new(AtomicUsize::new(0));
        let counter = dropped.clone();
        let worker_counter = dropped.clone();
        stream.add_output_handler(
            move |sample: CMSampleBuffer, kind| {
                if kind == SCStreamOutputType::Audio && raw_tx.try_send(sample).is_err() {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            },
            SCStreamOutputType::Audio,
        )?;
        let (tx, chunks) = bounded(64);
        let (stop, stop_rx) = bounded(1);
        let worker = thread::spawn(move || {
            let mut decimator = Decimator::default();
            let mut expected: Option<f64> = None;
            loop {
                crossbeam_channel::select! {
                    recv(stop_rx) -> _ => break,
                    recv(raw_rx) -> sample => {
                        let Ok(sample) = sample else { break; };
                        let result = decode(&sample).map(|(time, mono)| {
                            if expected.is_some_and(|e| (e - time).abs() > 0.003) {
                                decimator = Decimator::default();
                            }
                            expected = Some(time + mono.len() as f64 / 48_000.);
                            decimator.process(&mono, time)
                        });
                        if tx.try_send(result).is_err() { worker_counter.fetch_add(1, Ordering::Relaxed); }
                    }
                }
            }
        });
        // worker uses its own Arc; stream starts only after the consumer exists.
        stream
            .start_capture()
            .context("ScreenCaptureKit start_capture")?;
        Ok(Self {
            stream,
            chunks,
            dropped_callbacks: dropped,
            stop,
            worker: Some(worker),
        })
    }
    pub fn stop(&mut self) -> Result<()> {
        let result = self
            .stream
            .stop_capture()
            .context("ScreenCaptureKit stop_capture");
        let _ = self.stop.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        result
    }
}
impl Drop for SystemAudioTap {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.stop();
        }
    }
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
        "unsupported ScreenCaptureKit audio format: {format:?}"
    );
    let pts = sample.presentation_timestamp();
    ensure!(pts.timescale > 0, "invalid system audio PTS");
    let time = pts.value as f64 / pts.timescale as f64;
    let buffers = sample
        .audio_buffer_list()
        .map_err(|e| anyhow::anyhow!("audio_buffer_list OSStatus {e}"))?;
    let mut mono = Vec::new();
    // Handles stereo planar (two mono buffers) and stereo interleaved.
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
    Ok((time, mono))
}
