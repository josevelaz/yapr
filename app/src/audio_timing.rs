//! Host-clock audio alignment, adapted from echo-lab's measured capture pipeline.

use anyhow::{Result, ensure};
use std::collections::VecDeque;

pub const RATE: u32 = crate::audio::SAMPLE_RATE;
pub const FRAME: usize = 160;

/// Causal 63-tap low-pass (7.2 kHz), then 48→16 kHz decimation.
pub struct Decimator {
    taps: [f32; 63],
    history: [f32; 63],
    index: usize,
    phase: usize,
}

impl Default for Decimator {
    fn default() -> Self {
        let mut taps = std::array::from_fn(|i| {
            let d = i as f64 - 31.;
            let x = 0.3 * d;
            let sinc = if x.abs() < 1e-9 {
                1.
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            (0.3 * sinc * (0.5 - 0.5 * (2. * std::f64::consts::PI * i as f64 / 62.).cos())) as f32
        });
        let norm = taps.iter().sum::<f32>();
        for t in &mut taps {
            *t /= norm;
        }
        Self {
            taps,
            history: [0.; 63],
            index: 0,
            phase: 0,
        }
    }
}

impl Decimator {
    pub fn process(&mut self, input: &[f32], start_seconds: f64) -> TimedChunk {
        let offset = (3 - self.phase) % 3;
        let mut samples = Vec::with_capacity(input.len() / 3 + 1);
        for &x in input {
            self.history[self.index] = x;
            if self.phase == 0 {
                samples.push(
                    (0..63)
                        .map(|k| self.taps[k] * self.history[(self.index + 63 - k) % 63])
                        .sum(),
                );
            }
            self.index = (self.index + 1) % 63;
            self.phase = (self.phase + 1) % 3;
        }
        TimedChunk {
            start_seconds: start_seconds + (offset as f64 - 31.) / 48_000.,
            samples,
        }
    }
}

#[derive(Debug)]
pub struct TimedChunk {
    pub start_seconds: f64,
    pub samples: Vec<f32>,
}

/// Resamples packets onto a shared host-time grid. Neighboring timestamps correct
/// clock drift; missing intervals stay missing rather than shortening the recording.
#[derive(Default)]
pub struct Timeline {
    chunks: VecDeque<TimedChunk>,
    pub missing_samples: usize,
}

impl Timeline {
    pub fn push(&mut self, chunk: TimedChunk) -> Result<()> {
        ensure!(chunk.start_seconds.is_finite(), "invalid audio timestamp");
        if chunk.samples.is_empty()
            || self
                .chunks
                .back()
                .is_some_and(|last| last.start_seconds >= chunk.start_seconds)
        {
            return Ok(());
        }
        self.chunks.push_back(chunk);
        while self.chunks.len() > 256 {
            self.chunks.pop_front();
        }
        Ok(())
    }
    pub fn first_time(&self) -> Option<f64> {
        self.chunks.front().map(|c| c.start_seconds)
    }
    pub fn last_time(&self) -> Option<f64> {
        self.chunks
            .back()
            .map(|c| c.start_seconds + c.samples.len() as f64 / RATE as f64)
    }
    pub fn frame(&mut self, start: f64) -> [f32; FRAME] {
        while self.chunks.len() > 1 && self.chunks[1].start_seconds <= start {
            self.chunks.pop_front();
        }
        std::array::from_fn(|i| {
            let t = start + i as f64 / RATE as f64;
            let found = self
                .chunks
                .iter()
                .enumerate()
                .find(|(j, c)| t >= c.start_seconds && t < self.end_time(*j));
            let Some((j, chunk)) = found else {
                self.missing_samples += 1;
                return 0.;
            };
            let pos = (t - chunk.start_seconds) * chunk.samples.len() as f64
                / (self.end_time(j) - chunk.start_seconds);
            let k = pos.floor() as usize;
            let x = chunk.samples[k.min(chunk.samples.len() - 1)];
            let next = chunk
                .samples
                .get(k + 1)
                .copied()
                .or_else(|| {
                    self.chunks
                        .get(j + 1)
                        .filter(|c| (c.start_seconds - self.end_time(j)).abs() < 0.0001)
                        .and_then(|c| c.samples.first().copied())
                })
                .unwrap_or(x);
            x + (next - x) * (pos - k as f64) as f32
        })
    }
    fn end_time(&self, index: usize) -> f64 {
        let c = &self.chunks[index];
        let nominal = c.samples.len() as f64 / RATE as f64;
        if let Some(next) = self.chunks.get(index + 1) {
            let actual = next.start_seconds - c.start_seconds;
            if (actual - nominal).abs() < nominal * 0.05 {
                return next.start_seconds;
            }
        }
        c.start_seconds + nominal
    }
}

pub fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / samples.len().max(1) as f64).sqrt()
}
#[cfg(feature = "e2e")]
pub fn db(rms: f64) -> f64 {
    20. * rms.max(1e-12).log10()
}
#[cfg(feature = "e2e")]
pub fn thread_cpu_seconds() -> f64 {
    clock_seconds(libc::CLOCK_THREAD_CPUTIME_ID)
}
/// Same mach-uptime clock as SCK PTS and CPAL CoreAudio capture timestamps.
pub fn host_seconds() -> f64 {
    clock_seconds(libc::CLOCK_UPTIME_RAW)
}
fn clock_seconds(clock: libc::clockid_t) -> f64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: t is writable and both clocks are supported on macOS.
    unsafe {
        libc::clock_gettime(clock, &mut t);
    }
    t.tv_sec as f64 + t.tv_nsec as f64 * 1e-9
}
