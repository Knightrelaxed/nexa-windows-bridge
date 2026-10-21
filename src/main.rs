// ============================================================
// N.E.X.A 3.3 — WINDOWS BRIDGE DESKTOP COMPANION
// Native Slint Desktop Companion for Lenovo ThinkPad X380 Yoga
// ============================================================

mod config;
mod network;
mod platform;
mod ui;
mod audio;

use config::BridgeConfig;
use network::{ClientMessage, ConnectionState, NetworkClient, ServerMessage};
use platform::{PowerListener, PowerStateEvent};
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
        main_window.set_status_message("Ready to connect.".into());
    }

    // 4. Initialize Network Client
    let network_client = NetworkClient::new(Arc::clone(&config));

    // 5. Initialize WASAPI Duplex Audio Engine
    let audio_engine = Arc::new(tokio::sync::Mutex::new(
        audio::AudioEngine::new().map_err(|e| format!("Audio engine error: {}", e))?,
    ));

    // 6. Hook Connection State Watcher & Duplex Audio Engine Lifecycle
    let window_weak = main_window.as_weak();
    let mut state_rx = network_client.subscribe_state();
    let audio_engine_conn = Arc::clone(&audio_engine);
    let net_client_audio = network_client.clone();

    tokio::spawn(async move {
        let (mic_pcm_tx, mut mic_pcm_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(128);

        // Forward captured 16kHz PCM chunks from ThinkPad microphone to WebSocket
        let net_client_mic = net_client_audio.clone();
        tokio::spawn(async move {
            while let Some(pcm_chunk) = mic_pcm_rx.recv().await {
                if net_client_mic.is_connected() {
                    let _ = net_client_mic.send_binary(pcm_chunk).await;
                }
            }
        });

        while state_rx.changed().await.is_ok() {
            let state = state_rx.borrow().clone();
            let window_weak = window_weak.clone();

            match &state {
                ConnectionState::Connected => {
                    info!("[MAIN] 🎙️ Connection active: Starting WASAPI Duplex Audio Engine...");
                    let mut engine = audio_engine_conn.lock().await;
                    if let Err(e) = engine.start_duplex(mic_pcm_tx.clone()) {
                        tracing::warn!("[MAIN] Audio Duplex start warning: {}", e);
                    }
                }
                ConnectionState::Disconnected | ConnectionState::Suspended => {
                    info!("[MAIN] Connection inactive: Stopping WASAPI Duplex Audio Engine...");
                    let mut engine = audio_engine_conn.lock().await;
                    engine.stop_duplex();
                }
                _ => {}
            }

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = window_weak.upgrade() {
                    match state {
                        ConnectionState::Connected => {
                            win.set_connection_status("connected".into());
                            win.set_avatar_state("idle".into());
                            win.set_status_message("Online & Connected to N.E.X.A Server".into());
                        }
                        ConnectionState::Connecting => {
                            win.set_connection_status("connecting".into());
                            win.set_avatar_state("idle".into());
                            win.set_status_message("Connecting to Server...".into());
                        }
                        ConnectionState::Reconnecting {
                            attempt,
                            next_retry_ms,
                        } => {
                            win.set_connection_status("connecting".into());
                            win.set_avatar_state("idle".into());
                            win.set_status_message(
                                format!(
                                    "Reconnecting in {}ms (Attempt #{})",
                                    next_retry_ms, attempt
                                )
                                .into(),
                            );
                        }
                        ConnectionState::Disconnected => {
                            win.set_connection_status("offline".into());
                            win.set_avatar_state("offline".into());
                            win.set_status_message("Disconnected.".into());
                        }
                        ConnectionState::Suspended => {
                            win.set_connection_status("offline".into());
                            win.set_avatar_state("offline".into());
                            win.set_status_message("Laptop Sleeping (Suspended).".into());
                        }
                    }
                }
            });
        }
    });

    // 7. Hook Inbound Server Command & Audio Dispatcher
    let mut command_rx = network_client.subscribe_commands();
    let net_client_cmd = network_client.clone();
    let audio_engine_cmd = Arc::clone(&audio_engine);
    let window_weak_cmd = main_window.as_weak();

    tokio::spawn(async move {
        while let Ok(msg) = command_rx.recv().await {
            match msg {
                ServerMessage::AudioPlay { pcm_chunk } => {
                    use base64::prelude::*;
                    if let Ok(bytes) = BASE64_STANDARD.decode(&pcm_chunk) {
                        let mut engine = audio_engine_cmd.lock().await;
                        engine.play_response_pcm(&bytes);
                    }
                }
                ServerMessage::AudioInterrupted => {
                    info!("[MAIN] Server signaled audio interrupted (barge-in).");
                    let mut engine = audio_engine_cmd.lock().await;
                    engine.stop_speaking();
                }
                ServerMessage::StatusUpdate { status, text, .. } => {
                    let status_lower = status.to_lowercase();
                    let window_weak = window_weak_cmd.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = window_weak.upgrade() {
                            if status_lower == "speaking" {
                                win.set_avatar_state("speaking".into());
                            } else if status_lower == "listening" {
                                win.set_avatar_state("listening".into());
                            } else if status_lower == "processing" {
                                win.set_avatar_state("thinking".into());
                            }
                            if let Some(t) = text {
                                win.set_status_message(t.into());
                            }
                        }
                    });
                }
                ServerMessage::ReplyComplete => {
                    let window_weak = window_weak_cmd.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = window_weak.upgrade() {
                            win.set_avatar_state("idle".into());
                        }
                    });
                }
                ServerMessage::ExecuteCommand {
                    command_id,
                    action,
                    params,
                    ..
                } => {
                    info!("[MAIN] 🛠️ Executing command '{}' [{}]", action, command_id);

                    // Acknowledge receipt to server
                    let ack = ClientMessage::CommandResult {
                        command_id: command_id.clone(),
                        success: true,
                        message: format!("Command '{}' received by Windows Bridge", action),
                        data: serde_json::json!({
                            "action": action,
                            "params": params,
                            "status": "ACKNOWLEDGED"
                        }),
                    };
                    let _ = net_client_cmd.send_message(ack).await;
                }
                _ => {}
            }
        }
    });

    // 8. Hook User Barge-In Notification (Mic energy threshold exceeded during AI speech)
    let barge_in_notify = {
        let engine = audio_engine.lock().await;
        engine.barge_in_notify()
    };
    let net_client_barge = network_client.clone();
    let audio_engine_barge = Arc::clone(&audio_engine);
    let window_weak_barge = main_window.as_weak();

    tokio::spawn(async move {
        loop {
            barge_in_notify.notified().await;
            info!("[MAIN] 🗣️ User Barge-In detected! Preempting AI speech output.");
            {
                let mut engine = audio_engine_barge.lock().await;
                engine.stop_speaking();
            }
            let _ = net_client_barge
                .send_message(ClientMessage::AudioInterrupted)
                .await;

            let window_weak = window_weak_barge.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = window_weak.upgrade() {
                    win.set_avatar_state("listening".into());
                }
            });
        }
    });

    // 9. Register Windows OS Power Listener (Sleep & Wake)
    let net_client_power = network_client.clone();
    let _power_listener = PowerListener::spawn(move |event| match event {
        PowerStateEvent::Suspend => {
            net_client_power.notify_suspend();
        }
        PowerStateEvent::Resume => {
            net_client_power.notify_resume();
        }
    });

    // 8. Hook Slint UI Callbacks
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
                    win.set_cfg_server_url(url.clone());
                    win.set_cfg_auth_token(token.clone());
                    win.set_cfg_low_ram_mode(low_ram);
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
                            win.set_test_status_text(
                                "✅ Handshake Succeeded! Connected & Authenticated.".into(),
                            );
                        }
                        Err(e) => {
                            win.set_test_status_text(format!("❌ Failed: {}", e).into());
                        }
                    }
                }
            });
        });
    });

    let net_client_toggle = network_client.clone();
    main_window.on_toggle_connect(move || {
        if net_client_toggle.is_enabled() {
            net_client_toggle.disconnect();
        } else {
            net_client_toggle.connect();
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
