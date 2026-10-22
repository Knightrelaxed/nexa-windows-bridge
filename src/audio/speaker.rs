// ============================================================
// WASAPI SPEAKER PLAYBACK ENGINE
// Output Stream (24kHz Gemini Response -> Hardware Output)
// ============================================================

use super::audio_buffer::{create_audio_ring_buffer, AudioConsumer, AudioProducer};
use super::resampler::{i16_to_f32, pcm_bytes_to_i16, resample_linear, upmix_mono_to_channels};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{error, info, warn};

pub struct SendStream(#[allow(dead_code)] pub Stream);
unsafe impl Send for SendStream {}

// Ring buffer capacity constant: 30 seconds absorbs full Gemini Live burst audio without dropping (Claude Bug M3)
pub const PLAYBACK_BUFFER_SECONDS: usize = 30;

pub struct SpeakerPlayer {
    _stream: Option<SendStream>,
    producer: Option<AudioProducer<f32>>,
    is_playing: Arc<AtomicBool>,
    flush_requested: Arc<AtomicBool>,
    hardware_sample_rate: u32,
    hardware_channels: u16,
}

impl SpeakerPlayer {
    pub fn new(mut echo_ref_producer: Option<AudioProducer<f32>>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "No default audio output device available".to_string())?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown Device".into());
        info!("[SPEAKER] Initializing playback on output device: {}", device_name);

        let default_config = device
            .default_output_config()
            .map_err(|e| format!("Failed to get default output config: {}", e))?;

        let hardware_sample_rate = default_config.sample_rate().0;
        let hardware_channels = default_config.channels();
        let sample_format = default_config.sample_format();

        info!(
            "[SPEAKER] Hardware Output Config: {} Hz, {} channels, format: {:?}",
            hardware_sample_rate, hardware_channels, sample_format
        );

        // Ring buffer capacity: 30 seconds of hardware audio (absorb Gemini Live burst streaming without sample drops)
        let buffer_capacity = (hardware_sample_rate as usize) * (hardware_channels as usize) * PLAYBACK_BUFFER_SECONDS;
        let (producer, mut consumer) = create_audio_ring_buffer::<f32>(buffer_capacity);

        let is_playing = Arc::new(AtomicBool::new(false));
        let flush_requested = Arc::new(AtomicBool::new(false));

        let is_playing_clone = Arc::clone(&is_playing);
        let flush_requested_clone = Arc::clone(&flush_requested);

        let config: StreamConfig = default_config.into();
        let stream = match sample_format {
            SampleFormat::F32 => device
                .build_output_stream(
                    &config,
                    move |data: &mut [f32], _| {
                        playback_callback_f32(
                            data,
                            &mut consumer,
                            &is_playing_clone,
                            &flush_requested_clone,
                            echo_ref_producer.as_mut(),
                            hardware_channels,
                        );
                    },
                    move |err| {
                        error!("[SPEAKER] Stream error: {}", err);
                    },
                    None,
                )
                .map_err(|e| format!("Failed to build F32 output stream: {}", e))?,
            SampleFormat::I16 => device
                .build_output_stream(
                    &config,
                    move |data: &mut [i16], _| {
                        playback_callback_i16(
                            data,
                            &mut consumer,
                            &is_playing_clone,
                            &flush_requested_clone,
                            echo_ref_producer.as_mut(),
                            hardware_channels,
                        );
                    },
                    move |err| {
                        error!("[SPEAKER] Stream error: {}", err);
                    },
                    None,
                )
                .map_err(|e| format!("Failed to build I16 output stream: {}", e))?,
            _ => return Err(format!("Unsupported output sample format: {:?}", sample_format)),
        };

        stream
            .play()
            .map_err(|e| format!("Failed to start output stream: {}", e))?;

        info!("[SPEAKER] Output stream started successfully.");

        Ok(Self {
            _stream: Some(SendStream(stream)),
            producer: Some(producer),
            is_playing,
            flush_requested,
            hardware_sample_rate,
            hardware_channels,
        })
    }

    /// Feed incoming 24kHz 16-bit Mono PCM audio bytes from Gemini Live response.
    pub fn play_pcm_24k(&mut self, pcm_bytes: &[u8]) {
        if pcm_bytes.is_empty() {
            return;
        }

        // 1. Convert raw bytes to f32 samples (-1.0 to 1.0)
        let i16_samples = pcm_bytes_to_i16(pcm_bytes);
        let f32_24k = i16_to_f32(&i16_samples);

        // 2. Resample from 24kHz to hardware sample rate (e.g. 48kHz)
        let resampled = resample_linear(&f32_24k, 24000, self.hardware_sample_rate);

        // 3. Upmix to hardware channels (e.g. stereo)
        let final_samples = upmix_mono_to_channels(&resampled, self.hardware_channels);

        // 4. Push to real-time ring buffer
        if let Some(ref mut prod) = self.producer {
            let pushed = prod.push_slice(&final_samples);
            if pushed < final_samples.len() {
                warn!(
                    "[SPEAKER] Playback ring buffer full: dropped {} samples",
                    final_samples.len() - pushed
                );
            }
        }
    }

    /// Check if audio is currently playing.
    #[allow(dead_code)]
    pub fn is_playing(&self) -> bool {
        self.is_playing.load(Ordering::SeqCst)
    }

    /// Cloneable atomic handle to monitor active speaker playback in real-time.
    pub fn is_playing_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.is_playing)
    }

    /// Stop playback immediately (used on barge-in / interrupt).
    pub fn stop(&mut self) {
        self.flush_requested.store(true, Ordering::SeqCst);
        self.is_playing.store(false, Ordering::SeqCst);
    }
}

fn playback_callback_f32(
    data: &mut [f32],
    consumer: &mut AudioConsumer<f32>,
    is_playing: &Arc<AtomicBool>,
    flush_requested: &Arc<AtomicBool>,
    mut echo_ref_producer: Option<&mut AudioProducer<f32>>,
    hardware_channels: u16,
) {
    if flush_requested.swap(false, Ordering::SeqCst) {
        consumer.clear();
        data.fill(0.0);
        is_playing.store(false, Ordering::SeqCst);
        return;
    }

    let available = consumer.len();
    if available > 0 {
        is_playing.store(true, Ordering::SeqCst);
        let read = consumer.pop_slice(data);
        if read < data.len() {
            data[read..].fill(0.0);
        }

        // C1: Capture exact hardware playback samples as AEC render reference at DAC delivery time
        if let Some(ref mut prod) = echo_ref_producer {
            let ch = (hardware_channels.max(1)) as usize;
            let frames = read / ch;
            let mut mono_scratch = [0.0f32; 1024];
            let chunk_frames = frames.min(mono_scratch.len());

            for f in 0..chunk_frames {
                let base = f * ch;
                let mono_val = match ch {
                    1 => data[base],
                    2 => (data[base] + data[base + 1]) * 0.5,
                    _ => {
                        let mut sum = 0.0f32;
                        for c in 0..ch {
                            sum += data[base + c];
                        }
                        sum / (ch as f32)
                    }
                };
                mono_scratch[f] = mono_val;
            }
            let _ = prod.push_slice(&mono_scratch[..chunk_frames]);
        }
    } else {
        is_playing.store(false, Ordering::SeqCst);
        data.fill(0.0);
    }
}

fn playback_callback_i16(
    data: &mut [i16],
    consumer: &mut AudioConsumer<f32>,
    is_playing: &Arc<AtomicBool>,
    flush_requested: &Arc<AtomicBool>,
    mut echo_ref_producer: Option<&mut AudioProducer<f32>>,
    hardware_channels: u16,
) {
    if flush_requested.swap(false, Ordering::SeqCst) {
        consumer.clear();
        data.fill(0);
        is_playing.store(false, Ordering::SeqCst);
        return;
    }

    let available = consumer.len();
    if available > 0 {
        is_playing.store(true, Ordering::SeqCst);

        // H5: Zero-allocation stack buffer eliminates real-time thread GC/allocator jitter
        let mut temp = [0.0f32; 2048];
        let n = data.len().min(temp.len());
        let read = consumer.pop_slice(&mut temp[..n]);

        for i in 0..read {
            let s = temp[i].clamp(-1.0, 1.0);
            data[i] = if s >= 0.0 {
                (s * 32767.0).round() as i16
            } else {
                (s * 32768.0).round() as i16
            };
        }
        if read < data.len() {
            data[read..].fill(0);
        }

        // C1: Capture reference samples at physical playback time
        if let Some(ref mut prod) = echo_ref_producer {
            let ch = (hardware_channels.max(1)) as usize;
            let frames = read / ch;
            let mut mono_scratch = [0.0f32; 1024];
            let chunk_frames = frames.min(mono_scratch.len());

            for f in 0..chunk_frames {
                let base = f * ch;
                let mono_val = match ch {
                    1 => temp[base],
                    2 => (temp[base] + temp[base + 1]) * 0.5,
                    _ => {
                        let mut sum = 0.0f32;
                        for c in 0..ch {
                            sum += temp[base + c];
                        }
                        sum / (ch as f32)
                    }
                };
                mono_scratch[f] = mono_val;
            }
            let _ = prod.push_slice(&mono_scratch[..chunk_frames]);
        }
    } else {
        is_playing.store(false, Ordering::SeqCst);
        data.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playback_callback_normal_and_flush() {
        let (mut prod, mut cons) = create_audio_ring_buffer::<f32>(64);
        let (mut ref_prod, mut ref_cons) = create_audio_ring_buffer::<f32>(64);
        let is_playing = Arc::new(AtomicBool::new(false));
        let flush_requested = Arc::new(AtomicBool::new(false));

        // 1. Normal playback with AEC reference capture
        prod.push_slice(&[0.5, 0.5, 0.5, 0.5]);
        let mut buffer = [0.0f32; 4];
        playback_callback_f32(
            &mut buffer,
            &mut cons,
            &is_playing,
            &flush_requested,
            Some(&mut ref_prod),
            1,
        );
        assert_eq!(buffer, [0.5, 0.5, 0.5, 0.5]);
        assert!(is_playing.load(Ordering::SeqCst));

        // Verify that echo reference buffer received the played samples in real-time
        let mut ref_out = [0.0f32; 4];
        let ref_read = ref_cons.pop_slice(&mut ref_out);
        assert_eq!(ref_read, 4);
        assert_eq!(ref_out, [0.5, 0.5, 0.5, 0.5]);

        // 2. Queue more and trigger flush (barge-in scenario)
        prod.push_slice(&[0.8, 0.8, 0.8, 0.8]);
        flush_requested.store(true, Ordering::SeqCst);

        let mut buffer2 = [1.0f32; 4];
        playback_callback_f32(
            &mut buffer2,
            &mut cons,
            &is_playing,
            &flush_requested,
            Some(&mut ref_prod),
            1,
        );
        assert_eq!(buffer2, [0.0, 0.0, 0.0, 0.0]); // Silenced
        assert!(!is_playing.load(Ordering::SeqCst));
        assert!(cons.is_empty()); // Ring buffer drained
    }

    #[test]
    fn test_playback_buffer_capacity_absorbs_20s_burst() {
        // At 48kHz stereo, a 20s speech burst produces:
        // 48,000 samples/sec * 2 channels * 20s = 1,920,000 samples
        let sample_rate = 48000usize;
        let channels = 2usize;
        let burst_20s_samples = sample_rate * channels * 20;

        let buffer_capacity = sample_rate * channels * PLAYBACK_BUFFER_SECONDS;
        assert!(
            buffer_capacity >= burst_20s_samples,
            "Buffer capacity {} must be >= 20s burst samples {}",
            buffer_capacity,
            burst_20s_samples
        );
        let (mut prod, _cons) = create_audio_ring_buffer::<f32>(buffer_capacity);
        let dummy_chunk = vec![0.05f32; burst_20s_samples];
        let pushed = prod.push_slice(&dummy_chunk);
        assert_eq!(pushed, burst_20s_samples, "Ring buffer must absorb 20s burst without dropping");
    }
}
