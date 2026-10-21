// ============================================================
// WINDOWS BRIDGE PLATFORM SUBSYSTEM
// Native Windows Integrations (Power Events, System Tray, Hotkeys)
// ============================================================

pub mod autostart;
pub mod hotkeys;
pub mod memory;
pub mod power;
pub mod single_instance;
pub mod system_tray;
pub mod telemetry;

pub use autostart::{is_autostart_enabled, set_autostart_enabled};
pub use hotkeys::{HotkeyAction, HotkeyListener};
pub use memory::{compact_memory_if_low_ram, trim_working_set};
pub use power::{PowerListener, PowerStateEvent};
pub use single_instance::SingleInstanceGuard;
pub use system_tray::{SystemTray, TrayEvent};
pub use telemetry::SystemTelemetry;
