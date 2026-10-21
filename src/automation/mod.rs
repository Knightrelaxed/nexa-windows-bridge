//! Automation module for N.E.X.A Windows Bridge (Milestone 6.4)
//!
//! Provides desktop input automation (SendInput mouse & keyboard),
//! screen element smart-snapping (Windows UI Automation),
//! transparent visual overlays, and the central command dispatcher for EXECUTE_COMMAND.

pub mod element_finder;
pub mod highlighter;
pub mod input;

#[allow(unused_imports)]
pub use element_finder::*;
#[allow(unused_imports)]
pub use highlighter::*;
#[allow(unused_imports)]
pub use input::*;

use crate::network::ClientMessage;
use tracing::{info, warn};

/// Central command dispatcher for ServerMessage::ExecuteCommand.
/// Maps 7 standard actions to native Windows automation, UI Automation snapping,
/// and DirectComposition/Slint overlays, returning strongly-typed CommandResult messages.
pub async fn handle_execute_command(
    command_id: String,
    action: String,
    params: serde_json::Value,
    highlighter: Option<&HighlighterController>,
) -> ClientMessage {
    let action_clean = action.trim().to_uppercase();
    info!(
        "[AUTOMATION] ⚡ Dispatching command '{}' [{}] with params: {}",
        action_clean, command_id, params
    );

    match action_clean.as_str() {
        // ─────────────────────────────────────────────────────────────
        // 1. DESKTOP_CLICK
        // ─────────────────────────────────────────────────────────────
        "DESKTOP_CLICK" => {
            let x = match params.get("x").and_then(|v| v.as_f64()) {
                Some(val) if (0.0..=1000.0).contains(&val) => val,
                _ => {
                    return ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: "Koordinat x harus berupa angka dalam rentang 0-1000".to_string(),
                        data: serde_json::json!({ "error": "INVALID_COORDINATE_X" }),
                    };
                }
            };

            let y = match params.get("y").and_then(|v| v.as_f64()) {
                Some(val) if (0.0..=1000.0).contains(&val) => val,
                _ => {
                    return ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: "Koordinat y harus berupa angka dalam rentang 0-1000".to_string(),
                        data: serde_json::json!({ "error": "INVALID_COORDINATE_Y" }),
                    };
                }
            };

            let button_str = params
                .get("button")
                .and_then(|v| v.as_str())
                .unwrap_or("LEFT");
            let button = MouseButton::from_str_loose(button_str);
            let double_click = params
                .get("double_click")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // Trigger visual click ripple indicator if highlighter is active
            let screen = get_primary_screen_dimensions();
            let (px, py) = denormalize_to_pixels(x, y, screen.width, screen.height);
            if let Some(hl) = highlighter {
                hl.show_click_ripple(px, py);
            }

            match desktop_click(x, y, button, double_click) {
                Ok((res_px, res_py)) => ClientMessage::CommandResult {
                    command_id,
                    success: true,
                    message: format!(
                        "Titik ({:.1}, {:.1}) [pixel: {}, {}] berhasil diklik di layar desktop.",
                        x, y, res_px, res_py
                    ),
                    data: serde_json::json!({
                        "action": "desktopClick",
                        "x": x,
                        "y": y,
                        "pixel_x": res_px,
                        "pixel_y": res_py,
                        "button": format!("{:?}", button),
                        "double_click": double_click
                    }),
                },
                Err(err) => {
                    warn!("[AUTOMATION] desktop_click failed: {}", err);
                    ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: format!("Gagal mengeklik layar desktop: {}", err),
                        data: serde_json::json!({ "error": err }),
                    }
                }
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 2. DESKTOP_SCROLL
        // ─────────────────────────────────────────────────────────────
        "DESKTOP_SCROLL" => {
            let direction = params
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("DOWN");
            let amount = params
                .get("amount")
                .and_then(|v| v.as_i64())
                .unwrap_or(3) as i32;

            match desktop_scroll(direction, amount) {
                Ok(delta) => ClientMessage::CommandResult {
                    command_id,
                    success: true,
                    message: format!(
                        "Layar desktop berhasil digulir ke {} (delta: {}).",
                        direction.to_uppercase(),
                        delta
                    ),
                    data: serde_json::json!({
                        "action": "desktopScroll",
                        "direction": direction,
                        "amount": amount,
                        "delta": delta
                    }),
                },
                Err(err) => {
                    warn!("[AUTOMATION] desktop_scroll failed: {}", err);
                    ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: format!("Gagal menggulir layar desktop: {}", err),
                        data: serde_json::json!({ "error": err }),
                    }
                }
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 3. DESKTOP_TYPE_TEXT
        // ─────────────────────────────────────────────────────────────
        "DESKTOP_TYPE_TEXT" => {
            let text = match params.get("text").and_then(|v| v.as_str()) {
                Some(t) if !t.is_empty() => t,
                _ => {
                    return ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: "Teks yang akan diketik tidak boleh kosong.".to_string(),
                        data: serde_json::json!({ "error": "EMPTY_TEXT" }),
                    };
                }
            };

            let press_enter = params
                .get("press_enter")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            match desktop_type_text(text, press_enter) {
                Ok(chars_typed) => ClientMessage::CommandResult {
                    command_id,
                    success: true,
                    message: format!("Teks berhasil diketik di desktop ({} karakter).", chars_typed),
                    data: serde_json::json!({
                        "action": "desktopTypeText",
                        "chars_typed": chars_typed,
                        "press_enter": press_enter
                    }),
                },
                Err(err) => {
                    warn!("[AUTOMATION] desktop_type_text failed: {}", err);
                    ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: format!("Gagal mengetik teks di desktop: {}", err),
                        data: serde_json::json!({ "error": err }),
                    }
                }
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 4. DESKTOP_PRESS_KEY
        // ─────────────────────────────────────────────────────────────
        "DESKTOP_PRESS_KEY" => {
            let key = match params.get("key").and_then(|v| v.as_str()) {
                Some(k) if !k.trim().is_empty() => k.trim(),
                _ => {
                    return ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: "Nama tombol tidak boleh kosong.".to_string(),
                        data: serde_json::json!({ "error": "EMPTY_KEY" }),
                    };
                }
            };

            match desktop_press_key(key) {
                Ok(vk) => ClientMessage::CommandResult {
                    command_id,
                    success: true,
                    message: format!("Tombol '{}' (0x{:04X}) berhasil ditekan di desktop.", key, vk),
                    data: serde_json::json!({
                        "action": "desktopPressKey",
                        "key": key,
                        "virtual_key": vk
                    }),
                },
                Err(err) => {
                    warn!("[AUTOMATION] desktop_press_key failed: {}", err);
                    ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: format!("Gagal menekan tombol di desktop: {}", err),
                        data: serde_json::json!({ "error": err }),
                    }
                }
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 5. HIGHLIGHT_ELEMENT
        // ─────────────────────────────────────────────────────────────
        "HIGHLIGHT_ELEMENT" => {
            let ymin = match params.get("ymin").and_then(|v| v.as_f64()) {
                Some(v) if (0.0..=1000.0).contains(&v) => v,
                _ => 0.0,
            };
            let xmin = match params.get("xmin").and_then(|v| v.as_f64()) {
                Some(v) if (0.0..=1000.0).contains(&v) => v,
                _ => 0.0,
            };
            let ymax = match params.get("ymax").and_then(|v| v.as_f64()) {
                Some(v) if (0.0..=1000.0).contains(&v) => v,
                _ => 1000.0,
            };
            let xmax = match params.get("xmax").and_then(|v| v.as_f64()) {
                Some(v) if (0.0..=1000.0).contains(&v) => v,
                _ => 1000.0,
            };

            let style_str = params
                .get("style")
                .and_then(|v| v.as_str())
                .unwrap_or("BOX");
            let style = HighlightStyle::from_str_loose(style_str);
            let label = params
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let duration_ms = params
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(4000);

            let req = HighlightRequest::new(ymin, xmin, ymax, xmax, style, label.clone(), duration_ms);

            if let Some(hl) = highlighter {
                hl.show_highlight(req.clone());
            }

            ClientMessage::CommandResult {
                command_id,
                success: true,
                message: format!(
                    "Elemen [ymin={:.1}, xmin={:.1}, ymax={:.1}, xmax={:.1}] berhasil disorot dengan gaya {}.",
                    req.ymin, req.xmin, req.ymax, req.xmax, req.style.as_str()
                ),
                data: serde_json::json!({
                    "action": "highlightElement",
                    "ymin": req.ymin,
                    "xmin": req.xmin,
                    "ymax": req.ymax,
                    "xmax": req.xmax,
                    "style": req.style.as_str(),
                    "label": label,
                    "duration_ms": req.duration_ms
                }),
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 6. POINT_CURSOR
        // ─────────────────────────────────────────────────────────────
        "POINT_CURSOR" => {
            let x = match params.get("x").and_then(|v| v.as_f64()) {
                Some(v) if (0.0..=1000.0).contains(&v) => v,
                _ => {
                    return ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: "Koordinat x harus berupa angka dalam rentang 0-1000".to_string(),
                        data: serde_json::json!({ "error": "INVALID_X" }),
                    };
                }
            };

            let y = match params.get("y").and_then(|v| v.as_f64()) {
                Some(v) if (0.0..=1000.0).contains(&v) => v,
                _ => {
                    return ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: "Koordinat y harus berupa angka dalam rentang 0-1000".to_string(),
                        data: serde_json::json!({ "error": "INVALID_Y" }),
                    };
                }
            };

            let duration_ms = params
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(3000);

            if let Some(hl) = highlighter {
                hl.point_cursor(x, y, duration_ms);
            }

            ClientMessage::CommandResult {
                command_id,
                success: true,
                message: format!("Kursor penunjuk diarahkan ke ({:.1}, {:.1}) di layar.", x, y),
                data: serde_json::json!({
                    "action": "pointCursor",
                    "x": x,
                    "y": y,
                    "duration_ms": duration_ms
                }),
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 7. CLEAR_HIGHLIGHTS
        // ─────────────────────────────────────────────────────────────
        "CLEAR_HIGHLIGHTS" => {
            if let Some(hl) = highlighter {
                hl.clear_all();
            }

            ClientMessage::CommandResult {
                command_id,
                success: true,
                message: "Semua sorotan layar berhasil dibersihkan.".to_string(),
                data: serde_json::json!({
                    "action": "clearHighlights"
                }),
            }
        }

        // ─────────────────────────────────────────────────────────────
        // UNKNOWN ACTION
        // ─────────────────────────────────────────────────────────────
        _ => {
            warn!("[AUTOMATION] ❓ Unknown action requested: '{}'", action);
            ClientMessage::CommandResult {
                command_id,
                success: false,
                message: format!("Aksi '{}' tidak dikenal oleh Windows Bridge.", action),
                data: serde_json::json!({
                    "error": "UNKNOWN_ACTION",
                    "action": action
                }),
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dispatcher_desktop_click_invalid_coordinates() {
        let res = handle_execute_command(
            "cmd_1".to_string(),
            "DESKTOP_CLICK".to_string(),
            serde_json::json!({ "x": -5.0, "y": 500.0 }),
            None,
        )
        .await;

        match res {
            ClientMessage::CommandResult { success, command_id, .. } => {
                assert!(!success);
                assert_eq!(command_id, "cmd_1");
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_desktop_scroll_valid() {
        let res = handle_execute_command(
            "cmd_scroll".to_string(),
            "DESKTOP_SCROLL".to_string(),
            serde_json::json!({ "direction": "UP", "amount": 2 }),
            None,
        )
        .await;

        match res {
            ClientMessage::CommandResult { success, command_id, data, .. } => {
                assert!(success);
                assert_eq!(command_id, "cmd_scroll");
                assert_eq!(data["delta"], 240);
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_desktop_type_text_empty_fails() {
        let res = handle_execute_command(
            "cmd_type".to_string(),
            "DESKTOP_TYPE_TEXT".to_string(),
            serde_json::json!({ "text": "" }),
            None,
        )
        .await;

        match res {
            ClientMessage::CommandResult { success, .. } => {
                assert!(!success);
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_desktop_press_key_valid() {
        let res = handle_execute_command(
            "cmd_key".to_string(),
            "DESKTOP_PRESS_KEY".to_string(),
            serde_json::json!({ "key": "ENTER" }),
            None,
        )
        .await;

        match res {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(success);
                assert_eq!(data["key"], "ENTER");
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_highlight_element_valid() {
        let res = handle_execute_command(
            "cmd_hl".to_string(),
            "HIGHLIGHT_ELEMENT".to_string(),
            serde_json::json!({
                "ymin": 100.0,
                "xmin": 200.0,
                "ymax": 300.0,
                "xmax": 400.0,
                "style": "BOX",
                "label": "Test Button",
                "duration_ms": 3000
            }),
            None,
        )
        .await;

        match res {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(success);
                assert_eq!(data["style"], "BOX");
                assert_eq!(data["label"], "Test Button");
                assert_eq!(data["duration_ms"], 3000);
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_point_cursor_valid() {
        let res = handle_execute_command(
            "cmd_pt".to_string(),
            "POINT_CURSOR".to_string(),
            serde_json::json!({ "x": 500.0, "y": 600.0, "duration_ms": 2500 }),
            None,
        )
        .await;

        match res {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(success);
                assert_eq!(data["x"], 500.0);
                assert_eq!(data["y"], 600.0);
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_clear_highlights() {
        let res = handle_execute_command(
            "cmd_clear".to_string(),
            "CLEAR_HIGHLIGHTS".to_string(),
            serde_json::json!({}),
            None,
        )
        .await;

        match res {
            ClientMessage::CommandResult { success, .. } => {
                assert!(success);
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_unknown_action_fails() {
        let res = handle_execute_command(
            "cmd_unknown".to_string(),
            "NON_EXISTENT_ACTION_XYZ".to_string(),
            serde_json::json!({}),
            None,
        )
        .await;

        match res {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(!success);
                assert_eq!(data["error"], "UNKNOWN_ACTION");
            }
            _ => panic!("Expected CommandResult"),
        }
    }
}
