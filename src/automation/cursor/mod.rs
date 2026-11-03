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
use crate::automation::input::{ai_input_sequence, is_ai_input_active, set_human_interrupted};
use crate::ui::CursorMode;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

static GLOBAL_CURSOR: OnceLock<CursorController> = OnceLock::new();

/// Unified synchronous visual state to prevent lock-order inversion
#[derive(Clone)]
struct CursorVisualState {
    mode: CursorMode,
    badge: Option<String>,
}

/// RAII Scope Guard to hold virtual cursor visible in a specific state (e.g. Typing)
pub struct CursorHold {
    controller: &'static CursorController,
    epoch: u64,
}

impl Drop for CursorHold {
    fn drop(&mut self) {
        self.controller.rest_then_fade_if_current(self.epoch, 800);
    }
}

/// Controller managing the autonomous virtual cursor overlay
#[derive(Clone)]
pub struct CursorController {
    sprite: Arc<NativeCursorSprite>,
    cur_x: Arc<AtomicI32>,
    cur_y: Arc<AtomicI32>,
    target_x: Arc<AtomicI32>,
    target_y: Arc<AtomicI32>,
    visual_state: Arc<StdMutex<CursorVisualState>>,
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
            target_x: Arc::new(AtomicI32::new(960)),
            target_y: Arc::new(AtomicI32::new(540)),
            visual_state: Arc::new(StdMutex::new(CursorVisualState {
                mode: CursorMode::Idle,
                badge: None,
            })),
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

    /// Dynamically retarget an ongoing glide to new coordinates (e.g. snapped UIA control center)
    pub fn retarget(&self, target_x: i32, target_y: i32) {
        self.target_x.store(target_x, Ordering::Relaxed);
        self.target_y.store(target_y, Ordering::Relaxed);
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
        self.target_x.store(target_x, Ordering::Relaxed);
        self.target_y.store(target_y, Ordering::Relaxed);

        let controller = self.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                controller
                    .run_glide_loop(duration_ms, mode, rest_ms, epoch)
                    .await;
            });
        }
    }

    /// Hold the cursor visible in a specific mode (e.g. Typing) for the duration of an operation
    pub fn hold(&'static self, mode: CursorMode) -> CursorHold {
        let epoch = self.active_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.is_visible.store(true, Ordering::Relaxed);
        let (x, y) = self.current_position();
        let badge = {
            let mut st = self.visual_state.lock().unwrap();
            st.mode = mode;
            st.badge.clone()
        };
        self.sprite.update(x, y, 255, mode, None, badge);
        CursorHold {
            controller: self,
            epoch,
        }
    }

    /// Trigger a double-concentric click ripple at the current position
    pub fn trigger_click(&self, x: i32, y: i32) {
        let epoch = self.active_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let controller = self.clone();

        self.cur_x.store(x, Ordering::Relaxed);
        self.cur_y.store(y, Ordering::Relaxed);
        self.target_x.store(x, Ordering::Relaxed);
        self.target_y.store(y, Ordering::Relaxed);

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                controller.run_ripple_loop(x, y, epoch).await;
            });
        }
    }

    /// Set cursor mode without moving
    pub fn set_mode(&self, mode: CursorMode) {
        let (badge, x, y, alpha) = {
            let mut st = self.visual_state.lock().unwrap();
            st.mode = mode;
            let badge = st.badge.clone();
            let x = self.cur_x.load(Ordering::Relaxed);
            let y = self.cur_y.load(Ordering::Relaxed);
            let alpha = if self.is_visible.load(Ordering::Relaxed) { 255 } else { 0 };
            (badge, x, y, alpha)
        };
        self.sprite.update(x, y, alpha, mode, None, badge);
    }

    /// Override badge label text (e.g. "NEXA // ACT", or None for auto)
    pub fn set_badge_text(&self, text: Option<String>) {
        let (mode, x, y, alpha) = {
            let mut st = self.visual_state.lock().unwrap();
            st.badge = text.clone();
            let mode = st.mode;
            let x = self.cur_x.load(Ordering::Relaxed);
            let y = self.cur_y.load(Ordering::Relaxed);
            let alpha = if self.is_visible.load(Ordering::Relaxed) { 255 } else { 0 };
            (mode, x, y, alpha)
        };
        self.sprite.update(x, y, alpha, mode, None, text);
    }

    /// Hide cursor immediately
    pub fn hide(&self) {
        self.active_epoch.fetch_add(1, Ordering::SeqCst);
        self.is_visible.store(false, Ordering::Relaxed);
        self.sprite.hide();
    }

    /// Hide cursor if the current epoch is still matching (atomic compare-exchange to prevent TOCTOU)
    pub fn hide_if_current(&self, epoch: u64) {
        if self.active_epoch.compare_exchange(epoch, epoch + 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            self.is_visible.store(false, Ordering::Relaxed);
            self.sprite.hide();
        }
    }

    /// Schedule rest and fade-out if epoch is current
    pub fn rest_then_fade_if_current(&self, epoch: u64, rest_ms: u64) {
        let controller = self.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                controller.run_rest_and_fade(epoch, rest_ms).await;
            });
        }
    }

    // ── Internal Animation Loops ─────────────────────────────────────────

    async fn run_glide_loop(
        &self,
        duration_ms: u64,
        mode: CursorMode,
        rest_ms: u64,
        epoch: u64,
    ) {
        let badge = {
            let mut st = self.visual_state.lock().unwrap();
            st.mode = mode;
            st.badge.clone()
        };

        let start_x = self.cur_x.load(Ordering::Relaxed);
        let start_y = self.cur_y.load(Ordering::Relaxed);

        // 1. Initial appearance & redraw
        self.sprite.update(start_x, start_y, 255, mode, None, badge);
        self.is_visible.store(true, Ordering::Relaxed);

        let start_time = Instant::now();
        let total_dur = Duration::from_millis(duration_ms.max(50));
        let mut tick = tokio::time::interval(Duration::from_micros(16_667)); // 60 Hz interval
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut yield_watch = YieldWatcher::new();

        // 2. Smooth Time-Based Ease-Out Glide Interpolation (Cubic: 1 - (1 - t)^3)
        loop {
            tick.tick().await;
            if self.active_epoch.load(Ordering::SeqCst) != epoch {
                return; // Superseded by newer action
            }

            // Yield to Human with AI input immunity and histeresis
            if yield_watch.check_tripped(12) {
                set_human_interrupted(true);
                self.hide();
                return;
            }

            let elapsed = start_time.elapsed();
            let t = (elapsed.as_secs_f32() / total_dur.as_secs_f32()).clamp(0.0, 1.0);
            let ease_t = 1.0 - (1.0 - t).powi(3);

            let cur_target_x = self.target_x.load(Ordering::Relaxed);
            let cur_target_y = self.target_y.load(Ordering::Relaxed);

            let cx = (start_x as f32 + (cur_target_x - start_x) as f32 * ease_t).round() as i32;
            let cy = (start_y as f32 + (cur_target_y - start_y) as f32 * ease_t).round() as i32;

            self.cur_x.store(cx, Ordering::Relaxed);
            self.cur_y.store(cy, Ordering::Relaxed);

            self.sprite.move_to(cx, cy, 255);

            if t >= 1.0 {
                break;
            }
        }

        // Ensure exact target lock
        let final_tx = self.target_x.load(Ordering::Relaxed);
        let final_ty = self.target_y.load(Ordering::Relaxed);
        self.cur_x.store(final_tx, Ordering::Relaxed);
        self.cur_y.store(final_ty, Ordering::Relaxed);
        self.sprite.move_to(final_tx, final_ty, 255);

        // 3. Rest Phase -> Auto Fade-Out
        self.run_rest_and_fade(epoch, rest_ms).await;
    }

    async fn run_ripple_loop(&self, x: i32, y: i32, epoch: u64) {
        let badge = self.visual_state.lock().unwrap().badge.clone();
        let mode = CursorMode::Clicking;

        let start_time = Instant::now();
        let total_dur = Duration::from_millis(450);
        let mut tick = tokio::time::interval(Duration::from_micros(16_667)); // 60 Hz
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tick.tick().await;
            if self.active_epoch.load(Ordering::SeqCst) != epoch {
                return;
            }

            let elapsed = start_time.elapsed();
            let progress = (elapsed.as_secs_f32() / total_dur.as_secs_f32()).clamp(0.0, 1.0);
            self.sprite.update(x, y, 255, mode, Some(progress), badge.clone());

            if progress >= 1.0 {
                break;
            }
        }

        // Revert mode back to Idle after ripple completes
        {
            let mut st = self.visual_state.lock().unwrap();
            st.mode = CursorMode::Idle;
        }
        self.sprite.update(x, y, 255, CursorMode::Idle, None, badge);

        // Rest Phase (2.0s) -> Auto Fade-Out
        self.run_rest_and_fade(epoch, 2000).await;
    }

    async fn run_rest_and_fade(&self, epoch: u64, rest_ms: u64) {
        let mut yield_watch = YieldWatcher::new();
        let intervals = (rest_ms / 100).max(1);
        for _ in 0..intervals {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if self.active_epoch.load(Ordering::SeqCst) != epoch {
                return;
            }

            if yield_watch.check_tripped(12) {
                set_human_interrupted(true);
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

        self.hide_if_current(epoch);
    }
}

/// Helper to detect physical human mouse interference with AI input immunity and histeresis
struct YieldWatcher {
    base_pos: (i32, i32),
    last_seq: u64,
    strikes: u8,
}

impl YieldWatcher {
    fn new() -> Self {
        let mut pt = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut pt);
        }
        Self {
            base_pos: (pt.x, pt.y),
            last_seq: ai_input_sequence(),
            strikes: 0,
        }
    }

    fn check_tripped(&mut self, threshold_px: i32) -> bool {
        if is_ai_input_active() {
            let mut pt = POINT { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut pt);
            }
            self.base_pos = (pt.x, pt.y);
            self.last_seq = ai_input_sequence();
            self.strikes = 0;
            return false;
        }

        let cur_seq = ai_input_sequence();
        let mut cur_pt = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut cur_pt);
        }

        if cur_seq != self.last_seq {
            self.last_seq = cur_seq;
            self.base_pos = (cur_pt.x, cur_pt.y);
            self.strikes = 0;
            return false;
        }

        let dist_sq = (cur_pt.x - self.base_pos.0).pow(2) + (cur_pt.y - self.base_pos.1).pow(2);
        if dist_sq > threshold_px * threshold_px {
            self.strikes += 1;
        } else {
            self.strikes = 0;
        }

        self.strikes >= 2
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

    #[tokio::test]
    async fn test_cursor_controller_retarget() {
        let controller = CursorController::new();
        controller.target_x.store(100, Ordering::Relaxed);
        controller.target_y.store(100, Ordering::Relaxed);
        controller.retarget(450, 650);
        assert_eq!(controller.target_x.load(Ordering::Relaxed), 450);
        assert_eq!(controller.target_y.load(Ordering::Relaxed), 650);
    }

    #[tokio::test]
    async fn test_cursor_controller_hide_if_current_compare_exchange() {
        let controller = CursorController::new();
        controller.is_visible.store(true, Ordering::Relaxed);
        let epoch = controller.active_epoch.load(Ordering::SeqCst);

        // Hide when epoch matches succeeds
        controller.hide_if_current(epoch);
        assert!(!controller.is_visible());

        // Hide when epoch is stale fails to mutate newer state
        controller.is_visible.store(true, Ordering::Relaxed);
        let stale_epoch = epoch; // old epoch
        let _new_epoch = controller.active_epoch.load(Ordering::SeqCst);
        controller.hide_if_current(stale_epoch);
        assert!(controller.is_visible(), "Stale hide must not kill active cursor");
    }
}
