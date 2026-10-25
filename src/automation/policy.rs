//! Automation permission and action policy enforcement (Security Audit N2)

/// Returns true if the given action requires desktop automation to be armed.
/// Read-only, status, and cancellation actions are allowed when disarmed.
/// All physical inputs, app launching, lock screen, and system modifications require explicit arming.
pub fn requires_arm(action: &str) -> bool {
    let clean = action.trim().to_uppercase();
    !matches!(
        clean.as_str(),
        "ABORT_AUTOMATION"
            | "ABORT"
            | "CANCEL_AUTOMATION"
            | "CLEAR_HIGHLIGHTS"
            | "HIGHLIGHT_ELEMENT"
            | "POINT_CURSOR"
            | "GET_BATTERY_STATUS"
            | "SYSTEM_METRICS"
            | "TELEMETRY"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_requires_arm_policy() {
        // Safe actions that do not require arming
        assert!(!requires_arm("ABORT_AUTOMATION"));
        assert!(!requires_arm("abort"));
        assert!(!requires_arm("CANCEL_AUTOMATION"));
        assert!(!requires_arm("CLEAR_HIGHLIGHTS"));
        assert!(!requires_arm("HIGHLIGHT_ELEMENT"));
        assert!(!requires_arm("POINT_CURSOR"));
        assert!(!requires_arm("GET_BATTERY_STATUS"));
        assert!(!requires_arm("system_metrics"));
        assert!(!requires_arm("TELEMETRY"));

        // Dangerous actions that strictly require arming
        assert!(requires_arm("DESKTOP_CLICK"));
        assert!(requires_arm("desktop_click"));
        assert!(requires_arm("DESKTOP_TYPE_TEXT"));
        assert!(requires_arm("DESKTOP_PRESS_KEY"));
        assert!(requires_arm("DESKTOP_SCROLL"));
        assert!(requires_arm("LAUNCH_APP"));
        assert!(requires_arm("launch_app"));
        assert!(requires_arm("LOCK_SCREEN"));
        assert!(requires_arm("SHOW_DESKTOP"));
        assert!(requires_arm("SET_VOLUME"));
        assert!(requires_arm("MUTE"));
        assert!(requires_arm("UNMUTE"));
        assert!(requires_arm("TAKE_SCREENSHOT"));
    }
}
