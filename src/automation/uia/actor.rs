//! Aktor UIA: satu thread khusus ber-COM MTA yang memiliki SEMUA objek UIA.
//!
//! Mengapa bukan `spawn_blocking` per panggilan:
//!  - `CoCreateInstance(CUIAutomation)` + membangun kondisi/cache ulang tiap panggilan itu mahal.
//!  - Thread pool blocking tokio tidak bisa dibunuh; satu panggilan COM yang hang memakai satu
//!    thread pool selamanya dan bisa menumpuk (kebocoran thread).
//!  - Dengan satu aktor: antrean terbatas (backpressure → `Busy`), pembatalan kooperatif,
//!    circuit breaker, dan konteks bisa dibangun ulang saat provider mati.
//!
//! Thread UI Slint (STA, winit memanggil OleInitialize) TIDAK PERNAH menyentuh UIA.

use super::guard;
use super::scan::{
    activate_semantic, pick_among, revalidate, set_value_semantic, Scan, Semantic, UiaContext,
};
use super::snap::{default_radius_px, pick_best, SnapQuery};
use super::types::{
    ClickMethod, ClickReport, ControlKind, ControlSummary, SetTextReport, UiaError,
};
use crate::automation::element_finder::{pixel_rect_to_normalized, PixelRect, SnappedRect};
use crate::automation::input::{
    denormalize_to_pixels, desktop_click, desktop_press_key, get_primary_screen_dimensions,
    is_automation_abort_requested, MouseButton,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

const CLICK_BUDGET: Duration = Duration::from_millis(2500);
#[allow(dead_code)]
const SET_TEXT_BUDGET: Duration = Duration::from_millis(4000);
const SNAP_BUDGET: Duration = Duration::from_millis(350);
const REPLY_GRACE: Duration = Duration::from_millis(250);
const QUEUE_DEPTH: usize = 2;
const BREAKER_THRESHOLD: u32 = 3;
const BREAKER_OPEN_MS: u64 = 5_000;

// ─────────────────────────────────────────────────────────────────────────────
// Kebijakan & protokol (hanya DTO — aman menyeberang thread)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ClickPolicy {
    /// Coba pattern UIA (Invoke/Toggle/Select/Expand) sebelum klik fisik
    pub allow_semantic: bool,
    /// Jika tak ada kontrol interaktif di radius: klik koordinat mentah model (setelah Window Guard)
    pub allow_unsnapped_fallback: bool,
}

impl Default for ClickPolicy {
    fn default() -> Self {
        Self {
            allow_semantic: true,
            allow_unsnapped_fallback: true,
        }
    }
}

#[allow(dead_code)]
enum Request {
    Click {
        norm_x: f64,
        norm_y: f64,
        button: MouseButton,
        double: bool,
        policy: ClickPolicy,
    },
    SnapBox {
        ymin: f64,
        xmin: f64,
        ymax: f64,
        xmax: f64,
    },
    SetText {
        norm_x: f64,
        norm_y: f64,
        text: String,
        press_enter: bool,
    },
}

#[allow(dead_code)]
enum Response {
    Click(ClickReport),
    Snap(Option<PixelRect>),
    SetText(SetTextReport),
}

struct Job {
    req: Request,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    reply: oneshot::Sender<Result<Response, UiaError>>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Circuit breaker: setelah 3 timeout beruntun, UIA "dimatikan" 5 detik → pemanggil
// langsung jatuh ke jalur vision tanpa menunggu.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Breaker {
    timeouts: AtomicU32,
    open_until_ms: AtomicU64,
}

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

impl Breaker {
    fn allow(&self) -> bool {
        now_ms() >= self.open_until_ms.load(Ordering::Relaxed)
    }
    fn record_ok(&self) {
        self.timeouts.store(0, Ordering::Relaxed);
    }
    fn record_timeout(&self) {
        let n = self.timeouts.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= BREAKER_THRESHOLD {
            self.open_until_ms
                .store(now_ms() + BREAKER_OPEN_MS, Ordering::Relaxed);
            self.timeouts.store(0, Ordering::Relaxed);
            warn!("[UIA] Circuit breaker TERBUKA selama {} ms", BREAKER_OPEN_MS);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handle async (sisi tokio)
// ─────────────────────────────────────────────────────────────────────────────

pub struct UiaHandle {
    tx: SyncSender<Job>,
    breaker: Breaker,
}

static GLOBAL: OnceLock<Result<UiaHandle, UiaError>> = OnceLock::new();

impl UiaHandle {
    /// Aktor dibuat malas pada panggilan pertama → tanda tangan `handle_execute_command`
    /// dan seluruh test yang ada tidak perlu diubah.
    pub fn global() -> Result<&'static UiaHandle, UiaError> {
        match GLOBAL.get_or_init(Self::spawn) {
            Ok(h) => Ok(h),
            Err(e) => Err(e.clone()),
        }
    }

    fn spawn() -> Result<UiaHandle, UiaError> {
        let (tx, rx) = sync_channel::<Job>(QUEUE_DEPTH);
        std::thread::Builder::new()
            .name("nexa-uia".into())
            .spawn(move || actor_main(rx))
            .map_err(|e| UiaError::Unavailable(format!("gagal spawn thread UIA: {e}")))?;
        Ok(UiaHandle {
            tx,
            breaker: Breaker::default(),
        })
    }

    async fn call(&self, req: Request, budget: Duration) -> Result<Response, UiaError> {
        if !self.breaker.allow() {
            return Err(UiaError::CircuitOpen);
        }
        let (reply, rx) = oneshot::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            req,
            deadline: Instant::now() + budget,
            cancel: Arc::clone(&cancel),
            reply,
        };
        match self.tx.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => return Err(UiaError::Busy),
            Err(TrySendError::Disconnected(_)) => {
                return Err(UiaError::Unavailable("aktor UIA berhenti".into()))
            }
        }
        match tokio::time::timeout(budget + REPLY_GRACE, rx).await {
            Ok(Ok(res)) => {
                match &res {
                    Err(UiaError::Timeout(_)) => self.breaker.record_timeout(),
                    _ => self.breaker.record_ok(),
                }
                res
            }
            Ok(Err(_)) => Err(UiaError::Internal("aktor menutup channel balasan".into())),
            Err(_) => {
                // Aktor masih di dalam panggilan COM. Beri sinyal batal; hasil akan dibuang.
                cancel.store(true, Ordering::SeqCst);
                self.breaker.record_timeout();
                Err(UiaError::Timeout("handle"))
            }
        }
    }

    /// Klik hybrid: Window Guard → scan interaktif → snap → semantik (UIA pattern) → fisik.
    pub async fn click(
        &self,
        norm_x: f64,
        norm_y: f64,
        button: MouseButton,
        double: bool,
        policy: ClickPolicy,
    ) -> Result<ClickReport, UiaError> {
        let req = Request::Click {
            norm_x,
            norm_y,
            button,
            double,
            policy,
        };
        match self.call(req, CLICK_BUDGET).await? {
            Response::Click(r) => Ok(r),
            _ => Err(UiaError::Internal("tipe respons tidak cocok".into())),
        }
    }

    /// Magnet-snap untuk highlight. TIDAK PERNAH gagal: error/timeout → box visi apa adanya.
    pub async fn snap_box(&self, ymin: f64, xmin: f64, ymax: f64, xmax: f64) -> SnappedRect {
        let fallback = SnappedRect::new(ymin, xmin, ymax, xmax, false);
        let req = Request::SnapBox {
            ymin,
            xmin,
            ymax,
            xmax,
        };
        match self.call(req, SNAP_BUDGET).await {
            Ok(Response::Snap(Some(rect))) => {
                let s = get_primary_screen_dimensions();
                pixel_rect_to_normalized(&rect, s.width, s.height)
            }
            Ok(_) => fallback,
            Err(e) => {
                debug!("[UIA] snap_box → fallback vision: {}", e);
                fallback
            }
        }
    }

    /// Ganti isi field teks di titik (ValuePattern, fallback Ctrl+A + ketik terjaga Window Guard).
    #[allow(dead_code)]
    pub async fn set_text(
        &self,
        norm_x: f64,
        norm_y: f64,
        text: String,
        press_enter: bool,
    ) -> Result<SetTextReport, UiaError> {
        let req = Request::SetText {
            norm_x,
            norm_y,
            text,
            press_enter,
        };
        match self.call(req, SET_TEXT_BUDGET).await? {
            Response::SetText(r) => Ok(r),
            _ => Err(UiaError::Internal("tipe respons tidak cocok".into())),
        }
    }
}

/// Jalur terdegradasi (tanpa UIA, tetap dengan Window Guard). HANYA dipakai bila
/// `err.job_never_started()` — jangan pernah setelah `Timeout`, atau aksi bisa ganda.
pub fn degraded_click(
    norm_x: f64,
    norm_y: f64,
    button: MouseButton,
    double: bool,
) -> Result<ClickReport, UiaError> {
    let t0 = Instant::now();
    let s = get_primary_screen_dimensions();
    let (px, py) = denormalize_to_pixels(norm_x, norm_y, s.width, s.height);
    let target = guard::guard_point(px, py)?;
    if target.is_foreground {
        if let Some(ref snap) = target.snapshot {
            let _ = guard::verify_unchanged(snap);
        }
    }
    let (rx, ry) = desktop_click(norm_x, norm_y, button, double).map_err(UiaError::InputFailed)?;
    Ok(ClickReport {
        method: ClickMethod::PhysicalRaw,
        px: rx,
        py: ry,
        snapped: false,
        control: None,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Thread aktor
// ─────────────────────────────────────────────────────────────────────────────

/// RAII: CoUninitialize hanya setelah SEMUA objek COM thread ini dilepas.
/// Urutan deklarasi di `actor_main` (guard dulu, ctx sesudahnya) menjamin ctx di-drop lebih dulu.
struct ComGuard;

impl ComGuard {
    fn init_mta() -> Result<Self, UiaError> {
        // SAFETY: thread baru; belum ada model apartment.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_ok() {
            Ok(ComGuard) // S_OK maupun S_FALSE sama-sama harus diseimbangkan dengan CoUninitialize
        } else {
            Err(UiaError::Com(hr.0))
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: seimbang dengan CoInitializeEx yang sukses.
        unsafe { CoUninitialize() }
    }
}

fn actor_main(rx: Receiver<Job>) {
    let _com = match ComGuard::init_mta() {
        Ok(g) => g,
        Err(e) => {
            warn!("[UIA] CoInitializeEx gagal: {e}");
            while let Ok(job) = rx.recv() {
                let _ = job.reply.send(Err(e.clone()));
            }
            return;
        }
    };
    info!("[UIA] Aktor MTA siap");

    let mut ctx: Option<UiaContext> = None;

    while let Ok(job) = rx.recv() {
        // Buang job basi: pemanggil sudah menyerah / deadline lewat saat antre.
        if job.cancel.load(Ordering::SeqCst) || Instant::now() >= job.deadline {
            let _ = job.reply.send(Err(UiaError::Cancelled));
            continue;
        }

        if ctx.is_none() {
            match UiaContext::new() {
                Ok(c) => ctx = Some(c),
                Err(e) => {
                    let _ = job.reply.send(Err(e));
                    continue;
                }
            }
        }
        let Some(c) = ctx.as_ref() else {
            let _ = job.reply.send(Err(UiaError::Internal("konteks UIA hilang".into())));
            continue;
        };

        // Catatan: pada profil release (`panic = "abort"`) catch_unwind tidak berfungsi;
        // kode di bawah ditulis bebas-panic (tanpa unwrap/indexing). catch_unwind hanya
        // bermanfaat pada build dev/test atau bila Anda beralih ke panic = "unwind".
        let res = catch_unwind(AssertUnwindSafe(|| run_job(c, &job)))
            .unwrap_or_else(|_| Err(UiaError::Internal("panic di job UIA".into())));

        if let Err(e) = &res {
            if e.invalidates_context() {
                ctx = None; // bangun ulang pada job berikutnya
            }
        }
        let _ = job.reply.send(res);
    }
}

fn run_job(ctx: &UiaContext, job: &Job) -> Result<Response, UiaError> {
    let deadline = job.deadline;
    let cancel: &AtomicBool = &job.cancel;
    match &job.req {
        Request::Click {
            norm_x,
            norm_y,
            button,
            double,
            policy,
        } => run_click(ctx, *norm_x, *norm_y, *button, *double, *policy, deadline, cancel)
            .map(Response::Click),
        Request::SnapBox {
            ymin,
            xmin,
            ymax,
            xmax,
        } => run_snap(ctx, *ymin, *xmin, *ymax, *xmax, deadline, cancel).map(Response::Snap),
        Request::SetText {
            norm_x,
            norm_y,
            text,
            press_enter,
        } => run_set_text(ctx, *norm_x, *norm_y, text, *press_enter, deadline, cancel)
            .map(Response::SetText),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Job: klik hybrid
// ─────────────────────────────────────────────────────────────────────────────

fn snap_query(px: i32, py: i32, hint: Option<i64>) -> SnapQuery {
    let s = get_primary_screen_dimensions();
    SnapQuery {
        px,
        py,
        radius_px: default_radius_px(s.height),
        hint_area_px: hint,
        screen_w: s.width,
        screen_h: s.height,
    }
}

fn physical_click_at_pixel(px: i32, py: i32, button: MouseButton, double: bool) -> Result<(), UiaError> {
    if is_automation_abort_requested() {
        return Err(UiaError::Cancelled);
    }
    let s = get_primary_screen_dimensions();
    // +0.5 → pusat piksel; denormalize_to_pixels (floor) akan mengembalikan tepat (px, py)
    let nx = (px as f64 + 0.5) / (s.width.max(1) as f64) * 1000.0;
    let ny = (py as f64 + 0.5) / (s.height.max(1) as f64) * 1000.0;
    desktop_click(nx, ny, button, double)
        .map(|_| ())
        .map_err(UiaError::InputFailed)
}

#[allow(clippy::too_many_arguments)]
fn run_click(
    ctx: &UiaContext,
    norm_x: f64,
    norm_y: f64,
    button: MouseButton,
    double: bool,
    policy: ClickPolicy,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<ClickReport, UiaError> {
    let t0 = Instant::now();
    let screen = get_primary_screen_dimensions();
    let (px, py) = denormalize_to_pixels(norm_x, norm_y, screen.width, screen.height);

    // 1) Window Guard (titik pada jendela valid & responsif, bukan internal Nexa)
    let target = guard::guard_point(px, py)?;
    let query = snap_query(px, py, None);

    // 2) Scan + snap (+ 1x ulang bila elemen stale)
    // Scan pada target_hwnd; jika gagal/timeout, operasi baca murni ini tidak boleh mengaborsi klik
    for attempt in 0..2 {
        match ctx.scan_window(target.target_hwnd, deadline, cancel) {
            Ok(scan) => {
                if let Some(m) = pick_best(&scan.infos, &query) {
                    match act_on(&scan, m.index, &target, button, double, policy, t0) {
                        Ok(report) => return Ok(report),
                        Err(e) if e.is_transient() && attempt == 0 => {
                            debug!("[UIA] transient ({}), scan ulang", e);
                            std::thread::sleep(Duration::from_millis(60));
                            continue;
                        }
                        Err(e) => {
                            debug!("[UIA] act_on gagal ({}), lanjut ke fallback fisik", e);
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
            Err(e) => {
                debug!("[UIA] scan_window gagal/timeout ({}), fallback ke klik fisik", e);
                break;
            }
        }
    }

    // 3) Fallback: klik fisik di koordinat mentah
    if !policy.allow_unsnapped_fallback {
        return Err(UiaError::NoTarget);
    }
    if target.is_foreground {
        if let Some(ref snap) = target.snapshot {
            let _ = guard::verify_unchanged(snap);
        }
    }
    if is_automation_abort_requested() {
        return Err(UiaError::Cancelled);
    }
    let (rx, ry) = desktop_click(norm_x, norm_y, button, double).map_err(UiaError::InputFailed)?;
    Ok(ClickReport {
        method: ClickMethod::PhysicalRaw,
        px: rx,
        py: ry,
        snapped: false,
        control: None,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    })
}

fn act_on(
    scan: &Scan,
    idx: usize,
    target: &guard::GuardedTarget,
    button: MouseButton,
    double: bool,
    policy: ClickPolicy,
    t0: Instant,
) -> Result<ClickReport, UiaError> {
    let info = scan.infos.get(idx).ok_or(UiaError::StaleElement)?;
    let el = scan.elements.get(idx).ok_or(UiaError::StaleElement)?;
    let (cx, cy) = info.center();
    let summary = || {
        Some(ControlSummary {
            kind: info.kind,
            name: info.short_name(60),
            rect: info.rect,
        })
    };
    let report = |method| ClickReport {
        method,
        px: cx,
        py: cy,
        snapped: true,
        control: summary(),
        elapsed_ms: t0.elapsed().as_millis() as u64,
    };

    revalidate(info, el)?;

    // Tier-1 semantik: HANYA dilakukan jika jendela target saat ini adalah foreground,
    // klik kiri tunggal, dan foreground belum berubah.
    if target.is_foreground && policy.allow_semantic && button == MouseButton::Left && !double {
        if let Some(ref snap) = target.snapshot {
            if guard::verify_unchanged(snap).is_ok() {
                match activate_semantic(el, info.kind)? {
                    Semantic::Done(p) => return Ok(report(ClickMethod::Semantic(p))),
                    // Timeout = ambigu. Fallback fisik di sini berisiko klik ganda -> hentikan.
                    Semantic::Unconfirmed(p) => return Ok(report(ClickMethod::SemanticUnconfirmed(p))),
                    Semantic::NotApplicable | Semantic::Failed => {}
                }
            }
        }
    }

    // Tier-2 fisik di pusat kontrol hasil snap
    physical_click_at_pixel(cx, cy, button, double)?;
    Ok(report(ClickMethod::PhysicalSnapped))
}

// ─────────────────────────────────────────────────────────────────────────────
// Job: snap untuk highlight (tanpa syarat foreground — tidak ada input yang dikirim)
// ─────────────────────────────────────────────────────────────────────────────

fn run_snap(
    ctx: &UiaContext,
    ymin: f64,
    xmin: f64,
    ymax: f64,
    xmax: f64,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<Option<PixelRect>, UiaError> {
    let s = get_primary_screen_dimensions();
    let (px, py) = denormalize_to_pixels((xmin + xmax) / 2.0, (ymin + ymax) / 2.0, s.width, s.height);
    let Some(target) = guard::top_level_at(px, py) else {
        return Ok(None);
    };
    if !guard::is_responsive(target, 100) {
        return Err(UiaError::TargetNotResponding);
    }
    let (x0, y0) = denormalize_to_pixels(xmin, ymin, s.width, s.height);
    let (x1, y1) = denormalize_to_pixels(xmax, ymax, s.width, s.height);
    let area = ((x1 - x0).max(0) as i64) * ((y1 - y0).max(0) as i64);

    let scan = ctx.scan_window(target, deadline, cancel)?;
    let q = snap_query(px, py, if area > 0 { Some(area) } else { None });
    Ok(pick_best(&scan.infos, &q)
        .and_then(|m| scan.infos.get(m.index))
        .map(|c| c.rect))
}

// ─────────────────────────────────────────────────────────────────────────────
// Job: set teks (semantik → fallback fisik terjaga)
// ─────────────────────────────────────────────────────────────────────────────

fn run_set_text(
    ctx: &UiaContext,
    norm_x: f64,
    norm_y: f64,
    text: &str,
    press_enter: bool,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<SetTextReport, UiaError> {
    let t0 = Instant::now();
    let s = get_primary_screen_dimensions();
    let (px, py) = denormalize_to_pixels(norm_x, norm_y, s.width, s.height);
    let target = guard::guard_point(px, py)?;
    let q = snap_query(px, py, None);
    let finish = |method: &'static str| SetTextReport {
        method,
        chars: text.chars().count(),
        elapsed_ms: t0.elapsed().as_millis() as u64,
    };

    let scan = ctx.scan_window(target.target_hwnd, deadline, cancel)?;
    let Some(idx) = pick_among(&scan, &[ControlKind::Edit, ControlKind::ComboBox], &q) else {
        return Err(UiaError::NoTarget);
    };
    let info = scan.infos.get(idx).ok_or(UiaError::StaleElement)?;
    let el = scan.elements.get(idx).ok_or(UiaError::StaleElement)?;
    revalidate(info, el)?;
    let snap = target.snapshot.as_ref().ok_or_else(|| {
        UiaError::Guard(guard::GuardError::TargetNotForeground {
            foreground_pid: 0,
            target_pid: target.target_pid,
        })
    })?;
    guard::verify_unchanged(snap)?;

    match set_value_semantic(el, text)? {
        Semantic::Done(_) => {
            if press_enter {
                guard::verify_unchanged(snap)?;
                desktop_press_key("ENTER").map_err(UiaError::InputFailed)?;
            }
            return Ok(finish("semantic_value"));
        }
        // SetValue mungkin sudah menulis sebagian; mengetik ulang di atasnya = isi ganda.
        Semantic::Unconfirmed(_) => return Err(UiaError::Timeout("set_value")),
        Semantic::NotApplicable | Semantic::Failed => {}
    }

    // Fallback fisik: fokus lewat klik di pusat field → Ctrl+A → ketik per potongan terjaga.
    let (cx, cy) = info.center();
    physical_click_at_pixel(cx, cy, MouseButton::Left, false)?;
    std::thread::sleep(Duration::from_millis(40));
    guard::verify_unchanged(snap)?;
    desktop_press_key("CTRL+A").map_err(UiaError::InputFailed)?;
    guard::type_guarded(snap, text, press_enter)?;
    Ok(finish("physical_typed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_opens_after_threshold() {
        let b = Breaker::default();
        assert!(b.allow());
        b.record_timeout();
        b.record_timeout();
        assert!(b.allow());
        b.record_timeout();
        assert!(!b.allow());
    }

    #[test]
    fn breaker_resets_on_success() {
        let b = Breaker::default();
        b.record_timeout();
        b.record_timeout();
        b.record_ok();
        b.record_timeout();
        assert!(b.allow());
    }

    #[test]
    fn default_policy_allows_semantic() {
        let p = ClickPolicy::default();
        assert!(p.allow_semantic && p.allow_unsnapped_fallback);
    }

    #[tokio::test]
    #[ignore = "Butuh sesi desktop Windows interaktif"]
    async fn snap_box_never_fails() {
        let h = UiaHandle::global().expect("aktor UIA");
        let r = h.snap_box(400.0, 400.0, 450.0, 500.0).await;
        assert!((0.0..=1000.0).contains(&r.xmin));
    }
}
