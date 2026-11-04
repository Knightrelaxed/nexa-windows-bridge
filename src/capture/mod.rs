// ============================================================
// WINDOWS BRIDGE CAPTURE MODULE
// Direct3D 11 GPU Screen Capture & TurboJPEG Frame Pipeline
// ============================================================

pub mod encoder;
pub mod screen_capture;

#[allow(unused_imports)]
pub use encoder::{FrameEncoder, FrameRateLimiter};
pub use screen_capture::ScreenCaptureController;

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::OnceLock;
use std::time::Instant;

pub static LAST_CAPTURE_MS: AtomicU64 = AtomicU64::new(0);
pub static IS_CAPTURE_RUNNING: AtomicBool = AtomicBool::new(false);

pub static LAST_ARRIVAL_MS: AtomicU64 = AtomicU64::new(0);
pub static LAST_SENT_ARRIVAL_MS: AtomicU64 = AtomicU64::new(0);
pub static BURST_UNTIL_MS: AtomicU64 = AtomicU64::new(0);

pub fn now_ms() -> u64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_millis() as u64
}

