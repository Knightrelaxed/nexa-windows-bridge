// ============================================================
// N.E.X.A 3.3 — WINDOWS BRIDGE NETWORK PROTOCOL
// Strongly-Typed Messages for Client-Server WebSocket Exchange
// ============================================================

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "PING")]
    Ping { timestamp: u64 },

    #[serde(rename = "COMMAND_RESULT")]
    CommandResult {
        command_id: String,
        success: bool,
        message: String,
        #[serde(default)]
        data: serde_json::Value,
    },

    #[serde(rename = "DESKTOP_TELEMETRY")]
    Telemetry {
        #[serde(default)]
        cpu_usage: f32,
        #[serde(default)]
        ram_mb: u64,
        #[serde(default)]
        active_window: String,
    },

    #[serde(rename = "POWER_EVENT")]
    PowerEvent { event: String },

    #[serde(rename = "CALL_VIDEO_FRAME")]
    VideoFrame {
        command_id: String,
        image_chunk: String,
    },

    #[serde(rename = "CALL_AUDIO_STREAM")]
    AudioStream {
        command_id: String,
        pcm_chunk: String,
    },
}

impl ClientMessage {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "PONG")]
    Pong {
        #[serde(default)]
        timestamp: u64,
    },

    #[serde(rename = "EXECUTE_COMMAND")]
    ExecuteCommand {
        command_id: String,
        action: String,
        #[serde(default)]
        params: serde_json::Value,
        #[serde(default)]
        timestamp: u64,
    },

    #[serde(other)]
    Unknown,
}

impl ServerMessage {
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_message_ping_serialization() {
        let ping = ClientMessage::Ping {
            timestamp: 1787105000,
        };
        let json = ping.to_json().unwrap();
        assert!(json.contains("\"type\":\"PING\""));
        assert!(json.contains("\"timestamp\":1787105000"));
    }

    #[test]
    fn test_client_message_command_result_serialization() {
        let res = ClientMessage::CommandResult {
            command_id: "cmd_123".to_string(),
            success: true,
            message: "Click completed".to_string(),
            data: serde_json::json!({ "x": 500, "y": 500 }),
        };
        let json = res.to_json().unwrap();
        assert!(json.contains("\"type\":\"COMMAND_RESULT\""));
        assert!(json.contains("\"command_id\":\"cmd_123\""));
        assert!(json.contains("\"success\":true"));
    }

    #[test]
    fn test_server_message_execute_command_deserialization() {
        let json = r#"{
            "type": "EXECUTE_COMMAND",
            "command_id": "win_12345",
            "action": "DESKTOP_CLICK",
            "params": { "x": 100, "y": 200, "button": "LEFT" },
            "timestamp": 1787106000
        }"#;

        let msg = ServerMessage::from_json(json).unwrap();
        match msg {
            ServerMessage::ExecuteCommand {
                command_id,
                action,
                params,
                timestamp,
            } => {
                assert_eq!(command_id, "win_12345");
                assert_eq!(action, "DESKTOP_CLICK");
                assert_eq!(params["x"], 100);
                assert_eq!(params["button"], "LEFT");
                assert_eq!(timestamp, 1787106000);
            }
            _ => panic!("Expected ExecuteCommand"),
        }
    }

    #[test]
    fn test_server_message_pong_deserialization() {
        let json = r#"{"type": "PONG", "timestamp": 1787107000}"#;
        let msg = ServerMessage::from_json(json).unwrap();
        assert_eq!(msg, ServerMessage::Pong { timestamp: 1787107000 });
    }

    #[test]
    fn test_server_message_unknown_fallback() {
        let json = r#"{"type": "SOME_NEW_SERVER_FEATURE", "data": 123}"#;
        let msg = ServerMessage::from_json(json).unwrap();
        assert_eq!(msg, ServerMessage::Unknown);
    }
}
