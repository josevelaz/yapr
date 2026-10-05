//! Microphone capture and conversion to 16 kHz mono f32 audio.

use std::f64::consts::PI;
#[cfg(feature = "e2e")]
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat};
use crossbeam_channel::{Sender, bounded};

#[cfg(feature = "e2e")]
use crate::audio_timing::thread_cpu_seconds;
use crate::audio_timing::{Decimator, FRAME, TimedChunk, Timeline, host_seconds};
use crate::{echo, system_audio::SystemAudioSource};

pub const SAMPLE_RATE: u32 = 16_000;
/// Recordings stop growing after this many seconds.
pub const MAX_SECONDS: usize = 300;
const MAX_SAMPLES: usize = MAX_SECONDS * SAMPLE_RATE as usize;

/// Streaming windowed-sinc resampler for mono audio.
pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// Filter cutoff as a fraction of the input Nyquist rate.
    cutoff: f64,
    half_taps: usize,
    history: Vec<f32>,
    /// Position of the next output sample, in `history` indices.
    position: f64,
}

impl Resampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        let step = input_rate as f64 / output_rate as f64;
        let half_taps = 24;
        Self {
            step,
            cutoff: (1.0 / step).min(1.0) * 0.92,
            half_taps,
            // Leading zeros let the first outputs read a full filter window.
            history: vec![0.0; half_taps],
            position: half_taps as f64,
        }
    }

    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if self.step == 1.0 {
            output.extend_from_slice(input);
            return;
        }
        self.history.extend_from_slice(input);
        let half = self.half_taps as isize;
        while self.position + (half as f64) < self.history.len() as f64 {
            let center = self.position.floor() as isize;
            let mut sum = 0.0f64;
            for k in (center - half + 1)..=(center + half) {
                let distance = self.position - k as f64;
                sum += self.history[k as usize] as f64 * self.kernel(distance);
            }
            output.push(sum as f32);
            self.position += self.step;
        }
        let consumed = (self.position.floor() as isize - half).max(0) as usize;
        self.history.drain(..consumed);
        self.position -= consumed as f64;
    }

    fn kernel(&self, distance: f64) -> f64 {
        let x = distance * self.cutoff;
        let sinc = if x.abs() < 1e-9 {
            1.0
        } else {
            (PI * x).sin() / (PI * x)
        };
        let window = 0.5 + 0.5 * (PI * distance / self.half_taps as f64).cos();
        self.cutoff * sinc * window
    }
}

/// Reads a WAV file as 16 kHz mono f32 samples.
#[cfg(feature = "e2e")]
pub fn load_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader =
        hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect()
        }
    }
    .map_err(|e| e.to_string())?;
    let mono = to_mono(&interleaved, spec.channels as usize);
    let mut out = Vec::with_capacity(mono.len());
    Resampler::new(spec.sample_rate, SAMPLE_RATE).process(&mono, &mut out);
    Ok(out)
}

fn to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

struct Shared {
    samples: Mutex<Vec<f32>>,
    /// Peak level of the latest callback, as f32 bits.
    level: AtomicU32,
    echo: Arc<Mutex<echo::Status>>,
    #[cfg(feature = "e2e")]
    trace: Option<Arc<Mutex<echo::Trace>>>,
}

/// A running microphone capture. The cpal stream is not `Send`, so it lives on its own thread.
pub struct Recorder {
    shared: Arc<Shared>,
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
    chunks: Option<mpsc::Receiver<Vec<f32>>>,
}

impl Recorder {
    pub fn start() -> Result<Self, String> {
        Self::start_inner(
            false,
            #[cfg(feature = "e2e")]
            None,
        )
    }

    pub fn start_with_echo() -> Result<Self, String> {
        Self::start_inner(
            true,
            #[cfg(feature = "e2e")]
            None,
        )
    }

    /// E2E diagnostics only; normal dictation never retains raw mic/reference audio.
    #[cfg(feature = "e2e")]
    pub fn start_measurement(trace: Arc<Mutex<echo::Trace>>) -> Result<Self, String> {
        Self::start_inner(true, Some(trace))
    }

    fn start_inner(
        remove_computer_audio: bool,
        #[cfg(feature = "e2e")] trace: Option<Arc<Mutex<echo::Trace>>>,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            samples: Mutex::new(Vec::new()),
            level: AtomicU32::new(0),
            echo: Arc::new(Mutex::new(echo::Status::default())),
            #[cfg(feature = "e2e")]
            trace,
        });
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (chunks_tx, chunks_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let thread_shared = shared.clone();

        let thread = std::thread::Builder::new()
            .name("audio-worker".into())
            .spawn(move || {
                record(
                    thread_shared,
                    chunks_tx,
                    stop_rx,
                    ready_tx,
                    remove_computer_audio,
                )
            })
            .map_err(|e| e.to_string())?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shared,
                stop: stop_tx,
                thread: Some(thread),
                chunks: Some(chunks_rx),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => Err("microphone thread exited".into()),
        }
    }

    /// Takes the one live feed of 16 kHz mono f32 chunks. Drop it when not streaming.
    /// Chunks concatenate to the full recording returned by finish(), capped at five minutes.
    /// finish() drains the worker and closes the channel. Echo cancellation, when enabled,
    /// happens inside the worker before both this feed and the saved recording.
    pub fn take_chunks(&mut self) -> Option<mpsc::Receiver<Vec<f32>>> {
        self.chunks.take()
    }

    /// Peak level of the most recent audio, 0.0 to 1.0.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    pub fn echo_status(&self) -> Arc<Mutex<echo::Status>> {
        self.shared.echo.clone()
    }

    /// Stops the microphone and returns everything recorded.
    pub fn finish(mut self) -> Vec<f32> {
        self.close();
        std::mem::take(&mut *self.shared.samples.lock().unwrap())
    }

    fn close(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.close();
    }
}

struct RawMic {
    time: f64,
    samples: Vec<f32>,
}

fn open_stream(
    tx: Sender<RawMic>,
    #[cfg(feature = "e2e")] dropped: Arc<std::sync::atomic::AtomicUsize>,
) -> Result<(cpal::Stream, usize, u32), String> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or("No microphone found.")?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("Microphone unavailable: {e}"))?;
    let channels = supported.channels() as usize;
    let rate = supported.sample_rate();
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let stream = match format {
        SampleFormat::F32 => build::<f32>(
            &device,
            config,
            tx,
            #[cfg(feature = "e2e")]
            dropped,
        ),
        SampleFormat::I16 => build::<i16>(
            &device,
            config,
            tx,
            #[cfg(feature = "e2e")]
            dropped,
        ),
        SampleFormat::I32 => build::<i32>(
            &device,
            config,
            tx,
            #[cfg(feature = "e2e")]
            dropped,
        ),
        other => return Err(format!("Unsupported microphone sample format {other}.")),
    }
    .map_err(|e| format!("Could not open the microphone: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("Could not start the microphone: {e}"))?;
    Ok((stream, channels, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    tx: Sender<RawMic>,
    #[cfg(feature = "e2e")] dropped: Arc<std::sync::atomic::AtomicUsize>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample,
    f32: FromSample<T>,
{
    device.build_input_stream(
        config,
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            let time = info
                .timestamp()
                .capture
                .duration_since(cpal::StreamInstant::ZERO)
                .as_secs_f64();
            let result = tx.try_send(RawMic {
                time,
                samples: data.iter().map(|s| s.to_sample::<f32>()).collect(),
            });
            #[cfg(feature = "e2e")]
            if result.is_err() {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
            #[cfg(not(feature = "e2e"))]
            let _ = result;
        },
        // No formatting, logging, shared lock, resampling or blocking send on callbacks.
        |_| {},
        None,
    )
}

fn emit(shared: &Shared, tx: &mpsc::Sender<Vec<f32>>, output: &[f32]) {
    let mut samples = shared.samples.lock().unwrap();
    let count = output.len().min(MAX_SAMPLES.saturating_sub(samples.len()));
    if count > 0 {
        samples.extend_from_slice(&output[..count]);
        let _ = tx.send(output[..count].to_vec());
    }
}

fn fallback(shared: &Shared, source: &mut Option<SystemAudioSource>, reason: String) {
    *source = None;
    let mut status = shared.echo.lock().unwrap();
    status.active = false;
    if status.reason.is_none() {
        status.reason = Some(if reason.starts_with("Computer audio not removed:") {
            reason
        } else {
            format!("Computer audio not removed: {reason}")
        });
    }
}

fn record(
    shared: Arc<Shared>,
    chunks: mpsc::Sender<Vec<f32>>,
    stop: mpsc::Receiver<()>,
    ready: mpsc::Sender<Result<(), String>>,
    remove_computer_audio: bool,
) {
    let (tx, rx) = bounded(64);
    #[cfg(feature = "e2e")]
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (stream, channels, rate) = match open_stream(
        tx,
        #[cfg(feature = "e2e")]
        dropped.clone(),
    ) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let mut stream = Some(stream);
    let mut source = remove_computer_audio.then(SystemAudioSource::start);
    let _ = ready.send(Ok(()));
    let mut cleaner = echo::Cleaner::new();
    let mut mic = Timeline::default();
    let mut reference = Timeline::default();
    let mut decimator = Decimator::default();
    let mut resampler = Resampler::new(rate, SAMPLE_RATE);
    let mut expected: Option<f64> = None;
    let mut converted_time = 0.;
    let mut next_time = None;
    let mut stopped = false;
    let started = Instant::now();
    let mut last_reference = started;
    #[cfg(feature = "e2e")]
    let mut cpu = 0.;
    #[cfg(feature = "e2e")]
    let mut max_age: f64 = 0.;
    #[cfg(feature = "e2e")]
    let mut buffer_sum = 0.;
    #[cfg(feature = "e2e")]
    let mut buffer_max: f64 = 0.;
    #[cfg(feature = "e2e")]
    let mut frame_count = 0;
    loop {
        if !stopped && stop.try_recv().is_ok() {
            stream.take();
            stopped = true;
        }
        let mut tap_error = None;
        if let Some(tap) = &source {
            for chunk in tap.chunks.try_iter() {
                match chunk {
                    Ok(chunk) => {
                        #[cfg(feature = "e2e")]
                        {
                            max_age = max_age.max((host_seconds() - chunk.start_seconds) * 1000.);
                        }
                        if let Err(error) = reference.push(chunk) {
                            tap_error = Some(error.to_string());
                            break;
                        }
                        last_reference = Instant::now();
                    }
                    Err(error) => {
                        tap_error = Some(error.to_string());
                        break;
                    }
                }
            }
            if last_reference.elapsed() > Duration::from_secs(3) {
                tap_error = Some("system audio capture stalled; check Screen & System Audio Recording permission".into());
            }
        }
        if let Some(error) = tap_error {
            fallback(&shared, &mut source, error);
        }
        for raw in rx.try_iter() {
            let mono = to_mono(&raw.samples, channels);
            shared.level.store(
                mono.iter().fold(0.0f32, |m, x| m.max(x.abs())).to_bits(),
                Ordering::Relaxed,
            );
            if expected.is_none_or(|time| (time - raw.time).abs() > 0.003) {
                decimator = Decimator::default();
                resampler = Resampler::new(rate, SAMPLE_RATE);
                converted_time = raw.time;
            }
            expected = Some(raw.time + mono.len() as f64 / rate as f64);
            let chunk = if rate == 48_000 {
                decimator.process(&mono, raw.time)
            } else {
                let mut samples = Vec::new();
                resampler.process(&mono, &mut samples);
                let time = converted_time;
                converted_time += samples.len() as f64 / SAMPLE_RATE as f64;
                TimedChunk {
                    start_seconds: time,
                    samples,
                }
            };
            if let Err(error) = mic.push(chunk) {
                eprintln!("microphone timestamp: {error}");
            }
        }
        if next_time.is_none() {
            next_time = mic.first_time();
        }
        while let (Some(time), Some(end)) = (next_time, mic.last_time()) {
            if time >= end || (!stopped && time + 0.010 > end) {
                break;
            }
            // SCK measured packet lateness reaches 139 ms. Wait off callbacks.
            if source.is_some() && host_seconds() < time + 0.160 {
                break;
            }
            let capture = mic.frame(time);
            let missing_before = reference.missing_samples;
            let render = reference.frame(time);
            #[cfg(feature = "e2e")]
            let cpu_started = thread_cpu_seconds();
            let output = if source.is_some() && reference.missing_samples == missing_before {
                match cleaner.process(&capture, &render) {
                    Ok(clean) => {
                        shared.echo.lock().unwrap().active = cleaner.delay.is_some();
                        clean
                    }
                    Err(error) => {
                        fallback(
                            &shared,
                            &mut source,
                            format!("echo processing failed: {error}"),
                        );
                        capture
                    }
                }
            } else {
                capture
            };
            #[cfg(feature = "e2e")]
            {
                cpu += thread_cpu_seconds() - cpu_started;
            }
            let count = if stopped {
                (((end - time) * SAMPLE_RATE as f64).round() as usize).min(FRAME)
            } else {
                FRAME
            };
            emit(&shared, &chunks, &output[..count]);
            #[cfg(feature = "e2e")]
            {
                let buffer_ms = (host_seconds() - time) * 1000.;
                buffer_sum += buffer_ms;
                buffer_max = buffer_max.max(buffer_ms);
                frame_count += 1;
            }
            #[cfg(feature = "e2e")]
            if let Some(trace) = &shared.trace {
                let mut trace = trace.lock().unwrap();
                trace.first_host_time.get_or_insert(time);
                if trace.mic.len() < MAX_SAMPLES {
                    trace.mic.extend_from_slice(&capture[..count]);
                    trace.reference.extend_from_slice(&render[..count]);
                }
            }
            next_time = Some(time + 0.010);
        }
        if stopped
            && next_time
                .zip(mic.last_time())
                .is_none_or(|(time, end)| time >= end)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    #[cfg(feature = "e2e")]
    if let Some(trace) = &shared.trace {
        let mut trace = trace.lock().unwrap();
        trace.cpu_seconds = cpu;
        trace.delay_ms = cleaner.delay;
        trace.delay_correlation = cleaner.correlation;
        trace.mic_missing = mic.missing_samples;
        trace.reference_missing = reference.missing_samples;
        trace.mic_dropped = dropped.load(Ordering::Relaxed);
        trace.reference_dropped = source
            .as_ref()
            .map_or(0, |tap| tap.dropped.load(Ordering::Relaxed));
        trace.max_reference_age_ms = max_age;
        trace.mean_worker_buffer_ms = buffer_sum / frame_count.max(1) as f64;
        trace.max_worker_buffer_ms = buffer_max;
    }
}
