//! Subsistem UI Automation native (port filosofi Microsoft UFO ke Rust).
//!
//! Peta modul:
//! - `types` : DTO murni (Send + 'static), enum error, laporan hasil
//! - `snap`  : algoritma Smart Snap (fungsi murni, 100% unit-testable) + filter teks ala UFO
//! - `guard` : Window Guard (verifikasi foreground / PID / responsif) — tanpa COM
//! - `scan`  : kode COM (IUIAutomation): filter kontrol interaktif + eksekusi semantik
//! - `actor` : thread MTA khusus + handle async + circuit breaker
//!
//! Aturan emas: objek COM TIDAK PERNAH meninggalkan thread aktor. Yang menyeberang
//! thread hanya DTO biasa.

pub mod actor;
pub mod guard;
mod scan;
pub mod snap;
pub mod types;

#[allow(unused_imports)]
pub use actor::{degraded_click, ClickPolicy, UiaHandle};
#[allow(unused_imports)]
pub use types::{
    ClickMethod, ClickReport, ControlInfo, ControlKind, ControlSummary, SetTextReport, UiaError,
};
