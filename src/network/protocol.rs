// ============================================================
// WINDOWS BRIDGE NETWORK PROTOCOL
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

    #[serde(rename = "CALL_EVENT")]
    CallEvent {
        event: String,
        #[serde(default)]
        command_id: String,
    },

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

    #[serde(rename = "CALL_AUDIO_INTERRUPTED")]
    AudioInterrupted,
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

    #[serde(rename = "CALL_AUDIO_PLAY")]
    AudioPlay {
        pcm_chunk: String,
    },

    #[serde(rename = "CALL_STATUS_UPDATE")]
    StatusUpdate {
        #[serde(default)]
        status: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        user_transcript: Option<String>,
    },

    #[serde(rename = "CALL_AUDIO_INTERRUPTED")]
    AudioInterrupted,

    #[serde(rename = "CALL_REPLY_COMPLETE")]
    ReplyComplete,

    #[serde(rename = "CALL_LIVE_READY")]
    LiveReady {
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        model: Option<String>,
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
    fn test_server_message_audio_play_and_status() {
        let play_json = r#"{"type": "CALL_AUDIO_PLAY", "pcm_chunk": "AQID"}"#;
        let play_msg = ServerMessage::from_json(play_json).unwrap();
        assert_eq!(
            play_msg,
            ServerMessage::AudioPlay {
                pcm_chunk: "AQID".to_string()
            }
        );

        let status_json = r#"{"type": "CALL_STATUS_UPDATE", "status": "SPEAKING", "text": "Halo Tuan"}"#;
        let status_msg = ServerMessage::from_json(status_json).unwrap();
        match status_msg {
            ServerMessage::StatusUpdate { status, text, .. } => {
                assert_eq!(status, "SPEAKING");
                assert_eq!(text.unwrap(), "Halo Tuan");
            }
            _ => panic!("Expected StatusUpdate"),
        }
    }

    #[test]
    fn test_call_event_and_live_ready() {
        let call_event = ClientMessage::CallEvent {
            event: "CALL_ACCEPTED".to_string(),
            command_id: "win_123".to_string(),
        };
        let json = call_event.to_json().unwrap();
        assert!(json.contains("\"type\":\"CALL_EVENT\""));
        assert!(json.contains("\"event\":\"CALL_ACCEPTED\""));

        let ready_json = r#"{"type": "CALL_LIVE_READY", "sessionId": "DESKTOP_123", "model": "gemini-3.8-live"}"#;
        let ready_msg = ServerMessage::from_json(ready_json).unwrap();
        match ready_msg {
            ServerMessage::LiveReady { .. } => {}
            _ => panic!("Expected LiveReady"),
        }
    }

    #[test]
    fn test_server_message_unknown_fallback() {
        let json = r#"{"type": "SOME_NEW_SERVER_FEATURE", "data": 123}"#;
        let msg = ServerMessage::from_json(json).unwrap();
        assert_eq!(msg, ServerMessage::Unknown);
    }
}
