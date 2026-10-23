// ============================================================
// ACOUSTIC ECHO CANCELLATION (AEC3)
// WebRTC AEC3, Noise Suppression & Dynamic Ducking Engine
// ============================================================

use sonora::config::{
    EchoCanceller, GainController2, HighPassFilter, NoiseSuppression, NoiseSuppressionLevel,
    PreAmplifier,
};
use sonora::{AudioProcessing, Config, StreamConfig};
use std::collections::VecDeque;

pub const CHUNK_SIZE_10MS: usize = 160; // 16kHz * 0.01s = 160 samples
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

pub struct EchoCancellerEngine {
    apm: AudioProcessing,
    hardware_sample_rate: u32,
    hardware_channels: u16,
    render_accumulator: Vec<f32>,
    capture_accumulator: Vec<f32>,
    processed_queue: VecDeque<f32>,
    is_speaking: bool,
    #[allow(dead_code)]
    ducking_factor: f32,       // e.g. 0.125 = -18dB
    barge_in_threshold: f32,   // RMS threshold for user interrupting AI
    barge_in_detected: bool,
}

impl EchoCancellerEngine {
    pub fn new() -> Result<Self, sonora::Error> {
        Self::new_with_format(DEFAULT_SAMPLE_RATE, 1)
    }

    pub fn new_with_format(hardware_sample_rate: u32, hardware_channels: u16) -> Result<Self, sonora::Error> {
        let config = Config {
            echo_canceller: Some(EchoCanceller::default()),
            noise_suppression: Some(NoiseSuppression {
                level: NoiseSuppressionLevel::High,
                analyze_linear_aec_output_when_available: true,
            }),
            gain_controller2: Some(GainController2 {
                adaptive_digital: Some(sonora::config::AdaptiveDigital {
                    headroom_db: 5.0,
                    max_gain_db: 30.0,
                    initial_gain_db: 12.0,
                    max_gain_change_db_per_second: 15.0,
                    max_output_noise_level_dbfs: -45.0,
                }),
                fixed_digital: sonora::config::FixedDigital::default(),
                input_volume_controller: false,
            }),
            high_pass_filter: Some(HighPassFilter {
                apply_in_full_band: true,
            }),
            pre_amplifier: Some(PreAmplifier {
                fixed_gain_factor: 2.5, // +8dB clean analog-style pre-amp for ThinkPad mic array
            }),
            pipeline: sonora::config::Pipeline {
                multi_channel_render: true,
                multi_channel_capture: hardware_channels > 1,
                ..Default::default()
            },
            ..Default::default()
        };

        let apm = AudioProcessing::builder()
            .config(config)
            .capture_config(StreamConfig::new(hardware_sample_rate, hardware_channels))
            .render_config(StreamConfig::new(hardware_sample_rate, 1))
            .build();

        let frame_frames = (hardware_sample_rate / 100) as usize;
        let channels = hardware_channels.max(1) as usize;

        Ok(Self {
            apm,
            hardware_sample_rate,
            hardware_channels,
            render_accumulator: Vec::with_capacity(frame_frames * 4),
            capture_accumulator: Vec::with_capacity(frame_frames * channels * 4),
            processed_queue: VecDeque::with_capacity(3200),
            is_speaking: false,
            ducking_factor: 0.125, // -18dB attenuation
            barge_in_threshold: 0.12, // Voice barge-in RMS threshold after AEC
            barge_in_detected: false,
        })
    }

    /// Set or update the hardware audio format (sample rate and channels).
    pub fn set_hardware_format(&mut self, sample_rate: u32, channels: u16) {
        if self.hardware_sample_rate != sample_rate || self.hardware_channels != channels {
            self.hardware_sample_rate = sample_rate;
            self.hardware_channels = channels;
            self.render_accumulator.clear();
            self.capture_accumulator.clear();
            self.processed_queue.clear();
        }
    }

    /// Set whether N.E.X.A is currently speaking out of the laptop speakers.
    pub fn set_speaking(&mut self, speaking: bool) {
        self.is_speaking = speaking;
        if !speaking {
            self.barge_in_detected = false;
        }
    }

    /// Explicitly trigger user barge-in (e.g. from hotkey or UI click).
    #[allow(dead_code)]
    pub fn trigger_barge_in(&mut self) {
        self.barge_in_detected = true;
    }

    /// Check if user barge-in was detected during active playback.
    pub fn take_barge_in(&mut self) -> bool {
        let detected = self.barge_in_detected;
        self.barge_in_detected = false;
        detected
    }

    /// Feed reference speaker audio (mono f32 at hardware_sample_rate) played by the speakers.
    /// Immediately processes 10ms chunks into WebRTC AEC3 internal delay buffer.
    pub fn feed_render_samples(&mut self, samples: &[f32]) {
        self.render_accumulator.extend_from_slice(samples);

        let frame_frames = (self.hardware_sample_rate / 100) as usize;
        while self.render_accumulator.len() >= frame_frames {
            let chunk: Vec<f32> = self.render_accumulator.drain(..frame_frames).collect();
            let render_cfg = StreamConfig::new(self.hardware_sample_rate, 1);
            let mut render_out = vec![0.0f32; frame_frames];
            let render_src: &[&[f32]] = &[&chunk];
            let render_dest: &mut [&mut [f32]] = &mut [&mut render_out];
            let _ = self.apm.process_render_f32_with_config(render_src, &render_cfg, &render_cfg, render_dest);
        }
    }

    /// Feed captured microphone audio (interleaved f32 at hardware_sample_rate).
    /// WebRTC APM handles deinterleaving, beamforming, AEC3, NS, AGC2, and band-limited resampling to 16kHz mono.
    pub fn feed_capture_samples(&mut self, samples: &[f32]) {
        self.capture_accumulator.extend_from_slice(samples);

        let frame_frames = (self.hardware_sample_rate / 100) as usize;
        let channels = self.hardware_channels.max(1) as usize;
        let required_samples = frame_frames * channels;

        while self.capture_accumulator.len() >= required_samples {
            let chunk: Vec<f32> = self.capture_accumulator.drain(..required_samples).collect();

            let mut left = Vec::with_capacity(frame_frames);
            let mut right = if channels > 1 { Vec::with_capacity(frame_frames) } else { Vec::new() };

            if channels == 1 {
                left.extend_from_slice(&chunk);
            } else if channels == 2 {
                for frame in chunk.chunks_exact(2) {
                    left.push(frame[0]);
                    right.push(frame[1]);
                }
            } else {
                for frame in chunk.chunks_exact(channels) {
                    left.push(frame[0]);
                    right.push(frame[1]);
                }
            }

            let in_cfg = StreamConfig::new(self.hardware_sample_rate, self.hardware_channels);
            let out_cfg = StreamConfig::new(DEFAULT_SAMPLE_RATE, 1);
            let mut capture_out = vec![0.0f32; CHUNK_SIZE_10MS]; // 160 samples at 16kHz

            let process_result = if channels > 1 {
                let cap_src: &[&[f32]] = &[&left, &right];
                let cap_dest: &mut [&mut [f32]] = &mut [&mut capture_out];
                self.apm.process_capture_f32_with_config(cap_src, &in_cfg, &out_cfg, cap_dest)
            } else {
                let cap_src: &[&[f32]] = &[&left];
                let cap_dest: &mut [&mut [f32]] = &mut [&mut capture_out];
                self.apm.process_capture_f32_with_config(cap_src, &in_cfg, &out_cfg, cap_dest)
            };

            if process_result.is_ok() {
                // If speaker is actively playing, check if user is intentionally speaking (barge-in)
                if self.is_speaking {
                    let mut sum_sq = 0.0f32;
                    for &s in &capture_out {
                        sum_sq += s * s;
                    }
                    let rms = (sum_sq / capture_out.len() as f32).sqrt();
                    if rms > self.barge_in_threshold {
                        self.barge_in_detected = true;
                        self.is_speaking = false;
                    } else {
                        capture_out.fill(0.0);
                    }
                }
                self.processed_queue.extend(capture_out);
            } else {
                if self.is_speaking {
                    self.processed_queue.extend(vec![0.0f32; CHUNK_SIZE_10MS]);
                } else {
                    self.processed_queue.extend(vec![0.0f32; CHUNK_SIZE_10MS]);
                }
            }
        }
    }

    /// Read processed, echo-cancelled audio samples out into destination buffer.
    pub fn read_processed_samples(&mut self, dst: &mut [f32]) -> usize {
        let count = dst.len().min(self.processed_queue.len());
        for slot in dst.iter_mut().take(count) {
            *slot = self.processed_queue.pop_front().unwrap_or(0.0);
        }
        count
    }

    /// Check number of available processed samples.
    pub fn available_samples(&self) -> usize {
        self.processed_queue.len()
    }

    /// Clear all queues.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.render_accumulator.clear();
        self.capture_accumulator.clear();
        self.processed_queue.clear();
        self.barge_in_detected = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_echo_canceller_streaming_pipeline() {
        let mut aec = EchoCancellerEngine::new().unwrap();

        // Feed render samples (speaker output)
        let render_data = vec![0.2f32; 320]; // 20ms of render audio
        aec.feed_render_samples(&render_data);

        // Feed mic samples (capture input)
        let mic_data = vec![0.1f32; 320]; // 20ms of mic audio
        aec.feed_capture_samples(&mic_data);

        assert_eq!(aec.available_samples(), 320);

        let mut output = vec![0.0f32; 320];
        let read = aec.read_processed_samples(&mut output);
        assert_eq!(read, 320);
        assert_eq!(aec.available_samples(), 0);
    }

    #[test]
    fn test_barge_in_detection() {
        let mut aec = EchoCancellerEngine::new().unwrap();
        aec.set_speaking(true);

        // Explicit barge-in trigger (e.g. from hotkey or UI)
        assert!(!aec.take_barge_in());
        aec.trigger_barge_in();
        assert!(aec.take_barge_in());
        // After taking, it resets to false
        assert!(!aec.take_barge_in());
    }

    #[test]
    fn test_acoustic_muting_during_speaking_without_barge_in() {
        let mut aec = EchoCancellerEngine::new().unwrap();
        aec.set_speaking(true);

        // Loud speaker sound entering internal laptop mic (0.8 RMS)
        let loud_speaker_sound = vec![0.8f32; 160];
        aec.feed_capture_samples(&loud_speaker_sound);

        // Must NOT trigger false barge-in
        assert!(!aec.take_barge_in());
        let mut out = vec![1.0f32; 160];
        let read = aec.read_processed_samples(&mut out);
        assert_eq!(read, 160);
        // Captured speaker sound must be completely muted (0.0) to prevent self-interruption
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_sonora_native_resampling_and_downmix() {
        let config = Config {
            echo_canceller: Some(EchoCanceller::default()),
            noise_suppression: Some(NoiseSuppression {
                level: NoiseSuppressionLevel::High,
                analyze_linear_aec_output_when_available: true,
            }),
            gain_controller2: Some(GainController2 {
                adaptive_digital: Some(sonora::config::AdaptiveDigital {
                    headroom_db: 5.0,
                    max_gain_db: 30.0,
                    initial_gain_db: 12.0,
                    max_gain_change_db_per_second: 15.0,
                    max_output_noise_level_dbfs: -45.0,
                }),
                fixed_digital: sonora::config::FixedDigital::default(),
                input_volume_controller: false,
            }),
            high_pass_filter: Some(HighPassFilter {
                apply_in_full_band: true,
            }),
            pre_amplifier: Some(PreAmplifier {
                fixed_gain_factor: 2.0,
            }),
            pipeline: sonora::config::Pipeline {
                multi_channel_render: true,
                multi_channel_capture: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut apm = AudioProcessing::builder().config(config).build();

        // 1. Process 10ms render reference at 48kHz mono (480 samples)
        let render_cfg = StreamConfig::new(48000, 1);
        let render_src_data = vec![0.1f32; 480];
        let mut render_dest_data = vec![0.0f32; 480];
        let render_src: &[&[f32]] = &[&render_src_data];
        let render_dest: &mut [&mut [f32]] = &mut [&mut render_dest_data];

        let r_res = apm.process_render_f32_with_config(render_src, &render_cfg, &render_cfg, render_dest);
        assert!(r_res.is_ok(), "process_render_f32_with_config failed: {:?}", r_res.err());

        // 2. Process 10ms capture at 48kHz stereo (2x480 samples) -> 16kHz mono (160 samples)
        let cap_in_cfg = StreamConfig::new(48000, 2);
        let cap_out_cfg = StreamConfig::new(16000, 1);
        let left = vec![0.05f32; 480];
        let right = vec![0.05f32; 480];
        let cap_src: &[&[f32]] = &[&left, &right];
        let mut out_16k = vec![0.0f32; 160];
        let cap_dest: &mut [&mut [f32]] = &mut [&mut out_16k];

        let c_res = apm.process_capture_f32_with_config(cap_src, &cap_in_cfg, &cap_out_cfg, cap_dest);
        assert!(c_res.is_ok(), "process_capture_f32_with_config failed: {:?}", c_res.err());
        assert_eq!(out_16k.len(), 160);
    }
}
