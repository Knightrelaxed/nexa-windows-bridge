// ============================================================
// N.E.X.A 3.3 — ACOUSTIC ECHO CANCELLATION (AEC3)
// WebRTC AEC3, Noise Suppression & Dynamic Ducking Engine
// ============================================================

use sonora::config::{EchoCanceller, GainController2, NoiseSuppression};
use sonora::{AudioProcessing, Config, StreamConfig};
use std::collections::VecDeque;

pub const CHUNK_SIZE_10MS: usize = 160; // 16kHz * 0.01s = 160 samples
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

pub struct EchoCancellerEngine {
    apm: AudioProcessing,
    render_queue: VecDeque<f32>,
    capture_accumulator: Vec<f32>,
    processed_queue: VecDeque<f32>,
    is_speaking: bool,
    ducking_factor: f32,       // e.g. 0.125 = -18dB
    barge_in_threshold: f32,   // RMS threshold for user interrupting AI
    barge_in_detected: bool,
}

impl EchoCancellerEngine {
    pub fn new() -> Result<Self, sonora::Error> {
        let config = Config {
            echo_canceller: Some(EchoCanceller::default()),
            noise_suppression: Some(NoiseSuppression::default()),
            gain_controller2: Some(GainController2::default()),
            ..Default::default()
        };

        let apm = AudioProcessing::builder()
            .config(config)
            .capture_config(StreamConfig::new(DEFAULT_SAMPLE_RATE, 1))
            .render_config(StreamConfig::new(DEFAULT_SAMPLE_RATE, 1))
            .build();

        Ok(Self {
            apm,
            render_queue: VecDeque::with_capacity(3200), // ~200ms buffer
            capture_accumulator: Vec::with_capacity(CHUNK_SIZE_10MS * 2),
            processed_queue: VecDeque::with_capacity(3200),
            is_speaking: false,
            ducking_factor: 0.125, // -18dB attenuation
            barge_in_threshold: 0.07,
            barge_in_detected: false,
        })
    }

    /// Set whether N.E.X.A is currently speaking out of the laptop speakers.
    pub fn set_speaking(&mut self, speaking: bool) {
        self.is_speaking = speaking;
        if !speaking {
            self.barge_in_detected = false;
        }
    }

    /// Check if user barge-in was detected during active playback.
    pub fn take_barge_in(&mut self) -> bool {
        let detected = self.barge_in_detected;
        self.barge_in_detected = false;
        detected
    }

    /// Feed reference speaker audio (16kHz mono f32) played by the speakers.
    pub fn feed_render_samples(&mut self, samples: &[f32]) {
        self.render_queue.extend(samples);
        // Prevent unbounded growth if mic is inactive
        if self.render_queue.len() > 16000 {
            self.render_queue.drain(..self.render_queue.len() - 16000);
        }
    }

    /// Feed captured microphone audio (16kHz mono f32). Processes full 10ms chunks.
    pub fn feed_capture_samples(&mut self, samples: &[f32]) {
        self.capture_accumulator.extend_from_slice(samples);

        while self.capture_accumulator.len() >= CHUNK_SIZE_10MS {
            let chunk: Vec<f32> = self.capture_accumulator.drain(..CHUNK_SIZE_10MS).collect();

            // 1. Check RMS energy for barge-in
            let rms = (chunk.iter().map(|&s| s * s).sum::<f32>() / chunk.len() as f32).sqrt();
            if self.is_speaking && rms > self.barge_in_threshold {
                self.barge_in_detected = true;
            }

            // 2. Prepare render reference for this 10ms slice
            let mut render_chunk = vec![0.0f32; CHUNK_SIZE_10MS];
            for i in 0..CHUNK_SIZE_10MS {
                if let Some(s) = self.render_queue.pop_front() {
                    render_chunk[i] = s;
                }
            }

            // 3. Process render frame through WebRTC AEC3
            let mut render_out = vec![0.0f32; CHUNK_SIZE_10MS];
            let _ = self.apm.process_render_f32(&[&render_chunk], &mut [&mut render_out]);

            // 4. Process capture frame through WebRTC AEC3 + NS + AGC
            let mut capture_out = vec![0.0f32; CHUNK_SIZE_10MS];
            if self.apm.process_capture_f32(&[&chunk], &mut [&mut capture_out]).is_ok() {
                // 5. Apply turn-aware ducking if speaking and no barge-in
                if self.is_speaking && !self.barge_in_detected {
                    for sample in &mut capture_out {
                        *sample *= self.ducking_factor;
                    }
                }
                self.processed_queue.extend(capture_out);
            } else {
                self.processed_queue.extend(chunk);
            }
        }
    }

    /// Read processed, echo-cancelled audio samples out into destination buffer.
    pub fn read_processed_samples(&mut self, dst: &mut [f32]) -> usize {
        let count = dst.len().min(self.processed_queue.len());
        for i in 0..count {
            dst[i] = self.processed_queue.pop_front().unwrap_or(0.0);
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
        self.render_queue.clear();
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

        // Loud vocal from user (exceeds 0.07 threshold)
        let loud_voice = vec![0.5f32; 160];
        aec.feed_capture_samples(&loud_voice);

        assert!(aec.take_barge_in());
        // After taking, it resets
        assert!(!aec.take_barge_in());
    }
}
