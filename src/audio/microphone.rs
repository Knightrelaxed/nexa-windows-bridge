// ============================================================
// WASAPI MICROPHONE CAPTURE ENGINE
// Input Stream (ThinkPad Conexant Dual-Array Mic -> 16kHz PCM)
// ============================================================

use super::audio_buffer::{create_audio_ring_buffer, AudioConsumer};
use super::echo_canceller::EchoCancellerEngine;
use super::resampler::{downmix_to_mono, f32_to_i16, i16_to_pcm_bytes, resample_linear};
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
        echo_ref_rx: mpsc::Receiver<Vec<f32>>,
        aec_engine: Arc<Mutex<EchoCancellerEngine>>,
        barge_in_notify: Arc<tokio::sync::Notify>,
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

        // Spawn async background processing worker
        let is_running = Arc::clone(&is_capturing);
        tokio::spawn(run_mic_processing_worker(
            consumer,
            hardware_sample_rate,
            hardware_channels,
            outbound_pcm_tx,
            echo_ref_rx,
            aec_engine,
            barge_in_notify,
            is_running,
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
    hardware_sample_rate: u32,
    hardware_channels: u16,
    outbound_pcm_tx: mpsc::Sender<Vec<u8>>,
    mut echo_ref_rx: mpsc::Receiver<Vec<f32>>,
    aec_engine: Arc<Mutex<EchoCancellerEngine>>,
    barge_in_notify: Arc<tokio::sync::Notify>,
    is_running: Arc<AtomicBool>,
) {
    let mut raw_chunk = vec![0.0f32; 1024];
    let mut outbound_accumulator: Vec<i16> = Vec::with_capacity(1600);
    const FRAME_TARGET_SAMPLES: usize = 1600; // 100ms at 16kHz for Gemini Live API

    while is_running.load(Ordering::Relaxed) {
        // 1. Drain incoming render reference samples into AEC
        while let Ok(ref_chunk) = echo_ref_rx.try_recv() {
            if let Ok(mut aec) = aec_engine.lock() {
                aec.feed_render_samples(&ref_chunk);
            }
        }

        // 2. Read captured mic samples from lock-free ring buffer
        let read = consumer.pop_slice(&mut raw_chunk);
        if read > 0 {
            let captured_slice = &raw_chunk[..read];

            // Downmix to mono if multi-channel (stereo, quad, etc.)
            let mono_slice = downmix_to_mono(captured_slice, hardware_channels);

            // Resample to 16,000 Hz if needed
            let samples_16k = if hardware_sample_rate != 16000 {
                resample_linear(&mono_slice, hardware_sample_rate, 16000)
            } else {
                mono_slice
            };

            // Process through WebRTC AEC3 + Ducking Engine
            let mut processed_16k = Vec::with_capacity(samples_16k.len());
            let mut barge_in_fired = false;

            {
                if let Ok(mut aec) = aec_engine.lock() {
                    aec.feed_capture_samples(&samples_16k);

                    let mut temp = vec![0.0f32; aec.available_samples()];
                    let count = aec.read_processed_samples(&mut temp);
                    processed_16k.extend_from_slice(&temp[..count]);

                    if aec.take_barge_in() {
                        barge_in_fired = true;
                    }
                }
            }

            if barge_in_fired {
                info!("[MIC] 🗣️ User Barge-In detected! Preempting active AI playback.");
                barge_in_notify.notify_waiters();
            }

            // Accumulate 16-bit PCM samples into 100ms frames (1600 samples = 3200 bytes)
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
