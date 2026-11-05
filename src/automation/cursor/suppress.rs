//! Penekan kursor sistem Windows untuk "Seamless Morph" + pemulihan berlapis.
//!
//! Letak yang disarankan: `src/platform/cursor_suppress.rs` (daftarkan di `platform/mod.rs`).
//!
//! STATUS: draf referensi. TIDAK dikompilasi terhadap windows-sys 0.59 dan TIDAK diuji di
//! perangkat keras (lingkungan penyusun tidak punya toolchain Windows). Cocokkan tanda tangan
//! API dengan versi crate di Cargo.lock, lalu uji di X380 sebelum dipakai.
//!
//! Fitur windows-sys yang dibutuhkan sudah ada di Cargo.toml HEAD ac4025d:
//! Win32_UI_WindowsAndMessaging, Win32_System_Threading, Win32_System_LibraryLoader,
//! Win32_System_Console, Win32_Foundation.
//!
//! Lapisan pemulihan (dari terdekat ke terjauh):
//!   L1  RAII `SuppressGuard` (refcount)         -> keluar normal, early-return, `?`
//!   L2  `install_panic_restore()`               -> panic (berjalan juga pada panic = "abort")
//!   L3  lease + thread watchdog                 -> kebocoran logika, loop animasi mati/macet
//!   L4  `install_console_ctrl_handler()`        -> Ctrl+C / close / logoff (hanya mode --console)
//!   L5  proses guardian (`--cursor-guardian`)   -> TerminateProcess, crash keras, abort
//!   L6  penanda "kotor" + `heal_on_startup()`   -> sisa kerusakan saat bridge dijalankan lagi

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, Instant};
use tracing::{info, warn};
use windows_sys::Win32::Foundation::{CloseHandle, BOOL, LPARAM, LRESULT, WAIT_OBJECT_0, WPARAM};
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CopyIcon, CreateCursor, DestroyCursor, DispatchMessageW, GetMessageW,
    GetSystemMetrics, SetSystemCursor, SetWindowsHookExW, SystemParametersInfoW, TranslateMessage,
    UnhookWindowsHookEx, HCURSOR, LLMHF_INJECTED, LLMHF_LOWER_IL_INJECTED, MSG, MSLLHOOKSTRUCT,
    SM_CXCURSOR, SM_CYCURSOR, SPI_SETCURSORS, WH_MOUSE_LL, WM_MOUSEMOVE,
};

/// 13 ID kursor sistem yang terdokumentasi untuk SetSystemCursor (OCR_*).
/// Semuanya diganti, bukan hanya NORMAL/IBEAM/HAND: WAIT dan APPSTARTING sering muncul
/// tepat setelah klik, dan SIZE* muncul bila jalur glide melewati tepi jendela.
const SYSTEM_CURSOR_IDS: [u32; 13] = [
    32512, // OCR_NORMAL
    32513, // OCR_IBEAM
    32514, // OCR_WAIT
    32515, // OCR_CROSS
    32516, // OCR_UP
    32642, // OCR_SIZENWSE
    32643, // OCR_SIZENESW
    32644, // OCR_SIZEWE
    32645, // OCR_SIZENS
    32646, // OCR_SIZEALL
    32648, // OCR_NO
    32649, // OCR_HAND
    32650, // OCR_APPSTARTING
];

/// Gerak fisik (px, Manhattan) yang dianggap "manusia menyentuh mouse". Klik/roda: langsung.
const HUMAN_MIN_PX: i32 = 3;

static ENABLED: AtomicBool = AtomicBool::new(cfg!(not(test))); // jangan sentuh kursor saat `cargo test`
static ENGAGED: AtomicBool = AtomicBool::new(false);
static REFS: AtomicU32 = AtomicU32::new(0);
static LEASE_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
static STATE_LOCK: Mutex<()> = Mutex::new(());

static TRIPPED: AtomicBool = AtomicBool::new(false);
static HUMAN_TX: OnceLock<Sender<()>> = OnceLock::new();
static LAST_X: AtomicI32 = AtomicI32::new(i32::MIN);
static LAST_Y: AtomicI32 = AtomicI32::new(i32::MIN);
static ACCUM: AtomicI32 = AtomicI32::new(0);

fn now_ms() -> u64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Sakelar dari config (`hide_system_cursor`). Default: aktif, kecuali pada unit test.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
}

pub fn is_engaged() -> bool {
    ENGAGED.load(Ordering::SeqCst)
}

fn renew_lease(lease_ms: u64) {
    LEASE_UNTIL_MS.fetch_max(now_ms() + lease_ms, Ordering::SeqCst);
}

// ─────────────────────────────────────────────────────────────────────────────
// Primitif Win32
// ─────────────────────────────────────────────────────────────────────────────

/// Kursor transparan: AND=1 (pertahankan piksel layar), XOR=0. Scanline rata WORD (16 bit).
unsafe fn make_blank_cursor() -> HCURSOR {
    let w = GetSystemMetrics(SM_CXCURSOR).max(1);
    let h = GetSystemMetrics(SM_CYCURSOR).max(1);
    let stride = (((w + 15) / 16) * 2) as usize;
    let len = stride * h as usize;
    let and_plane = vec![0xFFu8; len];
    let xor_plane = vec![0x00u8; len];
    CreateCursor(
        std::ptr::null_mut(),
        0,
        0,
        w,
        h,
        and_plane.as_ptr().cast(),
        xor_plane.as_ptr().cast(),
    )
}

/// Ganti semua kursor sistem dengan kursor kosong.
/// SetSystemCursor MENGHANCURKAN handle yang diberikan (dokumentasi Microsoft), jadi tiap ID
/// butuh salinan sendiri. Gagal di tengah jalan -> pulihkan semuanya (jangan setengah-sembunyi).
unsafe fn apply_blank() -> bool {
    let master = make_blank_cursor();
    if master.is_null() {
        return false;
    }
    let mut ok = true;
    for id in SYSTEM_CURSOR_IDS {
        let copy = CopyIcon(master);
        if copy.is_null() {
            ok = false;
            break;
        }
        if SetSystemCursor(copy, id) == 0 {
            DestroyCursor(copy); // gagal -> sistem tidak mengambil alih handle
            ok = false;
            break;
        }
    }
    DestroyCursor(master);
    if !ok {
        reload_system_cursors();
    }
    ok
}

/// SPI_SETCURSORS memuat ulang kursor dari HKCU\Control Panel\Cursors: tanpa-status, idempoten,
/// dan mengembalikan skema pengguna apa adanya.
fn reload_system_cursors() -> bool {
    unsafe { SystemParametersInfoW(SPI_SETCURSORS, 0, std::ptr::null_mut(), 0) != 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// L6: penanda "kotor" di disk
// ─────────────────────────────────────────────────────────────────────────────

fn marker_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("NexaBridge")
        .join("cursor.dirty")
}

fn write_dirty_marker() {
    let p = marker_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, std::process::id().to_string());
}

fn clear_dirty_marker() {
    let _ = std::fs::remove_file(marker_path());
}

fn dirty_marker_exists() -> bool {
    marker_path().exists()
}

/// Panggil PALING AWAL di `main()`: bila proses sebelumnya mati saat kursor disembunyikan.
pub fn heal_on_startup() {
    if dirty_marker_exists() {
        warn!("[CURSOR] Penanda kotor ditemukan (proses sebelumnya berhenti tak wajar). Memulihkan kursor.");
        reload_system_cursors();
        clear_dirty_marker();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// L1: RAII + refcount
// ─────────────────────────────────────────────────────────────────────────────

/// Satu referensi penekanan. Referensi terakhir yang di-drop memulihkan kursor.
/// Simpan SATU guard di CursorController (diambil saat sprite tampil, dilepas saat sprite hilang).
pub struct SuppressGuard {
    _private: (),
}

impl SuppressGuard {
    /// `lease_ms`: umur lisensi. Tanpa `touch()` lagi sebelum habis, watchdog memulihkan kursor.
    pub fn acquire(lease_ms: u64) -> Option<Self> {
        if !ENABLED.load(Ordering::SeqCst) {
            return None;
        }
        let _g = STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        renew_lease(lease_ms);
        if REFS.fetch_add(1, Ordering::SeqCst) == 0 {
            // Tulis penanda SEBELUM mengubah kursor: bila crash di antaranya, guardian tetap memulihkan.
            write_dirty_marker();
            if !unsafe { apply_blank() } {
                REFS.fetch_sub(1, Ordering::SeqCst);
                clear_dirty_marker();
                return None;
            }
            TRIPPED.store(false, Ordering::SeqCst);
            ENGAGED.store(true, Ordering::SeqCst);
            ensure_watchdog();
        }
        Some(Self { _private: () })
    }

    /// Perpanjang lisensi. Panggil tiap tick animasi (16 ms) dan tiap 200 ms selama `hold()`.
    pub fn touch(&self, lease_ms: u64) {
        renew_lease(lease_ms);
    }
}

impl Drop for SuppressGuard {
    fn drop(&mut self) {
        let _g = STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // saturating: aman bila emergency_restore() sudah menolkan REFS
        let cur = REFS.load(Ordering::SeqCst);
        let prev = cur;
        if cur > 0 {
            REFS.store(cur - 1, Ordering::SeqCst);
        }
        if prev == 1 {
            restore_locked();
        }
    }
}

fn restore_locked() {
    if ENGAGED.swap(false, Ordering::SeqCst) {
        reload_system_cursors();
        clear_dirty_marker();
    }
}

/// Tanpa-lock dan tanpa-alokasi-berat: aman dari panic hook, handler sinyal, thread mana pun.
/// Sengaja TIDAK mengambil STATE_LOCK (bisa sedang dipegang thread yang panik).
pub fn emergency_restore() {
    REFS.store(0, Ordering::SeqCst);
    if ENGAGED.swap(false, Ordering::SeqCst) || dirty_marker_exists() {
        reload_system_cursors();
        clear_dirty_marker();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// L3: watchdog lisensi (thread std murni, tidak bergantung runtime tokio)
// ─────────────────────────────────────────────────────────────────────────────

fn ensure_watchdog() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("nexa-cursor-wd".into())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_millis(50));
                if ENGAGED.load(Ordering::SeqCst) && now_ms() > LEASE_UNTIL_MS.load(Ordering::SeqCst) {
                    warn!("[CURSOR] Lisensi penekanan habis tanpa pelepasan: memulihkan kursor sistem.");
                    emergency_restore();
                }
            });
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// L2 & L4: panic hook dan handler konsol
// ─────────────────────────────────────────────────────────────────────────────

/// Bungkus hook panic yang sudah ada: pulihkan kursor DULU, lalu jalankan hook lama.
/// Panggil SETELAH `std::panic::set_hook(...)` milik main.rs. Pada `panic = "abort"`, hook
/// tetap berjalan sebelum abort, tetapi Drop tidak -> hook inilah satu-satunya pemulih.
pub fn install_panic_restore() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        emergency_restore();
        prev(info);
    }));
}

unsafe extern "system" fn console_ctrl(_ctrl_type: u32) -> BOOL {
    emergency_restore();
    0 // FALSE: biarkan handler berikutnya/default menghentikan proses
}

/// Hanya bermakna bila proses punya konsol (`--console` atau build debug). Aplikasi subsystem
/// "windows" tanpa konsol tidak menerima Ctrl+C; untuk logoff/shutdown pakai WM_ENDSESSION
/// pada jendela pesan tersembunyi yang sudah ada.
pub fn install_console_ctrl_handler() {
    unsafe {
        SetConsoleCtrlHandler(Some(console_ctrl), 1);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// L5: proses guardian
// ─────────────────────────────────────────────────────────────────────────────

/// Jalankan dari `main()` SEBELUM `SingleInstanceGuard::acquire()` bila argumen berisi
/// `--cursor-guardian <pid>` (kalau tidak, mutex instans-tunggal menolak guardian).
/// Menunggu proses induk berakhir karena sebab apa pun, lalu memulihkan kursor bila kotor.
pub fn run_guardian(parent_pid: u32) -> ! {
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, parent_pid);
        if !h.is_null() {
            while WaitForSingleObject(h, 1000) != WAIT_OBJECT_0 {}
            CloseHandle(h);
        }
    }
    if dirty_marker_exists() {
        reload_system_cursors();
        clear_dirty_marker();
    }
    std::process::exit(0)
}

/// Luncurkan guardian dari proses utama (sekali, saat startup).
pub fn spawn_guardian() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let pid = std::process::id().to_string();
    let build = |flags: u32| {
        let mut c = Command::new(&exe);
        c.arg("--cursor-guardian")
            .arg(&pid)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags);
        c
    };
    // Breakaway gagal (ACCESS_DENIED) bila job induk melarangnya -> coba tanpa flag itu.
    if build(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB).spawn().is_err() {
        if let Err(e) = build(CREATE_NO_WINDOW).spawn() {
            warn!("[CURSOR] Gagal meluncurkan guardian: {e}");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Deteksi "manusia menyentuh mouse" (menggantikan polling YieldWatcher)
// ─────────────────────────────────────────────────────────────────────────────

/// Callback hook: HARUS O(1), tanpa alokasi, tanpa lock, tanpa panic. Bila melewati
/// LowLevelHooksTimeout, Windows 7+ melepas hook DIAM-DIAM tanpa pemberitahuan (maks 1000 ms
/// sejak Windows 10 1709). Pekerjaan berat dilempar ke thread pekerja lewat channel.
unsafe extern "system" fn ll_mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && lparam != 0 {
        let info = &*(lparam as *const MSLLHOOKSTRUCT);
        let injected = info.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0;
        let (px, py) = (info.pt.x, info.pt.y);
        let (lx, ly) = (LAST_X.swap(px, Ordering::Relaxed), LAST_Y.swap(py, Ordering::Relaxed));
        let (dx, dy) = if lx == i32::MIN || ly == i32::MIN {
            (0, 0)
        } else {
            ((px - lx).abs(), (py - ly).abs())
        };

        if injected {
            // SendInput kita sendiri (atau pihak lain): bukan manusia.
            ACCUM.store(0, Ordering::Relaxed);
        } else if ENGAGED.load(Ordering::Relaxed) {
            let moved = if wparam as u32 == WM_MOUSEMOVE {
                ACCUM.fetch_add(dx + dy, Ordering::Relaxed) + dx + dy
            } else {
                i32::MAX // klik atau roda fisik: langsung dianggap manusia
            };
            if moved >= HUMAN_MIN_PX && !TRIPPED.swap(true, Ordering::SeqCst) {
                if let Some(tx) = HUMAN_TX.get() {
                    let _ = tx.send(());
                }
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

/// Mulai pemantau. `on_human` dipanggil di thread pekerja SETELAH kursor sistem dipulihkan
/// (mis. `set_human_interrupted(true)` + `CursorController::global().hide()`).
/// Panggil sekali saat startup. Jangan pasang breakpoint di dalam callback saat debugging:
/// hook low-level yang tertahan membekukan input mouse seluruh sistem.
pub fn start_human_input_monitor<F>(on_human: F)
where
    F: Fn() + Send + 'static,
{
    let (tx, rx) = channel::<()>();
    if HUMAN_TX.set(tx).is_err() {
        return; // sudah berjalan
    }

    let _ = std::thread::Builder::new()
        .name("nexa-human-worker".into())
        .spawn(move || {
            for () in rx {
                {
                    let _g = STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                    restore_locked();
                }
                info!("[CURSOR] Input fisik terdeteksi: kursor sistem dipulihkan.");
                on_human();
            }
        });

    let _ = std::thread::Builder::new()
        .name("nexa-human-hook".into())
        .spawn(|| unsafe {
            let hmod = GetModuleHandleW(std::ptr::null());
            let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(ll_mouse_proc), hmod, 0);
            if hook.is_null() {
                warn!("[CURSOR] Gagal memasang WH_MOUSE_LL: deteksi manusia jatuh ke polling.");
                return;
            }
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            UnhookWindowsHookEx(hook);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suppress_disabled_under_cfg_test() {
        assert!(!ENABLED.load(Ordering::SeqCst), "Must be disabled by default during tests");
        let guard = SuppressGuard::acquire(500);
        assert!(guard.is_none(), "Acquire must return None when disabled");
        assert!(!is_engaged());
    }

    #[test]
    fn test_dirty_marker_roundtrip() {
        clear_dirty_marker();
        assert!(!dirty_marker_exists());
        write_dirty_marker();
        assert!(dirty_marker_exists());
        heal_on_startup();
        assert!(!dirty_marker_exists());
    }
}

