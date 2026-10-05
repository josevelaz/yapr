//! Conservative AEC: measure render→mic delay before enabling external-delay WebRTC.
//! Never use delay-agnostic suppression while the path is unknown; keep plain audio.

use anyhow::Result;
use serde::Serialize;
use webrtc_audio_processing::{Processor, config};

use crate::audio_timing::{FRAME, RATE, rms};

#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Default)]
#[cfg(feature = "e2e")]
pub struct Trace {
    pub mic: Vec<f32>,
    pub reference: Vec<f32>,
    pub cpu_seconds: f64,
    pub first_host_time: Option<f64>,
    pub delay_ms: Option<u16>,
    pub delay_correlation: f64,
    pub mic_missing: usize,
    pub reference_missing: usize,
    pub mic_dropped: usize,
    pub reference_dropped: usize,
    pub max_reference_age_ms: f64,
    pub mean_worker_buffer_ms: f64,
    pub max_worker_buffer_ms: f64,
}

#[derive(Clone, Copy, Debug)]
pub enum Noise {
    #[cfg(feature = "e2e")]
    Off,
    #[cfg(feature = "e2e")]
    Low,
    Moderate,
}

pub struct EchoCanceller {
    processor: Processor,
}
impl EchoCanceller {
    pub fn new(delay_ms: Option<u16>, _noise: Noise) -> Result<Self> {
        let mut aec3 = webrtc_audio_processing::experimental::EchoCanceller3Config::default();
        aec3.filter.export_linear_aec_output = true;
        aec3.delay.use_external_delay_estimator = delay_ms.is_some();
        aec3.erle.max_l = 100.;
        aec3.erle.max_h = 100.;
        aec3.suppressor.normal_tuning.mask_lf.enr_transparent = 5.;
        aec3.suppressor.normal_tuning.mask_lf.enr_suppress = 6.;
        aec3.suppressor.normal_tuning.mask_lf.emr_transparent = 0.5;
        aec3.suppressor.normal_tuning.mask_hf = aec3.suppressor.normal_tuning.mask_lf;
        aec3.suppressor.nearend_tuning = aec3.suppressor.normal_tuning;
        let processor = Processor::with_aec3_config(RATE, aec3)?;
        #[cfg(not(feature = "e2e"))]
        let noise_suppression = Some(config::NoiseSuppression {
            level: config::NoiseSuppressionLevel::Moderate,
            analyze_linear_aec_output: true,
        });
        #[cfg(feature = "e2e")]
        let noise_suppression = match _noise {
            Noise::Off => None,
            _ => Some(config::NoiseSuppression {
                level: match _noise {
                    Noise::Low => config::NoiseSuppressionLevel::Low,
                    _ => config::NoiseSuppressionLevel::Moderate,
                },
                analyze_linear_aec_output: true,
            }),
        };
        processor.set_config(config::Config {
            echo_canceller: Some(config::EchoCanceller::Full {
                stream_delay_ms: delay_ms,
            }),
            noise_suppression,
            high_pass_filter: Some(config::HighPassFilter::default()),
            gain_controller: None,
            ..Default::default()
        });
        Ok(Self { processor })
    }
    pub fn process(
        &mut self,
        mic: &[f32; FRAME],
        reference: &[f32; FRAME],
    ) -> Result<[f32; FRAME]> {
        let mut capture = *mic;
        self.processor.analyze_render_frame([&reference[..]])?;
        self.processor.process_capture_frame([&mut capture[..]])?;
        Ok(capture)
    }
}

/// Positive lag only: timestamps align devices; this estimates the acoustic path.
/// First differences reduce music's low-frequency periodic ambiguity. Two agreeing
/// windows are required. No clean speech fixture or dictated text informs this estimate.
pub fn estimate_delay(mic: &[f32], reference: &[f32]) -> Option<(u16, f64)> {
    if mic.len() < RATE as usize || mic.len() != reference.len() || rms(reference) < 0.002 {
        return None;
    }
    let whiten = |input: &[f32]| -> Vec<f64> {
        input
            .windows(2)
            .step_by(4)
            .map(|p| (p[1] - p[0]) as f64)
            .collect()
    };
    let mic = whiten(mic);
    let reference = whiten(reference);
    let start = 1000; // 250 ms maximum delay, 4 kHz analysis grid.
    let mic_power = mic[start..].iter().map(|x| x * x).sum::<f64>();
    let mut best = (0, 0.);
    for lag in 0..=1000 {
        let mut xy = 0.;
        let mut rr = 0.;
        for i in start..mic.len() {
            let r = reference[i - lag];
            xy += mic[i] * r;
            rr += r * r;
        }
        let correlation = xy.abs() / (mic_power * rr).sqrt().max(1e-20);
        if correlation > best.1 {
            best = (lag, correlation);
        }
    }
    (best.1 >= 0.2).then_some(((best.0 as f64 / 4.).round() as u16, best.1))
}

pub struct Cleaner {
    processor: Option<EchoCanceller>,
    mic: Vec<f32>,
    reference: Vec<f32>,
    frames: usize,
    candidate: Option<u16>,
    pub delay: Option<u16>,
    pub correlation: f64,
    silent_frames: usize,
}
impl Cleaner {
    pub fn new() -> Self {
        Self {
            processor: None,
            mic: Vec::new(),
            reference: Vec::new(),
            frames: 0,
            candidate: None,
            delay: None,
            correlation: 0.,
            silent_frames: 0,
        }
    }
    pub fn process(
        &mut self,
        mic: &[f32; FRAME],
        reference: &[f32; FRAME],
    ) -> Result<[f32; FRAME]> {
        self.frames += 1;
        self.mic.extend(mic);
        self.reference.extend(reference);
        let keep = (RATE * 2) as usize;
        if self.mic.len() > keep {
            let trim = self.mic.len() - keep;
            self.mic.drain(..trim);
            self.reference.drain(..trim);
        }
        if self.processor.is_none()
            && self.frames.is_multiple_of(50)
            && let Some((delay, correlation)) = estimate_delay(&self.mic, &self.reference)
        {
            if self.candidate.is_some_and(|last| last.abs_diff(delay) <= 5) {
                let mut processor = EchoCanceller::new(Some(delay), Noise::Moderate)?;
                // Prime on past audio, without retroactively changing any emitted samples.
                for (capture, render) in self
                    .mic
                    .as_chunks::<FRAME>()
                    .0
                    .iter()
                    .zip(self.reference.as_chunks::<FRAME>().0)
                    .take(self.mic.len() / FRAME - 1)
                {
                    let _ = processor.process(capture, render)?;
                }
                self.processor = Some(processor);
                self.delay = Some(delay);
                self.correlation = correlation;
            }
            self.candidate = Some(delay);
        }
        self.silent_frames = if rms(reference) < 0.00001 {
            self.silent_frames + 1
        } else {
            0
        };
        if let Some(processor) = &mut self.processor {
            let clean = processor.process(mic, reference)?;
            // With no far-end audio, leave near-end speech untouched (after echo tails).
            if self.silent_frames < 30 {
                return Ok(clean);
            }
        }
        Ok(*mic)
    }
}
