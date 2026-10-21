// ============================================================
// WINDOWS BRIDGE UI MODULE
// Slint Desktop Companion Interface & Event Controllers
// ============================================================

// Slint code generation imports
slint::include_modules!();

/// 9 Cognitive States for the Sentient Cybernetic Avatar
/// Perfectly aligned with Mobile Bridge and N.E.X.A Server.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CognitiveState {
    Idle,
    Listening,
    Thinking,
    Speaking,
    BargeIn,
    ScreenSharing,
    ToolExecuting,
    Connecting,
    Offline,
}

#[allow(dead_code)]
impl CognitiveState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Listening => "listening",
            Self::Thinking => "thinking",
            Self::Speaking => "speaking",
            Self::BargeIn => "barge_in",
            Self::ScreenSharing => "screen_sharing",
            Self::ToolExecuting => "tool_executing",
            Self::Connecting => "connecting",
            Self::Offline => "offline",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "idle" | "ready" => Self::Idle,
            "listening" => Self::Listening,
            "thinking" | "processing" => Self::Thinking,
            "speaking" => Self::Speaking,
            "barge_in" | "interrupted" => Self::BargeIn,
            "screen_sharing" | "vision" => Self::ScreenSharing,
            "tool_executing" | "executing" => Self::ToolExecuting,
            "connecting" | "reconnecting" => Self::Connecting,
            "offline" | "disconnecting" => Self::Offline,
            _ => Self::Idle,
        }
    }

    /// Parse status string into an optional CognitiveState.
    /// Returns None if the string is empty or whitespace-only (e.g. transcript-only updates),
    /// preventing spurious offline avatar state transitions mid-turn.
    pub fn try_from_status(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self::from_str_loose(trimmed))
    }

    pub fn hex_color(&self) -> &'static str {
        match self {
            Self::Idle => "#00E5FF",          // Soft Cyan
            Self::Listening => "#00B4D8",     // Vibrant Cyan-Blue
            Self::Thinking => "#FFB300",      // Cognitive Gold
            Self::Speaking => "#00FF88",      // Cyber Emerald
            Self::BargeIn => "#E0F7FA",       // Flash White-Cyan
            Self::ScreenSharing => "#00B0FF", // Holographic Azure
            Self::ToolExecuting => "#7C4DFF", // Quantum Violet
            Self::Connecting => "#FF9100",    // Amber Warning
            Self::Offline => "#37474F",       // Deep Slate Gray
        }
    }

    pub fn status_text(&self) -> &'static str {
        match self {
            Self::Listening => "Mendengarkan Tuan Faqih...",
            Self::Thinking => "Sedang menalar & memproses...",
            Self::Speaking => "N.E.X.A sedang berbicara...",
            Self::BargeIn => "Mendengarkan interupsi Tuan...",
            Self::ScreenSharing => "Mentransmisikan layar & audio (D3D11 1 FPS)...",
            Self::ToolExecuting => "Mengeksekusi perintah sistem...",
            Self::Connecting => "Menghubungkan kembali ke server...",
            Self::Idle => "Online & Siaga",
            Self::Offline => "Bridge Terputus (Offline)",
        }
    }

    pub fn is_orbital_rotating(&self) -> bool {
        matches!(self, Self::Thinking | Self::Connecting)
    }

    pub fn is_vocal(&self) -> bool {
        matches!(self, Self::Speaking | Self::Listening)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_main_window_ui_properties_and_dual_mode() {
        let main_window = MainWindow::new().expect("Failed to initialize MainWindow");

        // 1. Verify Avatar state & pulse scale
        main_window.set_avatar_state("thinking".into());
        assert_eq!(main_window.get_avatar_state().as_str(), "thinking");

        main_window.set_pulse_scale(1.25);
        assert_eq!(main_window.get_pulse_scale(), 1.25);

        // 2. Verify initial defaults (Windowed Mode)
        assert!(!main_window.get_is_fullscreen());
        assert!(!main_window.get_is_mic_muted());
        assert!(!main_window.get_is_screen_sharing());

        // 3. Toggle to Fullscreen Mode
        main_window.set_is_fullscreen(true);
        assert!(main_window.get_is_fullscreen());

        // 4. Toggle Mic Mute
        main_window.set_is_mic_muted(true);
        assert!(main_window.get_is_mic_muted());

        // 5. Toggle Screen Share
        main_window.set_is_screen_sharing(true);
        assert!(main_window.get_is_screen_sharing());

        // 6. Verify Telemetry HUD properties
        assert_eq!(main_window.get_telemetry_cpu().as_str(), "12%");
        assert_eq!(main_window.get_telemetry_ram().as_str(), "34%");
        assert_eq!(main_window.get_telemetry_audio().as_str(), "WASAPI 48kHz (AEC3)");
        assert_eq!(main_window.get_telemetry_ping().as_str(), "24ms");

        // 7. Update Telemetry values
        main_window.set_telemetry_cpu("18%".into());
        assert_eq!(main_window.get_telemetry_cpu().as_str(), "18%");

        // 8. Verify AudioMeter Volume level property
        assert_eq!(main_window.get_volume_level(), 0.0);
        main_window.set_volume_level(0.75);
        assert_eq!(main_window.get_volume_level(), 0.75);

        // 9. Verify Settings Modal visibility toggle
        assert!(!main_window.get_show_settings());
        main_window.set_show_settings(true);
        assert!(main_window.get_show_settings());

        // 10. Verify Configuration Bindings
        main_window.set_cfg_server_url("ws://localhost:3000/ws".into());
        assert_eq!(main_window.get_cfg_server_url().as_str(), "ws://localhost:3000/ws");
        main_window.set_cfg_auth_token("secret_token_123".into());
        assert_eq!(main_window.get_cfg_auth_token().as_str(), "secret_token_123");
        main_window.set_cfg_low_ram_mode(true);
        assert!(main_window.get_cfg_low_ram_mode());

        // 11. Verify Autostart Binding
        assert!(!main_window.get_cfg_autostart());
        main_window.set_cfg_autostart(true);
        assert!(main_window.get_cfg_autostart());

        // 12. Verify Status Message Binding
        main_window.set_status_message("N.E.X.A Connected".into());
        assert_eq!(main_window.get_status_message().as_str(), "N.E.X.A Connected");
    }

    #[test]
    fn test_cognitive_state_nine_spectrum_colors() {
        assert_eq!(CognitiveState::Idle.as_str(), "idle");
        assert_eq!(CognitiveState::Idle.hex_color(), "#00E5FF");
        assert_eq!(CognitiveState::Listening.as_str(), "listening");
        assert_eq!(CognitiveState::Listening.hex_color(), "#00B4D8");
        assert_eq!(CognitiveState::Thinking.as_str(), "thinking");
        assert_eq!(CognitiveState::Thinking.hex_color(), "#FFB300");
        assert_eq!(CognitiveState::Speaking.as_str(), "speaking");
        assert_eq!(CognitiveState::Speaking.hex_color(), "#00FF88");
        assert_eq!(CognitiveState::BargeIn.as_str(), "barge_in");
        assert_eq!(CognitiveState::BargeIn.hex_color(), "#E0F7FA");
        assert_eq!(CognitiveState::ScreenSharing.as_str(), "screen_sharing");
        assert_eq!(CognitiveState::ScreenSharing.hex_color(), "#00B0FF");
        assert_eq!(CognitiveState::ToolExecuting.as_str(), "tool_executing");
        assert_eq!(CognitiveState::ToolExecuting.hex_color(), "#7C4DFF");
        assert_eq!(CognitiveState::Connecting.as_str(), "connecting");
        assert_eq!(CognitiveState::Connecting.hex_color(), "#FF9100");
        assert_eq!(CognitiveState::Offline.as_str(), "offline");
        assert_eq!(CognitiveState::Offline.hex_color(), "#37474F");
    }

    #[test]
    fn test_cognitive_state_from_str_loose() {
        assert_eq!(CognitiveState::from_str_loose("idle"), CognitiveState::Idle);
        assert_eq!(CognitiveState::from_str_loose("READY"), CognitiveState::Idle);
        assert_eq!(CognitiveState::from_str_loose("listening"), CognitiveState::Listening);
        assert_eq!(CognitiveState::from_str_loose("processing"), CognitiveState::Thinking);
        assert_eq!(CognitiveState::from_str_loose("THINKING"), CognitiveState::Thinking);
        assert_eq!(CognitiveState::from_str_loose("speaking"), CognitiveState::Speaking);
        assert_eq!(CognitiveState::from_str_loose("interrupted"), CognitiveState::BargeIn);
        assert_eq!(CognitiveState::from_str_loose("vision"), CognitiveState::ScreenSharing);
        assert_eq!(CognitiveState::from_str_loose("executing"), CognitiveState::ToolExecuting);
        assert_eq!(CognitiveState::from_str_loose("reconnecting"), CognitiveState::Connecting);
        assert_eq!(CognitiveState::from_str_loose("offline"), CognitiveState::Offline);
        assert_eq!(CognitiveState::from_str_loose("disconnecting"), CognitiveState::Offline);
        assert_eq!(CognitiveState::from_str_loose("unknown_value"), CognitiveState::Idle);
    }

    #[test]
    fn test_empty_transcript_does_not_trigger_offline_state() {
        assert_eq!(CognitiveState::try_from_status(""), None);
        assert_eq!(CognitiveState::try_from_status("   "), None);
        assert_eq!(CognitiveState::try_from_status("speaking"), Some(CognitiveState::Speaking));
        assert_eq!(CognitiveState::try_from_status("LISTENING"), Some(CognitiveState::Listening));
    }

    #[test]
    fn test_cognitive_state_status_text_mobile_parity() {
        assert_eq!(CognitiveState::Listening.status_text(), "Mendengarkan Tuan Faqih...");
        assert_eq!(CognitiveState::Thinking.status_text(), "Sedang menalar & memproses...");
        assert_eq!(CognitiveState::Speaking.status_text(), "N.E.X.A sedang berbicara...");
        assert_eq!(CognitiveState::BargeIn.status_text(), "Mendengarkan interupsi Tuan...");
        assert_eq!(CognitiveState::ScreenSharing.status_text(), "Mentransmisikan layar & audio (D3D11 1 FPS)...");
        assert_eq!(CognitiveState::ToolExecuting.status_text(), "Mengeksekusi perintah sistem...");
        assert_eq!(CognitiveState::Connecting.status_text(), "Menghubungkan kembali ke server...");
        assert_eq!(CognitiveState::Idle.status_text(), "Online & Siaga");
        assert_eq!(CognitiveState::Offline.status_text(), "Bridge Terputus (Offline)");
    }

    #[test]
    fn test_cognitive_state_orbital_rotation_flag() {
        assert!(CognitiveState::Thinking.is_orbital_rotating());
        assert!(CognitiveState::Connecting.is_orbital_rotating());
        assert!(!CognitiveState::Idle.is_orbital_rotating());
        assert!(!CognitiveState::Speaking.is_orbital_rotating());
        assert!(!CognitiveState::Offline.is_orbital_rotating());
    }

    #[test]
    fn test_cognitive_state_is_vocal() {
        assert!(CognitiveState::Speaking.is_vocal());
        assert!(CognitiveState::Listening.is_vocal());
        assert!(!CognitiveState::Thinking.is_vocal());
        assert!(!CognitiveState::Idle.is_vocal());
        assert!(!CognitiveState::BargeIn.is_vocal());
        assert!(!CognitiveState::Offline.is_vocal());
        assert!(!CognitiveState::Connecting.is_vocal());
        assert!(!CognitiveState::ScreenSharing.is_vocal());
        assert!(!CognitiveState::ToolExecuting.is_vocal());
    }
}
