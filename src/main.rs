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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("nexa-bridge")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&log_dir)?;
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
                if !win.get_is_fullscreen() {
                    unsafe {
                        use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
                        let hwnd = get_main_window_hwnd();
                        if !hwnd.is_null() {
                            position_floating_pill_window(hwnd);
                            SetForegroundWindow(hwnd);
                        }
                    }
                }
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
    // NOTE: Do NOT show the overlay window here. It must remain hidden until
    // a highlight/ripple is actually requested. Slint windows are rendered via
    // their own GPU swapchain — DWM frame extension and SetLayeredWindowAttributes
    // do NOT make a Slint window transparent. The overlay window must only be
    // shown momentarily when overlay graphics are active, then hidden again.
    let overlay_window = ui::OverlayWindow::new().ok();

    let highlighter_controller = Arc::new(automation::HighlighterController::new(
        overlay_window.as_ref().map(|w| w.as_weak()),
    ));

    // 4. Initialize Network Client
    let network_client = NetworkClient::new(Arc::clone(&config));

    // 5. Initialize WASAPI Duplex Audio Engine (Option-wrapped: Claude Bug #5)
    let (audio_engine, is_speaker_playing, speaker_playback_rms) =
        match audio::AudioEngine::new() {
            Ok(e) => {
                let playing_h = e.is_speaker_playing_handle();
                let rms_h = e.speaker_playback_rms_handle();
                (
                    Some(Arc::new(tokio::sync::Mutex::new(e))),
                    Some(playing_h),
                    Some(rms_h),
                )
            }
            Err(e) => {
                error!("[MAIN] WASAPI Audio Engine initialization failed: {}", e);
                main_window.set_status_message(format!("Audio unavailable: {}", e).into());
                (None, None, None)
            }
        };

    // 5.1 Initialize Direct3D 11 GPU Screen Capture Controller
    let capture_controller: Arc<Mutex<Option<capture::ScreenCaptureController>>> =
        Arc::new(Mutex::new(None));
    let is_capture_desired = Arc::new(AtomicBool::new(false));
    let capture_generation = Arc::new(AtomicU64::new(0));

    // Session generation epoch & automation armed guard for security isolation (Audit K1, K2, N2)
    // Default-deny policy: automation starts DISARMED and requires explicit user enablement
    let session_generation = Arc::new(AtomicU64::new(0));
    let is_automation_armed = Arc::new(AtomicBool::new(false));

    let toggle_screen_share: Arc<dyn Fn(Option<bool>) + Send + Sync> = {
        let capture_ctrl = Arc::clone(&capture_controller);
        let is_capture_desired = Arc::clone(&is_capture_desired);
        let capture_generation = Arc::clone(&capture_generation);
        let net_client = network_client.clone();
        let window_weak = main_window.as_weak();
        let config_clone = Arc::clone(&config);

        Arc::new(move |target_state: Option<bool>| {
            let window_weak = window_weak.clone();
            let capture_ctrl = Arc::clone(&capture_ctrl);
            let is_capture_desired = Arc::clone(&is_capture_desired);
            let capture_generation = Arc::clone(&capture_generation);
            let net_client = net_client.clone();
            let config_clone = Arc::clone(&config_clone);

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = window_weak.upgrade() {
                    let current = win.get_is_screen_sharing();
                    let new_state = target_state.unwrap_or(!current);

                    win.set_is_screen_sharing(new_state);
                    info!("[MAIN] 👁️ Screen Sharing toggled to: {}", new_state);

                    unsafe {
                        let hwnd = get_main_window_hwnd();
                        enforce_system_cursor_visible(hwnd);
                    }

                    if new_state {
                        let gen = capture_generation.fetch_add(1, Ordering::SeqCst) + 1;
                        is_capture_desired.store(true, Ordering::SeqCst);

                        // If any previous controller is lingering, stop it in spawn_blocking immediately (Audit K3)
                        {
                            let mut ctrl = capture_ctrl.lock().unwrap();
                            if let Some(mut old_c) = ctrl.take() {
                                tokio::task::spawn_blocking(move || {
                                    old_c.stop();
                                });
                            }
                        }

                        let fps = {
                            let cfg = config_clone.lock().unwrap();
                            cfg.capture_fps.max(1)
                        };

                        // Channel capacity 1: only ever buffers 1 frame to prevent queue backlog
                        let (frame_tx, mut frame_rx) =
                            tokio::sync::mpsc::channel::<String>(1);
                        let net_client_frame = net_client.clone();
                        let is_capture_desired_tx = Arc::clone(&is_capture_desired);
                        let capture_gen_tx = Arc::clone(&capture_generation);

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
                                if !is_capture_desired_tx.load(Ordering::SeqCst)
                                    || capture_gen_tx.load(Ordering::SeqCst) != gen
                                {
                                    info!("[MAIN] Screen capture gen {} stopped, terminating frame transmitter loop.", gen);
                                    break;
                                }
                                if net_client_frame.is_connected() {
                                    let frame_msg = ClientMessage::VideoFrame {
                                        command_id: session_id.clone(),
                                        image_chunk: image_base64,
                                    };
                                    let _ = net_client_frame.send_message(frame_msg).await;
                                }
                            }
                            info!("[MAIN] Screen capture frame transmitter loop ended for gen {}.", gen);
                        });

                        // Run capture initialization in spawn_blocking so UI thread doesn't freeze (Claude Bug #7)
                        let capture_ctrl_bg = Arc::clone(&capture_ctrl);
                        let is_capture_desired_bg = Arc::clone(&is_capture_desired);
                        let capture_gen_bg = Arc::clone(&capture_generation);
                        let window_weak_bg = window_weak.clone();
                        tokio::task::spawn_blocking(move || {
                            match capture::ScreenCaptureController::start(frame_tx, fps) {
                                Ok(c) => {
                                    let still_desired = is_capture_desired_bg.load(Ordering::SeqCst)
                                        && capture_gen_bg.load(Ordering::SeqCst) == gen;
                                    if still_desired {
                                        let mut ctrl = capture_ctrl_bg.lock().unwrap();
                                        if is_capture_desired_bg.load(Ordering::SeqCst)
                                            && capture_gen_bg.load(Ordering::SeqCst) == gen
                                        {
                                            *ctrl = Some(c);
                                            let _ = slint::invoke_from_event_loop(move || {
                                                if let Some(win) = window_weak_bg.upgrade() {
                                                    unsafe {
                                                        let hwnd = get_main_window_hwnd();
                                                        enforce_system_cursor_visible(hwnd);
                                                    }
                                                    win.set_avatar_state(
                                                        ui::CognitiveState::ScreenSharing.as_str().into(),
                                                    );
                                                    win.set_status_message(
                                                        ui::CognitiveState::ScreenSharing.status_text().into(),
                                                    );
                                                }
                                            });
                                            return;
                                        }
                                    }
                                    info!("[MAIN] Screen capture gen {} was cancelled before activation, stopping immediately.", gen);
                                    let mut stopped_c = c;
                                    stopped_c.stop();
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
                        capture_generation.fetch_add(1, Ordering::SeqCst);
                        is_capture_desired.store(false, Ordering::SeqCst);
                        // Cleanly stop capture controller in background thread to avoid freezing UI (Audit K3)
                        let old_controller = {
                            let mut ctrl = capture_ctrl.lock().unwrap();
                            ctrl.take()
                        };
                        if let Some(mut c) = old_controller {
                            tokio::task::spawn_blocking(move || {
                                c.stop();
                            });
                        }
                        unsafe {
                            let hwnd = get_main_window_hwnd();
                            enforce_system_cursor_visible(hwnd);
                        }
                        platform::compact_memory_if_low_ram(true);
                        win.set_avatar_state(ui::CognitiveState::Idle.as_str().into());
                        win.set_status_message(ui::CognitiveState::Idle.status_text().into());

                        // Send explicit SCREEN_SHARE_STOPPED CallEvent to server
                        let net_client_stop = net_client.clone();
                        tokio::spawn(async move {
                            if net_client_stop.is_connected() {
                                let stop_msg = ClientMessage::CallEvent {
                                    event: "SCREEN_SHARE_STOPPED".to_string(),
                                    command_id: String::new(),
                                };
                                let _ = net_client_stop.send_message(stop_msg).await;
                                info!("[MAIN] Sent SCREEN_SHARE_STOPPED CallEvent to server.");
                            }
                        });
                    }
                }
            });
        })
    };

    // 5.2 Spawn Real-Time Speaker Playback Rhythm & UI Equalizer Driver
    // Driven directly by CPAL hardware DAC callbacks (ThinkPad X380 Yoga audio rhythm)
    if let (Some(is_playing_h), Some(rms_h)) =
        (is_speaker_playing.clone(), speaker_playback_rms.clone())
    {
        let window_weak_playback = main_window.as_weak();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(30));
            let mut was_speaking = false;
            let mut last_ui_tick = std::time::Instant::now();

            loop {
                interval.tick().await;
                let is_playing = is_playing_h.load(Ordering::Relaxed);
                let rms_bits = rms_h.load(Ordering::Relaxed);
                let rms = f32::from_bits(rms_bits);

                if is_playing {
                    was_speaking = true;
                    let vol = (rms * 3.5).min(1.0);
                    let pulse = 1.0 + vol * 0.35;
                    let now = std::time::Instant::now();
                    if now.duration_since(last_ui_tick) >= std::time::Duration::from_millis(30) {
                        last_ui_tick = now;
                        let window_weak = window_weak_playback.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = window_weak.upgrade() {
                                if win.get_avatar_state() != "speaking" && win.get_avatar_state() != "barge_in" {
                                    win.set_avatar_state(ui::CognitiveState::Speaking.as_str().into());
                                    win.set_status_message(ui::CognitiveState::Speaking.status_text().into());
                                }
                                win.set_volume_level(vol);
                                win.set_pulse_scale(pulse);
                            }
                        });
                    }
                } else if was_speaking {
                    was_speaking = false;
                    let window_weak = window_weak_playback.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = window_weak.upgrade() {
                            if win.get_avatar_state() == "speaking" {
                                win.set_avatar_state(ui::CognitiveState::Listening.as_str().into());
                                win.set_status_message(ui::CognitiveState::Listening.status_text().into());
                            }
                            win.set_volume_level(0.0);
                            win.set_pulse_scale(1.0);
                        }
                    });
                }
            }
        });
    }

    // 6. Hook Connection State Watcher & Duplex Audio Engine Lifecycle
    let window_weak = main_window.as_weak();
    let mut state_rx = network_client.subscribe_state();
    let audio_engine_conn = audio_engine.clone();
    let net_client_audio = network_client.clone();
    let toggle_share_conn = Arc::clone(&toggle_screen_share);
    let is_speaker_playing_mic = is_speaker_playing.clone();
    let session_gen_conn = Arc::clone(&session_generation);
    let armed_conn = Arc::clone(&is_automation_armed);

    tokio::spawn(async move {
        let (mic_pcm_tx, mut mic_pcm_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(128);

        // Forward captured 16kHz PCM chunks from ThinkPad microphone to WebSocket & Drive Slint UI
        let net_client_mic = net_client_audio.clone();
        let window_weak_mic = window_weak.clone();
        let is_speaker_playing_mic_loop = is_speaker_playing_mic.clone();
        tokio::spawn(async move {
            let mut prev_had_voice = false;
            let mut last_ui_mic_tick = std::time::Instant::now();
            while let Some(pcm_chunk) = mic_pcm_rx.recv().await {
                if net_client_mic.is_connected() {
                    let is_currently_playing = is_speaker_playing_mic_loop
                        .as_ref()
                        .map(|h| h.load(Ordering::Relaxed))
                        .unwrap_or(false);

                    if !is_currently_playing {
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
                        ConnectionState::AuthRejected => {
                            win.set_connection_status("offline".into());
                            win.set_avatar_state(ui::CognitiveState::Offline.as_str().into());
                            win.set_status_message("Token Ditolak (HTTP 401/403). Periksa Auth Token di Pengaturan.".into());
                            win.set_volume_level(0.0);
                            win.set_pulse_scale(1.0);
                            if win.get_is_screen_sharing() {
                                toggle_share_conn(Some(false));
                            }
                        }
                    }
                }
            });

            // Start or stop WASAPI duplex audio engine asynchronously in background task (Claude Bug #5 & #8, Audit K4)
            match &state {
                ConnectionState::Connected => {
                    info!("[MAIN] 🎙️ Connection active: Starting WASAPI Duplex Audio Engine...");
                    automation::reset_automation_abort();
                    // Audit N2: Do NOT auto-rearm on connection! Armed status is strictly user-controlled.

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
                ConnectionState::Disconnected
                | ConnectionState::Suspended
                | ConnectionState::AuthRejected => {
                    info!("[MAIN] Connection inactive: Stopping WASAPI Duplex & Invalidating Session...");
                    session_gen_conn.fetch_add(1, Ordering::SeqCst);
                    automation::request_automation_abort();
                    armed_conn.store(false, Ordering::SeqCst);

                    if let Some(audio_clone) = audio_engine_conn.as_ref().map(Arc::clone) {
                        tokio::spawn(async move {
                            let mut engine = audio_clone.lock().await;
                            engine.stop_duplex();
                        });
                    }
                }
                ConnectionState::Reconnecting { attempt, .. } if *attempt >= 6 => {
                    info!("[MAIN] Extended reconnection (#{}): Stopping WASAPI Duplex Audio Engine...", attempt);
                    session_gen_conn.fetch_add(1, Ordering::SeqCst);
                    automation::request_automation_abort();

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
    let is_speaker_playing_cmd = is_speaker_playing.clone();
    let session_gen_cmd = Arc::clone(&session_generation);

    // Dedicated background audio stream playback worker (Audit N6: isolates audio bursts from control queue)
    let mut audio_rx = network_client.subscribe_audio();
    let audio_engine_playback = audio_engine.clone();
    tokio::spawn(async move {
        loop {
            match audio_rx.recv().await {
                Ok(pcm_bytes) => {
                    if let Some(audio_clone) = audio_engine_playback.as_ref() {
                        let mut engine = audio_clone.lock().await;
                        engine.play_response_pcm(&pcm_bytes);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    warn!("[MAIN] Audio playback stream lagged by {} chunks; dropping stale frames", missed);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("[MAIN] Audio playback stream closed.");
                    break;
                }
            }
        }
    });

    // Dedicated sequential FIFO queue for EXECUTE_COMMAND with generation & panic isolation (Audit H10, K1, K2, K9)
    let (fifo_cmd_tx, mut fifo_cmd_rx) =
        tokio::sync::mpsc::channel::<(u64, String, String, serde_json::Value)>(32);
    let hl_worker = Arc::clone(&highlighter_cmd);
    let net_tx_worker = net_client_cmd.clone();
    let session_gen_worker = Arc::clone(&session_generation);
    let armed_worker = Arc::clone(&is_automation_armed);

    tokio::spawn(async move {
        while let Some((gen, cmd_id, action_name, action_params)) = fifo_cmd_rx.recv().await {
            let current_gen = session_gen_worker.load(Ordering::SeqCst);
            if gen != current_gen {
                info!(
                    "[MAIN] ⏭️ Stale command [{}] from generation {} dropped (current: {})",
                    cmd_id, gen, current_gen
                );
                continue;
            }

            // Check armed guard for all physical inputs, app launching, lock screen, and system modifications (Audit N2)
            if automation::requires_arm(&action_name) && !armed_worker.load(Ordering::SeqCst) {
                warn!("[MAIN] 🛡️ Command [{}] rejected: Desktop automation is DISARMED", cmd_id);
                let disarm_msg = ClientMessage::CommandResult {
                    command_id: cmd_id,
                    success: false,
                    message: "Otomatisasi desktop sedang dinonaktifkan (Disarmed). Izinkan otomasi melalui UI atau System Tray sebelum mengeksekusi aksi.".to_string(),
                    data: serde_json::json!({ "error": "AUTOMATION_DISARMED" }),
                };
                let _ = net_tx_worker.send_message(disarm_msg).await;
                continue;
            }

            // Panic isolation: wrap execution in task so unexpected panics never kill the worker (Audit K9)
            let hl_task = Arc::clone(&hl_worker);
            let cmd_id_task = cmd_id.clone();
            let action_task = action_name.clone();
            let handle = tokio::spawn(async move {
                automation::handle_execute_command(
                    cmd_id_task,
                    action_task,
                    action_params,
                    Some(&hl_task),
                )
                .await
            });

            let result = match handle.await {
                Ok(res) => res,
                Err(join_err) => {
                    error!("[MAIN] 💥 Panic/Join error executing command [{}]: {}", cmd_id, join_err);
                    ClientMessage::CommandResult {
                        command_id: cmd_id,
                        success: false,
                        message: "Internal panic occurred during command execution".to_string(),
                        data: serde_json::json!({ "error": "INTERNAL_PANIC" }),
                    }
                }
            };

            if session_gen_worker.load(Ordering::SeqCst) == gen {
                let _ = net_tx_worker.send_message(result).await;
            }
        }
        info!("[MAIN] FIFO command executor loop terminated.");
    });

    tokio::spawn(async move {
        loop {
            let msg = match command_rx.recv().await {
                Ok(m) => m,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    warn!("[MAIN] Command queue lagged by {} messages; continuing dispatch loop", missed);
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("[MAIN] Command queue closed; terminating dispatch loop");
                    break;
                }
            };
            match msg {
                ServerMessage::AudioPlay { pcm_chunk } => {
                    use base64::prelude::*;
                    if let Ok(bytes) = BASE64_STANDARD.decode(&pcm_chunk) {
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
                    let is_speaker_playing_status = is_speaker_playing_cmd.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = window_weak.upgrade() {
                            let is_spk_active = is_speaker_playing_status
                                .as_ref()
                                .map(|h| h.load(Ordering::Relaxed))
                                .unwrap_or(false);

                            if let Some(cog) = maybe_cog {
                                if cog == ui::CognitiveState::Listening && is_spk_active {
                                    // Hardware speaker buffer still has active speech playing.
                                    // Preserve Speaking avatar state until physical playback finishes!
                                } else {
                                    win.set_avatar_state(cog.as_str().into());
                                    if !cog.is_vocal() {
                                        win.set_volume_level(0.0);
                                        win.set_pulse_scale(1.0);
                                    }
                                }
                            }
                            if let Some(ref ut) = user_transcript {
                                win.set_transcript_user_text(ut.clone().into());
                            }
                            if let Some(ref t) = text {
                                win.set_transcript_assistant_text(t.clone().into());
                                win.set_status_message(t.clone().into());
                            } else if let Some(cog) = maybe_cog {
                                if !(cog == ui::CognitiveState::Listening && is_spk_active) {
                                    win.set_status_message(cog.status_text().into());
                                }
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
                    info!(
                        "[MAIN] 🛠️ Executing command '{}' [{}] params: {}",
                        action,
                        command_id,
                        automation::redact_params(&action, &params)
                    );

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

                    // Fast-path for ABORT_AUTOMATION: bypass FIFO queue and immediately invalidate in-flight/queued commands (Audit N3)
                    let action_upper = action.trim().to_uppercase();
                    if matches!(action_upper.as_str(), "ABORT_AUTOMATION" | "ABORT" | "CANCEL_AUTOMATION") {
                        info!("[MAIN] 🚨 Fast-path ABORT_AUTOMATION [{}]! Preempting queue & requesting abort.", command_id);
                        automation::request_automation_abort();
                        let new_gen = session_gen_cmd.fetch_add(1, Ordering::SeqCst) + 1;
                        let hl = Arc::clone(&highlighter_cmd);
                        tokio::spawn(async move {
                            hl.clear_all();
                        });

                        let net_abort_tx = net_client_cmd.clone();
                        let cmd_id_owned = command_id.clone();
                        tokio::spawn(async move {
                            let abort_msg = ClientMessage::CommandResult {
                                command_id: cmd_id_owned,
                                success: true,
                                message: "Otomatisasi desktop berhasil dihentikan seketika (Fast-path Abort).".to_string(),
                                data: serde_json::json!({ "action": "abortAutomation", "new_epoch": new_gen }),
                            };
                            let _ = net_abort_tx.send_message(abort_msg).await;
                        });
                        continue;
                    }

                    // Enqueue to serial FIFO queue with session epoch non-blockingly (Audit H10, K2, K6)
                    let current_gen = session_gen_cmd.load(Ordering::SeqCst);
                    if let Err(_) = fifo_cmd_tx.try_send((current_gen, command_id.clone(), action, params)) {
                        warn!("[MAIN] FIFO command queue full! Dropping command [{}]", command_id);
                        let net_drop_tx = net_client_cmd.clone();
                        tokio::spawn(async move {
                            let drop_msg = ClientMessage::CommandResult {
                                command_id,
                                success: false,
                                message: "Antrean perintah bridge penuh (Queue full). Perintah dibatalkan.".to_string(),
                                data: serde_json::json!({ "error": "QUEUE_FULL" }),
                            };
                            let _ = net_drop_tx.send_message(drop_msg).await;
                        });
                    }
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
    let armed_tray = Arc::clone(&is_automation_armed);
    let _system_tray = SystemTray::spawn(move |event| {
        let window_weak = window_weak_tray.clone();
        let audio_engine = audio_engine_tray.clone();
        let toggle_share = Arc::clone(&toggle_share_tray);
        let armed = Arc::clone(&armed_tray);
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
                    TrayEvent::ToggleArmed => {
                        let now_armed = !armed.load(Ordering::SeqCst);
                        armed.store(now_armed, Ordering::SeqCst);
                        info!("[TRAY] 🛡️ Desktop automation armed state changed: {}", now_armed);
                        let status_text = if now_armed {
                            "🛡️ Otomasi Desktop: DIIZINKAN (Armed)"
                        } else {
                            "🛡️ Otomasi Desktop: DINONAKTIFKAN (Disarmed)"
                        };
                        win.set_status_message(status_text.into());
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

    // 11. Register Windows Global Hotkeys (Boss Key, Emergency Stop, Mic Mute, Fullscreen, Screen Share)
    let window_weak_hotkey = main_window.as_weak();
    let audio_engine_hotkey = audio_engine.clone();
    let toggle_share_hotkey = Arc::clone(&toggle_screen_share);
    let highlighter_hotkey = Arc::clone(&highlighter_controller);
    let armed_hotkey = Arc::clone(&is_automation_armed);
    let session_gen_hotkey = Arc::clone(&session_generation);
    let _hotkey_listener = HotkeyListener::spawn(move |action| {
        let window_weak = window_weak_hotkey.clone();
        let audio_engine = audio_engine_hotkey.clone();
        let toggle_share = Arc::clone(&toggle_share_hotkey);
        let highlighter = Arc::clone(&highlighter_hotkey);
        let armed = Arc::clone(&armed_hotkey);
        let session_gen = Arc::clone(&session_gen_hotkey);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(win) = window_weak.upgrade() {
                match action {
                    HotkeyAction::EmergencyStop => {
                        info!("[HOTKEY] 🚨 EMERGENCY STOP (Win+Alt+X) triggered! Disarming and silencing.");
                        automation::request_automation_abort();
                        session_gen.fetch_add(1, Ordering::SeqCst);
                        armed.store(false, Ordering::SeqCst);
                        highlighter.clear_all();
                        toggle_share(Some(false));
                        win.set_is_mic_muted(true);
                        win.set_status_message("🚨 EMERGENCY STOP: Otomasi Dihentikan & Disarmed.".into());
                        if let Some(audio_clone) = audio_engine.as_ref() {
                            let audio_clone = Arc::clone(audio_clone);
                            tokio::spawn(async move {
                                let engine = audio_clone.lock().await;
                                engine.set_mic_muted(true);
                            });
                        }
                    }
                    HotkeyAction::BossKey => {
                        let is_vis = win.window().is_visible();
                        let is_min = win.window().is_minimized();
                        info!("[HOTKEY] 🕶️ Boss Key (Win+Alt+N) triggered (visible={}, minimized={})", is_vis, is_min);
                        // Unconditionally abort automation and disarm regardless of window visibility (Audit N4)
                        automation::request_automation_abort();
                        session_gen.fetch_add(1, Ordering::SeqCst);
                        armed.store(false, Ordering::SeqCst);
                        highlighter.clear_all();

                        if is_min {
                            win.window().set_minimized(false);
                            let _ = win.window().show();
                        } else if is_vis {
                            let _ = win.window().hide();
                            toggle_share(Some(false));
                            win.set_is_mic_muted(true);
                            if let Some(audio_clone) = audio_engine.as_ref() {
                                let audio_clone = Arc::clone(audio_clone);
                                tokio::spawn(async move {
                                    let engine = audio_clone.lock().await;
                                    engine.set_mic_muted(true);
                                });
                            }
                        } else {
                            win.window().set_minimized(false);
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
                        unsafe {
                            let hwnd = get_main_window_hwnd();
                            enforce_system_cursor_visible(hwnd);
                        }
                        if fs {
                            let timer_win = window_weak.clone();
                            slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
                                if let Some(w) = timer_win.upgrade() {
                                    if w.get_is_fullscreen() {
                                        unsafe {
                                            let hwnd = get_main_window_hwnd();
                                            enforce_system_cursor_visible(hwnd);
                                        }
                                    }
                                }
                            });
                        } else {
                            win.window().set_maximized(false);
                            let timer_win = window_weak.clone();
                            slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
                                if let Some(_w) = timer_win.upgrade() {
                                    unsafe {
                                        let hwnd = get_main_window_hwnd();
                                        if !hwnd.is_null() {
                                            position_floating_pill_window(hwnd);
                                        }
                                    }
                                }
                            });
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
            let normalized_url = BridgeConfig::normalize_ws_url(&url);
            if let Err(err) = BridgeConfig::validate_ws_url(&normalized_url) {
                if let Some(win) = window_weak.upgrade() {
                    win.set_status_message(format!("⚠️ Pengaturan ditolak: {}", err).into());
                }
                return;
            }

            let mut cfg = config_clone.lock().unwrap();
            cfg.server_url = normalized_url;
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
                        win.set_cfg_server_url(cfg.server_url.clone().into());
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

        let normalized_url = BridgeConfig::normalize_ws_url(&url_str);
        if let Err(err) = BridgeConfig::validate_ws_url(&normalized_url) {
            if let Some(win) = window_weak.upgrade() {
                win.set_test_status_text(format!("❌ {}", err).into());
            }
            return;
        }

        if let Some(win) = window_weak.upgrade() {
            win.set_is_testing(true);
            win.set_test_status_text("Testing connection...".into());
        }

        tokio::spawn(async move {
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

            unsafe {
                let hwnd = get_main_window_hwnd();
                enforce_system_cursor_visible(hwnd);
            }

            if fs {
                let timer_win = window_weak.clone();
                slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
                    if let Some(w) = timer_win.upgrade() {
                        if w.get_is_fullscreen() {
                            unsafe {
                                let hwnd = get_main_window_hwnd();
                                enforce_system_cursor_visible(hwnd);
                            }
                        }
                    }
                });
            } else {
                win.window().set_maximized(false);
                let timer_win = window_weak.clone();
                slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
                    if let Some(_w) = timer_win.upgrade() {
                        unsafe {
                            let hwnd = get_main_window_hwnd();
                            if !hwnd.is_null() {
                                position_floating_pill_window(hwnd);
                            }
                        }
                    }
                });
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

    // Hook Native Window Drag (Direct Realtime Threaded Tracking Loop)
    main_window.on_start_window_drag(move || {
        std::thread::spawn(move || {
            unsafe {
                use windows_sys::Win32::Foundation::{POINT, RECT};
                use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                    GetAsyncKeyState, ReleaseCapture, VK_LBUTTON,
                };
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    GetCursorPos, GetWindowRect, SetWindowPos, SWP_NOSIZE, SWP_NOZORDER,
                };

                let hwnd = get_main_window_hwnd();
                if hwnd.is_null() {
                    return;
                }

                // Check that left mouse button is currently down
                if (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) == 0 {
                    return;
                }

                let mut start_cursor = POINT { x: 0, y: 0 };
                GetCursorPos(&mut start_cursor);

                let mut start_rect = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                GetWindowRect(hwnd, &mut start_rect);

                ReleaseCapture();

                // Smoothly update window position while left mouse button is held down
                while (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) != 0 {
                    let mut cur_cursor = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut cur_cursor);

                    let dx = cur_cursor.x - start_cursor.x;
                    let dy = cur_cursor.y - start_cursor.y;

                    if dx != 0 || dy != 0 {
                        let new_x = start_rect.left + dx;
                        let new_y = start_rect.top + dy;
                        SetWindowPos(
                            hwnd,
                            std::ptr::null_mut(),
                            new_x,
                            new_y,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOZORDER,
                        );
                    }

                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        });
    });

    // Hook Settings Modal Open (Option A: Smooth Dynamic Expansion)
    let window_weak = main_window.as_weak();
    main_window.on_open_settings(move || {
        if let Some(win) = window_weak.upgrade() {
            win.set_show_settings(true);
            if !win.get_is_fullscreen() {
                unsafe {
                    let hwnd = get_main_window_hwnd();
                    if !hwnd.is_null() {
                        position_settings_window(hwnd);
                    }
                }
            }
        }
    });

    // Hook Settings Modal Close (Restores to Floating Capsule Pill)
    let window_weak = main_window.as_weak();
    main_window.on_close_settings(move || {
        if let Some(win) = window_weak.upgrade() {
            win.set_show_settings(false);
            if !win.get_is_fullscreen() {
                let timer_win = window_weak.clone();
                slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
                    if let Some(_w) = timer_win.upgrade() {
                        unsafe {
                            let hwnd = get_main_window_hwnd();
                            if !hwnd.is_null() {
                                position_floating_pill_window(hwnd);
                            }
                        }
                    }
                });
            }
        }
    });

    // Window Close Button [X]: Terminate application cleanly, stopping all audio and network sessions
    let net_client_close_req = network_client.clone();
    let capture_ctrl_close_req = Arc::clone(&capture_controller);
    let is_capture_desired_close_req = Arc::clone(&is_capture_desired);
    let capture_gen_close_req = Arc::clone(&capture_generation);
    main_window.window().on_close_requested(move || {
        info!("[MAIN] Window close [X] requested. Terminating application gracefully.");
        is_capture_desired_close_req.store(false, Ordering::SeqCst);
        capture_gen_close_req.fetch_add(1, Ordering::SeqCst);
        net_client_close_req.disconnect();
        if let Ok(mut ctrl) = capture_ctrl_close_req.lock() {
            if let Some(mut c) = ctrl.take() {
                c.stop();
            }
        }
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });

    // Hook Mode 2 Window Minimize [-] Button
    let window_weak = main_window.as_weak();
    main_window.on_minimize_window(move || {
        info!("[MAIN] 🗕 Minimize requested. Minimizing window.");
        if let Some(win) = window_weak.upgrade() {
            win.window().set_minimized(true);
        }
    });

    // Hook Application Close [✕] Button (From Pill Mode or Fullscreen HUD)
    let net_client_close = network_client.clone();
    let capture_ctrl_close_app = Arc::clone(&capture_controller);
    let is_capture_desired_close_app = Arc::clone(&is_capture_desired);
    let capture_gen_close_app = Arc::clone(&capture_generation);
    main_window.on_close_app(move || {
        info!("[MAIN] ✕ Application Close requested by user. Disconnecting and quitting.");
        is_capture_desired_close_app.store(false, Ordering::SeqCst);
        capture_gen_close_app.fetch_add(1, Ordering::SeqCst);
        net_client_close.disconnect();
        if let Ok(mut ctrl) = capture_ctrl_close_app.lock() {
            if let Some(mut c) = ctrl.take() {
                c.stop();
            }
        }
        let _ = slint::quit_event_loop();
    });

    // Keep overlay window alive across application runtime
    let _overlay_guard = overlay_window;

    // Run the Slint GUI Event Loop until explicit quit (Claude Bug #2 & #3)
    if !start_minimized {
        main_window.show()?;
        let timer_win = main_window.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
            if let Some(win) = timer_win.upgrade() {
                unsafe {
                    let hwnd = get_main_window_hwnd();
                    if !hwnd.is_null() {
                        enforce_system_cursor_visible(hwnd);
                        if !win.get_is_fullscreen() {
                            position_floating_pill_window(hwnd);
                            info!("[MAIN] 📍 Positioned floating pill window at Top-Center (Tengah Atas)");
                        }
                    }
                }
            }
        });
    } else {
        info!("[MAIN] 🔕 Started in Auto-Start Standby mode (minimized to system tray)");
    }
    slint::run_event_loop_until_quit()?;

    info!("[MAIN] Application event loop ended. Performing clean and graceful shutdown (Audit K7, N9)...");
    automation::request_automation_abort();
    session_generation.fetch_add(1, Ordering::SeqCst);
    highlighter_controller.clear_all();
    network_client.disconnect();
    if let Some(a) = &audio_engine {
        if let Ok(mut engine) = tokio::time::timeout(std::time::Duration::from_millis(1500), a.lock()).await {
            engine.stop_duplex();
        } else {
            warn!("[MAIN] Audio engine lock acquisition timed out during shutdown; proceeding.");
        }
    }
    if let Ok(mut ctrl) = capture_controller.lock() {
        if let Some(mut c) = ctrl.take() {
            c.stop();
        }
    }
    drop(_system_tray);
    drop(_hotkey_listener);
    drop(_power_listener);
    drop(_overlay_guard);
    drop(_log_guard);

    info!("[MAIN] Graceful shutdown complete. Exiting cleanly.");
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

/// Enforces visible mouse cursor across Windows OS, DWM, and HWND class level.
/// 1. Increments ShowCursor display counter until it is >= 0.
/// 2. Explicitly loads and sets the IDC_ARROW system cursor immediately.
/// 3. Updates GCLP_HCURSOR on the window class so DefWindowProcW defaults to IDC_ARROW.
unsafe fn enforce_system_cursor_visible(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        LoadCursorW, SetClassLongPtrW, SetCursor, ShowCursor, GCLP_HCURSOR, IDC_ARROW,
    };

    while ShowCursor(1) < 0 {}

    let arrow = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
    if !arrow.is_null() {
        SetCursor(arrow);
        if !hwnd.is_null() {
            SetClassLongPtrW(hwnd, GCLP_HCURSOR, arrow as isize);
        }
    }
}

/// Helper to reliably resolve the HWND of the Nexa Bridge main window.
/// Tries the active title "Nexa Bridge", then fallback "N.E.X.A Windows Bridge",
/// and finally enumerates visible windows owned by the current process.
unsafe fn get_main_window_hwnd() -> windows_sys::Win32::Foundation::HWND {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowW, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
    };

    let title1: Vec<u16> = "Nexa Bridge\0".encode_utf16().collect();
    let hwnd = FindWindowW(std::ptr::null(), title1.as_ptr());
    if !hwnd.is_null() {
        return hwnd;
    }

    let title2: Vec<u16> = "N.E.X.A Windows Bridge\0".encode_utf16().collect();
    let hwnd = FindWindowW(std::ptr::null(), title2.as_ptr());
    if !hwnd.is_null() {
        return hwnd;
    }

    struct EnumCtx {
        pid: u32,
        found: HWND,
    }

    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam as *mut EnumCtx);
        let mut window_pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut window_pid);
        if window_pid == ctx.pid && IsWindowVisible(hwnd) != 0 {
            let mut class_buf = [0u16; 64];
            let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 64);
            let class_name = String::from_utf16_lossy(&class_buf[..class_len as usize]);
            if !class_name.contains("Event Target") && !class_name.contains("IME") {
                ctx.found = hwnd;
                return 0; // stop enumeration
            }
        }
        1 // continue
    }

    let mut ctx = EnumCtx {
        pid: GetCurrentProcessId(),
        found: std::ptr::null_mut(),
    };

    EnumWindows(Some(enum_cb), &mut ctx as *mut _ as LPARAM);
    ctx.found
}

/// Helper to configure the pill window geometry and position (top-center, DPI-scaled, non-activating)
unsafe fn position_floating_pill_window(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SetWindowPos, HWND_TOPMOST, SM_CXSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    };

    let dpi = GetDpiForWindow(hwnd);
    let scale = if dpi > 0 { dpi as f64 / 96.0 } else { 1.0 };

    // Logical dimensions for the floating pill window: 460px x 64px
    let logical_w = 460.0;
    let logical_h = 64.0;
    let pill_w = (logical_w * scale).round() as i32;
    let pill_h = (logical_h * scale).round() as i32;

    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let pos_x = (screen_w - pill_w) / 2;
    let pos_y = (16.0 * scale).round() as i32;

    // Use SWP_SHOWWINDOW | SWP_NOACTIVATE so positioning pill never steals keyboard focus from active apps (Audit K8)
    SetWindowPos(
        hwnd,
        HWND_TOPMOST,
        pos_x,
        pos_y,
        pill_w,
        pill_h,
        SWP_SHOWWINDOW | SWP_NOACTIVATE,
    );
}

/// Helper to configure the settings dialog window geometry and position (DPI-scaled)
unsafe fn position_settings_window(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SetWindowPos, HWND_TOPMOST, SM_CXSCREEN, SWP_SHOWWINDOW,
    };

    let dpi = GetDpiForWindow(hwnd);
    let scale = if dpi > 0 { dpi as f64 / 96.0 } else { 1.0 };

    let logical_w = 560.0;
    let logical_h = 540.0;
    let w = (logical_w * scale).round() as i32;
    let h = (logical_h * scale).round() as i32;

    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let pos_x = (screen_w - w) / 2;
    let pos_y = (60.0 * scale).round() as i32;

    SetWindowPos(hwnd, HWND_TOPMOST, pos_x, pos_y, w, h, SWP_SHOWWINDOW);
}
