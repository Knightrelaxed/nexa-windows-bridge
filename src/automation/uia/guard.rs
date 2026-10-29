//! Window Guard — memastikan aksi hanya mengenai aplikasi foreground yang dimaksud.
//!
//! Beda filosofi dengan UFO: UFO memanggil `set_focus()` (MEMAKSA fokus) sebelum bertindak.
//! Di sini kita MEMVERIFIKASI dan menolak jika tidak cocok — agen tidak boleh diam-diam
//! menarik jendela lain ke depan hanya karena koordinat model meleset.
//!
//! Semua fungsi memakai Win32 biasa (tanpa COM) sehingga aman dipanggil dari thread mana pun.
//! Catatan TOCTOU: foreground bisa berubah di antara cek dan aksi (puluhan ms). Karena itu
//! `verify_unchanged` dipanggil sedekat mungkin dengan SendInput, dan di dalam loop mengetik.

use super::types::UiaError;
use crate::automation::input::{desktop_press_key, desktop_type_text, is_foreground_terminal_active};
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetForegroundWindow, GetWindowLongW, GetWindowRect,
    GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindow, IsWindowVisible,
    SendMessageTimeoutW, GA_ROOTOWNER, GWL_EXSTYLE, SMTO_ABORTIFHUNG, SMTO_BLOCK, WM_NULL,
    WS_EX_LAYERED, WS_EX_TRANSPARENT,
};

const DWMWA_CLOAKED: u32 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardError {
    NoForeground,
    OwnProcess,
    ProtectedProcess,
    WindowGone,
    ForegroundChanged,
    TargetNotForeground { foreground_pid: u32, target_pid: u32 },
    NoWindowAtPoint,
    NotResponding,
}

impl GuardError {
    pub fn code(&self) -> &'static str {
        match self {
            GuardError::NoForeground => "GUARD_NO_FOREGROUND",
            GuardError::OwnProcess => "GUARD_OWN_PROCESS",
            GuardError::ProtectedProcess => "GUARD_PROTECTED_PROCESS",
            GuardError::WindowGone => "GUARD_WINDOW_GONE",
            GuardError::ForegroundChanged => "GUARD_FOREGROUND_CHANGED",
            GuardError::TargetNotForeground { .. } => "GUARD_TARGET_NOT_FOREGROUND",
            GuardError::NoWindowAtPoint => "GUARD_NO_WINDOW_AT_POINT",
            GuardError::NotResponding => "TARGET_NOT_RESPONDING",
        }
    }
}

/// Identitas jendela foreground saat diperiksa. HWND disimpan sebagai `usize` agar `Send`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSnapshot {
    pub hwnd: usize,
    pub pid: u32,
    pub root_owner: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardedTarget {
    /// Jendela top-level di titik yang dituju (bebas dari hung app & bukan proses Nexa)
    pub target_hwnd: usize,
    pub target_pid: u32,
    /// Apakah jendela target saat ini berstatus foreground (aktif)?
    pub is_foreground: bool,
    /// Snapshot jendela foreground saat dicek (jika ada dan valid)
    pub snapshot: Option<WindowSnapshot>,
}

fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: hwnd boleh sudah tidak valid; API mengembalikan 0 tanpa efek samping.
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
    pid
}

fn root_owner_of(hwnd: HWND) -> usize {
    // SAFETY: lihat di atas.
    unsafe { GetAncestor(hwnd, GA_ROOTOWNER) as usize }
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut v: u32 = 0;
    // SAFETY: buffer 4 byte valid selama pemanggilan.
    let hr = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut v as *mut u32 as *mut c_void, 4) };
    hr >= 0 && v != 0
}

/// Ambil identitas foreground; tolak jika milik Nexa sendiri atau proses terproteksi (terminal).
pub fn capture_foreground() -> Result<WindowSnapshot, GuardError> {
    // SAFETY: tanpa argumen.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return Err(GuardError::NoForeground);
    }
    // SAFETY: IsWindow aman untuk handle apa pun.
    if unsafe { IsWindow(hwnd) } == 0 {
        return Err(GuardError::WindowGone);
    }
    let pid = pid_of(hwnd);
    if pid == 0 {
        return Err(GuardError::WindowGone);
    }
    if pid == std::process::id() {
        return Err(GuardError::OwnProcess);
    }
    if is_foreground_terminal_active() {
        return Err(GuardError::ProtectedProcess);
    }
    Ok(WindowSnapshot {
        hwnd: hwnd as usize,
        pid,
        root_owner: root_owner_of(hwnd),
    })
}

/// Foreground masih aplikasi yang sama? Popup/menu/dialog milik aplikasi yang sama diizinkan.
pub fn verify_unchanged(snap: &WindowSnapshot) -> Result<(), GuardError> {
    // SAFETY: tanpa argumen.
    let now = unsafe { GetForegroundWindow() };
    if now.is_null() {
        return Err(GuardError::NoForeground);
    }
    if now as usize == snap.hwnd {
        return Ok(());
    }
    if pid_of(now) == snap.pid || root_owner_of(now) == snap.root_owner {
        return Ok(());
    }
    Err(GuardError::ForegroundChanged)
}

// ─────────────────────────────────────────────────────────────────────────────
// Hit-test top-level berdasarkan Z-order (melewati overlay Nexa & overlay click-through lain)
// ─────────────────────────────────────────────────────────────────────────────

struct HitState {
    px: i32,
    py: i32,
    own_pid: u32,
    found: usize,
}

unsafe extern "system" fn hit_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam adalah &mut HitState milik `top_level_at`, hidup selama EnumWindows.
    let st = &mut *(lparam as *mut HitState);

    if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
        return 1;
    }
    if pid_of(hwnd) == st.own_pid {
        return 1; // overlay / pill Nexa
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_LAYERED != 0 && ex & WS_EX_TRANSPARENT != 0 {
        return 1; // overlay click-through aplikasi lain
    }
    if is_cloaked(hwnd) {
        return 1; // jendela UWP tersuspensi / desktop virtual lain
    }
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if GetWindowRect(hwnd, &mut r) == 0 {
        return 1;
    }
    if st.px >= r.left && st.px < r.right && st.py >= r.top && st.py < r.bottom {
        st.found = hwnd as usize;
        return 0; // berhenti: EnumWindows berurutan dari Z-order teratas
    }
    1
}

/// Jendela top-level paling atas (Z-order) yang berisi titik, selain milik Nexa.
pub fn top_level_at(px: i32, py: i32) -> Option<usize> {
    let mut st = HitState {
        px,
        py,
        own_pid: std::process::id(),
        found: 0,
    };
    // SAFETY: st hidup sampai EnumWindows selesai; callback tidak menyimpan pointer.
    unsafe {
        EnumWindows(Some(hit_cb), &mut st as *mut HitState as LPARAM);
    }
    if st.found != 0 {
        Some(st.found)
    } else {
        None
    }
}

/// Deteksi "Not Responding": `IsHungAppWindow` + ping `WM_NULL` ber-timeout.
/// Wajib dipanggil SEBELUM panggilan UIA lintas-proses (UIA ke proses hang = menunggu sampai timeout).
pub fn is_responsive(hwnd: usize, timeout_ms: u32) -> bool {
    let h = hwnd as HWND;
    // SAFETY: handle usang hanya menyebabkan kegagalan terkontrol.
    unsafe {
        if IsHungAppWindow(h) != 0 {
            return false;
        }
        let mut out: usize = 0;
        SendMessageTimeoutW(h, WM_NULL, 0, 0, SMTO_ABORTIFHUNG | SMTO_BLOCK, timeout_ms, &mut out) != 0
    }
}

/// Guard lengkap untuk aksi input pada titik layar (px, py):
/// mencari jendela top-level di titik → tolak jika milik internal Nexa → tolak jika hung/unresponsive.
/// Menandai apakah jendela target adalah jendela aktif (foreground) saat ini.
pub fn guard_point(px: i32, py: i32) -> Result<GuardedTarget, GuardError> {
    let target = top_level_at(px, py).ok_or(GuardError::NoWindowAtPoint)?;
    let target_h = target as HWND;
    let target_pid = pid_of(target_h);

    if target_pid == std::process::id() {
        return Err(GuardError::OwnProcess);
    }

    if !is_responsive(target, 150) {
        return Err(GuardError::NotResponding);
    }

    let (is_foreground, snapshot) = match capture_foreground() {
        Ok(snap) => {
            let is_fg = target_pid == snap.pid || root_owner_of(target_h) == snap.root_owner;
            (is_fg, Some(snap))
        }
        Err(_) => (false, None),
    };

    Ok(GuardedTarget {
        target_hwnd: target,
        target_pid,
        is_foreground,
        snapshot,
    })
}

/// Mengetik per potongan 16 karakter; foreground diverifikasi sebelum tiap potongan.
/// Memakai `desktop_type_text` yang sudah ada (abort flag + guard terminal tetap berlaku).
pub fn type_guarded(snap: &WindowSnapshot, text: &str, press_enter: bool) -> Result<usize, UiaError> {
    const CHUNK: usize = 16;
    let chars: Vec<char> = text.chars().collect();
    let mut typed = 0usize;
    for chunk in chars.chunks(CHUNK) {
        verify_unchanged(snap)?;
        let s: String = chunk.iter().collect();
        typed += desktop_type_text(&s, false).map_err(UiaError::InputFailed)?;
    }
    if press_enter {
        verify_unchanged(snap)?;
        desktop_press_key("ENTER").map_err(UiaError::InputFailed)?;
    }
    Ok(typed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_outside_any_window_is_none() {
        assert!(top_level_at(-30_000, -30_000).is_none());
    }

    #[test]
    fn capture_foreground_never_panics() {
        let _ = capture_foreground();
    }

    #[test]
    fn codes_are_stable() {
        assert_eq!(GuardError::OwnProcess.code(), "GUARD_OWN_PROCESS");
        assert_eq!(GuardError::NotResponding.code(), "TARGET_NOT_RESPONDING");
    }

    #[test]
    fn guard_point_outside_screen_returns_no_window() {
        assert_eq!(guard_point(-30_000, -30_000), Err(GuardError::NoWindowAtPoint));
    }
}
