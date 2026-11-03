//! NEXA Virtual Independent Cursor Controller (Dual-Cursor Co-Working Engine)
//!
//! Provides the public API for the AI cursor:
//! - Smooth ease-out gliding (180ms - 320ms travel)
//! - Double concentric click ripple actuation
//! - 9 cognitive & operational modes with dynamic telemetry badges
//! - 2.0s resting state with automatic smooth fade-out
//! - Yield-to-human detection: instant fade-out if hardware mouse moves

#![allow(dead_code)]

pub mod raster;
pub mod sprite;

use self::sprite::NativeCursorSprite;
use crate::ui::CursorMode;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Mutex as TokioMutex;
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

static GLOBAL_CURSOR: OnceLock<CursorController> = OnceLock::new();

/// Controller managing the autonomous virtual cursor overlay
#[derive(Clone)]
pub struct CursorController {
    sprite: Arc<NativeCursorSprite>,
    cur_x: Arc<AtomicI32>,
    cur_y: Arc<AtomicI32>,
    cur_mode: Arc<TokioMutex<CursorMode>>,
    cur_badge: Arc<TokioMutex<Option<String>>>,
    active_epoch: Arc<AtomicU64>,
    is_visible: Arc<AtomicBool>,
}

impl CursorController {
    /// Retrieve the global virtual cursor controller instance (lazily initialized)
    pub fn global() -> &'static CursorController {
        GLOBAL_CURSOR.get_or_init(Self::new)
    }

    /// Create and spawn a new virtual cursor controller
    pub fn new() -> Self {
        Self {
            sprite: Arc::new(NativeCursorSprite::spawn()),
            cur_x: Arc::new(AtomicI32::new(960)),
            cur_y: Arc::new(AtomicI32::new(540)),
            cur_mode: Arc::new(TokioMutex::new(CursorMode::Idle)),
            cur_badge: Arc::new(TokioMutex::new(None)),
            active_epoch: Arc::new(AtomicU64::new(0)),
            is_visible: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Retrieve the current cursor hotspot coordinates
    pub fn current_position(&self) -> (i32, i32) {
        (
            self.cur_x.load(Ordering::Relaxed),
            self.cur_y.load(Ordering::Relaxed),
        )
    }

    /// Check if cursor is currently visible on screen
    pub fn is_visible(&self) -> bool {
        self.is_visible.load(Ordering::Relaxed)
    }

    /// Retrieve the current active epoch counter
    pub fn active_epoch(&self) -> u64 {
        self.active_epoch.load(Ordering::SeqCst)
    }

    /// Set visibility flag directly (primarily for testing)
    pub fn set_visible_for_test(&self, visible: bool) {
        self.is_visible.store(visible, Ordering::Relaxed);
    }

    /// Glide smoothly to a target pixel coordinate with ease-out interpolation (default 2.0s rest)
    pub fn glide_to(&self, target_x: i32, target_y: i32, duration_ms: u64, mode: CursorMode) {
        self.glide_to_with_rest(target_x, target_y, duration_ms, mode, 2000);
    }

    /// Glide smoothly to a target pixel coordinate with custom rest duration before fade-out
    pub fn glide_to_with_rest(
        &self,
        target_x: i32,
        target_y: i32,
        duration_ms: u64,
        mode: CursorMode,
        rest_ms: u64,
    ) {
        let epoch = self.active_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let controller = self.clone();

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                controller
                    .run_glide_loop(target_x, target_y, duration_ms, mode, rest_ms, epoch)
                    .await;
            });
        }
    }

    /// Trigger a double-concentric click ripple at the current position
    pub fn trigger_click(&self, x: i32, y: i32) {
        let epoch = self.active_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let controller = self.clone();

        self.cur_x.store(x, Ordering::Relaxed);
        self.cur_y.store(y, Ordering::Relaxed);

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                controller.run_ripple_loop(x, y, epoch).await;
            });
        }
    }

    /// Set cursor mode without moving
    pub fn set_mode(&self, mode: CursorMode) {
        let controller = self.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let mut m = controller.cur_mode.lock().await;
                *m = mode;
                let badge = controller.cur_badge.lock().await.clone();
                let x = controller.cur_x.load(Ordering::Relaxed);
                let y = controller.cur_y.load(Ordering::Relaxed);
                let alpha = if controller.is_visible.load(Ordering::Relaxed) { 255 } else { 0 };
                controller.sprite.update(x, y, alpha, mode, None, badge);
            });
        }
    }

    /// Override badge label text (e.g. "NEXA // ACT", or None for auto)
    pub fn set_badge_text(&self, text: Option<String>) {
        let controller = self.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let mut b = controller.cur_badge.lock().await;
                *b = text.clone();
                let mode = *controller.cur_mode.lock().await;
                let x = controller.cur_x.load(Ordering::Relaxed);
                let y = controller.cur_y.load(Ordering::Relaxed);
                let alpha = if controller.is_visible.load(Ordering::Relaxed) { 255 } else { 0 };
                controller.sprite.update(x, y, alpha, mode, None, text);
            });
        }
    }

    /// Hide cursor immediately
    pub fn hide(&self) {
        self.active_epoch.fetch_add(1, Ordering::SeqCst);
        self.is_visible.store(false, Ordering::Relaxed);
        self.sprite.hide();
    }

    // ── Internal Animation Loops ─────────────────────────────────────────

    async fn run_glide_loop(
        &self,
        target_x: i32,
        target_y: i32,
        duration_ms: u64,
        mode: CursorMode,
        rest_ms: u64,
        epoch: u64,
    ) {
        {
            let mut m = self.cur_mode.lock().await;
            *m = mode;
        }

        let start_x = self.cur_x.load(Ordering::Relaxed);
        let start_y = self.cur_y.load(Ordering::Relaxed);
        let badge = self.cur_badge.lock().await.clone();

        // 1. Initial appearance & redraw
        self.sprite.update(start_x, start_y, 255, mode, None, badge);
        self.is_visible.store(true, Ordering::Relaxed);

        let dur = duration_ms.max(50) as f32;
        let step_ms = 8;
        let steps = (dur / step_ms as f32).ceil() as u32;

        let mut initial_mouse = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut initial_mouse);
        }

        // 2. Smooth Ease-Out Glide Interpolation (Cubic: 1 - (1 - t)^3)
        for i in 1..=steps {
            if self.active_epoch.load(Ordering::SeqCst) != epoch {
                return; // Superseded by newer action
            }

            // Yield to Human: check physical mouse
            let mut cur_mouse = POINT { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut cur_mouse);
            }
            let mouse_dist_sq = (cur_mouse.x - initial_mouse.x).pow(2) + (cur_mouse.y - initial_mouse.y).pow(2);
            if mouse_dist_sq > 16 {
                // Human moved hardware mouse (>4px): yield instantly
                self.hide();
                return;
            }

            let t = (i as f32 / steps as f32).clamp(0.0, 1.0);
            let ease_t = 1.0 - (1.0 - t).powi(3);

            let cx = (start_x as f32 + (target_x - start_x) as f32 * ease_t).round() as i32;
            let cy = (start_y as f32 + (target_y - start_y) as f32 * ease_t).round() as i32;

            self.cur_x.store(cx, Ordering::Relaxed);
            self.cur_y.store(cy, Ordering::Relaxed);

            self.sprite.move_to(cx, cy, 255);
            tokio::time::sleep(Duration::from_millis(step_ms)).await;
        }

        // Ensure exact target lock
        self.cur_x.store(target_x, Ordering::Relaxed);
        self.cur_y.store(target_y, Ordering::Relaxed);
        self.sprite.move_to(target_x, target_y, 255);

        // 3. Rest Phase -> Auto Fade-Out
        self.run_rest_and_fade(epoch, rest_ms).await;
    }

    async fn run_ripple_loop(&self, x: i32, y: i32, epoch: u64) {
        let badge = self.cur_badge.lock().await.clone();
        let mode = CursorMode::Clicking;

        let total_ms = 450;
        let step_ms = 16;
        let steps = total_ms / step_ms;

        for i in 0..=steps {
            if self.active_epoch.load(Ordering::SeqCst) != epoch {
                return;
            }

            let progress = i as f32 / steps as f32;
            self.sprite.update(x, y, 255, mode, Some(progress), badge.clone());
            tokio::time::sleep(Duration::from_millis(step_ms)).await;
        }

        // Revert mode back to Idle after ripple completes
        {
            let mut m = self.cur_mode.lock().await;
            *m = CursorMode::Idle;
        }
        self.sprite.update(x, y, 255, CursorMode::Idle, None, badge);

        // Rest Phase (2.0s) -> Auto Fade-Out
        self.run_rest_and_fade(epoch, 2000).await;
    }

    async fn run_rest_and_fade(&self, epoch: u64, rest_ms: u64) {
        // Rest with human yield check
        let mut initial_mouse = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut initial_mouse);
        }

        let intervals = (rest_ms / 100).max(1);
        for _ in 0..intervals {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if self.active_epoch.load(Ordering::SeqCst) != epoch {
                return;
            }

            let mut cur_mouse = POINT { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut cur_mouse);
            }
            let mouse_dist_sq = (cur_mouse.x - initial_mouse.x).pow(2) + (cur_mouse.y - initial_mouse.y).pow(2);
            if mouse_dist_sq > 16 {
                // Yield to human
                self.hide();
                return;
            }
        }

        // Smooth Fade Out (150ms)
        let fade_steps = 10;
        let x = self.cur_x.load(Ordering::Relaxed);
        let y = self.cur_y.load(Ordering::Relaxed);

        for step in 1..=fade_steps {
            if self.active_epoch.load(Ordering::SeqCst) != epoch {
                return;
            }
            let alpha = (255 - (255 * step / fade_steps)) as u8;
            self.sprite.move_to(x, y, alpha);
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        if self.active_epoch.load(Ordering::SeqCst) == epoch {
            self.hide();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_controller_initial_state() {
        let controller = CursorController::new();
        assert!(!controller.is_visible());
        let (x, y) = controller.current_position();
        assert_eq!(x, 960);
        assert_eq!(y, 540);
    }

    #[tokio::test]
    async fn test_cursor_controller_position_mutation_and_hide() {
        let controller = CursorController::new();
        controller.cur_x.store(100, Ordering::Relaxed);
        controller.cur_y.store(200, Ordering::Relaxed);
        assert_eq!(controller.current_position(), (100, 200));

        controller.is_visible.store(true, Ordering::Relaxed);
        assert!(controller.is_visible());

        controller.hide();
        assert!(!controller.is_visible());
    }

    #[tokio::test]
    async fn test_cursor_controller_glide_epoch_bump() {
        let controller = CursorController::new();
        let e1 = controller.active_epoch.load(Ordering::SeqCst);
        controller.glide_to(500, 500, 100, CursorMode::Hover);
        let e2 = controller.active_epoch.load(Ordering::SeqCst);
        assert!(e2 > e1, "Glide must increment active epoch");
    }

    #[tokio::test]
    async fn test_cursor_controller_trigger_click_epoch_bump() {
        let controller = CursorController::new();
        let e1 = controller.active_epoch.load(Ordering::SeqCst);
        controller.trigger_click(300, 300);
        let e2 = controller.active_epoch.load(Ordering::SeqCst);
        assert!(e2 > e1, "Trigger click must increment active epoch");
        assert_eq!(controller.current_position(), (300, 300));
    }
}
