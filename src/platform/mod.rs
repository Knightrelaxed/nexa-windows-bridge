// ============================================================
// WINDOWS BRIDGE PLATFORM SUBSYSTEM
// Native Windows Integrations (Power Events, System Tray, Hotkeys)
// ============================================================

pub mod hotkeys;
pub mod power;
pub mod system_tray;
pub mod telemetry;

pub use hotkeys::{HotkeyAction, HotkeyListener};
pub use power::{PowerListener, PowerStateEvent};
pub use system_tray::{SystemTray, TrayEvent};
pub use telemetry::SystemTelemetry;
