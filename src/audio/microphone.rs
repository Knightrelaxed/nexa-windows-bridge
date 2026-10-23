// ============================================================
// WASAPI MICROPHONE CAPTURE ENGINE
// Input Stream (ThinkPad Conexant Dual-Array Mic -> 16kHz PCM)
// ============================================================

use super::audio_buffer::{create_audio_ring_buffer, AudioConsumer};
use super::echo_canceller::EchoCancellerEngine;
use super::resampler::{f32_to_i16, i16_to_pcm_bytes};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info};

pub struct SendStream(#[allow(dead_code)] pub Stream);
unsafe impl Send for SendStream {}

pub struct MicrophoneCapture {
    _stream: Option<SendStream>,
    is_capturing: Arc<AtomicBool>,
    #[allow(dead_code)]
    is_muted: Arc<AtomicBool>,
}

impl MicrophoneCapture {
    pub fn new(
        outbound_pcm_tx: mpsc::Sender<Vec<u8>>,
        echo_ref_consumer: AudioConsumer<f32>,
        aec_engine: Arc<Mutex<EchoCancellerEngine>>,
        barge_in_notify: Arc<tokio::sync::Notify>,
        is_speaker_playing: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "No default audio input device available".to_string())?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown Device".into());
        info!("[MIC] Initializing capture on input device: {}", device_name);

        let default_config = device
            .default_input_config()
            .map_err(|e| format!("Failed to get default input config: {}", e))?;

        let hardware_sample_rate = default_config.sample_rate().0;
        let hardware_channels = default_config.channels();
        let sample_format = default_config.sample_format();

        info!(
            "[MIC] Hardware Input Config: {} Hz, {} channels, format: {:?}",
            hardware_sample_rate, hardware_channels, sample_format
        );

        // Ring buffer capacity: 1 second of audio
        let buffer_capacity = (hardware_sample_rate as usize) * (hardware_channels as usize);
        let (mut producer, consumer) = create_audio_ring_buffer::<f32>(buffer_capacity);

        let is_capturing = Arc::new(AtomicBool::new(true));
        let is_muted = Arc::new(AtomicBool::new(false));

        let is_capturing_clone = Arc::clone(&is_capturing);
        let is_muted_clone = Arc::clone(&is_muted);

        let config: StreamConfig = default_config.into();
        let stream = match sample_format {
            SampleFormat::F32 => device
                .build_input_stream(
                    &config,
                    move |data: &[f32], _| {
                        if is_capturing_clone.load(Ordering::Relaxed) {
                            if is_muted_clone.load(Ordering::Relaxed) {
                                // Muted: push silence
                                let silence = vec![0.0f32; data.len()];
                                let _ = producer.push_slice(&silence);
                            } else {
                                let _ = producer.push_slice(data);
                            }
                        }
                    },
                    move |err| {
                        error!("[MIC] Stream error: {}", err);
                    },
                    None,
                )
                .map_err(|e| format!("Failed to build F32 input stream: {}", e))?,
            SampleFormat::I16 => device
                .build_input_stream(
                    &config,
                    move |data: &[i16], _| {
                        if is_capturing_clone.load(Ordering::Relaxed) {
                            if is_muted_clone.load(Ordering::Relaxed) {
                                let silence = vec![0.0f32; data.len()];
                                let _ = producer.push_slice(&silence);
                            } else {
                                let f32_samples: Vec<f32> =
                                    data.iter().map(|&s| s as f32 / 32768.0).collect();
                                let _ = producer.push_slice(&f32_samples);
                            }
                        }
                    },
                    move |err| {
                        error!("[MIC] Stream error: {}", err);
                    },
                    None,
                )
                .map_err(|e| format!("Failed to build I16 input stream: {}", e))?,
            _ => return Err(format!("Unsupported input sample format: {:?}", sample_format)),
        };

        stream
            .play()
            .map_err(|e| format!("Failed to start input stream: {}", e))?;

        info!("[MIC] Capture stream started successfully.");

        // Synchronize AEC engine with exact hardware format
        if let Ok(mut aec) = aec_engine.lock() {
            aec.set_hardware_format(hardware_sample_rate, hardware_channels);
        }

        // Spawn async background processing worker
        let is_running = Arc::clone(&is_capturing);
        tokio::spawn(run_mic_processing_worker(
            consumer,
            hardware_sample_rate,
            hardware_channels,
            outbound_pcm_tx,
            echo_ref_consumer,
            aec_engine,
            barge_in_notify,
            is_running,
            is_speaker_playing,
        ));

        Ok(Self {
            _stream: Some(SendStream(stream)),
            is_capturing,
            is_muted,
        })
    }

    #[allow(dead_code)]
    pub fn set_muted(&self, muted: bool) {
        self.is_muted.store(muted, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    pub fn is_muted(&self) -> bool {
        self.is_muted.load(Ordering::SeqCst)
    }

    pub fn stop(&self) {
        self.is_capturing.store(false, Ordering::SeqCst);
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_mic_processing_worker(
    mut consumer: AudioConsumer<f32>,
    _hardware_sample_rate: u32,
    hardware_channels: u16,
    outbound_pcm_tx: mpsc::Sender<Vec<u8>>,
    mut echo_ref_consumer: AudioConsumer<f32>,
    aec_engine: Arc<Mutex<EchoCancellerEngine>>,
    barge_in_notify: Arc<tokio::sync::Notify>,
    is_running: Arc<AtomicBool>,
    is_speaker_playing: Arc<AtomicBool>,
) {
    let mut raw_chunk = vec![0.0f32; 1024];
    let mut outbound_accumulator: Vec<i16> = Vec::with_capacity(1600);
    const FRAME_TARGET_SAMPLES: usize = 1600; // 100ms at 16kHz for Gemini Live API

    let mut last_speaker_active = std::time::Instant::now()
        .checked_sub(Duration::from_secs(10))
        .unwrap_or_else(std::time::Instant::now);

    while is_running.load(Ordering::Relaxed) {
        let speaker_active_now = is_speaker_playing.load(Ordering::Relaxed);
        if speaker_active_now {
            last_speaker_active = std::time::Instant::now();
        }
        // Minimal 80ms reverb hold prevents clipping the first words of the user's response
        let is_in_speaker_or_reverb = speaker_active_now || last_speaker_active.elapsed() < Duration::from_millis(80);

        // 1. Drain incoming render reference samples from real-time speaker callback (Claude Bug C1)
        let mut ref_scratch = [0.0f32; 1024];
        let ref_read = echo_ref_consumer.pop_slice(&mut ref_scratch);
        if ref_read > 0 {
            if let Ok(mut aec) = aec_engine.lock() {
                aec.feed_render_samples(&ref_scratch[..ref_read]);
            }
        }

        // Keep AEC engine state synchronized with speaker playback & acoustic reverberation tail
        if let Ok(mut aec) = aec_engine.lock() {
            aec.set_speaking(is_in_speaker_or_reverb);
        }

        // 2. Read captured mic samples strictly aligned to hardware channels to prevent channel phase shift
        let available = consumer.len();
        let ch = (hardware_channels.max(1)) as usize;
        let to_read = (available.min(raw_chunk.len()) / ch) * ch;
        if to_read > 0 {
            let read = consumer.pop_slice(&mut raw_chunk[..to_read]);
            let captured_slice = &raw_chunk[..read];

            // Process through WebRTC AEC3 + Beamforming + Acoustic Suppression + AGC2 Engine
            let mut processed_16k = Vec::new();
            let mut barge_in_fired = false;

            {
                if let Ok(mut aec) = aec_engine.lock() {
                    aec.feed_capture_samples(captured_slice);

                    let avail = aec.available_samples();
                    if avail > 0 {
                        let mut temp = vec![0.0f32; avail];
                        let count = aec.read_processed_samples(&mut temp);
                        processed_16k.extend_from_slice(&temp[..count]);
                    }

                    if aec.take_barge_in() {
                        barge_in_fired = true;
                    }
                }
            }

            if barge_in_fired {
                info!("[MIC] 🗣️ User Barge-In detected! Preempting active AI playback.");
                barge_in_notify.notify_waiters();
            }

            // Transmit frames only if speaker is inactive, OR user deliberately barged in
            if !is_in_speaker_or_reverb || barge_in_fired {
                if !processed_16k.is_empty() {
                    let i16_samples = f32_to_i16(&processed_16k);
                    outbound_accumulator.extend(i16_samples);

                    while outbound_accumulator.len() >= FRAME_TARGET_SAMPLES {
                        let frame: Vec<i16> =
                            outbound_accumulator.drain(..FRAME_TARGET_SAMPLES).collect();
                        let pcm_bytes = i16_to_pcm_bytes(&frame);
                        let _ = outbound_pcm_tx.send(pcm_bytes).await;
                    }
                }
            } else {
                // Speaker active and no barge-in: clear outbound accumulator
                // to prevent any residual echo chunks from leaking to server
                outbound_accumulator.clear();
            }
        } else {
            // Buffer empty: sleep briefly (10ms)
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    // Flush any remaining samples on exit
    if !outbound_accumulator.is_empty() {
        let pcm_bytes = i16_to_pcm_bytes(&outbound_accumulator);
        let _ = outbound_pcm_tx.send(pcm_bytes).await;
    }
}
