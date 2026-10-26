//! Automation permission and action policy enforcement (Security Audit N2, R2)

/// Canonicalizes an action name by trimming, converting to uppercase,
/// and stripping all underscores, hyphens, and whitespace (Audit R2).
pub fn canonical_action(action: &str) -> String {
    action
        .trim()
        .to_uppercase()
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .collect()
}

/// Returns true if the action is an abort/cancellation action (Audit R2).
pub fn is_abort_action(action: &str) -> bool {
    matches!(
        canonical_action(action).as_str(),
        "ABORTAUTOMATION" | "ABORT" | "CANCELAUTOMATION" | "CANCEL" | "STOPAUTOMATION"
    )
}

/// Returns true if the given action requires desktop automation to be armed.
/// Read-only, status, highlight, and cancellation actions are allowed when disarmed.
/// All physical inputs, app launching, lock screen, and system modifications require explicit arming.
pub fn requires_arm(action: &str) -> bool {
    !matches!(
        canonical_action(action).as_str(),
        "ABORTAUTOMATION"
            | "ABORT"
            | "CANCELAUTOMATION"
            | "CANCEL"
            | "STOPAUTOMATION"
            | "CLEARHIGHLIGHTS"
            | "CLEARHIGHLIGHT"
            | "HIGHLIGHTELEMENT"
            | "HIGHLIGHT"
            | "POINTCURSOR"
            | "GETBATTERYSTATUS"
            | "BATTERYSTATUS"
            | "SYSTEMMETRICS"
            | "GETSYSTEMMETRICS"
            | "TELEMETRY"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_action_and_abort() {
        assert_eq!(canonical_action("highlightElement"), "HIGHLIGHTELEMENT");
        assert_eq!(canonical_action("HIGHLIGHT_ELEMENT"), "HIGHLIGHTELEMENT");
        assert_eq!(canonical_action("abort-automation"), "ABORTAUTOMATION");
        assert_eq!(canonical_action("  ABORT AUTOMATION  "), "ABORTAUTOMATION");

        assert!(is_abort_action("abortAutomation"));
        assert!(is_abort_action("ABORT_AUTOMATION"));
        assert!(is_abort_action("abort-automation"));
        assert!(is_abort_action("ABORT"));
        assert!(is_abort_action("cancelAutomation"));
        assert!(!is_abort_action("desktopClick"));
    }

    #[test]
    fn test_requires_arm_policy_comprehensive_cases() {
        // Safe actions in ALL casing styles (snake_case, camelCase, kebab-case, uppercase)
        assert!(!requires_arm("ABORT_AUTOMATION"));
        assert!(!requires_arm("abortAutomation"));
        assert!(!requires_arm("abort-automation"));
        assert!(!requires_arm("abort"));
        assert!(!requires_arm("CANCEL_AUTOMATION"));
        assert!(!requires_arm("cancelAutomation"));
        assert!(!requires_arm("CLEAR_HIGHLIGHTS"));
        assert!(!requires_arm("clearHighlights"));
        assert!(!requires_arm("HIGHLIGHT_ELEMENT"));
        assert!(!requires_arm("highlightElement"));
        assert!(!requires_arm("highlight-element"));
        assert!(!requires_arm("POINT_CURSOR"));
        assert!(!requires_arm("pointCursor"));
        assert!(!requires_arm("GET_BATTERY_STATUS"));
        assert!(!requires_arm("getBatteryStatus"));
        assert!(!requires_arm("system_metrics"));
        assert!(!requires_arm("systemMetrics"));
        assert!(!requires_arm("getSystemMetrics"));
        assert!(!requires_arm("TELEMETRY"));

        // Dangerous actions in ALL casing styles strictly require arming
        assert!(requires_arm("DESKTOP_CLICK"));
        assert!(requires_arm("desktopClick"));
        assert!(requires_arm("desktop_click"));
        assert!(requires_arm("DESKTOP_TYPE_TEXT"));
        assert!(requires_arm("desktopTypeText"));
        assert!(requires_arm("DESKTOP_PRESS_KEY"));
        assert!(requires_arm("desktopPressKey"));
        assert!(requires_arm("DESKTOP_SCROLL"));
        assert!(requires_arm("desktopScroll"));
        assert!(requires_arm("LAUNCH_APP"));
        assert!(requires_arm("launchApp"));
        assert!(requires_arm("LOCK_SCREEN"));
        assert!(requires_arm("lockScreen"));
        assert!(requires_arm("SHOW_DESKTOP"));
        assert!(requires_arm("showDesktop"));
        assert!(requires_arm("SET_VOLUME"));
        assert!(requires_arm("setVolume"));
        assert!(requires_arm("MUTE"));
        assert!(requires_arm("mute"));
        assert!(requires_arm("UNMUTE"));
        assert!(requires_arm("unmute"));
        assert!(requires_arm("TAKE_SCREENSHOT"));
        assert!(requires_arm("takeScreenshot"));
    }
}
