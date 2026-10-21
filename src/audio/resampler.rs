// ============================================================
// N.E.X.A 3.3 — AUDIO RESAMPLER & FORMAT CONVERTER
// Zero-Allocation / Fast Linear Interpolation & Channel Mixers
// ============================================================

/// Downmix interleaved multi-channel audio (stereo, quad, surround) to mono by averaging channels.
pub fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 || interleaved.is_empty() {
        return interleaved.to_vec();
    }
    let ch = channels as usize;
    let mut mono = Vec::with_capacity(interleaved.len() / ch);
    for chunk in interleaved.chunks_exact(ch) {
        let sum: f32 = chunk.iter().sum();
        mono.push(sum / channels as f32);
    }
    mono
}

/// Downmix interleaved stereo (L, R, L, R...) into mono by averaging channels.
#[allow(dead_code)]
pub fn downmix_stereo_to_mono(stereo: &[f32]) -> Vec<f32> {
    downmix_to_mono(stereo, 2)
}

/// Upmix mono audio to interleaved multi-channel (e.g. mono to stereo, quad).
pub fn upmix_mono_to_channels(mono: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return mono.to_vec();
    }
    let ch = channels as usize;
    let mut interleaved = Vec::with_capacity(mono.len() * ch);
    for &sample in mono {
        for _ in 0..ch {
            interleaved.push(sample);
        }
    }
    interleaved
}

/// Upmix mono to interleaved stereo (M -> L, R).
#[allow(dead_code)]
pub fn upmix_mono_to_stereo(mono: &[f32]) -> Vec<f32> {
    upmix_mono_to_channels(mono, 2)
}

/// Resample single-channel f32 audio using high-precision linear interpolation.
pub fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == to_rate {
        return input.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let target_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut output = Vec::with_capacity(target_len);

    for i in 0..target_len {
        let src_idx_f = i as f64 * ratio;
        let idx0 = src_idx_f.floor() as usize;
        let frac = (src_idx_f - idx0 as f64) as f32;

        if idx0 + 1 < input.len() {
            let s0 = input[idx0];
            let s1 = input[idx0 + 1];
            output.push(s0 + frac * (s1 - s0));
        } else if idx0 < input.len() {
            output.push(input[idx0]);
        } else {
            output.push(0.0);
        }
    }

    output
}

/// Convert f32 normalized audio (-1.0 to 1.0) to 16-bit signed PCM (i16).
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| {
            let clamped = s.clamp(-1.0, 1.0);
            if clamped >= 0.0 {
                (clamped * 32767.0).round() as i16
            } else {
                (clamped * 32768.0).round() as i16
            }
        })
        .collect()
}

/// Convert 16-bit signed PCM (i16) to f32 normalized audio (-1.0 to 1.0).
pub fn i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&s| s as f32 / 32768.0).collect()
}

/// Convert raw little-endian PCM bytes to i16 slice.
#[allow(clippy::chunks_exact_to_as_chunks)]
pub fn pcm_bytes_to_i16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

/// Convert i16 slice to raw little-endian PCM bytes.
pub fn i16_to_pcm_bytes(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_downmix_stereo() {
        let stereo = vec![1.0, 0.5, -0.5, -1.0];
        let mono = downmix_stereo_to_mono(&stereo);
        assert_eq!(mono, vec![0.75, -0.75]);
    }

    #[test]
    fn test_downmix_quad_channel() {
        let quad = vec![1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0];
        let mono = downmix_to_mono(&quad, 4);
        assert_eq!(mono, vec![1.0, -1.0]);
    }

    #[test]
    fn test_upmix_mono() {
        let mono = vec![0.5, -0.5];
        let stereo = upmix_mono_to_stereo(&mono);
        assert_eq!(stereo, vec![0.5, 0.5, -0.5, -0.5]);
    }

    #[test]
    fn test_upmix_quad_channel() {
        let mono = vec![0.5];
        let quad = upmix_mono_to_channels(&mono, 4);
        assert_eq!(quad, vec![0.5, 0.5, 0.5, 0.5]);
    }

    #[test]
    fn test_resample_48k_to_16k() {
        // 48kHz to 16kHz is a 3:1 downsample
        let input = vec![0.0, 0.33, 0.66, 1.0, 0.66, 0.33];
        let resampled = resample_linear(&input, 48000, 16000);
        assert_eq!(resampled.len(), 2);
        assert!((resampled[0] - 0.0).abs() < 1e-4);
        assert!((resampled[1] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_pcm_conversion_roundtrip() {
        let original_i16 = vec![0, 16384, -16384, 32767, -32768];
        let bytes = i16_to_pcm_bytes(&original_i16);
        assert_eq!(bytes.len(), 10);
        let decoded = pcm_bytes_to_i16(&bytes);
        assert_eq!(decoded, original_i16);

        let f32_samples = i16_to_f32(&original_i16);
        let back_to_i16 = f32_to_i16(&f32_samples);
        for (a, b) in original_i16.iter().zip(back_to_i16.iter()) {
            assert!((a - b).abs() <= 1); // Quantization rounding tolerance
        }
    }
}
