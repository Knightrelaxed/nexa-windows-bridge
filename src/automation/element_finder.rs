//! Magnetic Smart-Snap via Windows UI Automation (Milestone 6.2)
//!
//! Queries Windows UI Automation to locate native OS controls at the target point
//! and snaps bounding boxes to pixel-perfect physical boundaries.

#![allow(dead_code)]

use crate::automation::input::{denormalize_to_pixels, get_primary_screen_dimensions};
use tracing::{debug, info};
use windows::Win32::Foundation::POINT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomationElement};

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
        (self.right - self.left).max(0)
    }

    pub fn height(&self) -> i32 {
        (self.bottom - self.top).max(0)
    }

    /// Check if dimensions are rational for an interactive UI control
    /// (e.g. at least 4x4 pixels, and not occupying >95% of the screen)
    pub fn is_valid_control(&self, screen_w: i32, screen_h: i32) -> bool {
        let w = self.width();
        let h = self.height();

        if w < 4 || h < 4 {
            return false;
        }

        let max_w = ((screen_w as f64) * 0.95).round() as i32;
        let max_h = ((screen_h as f64) * 0.95).round() as i32;

        if w > max_w && h > max_h {
            // Likely root desktop or full display window, not an individual button/element
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
    pub fn new(ymin: f64, xmin: f64, ymax: f64, xmax: f64, is_snapped: bool) -> Self {
        Self {
            ymin: ymin.clamp(0.0, 1000.0),
            xmin: xmin.clamp(0.0, 1000.0),
            ymax: ymax.clamp(0.0, 1000.0),
            xmax: xmax.clamp(0.0, 1000.0),
            is_snapped,
        }
    }
}

/// Convert physical pixel bounds back into normalized coordinates (0..=1000)
pub fn pixel_rect_to_normalized(rect: &PixelRect, screen_w: i32, screen_h: i32) -> SnappedRect {
    let max_w = (screen_w - 1).max(1) as f64;
    let max_h = (screen_h - 1).max(1) as f64;

    let xmin = ((rect.left as f64 / max_w) * 1000.0).clamp(0.0, 1000.0);
    let ymin = ((rect.top as f64 / max_h) * 1000.0).clamp(0.0, 1000.0);
    let xmax = ((rect.right as f64 / max_w) * 1000.0).clamp(0.0, 1000.0);
    let ymax = ((rect.bottom as f64 / max_h) * 1000.0).clamp(0.0, 1000.0);

    SnappedRect::new(ymin, xmin, ymax, xmax, true)
}

/// Query Windows UI Automation to locate the element at physical screen coordinates (px, py).
/// Returns Some(PixelRect) if a valid control is found, or None.
pub fn find_element_bounds_at_point(px: i32, py: i32, screen_w: i32, screen_h: i32) -> Option<PixelRect> {
    unsafe {
        // Initialize COM library for current thread if not already initialized
        let co_init_res = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let com_initialized = co_init_res.is_ok();

        let uia: Result<IUIAutomation, _> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER);
        let uia = match uia {
            Ok(u) => u,
            Err(err) => {
                debug!("[UIA] CoCreateInstance(CUIAutomation) failed: {:?}", err);
                if com_initialized {
                    CoUninitialize();
                }
                return None;
            }
        };

        let pt = POINT { x: px, y: py };
        let elem_res: Result<IUIAutomationElement, _> = uia.ElementFromPoint(pt);

        let elem = match elem_res {
            Ok(e) => e,
            Err(err) => {
                debug!("[UIA] ElementFromPoint failed at ({}, {}): {:?}", px, py, err);
                if com_initialized {
                    CoUninitialize();
                }
                return None;
            }
        };

        let rect_res = elem.CurrentBoundingRectangle();

        if com_initialized {
            CoUninitialize();
        }

        match rect_res {
            Ok(rect) => {
                let pixel_rect = PixelRect::new(rect.left, rect.top, rect.right, rect.bottom);
                if pixel_rect.is_valid_control(screen_w, screen_h) && pixel_rect.contains_or_near(px, py, 30) {
                    Some(pixel_rect)
                } else {
                    None
                }
            }
            Err(err) => {
                debug!("[UIA] CurrentBoundingRectangle failed: {:?}", err);
                None
            }
        }
    }
}

/// Smart-snap a bounding box:
/// Calculates the center of the AI bounding box, queries UIA, and if a valid control is found,
/// snaps to its exact physical sub-pixel boundary.
/// If UIA fails or finds no control, smoothly falls back to the original AI bounding box.
pub fn smart_snap_bounding_box(
    ymin: f64,
    xmin: f64,
    ymax: f64,
    xmax: f64,
) -> SnappedRect {
    let screen = get_primary_screen_dimensions();
    let center_x = (xmin + xmax) / 2.0;
    let center_y = (ymin + ymax) / 2.0;

    let (px, py) = denormalize_to_pixels(center_x, center_y, screen.width, screen.height);

    if let Some(snapped_px_rect) = find_element_bounds_at_point(px, py, screen.width, screen.height) {
        let snapped = pixel_rect_to_normalized(&snapped_px_rect, screen.width, screen.height);
        info!(
            "[UIA] 🧲 Magnetic Snap SUCCESS: target=({:.1}, {:.1}) -> snapped box=[ymin={:.1}, xmin={:.1}, ymax={:.1}, xmax={:.1}] (px: {}x{} at {},{})",
            center_x, center_y, snapped.ymin, snapped.xmin, snapped.ymax, snapped.xmax,
            snapped_px_rect.width(), snapped_px_rect.height(), snapped_px_rect.left, snapped_px_rect.top
        );
        snapped
    } else {
        debug!(
            "[UIA] 📐 Using Vision Fallback: box=[ymin={:.1}, xmin={:.1}, ymax={:.1}, xmax={:.1}]",
            ymin, xmin, ymax, xmax
        );
        SnappedRect::new(ymin, xmin, ymax, xmax, false)
    }
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
        assert!((norm_tl.xmax - 100.0).abs() < 2.0);
        assert!((norm_tl.ymax - 100.0).abs() < 2.0);
        assert!(norm_tl.is_snapped);

        // Center screen
        let rect_center = PixelRect::new(960, 540, 1152, 648);
        let norm_center = pixel_rect_to_normalized(&rect_center, screen_w, screen_h);
        assert!((norm_center.xmin - 500.0).abs() < 2.0);
        assert!((norm_center.ymin - 500.0).abs() < 2.0);
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
    fn test_smart_snap_bounding_box_execution() {
        // Runs smart_snap_bounding_box with valid bounds
        // Whether UIA succeeds or falls back, it must return a valid SnappedRect within [0..1000]
        let res = smart_snap_bounding_box(100.0, 200.0, 300.0, 400.0);
        assert!(res.ymin >= 0.0 && res.ymin <= 1000.0);
        assert!(res.xmin >= 0.0 && res.xmin <= 1000.0);
        assert!(res.ymax >= 0.0 && res.ymax <= 1000.0);
        assert!(res.xmax >= 0.0 && res.xmax <= 1000.0);
    }
}
