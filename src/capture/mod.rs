// ============================================================
// N.E.X.A 3.3 — WINDOWS BRIDGE CAPTURE MODULE
// Direct3D 11 GPU Screen Capture & TurboJPEG Frame Pipeline
// ============================================================

pub mod encoder;
pub mod screen_capture;

#[allow(unused_imports)]
pub use encoder::{FrameEncoder, FrameRateLimiter};
pub use screen_capture::ScreenCaptureController;
