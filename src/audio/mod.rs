// ============================================================
// AUDIO SUBSYSTEM ORCHESTRATOR
// WASAPI Duplex Audio Engine with WebRTC AEC3 Echo Cancellation
// Tuned for Lenovo ThinkPad X380 Yoga (Headset-Free Operation)
// ============================================================

pub mod audio_buffer;
pub mod echo_canceller;
pub mod microphone;
pub mod resampler;
pub mod speaker;

use echo_canceller::EchoCancellerEngine;
use microphone::MicrophoneCapture;
use speaker::SpeakerPlayer;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Notify};
use tracing::info;

pub struct AudioEngine {
    speaker: Option<SpeakerPlayer>,
    microphone: Option<MicrophoneCapture>,
    aec_engine: Arc<Mutex<EchoCancellerEngine>>,
    barge_in_notify: Arc<Notify>,
    is_active: bool,
}

impl AudioEngine {
    pub fn new() -> Result<Self, String> {
        let aec_engine = Arc::new(Mutex::new(
            EchoCancellerEngine::new().map_err(|e| format!("Failed to create AEC3 engine: {}", e))?,
        ));
        let barge_in_notify = Arc::new(Notify::new());

        Ok(Self {
            speaker: None,
            microphone: None,
            aec_engine,
            barge_in_notify,
            is_active: false,
        })
    }

    /// Start WASAPI Duplex audio streams (Speaker + Mic + AEC3).
    /// `outbound_pcm_tx`: channel receiving cleaned 16kHz mono PCM frames from mic to send to server.
    pub fn start_duplex(
        &mut self,
        outbound_pcm_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<(), String> {
        if self.is_active {
            info!("[AUDIO] Duplex audio engine is already active.");
            return Ok(());
        }

        info!("[AUDIO] Starting WASAPI Duplex Audio Engine...");

        // Lock-free ring buffer for AEC reference signal (Claude Bug C1)
        // 4 seconds of hardware-rate mono audio absorbs async scheduling jitter with zero heap allocation
        let ref_buffer_capacity = 48000 * 4;
        let (echo_ref_prod, echo_ref_cons) = audio_buffer::create_audio_ring_buffer::<f32>(ref_buffer_capacity);

        // 1. Initialize Speaker (Playback with real-time DAC reference capture)
        let speaker = SpeakerPlayer::new(Some(echo_ref_prod))
            .map_err(|e| format!("Failed to initialize speaker: {}", e))?;

        let is_speaker_playing = speaker.is_playing_handle();

        // 2. Initialize Microphone (Capture with real-time synchronized AEC3)
        let microphone = MicrophoneCapture::new(
            outbound_pcm_tx,
            echo_ref_cons,
            Arc::clone(&self.aec_engine),
            Arc::clone(&self.barge_in_notify),
            is_speaker_playing,
        )
        .map_err(|e| format!("Failed to initialize microphone: {}", e))?;

        self.speaker = Some(speaker);
        self.microphone = Some(microphone);
        self.is_active = true;

        info!("[AUDIO] ✅ WASAPI Duplex Audio started successfully (ThinkPad X380 Yoga Tuned).");
        Ok(())
    }

    /// Feed incoming Gemini Live response audio chunk (24kHz 16-bit Mono PCM).
    pub fn play_response_pcm(&mut self, pcm_bytes: &[u8]) {
        if let Some(ref mut spk) = self.speaker {
            // Signal AEC that speaker is actively playing
            if let Ok(mut aec) = self.aec_engine.lock() {
                aec.set_speaking(true);
            }
            spk.play_pcm_24k(pcm_bytes);
        }
    }

    /// Check if speaker is currently playing response audio.
    #[allow(dead_code)]
    pub fn is_speaking(&self) -> bool {
        if let Some(ref spk) = self.speaker {
            let playing = spk.is_playing();
            if !playing {
                if let Ok(mut aec) = self.aec_engine.lock() {
                    aec.set_speaking(false);
                }
            }
            playing
        } else {
            false
        }
    }

    /// Stop playback immediately (on barge-in / interrupt).
    pub fn stop_speaking(&mut self) {
        if let Some(ref mut spk) = self.speaker {
            spk.stop();
        }
        if let Ok(mut aec) = self.aec_engine.lock() {
            aec.set_speaking(false);
        }
    }

    /// Set microphone mute state.
    #[allow(dead_code)]
    pub fn set_mic_muted(&self, muted: bool) {
        if let Some(ref mic) = self.microphone {
            mic.set_muted(muted);
        }
    }

    /// Check if microphone is muted.
    #[allow(dead_code)]
    pub fn is_mic_muted(&self) -> bool {
        self.microphone.as_ref().map(|m| m.is_muted()).unwrap_or(false)
    }

    /// Subscribe to user barge-in notification.
    pub fn barge_in_notify(&self) -> Arc<Notify> {
        Arc::clone(&self.barge_in_notify)
    }

    /// Stop all audio streams and release hardware handles.
    pub fn stop_duplex(&mut self) {
        info!("[AUDIO] Stopping WASAPI Duplex Audio Engine...");
        if let Some(ref mic) = self.microphone {
            mic.stop();
        }
        if let Some(ref mut spk) = self.speaker {
            spk.stop();
        }
        self.microphone = None;
        self.speaker = None;
        self.is_active = false;
        info!("[AUDIO] Duplex Audio Engine stopped.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}

    #[test]
    fn check_send() {
        assert_send::<EchoCancellerEngine>();
        assert_send::<SpeakerPlayer>();
        assert_send::<MicrophoneCapture>();
        assert_send::<AudioEngine>();
    }
}
