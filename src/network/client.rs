// ============================================================
// WINDOWS BRIDGE WEBSOCKET CLIENT
// Resilient Async Client with Exponential Backoff & Keepalive
// ============================================================

use super::protocol::{ClientMessage, ServerMessage};
use crate::config::BridgeConfig;
use futures_util::{FutureExt, SinkExt, StreamExt};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch, Notify};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting { attempt: u32, next_retry_ms: u64 },
    Suspended,
    AuthRejected,
}

#[derive(Debug, Clone)]
pub enum OutboundItem {
    Text(String),
    #[allow(dead_code)]
    Binary(Vec<u8>),
}

#[derive(Clone)]
pub struct NetworkClient {
    #[allow(dead_code)]
    config: Arc<Mutex<BridgeConfig>>,
    state_tx: Arc<watch::Sender<ConnectionState>>,
    outbound_tx: mpsc::Sender<OutboundItem>,
    command_tx: broadcast::Sender<ServerMessage>,
    audio_tx: broadcast::Sender<Vec<u8>>,
    reconnect_notify: Arc<Notify>,
    session_interrupt: Arc<Notify>,
    enabled: Arc<Mutex<bool>>,
    is_suspended: Arc<Mutex<bool>>,
    last_rtt_ms: Arc<AtomicU32>,
}

impl NetworkClient {
    pub fn new(config: Arc<Mutex<BridgeConfig>>) -> Self {
        let (state_tx, _) = watch::channel(ConnectionState::Disconnected);
        let (outbound_tx, outbound_rx) = mpsc::channel::<OutboundItem>(128);
        let (command_tx, _) = broadcast::channel::<ServerMessage>(512);
        let (audio_tx, _) = broadcast::channel::<Vec<u8>>(512);
        let reconnect_notify = Arc::new(Notify::new());
        let session_interrupt = Arc::new(Notify::new());
        let enabled = Arc::new(Mutex::new(false));
        let is_suspended = Arc::new(Mutex::new(false));
        let last_rtt_ms = Arc::new(AtomicU32::new(0));

        let client = Self {
            config: Arc::clone(&config),
            state_tx: Arc::new(state_tx),
            outbound_tx,
            command_tx: command_tx.clone(),
            audio_tx: audio_tx.clone(),
            reconnect_notify: Arc::clone(&reconnect_notify),
            session_interrupt: Arc::clone(&session_interrupt),
            enabled: Arc::clone(&enabled),
            is_suspended: Arc::clone(&is_suspended),
            last_rtt_ms: Arc::clone(&last_rtt_ms),
        };

        // Spawn background connection lifecycle worker
        tokio::spawn(run_client_worker(
            config,
            Arc::clone(&client.state_tx),
            outbound_rx,
            command_tx,
            audio_tx,
            reconnect_notify,
            session_interrupt,
            enabled,
            is_suspended,
            Arc::clone(&client.last_rtt_ms),
        ));

        client
    }

    /// Retrieve the most recent measured round-trip time in milliseconds
    pub fn get_last_rtt_ms(&self) -> u32 {
        self.last_rtt_ms.load(Ordering::Relaxed)
    }

    /// Subscribe to real-time connection state updates
    pub fn subscribe_state(&self) -> watch::Receiver<ConnectionState> {
        self.state_tx.subscribe()
    }

    /// Subscribe to incoming server control commands (isolated from audio traffic, Audit N6)
    pub fn subscribe_commands(&self) -> broadcast::Receiver<ServerMessage> {
        self.command_tx.subscribe()
    }

    /// Subscribe to incoming streaming raw PCM audio frames (Audit N6)
    pub fn subscribe_audio(&self) -> broadcast::Receiver<Vec<u8>> {
        self.audio_tx.subscribe()
    }

    /// Send a structured protocol message to the server
    pub async fn send_message(&self, msg: ClientMessage) -> Result<(), String> {
        let json = msg.to_json().map_err(|e| e.to_string())?;
        self.outbound_tx
            .send(OutboundItem::Text(json))
            .await
            .map_err(|e| format!("Failed to queue message: {}", e))
    }

    /// Send raw binary PCM audio frame to the server
    #[allow(dead_code)]
    pub async fn send_binary(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.outbound_tx
            .send(OutboundItem::Binary(bytes))
            .await
            .map_err(|e| format!("Failed to queue binary data: {}", e))
    }

    /// Enable connection worker
    pub fn connect(&self) {
        {
            let mut en = self.enabled.lock().unwrap();
            *en = true;
        }
        self.reconnect_notify.notify_one();
    }

    /// Disable and disconnect
    pub fn disconnect(&self) {
        {
            let mut en = self.enabled.lock().unwrap();
            *en = false;
        }
        self.session_interrupt.notify_one();
        let _ = self.state_tx.send(ConnectionState::Disconnected);
    }

    /// Notify client of Windows OS Sleep (WM_POWERBROADCAST PBT_APMSUSPEND)
    pub fn notify_suspend(&self) {
        info!("[NETWORK] OS Suspend signal received. Entering suspended state...");
        {
            let mut susp = self.is_suspended.lock().unwrap();
            *susp = true;
        }
        self.session_interrupt.notify_one();
        let _ = self.state_tx.send(ConnectionState::Suspended);
    }

    /// Notify client of Windows OS Wake (WM_POWERBROADCAST PBT_APMRESUMEAUTOMATIC)
    pub fn notify_resume(&self) {
        info!("[NETWORK] OS Resume signal received. Triggering instant reconnect...");
        {
            let mut susp = self.is_suspended.lock().unwrap();
            *susp = false;
        }
        self.reconnect_notify.notify_one();
    }

    /// Check if client is currently in connected state
    #[allow(dead_code)]
    pub fn is_connected(&self) -> bool {
        *self.state_tx.borrow() == ConnectionState::Connected
    }

    /// Check if client connection worker is enabled (connecting, connected, or reconnecting)
    pub fn is_enabled(&self) -> bool {
        *self.enabled.lock().unwrap()
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_client_worker(
    config: Arc<Mutex<BridgeConfig>>,
    state_tx: Arc<watch::Sender<ConnectionState>>,
    mut outbound_rx: mpsc::Receiver<OutboundItem>,
    command_tx: broadcast::Sender<ServerMessage>,
    audio_tx: broadcast::Sender<Vec<u8>>,
    reconnect_notify: Arc<Notify>,
    session_interrupt: Arc<Notify>,
    enabled: Arc<Mutex<bool>>,
    is_suspended: Arc<Mutex<bool>>,
    last_rtt_ms: Arc<AtomicU32>,
) {
    let mut attempt = 0;
    let mut backoff_ms = 500;
    const MAX_BACKOFF_MS: u64 = 10000;

    loop {
        // 1. Wait if connection is not enabled or suspended
        while !*enabled.lock().unwrap() || *is_suspended.lock().unwrap() {
            attempt = 0;
            backoff_ms = 500;
            last_rtt_ms.store(0, Ordering::Relaxed);
            reconnect_notify.notified().await;
        }

        let (url, token) = {
            let cfg = config.lock().unwrap();
            (cfg.websocket_url(), cfg.auth_token.clone())
        };

        let _ = state_tx.send(ConnectionState::Connecting);
        info!("[NETWORK] Connecting to: {}", url);

        let handshake_req = match build_handshake_request(&url, &token) {
            Ok(req) => req,
            Err(e) => {
                error!("[NETWORK] Invalid request: {}", e);
                tokio::time::sleep(Duration::from_millis(2000)).await;
                continue;
            }
        };

        // Drain any stale session interrupt permits before each connection attempt (Claude Bug H1)
        while session_interrupt.notified().now_or_never().is_some() {}

        // 2. Perform WebSocket handshake with 10s timeout & instant cancellation (Claude Bug H3)
        let connect_future = tokio_tungstenite::connect_async(handshake_req);
        let connect_res = tokio::select! {
            res = tokio::time::timeout(Duration::from_secs(10), connect_future) => Some(res),
            _ = session_interrupt.notified() => None,
        };

        match connect_res {
            None => {
                info!("[NETWORK] Handshake cancelled by user disconnect or OS suspend.");
                continue;
            }
            Some(Err(_)) => {
                warn!("[NETWORK] Handshake timed out after 10s.");
            }
            Some(Ok(Err(tokio_tungstenite::tungstenite::Error::Http(resp))))
                if matches!(resp.status().as_u16(), 401 | 403) =>
            {
                error!(
                    "[NETWORK] ⛔ Token authentication rejected (HTTP {}). Stopping retries.",
                    resp.status()
                );
                *enabled.lock().unwrap() = false;
                let _ = state_tx.send(ConnectionState::AuthRejected);
                continue;
            }
            Some(Ok(Err(e))) => {
                warn!("[NETWORK] Connection attempt failed: {}", e);
            }
            Some(Ok(Ok((ws_stream, response)))) => {
                info!(
                    "[NETWORK] ✅ Connected successfully! HTTP Status: {:?}",
                    response.status()
                );
                attempt = 0;
                backoff_ms = 500;
                let _ = state_tx.send(ConnectionState::Connected);

                let (mut ws_write, mut ws_read) = ws_stream.split();
                let mut ping_interval = tokio::time::interval(Duration::from_secs(5));
                ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                // Drain any stale outbound items from previous sessions
                while outbound_rx.try_recv().is_ok() {}

                let mut ping_sent_instant: Option<std::time::Instant> = None;

                // Send immediate initial ping to measure initial RTT right away
                {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    if let Ok(json) = (ClientMessage::Ping { timestamp: now }).to_json() {
                        ping_sent_instant = Some(std::time::Instant::now());
                        let _ = ws_write.send(Message::Text(json.into())).await;
                    }
                }

                // 3. Active Session Loop
                let mut session_active = true;
                let mut last_rx = tokio::time::Instant::now();
                while session_active {
                    if !*enabled.lock().unwrap() || *is_suspended.lock().unwrap() {
                        info!("[NETWORK] Session interrupted by user disconnect or OS suspend.");
                        let _ = ws_write.send(Message::Close(None)).await;
                        break;
                    }

                    tokio::select! {
                        // Instant session interrupt (triggered on disconnect or OS suspend)
                        _ = session_interrupt.notified() => {
                            info!("[NETWORK] Active session interrupted by disconnect or suspend.");
                            let _ = ws_write.send(Message::Close(None)).await;
                            break;
                        }

                        // Keepalive Watchdog (Sends PING every 5s and checks 15s half-open timeout, Claude Bug H2)
                        _ = ping_interval.tick() => {
                            if last_rx.elapsed() > Duration::from_secs(15) {
                                warn!("[NETWORK] Keepalive timeout: no server packet received for >15s. Session half-open, reconnecting.");
                                session_active = false;
                                continue;
                            }

                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64;
                            let ping_msg = ClientMessage::Ping { timestamp: now };
                            if let Ok(json) = ping_msg.to_json() {
                                ping_sent_instant = Some(std::time::Instant::now());
                                if let Err(e) = ws_write.send(Message::Text(json.into())).await {
                                    warn!("[NETWORK] Failed to send ping: {}", e);
                                    session_active = false;
                                }
                            }
                        }

                        // Flush outbound queue (text commands & audio frames)
                        Some(item) = outbound_rx.recv() => {
                            let res = match item {
                                OutboundItem::Text(txt) => ws_write.send(Message::Text(txt.into())).await,
                                OutboundItem::Binary(bin) => ws_write.send(Message::Binary(bin.into())).await,
                            };
                            if let Err(e) = res {
                                warn!("[NETWORK] Outbound send failed: {}", e);
                                session_active = false;
                            }
                        }

                        // Read incoming packets from server
                        msg_opt = ws_read.next() => {
                            if let Some(Ok(_)) = &msg_opt {
                                last_rx = tokio::time::Instant::now();
                            }
                            match msg_opt {
                                Some(Ok(Message::Text(text))) => {
                                    match ServerMessage::from_json(&text) {
                                        Ok(ServerMessage::Pong { timestamp: _ }) => {
                                            // Measure true RTT from local stopwatch, immune to server clock drift (Audit H9)
                                            if let Some(sent) = ping_sent_instant.take() {
                                                let rtt = sent.elapsed().as_millis() as u32;
                                                last_rtt_ms.store(rtt, Ordering::Relaxed);
                                            }
                                        }
                                        Ok(ServerMessage::AudioPlay { pcm_chunk }) => {
                                            // Decode streaming speech audio to dedicated audio queue; do NOT flood command queue (Audit N6)
                                            use base64::prelude::*;
                                            if let Ok(bytes) = BASE64_STANDARD.decode(&pcm_chunk) {
                                                let _ = audio_tx.send(bytes);
                                            }
                                        }
                                        Ok(ServerMessage::Unknown) => {
                                            // Silently ignore future extension types
                                        }
                                        Ok(msg) => {
                                            let _ = command_tx.send(msg);
                                        }
                                        Err(e) => {
                                            warn!("[NETWORK] Failed to parse server message: {}", e);
                                        }
                                    }
                                }
                                Some(Ok(Message::Binary(bin))) => {
                                    // Direct streaming audio delivery without base64 overhead (Audit N6)
                                    let _ = audio_tx.send(bin.to_vec());
                                }
                                Some(Ok(Message::Ping(data))) => {
                                    let _ = ws_write.send(Message::Pong(data)).await;
                                }
                                Some(Ok(Message::Close(frame))) => {
                                    info!("[NETWORK] Server sent close frame: {:?}", frame);
                                    session_active = false;
                                }
                                Some(Err(e)) => {
                                    warn!("[NETWORK] Socket read error: {}", e);
                                    session_active = false;
                                }
                                None => {
                                    info!("[NETWORK] WebSocket stream terminated.");
                                    session_active = false;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        // 4. Handle reconnection and exponential backoff
        if *enabled.lock().unwrap() && !*is_suspended.lock().unwrap() {
            attempt += 1;
            let _ = state_tx.send(ConnectionState::Reconnecting {
                attempt,
                next_retry_ms: backoff_ms,
            });
            info!(
                "[NETWORK] Reconnecting in {}ms (Attempt #{})...",
                backoff_ms, attempt
            );

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                _ = reconnect_notify.notified() => {
                    info!("[NETWORK] Reconnect timer preempted by instant trigger.");
                    attempt = 0;
                    backoff_ms = 500;
                }
                _ = session_interrupt.notified() => {
                    info!("[NETWORK] Reconnect backoff preempted by interrupt.");
                }
            }

            backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
        } else if *is_suspended.lock().unwrap() {
            let _ = state_tx.send(ConnectionState::Suspended);
        } else {
            let _ = state_tx.send(ConnectionState::Disconnected);
        }
    }
}

fn build_handshake_request(
    url: &str,
    token: &str,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
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

    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_handshake_request_headers() {
        let req = build_handshake_request("ws://127.0.0.1:3000/ws", "secret123").unwrap();
        assert_eq!(
            req.headers().get("Authorization").unwrap(),
            "Bearer secret123"
        );
        assert_eq!(req.headers().get("X-Device-Type").unwrap(), "windows");
    }

    #[tokio::test]
    async fn test_network_client_lifecycle_and_power_states() {
        let cfg = Arc::new(Mutex::new(BridgeConfig::default()));
        let client = NetworkClient::new(cfg);

        let state_rx = client.subscribe_state();
        assert_eq!(*state_rx.borrow(), ConnectionState::Disconnected);
        assert!(!client.is_enabled());

        // Connect
        client.connect();
        assert!(client.is_enabled());

        // Suspend
        client.notify_suspend();
        assert_eq!(*state_rx.borrow(), ConnectionState::Suspended);

        // Resume
        client.notify_resume();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Disconnect
        client.disconnect();
        assert_eq!(*state_rx.borrow(), ConnectionState::Disconnected);
        assert!(!client.is_enabled());
    }
}
