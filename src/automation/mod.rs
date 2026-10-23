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

/// Helper to sanitize sensitive parameters (e.g. passwords/text in DESKTOP_TYPE_TEXT) from disk logs (Audit K5)
pub fn redact_params(action: &str, params: &serde_json::Value) -> String {
    let a = action.trim().to_uppercase();
    if a.contains("TYPE_TEXT") || a.contains("TYPETEXT") {
        let text_len = params
            .get("text")
            .or_else(|| params.get("content"))
            .and_then(|v| v.as_str())
            .map(|s| s.chars().count())
            .unwrap_or(0);
        let press_enter = params
            .get("press_enter")
            .or_else(|| params.get("pressEnter"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        format!(
            r#"{{"text":"<redacted len={}>","press_enter":{}}}"#,
            text_len, press_enter
        )
    } else {
        params.to_string()
    }
}

/// Helper to launch desktop applications, URLs, or system targets safely.
pub fn launch_desktop_app(
    app_name: &str,
    path: Option<&str>,
    url: Option<&str>,
) -> Result<String, String> {
    let target = if let Some(u) = url {
        if !u.trim().is_empty() {
            u.trim()
        } else {
            app_name.trim()
        }
    } else if let Some(p) = path {
        if !p.trim().is_empty() {
            p.trim()
        } else {
            app_name.trim()
        }
    } else {
        app_name.trim()
    };

    if target.is_empty() {
        return Err("Nama aplikasi atau target tidak boleh kosong.".to_string());
    }

    let lower = target.to_lowercase();

    // Security guard: disallow arbitrary dangerous script files
    if lower.ends_with(".bat")
        || lower.ends_with(".cmd")
        || lower.ends_with(".vbs")
        || lower.ends_with(".ps1")
        || lower.ends_with(".reg")
    {
        return Err("Eksekusi file skrip ditolak oleh kebijakan keamanan sistem.".to_string());
    }

    // URL handling
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let spawn_res = std::process::Command::new("cmd")
            .args(["/c", "start", "", target])
            .spawn();
        return match spawn_res {
            Ok(_) => Ok(format!("URL '{}' berhasil dibuka di browser.", target)),
            Err(e) => Err(format!("Gagal membuka URL di browser: {}", e)),
        };
    }

    // Known Desktop Applications Mapping
    let (prog, args): (&str, Vec<&str>) = match lower.as_str() {
        "chrome" | "google chrome" | "google-chrome" | "browser" => {
            ("cmd", vec!["/c", "start", "chrome"])
        }
        "notepad" | "catatan" | "editor" => ("notepad.exe", vec![]),
        "calc" | "calculator" | "kalkulator" => ("calc.exe", vec![]),
        "spotify" | "musik" => ("cmd", vec!["/c", "start", "spotify:"]),
        "vscode" | "vs code" | "visual studio code" | "code" => ("cmd", vec!["/c", "code"]),
        "explorer" | "file explorer" | "file_explorer" | "berkas" => ("explorer.exe", vec![]),
        "cmd" | "command prompt" => ("cmd.exe", vec![]),
        "powershell" | "terminal" => ("powershell.exe", vec![]),
        "settings" | "pengaturan" => ("cmd", vec!["/c", "start", "ms-settings:"]),
        "taskmgr" | "task manager" | "task_manager" => ("taskmgr.exe", vec![]),
        _ => ("cmd", vec!["/c", "start", "", target]),
    };

    let spawn_res = std::process::Command::new(prog).args(args).spawn();

    match spawn_res {
        Ok(_) => Ok(format!("Aplikasi '{}' berhasil dibuka di desktop.", target)),
        Err(e) => Err(format!("Gagal membuka aplikasi '{}': {}", target, e)),
    }
}

/// Central command dispatcher for ServerMessage::ExecuteCommand.
/// Maps standard actions to native Windows automation, UI Automation snapping,
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
        action_clean,
        command_id,
        redact_params(&action_clean, &params)
    );

    match action_clean.as_str() {
        // ─────────────────────────────────────────────────────────────
        // 1. DESKTOP_CLICK
        // ─────────────────────────────────────────────────────────────
        "DESKTOP_CLICK" | "DESKTOPCLICK" | "DESKTOP-CLICK" => {
            let x = match params
                .get("x")
                .or_else(|| params.get("norm_x"))
                .or_else(|| params.get("pixel_x"))
                .and_then(|v| v.as_f64())
            {
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

            let y = match params
                .get("y")
                .or_else(|| params.get("norm_y"))
                .or_else(|| params.get("pixel_y"))
                .and_then(|v| v.as_f64())
            {
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
                .or_else(|| params.get("doubleClick"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // Execute physical click via spawn_blocking FIRST so overlay window never intercepts it (Audit C2)
            let click_res = tokio::task::spawn_blocking(move || {
                desktop_click(x, y, button, double_click)
            })
            .await
            .map_err(|e| format!("Join error: {}", e))
            .and_then(|r| r);

            match click_res {
                Ok((res_px, res_py)) => {
                    // Trigger visual click ripple indicator and auto-dissolve active highlight ONLY after click succeeds
                    if let Some(hl) = highlighter {
                        hl.dissolve_active_highlight();
                        hl.show_click_ripple(res_px, res_py);
                    }

                    ClientMessage::CommandResult {
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
                    }
                }
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
        "DESKTOP_SCROLL" | "DESKTOPSCROLL" | "DESKTOP-SCROLL" => {
            let direction = params
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("DOWN")
                .to_string();
            let amount = params
                .get("amount")
                .or_else(|| params.get("lines"))
                .or_else(|| params.get("notches"))
                .and_then(|v| v.as_i64())
                .unwrap_or(3) as i32;

            let dir_clone = direction.clone();
            let scroll_res = tokio::task::spawn_blocking(move || {
                desktop_scroll(&dir_clone, amount)
            })
            .await
            .map_err(|e| format!("Join error: {}", e))
            .and_then(|r| r);

            match scroll_res {
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
        "DESKTOP_TYPE_TEXT" | "DESKTOPTYPETEXT" | "DESKTOP_TYPETEXT" | "DESKTOP-TYPE-TEXT" => {
            let text = match params
                .get("text")
                .or_else(|| params.get("content"))
                .and_then(|v| v.as_str())
            {
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

            // Security Policy Guard: Limit maximum text length per command (Claude 2 & 3 Audit)
            if text.chars().count() > 1000 {
                warn!("[AUTOMATION] ⛔ desktopTypeText blocked: text too long ({} chars)", text.chars().count());
                return ClientMessage::CommandResult {
                    command_id,
                    success: false,
                    message: "Teks yang akan diketik melebihi batas keamanan (maksimal 1000 karakter).".to_string(),
                    data: serde_json::json!({ "error": "TEXT_TOO_LONG" }),
                };
            }

            let press_enter = params
                .get("press_enter")
                .or_else(|| params.get("pressEnter"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let text_owned = text.to_string();
            reset_automation_abort();
            let type_res = tokio::task::spawn_blocking(move || {
                desktop_type_text(&text_owned, press_enter)
            })
            .await
            .map_err(|e| format!("Join error: {}", e))
            .and_then(|r| r);

            match type_res {
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
        "DESKTOP_PRESS_KEY" | "DESKTOPPRESSKEY" | "DESKTOP_PRESSKEY" | "DESKTOP-PRESS-KEY" => {
            let key = match params
                .get("key")
                .or_else(|| params.get("key_name"))
                .or_else(|| params.get("keyName"))
                .and_then(|v| v.as_str())
            {
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

            // Security Policy Guard: Disallow dangerous system keys/combinations (Claude 2, 3 & 4 Audit)
            // Initial fast-path check on normalized string + strict VK-level check inside desktop_press_key
            let key_upper = key.to_uppercase();
            let is_blocked_str = key_upper.split('+').any(|p| {
                let p = p.trim();
                p == "WIN" || p == "WINDOWS" || p == "SUPER" || p == "LWIN" || p == "RWIN"
            }) || key_upper.replace(' ', "").contains("ALT+F4")
               || (key_upper.contains("CTRL") && key_upper.contains("ALT") && (key_upper.contains("DEL") || key_upper.contains("DELETE")))
               || (key_upper.contains("CTRL") && key_upper.contains("SHIFT") && (key_upper.contains("ESC") || key_upper.contains("ESCAPE")));

            if is_blocked_str {
                warn!("[AUTOMATION] ⛔ Key press '{}' BLOCKED by security policy", key);
                return ClientMessage::CommandResult {
                    command_id,
                    success: false,
                    message: format!("Penekanan tombol '{}' diblokir oleh kebijakan keamanan sistem.", key),
                    data: serde_json::json!({ "error": "SECURITY_POLICY_VIOLATION", "key": key }),
                };
            }

            let key_owned = key.to_string();
            let press_res = tokio::task::spawn_blocking(move || {
                desktop_press_key(&key_owned)
            })
            .await
            .map_err(|e| format!("Join error: {}", e))
            .and_then(|r| r);

            match press_res {
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
                    let err_code = if err.contains("kebijakan keamanan") || err.contains("diblokir") {
                        "SECURITY_POLICY_VIOLATION"
                    } else {
                        "PRESS_KEY_FAILED"
                    };
                    ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: format!("Gagal menekan tombol di desktop: {}", err),
                        data: serde_json::json!({ "error": err_code, "key": key }),
                    }
                }
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 5. HIGHLIGHT_ELEMENT
        // ─────────────────────────────────────────────────────────────
        "HIGHLIGHT_ELEMENT" | "HIGHLIGHTELEMENT" | "HIGHLIGHT-ELEMENT" => {
            // Support both flat bounds (ymin, xmin, ymax, xmax) and box_2d array: [ymin, xmin, ymax, xmax]
            // Strictly validate coordinates to prevent silent 0..1000 whole-screen false-positives (Audit N11)
            let parsed_coords = if let Some(arr) = params.get("box_2d").and_then(|v| v.as_array()) {
                if arr.len() == 4 {
                    let ymin = arr[0].as_f64();
                    let xmin = arr[1].as_f64();
                    let ymax = arr[2].as_f64();
                    let xmax = arr[3].as_f64();
                    match (ymin, xmin, ymax, xmax) {
                        (Some(y0), Some(x0), Some(y1), Some(x1))
                            if (0.0..=1000.0).contains(&y0)
                                && (0.0..=1000.0).contains(&x0)
                                && (0.0..=1000.0).contains(&y1)
                                && (0.0..=1000.0).contains(&x1) =>
                        {
                            Some((y0.min(y1), x0.min(x1), y0.max(y1), x0.max(x1)))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            } else {
                let ymin = params.get("ymin").and_then(|v| v.as_f64());
                let xmin = params.get("xmin").and_then(|v| v.as_f64());
                let ymax = params.get("ymax").and_then(|v| v.as_f64());
                let xmax = params.get("xmax").and_then(|v| v.as_f64());
                match (ymin, xmin, ymax, xmax) {
                    (Some(y0), Some(x0), Some(y1), Some(x1))
                        if (0.0..=1000.0).contains(&y0)
                            && (0.0..=1000.0).contains(&x0)
                            && (0.0..=1000.0).contains(&y1)
                            && (0.0..=1000.0).contains(&x1) =>
                    {
                        Some((y0.min(y1), x0.min(x1), y0.max(y1), x0.max(x1)))
                    }
                    _ => None,
                }
            };

            let (ymin, xmin, ymax, xmax) = match parsed_coords {
                Some(coords) => coords,
                None => {
                    return ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: "Parameter koordinat sorotan (ymin, xmin, ymax, xmax atau box_2d) harus berupa angka valid dalam rentang 0-1000.".to_string(),
                        data: serde_json::json!({ "error": "INVALID_COORDINATES" }),
                    };
                }
            };

            let style_str = params
                .get("style")
                .and_then(|v| v.as_str())
                .unwrap_or("BOX");
            let style = HighlightStyle::from_str_loose(style_str);
            let label = params
                .get("label")
                .or_else(|| params.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let duration_ms = params
                .get("duration_ms")
                .or_else(|| params.get("durationMs"))
                .or_else(|| params.get("duration"))
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
        "POINT_CURSOR" | "POINTCURSOR" | "POINT-CURSOR" => {
            let x = match params
                .get("x")
                .or_else(|| params.get("norm_x"))
                .or_else(|| params.get("pixel_x"))
                .and_then(|v| v.as_f64())
            {
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

            let y = match params
                .get("y")
                .or_else(|| params.get("norm_y"))
                .or_else(|| params.get("pixel_y"))
                .and_then(|v| v.as_f64())
            {
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
                .or_else(|| params.get("durationMs"))
                .or_else(|| params.get("duration"))
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
        "CLEAR_HIGHLIGHTS" | "CLEARHIGHLIGHTS" | "CLEAR_HIGHLIGHT" | "CLEARHIGHLIGHT" | "CLEAR-HIGHLIGHTS" => {
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
        // 8. ABORT_AUTOMATION (Audit N1, N13)
        // ─────────────────────────────────────────────────────────────
        "ABORT_AUTOMATION" | "ABORTAUTOMATION" | "ABORT-AUTOMATION" | "ABORT" | "CANCEL_AUTOMATION" | "CANCELAUTOMATION" | "CANCEL-AUTOMATION" => {
            request_automation_abort();
            info!("[AUTOMATION] 🛑 Abort signal triggered via command {}", command_id);
            ClientMessage::CommandResult {
                command_id,
                success: true,
                message: "Sinyal pembatalan otomatisasi berhasil dikirim.".to_string(),
                data: serde_json::json!({
                    "action": "abortAutomation"
                }),
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 9. LAUNCH_APP / OPEN_APP (Milestone 6.5 Voice Intent Parity)
        // ─────────────────────────────────────────────────────────────
        "LAUNCH_APP" | "LAUNCHAPP" | "OPEN_APP" | "OPENAPP" | "OPEN_APPLICATION" => {
            let app_name = params
                .get("app_name")
                .or_else(|| params.get("appName"))
                .or_else(|| params.get("target"))
                .or_else(|| params.get("package_name"))
                .or_else(|| params.get("packageName"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let path_opt = params.get("path").and_then(|v| v.as_str());
            let url_opt = params.get("url").and_then(|v| v.as_str());

            let app_name_owned = app_name.to_string();
            let path_owned = path_opt.map(|s| s.to_string());
            let url_owned = url_opt.map(|s| s.to_string());

            let launch_res = tokio::task::spawn_blocking(move || {
                launch_desktop_app(
                    &app_name_owned,
                    path_owned.as_deref(),
                    url_owned.as_deref(),
                )
            })
            .await
            .map_err(|e| format!("Join error: {}", e))
            .and_then(|r| r);

            match launch_res {
                Ok(msg) => ClientMessage::CommandResult {
                    command_id,
                    success: true,
                    message: msg,
                    data: serde_json::json!({
                        "action": "launchApp",
                        "app_name": app_name
                    }),
                },
                Err(err) => {
                    warn!("[AUTOMATION] launch_app failed: {}", err);
                    ClientMessage::CommandResult {
                        command_id,
                        success: false,
                        message: err.clone(),
                        data: serde_json::json!({ "error": err }),
                    }
                }
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 10. SET_VOLUME
        // ─────────────────────────────────────────────────────────────
        "SET_VOLUME" | "SETVOLUME" | "SET-VOLUME" => {
            let volume_level = params
                .get("volume_level")
                .or_else(|| params.get("volumeLevel"))
                .or_else(|| params.get("level"))
                .and_then(|v| v.as_f64());

            let res = tokio::task::spawn_blocking(move || {
                if let Some(lvl) = volume_level {
                    if lvl <= 0.0 {
                        desktop_press_key("VOLUME_MUTE")
                    } else {
                        desktop_press_key("VOLUME_UP")
                    }
                } else {
                    desktop_press_key("VOLUME_UP")
                }
            })
            .await
            .map_err(|e| format!("Join error: {}", e))
            .and_then(|r| r);

            match res {
                Ok(vk) => ClientMessage::CommandResult {
                    command_id,
                    success: true,
                    message: "Volume laptop berhasil disesuaikan.".to_string(),
                    data: serde_json::json!({
                        "action": "setVolume",
                        "vk": vk,
                        "volume_level": volume_level
                    }),
                },
                Err(err) => ClientMessage::CommandResult {
                    command_id,
                    success: false,
                    message: format!("Gagal mengatur volume laptop: {}", err),
                    data: serde_json::json!({ "error": err }),
                },
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 11. MUTE / UNMUTE / TOGGLE_MUTE
        // ─────────────────────────────────────────────────────────────
        "MUTE" | "UNMUTE" | "TOGGLE_MUTE" | "TOGGLEMUTE" => {
            let res = tokio::task::spawn_blocking(|| {
                desktop_press_key("VOLUME_MUTE")
            })
            .await
            .map_err(|e| format!("Join error: {}", e))
            .and_then(|r| r);

            match res {
                Ok(vk) => ClientMessage::CommandResult {
                    command_id,
                    success: true,
                    message: "Status audio/mute laptop berhasil diubah.".to_string(),
                    data: serde_json::json!({ "action": "mute", "vk": vk }),
                },
                Err(err) => ClientMessage::CommandResult {
                    command_id,
                    success: false,
                    message: format!("Gagal mengubah status audio laptop: {}", err),
                    data: serde_json::json!({ "error": err }),
                },
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 12. LOCK_SCREEN
        // ─────────────────────────────────────────────────────────────
        "LOCK_SCREEN" | "LOCKSCREEN" | "LOCK-SCREEN" => {
            let res = tokio::task::spawn_blocking(|| {
                std::process::Command::new("rundll32.exe")
                    .args(["user32.dll,LockWorkStation"])
                    .spawn()
                    .map_err(|e| format!("Gagal mengunci laptop: {}", e))
            })
            .await
            .map_err(|e| format!("Join error: {}", e))
            .and_then(|r| r);

            match res {
                Ok(_) => ClientMessage::CommandResult {
                    command_id,
                    success: true,
                    message: "Layar laptop Windows berhasil dikunci.".to_string(),
                    data: serde_json::json!({ "action": "lockScreen" }),
                },
                Err(err) => ClientMessage::CommandResult {
                    command_id,
                    success: false,
                    message: format!("Gagal mengunci layar laptop: {}", err),
                    data: serde_json::json!({ "error": err }),
                },
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 13. GET_BATTERY_STATUS
        // ─────────────────────────────────────────────────────────────
        "GET_BATTERY_STATUS" | "BATTERY_STATUS" | "GETBATTERYSTATUS" => {
            let battery = crate::platform::power::get_battery_status();
            match battery {
                Some(b) => ClientMessage::CommandResult {
                    command_id,
                    success: true,
                    message: format!(
                        "Baterai laptop saat ini {}%{}.",
                        b.level_percent,
                        if b.is_charging { " (sedang diisi daya)" } else { "" }
                    ),
                    data: serde_json::json!({
                        "action": "getBatteryStatus",
                        "level_percent": b.level_percent,
                        "is_charging": b.is_charging,
                        "ac_connected": b.ac_connected
                    }),
                },
                None => ClientMessage::CommandResult {
                    command_id,
                    success: false,
                    message: "Gagal membaca status baterai laptop.".to_string(),
                    data: serde_json::json!({ "error": "BATTERY_STATUS_UNAVAILABLE" }),
                },
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 14. SYSTEM_METRICS / GET_SYSTEM_METRICS
        // ─────────────────────────────────────────────────────────────
        "SYSTEM_METRICS" | "GET_SYSTEM_METRICS" | "GETSYSTEMMETRICS" => {
            let mut telemetry = crate::platform::telemetry::SystemTelemetry::default();
            std::thread::sleep(std::time::Duration::from_millis(15));
            let cpu = telemetry.sample_cpu_percent();
            let ram = crate::platform::telemetry::SystemTelemetry::sample_ram_percent();

            ClientMessage::CommandResult {
                command_id,
                success: true,
                message: format!("Status sistem laptop: CPU {}%, RAM {}% terpakai.", cpu, ram),
                data: serde_json::json!({
                    "action": "systemMetrics",
                    "cpu_percent": cpu,
                    "ram_percent": ram
                }),
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 15. SHOW_DESKTOP / MINIMIZE_ALL / GO_HOME_SCREEN
        // ─────────────────────────────────────────────────────────────
        "SHOW_DESKTOP" | "SHOWDESKTOP" | "MINIMIZE_ALL" | "MINIMIZEALL" | "GO_HOME_SCREEN" => {
            let res = tokio::task::spawn_blocking(|| {
                std::process::Command::new("powershell")
                    .args(["-NoProfile", "-Command", "(New-Object -ComObject Shell.Application).MinimizeAll()"])
                    .spawn()
                    .map_err(|e| format!("Gagal meminimalkan jendela: {}", e))
            })
            .await
            .map_err(|e| format!("Join error: {}", e))
            .and_then(|r| r);

            match res {
                Ok(_) => ClientMessage::CommandResult {
                    command_id,
                    success: true,
                    message: "Tampilan desktop berhasil dibuka (semua jendela diminimalkan).".to_string(),
                    data: serde_json::json!({ "action": "showDesktop" }),
                },
                Err(err) => ClientMessage::CommandResult {
                    command_id,
                    success: false,
                    message: format!("Gagal membuka tampilan desktop: {}", err),
                    data: serde_json::json!({ "error": err }),
                },
            }
        }

        // ─────────────────────────────────────────────────────────────
        // 16. TAKE_SCREENSHOT
        // ─────────────────────────────────────────────────────────────
        "TAKE_SCREENSHOT" | "TAKESCREENSHOT" | "CAPTURE_SCREEN" | "SCREENSHOT" => {
            let res = tokio::task::spawn_blocking(|| {
                desktop_press_key("PRINTSCREEN")
            })
            .await
            .map_err(|e| format!("Join error: {}", e))
            .and_then(|r| r);

            match res {
                Ok(_) => ClientMessage::CommandResult {
                    command_id,
                    success: true,
                    message: "Tangkapan layar desktop berhasil diambil (disimpan ke clipboard).".to_string(),
                    data: serde_json::json!({ "action": "takeScreenshot" }),
                },
                Err(err) => ClientMessage::CommandResult {
                    command_id,
                    success: false,
                    message: format!("Gagal mengambil tangkapan layar: {}", err),
                    data: serde_json::json!({ "error": err }),
                },
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
            ClientMessage::CommandResult { success, command_id, data, message } => {
                assert_eq!(command_id, "cmd_scroll");
                if success {
                    assert_eq!(data["delta"], 240);
                } else {
                    // Under UIPI/headless background runner: SendInput is properly verified and rejected (error code 5)
                    assert!(message.contains("SendInput failed") || data["error"].as_str().unwrap().contains("SendInput"));
                }
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
            ClientMessage::CommandResult { success, command_id, data, message } => {
                assert_eq!(command_id, "cmd_key");
                if success {
                    assert_eq!(data["key"], "ENTER");
                } else {
                    assert!(message.contains("SendInput failed") || data["error"].as_str().unwrap().contains("SendInput"));
                }
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

    #[tokio::test]
    async fn test_dispatcher_camel_case_action_names() {
        // desktopScroll (camelCase)
        let res1 = handle_execute_command(
            "cmd_c1".to_string(),
            "desktopScroll".to_string(),
            serde_json::json!({ "direction": "UP", "amount": 1 }),
            None,
        )
        .await;
        match res1 {
            ClientMessage::CommandResult { command_id, success, data, message } => {
                assert_eq!(command_id, "cmd_c1");
                if success {
                    assert_eq!(data["delta"], 120);
                } else {
                    assert!(message.contains("SendInput failed") || data["error"].as_str().unwrap().contains("SendInput"));
                }
            }
            _ => panic!("Expected CommandResult"),
        }

        // clearHighlights (camelCase)
        let res2 = handle_execute_command(
            "cmd_c2".to_string(),
            "clearHighlights".to_string(),
            serde_json::json!({}),
            None,
        )
        .await;
        match res2 {
            ClientMessage::CommandResult { success, .. } => assert!(success),
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_key_combo_and_box_2d() {
        // Key combo: CTRL+C
        let res_combo = handle_execute_command(
            "cmd_combo".to_string(),
            "desktopPressKey".to_string(),
            serde_json::json!({ "key": "CTRL+C" }),
            None,
        )
        .await;
        match res_combo {
            ClientMessage::CommandResult { command_id, success, data, message } => {
                assert_eq!(command_id, "cmd_combo");
                if success {
                    assert_eq!(data["key"], "CTRL+C");
                } else {
                    assert!(message.contains("SendInput failed") || data["error"].as_str().unwrap().contains("SendInput"));
                }
            }
            _ => panic!("Expected CommandResult"),
        }

        // Box 2D array parameter fallback
        let res_box2d = handle_execute_command(
            "cmd_b2d".to_string(),
            "highlightElement".to_string(),
            serde_json::json!({
                "box_2d": [150.0, 250.0, 350.0, 450.0],
                "style": "RING",
                "label": "Array Target"
            }),
            None,
        )
        .await;
        match res_box2d {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(success);
                assert_eq!(data["ymin"], 150.0);
                assert_eq!(data["xmin"], 250.0);
                assert_eq!(data["ymax"], 350.0);
                assert_eq!(data["xmax"], 450.0);
                assert_eq!(data["style"], "RING");
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_security_policy_blocks_dangerous_keys() {
        // Block WIN+R
        let res_win_r = handle_execute_command(
            "cmd_sec1".to_string(),
            "desktopPressKey".to_string(),
            serde_json::json!({ "key": "WIN+R" }),
            None,
        )
        .await;
        match res_win_r {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(!success);
                assert_eq!(data["error"], "SECURITY_POLICY_VIOLATION");
            }
            _ => panic!("Expected CommandResult"),
        }

        // Block ALT+F4
        let res_alt_f4 = handle_execute_command(
            "cmd_sec2".to_string(),
            "desktopPressKey".to_string(),
            serde_json::json!({ "key": "ALT+F4" }),
            None,
        )
        .await;
        match res_alt_f4 {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(!success);
                assert_eq!(data["error"], "SECURITY_POLICY_VIOLATION");
            }
            _ => panic!("Expected CommandResult"),
        }

        // Block "ALT + F4" with spaces (Audit N3)
        let res_alt_space_f4 = handle_execute_command(
            "cmd_sec3".to_string(),
            "desktopPressKey".to_string(),
            serde_json::json!({ "key": "ALT + F4" }),
            None,
        )
        .await;
        match res_alt_space_f4 {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(!success);
                assert_eq!(data["error"], "SECURITY_POLICY_VIOLATION");
            }
            _ => panic!("Expected CommandResult"),
        }

        // Block "CONTROL + SHIFT + ESC" (Task Manager) (Audit N3)
        let res_taskmgr = handle_execute_command(
            "cmd_sec4".to_string(),
            "desktopPressKey".to_string(),
            serde_json::json!({ "key": "CONTROL + SHIFT + ESC" }),
            None,
        )
        .await;
        match res_taskmgr {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(!success);
                assert_eq!(data["error"], "SECURITY_POLICY_VIOLATION");
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_highlight_element_invalid_coordinates_rejected() {
        // Missing ymax, xmax
        let res_invalid = handle_execute_command(
            "cmd_hl_bad".to_string(),
            "highlightElement".to_string(),
            serde_json::json!({ "ymin": 100.0, "xmin": 200.0 }),
            None,
        )
        .await;
        match res_invalid {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(!success);
                assert_eq!(data["error"], "INVALID_COORDINATES");
            }
            _ => panic!("Expected CommandResult"),
        }

        // Malformed box_2d with only 3 elements
        let res_box_short = handle_execute_command(
            "cmd_box_short".to_string(),
            "highlightElement".to_string(),
            serde_json::json!({ "box_2d": [100.0, 200.0, 300.0] }),
            None,
        )
        .await;
        match res_box_short {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(!success);
                assert_eq!(data["error"], "INVALID_COORDINATES");
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_abort_automation() {
        let res_abort = handle_execute_command(
            "cmd_abort".to_string(),
            "abortAutomation".to_string(),
            serde_json::json!({}),
            None,
        )
        .await;
        match res_abort {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(success);
                assert_eq!(data["action"], "abortAutomation");
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_security_policy_blocks_oversized_text() {
        let long_text = "A".repeat(1001);
        let res_long = handle_execute_command(
            "cmd_long".to_string(),
            "desktopTypeText".to_string(),
            serde_json::json!({ "text": long_text }),
            None,
        )
        .await;
        match res_long {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(!success);
                assert_eq!(data["error"], "TEXT_TOO_LONG");
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[test]
    fn test_redact_params() {
        let sensitive = serde_json::json!({
            "text": "MySecretPassword123!",
            "press_enter": true
        });
        let redacted = redact_params("DESKTOP_TYPE_TEXT", &sensitive);
        assert!(!redacted.contains("MySecretPassword123!"));
        assert!(redacted.contains("<redacted len=20>"));
        assert!(redacted.contains("press_enter"));

        let click_params = serde_json::json!({ "x": 500.0, "y": 600.0 });
        let non_sensitive = redact_params("DESKTOP_CLICK", &click_params);
        assert!(non_sensitive.contains("500"));
    }

    #[tokio::test]
    async fn test_dispatcher_launch_app_security_and_validation() {
        // Empty app name fails
        let res_empty = handle_execute_command(
            "cmd_empty_app".to_string(),
            "LAUNCH_APP".to_string(),
            serde_json::json!({ "app_name": "" }),
            None,
        )
        .await;
        match res_empty {
            ClientMessage::CommandResult { success, .. } => assert!(!success),
            _ => panic!("Expected CommandResult"),
        }

        // Dangerous script extension is blocked by security guard
        let res_bat = handle_execute_command(
            "cmd_bat".to_string(),
            "LAUNCH_APP".to_string(),
            serde_json::json!({ "app_name": "malicious.bat" }),
            None,
        )
        .await;
        match res_bat {
            ClientMessage::CommandResult { success, message, .. } => {
                assert!(!success);
                assert!(message.contains("kebijakan keamanan"));
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_system_metrics_execution() {
        let res = handle_execute_command(
            "cmd_metrics".to_string(),
            "SYSTEM_METRICS".to_string(),
            serde_json::json!({}),
            None,
        )
        .await;
        match res {
            ClientMessage::CommandResult { success, data, .. } => {
                assert!(success);
                assert!(data["cpu_percent"].as_u64().is_some());
                assert!(data["ram_percent"].as_u64().is_some());
            }
            _ => panic!("Expected CommandResult"),
        }
    }

    #[tokio::test]
    async fn test_dispatcher_get_battery_status_execution() {
        let res = handle_execute_command(
            "cmd_bat_stat".to_string(),
            "GET_BATTERY_STATUS".to_string(),
            serde_json::json!({}),
            None,
        )
        .await;
        match res {
            ClientMessage::CommandResult { command_id, .. } => {
                assert_eq!(command_id, "cmd_bat_stat");
            }
            _ => panic!("Expected CommandResult"),
        }
    }
}
