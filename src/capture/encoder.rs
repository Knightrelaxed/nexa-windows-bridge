// ============================================================
// N.E.X.A 3.3 — WINDOWS BRIDGE FRAME ENCODER & RATE LIMITER
// Direct3D 11 GPU Frame to Base64 JPEG Compression Pipeline
// ============================================================

use base64::prelude::*;
use std::time::{Duration, Instant};

/// Rate limiter to enforce strict frame clamping (e.g. 1 FPS for Gemini Live Vision).
#[derive(Debug, Clone)]
pub struct FrameRateLimiter {
    interval: Duration,
    last_capture: Option<Instant>,
}

impl FrameRateLimiter {
    pub fn new(target_fps: u32) -> Self {
        let fps = target_fps.max(1);
        let interval_ms = (1000.0 / fps as f64).round() as u64;
        Self {
            interval: Duration::from_millis(interval_ms),
            last_capture: None,
        }
    }

    /// Check if enough time has elapsed to capture/process the next frame.
    /// Returns `true` and resets the timer if ready, otherwise `false`.
    pub fn should_capture(&mut self) -> bool {
        let now = Instant::now();
        match self.last_capture {
            Some(last) if now.duration_since(last) < self.interval => false,
            _ => {
                self.last_capture = Some(now);
                true
            }
        }
    }

    #[allow(dead_code)]
    pub fn interval(&self) -> Duration {
        self.interval
    }
}

/// Zero-allocation frame encoder from raw RGBA/BGRA byte buffers to Base64 JPEG.
pub struct FrameEncoder {
    pub quality: u8,
    pub unpadded_buffer: Vec<u8>,
    pub rgb_buffer: Vec<u8>,
    pub jpeg_buffer: Vec<u8>,
}

impl Default for FrameEncoder {
    fn default() -> Self {
        Self::new(70)
    }
}

impl FrameEncoder {
    pub fn new(quality: u8) -> Self {
        Self {
            quality: quality.clamp(1, 100),
            unpadded_buffer: Vec::with_capacity(1920 * 1080 * 4),
            rgb_buffer: Vec::with_capacity(1920 * 1080 * 3),
            jpeg_buffer: Vec::with_capacity(128 * 1024),
        }
    }

    /// Encode a Windows.Graphics.Capture FrameBuffer into a compressed JPEG Base64 string.
    pub fn encode_frame_buffer(
        &mut self,
        frame_buffer: &windows_capture::frame::FrameBuffer,
    ) -> Result<String, String> {
        let width = frame_buffer.width();
        let height = frame_buffer.height();
        if width == 0 || height == 0 {
            return Err("Invalid frame dimensions: width or height is 0".to_string());
        }

        let unpadded = frame_buffer.as_nopadding_buffer(&mut self.unpadded_buffer);
        let pixel_count = (width as usize) * (height as usize);
        let expected_len = pixel_count * 4;
        let slice = &unpadded[..expected_len.min(unpadded.len())];

        Self::copy_rgba_to_rgb(&mut self.rgb_buffer, slice, pixel_count);

        self.jpeg_buffer.clear();
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut self.jpeg_buffer, self.quality);

        encoder
            .encode(
                &self.rgb_buffer,
                width,
                height,
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("JPEG encoding failed: {}", e))?;

        let base64_str = BASE64_STANDARD.encode(&self.jpeg_buffer);
        Ok(base64_str)
    }

    /// Encode raw tightly-packed RGBA8 pixel bytes into a compressed JPEG Base64 string.
    #[allow(dead_code)]
    pub fn encode_rgba_to_base64_jpeg(
        &mut self,
        rgba_bytes: &[u8],
        width: u32,
        height: u32,
    ) -> Result<String, String> {
        if width == 0 || height == 0 {
            return Err("Invalid frame dimensions: width or height is 0".to_string());
        }

        let pixel_count = (width as usize) * (height as usize);
        let expected_len = pixel_count * 4;
        if rgba_bytes.len() < expected_len {
            return Err(format!(
                "Buffer length mismatch: expected at least {} bytes, got {}",
                expected_len,
                rgba_bytes.len()
            ));
        }

        Self::copy_rgba_to_rgb(&mut self.rgb_buffer, &rgba_bytes[..expected_len], pixel_count);

        self.jpeg_buffer.clear();
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut self.jpeg_buffer, self.quality);

        encoder
            .encode(
                &self.rgb_buffer,
                width,
                height,
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| format!("JPEG encoding failed: {}", e))?;

        let base64_str = BASE64_STANDARD.encode(&self.jpeg_buffer);
        Ok(base64_str)
    }

    /// Strip the alpha channel from RGBA8 into pre-allocated contiguous RGB8 buffer.
    fn copy_rgba_to_rgb(rgb_buffer: &mut Vec<u8>, rgba_slice: &[u8], pixel_count: usize) {
        rgb_buffer.clear();
        let required_len = pixel_count * 3;
        if rgb_buffer.capacity() < required_len {
            rgb_buffer.reserve(required_len - rgb_buffer.capacity());
        }

        let (chunks, _) = rgba_slice.as_chunks::<4>();
        for chunk in chunks {
            rgb_buffer.push(chunk[0]);
            rgb_buffer.push(chunk[1]);
            rgb_buffer.push(chunk[2]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_rate_limiter_1fps() {
        let mut limiter = FrameRateLimiter::new(1);
        assert_eq!(limiter.interval(), Duration::from_millis(1000));

        // First call should always return true
        assert!(limiter.should_capture());

        // Immediate subsequent call should return false (throttled)
        assert!(!limiter.should_capture());
    }

    #[test]
    fn test_frame_rate_limiter_clamp_minimum() {
        let limiter = FrameRateLimiter::new(0);
        assert_eq!(limiter.interval(), Duration::from_millis(1000));
    }

    #[test]
    fn test_frame_encoder_rgba_to_base64() {
        let mut encoder = FrameEncoder::new(75);
        let width = 64;
        let height = 64;
        let mut mock_rgba = vec![0u8; (width * height * 4) as usize];

        // Fill with a cyan pattern
        let (chunks, _) = mock_rgba.as_chunks_mut::<4>();
        for chunk in chunks {
            chunk[0] = 0;   // R
            chunk[1] = 229; // G
            chunk[2] = 255; // B
            chunk[3] = 255; // A
        }

        let res = encoder.encode_rgba_to_base64_jpeg(&mock_rgba, width, height);
        assert!(res.is_ok(), "Encoding failed: {:?}", res.err());

        let base64_str = res.unwrap();
        assert!(!base64_str.is_empty());

        // Verify Base64 decodes back to valid JPEG magic header (0xFF, 0xD8, 0xFF)
        let decoded = BASE64_STANDARD.decode(&base64_str).expect("Valid base64");
        assert!(decoded.len() > 10);
        assert_eq!(decoded[0], 0xFF);
        assert_eq!(decoded[1], 0xD8);
        assert_eq!(decoded[2], 0xFF);
    }

    #[test]
    fn test_frame_encoder_invalid_dimensions() {
        let mut encoder = FrameEncoder::new(70);
        let res = encoder.encode_rgba_to_base64_jpeg(&[], 0, 0);
        assert!(res.is_err());
    }
}
