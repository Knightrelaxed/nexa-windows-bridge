// ============================================================
// WINDOWS BRIDGE DESKTOP COMPANION
// Native Slint Desktop Companion for Lenovo ThinkPad X380 Yoga
// ============================================================

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod network;
mod platform;
mod ui;
mod audio;
mod capture;
mod automation;

use config::BridgeConfig;
use network::{ClientMessage, ConnectionState, NetworkClient, ServerMessage};
use platform::{
    HotkeyAction, HotkeyListener, PowerListener, PowerStateEvent, SystemTelemetry, SystemTray,
    TrayEvent,
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use ui::MainWindow;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 0. Single-Instance Mutex Arbiter (Milestone 7.4 / Claude Bug #4)
    let single_instance = platform::SingleInstanceGuard::acquire();
    if !single_instance.is_unique() {
        eprintln!("N.E.X.A Bridge is already running. Signaling existing instance to focus.");
        return Ok(());
    }

    // Parse command line arguments (e.g. --help, --version, --autostart, --minimized, --console)
    let args: Vec<String> = std::env::args().collect();

    // Attach to parent console if requested or started with --console
    #[cfg(not(debug_assertions))]
    if args.iter().any(|arg| arg == "--console" || arg == "-c" || arg == "--help" || arg == "-h" || arg == "--version" || arg == "-v") {
        unsafe {
            use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("N.E.X.A Windows Bridge v0.1.0");
        println!("Native Windows Desktop Companion for Lenovo ThinkPad X380 Yoga");
        println!();
        println!("USAGE:");
        println!("    nexa-windows-bridge.exe [OPTIONS]");
        println!();
        println!("OPTIONS:");
        println!("    -h, --help        Print help information");
        println!("    -v, --version     Print version information");
        println!("    -c, --console     Attach to command line console for debugging logs");
        println!("    --autostart       Start standby quietly in the Windows system tray");
        println!("    --minimized       Start window minimized to system tray");
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--version" || arg == "-v") {
        println!("nexa-windows-bridge {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let start_minimized = args.iter().any(|arg| arg == "--autostart" || arg == "--minimized");

    // 1. Initialize persistent logging to %LOCALAPPDATA%\NexaBridge\logs + panic hook (Claude Bug #6)
    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("NexaBridge")
        .join("logs");
    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = tracing_appender::rolling::daily(&log_dir, "nexa-bridge.log");
    let (non_blocking_file, _log_guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,nexa_windows_bridge=debug".into());

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking_file)
        .with_ansi(false);

    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stdout_layer)
        .init();

    // Panic hook to capture critical crash backtraces to disk (Claude Bug #6)
    let panic_log_dir = log_dir.clone();
    std::panic::set_hook(Box::new(move |panic_info| {
        let payload = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            *s
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.as_str()
        } else {
            "Unknown panic payload"
        };
        let location = panic_info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());
        let panic_msg = format!(
            "[CRITICAL PANIC] at {}: {}\nBacktrace:\n{:?}",
            location,
            payload,
            std::backtrace::Backtrace::capture()
        );
        eprintln!("{}", panic_msg);
        let crash_file = panic_log_dir.join("crash.log");
        let _ = std::fs::write(&crash_file, &panic_msg);
    }));

    info!(
        "Starting N.E.X.A Windows Bridge v{} (Lenovo ThinkPad X380 Yoga)...",
        env!("CARGO_PKG_VERSION")
    );

    // 2. Load configuration
    let config = Arc::new(Mutex::new(BridgeConfig::load()));

    // 3. Initialize Slint MainWindow
    let main_window = MainWindow::new()?;

    // Wake listener: brings window to foreground when a second instance is launched (Claude Bug #4)
    let window_weak_wake = main_window.as_weak();
    single_instance.spawn_wake_listener(move || {
        let window_weak = window_weak_wake.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(win) = window_weak.upgrade() {
                win.window().set_minimized(false);
                let _ = win.window().show();
            }
        });
    });

    // Sync initial configuration & Auto-Start status to UI (Claude Bug #1 & #10)
    {
        let cfg = config.lock().unwrap();
        let is_autostart = platform::is_autostart_enabled();
        main_window.set_cfg_server_url(cfg.server_url.clone().into());
        main_window.set_cfg_auth_token(cfg.auth_token.clone().into());
        main_window.set_cfg_fps(cfg.capture_fps as i32);
        main_window.set_cfg_low_ram_mode(cfg.low_ram_mode);
        main_window.set_cfg_autostart(is_autostart);
        main_window.set_show_card_vision(cfg.show_card_vision);
        main_window.set_show_card_automation(cfg.show_card_automation);
        main_window.set_show_card_transcript(cfg.show_card_transcript);
        main_window.set_show_card_telemetry(cfg.show_card_telemetry);
        main_window.set_show_card_hotkeys(cfg.show_card_hotkeys);
        main_window.set_split_left_width(cfg.split_left_width);
        main_window.set_split_right_width(cfg.split_right_width);
        main_window.set_split_left_h(cfg.split_left_h);
        main_window.set_split_center_h(cfg.split_center_h);
        main_window.set_split_right_h(cfg.split_right_h);
        main_window.set_app_version(env!("CARGO_PKG_VERSION").into());
        main_window.set_status_message("Ready to connect.".into());
    }

    // 3.1 Initialize Slint OverlayWindow and Stealth Highlighter Controller
    let overlay_window = ui::OverlayWindow::new().ok();
    if let Some(ref ow) = overlay_window {
        let ow_weak = ow.as_weak();
        let screen = automation::get_primary_screen_dimensions();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ow_weak.upgrade() {
                w.set_window_width(screen.width as f32);
                w.set_window_height(screen.height as f32);
                w.set_highlight_visible(false);
                w.set_ripple_visible(false);
                let _ = w.show();
            }
        });

        // Apply Win32 stealth styles & DWM alpha frame extension immediately at startup
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(60)).await;
            automation::apply_stealth_window_styles();
        });
    }

    let highlighter_controller = Arc::new(automation::HighlighterController::new(
        overlay_window.as_ref().map(|w| w.as_weak()),
    ));

    // 4. Initialize Network Client
    let network_client = NetworkClient::new(Arc::clone(&config));

    // 5. Initialize WASAPI Duplex Audio Engine (Option-wrapped: Claude Bug #5)
    let audio_engine: Option<Arc<tokio::sync::Mutex<audio::AudioEngine>>> =
        match audio::AudioEngine::new() {
            Ok(e) => Some(Arc::new(tokio::sync::Mutex::new(e))),
            Err(e) => {
                error!("[MAIN] WASAPI Audio Engine initialization failed: {}", e);
                main_window.set_status_message(format!("Audio unavailable: {}", e).into());
                None
            }
        };

    // 5.1 Initialize Direct3D 11 GPU Screen Capture Controller
    let capture_controller: Arc<Mutex<Option<capture::ScreenCaptureController>>> =
        Arc::new(Mutex::new(None));

    let toggle_screen_share: Arc<dyn Fn(Option<bool>) + Send + Sync> = {
        let capture_ctrl = Arc::clone(&capture_controller);
        let net_client = network_client.clone();
        let window_weak = main_window.as_weak();
        let config_clone = Arc::clone(&config);

        Arc::new(move |target_state: Option<bool>| {
            let window_weak = window_weak.clone();
            let capture_ctrl = Arc::clone(&capture_ctrl);
            let net_client = net_client.clone();
            let config_clone = Arc::clone(&config_clone);

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = window_weak.upgrade() {
                    let current = win.get_is_screen_sharing();
                    let new_state = target_state.unwrap_or(!current);

                    win.set_is_screen_sharing(new_state);
                    info!("[MAIN] 👁️ Screen Sharing toggled to: {}", new_state);

                    let mut ctrl = capture_ctrl.lock().unwrap();

                    if new_state {
                        let fps = {
                            let cfg = config_clone.lock().unwrap();
                            cfg.capture_fps.max(1)
                        };

                        // Channel capacity 1: only ever buffers 1 frame to prevent queue backlog
                        let (frame_tx, mut frame_rx) =
                            tokio::sync::mpsc::channel::<String>(1);
                        let net_client_frame = net_client.clone();

                        // Stable session_id per screen sharing session (Claude Bug #9)
                        let session_id = format!(
                            "session_desktop_{}",
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis()
                        );

                        tokio::spawn(async move {
                            while let Some(image_base64) = frame_rx.recv().await {
                                if net_client_frame.is_connected() {
                                    let frame_msg = ClientMessage::VideoFrame {
                                        command_id: session_id.clone(),
                                        image_chunk: image_base64,
                                    };
                                    let _ = net_client_frame.send_message(frame_msg).await;
                                }
                            }
                            info!("[MAIN] Screen capture frame transmitter loop ended.");
                        });

                        // Run capture initialization in spawn_blocking so UI thread doesn't freeze (Claude Bug #7)
                        let capture_ctrl_bg = Arc::clone(&capture_ctrl);
                        let window_weak_bg = window_weak.clone();
                        tokio::task::spawn_blocking(move || {
                            match capture::ScreenCaptureController::start(frame_tx, fps) {
                                Ok(c) => {
                                    {
                                        let mut ctrl = capture_ctrl_bg.lock().unwrap();
                                        *ctrl = Some(c);
                                    }
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(win) = window_weak_bg.upgrade() {
                                            win.set_avatar_state(
                                                ui::CognitiveState::ScreenSharing.as_str().into(),
                                            );
                                            win.set_status_message(
                                                ui::CognitiveState::ScreenSharing.status_text().into(),
                                            );
                                        }
                                    });
                                }
                                Err(e) => {
                                    error!("[MAIN] Failed to start screen capture: {}", e);
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(win) = window_weak_bg.upgrade() {
                                            win.set_is_screen_sharing(false);
                                            win.set_status_message(
                                                format!("Screen Capture Error: {}", e).into(),
                                            );
                                        }
                                    });
                                }
                            }
                        });
                    } else {
                        if let Some(mut c) = ctrl.take() {
                            c.stop();
                        }
                        platform::compact_memory_if_low_ram(true);
                        win.set_avatar_state(ui::CognitiveState::Idle.as_str().into());
                        win.set_status_message(ui::CognitiveState::Idle.status_text().into());
                    }
                }
            });
        })
    };

    // 6. Hook Connection State Watcher & Duplex Audio Engine Lifecycle
    let window_weak = main_window.as_weak();
    let mut state_rx = network_client.subscribe_state();
    let audio_engine_conn = audio_engine.clone();
    let net_client_audio = network_client.clone();
    let toggle_share_conn = Arc::clone(&toggle_screen_share);

    tokio::spawn(async move {
        let (mic_pcm_tx, mut mic_pcm_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(128);

        // Forward captured 16kHz PCM chunks from ThinkPad microphone to WebSocket & Drive Slint UI
        let net_client_mic = net_client_audio.clone();
        let window_weak_mic = window_weak.clone();
        tokio::spawn(async move {
            let mut prev_had_voice = false;
            let mut last_ui_mic_tick = std::time::Instant::now();
            while let Some(pcm_chunk) = mic_pcm_rx.recv().await {
                if net_client_mic.is_connected() {
                    // Compute Mic RMS for user voice reactivity when listening
                    let pcm_i16 = audio::resampler::pcm_bytes_to_i16(&pcm_chunk);
                    let mut sum_sq: f64 = 0.0;
                    for &sample in &pcm_i16 {
                        let s = sample as f64 / 32768.0;
                        sum_sq += s * s;
                    }
                    let rms = if !pcm_i16.is_empty() {
                        (sum_sq / pcm_i16.len() as f64).sqrt() as f32
                    } else {
                        0.0
                    };

                    if rms > 0.015 {
                        let vol = (rms * 3.5).min(1.0);
                        let pulse = 1.0 + vol * 0.25;
                        prev_had_voice = true;
                        let now = std::time::Instant::now();
                        if now.duration_since(last_ui_mic_tick) >= std::time::Duration::from_millis(60) {
                            last_ui_mic_tick = now;
                            let window_weak = window_weak_mic.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(win) = window_weak.upgrade() {
                                    if win.get_avatar_state() == "listening" {
                                        win.set_volume_level(vol);
                                        win.set_pulse_scale(pulse);
                                    }
                                }
                            });
                        }
                    } else if prev_had_voice {
                        prev_had_voice = false;
                        let window_weak = window_weak_mic.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = window_weak.upgrade() {
                                if win.get_avatar_state() == "listening" {
                                    win.set_volume_level(0.0);
                                    win.set_pulse_scale(1.0);
                                }
                            }
                        });
                    }

                    let _ = net_client_mic.send_binary(pcm_chunk).await;
                }
            }
        });

        while state_rx.changed().await.is_ok() {
            let state = state_rx.borrow().clone();
            let window_weak = window_weak.clone();

            // Update UI status immediately to prevent UI hanging on audio device lock (Claude Bug #8)
            let state_clone = state.clone();
            let toggle_share_conn = toggle_share_conn.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = window_weak.upgrade() {
                    match state_clone {
                        ConnectionState::Connected => {
                            win.set_connection_status("connected".into());
                            win.set_avatar_state(ui::CognitiveState::Idle.as_str().into());
                            win.set_status_message(ui::CognitiveState::Idle.status_text().into());
                        }
                        ConnectionState::Connecting => {
                            win.set_connection_status("connecting".into());
                            win.set_avatar_state(ui::CognitiveState::Connecting.as_str().into());
                            win.set_status_message(ui::CognitiveState::Connecting.status_text().into());
                        }
                        ConnectionState::Reconnecting {
                            attempt,
                            next_retry_ms,
                        } => {
                            win.set_connection_status("connecting".into());
                            win.set_avatar_state(ui::CognitiveState::Connecting.as_str().into());
                            win.set_status_message(
                                format!(
                                    "Menghubungkan kembali ke server dalam {}ms (Percobaan #{})",
                                    next_retry_ms, attempt
                                )
                                .into(),
                            );
                        }
                        ConnectionState::Disconnected => {
                            win.set_connection_status("offline".into());
                            win.set_avatar_state(ui::CognitiveState::Offline.as_str().into());
                            win.set_status_message(ui::CognitiveState::Offline.status_text().into());
                            win.set_volume_level(0.0);
                            win.set_pulse_scale(1.0);
                            if win.get_is_screen_sharing() {
                                toggle_share_conn(Some(false));
                            }
                        }
                        ConnectionState::Suspended => {
                            win.set_connection_status("offline".into());
                            win.set_avatar_state(ui::CognitiveState::Offline.as_str().into());
                            win.set_status_message("System suspended (Sleep).".into());
                            win.set_volume_level(0.0);
                            win.set_pulse_scale(1.0);
                            if win.get_is_screen_sharing() {
                                toggle_share_conn(Some(false));
                            }
                        }
                    }
                }
            });

            // Start or stop WASAPI duplex audio engine asynchronously in background task (Claude Bug #5 & #8)
            match &state {
                ConnectionState::Connected => {
                    info!("[MAIN] 🎙️ Connection active: Starting WASAPI Duplex Audio Engine...");
                    if let Some(audio_clone) = audio_engine_conn.as_ref().map(Arc::clone) {
                        let mic_tx = mic_pcm_tx.clone();
                        tokio::spawn(async move {
                            let mut engine = audio_clone.lock().await;
                            if let Err(e) = engine.start_duplex(mic_tx) {
                                warn!("[MAIN] Audio Duplex start warning: {}", e);
                            }
                        });
                    }

                    // Explicitly notify server that desktop companion is ready for Live Voice
                    let net_client_handshake = net_client_audio.clone();
                    let now_ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    tokio::spawn(async move {
                        let call_start_msg = ClientMessage::CallEvent {
                            event: "CALL_ACCEPTED".to_string(),
                            command_id: format!("win_desktop_{}", now_ts),
                        };
                        let _ = net_client_handshake.send_message(call_start_msg).await;
                    });
                }
                ConnectionState::Disconnected | ConnectionState::Suspended => {
                    info!("[MAIN] Connection inactive: Stopping WASAPI Duplex Audio Engine...");
                    if let Some(audio_clone) = audio_engine_conn.as_ref().map(Arc::clone) {
                        tokio::spawn(async move {
                            let mut engine = audio_clone.lock().await;
                            engine.stop_duplex();
                        });
                    }
                }
                _ => {}
            }
        }
    });

    // 7. Hook Inbound Server Command & Audio Dispatcher
    let mut command_rx = network_client.subscribe_commands();
    let net_client_cmd = network_client.clone();
    let audio_engine_cmd = audio_engine.clone();
    let window_weak_cmd = main_window.as_weak();
    let highlighter_cmd = Arc::clone(&highlighter_controller);

    tokio::spawn(async move {
        let mut last_ui_audio_tick = std::time::Instant::now();
        while let Ok(msg) = command_rx.recv().await {
            match msg {
                ServerMessage::AudioPlay { pcm_chunk } => {
                    use base64::prelude::*;
                    if let Ok(bytes) = BASE64_STANDARD.decode(&pcm_chunk) {
                        let pcm_i16 = audio::resampler::pcm_bytes_to_i16(&bytes);
                        let mut sum_sq: f64 = 0.0;
                        for &sample in &pcm_i16 {
                            let s = sample as f64 / 32768.0;
                            sum_sq += s * s;
                        }
                        let rms = if !pcm_i16.is_empty() {
                            (sum_sq / pcm_i16.len() as f64).sqrt() as f32
                        } else {
                            0.0
                        };
                        let vol = (rms * 3.5).min(1.0);
                        let pulse = 1.0 + vol * 0.35;

                        // Throttle Slint UI event loop dispatch to ~16 updates/sec (60ms) to prevent UI thread lockup during burst streaming
                        let now = std::time::Instant::now();
                        if now.duration_since(last_ui_audio_tick) >= std::time::Duration::from_millis(60) {
                            last_ui_audio_tick = now;
                            let window_weak = window_weak_cmd.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(win) = window_weak.upgrade() {
                                    win.set_volume_level(vol);
                                    win.set_pulse_scale(pulse);
                                }
                            });
                        }

                        if let Some(audio_clone) = audio_engine_cmd.as_ref() {
                            let mut engine = audio_clone.lock().await;
                            engine.play_response_pcm(&bytes);
                        }
                    }
                }
                ServerMessage::AudioInterrupted => {
                    info!("[MAIN] Server signaled audio interrupted (barge-in).");
                    if let Some(audio_clone) = audio_engine_cmd.as_ref() {
                        let mut engine = audio_clone.lock().await;
                        engine.stop_speaking();
                    }

                    let window_weak = window_weak_cmd.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = window_weak.upgrade() {
                            win.set_volume_level(0.0);
                            win.set_pulse_scale(1.0);
                        }
                    });
                }
                ServerMessage::StatusUpdate {
                    status,
                    text,
                    user_transcript,
                } => {
                    let maybe_cog = ui::CognitiveState::try_from_status(&status);
                    let window_weak = window_weak_cmd.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = window_weak.upgrade() {
                            if let Some(cog) = maybe_cog {
                                win.set_avatar_state(cog.as_str().into());
                                if !cog.is_vocal() {
                                    win.set_volume_level(0.0);
                                    win.set_pulse_scale(1.0);
                                }
                            }
                            if let Some(ref ut) = user_transcript {
                                win.set_transcript_user_text(ut.clone().into());
                            }
                            if let Some(ref t) = text {
                                win.set_transcript_assistant_text(t.clone().into());
                                win.set_status_message(t.clone().into());
                            } else if let Some(cog) = maybe_cog {
                                win.set_status_message(cog.status_text().into());
                            }
                        }
                    });
                }
                ServerMessage::ReplyComplete => {
                    let window_weak = window_weak_cmd.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = window_weak.upgrade() {
                            win.set_avatar_state(ui::CognitiveState::Idle.as_str().into());
                            win.set_status_message(ui::CognitiveState::Idle.status_text().into());
                            win.set_volume_level(0.0);
                            win.set_pulse_scale(1.0);
                        }
                    });
                }
                ServerMessage::LiveReady { session_id, model } => {
                    info!("[MAIN] ⚡ Gemini Live Voice Ready! Session: {:?}, Model: {:?}", session_id, model);
                    let window_weak = window_weak_cmd.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = window_weak.upgrade() {
                            win.set_avatar_state(ui::CognitiveState::Listening.as_str().into());
                            win.set_status_message("Ready for voice input".into());
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

                    let window_weak_auto = window_weak_cmd.clone();
                    // Safe UTF-8 substring extraction to prevent character boundary panics (Claude Bug #12)
                    let short_id: String = command_id.chars().take(8).collect();
                    let action_log = format!("Exec: {} [{}]", action, short_id);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = window_weak_auto.upgrade() {
                            let p1 = win.get_auto_log_1();
                            let p2 = win.get_auto_log_2();
                            win.set_auto_log_3(p2);
                            win.set_auto_log_2(p1);
                            win.set_auto_log_1(action_log.into());
                        }
                    });

                    let hl = Arc::clone(&highlighter_cmd);
                    let net_tx = net_client_cmd.clone();
                    tokio::spawn(async move {
                        let result = automation::handle_execute_command(
                            command_id,
                            action,
                            params,
                            Some(&hl),
                        )
                        .await;

                        let _ = net_tx.send_message(result).await;
                    });
                }
                _ => {}
            }
        }
    });

    // 8. Hook User Barge-In Notification (Mic energy threshold exceeded during AI speech)
    if let Some(audio_engine_barge) = audio_engine.as_ref().map(Arc::clone) {
        let barge_in_notify = {
            let engine = audio_engine_barge.lock().await;
            engine.barge_in_notify()
        };
        let net_client_barge = network_client.clone();
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
                        win.set_avatar_state(ui::CognitiveState::BargeIn.as_str().into());
                        win.set_status_message(ui::CognitiveState::BargeIn.status_text().into());
                        win.set_volume_level(0.0);
                        win.set_pulse_scale(1.0);
                    }
                });
            }
        });
    }

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

    // 10. Register Windows Native System Tray Controller
    let window_weak_tray = main_window.as_weak();
    let audio_engine_tray = audio_engine.clone();
    let toggle_share_tray = Arc::clone(&toggle_screen_share);
    let _system_tray = SystemTray::spawn(move |event| {
        let window_weak = window_weak_tray.clone();
        let audio_engine = audio_engine_tray.clone();
        let toggle_share = Arc::clone(&toggle_share_tray);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(win) = window_weak.upgrade() {
                match event {
                    TrayEvent::ToggleWindow => {
                        let is_vis = win.window().is_visible();
                        let is_min = win.window().is_minimized();
                        info!("[TRAY] 🖱️ Tray icon clicked (visible={}, minimized={})", is_vis, is_min);
                        if is_min {
                            win.window().set_minimized(false);
                            let _ = win.window().show();
                        } else if is_vis {
                            let _ = win.window().hide();
                        } else {
                            let _ = win.window().show();
                        }
                    }
                    TrayEvent::ShowWindow => {
                        info!("[TRAY] Context menu 'Buka N.E.X.A' activated.");
                        win.window().set_minimized(false);
                        let _ = win.window().show();
                    }
                    TrayEvent::ToggleMute => {
                        let muted = !win.get_is_mic_muted();
                        info!("[TRAY] Context menu 'Mute / Unmute' activated: muted={}", muted);
                        win.set_is_mic_muted(muted);
                        if let Some(audio_clone) = audio_engine.as_ref() {
                            let audio_clone = Arc::clone(audio_clone);
                            tokio::spawn(async move {
                                let engine = audio_clone.lock().await;
                                engine.set_mic_muted(muted);
                            });
                        }
                    }
                    TrayEvent::ToggleFullscreen => {
                        let fs = !win.get_is_fullscreen();
                        info!("[TRAY] Context menu 'Layar Penuh' activated: fs={}", fs);
                        win.set_is_fullscreen(fs);
                        win.window().set_fullscreen(fs);
                        if !fs {
                            win.window().set_maximized(false);
                        }
                    }
                    TrayEvent::ToggleScreenShare => {
                        info!("[TRAY] Context menu 'Berbagi Layar' activated.");
                        toggle_share(None);
                    }
                    TrayEvent::OpenSettings => {
                        info!("[TRAY] Context menu 'Pengaturan' activated.");
                        win.window().set_minimized(false);
                        let _ = win.window().show();
                        win.set_show_settings(true);
                    }
                    TrayEvent::Quit => {
                        info!("[TRAY] Context menu 'Keluar' activated. Exiting application gracefully.");
                        let _ = slint::quit_event_loop();
                    }
                }
            }
        });
    });

    // 11. Register Windows Global Hotkeys (Boss Key, Mic Mute, Fullscreen, Screen Share)
    let window_weak_hotkey = main_window.as_weak();
    let audio_engine_hotkey = audio_engine.clone();
    let toggle_share_hotkey = Arc::clone(&toggle_screen_share);
    let _hotkey_listener = HotkeyListener::spawn(move |action| {
        let window_weak = window_weak_hotkey.clone();
        let audio_engine = audio_engine_hotkey.clone();
        let toggle_share = Arc::clone(&toggle_share_hotkey);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(win) = window_weak.upgrade() {
                match action {
                    HotkeyAction::BossKey => {
                        let is_vis = win.window().is_visible();
                        let is_min = win.window().is_minimized();
                        info!("[HOTKEY] 🕶️ Boss Key (Win+Alt+N) triggered (visible={}, minimized={})", is_vis, is_min);
                        if is_min {
                            win.window().set_minimized(false);
                            let _ = win.window().show();
                        } else if is_vis {
                            let _ = win.window().hide();
                        } else {
                            let _ = win.window().show();
                        }
                    }
                    HotkeyAction::ToggleMute => {
                        let muted = !win.get_is_mic_muted();
                        info!("[HOTKEY] 🎙️ Mic Mute (Win+Alt+M) triggered: muted={}", muted);
                        win.set_is_mic_muted(muted);
                        if let Some(audio_clone) = audio_engine.as_ref() {
                            let audio_clone = Arc::clone(audio_clone);
                            tokio::spawn(async move {
                                let engine = audio_clone.lock().await;
                                engine.set_mic_muted(muted);
                            });
                        }
                    }
                    HotkeyAction::ToggleFullscreen => {
                        let fs = !win.get_is_fullscreen();
                        info!("[HOTKEY] ⛶ Fullscreen Toggle (F11 / Win+Alt+F) triggered: fs={}", fs);
                        win.set_is_fullscreen(fs);
                        win.window().set_fullscreen(fs);
                        if !fs {
                            win.window().set_maximized(false);
                        }
                    }
                    HotkeyAction::ToggleScreenShare => {
                        info!("[HOTKEY] 🖥️ Screen Share (Win+Alt+S) triggered.");
                        toggle_share(None);
                    }
                }
            }
        });
    });

    // 12. Periodic System Telemetry Updater (CPU, RAM, Audio, Ping) for Fullscreen HUD
    let window_weak_telem = main_window.as_weak();
    let net_client_telem = network_client.clone();
    tokio::spawn(async move {
        let mut telemetry = SystemTelemetry::new();
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));

        loop {
            interval.tick().await;

            if window_weak_telem.upgrade().is_none() {
                break;
            }

            let cpu_pct = telemetry.sample_cpu_percent();
            let ram_pct = SystemTelemetry::sample_ram_percent();
            let is_connected = net_client_telem.is_connected();

            let window_weak = window_weak_telem.clone();
            let rtt = net_client_telem.get_last_rtt_ms();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = window_weak.upgrade() {
                    win.set_telemetry_cpu(format!("{}%", cpu_pct).into());
                    win.set_telemetry_cpu_val((cpu_pct as f32) / 100.0);
                    win.set_telemetry_ram(format!("{}%", ram_pct).into());
                    win.set_telemetry_ram_val((ram_pct as f32) / 100.0);

                    if win.get_is_mic_muted() {
                        win.set_telemetry_audio("WASAPI (Muted)".into());
                    } else if is_connected {
                        win.set_telemetry_audio("WASAPI 48kHz (AEC3)".into());
                    } else {
                        win.set_telemetry_audio("Standby".into());
                    }

                    if is_connected {
                        if rtt > 0 {
                            win.set_telemetry_ping(format!("{}ms", rtt).into());
                        } else {
                            win.set_telemetry_ping("<10ms".into());
                        }
                    } else {
                        win.set_telemetry_ping("--".into());
                    }
                }
            });
        }
    });

    // 8. Hook Slint UI Callbacks
    let config_clone = Arc::clone(&config);
    let window_weak = main_window.as_weak();
    main_window.on_save_settings(
        move |url, token, fps, low_ram, autostart, vision, auto, trans, telem, hotkeys| {
            let mut cfg = config_clone.lock().unwrap();
            cfg.server_url = url.to_string();
            cfg.auth_token = token.to_string();
            cfg.capture_fps = fps.max(1) as u32;
            cfg.low_ram_mode = low_ram;
            cfg.autostart = autostart;
            cfg.show_card_vision = vision;
            cfg.show_card_automation = auto;
            cfg.show_card_transcript = trans;
            cfg.show_card_telemetry = telem;
            cfg.show_card_hotkeys = hotkeys;

            if let Some(win) = window_weak.upgrade() {
                cfg.split_left_width = win.get_split_left_width();
                cfg.split_right_width = win.get_split_right_width();
                cfg.split_left_h = win.get_split_left_h();
                cfg.split_center_h = win.get_split_center_h();
                cfg.split_right_h = win.get_split_right_h();
            }

            let _ = platform::set_autostart_enabled(autostart);

            match cfg.save() {
                Ok(_) => {
                    info!("Settings saved successfully (autostart={}, fps={}).", autostart, fps);
                    if let Some(win) = window_weak.upgrade() {
                        win.set_cfg_server_url(url.clone());
                        win.set_cfg_auth_token(token.clone());
                        win.set_cfg_fps(fps.max(1));
                        win.set_cfg_low_ram_mode(low_ram);
                        win.set_cfg_autostart(autostart);
                        win.set_show_card_vision(vision);
                        win.set_show_card_automation(auto);
                        win.set_show_card_transcript(trans);
                        win.set_show_card_telemetry(telem);
                        win.set_show_card_hotkeys(hotkeys);
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
        },
    );

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

    // Hook Fullscreen Toggle
    let window_weak = main_window.as_weak();
    main_window.on_toggle_fullscreen(move || {
        if let Some(win) = window_weak.upgrade() {
            let fs = win.get_is_fullscreen();
            info!("[MAIN] ⛶ Fullscreen toggled: {}", fs);
            win.window().set_fullscreen(fs);
            if !fs {
                win.window().set_maximized(false);
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        FindWindowW, GetSystemMetrics, SetWindowPos, HWND_TOPMOST, SM_CXSCREEN,
                        SWP_SHOWWINDOW,
                    };
                    let title: Vec<u16> = "N.E.X.A Windows Bridge\0".encode_utf16().collect();
                    let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
                    if !hwnd.is_null() {
                        let screen_w = GetSystemMetrics(SM_CXSCREEN);
                        let pill_w = 400;
                        let pill_h = 64;
                        let pos_x = (screen_w - pill_w) / 2;
                        let pos_y = 16;
                        SetWindowPos(hwnd, HWND_TOPMOST, pos_x, pos_y, pill_w, pill_h, SWP_SHOWWINDOW);
                    }
                }
            }
        }
    });

    // Hook Microphone Mute Toggle
    let audio_engine_mute = audio_engine.clone();
    let window_weak = main_window.as_weak();
    main_window.on_toggle_mic_mute(move || {
        if let Some(win) = window_weak.upgrade() {
            let muted = win.get_is_mic_muted();
            info!("[MAIN] 🎙️ Microphone mute toggled: {}", muted);
            if let Some(audio_clone) = audio_engine_mute.as_ref() {
                let audio_clone = Arc::clone(audio_clone);
                tokio::spawn(async move {
                    let engine = audio_clone.lock().await;
                    engine.set_mic_muted(muted);
                });
            }
        }
    });

    // Hook Screen Sharing Toggle
    let toggle_share_ui = Arc::clone(&toggle_screen_share);
    main_window.on_toggle_screen_share(move || {
        toggle_share_ui(None);
    });

    // Hook Native Window Drag (WM_NCLBUTTONDOWN + HTCAPTION)
    main_window.on_start_window_drag(move || {
        unsafe {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                FindWindowW, SendMessageW, HTCAPTION, WM_NCLBUTTONDOWN,
            };
            let title: Vec<u16> = "N.E.X.A Windows Bridge\0".encode_utf16().collect();
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if !hwnd.is_null() {
                ReleaseCapture();
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
            }
        }
    });

    // Hook Settings Modal Open (Option A: Smooth Dynamic Expansion)
    let window_weak = main_window.as_weak();
    main_window.on_open_settings(move || {
        if let Some(win) = window_weak.upgrade() {
            win.set_show_settings(true);
            if !win.get_is_fullscreen() {
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        FindWindowW, GetSystemMetrics, SetWindowPos, HWND_TOPMOST, SM_CXSCREEN,
                        SWP_SHOWWINDOW,
                    };
                    let title: Vec<u16> = "N.E.X.A Windows Bridge\0".encode_utf16().collect();
                    let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
                    if !hwnd.is_null() {
                        let screen_w = GetSystemMetrics(SM_CXSCREEN);
                        let w = 560;
                        let h = 540;
                        let pos_x = (screen_w - w) / 2;
                        let pos_y = 60;
                        SetWindowPos(hwnd, HWND_TOPMOST, pos_x, pos_y, w, h, SWP_SHOWWINDOW);
                    }
                }
            }
        }
    });

    // Hook Settings Modal Close (Restores to 400x64 Floating Capsule Pill)
    let window_weak = main_window.as_weak();
    main_window.on_close_settings(move || {
        if let Some(win) = window_weak.upgrade() {
            win.set_show_settings(false);
            if !win.get_is_fullscreen() {
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        FindWindowW, GetSystemMetrics, SetWindowPos, HWND_TOPMOST, SM_CXSCREEN,
                        SWP_SHOWWINDOW,
                    };
                    let title: Vec<u16> = "N.E.X.A Windows Bridge\0".encode_utf16().collect();
                    let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
                    if !hwnd.is_null() {
                        let screen_w = GetSystemMetrics(SM_CXSCREEN);
                        let pill_w = 400;
                        let pill_h = 64;
                        let pos_x = (screen_w - pill_w) / 2;
                        let pos_y = 16;
                        SetWindowPos(hwnd, HWND_TOPMOST, pos_x, pos_y, pill_w, pill_h, SWP_SHOWWINDOW);
                    }
                }
            }
        }
    });

    // Intercept Window Close Button [X]: Hide window to system tray instead of abrupt termination (Claude Bug #2)
    main_window.window().on_close_requested(|| {
        info!("[MAIN] Window close [X] requested. Hiding window to system tray.");
        slint::CloseRequestResponse::HideWindow
    });

    // Keep overlay window alive across application runtime
    let _overlay_guard = overlay_window;

    // Run the Slint GUI Event Loop until explicit quit (Claude Bug #2 & #3)
    if !start_minimized {
        main_window.show()?;
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                FindWindowW, GetSystemMetrics, SetWindowPos, HWND_TOPMOST, SM_CXSCREEN,
                SWP_SHOWWINDOW,
            };
            let title: Vec<u16> = "N.E.X.A Windows Bridge\0".encode_utf16().collect();
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if !hwnd.is_null() {
                let screen_w = GetSystemMetrics(SM_CXSCREEN);
                let pill_w = 400;
                let pill_h = 64;
                let pos_x = (screen_w - pill_w) / 2;
                let pos_y = 16;
                SetWindowPos(hwnd, HWND_TOPMOST, pos_x, pos_y, pill_w, pill_h, SWP_SHOWWINDOW);
            }
        }
    } else {
        info!("[MAIN] 🔕 Started in Auto-Start Standby mode (minimized to system tray)");
    }
    slint::run_event_loop_until_quit()?;
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
