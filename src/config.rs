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
}

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
            if let Ok(content) = fs::read_to_string(&bridge_path) {
                if let Ok(cfg) = serde_json::from_str::<BridgeConfig>(&content) {
                    tracing::info!("Loaded config from {}", bridge_path.display());
                    return cfg;
                }
            }
        }

        let legacy_path = Self::legacy_config_path();
        if legacy_path.exists() {
            if let Ok(content) = fs::read_to_string(&legacy_path) {
                if let Ok(legacy) = serde_json::from_str::<LegacyConfig>(&content) {
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
            }
        }

        Self::default()
    }

    /// Save configuration to ~/.nexa-bridge.json
    pub fn save(&self) -> Result<(), std::io::Error> {
        let path = Self::config_path();
        let content = serde_json::to_string_pretty(self)
            .map_err(std::io::Error::other)?;
        fs::write(&path, content)?;
        tracing::info!("Saved config to {}", path.display());
        Ok(())
    }

    /// Normalize server URL into a WebSocket endpoint
    pub fn normalize_ws_url(url: &str) -> String {
        let mut u = url.trim().to_string();
        if u.starts_with("https://") {
            u = u.replacen("https://", "wss://", 1);
        } else if u.starts_with("http://") {
            u = u.replacen("http://", "ws://", 1);
        } else if !u.starts_with("ws://") && !u.starts_with("wss://") {
            u = format!("ws://{}", u);
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
    }

    #[test]
    fn test_partial_json_deserialization_with_defaults() {
        let json = r#"{"server_url": "ws://example.com/ws"}"#;
        let cfg: BridgeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.server_url, "ws://example.com/ws");
        assert!(cfg.low_ram_mode);
        assert_eq!(cfg.capture_fps, 1);
        assert_eq!(cfg.audio_input_device, "Default");
        assert_eq!(cfg.auth_token, "");
    }
}
