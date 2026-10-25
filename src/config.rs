use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BridgeConfig {
    pub server_url: String,
    pub auth_token: String,
    pub audio_input_device: String,
    pub audio_output_device: String,
    pub capture_fps: u32,
    pub low_ram_mode: bool,
    pub auto_connect: bool,
    pub autostart: bool,
    pub show_card_vision: bool,
    pub show_card_automation: bool,
    pub show_card_transcript: bool,
    pub show_card_telemetry: bool,
    pub show_card_hotkeys: bool,
    #[serde(default = "default_split_left_width")]
    pub split_left_width: f32,
    #[serde(default = "default_split_right_width")]
    pub split_right_width: f32,
    #[serde(default = "default_split_left_h")]
    pub split_left_h: f32,
    #[serde(default = "default_split_center_h")]
    pub split_center_h: f32,
    #[serde(default = "default_split_right_h")]
    pub split_right_h: f32,
}

fn default_split_left_width() -> f32 { 280.0 }
fn default_split_right_width() -> f32 { 300.0 }
fn default_split_left_h() -> f32 { 240.0 }
fn default_split_center_h() -> f32 { 440.0 }
fn default_split_right_h() -> f32 { 260.0 }

#[derive(Debug, Deserialize)]
struct LegacyConfig {
    #[serde(rename = "serverUrl")]
    server_url: Option<String>,
    secret: Option<String>,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            server_url: "ws://127.0.0.1:3000/ws".to_string(),
            auth_token: String::new(),
            audio_input_device: "Default".to_string(),
            audio_output_device: "Default".to_string(),
            capture_fps: 1,
            low_ram_mode: true,
            auto_connect: false,
            autostart: false,
            show_card_vision: true,
            show_card_automation: true,
            show_card_transcript: true,
            show_card_telemetry: true,
            show_card_hotkeys: true,
            split_left_width: default_split_left_width(),
            split_right_width: default_split_right_width(),
            split_left_h: default_split_left_h(),
            split_center_h: default_split_center_h(),
            split_right_h: default_split_right_h(),
        }
    }
}

impl BridgeConfig {
    /// Standard config file path: ~/.nexa-bridge.json
    pub fn config_path() -> PathBuf {
        if let Some(home) = dirs::home_dir() {
            home.join(".nexa-bridge.json")
        } else {
            PathBuf::from(".nexa-bridge.json")
        }
    }

    /// Legacy config file path: ~/.nexa-config.json
    pub fn legacy_config_path() -> PathBuf {
        if let Some(home) = dirs::home_dir() {
            home.join(".nexa-config.json")
        } else {
            PathBuf::from(".nexa-config.json")
        }
    }

    /// Load config from ~/.nexa-bridge.json, falling back to ~/.nexa-config.json or defaults.
    pub fn load() -> Self {
        let bridge_path = Self::config_path();
        if bridge_path.exists() {
            match fs::read_to_string(&bridge_path) {
                Ok(content) => match serde_json::from_str::<BridgeConfig>(&content) {
                    Ok(cfg) => {
                        tracing::info!("Loaded config from {}", bridge_path.display());
                        return cfg;
                    }
                    Err(e) => {
                        tracing::error!(
                            "Gagal mem-parse file konfigurasi di {}: {}. Menggunakan fallback.",
                            bridge_path.display(),
                            e
                        );
                    }
                },
                Err(e) => {
                    tracing::error!(
                        "Gagal membaca file konfigurasi di {}: {}. Menggunakan fallback.",
                        bridge_path.display(),
                        e
                    );
                }
            }
        }

        let legacy_path = Self::legacy_config_path();
        if legacy_path.exists() {
            match fs::read_to_string(&legacy_path) {
                Ok(content) => match serde_json::from_str::<LegacyConfig>(&content) {
                    Ok(legacy) => {
                        tracing::info!("Found legacy config at {}", legacy_path.display());
                        let mut cfg = Self::default();
                        if let Some(url) = legacy.server_url {
                            cfg.server_url = Self::normalize_ws_url(&url);
                        }
                        if let Some(secret) = legacy.secret {
                            cfg.auth_token = secret;
                        }
                        // Save as new config
                        let _ = cfg.save();
                        return cfg;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Gagal mem-parse legacy config di {}: {}",
                            legacy_path.display(),
                            e
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        "Gagal membaca legacy config di {}: {}",
                        legacy_path.display(),
                        e
                    );
                }
            }
        }

        Self::default()
    }

    /// Save configuration atomically to ~/.nexa-bridge.json using a tempfile + atomic rename (Security Audit N5)
    pub fn save(&self) -> Result<(), std::io::Error> {
        let path = Self::config_path();
        let tmp_path = path.with_extension("tmp");
        let content = serde_json::to_string_pretty(self)
            .map_err(std::io::Error::other)?;
        fs::write(&tmp_path, content)?;
        fs::rename(&tmp_path, &path)?;
        tracing::info!("Saved config atomically to {}", path.display());
        Ok(())
    }

    /// Check whether a host is a local loopback address (127.0.0.1, localhost, ::1)
    pub fn is_local_host(url: &str) -> bool {
        let u = url.trim().to_lowercase();
        let stripped = u
            .strip_prefix("wss://")
            .or_else(|| u.strip_prefix("ws://"))
            .or_else(|| u.strip_prefix("https://"))
            .or_else(|| u.strip_prefix("http://"))
            .unwrap_or(&u);

        stripped.starts_with("127.0.0.1")
            || stripped.starts_with("localhost")
            || stripped.starts_with("[::1]")
            || stripped.starts_with("::1")
    }

    /// Enforce TLS for all non-local connections (Security Audit N5).
    /// Non-local hosts MUST use wss:// to prevent cleartext token and audio transmission over the internet.
    pub fn validate_ws_url(url: &str) -> Result<(), String> {
        let u = url.trim();
        if u.is_empty() {
            return Err("URL server WebSocket tidak boleh kosong.".to_string());
        }

        if u.starts_with("wss://") || Self::is_local_host(u) {
            Ok(())
        } else if u.starts_with("ws://") {
            Err("Protokol tidak aman: ws:// hanya diizinkan untuk host lokal (localhost / 127.0.0.1). Gunakan wss:// untuk server jarak jauh/produksi.".to_string())
        } else {
            // Missing scheme will default to wss:// for non-local via normalize_ws_url
            Ok(())
        }
    }

    /// Normalize server URL into a WebSocket endpoint.
    /// Non-local endpoints without scheme automatically default to secure wss:// (Audit N5).
    pub fn normalize_ws_url(url: &str) -> String {
        let mut u = url.trim().to_string();
        if u.starts_with("https://") {
            u = u.replacen("https://", "wss://", 1);
        } else if u.starts_with("http://") {
            if Self::is_local_host(&u) {
                u = u.replacen("http://", "ws://", 1);
            } else {
                u = u.replacen("http://", "wss://", 1);
            }
        } else if !u.starts_with("ws://") && !u.starts_with("wss://") {
            if Self::is_local_host(&u) {
                u = format!("ws://{}", u);
            } else {
                u = format!("wss://{}", u);
            }
        }

        if !u.ends_with("/ws") {
            if u.ends_with('/') {
                u.push_str("ws");
            } else {
                u.push_str("/ws");
            }
        }
        u
    }

    #[allow(dead_code)]
    pub fn websocket_url(&self) -> String {
        Self::normalize_ws_url(&self.server_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_ws_url() {
        assert_eq!(
            BridgeConfig::normalize_ws_url("https://nexa-server.azure.com"),
            "wss://nexa-server.azure.com/ws"
        );
        assert_eq!(
            BridgeConfig::normalize_ws_url("http://localhost:3000"),
            "ws://localhost:3000/ws"
        );
        assert_eq!(
            BridgeConfig::normalize_ws_url("wss://custom.domain/ws"),
            "wss://custom.domain/ws"
        );
        // Non-local missing scheme defaults to wss://
        assert_eq!(
            BridgeConfig::normalize_ws_url("nexa.example.com"),
            "wss://nexa.example.com/ws"
        );
        // Local missing scheme defaults to ws://
        assert_eq!(
            BridgeConfig::normalize_ws_url("127.0.0.1:3000"),
            "ws://127.0.0.1:3000/ws"
        );
    }

    #[test]
    fn test_validate_ws_url_tls_enforcement() {
        // Localhost allowed on ws://
        assert!(BridgeConfig::validate_ws_url("ws://localhost:3000/ws").is_ok());
        assert!(BridgeConfig::validate_ws_url("ws://127.0.0.1:3000/ws").is_ok());
        assert!(BridgeConfig::validate_ws_url("ws://[::1]:3000/ws").is_ok());

        // Secure wss:// allowed everywhere
        assert!(BridgeConfig::validate_ws_url("wss://server.nexa.ai/ws").is_ok());

        // Insecure ws:// rejected for remote host
        assert!(BridgeConfig::validate_ws_url("ws://nexa.remote-server.com/ws").is_err());
        assert!(BridgeConfig::validate_ws_url("ws://192.168.1.50:3000/ws").is_err());
    }

    #[test]
    fn test_partial_json_deserialization_with_defaults() {
        let json = r#"{"server_url": "ws://127.0.0.1:3000/ws"}"#;
        let cfg: BridgeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.server_url, "ws://127.0.0.1:3000/ws");
        assert!(cfg.low_ram_mode);
        assert_eq!(cfg.capture_fps, 1);
        assert_eq!(cfg.audio_input_device, "Default");
        assert_eq!(cfg.auth_token, "");
    }
}
