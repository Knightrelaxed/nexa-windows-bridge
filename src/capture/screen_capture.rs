// ============================================================
// WINDOWS GPU SCREEN CAPTURE ENGINE
// Direct3D 11 / Windows.Graphics.Capture API Implementation
// ============================================================

use super::encoder::{FrameEncoder, FrameRateLimiter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
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
    frame_tx: watch::Sender<Option<(u64, String)>>,
    rate_limiter: FrameRateLimiter,
    encoder: FrameEncoder,
    last_sent: Instant,
    is_active: Arc<AtomicBool>,
}

impl GraphicsCaptureApiHandler for ScreenCaptureHandler {
    type Flags = (watch::Sender<Option<(u64, String)>>, u32, Arc<AtomicBool>);
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
            last_sent: Instant::now(),
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

        let arrival = super::now_ms();
        super::LAST_ARRIVAL_MS.store(arrival, Ordering::SeqCst);

        let burst = arrival < super::BURST_UNTIL_MS.load(Ordering::SeqCst);
        let due = if burst {
            // Burst mode: batasi maksimal ~8 FPS (interval >= 120ms) agar mendapatkan frame pasca-aksi
            // dengan cepat tanpa membanjiri WebSocket dengan puluhan JPEG per detik.
            arrival.saturating_sub(super::LAST_CAPTURE_MS.load(Ordering::SeqCst)) >= 120
        } else {
            self.rate_limiter.should_capture()
        };

        if !due {
            return Ok(());
        }

        self.last_sent = Instant::now();
        super::LAST_CAPTURE_MS.store(arrival, Ordering::SeqCst);

        let buffer = frame.buffer()?;

        static CHECK: std::sync::Once = std::sync::Once::new();
        CHECK.call_once(|| {
            let s = crate::automation::get_primary_screen_dimensions();
            if (buffer.width() as i32, buffer.height() as i32) != (s.width, s.height) {
                warn!(
                    "[CAPTURE] ⚠️ Frame {}x{} != layar {}x{}: DPI awareness/skala salah, semua koordinat akan meleset",
                    buffer.width(),
                    buffer.height(),
                    s.width,
                    s.height
                );
            } else {
                info!(
                    "[CAPTURE] ✅ Frame dimensions {}x{} verified matching primary screen {}x{}",
                    buffer.width(),
                    buffer.height(),
                    s.width,
                    s.height
                );
            }
        });

        match self.encoder.encode_frame_buffer(&buffer) {
            Ok(base64_str) => {
                // watch::Sender guarantees latest frame replaces unread frame with zero latency and zero backlog
                let _ = self.frame_tx.send(Some((arrival, base64_str)));
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
        super::IS_CAPTURE_RUNNING.store(false, Ordering::SeqCst);
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

    /// Start Direct3D 11 screen capture on the primary monitor
    pub fn start(frame_tx: watch::Sender<Option<(u64, String)>>, fps: u32) -> Result<Self, String> {
        let primary_monitor = Monitor::primary()
            .map_err(|e| format!("Primary monitor not found: {}", e))?;

        let is_active = Arc::new(AtomicBool::new(true));

        let settings = Settings::new(
            primary_monitor,
            CursorCaptureSettings::WithoutCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Custom(Duration::from_millis(100)),
            DirtyRegionSettings::Default,
            ColorFormat::Rgba8,
            (frame_tx, fps, Arc::clone(&is_active)),
        );

        let control = ScreenCaptureHandler::start_free_threaded(settings)
            .map_err(|e| format!("Failed to start screen capture: {}", e))?;

        super::IS_CAPTURE_RUNNING.store(true, Ordering::SeqCst);
        info!("[CAPTURE] ✅ Direct3D 11 screen capture active (1 FPS, Borderless, WithoutCursor).");

        Ok(Self {
            is_active,
            control: Some(control),
        })
    }

    /// Stop screen capture session cleanly
    pub fn stop(&mut self) {
        self.is_active.store(false, Ordering::SeqCst);
        super::IS_CAPTURE_RUNNING.store(false, Ordering::SeqCst);
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
