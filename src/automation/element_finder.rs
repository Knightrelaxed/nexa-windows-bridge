//! Magnetic Smart-Snap via Windows UI Automation (Milestone 6.2)
//!
//! Queries Windows UI Automation to locate native OS controls at the target point
//! and snaps bounding boxes to pixel-perfect physical boundaries.

#![allow(dead_code)]

use crate::automation::input::{denormalize_to_pixels, get_primary_screen_dimensions};
use tracing::{debug, info, warn};
use windows::Win32::Foundation::POINT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};

/// RAII Guard ensuring CoUninitialize is called only after all COM objects are released
struct ComGuard {
    need_uninit: bool,
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.need_uninit {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

/// Physical pixel rectangle of a detected UI element
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PixelRect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn width(&self) -> i32 {
        self.right.saturating_sub(self.left).max(0)
    }

    pub fn height(&self) -> i32 {
        self.bottom.saturating_sub(self.top).max(0)
    }

    /// Check if dimensions are rational for an interactive UI control
    /// (e.g. at least 4x4 pixels, valid order, and not occupying >95% of the screen)
    pub fn is_valid_control(&self, screen_w: i32, screen_h: i32) -> bool {
        if self.right <= self.left || self.bottom <= self.top {
            return false;
        }

        let w = self.width();
        let h = self.height();

        if w < 4 || h < 4 {
            return false;
        }

        let max_w = ((screen_w as f64) * 0.95).round() as i32;
        let max_h = ((screen_h as f64) * 0.95).round() as i32;

        if w > max_w || h > max_h {
            // Likely root desktop, taskbar, or full display window, not an individual button/element (Audit N7)
            return false;
        }

        true
    }

    /// Check if a physical point (px, py) is contained within or near this rectangle
    pub fn contains_or_near(&self, px: i32, py: i32, tolerance: i32) -> bool {
        px >= self.left - tolerance
            && px <= self.right + tolerance
            && py >= self.top - tolerance
            && py <= self.bottom + tolerance
    }
}

/// Normalized bounding box with indicator whether it was snapped via UIA
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnappedRect {
    pub ymin: f64,
    pub xmin: f64,
    pub ymax: f64,
    pub xmax: f64,
    pub is_snapped: bool,
}

impl SnappedRect {
    /// Create a new SnappedRect with guaranteed ordered bounds and clamped to [0..=1000]
    pub fn new(ymin: f64, xmin: f64, ymax: f64, xmax: f64, is_snapped: bool) -> Self {
        let (clean_ymin, clean_ymax) = if ymin <= ymax {
            (ymin, ymax)
        } else {
            (ymax, ymin)
        };
        let (clean_xmin, clean_xmax) = if xmin <= xmax {
            (xmin, xmax)
        } else {
            (xmax, xmin)
        };

        Self {
            ymin: clean_ymin.clamp(0.0, 1000.0),
            xmin: clean_xmin.clamp(0.0, 1000.0),
            ymax: clean_ymax.clamp(0.0, 1000.0),
            xmax: clean_xmax.clamp(0.0, 1000.0),
            is_snapped,
        }
    }
}

/// Convert physical pixel bounds back into normalized coordinates (0..=1000)
pub fn pixel_rect_to_normalized(rect: &PixelRect, screen_w: i32, screen_h: i32) -> SnappedRect {
    let w = (screen_w.max(1)) as f64;
    let h = (screen_h.max(1)) as f64;

    let xmin = ((rect.left as f64 / w) * 1000.0).clamp(0.0, 1000.0);
    let ymin = ((rect.top as f64 / h) * 1000.0).clamp(0.0, 1000.0);
    let xmax = ((rect.right as f64 / w) * 1000.0).clamp(0.0, 1000.0);
    let ymax = ((rect.bottom as f64 / h) * 1000.0).clamp(0.0, 1000.0);

    SnappedRect::new(ymin, xmin, ymax, xmax, true)
}

/// Query Windows UI Automation to locate the element at physical screen coordinates (px, py).
/// Returns Some(PixelRect) if a valid control is found, or None.
pub fn find_element_bounds_at_point(px: i32, py: i32, screen_w: i32, screen_h: i32) -> Option<PixelRect> {
    unsafe {
        // Initialize COM library for the current thread (using MTA for worker thread compatibility)
        let co_init_res = CoInitializeEx(None, COINIT_MULTITHREADED);
        let need_uninit = co_init_res.is_ok();
        let _guard = ComGuard { need_uninit };

        let uia: Result<IUIAutomation, _> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
        let uia = match uia {
            Ok(u) => u,
            Err(err) => {
                debug!("[UIA] CoCreateInstance(CUIAutomation) failed: {:?}", err);
                return None;
            }
        };

        let pt = POINT { x: px, y: py };
        let elem = match uia.ElementFromPoint(pt) {
            Ok(e) => e,
            Err(err) => {
                debug!("[UIA] ElementFromPoint failed at ({}, {}): {:?}", px, py, err);
                return None;
            }
        };

        // Filter out Nexa Bridge's own process to never snap to own overlay or pill (Audit N7)
        if let Ok(pid) = elem.CurrentProcessId() {
            if pid as u32 == std::process::id() {
                debug!("[UIA] Element belongs to Nexa Bridge itself (PID {}), ignoring", pid);
                return None;
            }
        }

        let rect = match elem.CurrentBoundingRectangle() {
            Ok(r) => r,
            Err(err) => {
                debug!("[UIA] CurrentBoundingRectangle failed: {:?}", err);
                return None;
            }
        };

        let pixel_rect = PixelRect::new(rect.left, rect.top, rect.right, rect.bottom);
        if pixel_rect.is_valid_control(screen_w, screen_h) && pixel_rect.contains_or_near(px, py, 30) {
            Some(pixel_rect)
        } else {
            None
        }
    }
}

static UIA_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Smart-snap a bounding box with an explicit timeout guard (Claude Bug H8).
/// If UIA or cross-process COM hangs or exceeds `timeout`, immediately falls back
/// to the AI vision bounding box to prevent UI freezing.
pub fn smart_snap_bounding_box_timeout(
    ymin: f64,
    xmin: f64,
    ymax: f64,
    xmax: f64,
    timeout: std::time::Duration,
) -> SnappedRect {
    // Prevent unbounded thread leaks if target application COM hangs (Audit N7)
    if UIA_BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        debug!("[UIA] Previous query still in flight, immediately using Vision fallback to prevent thread leaks");
        return SnappedRect::new(ymin, xmin, ymax, xmax, false);
    }

    let screen = get_primary_screen_dimensions();
    let center_x = (xmin + xmax) / 2.0;
    let center_y = (ymin + ymax) / 2.0;
    let (px, py) = denormalize_to_pixels(center_x, center_y, screen.width, screen.height);

    let (tx, rx) = std::sync::mpsc::channel();
    let sw = screen.width;
    let sh = screen.height;
    std::thread::spawn(move || {
        let res = find_element_bounds_at_point(px, py, sw, sh);
        let _ = tx.send(res);
        UIA_BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
    });

    match rx.recv_timeout(timeout) {
        Ok(Some(snapped_px_rect)) => {
            let snapped = pixel_rect_to_normalized(&snapped_px_rect, screen.width, screen.height);
            info!(
                "[UIA] 🧲 Magnetic Snap SUCCESS: target=({:.1}, {:.1}) -> snapped box=[ymin={:.1}, xmin={:.1}, ymax={:.1}, xmax={:.1}] (px: {}x{} at {},{})",
                center_x, center_y, snapped.ymin, snapped.xmin, snapped.ymax, snapped.xmax,
                snapped_px_rect.width(), snapped_px_rect.height(), snapped_px_rect.left, snapped_px_rect.top
            );
            snapped
        }
        Ok(None) => {
            debug!(
                "[UIA] 📐 Using Vision Fallback: box=[ymin={:.1}, xmin={:.1}, ymax={:.1}, xmax={:.1}]",
                ymin, xmin, ymax, xmax
            );
            SnappedRect::new(ymin, xmin, ymax, xmax, false)
        }
        Err(_) => {
            warn!(
                "[UIA] ⚠️ Smart snap timed out after {:?}. Falling back to Vision box.",
                timeout
            );
            SnappedRect::new(ymin, xmin, ymax, xmax, false)
        }
    }
}

/// Smart-snap a bounding box:
/// Calculates the center of the AI bounding box, queries UIA, and if a valid control is found,
/// snaps to its exact physical sub-pixel boundary.
/// If UIA fails, times out (150ms), or finds no control, smoothly falls back to the original AI bounding box.
pub fn smart_snap_bounding_box(
    ymin: f64,
    xmin: f64,
    ymax: f64,
    xmax: f64,
) -> SnappedRect {
    smart_snap_bounding_box_timeout(ymin, xmin, ymax, xmax, std::time::Duration::from_millis(150))
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pixel_rect_dimensions_and_validity() {
        let screen_w = 1920;
        let screen_h = 1080;

        // Normal button: 120x40 at (500, 300)
        let button = PixelRect::new(500, 300, 620, 340);
        assert_eq!(button.width(), 120);
        assert_eq!(button.height(), 40);
        assert!(button.is_valid_control(screen_w, screen_h));
        assert!(button.contains_or_near(550, 320, 0));
        assert!(button.contains_or_near(495, 320, 10)); // within tolerance

        // Too small (< 4px)
        let tiny = PixelRect::new(100, 100, 102, 102);
        assert!(!tiny.is_valid_control(screen_w, screen_h));

        // Inverted or degenerate (right <= left)
        let inverted = PixelRect::new(600, 300, 500, 340);
        assert!(!inverted.is_valid_control(screen_w, screen_h));

        // Too huge (> 95% of screen, e.g. root desktop)
        let desktop = PixelRect::new(0, 0, 1920, 1080);
        assert!(!desktop.is_valid_control(screen_w, screen_h));
    }

    #[test]
    fn test_pixel_rect_to_normalized() {
        let screen_w = 1920;
        let screen_h = 1080;

        // Top-left corner
        let rect_tl = PixelRect::new(0, 0, 192, 108);
        let norm_tl = pixel_rect_to_normalized(&rect_tl, screen_w, screen_h);
        assert_eq!(norm_tl.xmin, 0.0);
        assert_eq!(norm_tl.ymin, 0.0);
        assert!((norm_tl.xmax - 100.0).abs() < 1e-4);
        assert!((norm_tl.ymax - 100.0).abs() < 1e-4);
        assert!(norm_tl.is_snapped);

        // Center screen
        let rect_center = PixelRect::new(960, 540, 1152, 648);
        let norm_center = pixel_rect_to_normalized(&rect_center, screen_w, screen_h);
        assert!((norm_center.xmin - 500.0).abs() < 1e-4);
        assert!((norm_center.ymin - 500.0).abs() < 1e-4);
        assert!(norm_center.is_snapped);
    }

    #[test]
    fn test_snapped_rect_clamping_and_fallback() {
        // Fallback test
        let fallback = SnappedRect::new(100.0, 200.0, 300.0, 400.0, false);
        assert_eq!(fallback.ymin, 100.0);
        assert_eq!(fallback.xmin, 200.0);
        assert_eq!(fallback.ymax, 300.0);
        assert_eq!(fallback.xmax, 400.0);
        assert!(!fallback.is_snapped);

        // Clamping checks
        let clamped = SnappedRect::new(-50.0, -100.0, 1200.0, 1500.0, true);
        assert_eq!(clamped.ymin, 0.0);
        assert_eq!(clamped.xmin, 0.0);
        assert_eq!(clamped.ymax, 1000.0);
        assert_eq!(clamped.xmax, 1000.0);
    }

    #[test]
    fn test_snapped_rect_reversed_bounds_normalization() {
        // If inputs are inverted (e.g. ymin > ymax or xmin > xmax),
        // SnappedRect::new should automatically swap them into correct order
        let reversed = SnappedRect::new(600.0, 800.0, 200.0, 100.0, true);
        assert_eq!(reversed.ymin, 200.0);
        assert_eq!(reversed.ymax, 600.0);
        assert_eq!(reversed.xmin, 100.0);
        assert_eq!(reversed.xmax, 800.0);
    }

    #[test]
    fn test_smart_snap_bounding_box_execution() {
        // Runs smart_snap_bounding_box with valid bounds
        // Whether UIA succeeds or falls back, it must return a valid SnappedRect within [0..1000]
        let res = smart_snap_bounding_box(100.0, 200.0, 300.0, 400.0);
        assert!(res.ymin >= 0.0 && res.ymin <= 1000.0);
        assert!(res.xmin >= 0.0 && res.xmin <= 1000.0);
        assert!(res.ymax >= 0.0 && res.ymax <= 1000.0);
        assert!(res.xmax >= 0.0 && res.xmax <= 1000.0);
    }

    #[test]
    fn test_smart_snap_bounding_box_timeout_fallback() {
        // With an ultra-short timeout (0ms), it should immediately fall back to the vision box without hanging
        let res = smart_snap_bounding_box_timeout(
            150.0,
            250.0,
            350.0,
            450.0,
            std::time::Duration::from_millis(0),
        );
        assert_eq!(res.ymin, 150.0);
        assert_eq!(res.xmin, 250.0);
        assert_eq!(res.ymax, 350.0);
        assert_eq!(res.xmax, 450.0);
        assert!(!res.is_snapped);
    }
}
