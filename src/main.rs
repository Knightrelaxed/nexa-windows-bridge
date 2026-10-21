// ============================================================
// N.E.X.A 3.3 — WINDOWS BRIDGE DESKTOP COMPANION
// Native Slint Desktop Companion for Lenovo ThinkPad X380 Yoga
// ============================================================

mod config;
mod ui;

use config::BridgeConfig;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use tracing::{error, info};
use ui::MainWindow;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,nexa_windows_bridge=debug".into()),
        )
        .init();

    info!("Starting N.E.X.A Windows Bridge v0.1.0 (Lenovo ThinkPad X380 Yoga)...");

    // 2. Load configuration
    let config = Arc::new(Mutex::new(BridgeConfig::load()));

    // 3. Initialize Slint MainWindow
    let main_window = MainWindow::new()?;

    // Sync initial configuration to UI
    {
        let cfg = config.lock().unwrap();
        main_window.set_cfg_server_url(cfg.server_url.clone().into());
        main_window.set_cfg_auth_token(cfg.auth_token.clone().into());
        main_window.set_cfg_low_ram_mode(cfg.low_ram_mode);
        main_window.set_status_message("Loaded configuration successfully.".into());
    }

    // 4. Hook Save Settings Callback
    let config_clone = Arc::clone(&config);
    let window_weak = main_window.as_weak();
    main_window.on_save_settings(move |url, token, _fps, low_ram| {
        let mut cfg = config_clone.lock().unwrap();
        cfg.server_url = url.to_string();
        cfg.auth_token = token.to_string();
        cfg.low_ram_mode = low_ram;

        match cfg.save() {
            Ok(_) => {
                info!("Settings saved successfully.");
                if let Some(win) = window_weak.upgrade() {
                    win.set_status_message("Settings saved & applied.".into());
                }
            }
            Err(e) => {
                error!("Failed to save settings: {}", e);
                if let Some(win) = window_weak.upgrade() {
                    win.set_status_message(format!("Error saving settings: {}", e).into());
                }
            }
        }
    });

    // 5. Hook Test Connection Callback
    let window_weak = main_window.as_weak();
    main_window.on_test_connection(move |url, token| {
        let url_str = url.to_string();
        let token_str = token.to_string();
        let window_weak = window_weak.clone();

        if let Some(win) = window_weak.upgrade() {
            win.set_is_testing(true);
            win.set_test_status_text("Testing connection...".into());
        }

        tokio::spawn(async move {
            let normalized_url = BridgeConfig::normalize_ws_url(&url_str);
            info!("Testing WebSocket handshake to: {}", normalized_url);

            let res = test_ws_handshake(&normalized_url, &token_str).await;

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = window_weak.upgrade() {
                    win.set_is_testing(false);
                    match res {
                        Ok(_) => {
                            win.set_test_status_text("✅ Handshake Succeeded! Connected & Authenticated.".into());
                        }
                        Err(e) => {
                            win.set_test_status_text(format!("❌ Failed: {}", e).into());
                        }
                    }
                }
            });
        });
    });

    // 6. Hook Toggle Connect Callback
    let window_weak = main_window.as_weak();
    main_window.on_toggle_connect(move || {
        if let Some(win) = window_weak.upgrade() {
            let current = win.get_connection_status();
            if current == "connected" {
                win.set_connection_status("offline".into());
                win.set_avatar_state("offline".into());
                win.set_status_message("Disconnected.".into());
            } else {
                win.set_connection_status("connecting".into());
                win.set_avatar_state("idle".into());
                win.set_status_message("Connecting to N.E.X.A Server...".into());
            }
        }
    });

    // Run the Slint GUI Event Loop
    main_window.run()?;
    Ok(())
}

/// Helper function to perform a test WebSocket connection with Bearer auth
async fn test_ws_handshake(url: &str, token: &str) -> Result<(), String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::HeaderValue;

    let mut request = url
        .into_client_request()
        .map_err(|e| format!("Invalid URL: {}", e))?;

    if !token.is_empty() {
        let auth_val = format!("Bearer {}", token);
        if let Ok(header_val) = HeaderValue::from_str(&auth_val) {
            request.headers_mut().insert("Authorization", header_val);
        }
    }

    request
        .headers_mut()
        .insert("X-Device-Type", HeaderValue::from_static("windows"));

    let connect_future = tokio_tungstenite::connect_async(request);
    match tokio::time::timeout(std::time::Duration::from_secs(4), connect_future).await {
        Ok(Ok((_ws_stream, response))) => {
            info!("Handshake HTTP response: {:?}", response.status());
            Ok(())
        }
        Ok(Err(e)) => Err(format!("Connection error: {}", e)),
        Err(_) => Err("Connection timed out after 4s".to_string()),
    }
}
