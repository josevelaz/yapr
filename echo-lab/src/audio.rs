use crate::{FRAME, RATE};
use anyhow::{Result, ensure};
use std::{collections::VecDeque, path::Path};

pub fn read_wav(path: impl AsRef<Path>) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => reader
            .samples::<i32>()
            .map(|s| s.map(|s| s as f32 / (1u64 << (spec.bits_per_sample - 1)) as f32))
            .collect::<Result<_, _>>()?,
    };
    let mono: Vec<_> = samples
        .chunks_exact(spec.channels as usize)
        .map(|ch| ch.iter().sum::<f32>() / ch.len() as f32)
        .collect();
    Ok(resample(&mono, spec.sample_rate, RATE))
}

pub fn write_wav(path: impl AsRef<Path>, samples: &[f32]) -> Result<()> {
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

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.
    } else {
        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
    }
}

/// Windowed-sinc conversion for fixtures (no callback use).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    let cutoff = (to as f64 / from as f64).min(1.) * 0.9;
    (0..input.len() * to as usize / from as usize)
        .map(|i| {
            let pos = i as f64 * from as f64 / to as f64;
            let center = pos.floor() as isize;
            let (mut sum, mut norm) = (0., 0.);
            for j in center - 32..=center + 32 {
                let distance = j as f64 - pos;
                if distance.abs() > 32. {
                    continue;
                }
                let weight = cutoff
                    * sinc(distance * cutoff)
                    * (0.5 + 0.5 * (std::f64::consts::PI * distance / 32.).cos());
                norm += weight;
                if j >= 0 && (j as usize) < input.len() {
                    sum += weight * input[j as usize] as f64;
                }
            }
            (sum / norm) as f32
        })
        .collect()
}

/// Causal 63-tap low-pass (7.2 kHz) then decimate by 3. State spans callbacks.
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
            (0.3 * sinc(0.3 * d) * (0.5 - 0.5 * (2. * std::f64::consts::PI * i as f64 / 62.).cos()))
                as f32
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

/// Bounded timestamp timeline. Sample onto a common host-time 16 kHz grid.
/// Adjacent packet timestamps determine the actual rate (up to 5% deviation),
/// so separate device clocks cannot cause an ever-growing FIFO or delay.
/// Gaps are zero-filled and counted, never concatenated away.
#[derive(Default)]
pub struct Timeline {
    chunks: VecDeque<TimedChunk>,
    pub missing_samples: usize,
}
impl Timeline {
    pub fn push(&mut self, chunk: TimedChunk) -> Result<()> {
        ensure!(chunk.start_seconds.is_finite(), "invalid audio timestamp");
        if chunk.samples.is_empty() {
            return Ok(());
        }
        if self
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
    pub fn frame(&mut self, start: f64) -> [f32; FRAME] {
        while self.chunks.len() > 1 && self.chunks[1].start_seconds <= start {
            self.chunks.pop_front();
        }
        std::array::from_fn(|i| {
            let t = start + i as f64 / RATE as f64;
            let found = self.chunks.iter().enumerate().find(|(j, c)| {
                let end = self.end_time(*j);
                t >= c.start_seconds && t < end
            });
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
pub fn db(rms: f64) -> f64 {
    20. * rms.max(1e-12).log10()
}

/// CPU time for the current worker thread, excluding sleep / other threads.
pub fn thread_cpu_seconds() -> f64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: t is writable; macOS supports this clock.
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t);
    }
    t.tv_sec as f64 + t.tv_nsec as f64 * 1e-9
}

/// Same mach-uptime clock as ScreenCaptureKit PTS and CPAL's CoreAudio backend.
pub fn host_seconds() -> f64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_UPTIME_RAW, &mut t);
    }
    t.tv_sec as f64 + t.tv_nsec as f64 * 1e-9
}
