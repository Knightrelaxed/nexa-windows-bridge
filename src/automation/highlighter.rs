//! Stealth Overlay Window & Highlight Controller (Milestone 6.3)
//!
//! Provides hardware-accelerated DirectComposition / Win32 transparent click-through
//! overlays for bounding boxes (BOX), rings (RING), pointers (POINTER), spotlight (SPOTLIGHT),
//! and 300ms click ripple indicators.

#![allow(dead_code)]

use crate::automation::element_finder::smart_snap_bounding_box;
use crate::automation::input::{denormalize_to_pixels, get_primary_screen_dimensions};
use crate::ui::OverlayWindow;
use slint::ComponentHandle;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};
use windows_sys::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows_sys::Win32::UI::Controls::MARGINS;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Visual styling options for screen highlights
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightStyle {
    Box,
    Ring,
    Pointer,
    Spotlight,
}

impl HighlightStyle {
    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_uppercase().as_str() {
            "RING" => HighlightStyle::Ring,
            "POINTER" => HighlightStyle::Pointer,
            "SPOTLIGHT" => HighlightStyle::Spotlight,
            _ => HighlightStyle::Box,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            HighlightStyle::Box => "BOX",
            HighlightStyle::Ring => "RING",
            HighlightStyle::Pointer => "POINTER",
            HighlightStyle::Spotlight => "SPOTLIGHT",
        }
    }
}

/// Clamps duration between 500 ms and 15,000 ms (default 4,000 ms)
pub fn clamp_highlight_duration(duration_ms: u64) -> u64 {
    if duration_ms == 0 {
        4000
    } else {
        duration_ms.clamp(500, 15000)
    }
}

/// Request parameters for highlighting an element
#[derive(Debug, Clone, PartialEq)]
pub struct HighlightRequest {
    pub ymin: f64,
    pub xmin: f64,
    pub ymax: f64,
    pub xmax: f64,
    pub style: HighlightStyle,
    pub label: String,
    pub duration_ms: u64,
}

impl HighlightRequest {
    pub fn new(
        ymin: f64,
        xmin: f64,
        ymax: f64,
        xmax: f64,
        style: HighlightStyle,
        label: String,
        duration_ms: u64,
    ) -> Self {
        Self {
            ymin,
            xmin,
            ymax,
            xmax,
            style,
            label,
            duration_ms: clamp_highlight_duration(duration_ms),
        }
    }
}

/// Apply Win32 Stealth windowing flags (click-through, topmost, noactivate).
///
/// Retries up to `max_retries` times with `retry_delay_ms` between attempts,
/// because the HWND may not be findable immediately after Slint's `w.show()`.
/// Returns true if styles were successfully applied.
pub fn apply_stealth_window_styles() -> bool {
    apply_stealth_window_styles_with_retry(5, 20)
}

fn apply_stealth_window_styles_with_retry(max_retries: u32, retry_delay_ms: u64) -> bool {
    for attempt in 0..max_retries {
        if try_apply_stealth_styles() {
            return true;
        }
        if attempt + 1 < max_retries {
            std::thread::sleep(Duration::from_millis(retry_delay_ms));
        }
    }
    warn!("[HIGHLIGHTER] Failed to apply stealth styles after {} attempts", max_retries);
    false
}

static LAST_STYLED_HWND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn try_apply_stealth_styles() -> bool {
    unsafe {
        let title: Vec<u16> = "N.E.X.A Overlay\0".encode_utf16().collect();
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd.is_null() {
            debug!("[HIGHLIGHTER] Overlay window HWND not found yet");
            return false;
        }

        let hwnd_val = hwnd as usize;

        // If styles were already applied to this exact HWND, skip heavy DWM recomputation (Audit C1)
        if LAST_STYLED_HWND.load(Ordering::Relaxed) == hwnd_val {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
            return true;
        }

        // 1. Configure stealth extended window styles:
        // - WS_EX_TOPMOST: Stays above all regular windows
        // - WS_EX_TRANSPARENT: Mouse clicks pass straight through without intercepting
        // - WS_EX_NOACTIVATE: Does not steal focus or keyboard input
        // - WS_EX_TOOLWINDOW: Prevents overlay from appearing in Alt-Tab task switcher (Claude Bug M5)
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let required_flags = (WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW) as i32;
        if (ex_style & required_flags) != required_flags {
            SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style | required_flags);
        }

        // 2. Extend DWM frame into client area (-1 margins) to guarantee hardware alpha glass transparency
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins);

        // 3. Exclude overlay from screen capture so Gemini Vision never captures its own overlays! (Audit #8)
        const WDA_EXCLUDEFROMCAPTURE: u32 = 0x00000011;
        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);

        // 4. Position window to cover full primary screen, topmost, no activation
        let screen = get_primary_screen_dimensions();
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            screen.width,
            screen.height,
            SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );

        LAST_STYLED_HWND.store(hwnd_val, Ordering::SeqCst);
        debug!(
            "[HIGHLIGHTER] Applied stealth styles (WS_EX_TRANSPARENT + WS_EX_TOOLWINDOW + WDA_EXCLUDEFROMCAPTURE + DWM Glass) to HWND 0x{:X}",
            hwnd_val
        );
        true
    }
}

/// Controller managing the lifetime, geometry, animations, and auto-dismiss of overlays
#[derive(Clone)]
pub struct HighlighterController {
    overlay_window: Option<slint::Weak<OverlayWindow>>,
    active_epoch: Arc<AtomicU64>,
    active_ripple_epoch: Arc<AtomicU64>,
}

impl HighlighterController {
    /// Create a controller with an optional weak Slint OverlayWindow instance
    pub fn new(overlay_window: Option<slint::Weak<OverlayWindow>>) -> Self {
        Self {
            overlay_window,
            active_epoch: Arc::new(AtomicU64::new(0)),
            active_ripple_epoch: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Display an element highlight with magnetic snapping, optional label, and auto-dissolve
    pub fn show_highlight(&self, req: HighlightRequest) {
        let epoch = self.active_epoch.fetch_add(1, Ordering::SeqCst) + 1;

        // Perform Magnetic Smart-Snap via Windows UI Automation
        let snapped = smart_snap_bounding_box(req.ymin, req.xmin, req.ymax, req.xmax);

        let screen = get_primary_screen_dimensions();
        let (px0, py0) = denormalize_to_pixels(snapped.xmin, snapped.ymin, screen.width, screen.height);
        let (px1, py1) = denormalize_to_pixels(snapped.xmax, snapped.ymax, screen.width, screen.height);

        let target_x = px0 as f32;
        let target_y = py0 as f32;
        let target_w = (px1 - px0).max(12) as f32;
        let target_h = (py1 - py0).max(12) as f32;

        let style_str = req.style.as_str().to_string();
        let label = req.label.clone();
        let duration = Duration::from_millis(req.duration_ms);

        info!(
            "[HIGHLIGHTER] 🌟 show_highlight [style={}, label='{}']: rect=({}, {} - {}x{}) duration={}ms",
            style_str, label, target_x, target_y, target_w, target_h, req.duration_ms
        );

        if let Some(ref win_weak) = self.overlay_window {
            let win_handle = win_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = win_handle.upgrade() {
                    // Divide physical pixels by Slint window scale factor for High-DPI parity (Claude Bug M6)
                    let scale = w.window().scale_factor();
                    let scale = if scale > 0.0 { scale } else { 1.0 };

                    // Set all properties BEFORE showing the window
                    w.set_window_width(screen.width as f32 / scale);
                    w.set_window_height(screen.height as f32 / scale);
                    w.set_target_x(target_x / scale);
                    w.set_target_y(target_y / scale);
                    w.set_target_w(target_w / scale);
                    w.set_target_h(target_h / scale);
                    w.set_highlight_style(style_str.into());
                    w.set_highlight_label(label.into());
                    w.set_highlight_opacity(1.0);
                    w.set_highlight_visible(true);
                    let _ = w.show();
                }
            });

            // Apply stealth styles (with retry) safely guarded by Tokio handle (Claude Bug M7)
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    // Small delay to let Slint process the show() and create the HWND
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    apply_stealth_window_styles();
                });

                // Spawn auto-dissolve timer for the current epoch
                let active_epoch = Arc::clone(&self.active_epoch);
                let win_handle_timer = win_weak.clone();

                handle.spawn(async move {
                    tokio::time::sleep(duration).await;
                    if active_epoch.load(Ordering::SeqCst) == epoch {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = win_handle_timer.upgrade() {
                                w.set_highlight_visible(false);
                                if !w.get_ripple_visible() {
                                    let _ = w.hide();
                                }
                            }
                        });
                        debug!("[HIGHLIGHTER] Auto-dissolved highlight epoch {}", epoch);
                    }
                });
            }
        }
    }

    /// Smoothly dissolve the currently active highlight (e.g. on user click or action)
    pub fn dissolve_active_highlight(&self) {
        if let Some(ref win_weak) = self.overlay_window {
            let win_hide = win_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = win_hide.upgrade() {
                    w.set_highlight_visible(false);
                    if !w.get_ripple_visible() {
                        let _ = w.hide();
                    }
                }
            });
        }
    }

    /// Point cursor at specific normalized coordinates (0..=1000)
    pub fn point_cursor(&self, norm_x: f64, norm_y: f64, duration_ms: u64) {
        let req = HighlightRequest::new(
            norm_y,
            norm_x,
            norm_y,
            norm_x,
            HighlightStyle::Pointer,
            "".to_string(),
            duration_ms,
        );
        self.show_highlight(req);
    }

    /// Display expanding 300ms click ripple indicator at physical screen coordinates (px, py)
    pub fn show_click_ripple(&self, px: i32, py: i32) {
        let rx = px as f32;
        let ry = py as f32;

        let ripple_epoch = self.active_ripple_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        debug!(
            "[HIGHLIGHTER] 🔘 show_click_ripple at ({}, {}) [ripple_epoch={}]",
            px, py, ripple_epoch
        );

        if let Some(ref win_weak) = self.overlay_window {
            let win_handle = win_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = win_handle.upgrade() {
                    let scale = w.window().scale_factor();
                    let scale = if scale > 0.0 { scale } else { 1.0 };
                    w.set_ripple_x(rx / scale);
                    w.set_ripple_y(ry / scale);
                    w.set_ripple_radius(12.0);
                    w.set_ripple_opacity(0.85);
                    w.set_ripple_visible(true);
                    let _ = w.show();
                }
            });

            // Ensure window is transparent, topmost, and click-through
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    apply_stealth_window_styles();
                });

                // Run 300ms ripple expansion animation with atomic epoch cancellation
                let win_anim = win_weak.clone();
                let active_ripple_epoch = Arc::clone(&self.active_ripple_epoch);

                handle.spawn(async move {
                    let steps = 10;
                    let step_delay = Duration::from_millis(30);

                    for i in 1..=steps {
                        tokio::time::sleep(step_delay).await;

                        // Bail out immediately if a newer ripple has superseded this one
                        if active_ripple_epoch.load(Ordering::SeqCst) != ripple_epoch {
                            return;
                        }

                        let progress = (i as f32) / (steps as f32);
                        let radius = 12.0 + (33.0 * progress); // 12px -> 45px
                        let opacity = 0.85 * (1.0 - progress);

                        let win_step = win_anim.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = win_step.upgrade() {
                                w.set_ripple_radius(radius);
                                w.set_ripple_opacity(opacity);
                            }
                        });
                    }

                    // Finish ripple only if this is still the active epoch
                    if active_ripple_epoch.load(Ordering::SeqCst) == ripple_epoch {
                        let win_final = win_anim.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = win_final.upgrade() {
                                w.set_ripple_visible(false);
                                if !w.get_highlight_visible() {
                                    let _ = w.hide();
                                }
                            }
                        });
                    }
                });
            }
        }
    }

    /// Clear all active highlights and ripples immediately
    pub fn clear_all(&self) {
        self.active_epoch.fetch_add(1, Ordering::SeqCst);
        self.active_ripple_epoch.fetch_add(1, Ordering::SeqCst);
        info!("[HIGHLIGHTER] 🧹 Cleared all screen highlights and overlays");

        if let Some(ref win_weak) = self.overlay_window {
            let win_handle = win_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = win_handle.upgrade() {
                    w.set_highlight_visible(false);
                    w.set_ripple_visible(false);
                    w.set_highlight_opacity(1.0);
                    let _ = w.hide();
                }
            });
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_style_parsing() {
        assert_eq!(HighlightStyle::from_str_loose("BOX"), HighlightStyle::Box);
        assert_eq!(HighlightStyle::from_str_loose("box"), HighlightStyle::Box);
        assert_eq!(HighlightStyle::from_str_loose("RING"), HighlightStyle::Ring);
        assert_eq!(HighlightStyle::from_str_loose("ring"), HighlightStyle::Ring);
        assert_eq!(HighlightStyle::from_str_loose("POINTER"), HighlightStyle::Pointer);
        assert_eq!(HighlightStyle::from_str_loose("pointer"), HighlightStyle::Pointer);
        assert_eq!(HighlightStyle::from_str_loose("SPOTLIGHT"), HighlightStyle::Spotlight);
        assert_eq!(HighlightStyle::from_str_loose("spotlight"), HighlightStyle::Spotlight);
        assert_eq!(HighlightStyle::from_str_loose("unknown"), HighlightStyle::Box);

        assert_eq!(HighlightStyle::Box.as_str(), "BOX");
        assert_eq!(HighlightStyle::Ring.as_str(), "RING");
        assert_eq!(HighlightStyle::Pointer.as_str(), "POINTER");
        assert_eq!(HighlightStyle::Spotlight.as_str(), "SPOTLIGHT");
    }

    #[test]
    fn test_clamp_highlight_duration() {
        assert_eq!(clamp_highlight_duration(0), 4000); // Default
        assert_eq!(clamp_highlight_duration(200), 500); // Clamped minimum
        assert_eq!(clamp_highlight_duration(5000), 5000);
        assert_eq!(clamp_highlight_duration(25000), 15000); // Clamped maximum
    }

    #[test]
    fn test_highlight_request_constructor() {
        let req = HighlightRequest::new(
            100.0,
            200.0,
            300.0,
            400.0,
            HighlightStyle::Ring,
            "Target Icon".to_string(),
            6000,
        );

        assert_eq!(req.ymin, 100.0);
        assert_eq!(req.xmin, 200.0);
        assert_eq!(req.ymax, 300.0);
        assert_eq!(req.xmax, 400.0);
        assert_eq!(req.style, HighlightStyle::Ring);
        assert_eq!(req.label, "Target Icon");
        assert_eq!(req.duration_ms, 6000);
    }

    #[test]
    fn test_highlighter_controller_headless_lifecycle() {
        let controller = HighlighterController::new(None);

        // Can invoke show_highlight without panicking in headless mode
        let req = HighlightRequest::new(
            100.0,
            100.0,
            200.0,
            200.0,
            HighlightStyle::Box,
            "Submit".to_string(),
            3000,
        );
        controller.show_highlight(req);

        // Can invoke point_cursor
        controller.point_cursor(500.0, 500.0, 2000);

        // Can invoke show_click_ripple
        controller.show_click_ripple(960, 540);

        // Can clear all
        controller.clear_all();
    }
}
