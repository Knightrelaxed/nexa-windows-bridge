//! Desktop Input Automation via Win32 SendInput (Milestone 6.1)
//!
//! Provides precision mouse clicking, scrolling, Unicode text typing,
//! and virtual key pressing without being affected by Windows display scaling.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;
use tracing::{info, warn};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
};

/// Global cancellation flag to immediately halt long-running input actions (e.g. typing)
static AUTOMATION_ABORT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Signal an abort to any running automation loops
pub fn request_automation_abort() {
    AUTOMATION_ABORT_REQUESTED.store(true, Ordering::SeqCst);
}

/// Reset the abort signal before starting new automation actions
pub fn reset_automation_abort() {
    AUTOMATION_ABORT_REQUESTED.store(false, Ordering::SeqCst);
}

/// Check if an abort has been requested
pub fn is_automation_abort_requested() -> bool {
    AUTOMATION_ABORT_REQUESTED.load(Ordering::SeqCst)
}

/// Mouse buttons supported by desktop automation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_uppercase().as_str() {
            "RIGHT" => MouseButton::Right,
            "MIDDLE" => MouseButton::Middle,
            _ => MouseButton::Left,
        }
    }
}

/// Screen dimensions for pixel calculation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenDimensions {
    pub width: i32,
    pub height: i32,
}

/// Retrieve current primary screen dimensions (in physical pixels)
pub fn get_primary_screen_dimensions() -> ScreenDimensions {
    unsafe {
        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        ScreenDimensions {
            width: if w > 0 { w } else { 1920 },
            height: if h > 0 { h } else { 1080 },
        }
    }
}

/// Retrieve virtual desktop dimensions (spanning all connected displays)
pub fn get_virtual_screen_dimensions() -> ScreenDimensions {
    unsafe {
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if w > 0 && h > 0 {
            ScreenDimensions {
                width: w,
                height: h,
            }
        } else {
            get_primary_screen_dimensions()
        }
    }
}

/// Convert normalized coordinates (0..=1000) from Gemini Live into screen physical pixels
pub fn denormalize_to_pixels(norm_x: f64, norm_y: f64, width: i32, height: i32) -> (i32, i32) {
    let clamped_x = norm_x.clamp(0.0, 1000.0);
    let clamped_y = norm_y.clamp(0.0, 1000.0);

    let max_w = (width - 1).max(0);
    let max_h = (height - 1).max(0);

    let px = ((clamped_x / 1000.0) * (width as f64)).floor() as i32;
    let py = ((clamped_y / 1000.0) * (height as f64)).floor() as i32;

    (px.clamp(0, max_w), py.clamp(0, max_h))
}

/// Convert normalized coordinates (0..=1000) directly into Windows SendInput absolute coordinates (0..=65535)
pub fn denormalize_to_mouse_absolute(norm_x: f64, norm_y: f64) -> (i32, i32) {
    let clamped_x = norm_x.clamp(0.0, 1000.0);
    let clamped_y = norm_y.clamp(0.0, 1000.0);

    let abs_x = ((clamped_x / 1000.0) * 65535.0).round() as i32;
    let abs_y = ((clamped_y / 1000.0) * 65535.0).round() as i32;

    (abs_x.clamp(0, 65535), abs_y.clamp(0, 65535))
}

/// Calculate mouse wheel delta value for scrolling
pub fn calculate_scroll_wheel_delta(direction: &str, amount: i32) -> i32 {
    let notches = if amount <= 0 { 3 } else { amount.min(50) };
    let wheel_delta = 120; // Win32 WHEEL_DELTA constant

    match direction.trim().to_uppercase().as_str() {
        "UP" => notches * wheel_delta,
        _ => -(notches * wheel_delta), // Default DOWN
    }
}

/// Map textual key name to Win32 Virtual Key code (VK_*)
pub fn map_key_name_to_vk(key_name: &str) -> Option<u16> {
    let cleaned = key_name.trim().to_uppercase();
    match cleaned.as_str() {
        "ENTER" | "RETURN" => Some(VK_RETURN),
        "SPACE" => Some(VK_SPACE),
        "ESCAPE" | "ESC" => Some(VK_ESCAPE),
        "TAB" => Some(VK_TAB),
        "BACKSPACE" | "BACK" => Some(VK_BACK),
        "DELETE" | "DEL" => Some(VK_DELETE),
        "INSERT" | "INS" => Some(VK_INSERT),
        "UP" => Some(VK_UP),
        "DOWN" => Some(VK_DOWN),
        "LEFT" => Some(VK_LEFT),
        "RIGHT" => Some(VK_RIGHT),
        "HOME" => Some(VK_HOME),
        "END" => Some(VK_END),
        "PAGEUP" | "PGUP" | "PAGE_UP" => Some(VK_PRIOR),
        "PAGEDOWN" | "PGDN" | "PAGE_DOWN" => Some(VK_NEXT),
        "WIN" | "WINDOWS" | "SUPER" => Some(VK_LWIN),
        "ALT" => Some(VK_MENU),
        "CTRL" | "CONTROL" => Some(VK_CONTROL),
        "SHIFT" => Some(VK_SHIFT),
        "CAPSLOCK" | "CAPS" => Some(VK_CAPITAL),
        "NUMLOCK" => Some(VK_NUMLOCK),
        "SCROLLLOCK" => Some(VK_SCROLL),
        "PRINTSCREEN" | "PRTSC" | "SNAPSHOT" => Some(VK_SNAPSHOT),
        "PAUSE" => Some(VK_PAUSE),
        "F1" => Some(VK_F1),
        "F2" => Some(VK_F2),
        "F3" => Some(VK_F3),
        "F4" => Some(VK_F4),
        "F5" => Some(VK_F5),
        "F6" => Some(VK_F6),
        "F7" => Some(VK_F7),
        "F8" => Some(VK_F8),
        "F9" => Some(VK_F9),
        "F10" => Some(VK_F10),
        "F11" => Some(VK_F11),
        "F12" => Some(VK_F12),
        "VOLUME_MUTE" | "MUTE" => Some(VK_VOLUME_MUTE),
        "VOLUME_DOWN" | "VOLDOWN" => Some(VK_VOLUME_DOWN),
        "VOLUME_UP" | "VOLUP" => Some(VK_VOLUME_UP),
        "MEDIA_PLAY_PAUSE" | "PLAY_PAUSE" | "PLAY" => Some(VK_MEDIA_PLAY_PAUSE),
        "MEDIA_NEXT" | "NEXT_TRACK" => Some(VK_MEDIA_NEXT_TRACK),
        "MEDIA_PREV" | "PREV_TRACK" => Some(VK_MEDIA_PREV_TRACK),
        "MEDIA_STOP" => Some(VK_MEDIA_STOP),
        _ => {
            // Support single ASCII uppercase letters and digits (e.g. 'A', '1')
            if cleaned.len() == 1 {
                let ch = cleaned.chars().next().unwrap();
                if ch.is_ascii_alphanumeric() {
                    return Some(ch as u16);
                }
            }
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Win32 SendInput Execution Layer
// ─────────────────────────────────────────────────────────────────────────────

/// Check whether a Virtual Key is an extended key requiring KEYEVENTF_EXTENDEDKEY (Audit N10)
pub fn is_extended_key(vk: u16) -> bool {
    matches!(
        vk,
        VK_UP
            | VK_DOWN
            | VK_LEFT
            | VK_RIGHT
            | VK_INSERT
            | VK_DELETE
            | VK_HOME
            | VK_END
            | VK_PRIOR
            | VK_NEXT
            | VK_NUMLOCK
            | VK_RCONTROL
            | VK_RMENU
            | VK_LWIN
            | VK_RWIN
            | VK_DIVIDE
    )
}

/// Helper to construct a Win32 KEYBDINPUT event with proper KEYEVENTF_EXTENDEDKEY handling (Audit N10)
pub fn make_keyboard_input(vk: u16, key_up: bool) -> INPUT {
    let mut flags = if key_up { KEYEVENTF_KEYUP } else { 0 };
    if is_extended_key(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Verify whether a parsed sequence of Virtual Keys violates security policy (Audit N3)
pub fn is_forbidden_key_combo(vks: &[u16]) -> bool {
    // 1. Any Windows key (LWIN / RWIN) is forbidden for autonomous execution
    let has_win = vks.iter().any(|&k| k == VK_LWIN || k == VK_RWIN);
    if has_win {
        return true;
    }

    // 2. ALT + F4 (App force close)
    let has_alt = vks.iter().any(|&k| k == VK_MENU);
    let has_f4 = vks.iter().any(|&k| k == VK_F4);
    if has_alt && has_f4 {
        return true;
    }

    // 3. CTRL + ALT + DEL / DECIMAL / ESC
    let has_ctrl = vks.iter().any(|&k| k == VK_CONTROL);
    let has_shift = vks.iter().any(|&k| k == VK_SHIFT);
    let has_esc = vks.iter().any(|&k| k == VK_ESCAPE);
    let has_del = vks.iter().any(|&k| k == VK_DELETE || k == VK_DECIMAL);

    if has_ctrl && has_alt && (has_del || has_esc) {
        return true;
    }

    // 4. CTRL + SHIFT + ESC (Task Manager)
    if has_ctrl && has_shift && has_esc {
        return true;
    }

    false
}

/// RAII guard to guarantee all pressed keys are released even if an error occurs midway (Audit N2)
pub struct HeldKeysGuard {
    held: Vec<u16>,
}

impl HeldKeysGuard {
    pub fn new() -> Self {
        Self { held: Vec::new() }
    }

    pub fn record_down(&mut self, vk: u16) {
        self.held.push(vk);
    }

    pub fn release_all(&mut self) -> Result<(), String> {
        let mut last_err = Ok(());
        while let Some(vk) = self.held.pop() {
            let up_input = make_keyboard_input(vk, true);
            unsafe {
                if let Err(e) = send_inputs(&[up_input]) {
                    last_err = Err(e);
                }
            }
        }
        last_err
    }
}

impl Drop for HeldKeysGuard {
    fn drop(&mut self) {
        // Release any remaining held keys in reverse order
        while let Some(vk) = self.held.pop() {
            let up_input = make_keyboard_input(vk, true);
            unsafe {
                let _ = send_inputs(&[up_input]);
            }
        }
    }
}

/// RAII guard to guarantee pressed mouse button is released if an error occurs midway (Audit N2)
pub struct HeldMouseGuard {
    active: Option<(i32, i32, u32)>,
}

impl HeldMouseGuard {
    pub fn new() -> Self {
        Self { active: None }
    }

    pub fn record_down(&mut self, abs_x: i32, abs_y: i32, up_flag: u32) {
        self.active = Some((abs_x, abs_y, up_flag));
    }

    pub fn defuse(&mut self) {
        self.active = None;
    }
}

impl Drop for HeldMouseGuard {
    fn drop(&mut self) {
        if let Some((abs_x, abs_y, up_flag)) = self.active.take() {
            let up_input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: abs_x,
                        dy: abs_y,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE | up_flag,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            unsafe {
                let _ = send_inputs(&[up_input]);
            }
        }
    }
}

/// Helper to send an array of Win32 INPUT events and verify full transmission.
/// Returns an error if SendInput blocked or failed (e.g. UIPI / elevated target).
pub(crate) unsafe fn send_inputs(inputs: &[INPUT]) -> Result<(), String> {
    if inputs.is_empty() {
        return Ok(());
    }
    let count = inputs.len() as u32;
    let sent = SendInput(count, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
    if sent as usize != inputs.len() {
        let err = GetLastError();
        return Err(format!(
            "SendInput failed: sent {}/{} events (error code {}). Target window may be elevated (UIPI) or secure desktop",
            sent,
            inputs.len(),
            err
        ));
    }
    Ok(())
}

/// Perform a mouse click at normalized coordinates (0..=1000).
/// Returns the physical screen pixel (x, y) clicked.
pub fn desktop_click(
    norm_x: f64,
    norm_y: f64,
    button: MouseButton,
    double_click: bool,
) -> Result<(i32, i32), String> {
    if !(0.0..=1000.0).contains(&norm_x) || !(0.0..=1000.0).contains(&norm_y) {
        return Err(format!(
            "Coordinates out of bounds: ({:.1}, {:.1}) must be between 0 and 1000",
            norm_x, norm_y
        ));
    }

    let screen = get_primary_screen_dimensions();
    let (px, py) = denormalize_to_pixels(norm_x, norm_y, screen.width, screen.height);
    let (abs_x, abs_y) = denormalize_to_mouse_absolute(norm_x, norm_y);

    let (down_flag, up_flag) = match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };

    let mut mouse_guard = HeldMouseGuard::new();
    unsafe {
        // Step 1: Move mouse to target position
        let move_input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: abs_x,
                    dy: abs_y,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        send_inputs(&[move_input])?;
        sleep(Duration::from_millis(15));

        // Helper closure to trigger a single down/up sequence with position lock & RAII guard (Audit N2)
        let trigger_click = |guard: &mut HeldMouseGuard| -> Result<(), String> {
            let down_input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: abs_x,
                        dy: abs_y,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE | down_flag,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            send_inputs(&[down_input])?;
            guard.record_down(abs_x, abs_y, up_flag);
            sleep(Duration::from_millis(20));

            let up_input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: abs_x,
                        dy: abs_y,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE | up_flag,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            send_inputs(&[up_input])?;
            guard.defuse();
            Ok(())
        };

        // First click
        trigger_click(&mut mouse_guard)?;

        // Optional double-click
        if double_click {
            sleep(Duration::from_millis(70));
            trigger_click(&mut mouse_guard)?;
        }
    }

    info!(
        "[AUTOMATION] 🖱️ desktop_click: norm=({:.1}, {:.1}) -> pixel=({}, {}) button={:?} dbl={}",
        norm_x, norm_y, px, py, button, double_click
    );

    Ok((px, py))
}

/// Perform mouse wheel scroll in the specified direction ("UP" or "DOWN").
/// Returns the scroll delta applied.
pub fn desktop_scroll(direction: &str, amount: i32) -> Result<i32, String> {
    let delta = calculate_scroll_wheel_delta(direction, amount);

    unsafe {
        let scroll_input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: delta as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        send_inputs(&[scroll_input])?;
    }

    info!(
        "[AUTOMATION] 📜 desktop_scroll: direction='{}', amount={}, delta={}",
        direction, amount, delta
    );

    Ok(delta)
}

/// Type Unicode text using KEYEVENTF_UNICODE to ensure 100% layout-independent character entry.
/// Correctly handles surrogate pairs and maps newlines ('\n') to VK_RETURN for universal compatibility.
/// Supports immediate cancellation if an abort signal is received (Audit N13).
/// Returns the number of characters typed.
pub fn desktop_type_text(text: &str, press_enter: bool) -> Result<usize, String> {
    if text.is_empty() {
        return Err("Text to type cannot be empty".to_string());
    }

    if is_automation_abort_requested() {
        return Err("Automation execution was cancelled before typing began".to_string());
    }

    let mut char_count = 0;

    unsafe {
        for ch in text.chars() {
            if is_automation_abort_requested() {
                warn!(
                    "[AUTOMATION] 🛑 desktop_type_text aborted by cancel signal (typed {} characters)",
                    char_count
                );
                return Err("Automation aborted during text typing".to_string());
            }

            if ch == '\r' {
                continue; // Skip standalone CR; LF handles Enter
            }

            if ch == '\n' {
                // Send Enter key sequence for genuine newline across text boxes
                let enter_down = make_keyboard_input(VK_RETURN, false);
                send_inputs(&[enter_down])?;
                sleep(Duration::from_millis(10));

                let enter_up = make_keyboard_input(VK_RETURN, true);
                send_inputs(&[enter_up])?;
                char_count += 1;
                sleep(Duration::from_millis(10));
                continue;
            }

            // Encode character to UTF-16 code units (1 unit for BMP, 2 for surrogate pairs)
            let mut buf = [0u16; 2];
            let encoded = ch.encode_utf16(&mut buf);

            for &code_unit in encoded.iter() {
                // Key down
                let key_down = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: 0,
                            wScan: code_unit,
                            dwFlags: KEYEVENTF_UNICODE,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                send_inputs(&[key_down])?;

                // Key up
                let key_up = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: 0,
                            wScan: code_unit,
                            dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                send_inputs(&[key_up])?;
            }

            char_count += 1;

            // Brief pause between characters to avoid buffer overrun
            sleep(Duration::from_millis(5));
        }

        // If explicitly requested, follow with a trailing Enter key
        if press_enter {
            sleep(Duration::from_millis(20));
            let enter_down = make_keyboard_input(VK_RETURN, false);
            send_inputs(&[enter_down])?;
            sleep(Duration::from_millis(15));

            let enter_up = make_keyboard_input(VK_RETURN, true);
            send_inputs(&[enter_up])?;
        }
    }

    info!(
        "[AUTOMATION] ⌨️ desktop_type_text: typed {} characters (press_enter={})",
        char_count, press_enter
    );

    Ok(char_count)
}

/// Press a specific keyboard virtual key or key combination (e.g. "ENTER", "SPACE", "CTRL+C", "ALT+TAB").
/// Uses RAII guards to guarantee keys are released even if SendInput errors midway (Audit N2).
/// Properly attaches KEYEVENTF_EXTENDEDKEY to navigation/system keys (Audit N10).
/// Verifies parsed VK sequences against security policies (Audit N3).
/// Returns the primary/last VK code pressed.
pub fn desktop_press_key(key: &str) -> Result<u16, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("Key name cannot be empty".to_string());
    }

    let vks = if trimmed.contains('+') {
        let parts: Vec<&str> = trimmed
            .split('+')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            return Err("Invalid key combination: empty parts".to_string());
        }

        let mut list = Vec::with_capacity(parts.len());
        for part in &parts {
            let vk = map_key_name_to_vk(part).ok_or_else(|| {
                format!(
                    "Unsupported or unknown key name in combo: '{}'. Supported: ENTER, SPACE, ESCAPE, TAB, CTRL, ALT, SHIFT, WIN, etc.",
                    part
                )
            })?;
            list.push(vk);
        }
        list
    } else {
        let vk = map_key_name_to_vk(trimmed).ok_or_else(|| {
            format!(
                "Unsupported or unknown key name: '{}'. Supported: ENTER, SPACE, ESCAPE, TAB, BACKSPACE, UP, DOWN, LEFT, RIGHT, etc.",
                trimmed
            )
        })?;
        vec![vk]
    };

    // Security Policy Check on actual parsed Virtual Keys (Audit N3)
    if is_forbidden_key_combo(&vks) {
        return Err(format!(
            "Penekanan kombinasi tombol '{}' diblokir oleh kebijakan keamanan sistem.",
            trimmed
        ));
    }

    let mut guard = HeldKeysGuard::new();
    unsafe {
        // Press all keys down in forward order
        for &vk in &vks {
            let key_down = make_keyboard_input(vk, false);
            send_inputs(&[key_down])?;
            guard.record_down(vk);
            sleep(Duration::from_millis(10));
        }

        sleep(Duration::from_millis(20));

        // Release all keys in reverse order safely via RAII guard
        guard.release_all()?;
    }

    let last_vk = *vks.last().unwrap();
    info!(
        "[AUTOMATION] 🔘 desktop_press_key: successfully pressed '{}' (VKs: {:?})",
        trimmed, vks
    );

    Ok(last_vk)
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_denormalize_to_pixels() {
        let width = 1920;
        let height = 1080;

        // Top-left
        let (x0, y0) = denormalize_to_pixels(0.0, 0.0, width, height);
        assert_eq!((x0, y0), (0, 0));

        // Center
        let (xc, yc) = denormalize_to_pixels(500.0, 500.0, width, height);
        assert_eq!((xc, yc), (960, 540));

        // Bottom-right
        let (x1, y1) = denormalize_to_pixels(1000.0, 1000.0, width, height);
        assert_eq!((x1, y1), (1919, 1079));

        // Out-of-bounds clamping
        let (x_neg, y_neg) = denormalize_to_pixels(-100.0, -50.0, width, height);
        assert_eq!((x_neg, y_neg), (0, 0));

        let (x_ovr, y_ovr) = denormalize_to_pixels(1500.0, 2000.0, width, height);
        assert_eq!((x_ovr, y_ovr), (1919, 1079));
    }

    #[test]
    fn test_denormalize_to_mouse_absolute() {
        // Top-left
        let (abs_x0, abs_y0) = denormalize_to_mouse_absolute(0.0, 0.0);
        assert_eq!((abs_x0, abs_y0), (0, 0));

        // Midpoint
        let (abs_xm, abs_ym) = denormalize_to_mouse_absolute(500.0, 500.0);
        assert_eq!((abs_xm, abs_ym), (32768, 32768));

        // Max corner
        let (abs_x1, abs_y1) = denormalize_to_mouse_absolute(1000.0, 1000.0);
        assert_eq!((abs_x1, abs_y1), (65535, 65535));
    }

    #[test]
    fn test_calculate_scroll_wheel_delta() {
        assert_eq!(calculate_scroll_wheel_delta("UP", 3), 360);
        assert_eq!(calculate_scroll_wheel_delta("up", 1), 120);
        assert_eq!(calculate_scroll_wheel_delta("DOWN", 5), -600);
        assert_eq!(calculate_scroll_wheel_delta("down", 0), -360); // Default 3 notches
    }

    #[test]
    fn test_map_key_name_to_vk() {
        assert_eq!(map_key_name_to_vk("ENTER"), Some(VK_RETURN));
        assert_eq!(map_key_name_to_vk("return"), Some(VK_RETURN));
        assert_eq!(map_key_name_to_vk("SPACE"), Some(VK_SPACE));
        assert_eq!(map_key_name_to_vk("ESC"), Some(VK_ESCAPE));
        assert_eq!(map_key_name_to_vk("escape"), Some(VK_ESCAPE));
        assert_eq!(map_key_name_to_vk("TAB"), Some(VK_TAB));
        assert_eq!(map_key_name_to_vk("BACKSPACE"), Some(VK_BACK));
        assert_eq!(map_key_name_to_vk("UP"), Some(VK_UP));
        assert_eq!(map_key_name_to_vk("DOWN"), Some(VK_DOWN));
        assert_eq!(map_key_name_to_vk("LEFT"), Some(VK_LEFT));
        assert_eq!(map_key_name_to_vk("RIGHT"), Some(VK_RIGHT));
        assert_eq!(map_key_name_to_vk("F5"), Some(VK_F5));
        assert_eq!(map_key_name_to_vk("A"), Some(b'A' as u16));
        assert_eq!(map_key_name_to_vk("7"), Some(b'7' as u16));
        assert_eq!(map_key_name_to_vk("PRTSC"), Some(VK_SNAPSHOT));
        assert_eq!(map_key_name_to_vk("CAPSLOCK"), Some(VK_CAPITAL));
        assert_eq!(map_key_name_to_vk("VOLUME_UP"), Some(VK_VOLUME_UP));
        assert_eq!(map_key_name_to_vk("MUTE"), Some(VK_VOLUME_MUTE));
        assert_eq!(map_key_name_to_vk("PLAY_PAUSE"), Some(VK_MEDIA_PLAY_PAUSE));

        assert_eq!(map_key_name_to_vk("NON_EXISTENT_KEY_1234"), None);
    }

    #[test]
    fn test_mouse_button_from_str() {
        assert_eq!(MouseButton::from_str_loose("LEFT"), MouseButton::Left);
        assert_eq!(MouseButton::from_str_loose("left"), MouseButton::Left);
        assert_eq!(MouseButton::from_str_loose("RIGHT"), MouseButton::Right);
        assert_eq!(MouseButton::from_str_loose("middle"), MouseButton::Middle);
        assert_eq!(MouseButton::from_str_loose("unknown"), MouseButton::Left);
    }

    #[test]
    fn test_desktop_click_out_of_bounds() {
        let res = desktop_click(-10.0, 500.0, MouseButton::Left, false);
        assert!(res.is_err());
        let res2 = desktop_click(500.0, 1001.0, MouseButton::Left, false);
        assert!(res2.is_err());
    }

    #[test]
    fn test_desktop_type_text_empty() {
        let res = desktop_type_text("", false);
        assert!(res.is_err());
    }

    #[test]
    fn test_is_extended_key() {
        assert!(is_extended_key(VK_UP));
        assert!(is_extended_key(VK_DOWN));
        assert!(is_extended_key(VK_LEFT));
        assert!(is_extended_key(VK_RIGHT));
        assert!(is_extended_key(VK_INSERT));
        assert!(is_extended_key(VK_DELETE));
        assert!(is_extended_key(VK_HOME));
        assert!(is_extended_key(VK_END));
        assert!(is_extended_key(VK_PRIOR));
        assert!(is_extended_key(VK_NEXT));
        assert!(is_extended_key(VK_NUMLOCK));
        assert!(is_extended_key(VK_LWIN));
        assert!(is_extended_key(VK_RWIN));

        // Standard non-extended keys
        assert!(!is_extended_key(VK_RETURN));
        assert!(!is_extended_key(VK_SPACE));
        assert!(!is_extended_key(b'A' as u16));
        assert!(!is_extended_key(VK_TAB));
    }

    #[test]
    fn test_is_forbidden_key_combo() {
        // Windows keys
        assert!(is_forbidden_key_combo(&[VK_LWIN]));
        assert!(is_forbidden_key_combo(&[VK_RWIN]));
        assert!(is_forbidden_key_combo(&[VK_LWIN, b'R' as u16]));

        // ALT + F4
        assert!(is_forbidden_key_combo(&[VK_MENU, VK_F4]));
        assert!(is_forbidden_key_combo(&[VK_F4, VK_MENU]));

        // CTRL + ALT + DEL
        assert!(is_forbidden_key_combo(&[VK_CONTROL, VK_MENU, VK_DELETE]));
        assert!(is_forbidden_key_combo(&[VK_CONTROL, VK_MENU, VK_DECIMAL]));

        // CTRL + SHIFT + ESC
        assert!(is_forbidden_key_combo(&[VK_CONTROL, VK_SHIFT, VK_ESCAPE]));

        // Allowed combos
        assert!(!is_forbidden_key_combo(&[VK_CONTROL, b'C' as u16]));
        assert!(!is_forbidden_key_combo(&[VK_CONTROL, b'V' as u16]));
        assert!(!is_forbidden_key_combo(&[VK_MENU, VK_TAB]));
        assert!(!is_forbidden_key_combo(&[VK_RETURN]));
    }

    #[test]
    fn test_automation_abort_flag() {
        reset_automation_abort();
        assert!(!is_automation_abort_requested());

        request_automation_abort();
        assert!(is_automation_abort_requested());

        let res = desktop_type_text("Hello", false);
        assert!(res.is_err());

        reset_automation_abort();
        assert!(!is_automation_abort_requested());
    }
}
