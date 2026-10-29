//! Tipe data murni subsistem UIA (tanpa COM/Win32 → `Send + 'static`, mudah di-unit-test).

use super::guard::GuardError;
use crate::automation::element_finder::PixelRect;
use std::fmt;

/// Jenis kontrol interaktif (padanan `CONTROL_LIST` pada konfigurasi UFO).
/// `Document`, `Pane`, `Group`, `Text`, `Image` sengaja TIDAK dimasukkan:
/// itu kontainer/dekorasi yang menyebabkan snap ke elemen raksasa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlKind {
    Button,
    SplitButton,
    CheckBox,
    RadioButton,
    ComboBox,
    Edit,
    Hyperlink,
    MenuItem,
    TabItem,
    ListItem,
    TreeItem,
    DataItem,
}

impl ControlKind {
    pub const DEFAULT_INTERACTIVE: [ControlKind; 12] = [
        ControlKind::Button,
        ControlKind::SplitButton,
        ControlKind::CheckBox,
        ControlKind::RadioButton,
        ControlKind::ComboBox,
        ControlKind::Edit,
        ControlKind::Hyperlink,
        ControlKind::MenuItem,
        ControlKind::TabItem,
        ControlKind::ListItem,
        ControlKind::TreeItem,
        ControlKind::DataItem,
    ];

    /// Nilai `UIA_*ControlTypeId` (didefinisikan lokal agar modul ini bebas dependensi Win32).
    pub const fn uia_id(self) -> i32 {
        match self {
            ControlKind::Button => 50000,
            ControlKind::CheckBox => 50002,
            ControlKind::ComboBox => 50003,
            ControlKind::Edit => 50004,
            ControlKind::Hyperlink => 50005,
            ControlKind::ListItem => 50007,
            ControlKind::MenuItem => 50011,
            ControlKind::RadioButton => 50013,
            ControlKind::TabItem => 50019,
            ControlKind::TreeItem => 50024,
            ControlKind::DataItem => 50029,
            ControlKind::SplitButton => 50031,
        }
    }

    pub fn from_uia_id(id: i32) -> Option<Self> {
        Self::DEFAULT_INTERACTIVE
            .iter()
            .copied()
            .find(|k| k.uia_id() == id)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ControlKind::Button => "Button",
            ControlKind::SplitButton => "SplitButton",
            ControlKind::CheckBox => "CheckBox",
            ControlKind::RadioButton => "RadioButton",
            ControlKind::ComboBox => "ComboBox",
            ControlKind::Edit => "Edit",
            ControlKind::Hyperlink => "Hyperlink",
            ControlKind::MenuItem => "MenuItem",
            ControlKind::TabItem => "TabItem",
            ControlKind::ListItem => "ListItem",
            ControlKind::TreeItem => "TreeItem",
            ControlKind::DataItem => "DataItem",
        }
    }
}

/// Snapshot sebuah kontrol (hasil cache UIA). Tidak memegang objek COM.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlInfo {
    pub kind: ControlKind,
    pub name: String,
    pub automation_id: String,
    pub rect: PixelRect,
    pub pid: u32,
}

impl ControlInfo {
    /// Luas dalam piksel; aman dari overflow walau UIA mengembalikan rect sampah.
    pub fn area(&self) -> i64 {
        let w = (self.rect.right as i64 - self.rect.left as i64).max(0);
        let h = (self.rect.bottom as i64 - self.rect.top as i64).max(0);
        w.saturating_mul(h)
    }

    pub fn center(&self) -> (i32, i32) {
        let cx = self.rect.left as i64 + (self.rect.right as i64 - self.rect.left as i64) / 2;
        let cy = self.rect.top as i64 + (self.rect.bottom as i64 - self.rect.top as i64) / 2;
        (
            cx.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            cy.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        )
    }

    pub fn short_name(&self, max_chars: usize) -> String {
        self.name.chars().take(max_chars).collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Laporan hasil
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickMethod {
    /// Pattern UIA berhasil (mis. "invoke", "toggle", "select", "expand")
    Semantic(&'static str),
    /// Pattern UIA timeout: aksi MUNGKIN sudah terjadi. Jangan di-retry / di-fallback.
    SemanticUnconfirmed(&'static str),
    /// SendInput di pusat kontrol hasil snap
    PhysicalSnapped,
    /// SendInput di koordinat mentah dari model (tidak ada kontrol dalam radius)
    PhysicalRaw,
}

impl ClickMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            ClickMethod::Semantic(_) => "semantic",
            ClickMethod::SemanticUnconfirmed(_) => "semantic_unconfirmed",
            ClickMethod::PhysicalSnapped => "physical_snapped",
            ClickMethod::PhysicalRaw => "physical_raw",
        }
    }

    pub fn pattern(&self) -> Option<&'static str> {
        match self {
            ClickMethod::Semantic(p) | ClickMethod::SemanticUnconfirmed(p) => Some(p),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControlSummary {
    pub kind: ControlKind,
    pub name: String,
    pub rect: PixelRect,
}

#[derive(Debug, Clone)]
pub struct ClickReport {
    pub method: ClickMethod,
    pub px: i32,
    pub py: i32,
    pub snapped: bool,
    pub control: Option<ControlSummary>,
    pub elapsed_ms: u64,
}

impl ClickReport {
    /// Kunci lama (`pixel_x`, `pixel_y`, ...) dipertahankan agar backend Nexa tidak rusak.
    pub fn to_json(&self, norm_x: f64, norm_y: f64) -> serde_json::Value {
        serde_json::json!({
            "action": "desktopClick",
            "x": norm_x,
            "y": norm_y,
            "pixel_x": self.px,
            "pixel_y": self.py,
            "method": self.method.as_str(),
            "pattern": self.method.pattern(),
            "confirmed": !matches!(self.method, ClickMethod::SemanticUnconfirmed(_)),
            "snapped": self.snapped,
            "control": self.control.as_ref().map(|c| serde_json::json!({
                "kind": c.kind.as_str(),
                "name": c.name,
                "rect": [c.rect.left, c.rect.top, c.rect.right, c.rect.bottom],
            })),
            "elapsed_ms": self.elapsed_ms,
        })
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SetTextReport {
    pub method: &'static str,
    pub chars: usize,
    pub elapsed_ms: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Error
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiaError {
    Unavailable(String),
    Busy,
    CircuitOpen,
    /// Batas waktu terlewati. Hasil aksi TIDAK terkonfirmasi.
    Timeout(&'static str),
    Cancelled,
    TargetNotResponding,
    StaleElement,
    NotEnabled,
    ElevatedTarget,
    ProviderUnavailable(i32),
    NoTarget,
    Guard(GuardError),
    InputFailed(String),
    Com(i32),
    Internal(String),
}

impl From<GuardError> for UiaError {
    fn from(g: GuardError) -> Self {
        match g {
            GuardError::NotResponding => UiaError::TargetNotResponding,
            other => UiaError::Guard(other),
        }
    }
}

impl UiaError {
    /// Kode stabil untuk `data.error` pada `CommandResult`.
    pub fn code(&self) -> &'static str {
        match self {
            UiaError::Unavailable(_) => "UIA_UNAVAILABLE",
            UiaError::Busy => "UIA_BUSY",
            UiaError::CircuitOpen => "UIA_CIRCUIT_OPEN",
            UiaError::Timeout(_) => "UIA_TIMEOUT",
            UiaError::Cancelled => "CANCELLED",
            UiaError::TargetNotResponding => "TARGET_NOT_RESPONDING",
            UiaError::StaleElement => "STALE_ELEMENT",
            UiaError::NotEnabled => "ELEMENT_DISABLED",
            UiaError::ElevatedTarget => "ELEVATED_TARGET",
            UiaError::ProviderUnavailable(_) => "PROVIDER_UNAVAILABLE",
            UiaError::NoTarget => "NO_TARGET",
            UiaError::Guard(g) => g.code(),
            UiaError::InputFailed(_) => "INPUT_FAILED",
            UiaError::Com(_) => "COM_ERROR",
            UiaError::Internal(_) => "INTERNAL",
        }
    }

    /// Aman di-retry satu kali setelah scan ulang (hanya untuk operasi idempoten / sebelum aksi).
    pub fn is_transient(&self) -> bool {
        matches!(self, UiaError::StaleElement | UiaError::ProviderUnavailable(_))
    }

    /// Konteks UIA (IUIAutomation + cache) sebaiknya dibangun ulang.
    pub fn invalidates_context(&self) -> bool {
        matches!(
            self,
            UiaError::ProviderUnavailable(_) | UiaError::Internal(_) | UiaError::Unavailable(_)
        )
    }

    /// Job belum sempat berjalan → aman untuk degradasi ke jalur lain tanpa risiko aksi ganda.
    pub fn job_never_started(&self) -> bool {
        matches!(
            self,
            UiaError::Unavailable(_) | UiaError::Busy | UiaError::CircuitOpen
        )
    }
}

impl fmt::Display for UiaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UiaError::Unavailable(s) => write!(f, "UI Automation tidak tersedia: {s}"),
            UiaError::Busy => write!(f, "Aktor UI Automation sedang sibuk"),
            UiaError::CircuitOpen => write!(
                f,
                "UI Automation dinonaktifkan sementara setelah beberapa timeout beruntun"
            ),
            UiaError::Timeout(p) => write!(
                f,
                "UI Automation melewati batas waktu ({p}); hasil aksi tidak terkonfirmasi, periksa layar"
            ),
            UiaError::Cancelled => write!(f, "Operasi dibatalkan"),
            UiaError::TargetNotResponding => {
                write!(f, "Aplikasi target tidak merespons (Not Responding)")
            }
            UiaError::StaleElement => write!(f, "Elemen UI sudah berubah atau hilang (stale)"),
            UiaError::NotEnabled => write!(f, "Elemen target sedang dinonaktifkan (disabled)"),
            UiaError::ElevatedTarget => write!(
                f,
                "Aplikasi target berjalan sebagai Administrator; akses UIA/input ditolak (UIPI)"
            ),
            UiaError::ProviderUnavailable(hr) => write!(
                f,
                "Provider UIA aplikasi target tidak merespons (HRESULT 0x{:08X})",
                *hr as u32
            ),
            UiaError::NoTarget => write!(f, "Tidak ada kontrol interaktif di sekitar titik tersebut"),
            UiaError::Guard(g) => write!(f, "Window Guard menolak aksi: {}", g.code()),
            UiaError::InputFailed(s) => write!(f, "Input fisik gagal: {s}"),
            UiaError::Com(hr) => write!(f, "Kesalahan COM (HRESULT 0x{:08X})", *hr as u32),
            UiaError::Internal(s) => write!(f, "Kesalahan internal: {s}"),
        }
    }
}

impl std::error::Error for UiaError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_kind_roundtrip() {
        for k in ControlKind::DEFAULT_INTERACTIVE {
            assert_eq!(ControlKind::from_uia_id(k.uia_id()), Some(k));
        }
        assert_eq!(ControlKind::from_uia_id(50033), None); // Pane
        assert_eq!(ControlKind::from_uia_id(50030), None); // Document
    }

    #[test]
    fn extreme_rect_does_not_overflow() {
        let c = ControlInfo {
            kind: ControlKind::Button,
            name: "x".into(),
            automation_id: String::new(),
            rect: PixelRect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
            pid: 1,
        };
        assert!(c.area() > 0);
        let _ = c.center();
    }

    #[test]
    fn error_codes_and_flags() {
        assert_eq!(UiaError::Busy.code(), "UIA_BUSY");
        assert!(UiaError::StaleElement.is_transient());
        assert!(!UiaError::Timeout("x").is_transient());
        assert!(UiaError::Busy.job_never_started());
        assert!(!UiaError::Timeout("handle").job_never_started());
        assert_eq!(
            UiaError::from(GuardError::NotResponding),
            UiaError::TargetNotResponding
        );
    }
}
