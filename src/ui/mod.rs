// ============================================================
// N.E.X.A 3.3 — WINDOWS BRIDGE UI MODULE
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
            _ => Self::Offline,
        }
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
        assert_eq!(CognitiveState::from_str_loose("unknown_value"), CognitiveState::Offline);
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
}
