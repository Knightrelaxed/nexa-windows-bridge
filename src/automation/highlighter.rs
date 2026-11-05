//! Native Win32 Stealth Overlay Window & Highlight Controller (Milestone 6.3)
//!
//! Provides 100% transparent GDI click-through overlays for bounding boxes (BOX),
//! rings (RING), pointers (POINTER), and spotlight (SPOTLIGHT).
//!
//! ARCHITECTURE (Immunity from Black Screen):
//! Modern Windows DWM composites DirectX / OpenGL / Slint swapchain windows with an opaque
//! clear color unless DirectComposition is configured. To prevent any black screen flicker or
//! fullscreen blackout, this module implements a dedicated native Win32 GDI layered window
//! with `LWA_COLORKEY` set to black (0,0,0). Every pixel drawn as black is 100% hardware-transparent,
//! while cyan bounding boxes, target rings, and labels render with crystal clarity.

#![allow(dead_code)]

use crate::automation::element_finder::{smart_snap_bounding_box, SnappedRect};
use crate::automation::input::{denormalize_to_pixels, get_primary_screen_dimensions};
use crate::automation::uia;
use crate::ui::OverlayWindow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, info};
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
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

// ─────────────────────────────────────────────────────────────────────────────
// Native Win32 GDI Transparent Overlay Window
// ─────────────────────────────────────────────────────────────────────────────

const WM_SHOW_HIGHLIGHT: u32 = WM_USER + 201;
const WM_HIDE_HIGHLIGHT: u32 = WM_USER + 202;
const WM_STOP_OVERLAY: u32 = WM_USER + 203;

#[derive(Clone)]
pub struct ActiveHighlight {
    pub rect: RECT,
    pub style: HighlightStyle,
    pub label: String,
}

#[derive(Clone)]
pub struct ActiveRipple {
    pub cx: i32,
    pub cy: i32,
    pub radius: i32,
}

#[derive(Clone, Default)]
struct OverlayDrawState {
    highlight: Option<ActiveHighlight>,
    ripple: Option<ActiveRipple>,
}

impl OverlayDrawState {
    pub fn is_empty(&self) -> bool {
        self.highlight.is_none() && self.ripple.is_none()
    }
}

static GLOBAL_OVERLAY_STATE: Mutex<Option<Arc<Mutex<OverlayDrawState>>>> = Mutex::new(None);

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);

            let mut cr: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut cr);

            // 1. Fill entire window with pure black (0x00000000) which is colorkeyed out to 100% transparent
            FillRect(hdc, &cr, GetStockObject(BLACK_BRUSH as i32) as HBRUSH);

            // 2. Draw active highlight & ripple independently (Dual-Layer Isolation)
            if let Ok(guard) = GLOBAL_OVERLAY_STATE.lock() {
                if let Some(ref state_arc) = *guard {
                    if let Ok(state) = state_arc.lock() {
                        let cyan_color: u32 = 0x00FFE500; // BGR: RGB(0, 229, 255)
                        let white_color: u32 = 0x00FFFFFF;

                        // Layer 1: Persistent Highlight (Box, Ring, Pointer, Spotlight)
                        if let Some(ref hl) = state.highlight {
                            match hl.style {
                                HighlightStyle::Box | HighlightStyle::Spotlight => {
                                    let pen = CreatePen(PS_SOLID as i32, 3, cyan_color);
                                    let null_brush = GetStockObject(NULL_BRUSH as i32);
                                    let old_pen = SelectObject(hdc, pen as HGDIOBJ);
                                    let old_brush = SelectObject(hdc, null_brush);

                                    let l = hl.rect.left;
                                    let t = hl.rect.top;
                                    let r = hl.rect.right.max(l + 16);
                                    let b = hl.rect.bottom.max(t + 16);

                                    RoundRect(hdc, l, t, r, b, 8, 8);

                                    // Corner precision ticks
                                    let tick_pen = CreatePen(PS_SOLID as i32, 2, white_color);
                                    SelectObject(hdc, tick_pen as HGDIOBJ);
                                    RoundRect(hdc, l - 2, t - 2, l + 6, t + 6, 2, 2);
                                    RoundRect(hdc, r - 6, b - 6, r + 2, b + 2, 2, 2);

                                    SelectObject(hdc, old_pen);
                                    SelectObject(hdc, old_brush);
                                    DeleteObject(pen as HGDIOBJ);
                                    DeleteObject(tick_pen as HGDIOBJ);

                                    // Floating label badge
                                    if !hl.label.is_empty() {
                                        let label_h = 22;
                                        let label_top = (t - label_h - 4).max(2);
                                        let wide_label: Vec<u16> =
                                            hl.label.encode_utf16().collect();
                                        let label_w =
                                            (wide_label.len() as i32 * 8 + 18).clamp(44, 380);

                                        let label_rect = RECT {
                                            left: l,
                                            top: label_top,
                                            right: l + label_w,
                                            bottom: label_top + label_h,
                                        };

                                        // Dark badge background (dark navy 0x002A170F so it's not colorkeyed out)
                                        let badge_brush = CreateSolidBrush(0x002A170F);
                                        let badge_pen = CreatePen(PS_SOLID as i32, 1, cyan_color);
                                        let old_bp = SelectObject(hdc, badge_pen as HGDIOBJ);
                                        let old_bb = SelectObject(hdc, badge_brush as HGDIOBJ);

                                        RoundRect(
                                            hdc,
                                            label_rect.left,
                                            label_rect.top,
                                            label_rect.right,
                                            label_rect.bottom,
                                            4,
                                            4,
                                        );

                                        SelectObject(hdc, old_bp);
                                        SelectObject(hdc, old_bb);
                                        DeleteObject(badge_brush as HGDIOBJ);
                                        DeleteObject(badge_pen as HGDIOBJ);

                                        SetBkMode(hdc, TRANSPARENT as i32);
                                        SetTextColor(hdc, cyan_color);
                                        let mut tr = label_rect;
                                        DrawTextW(
                                            hdc,
                                            wide_label.as_ptr(),
                                            wide_label.len() as i32,
                                            &mut tr,
                                            (DT_CENTER | DT_VCENTER | DT_SINGLELINE) as u32,
                                        );
                                    }
                                }
                                HighlightStyle::Ring => {
                                    let pen = CreatePen(PS_SOLID as i32, 3, cyan_color);
                                    let null_brush = GetStockObject(NULL_BRUSH as i32);
                                    let old_pen = SelectObject(hdc, pen as HGDIOBJ);
                                    let old_brush = SelectObject(hdc, null_brush);

                                    Ellipse(
                                        hdc,
                                        hl.rect.left,
                                        hl.rect.top,
                                        hl.rect.right,
                                        hl.rect.bottom,
                                    );

                                    SelectObject(hdc, old_pen);
                                    SelectObject(hdc, old_brush);
                                    DeleteObject(pen as HGDIOBJ);
                                }
                                HighlightStyle::Pointer => {
                                    let cx = (hl.rect.left + hl.rect.right) / 2;
                                    let cy = (hl.rect.top + hl.rect.bottom) / 2;
                                    let pen = CreatePen(PS_SOLID as i32, 2, cyan_color);
                                    let null_brush = GetStockObject(NULL_BRUSH as i32);
                                    let old_pen = SelectObject(hdc, pen as HGDIOBJ);
                                    let old_brush = SelectObject(hdc, null_brush);

                                    // Concentric target rings
                                    Ellipse(hdc, cx - 18, cy - 18, cx + 18, cy + 18);
                                    Ellipse(hdc, cx - 8, cy - 8, cx + 8, cy + 8);

                                    // Center dot
                                    let dot_brush = CreateSolidBrush(cyan_color);
                                    SelectObject(hdc, dot_brush as HGDIOBJ);
                                    Ellipse(hdc, cx - 3, cy - 3, cx + 3, cy + 3);

                                    SelectObject(hdc, old_pen);
                                    SelectObject(hdc, old_brush);
                                    DeleteObject(pen as HGDIOBJ);
                                    DeleteObject(dot_brush as HGDIOBJ);
                                }
                            }
                        }

                        // Layer 2: Click Ripple Indicator (Concentric pulse rings)
                        if let Some(ref rip) = state.ripple {
                            let cx = rip.cx;
                            let cy = rip.cy;
                            let r = rip.radius;

                            let pen = CreatePen(PS_SOLID as i32, 2, cyan_color);
                            let null_brush = GetStockObject(NULL_BRUSH as i32);
                            let old_pen = SelectObject(hdc, pen as HGDIOBJ);
                            let old_brush = SelectObject(hdc, null_brush);

                            Ellipse(hdc, cx - r, cy - r, cx + r, cy + r);
                            let inner_r = (r / 2).max(4);
                            Ellipse(hdc, cx - inner_r, cy - inner_r, cx + inner_r, cy + inner_r);

                            SelectObject(hdc, old_pen);
                            SelectObject(hdc, old_brush);
                            DeleteObject(pen as HGDIOBJ);
                        }
                    }
                }
            }

            EndPaint(hwnd, &ps);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub struct NativeOverlay {
    hwnd: Option<usize>,
    state: Arc<Mutex<OverlayDrawState>>,
}

impl NativeOverlay {
    pub fn spawn() -> Self {
        let state = Arc::new(Mutex::new(OverlayDrawState::default()));
        let (hwnd_tx, hwnd_rx) = std::sync::mpsc::channel::<Option<usize>>();

        let state_clone = Arc::clone(&state);
        std::thread::spawn(move || {
            {
                if let Ok(mut global) = GLOBAL_OVERLAY_STATE.lock() {
                    *global = Some(Arc::clone(&state_clone));
                }
            }

            let class_name: Vec<u16> = "NexaNativeStealthOverlayClass\0".encode_utf16().collect();

            unsafe {
                let wnd_class = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(overlay_wnd_proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: GetModuleHandleW(std::ptr::null()),
                    hIcon: std::ptr::null_mut(),
                    hCursor: std::ptr::null_mut(),
                    hbrBackground: GetStockObject(BLACK_BRUSH as i32) as HBRUSH,
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class_name.as_ptr(),
                };

                RegisterClassW(&wnd_class);

                let screen = get_primary_screen_dimensions();
                let hwnd = CreateWindowExW(
                    WS_EX_LAYERED
                        | WS_EX_TRANSPARENT
                        | WS_EX_TOPMOST
                        | WS_EX_NOACTIVATE
                        | WS_EX_TOOLWINDOW,
                    class_name.as_ptr(),
                    class_name.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    screen.width,
                    screen.height,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    GetModuleHandleW(std::ptr::null()),
                    std::ptr::null(),
                );

                if hwnd.is_null() {
                    let _ = hwnd_tx.send(None);
                    return;
                }

                // 1. Colorkey: Black RGB(0,0,0) is keyed out as 100% transparent.
                SetLayeredWindowAttributes(hwnd, 0x00000000, 0, LWA_COLORKEY);

                // 2. Exclude overlay from screen capture so Gemini Vision never captures its own overlays
                const WDA_EXCLUDEFROMCAPTURE: u32 = 0x00000011;
                let wda_ok = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
                if wda_ok == 0 {
                    tracing::warn!(
                        "[HIGHLIGHTER] ⚠️ SetWindowDisplayAffinity(0x11) failed: error 0x{:X}",
                        GetLastError()
                    );
                }

                // Start invisible
                ShowWindow(hwnd, SW_HIDE);

                let hwnd_usize = hwnd as usize;
                let _ = hwnd_tx.send(Some(hwnd_usize));

                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    if msg.message == WM_SHOW_HIGHLIGHT {
                        let cur_screen = get_primary_screen_dimensions();
                        SetWindowPos(
                            hwnd,
                            HWND_TOPMOST,
                            0,
                            0,
                            cur_screen.width,
                            cur_screen.height,
                            SWP_NOACTIVATE,
                        );
                        InvalidateRect(hwnd, std::ptr::null(), 1);
                        UpdateWindow(hwnd);
                        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    } else if msg.message == WM_HIDE_HIGHLIGHT {
                        let is_empty = GLOBAL_OVERLAY_STATE
                            .lock()
                            .ok()
                            .and_then(|g| g.as_ref().map(|arc| arc.lock().map(|s| s.is_empty()).unwrap_or(true)))
                            .unwrap_or(true);
                        if is_empty {
                            ShowWindow(hwnd, SW_HIDE);
                        } else {
                            InvalidateRect(hwnd, std::ptr::null(), 1);
                            UpdateWindow(hwnd);
                        }
                    } else if msg.message == WM_STOP_OVERLAY {
                        DestroyWindow(hwnd);
                        break;
                    } else {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }
        });

        let hwnd = hwnd_rx.recv_timeout(Duration::from_millis(500)).unwrap_or(None);
        Self { hwnd, state }
    }

    pub fn show_highlight(&self, rect: RECT, style: HighlightStyle, label: String) {
        if let Ok(mut st) = self.state.lock() {
            st.highlight = Some(ActiveHighlight { rect, style, label });
        }
        if let Some(h) = self.hwnd {
            unsafe {
                PostMessageW(h as HWND, WM_SHOW_HIGHLIGHT, 0, 0);
            }
        }
    }

    pub fn hide_highlight(&self) {
        if let Ok(mut st) = self.state.lock() {
            st.highlight = None;
        }
        if let Some(h) = self.hwnd {
            unsafe {
                PostMessageW(h as HWND, WM_HIDE_HIGHLIGHT, 0, 0);
            }
        }
    }

    pub fn show_ripple(&self, cx: i32, cy: i32, radius: i32) {
        if let Ok(mut st) = self.state.lock() {
            st.ripple = Some(ActiveRipple { cx, cy, radius });
        }
        if let Some(h) = self.hwnd {
            unsafe {
                PostMessageW(h as HWND, WM_SHOW_HIGHLIGHT, 0, 0);
            }
        }
    }

    pub fn hide_ripple(&self) {
        if let Ok(mut st) = self.state.lock() {
            st.ripple = None;
        }
        if let Some(h) = self.hwnd {
            unsafe {
                PostMessageW(h as HWND, WM_HIDE_HIGHLIGHT, 0, 0);
            }
        }
    }

    pub fn clear_all(&self) {
        if let Ok(mut st) = self.state.lock() {
            st.highlight = None;
            st.ripple = None;
        }
        if let Some(h) = self.hwnd {
            unsafe {
                PostMessageW(h as HWND, WM_HIDE_HIGHLIGHT, 0, 0);
            }
        }
    }

    pub fn show(&self, rect: RECT, style: HighlightStyle, label: String) {
        self.show_highlight(rect, style, label);
    }

    pub fn hide(&self) {
        self.hide_highlight();
    }
}

/// Backwards-compatibility helper (now guaranteed by NativeOverlay)
pub fn apply_stealth_window_styles() -> bool {
    true
}

/// Expands a bounding box symmetrically to a minimum 16x16 size around its center
/// so point clicks and small controls do not shift their visual center (Audit Y4b).
pub fn rect_min16(px0: i32, py0: i32, px1: i32, py1: i32) -> RECT {
    let (mut l, mut t, mut r, mut b) = (px0, py0, px1, py1);
    if r - l < 16 {
        let c = (l + r) / 2;
        l = c - 8;
        r = c + 8;
    }
    if b - t < 16 {
        let c = (t + b) / 2;
        t = c - 8;
        b = c + 8;
    }
    RECT {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

static LAST_HIGHLIGHT_LOCK: Mutex<Option<(Instant, SnappedRect)>> = Mutex::new(None);

/// Controller managing the lifetime, geometry, animations, and auto-dismiss of overlays
#[derive(Clone)]
pub struct HighlighterController {
    overlay: Arc<NativeOverlay>,
    pub active_epoch: Arc<AtomicU64>,
    pub active_ripple_epoch: Arc<AtomicU64>,
}

impl HighlighterController {
    /// Retrieve the most recently snapped highlight box if within the 10-second lease (Audit N5)
    pub fn get_last_highlight_lock() -> Option<SnappedRect> {
        let guard = LAST_HIGHLIGHT_LOCK.lock().ok()?;
        let (ts, rect) = (*guard)?;
        if ts.elapsed() <= Duration::from_secs(10) {
            Some(rect)
        } else {
            None
        }
    }

    /// Update the target lock for synchronization with subsequent click commands (Audit N5)
    pub fn set_last_highlight_lock(rect: SnappedRect) {
        if let Ok(mut guard) = LAST_HIGHLIGHT_LOCK.lock() {
            *guard = Some((Instant::now(), rect));
        }
    }

    /// Create a controller with native Win32 GDI overlay window
    pub fn new(_overlay_window: Option<slint::Weak<OverlayWindow>>) -> Self {
        Self {
            overlay: Arc::new(NativeOverlay::spawn()),
            active_epoch: Arc::new(AtomicU64::new(0)),
            active_ripple_epoch: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Display an already-snapped element highlight, updating the target lock and auto-dissolving (Audit N5)
    pub fn show_snapped(
        &self,
        snapped: SnappedRect,
        style: HighlightStyle,
        label: String,
        duration_ms: u64,
    ) {
        Self::set_last_highlight_lock(snapped);
        let epoch = self.active_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let active_epoch = Arc::clone(&self.active_epoch);
        let overlay = Arc::clone(&self.overlay);

        let screen = get_primary_screen_dimensions();
        let (px0, py0) = denormalize_to_pixels(snapped.xmin, snapped.ymin, screen.width, screen.height);
        let (px1, py1) = denormalize_to_pixels(snapped.xmax, snapped.ymax, screen.width, screen.height);
        let target_rect = rect_min16(px0, py0, px1, py1);

        info!(
            "[HIGHLIGHTER] 🌟 show_snapped [style={}, label='{}']: rect=({}, {} - {}x{}) duration={}ms (snapped={})",
            style.as_str(),
            label,
            target_rect.left,
            target_rect.top,
            target_rect.right - target_rect.left,
            target_rect.bottom - target_rect.top,
            duration_ms,
            snapped.is_snapped
        );

        overlay.show_highlight(target_rect, style, label);

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                tokio::time::sleep(Duration::from_millis(duration_ms)).await;
                if active_epoch.load(Ordering::SeqCst) == epoch {
                    overlay.hide_highlight();
                    debug!("[HIGHLIGHTER] Auto-dissolved highlight epoch {}", epoch);
                }
            });
        }
    }

    /// Display an element highlight with magnetic snapping, optional label, and auto-dissolve
    pub fn show_highlight(&self, req: HighlightRequest) {
        let epoch = self.active_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let active_epoch = Arc::clone(&self.active_epoch);
        let overlay = Arc::clone(&self.overlay);

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                // Perform Magnetic Smart-Snap via UFO-adapted UIA module (non-blocking)
                let snapped = match uia::UiaHandle::global() {
                    Ok(uia) => uia.snap_box(req.ymin, req.xmin, req.ymax, req.xmax).await,
                    Err(_) => smart_snap_bounding_box(req.ymin, req.xmin, req.ymax, req.xmax),
                };
                Self::set_last_highlight_lock(snapped);

                // Check epoch: if superseded by a newer highlight while snapping, don't show outdated box
                if active_epoch.load(Ordering::SeqCst) != epoch {
                    return;
                }

                let screen = get_primary_screen_dimensions();
                let (px0, py0) = denormalize_to_pixels(
                    snapped.xmin,
                    snapped.ymin,
                    screen.width,
                    screen.height,
                );
                let (px1, py1) = denormalize_to_pixels(
                    snapped.xmax,
                    snapped.ymax,
                    screen.width,
                    screen.height,
                );

                let target_rect = rect_min16(px0, py0, px1, py1);

                info!(
                    "[HIGHLIGHTER] 🌟 show_highlight [style={}, label='{}']: rect=({}, {} - {}x{}) duration={}ms (snapped={})",
                    req.style.as_str(),
                    req.label,
                    target_rect.left,
                    target_rect.top,
                    target_rect.right - target_rect.left,
                    target_rect.bottom - target_rect.top,
                    req.duration_ms,
                    snapped.is_snapped
                );

                overlay.show_highlight(target_rect, req.style, req.label);

                // Auto-dissolve timer for the current epoch
                tokio::time::sleep(Duration::from_millis(req.duration_ms)).await;
                if active_epoch.load(Ordering::SeqCst) == epoch {
                    overlay.hide_highlight();
                    debug!("[HIGHLIGHTER] Auto-dissolved highlight epoch {}", epoch);
                }
            });
        } else {
            // Synchronous fallback (e.g. sync unit tests)
            let snapped = smart_snap_bounding_box(req.ymin, req.xmin, req.ymax, req.xmax);
            Self::set_last_highlight_lock(snapped);
            let screen = get_primary_screen_dimensions();
            let (px0, py0) =
                denormalize_to_pixels(snapped.xmin, snapped.ymin, screen.width, screen.height);
            let (px1, py1) =
                denormalize_to_pixels(snapped.xmax, snapped.ymax, screen.width, screen.height);

            let target_rect = rect_min16(px0, py0, px1, py1);

            overlay.show_highlight(target_rect, req.style, req.label);
        }
    }

    /// Smoothly dissolve the currently active highlight (e.g. on user click or action)
    pub fn dissolve_active_highlight(&self) {
        self.active_epoch.fetch_add(1, Ordering::SeqCst);
        self.overlay.hide_highlight();
    }

    /// Point cursor at specific normalized coordinates (0..=1000)
    pub fn point_cursor(&self, norm_x: f64, norm_y: f64, duration_ms: u64) {
        let req = HighlightRequest::new(
            norm_y,
            norm_x,
            norm_y,
            norm_x,
            HighlightStyle::Pointer,
            String::new(),
            duration_ms,
        );
        self.show_highlight(req);
    }

    /// Display expanding click ripple indicator at physical screen coordinates (px, py)
    pub fn show_click_ripple(&self, px: i32, py: i32) {
        self.overlay.show_ripple(px, py, 18);
        let overlay = Arc::clone(&self.overlay);
        let ripple_epoch = self.active_ripple_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let active_ripple_epoch = Arc::clone(&self.active_ripple_epoch);

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                if active_ripple_epoch.load(Ordering::SeqCst) == ripple_epoch {
                    overlay.hide_ripple();
                }
            });
        }
    }

    /// Clear all active highlights and ripples immediately
    pub fn clear_all(&self) {
        self.active_epoch.fetch_add(1, Ordering::SeqCst);
        self.active_ripple_epoch.fetch_add(1, Ordering::SeqCst);
        self.overlay.clear_all();
        crate::automation::CursorController::global().hide();
        info!("[HIGHLIGHTER] 🧹 Cleared all screen highlights and overlays");
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
        assert_eq!(
            HighlightStyle::from_str_loose("POINTER"),
            HighlightStyle::Pointer
        );
        assert_eq!(
            HighlightStyle::from_str_loose("pointer"),
            HighlightStyle::Pointer
        );
        assert_eq!(
            HighlightStyle::from_str_loose("SPOTLIGHT"),
            HighlightStyle::Spotlight
        );
        assert_eq!(
            HighlightStyle::from_str_loose("spotlight"),
            HighlightStyle::Spotlight
        );
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
    fn test_highlighter_controller_lifecycle() {
        let controller = HighlighterController::new(None);

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
        controller.point_cursor(500.0, 500.0, 2000);
        controller.show_click_ripple(960, 540);
        controller.dissolve_active_highlight();
        controller.clear_all();
    }

    #[test]
    fn test_dissolve_active_highlight_bumps_epoch() {
        let controller = HighlighterController::new(None);
        let ep0 = controller.active_epoch.load(Ordering::SeqCst);
        controller.dissolve_active_highlight();
        let ep1 = controller.active_epoch.load(Ordering::SeqCst);
        assert_eq!(ep1, ep0 + 1);
    }

    #[test]
    fn test_dual_layer_state_isolation() {
        let mut state = OverlayDrawState::default();
        assert!(state.is_empty());

        state.highlight = Some(ActiveHighlight {
            rect: RECT { left: 10, top: 10, right: 100, bottom: 100 },
            style: HighlightStyle::Box,
            label: "Test".to_string(),
        });
        assert!(!state.is_empty());
        assert!(state.highlight.is_some());
        assert!(state.ripple.is_none());

        state.ripple = Some(ActiveRipple {
            cx: 50,
            cy: 50,
            radius: 18,
        });
        assert!(!state.is_empty());
        assert!(state.highlight.is_some());
        assert!(state.ripple.is_some());

        // Dissolving ripple does NOT destroy the active highlight
        state.ripple = None;
        assert!(!state.is_empty());
        assert!(state.highlight.is_some());

        // Dissolving highlight now leaves state empty
        state.highlight = None;
        assert!(state.is_empty());
    }

    #[test]
    fn test_rect_min16_preserves_center() {
        // Point coordinates: px0 == px1, py0 == py1
        let r_point = rect_min16(500, 300, 500, 300);
        assert_eq!(r_point.left, 492);
        assert_eq!(r_point.right, 508);
        assert_eq!(r_point.top, 292);
        assert_eq!(r_point.bottom, 308);
        assert_eq!((r_point.left + r_point.right) / 2, 500);
        assert_eq!((r_point.top + r_point.bottom) / 2, 300);

        // Box larger than 16x16: unmodified
        let r_large = rect_min16(100, 100, 200, 200);
        assert_eq!(r_large.left, 100);
        assert_eq!(r_large.top, 100);
        assert_eq!(r_large.right, 200);
        assert_eq!(r_large.bottom, 200);
    }
}
