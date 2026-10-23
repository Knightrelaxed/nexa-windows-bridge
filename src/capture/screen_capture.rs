// ============================================================
// WINDOWS GPU SCREEN CAPTURE ENGINE
// Direct3D 11 / Windows.Graphics.Capture API Implementation
// ============================================================

use super::encoder::{FrameEncoder, FrameRateLimiter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tracing::{info, warn};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

pub struct ScreenCaptureHandler {
    frame_tx: Sender<String>,
    rate_limiter: FrameRateLimiter,
    encoder: FrameEncoder,
    is_active: Arc<AtomicBool>,
}

impl GraphicsCaptureApiHandler for ScreenCaptureHandler {
    type Flags = (Sender<String>, u32, Arc<AtomicBool>);
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let (tx, fps, is_active) = ctx.flags;
        info!(
            "[CAPTURE] Initializing D3D11 screen capture session (Target: {} FPS)...",
            fps
        );
        Ok(Self {
            frame_tx: tx,
            rate_limiter: FrameRateLimiter::new(fps),
            // Quality 55 (~120KB per frame): sharp legible text for Gemini Live Vision without choking audio bandwidth
            encoder: FrameEncoder::new(55),
            is_active,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if !self.is_active.load(Ordering::Relaxed) {
            info!("[CAPTURE] Capture stop signaled, stopping session...");
            capture_control.stop();
            return Ok(());
        }

        if !self.rate_limiter.should_capture() {
            return Ok(());
        }

        let buffer = frame.buffer()?;
        match self.encoder.encode_frame_buffer(&buffer) {
            Ok(base64_str) => {
                // Non-blocking try_send: drops frame if previous frame is still transmitting
                // This guarantees zero frame backlog and preserves 100% of WebSocket bandwidth for voice
                match self.frame_tx.try_send(base64_str) {
                    Ok(_) => {}
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        // Previous frame still in flight: drop stale frame
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        capture_control.stop();
                    }
                }
            }
            Err(e) => {
                warn!("[CAPTURE] Frame compression failed: {}", e);
            }
        }

        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        info!("[CAPTURE] Direct3D 11 screen capture session closed.");
        self.is_active.store(false, Ordering::SeqCst);
        Ok(())
    }
}

pub struct ScreenCaptureController {
    is_active: Arc<AtomicBool>,
    control: Option<CaptureControl<ScreenCaptureHandler, Box<dyn std::error::Error + Send + Sync>>>,
}

impl ScreenCaptureController {
    /// Check if native screen capture is supported on this machine
    #[allow(dead_code)]
    pub fn is_supported() -> bool {
        Monitor::primary().is_ok()
    }

    /// Start 1 FPS Direct3D 11 screen capture on the primary monitor
    pub fn start(frame_tx: Sender<String>, fps: u32) -> Result<Self, String> {
        let primary_monitor = Monitor::primary()
            .map_err(|e| format!("Primary monitor not found: {}", e))?;

        let is_active = Arc::new(AtomicBool::new(true));

        let settings = Settings::new(
            primary_monitor,
            CursorCaptureSettings::WithoutCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Custom(std::time::Duration::from_millis(1000)),
            DirtyRegionSettings::Default,
            ColorFormat::Rgba8,
            (frame_tx, fps, Arc::clone(&is_active)),
        );

        let control = ScreenCaptureHandler::start_free_threaded(settings)
            .map_err(|e| format!("Failed to start screen capture: {}", e))?;

        info!("[CAPTURE] ✅ Direct3D 11 screen capture active (1 FPS, Borderless, WithoutCursor).");

        Ok(Self {
            is_active,
            control: Some(control),
        })
    }

    /// Stop screen capture session cleanly
    pub fn stop(&mut self) {
        self.is_active.store(false, Ordering::SeqCst);
        if let Some(control) = self.control.take() {
            let _ = control.stop();
            info!("[CAPTURE] Screen capture stopped.");
        }
    }

    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        self.is_active.load(Ordering::Relaxed)
    }
}

impl Drop for ScreenCaptureController {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screen_capture_support_check() {
        // In desktop Windows environment, Monitor::primary() should be checked without panicking
        let supported = ScreenCaptureController::is_supported();
        println!("Screen capture supported: {}", supported);
    }
}
