//! 16 kHz mono AEC prototype. One worker owns the canceller. Audio callbacks
//! only copy/retain buffers and try_send; decoding, resampling, AEC and disk I/O
//! run off the callback threads. See README.md for timing and permission limits.
pub mod audio;
pub mod system_audio;
pub use system_audio::SystemAudioTap;

use anyhow::{Result, ensure};
use std::collections::VecDeque;
use webrtc_audio_processing::{Processor, config};

pub const RATE: u32 = 16_000;
pub const FRAME: usize = 160;

pub struct EchoCanceller {
    processor: Processor,
    mic: VecDeque<f32>,
    reference: VecDeque<f32>,
}

impl EchoCanceller {
    pub fn new() -> Result<Self> {
        Self::with_delay(None)
    }

    /// Delay is render-to-capture time, not extra samples inserted in reference.
    /// Some(ms) uses WebRTC's external delay estimator; None uses its automatic
    /// estimator. Do not pin a live delay without measuring the playback path.
    pub fn with_delay(delay_ms: Option<u16>) -> Result<Self> {
        let mut aec3 = webrtc_audio_processing::experimental::EchoCanceller3Config::default();
        aec3.filter.export_linear_aec_output = true;
        aec3.delay.use_external_delay_estimator = delay_ms.is_some();
        // Music + quieter dictation needs less aggressive residual suppression
        // than WebRTC's conversational defaults. Keep moderate NS on the linear
        // AEC output, rather than learning the loud echo as stationary noise.
        // These settings are measured offline, not validated on a live room yet.
        aec3.erle.max_l = 100.;
        aec3.erle.max_h = 100.;
        aec3.suppressor.normal_tuning.mask_lf.enr_transparent = 5.;
        aec3.suppressor.normal_tuning.mask_lf.enr_suppress = 6.;
        aec3.suppressor.normal_tuning.mask_lf.emr_transparent = 0.5;
        aec3.suppressor.normal_tuning.mask_hf = aec3.suppressor.normal_tuning.mask_lf;
        aec3.suppressor.nearend_tuning = aec3.suppressor.normal_tuning;
        let processor = Processor::with_aec3_config(RATE, aec3)?;
        processor.set_config(config::Config {
            echo_canceller: Some(config::EchoCanceller::Full {
                stream_delay_ms: delay_ms,
            }),
            noise_suppression: Some(config::NoiseSuppression {
                analyze_linear_aec_output: true,
                ..Default::default()
            }),
            high_pass_filter: Some(config::HighPassFilter::default()),
            gain_controller: None,
            ..Default::default()
        });
        Ok(Self {
            processor,
            mic: VecDeque::new(),
            reference: VecDeque::new(),
        })
    }

    /// Equal-length, time-aligned input chunks; retains incomplete 10 ms frames.
    /// Output can be shorter than input until flush(). No gain control or clipping.
    pub fn process(&mut self, mic: &[f32], reference: &[f32]) -> Result<Vec<f32>> {
        ensure!(mic.len() == reference.len(), "mic/reference lengths differ");
        self.mic.extend(mic);
        self.reference.extend(reference);
        let mut output = Vec::with_capacity(self.mic.len());
        while self.mic.len() >= FRAME {
            let mut capture = [0.; FRAME];
            let mut render = [0.; FRAME];
            for i in 0..FRAME {
                capture[i] = self.mic.pop_front().unwrap();
                render[i] = self.reference.pop_front().unwrap();
            }
            self.processor.analyze_render_frame([&render[..]])?;
            self.processor.process_capture_frame([&mut capture[..]])?;
            output.extend(capture);
        }
        Ok(output)
    }

    /// Zero-pads the last frame, returning only the unconsumed sample count.
    pub fn flush(&mut self) -> Result<Vec<f32>> {
        let remaining = self.mic.len();
        if remaining == 0 {
            return Ok(Vec::new());
        }
        let zeros = vec![0.; FRAME - remaining];
        let mut tail = self.process(&zeros, &zeros)?;
        tail.truncate(remaining);
        Ok(tail)
    }
}
