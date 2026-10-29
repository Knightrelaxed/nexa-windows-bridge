//! Lapisan COM. SEMUA tipe di file ini (IUIAutomation*, IUIAutomationElement*) hanya boleh
//! hidup di thread aktor MTA. Tidak ada yang `pub` ke luar modul `uia`.
//!
//! Port dari `inspector.py` UFO: kondisi filter dievaluasi di sisi provider
//! (`FindAllBuildCache` + `CacheRequest`), bukan dengan iterasi pohon di sisi klien.

use super::snap::pick_best;
use super::types::{ControlInfo, ControlKind, UiaError};
use crate::automation::element_finder::PixelRect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use windows::core::{Interface, BSTR};
use windows::Win32::Foundation::{HWND, RECT, VARIANT_BOOL};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::{
    VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_I4,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomation6, IUIAutomationCacheRequest,
    IUIAutomationCondition, IUIAutomationElement, IUIAutomationExpandCollapsePattern,
    IUIAutomationInvokePattern, IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern,
    IUIAutomationValuePattern, TreeScope_Descendants, UIA_AutomationIdPropertyId,
    UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_ExpandCollapsePatternId,
    UIA_InvokePatternId, UIA_IsControlElementPropertyId, UIA_IsEnabledPropertyId,
    UIA_IsOffscreenPropertyId, UIA_NamePropertyId, UIA_PATTERN_ID, UIA_ProcessIdPropertyId,
    UIA_PROPERTY_ID, UIA_SelectionItemPatternId, UIA_TogglePatternId, UIA_ValuePatternId,
};

/// Batas elemen per scan (sama dengan UFO: `min(com_elem_array.Length, 500)`).
const MAX_ELEMENTS: i32 = 500;
/// Batas keras panggilan lintas-proses (butuh Win10 1809+ / IUIAutomation6).
const CONNECTION_TIMEOUT_MS: u32 = 1000;
const TRANSACTION_TIMEOUT_MS: u32 = 1500;

fn variant_bool(b: bool) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BOOL,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    boolVal: VARIANT_BOOL(if b { -1 } else { 0 }),
                },
            }),
        },
    }
}

fn variant_i32(v: i32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    lVal: v,
                },
            }),
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HRESULT → UiaError
// ─────────────────────────────────────────────────────────────────────────────

pub(super) fn classify(e: &windows::core::Error) -> UiaError {
    match e.code().0 as u32 {
        0x8004_0200 => UiaError::NotEnabled,                  // UIA_E_ELEMENTNOTENABLED
        0x8004_0201 | 0x8001_0108 => UiaError::StaleElement,  // UIA_E_ELEMENTNOTAVAILABLE / RPC_E_DISCONNECTED
        0x8013_1505 => UiaError::Timeout("uia"),              // UIA_E_TIMEOUT
        0x8007_0005 => UiaError::ElevatedTarget,              // E_ACCESSDENIED
        c @ (0x8001_0001 | 0x8001_0105 | 0x8001_010A | 0x8007_06BA | 0x8007_06BE) => {
            UiaError::ProviderUnavailable(c as i32)           // call rejected / server fault / retry-later / RPC down
        }
        c => UiaError::Com(c as i32),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Konteks UIA (dibuat sekali per thread aktor, dipakai ulang)
// ─────────────────────────────────────────────────────────────────────────────

pub(super) struct UiaContext {
    uia: IUIAutomation,
    cache: IUIAutomationCacheRequest,
    interactive: IUIAutomationCondition,
}

/// Hasil scan: DTO + elemen COM sejajar per indeks. Jangan pernah dikirim antar-thread.
pub(super) struct Scan {
    pub infos: Vec<ControlInfo>,
    pub elements: Vec<IUIAutomationElement>,
}

fn prop_cond(
    uia: &IUIAutomation,
    id: UIA_PROPERTY_ID,
    v: &VARIANT,
) -> Result<IUIAutomationCondition, UiaError> {
    // SAFETY: pemanggilan COM dengan argumen valid pada thread MTA.
    unsafe { uia.CreatePropertyCondition(id, v) }.map_err(|e| classify(&e))
}

/// IsEnabled ∧ ¬IsOffscreen ∧ IsControlElement ∧ (ControlType ∈ kinds) — sama dengan
/// `_get_control_filter_condition` UFO, tetapi OR-chain dibangun dengan `CreateOrCondition`
/// berlapis (menghindari SAFEARRAY).
fn build_interactive_condition(
    uia: &IUIAutomation,
    kinds: &[ControlKind],
) -> Result<IUIAutomationCondition, UiaError> {
    let enabled_v = variant_bool(true);
    let enabled = prop_cond(uia, UIA_IsEnabledPropertyId, &enabled_v)?;
    let onscreen_v = variant_bool(false);
    let onscreen = prop_cond(uia, UIA_IsOffscreenPropertyId, &onscreen_v)?;
    let is_control_v = variant_bool(true);
    let is_control = prop_cond(uia, UIA_IsControlElementPropertyId, &is_control_v)?;

    let mut types: Option<IUIAutomationCondition> = None;
    for k in kinds {
        let type_v = variant_i32(k.uia_id());
        let c = prop_cond(uia, UIA_ControlTypePropertyId, &type_v)?;
        types = Some(match types {
            None => c,
            // SAFETY: lihat prop_cond.
            Some(prev) => unsafe { uia.CreateOrCondition(&prev, &c) }.map_err(|e| classify(&e))?,
        });
    }
    let types = types.ok_or_else(|| UiaError::Internal("daftar ControlKind kosong".into()))?;

    // SAFETY: lihat prop_cond.
    unsafe {
        let a = uia.CreateAndCondition(&enabled, &onscreen).map_err(|e| classify(&e))?;
        let b = uia.CreateAndCondition(&a, &is_control).map_err(|e| classify(&e))?;
        uia.CreateAndCondition(&b, &types).map_err(|e| classify(&e))
    }
}

fn build_cache(uia: &IUIAutomation) -> Result<IUIAutomationCacheRequest, UiaError> {
    // SAFETY: lihat prop_cond.
    unsafe {
        let cache = uia.CreateCacheRequest().map_err(|e| classify(&e))?;
        for p in [
            UIA_ControlTypePropertyId,
            UIA_NamePropertyId,
            UIA_BoundingRectanglePropertyId,
            UIA_AutomationIdPropertyId,
            UIA_ProcessIdPropertyId,
        ] {
            cache.AddProperty(p).map_err(|e| classify(&e))?;
        }
        Ok(cache)
    }
}

fn sanitize(r: RECT) -> PixelRect {
    PixelRect::new(r.left, r.top, r.right.max(r.left), r.bottom.max(r.top))
}

fn read_cached(el: &IUIAutomationElement) -> Option<ControlInfo> {
    // SAFETY: getter cache; gagal → None (elemen dilewati).
    unsafe {
        let kind = ControlKind::from_uia_id(el.CachedControlType().ok()?.0)?;
        let rect = sanitize(el.CachedBoundingRectangle().ok()?);
        let name = el.CachedName().map(|b| b.to_string()).unwrap_or_default();
        let automation_id = el.CachedAutomationId().map(|b| b.to_string()).unwrap_or_default();
        let pid = el.CachedProcessId().ok()?.max(0) as u32;
        Some(ControlInfo {
            kind,
            name,
            automation_id,
            rect,
            pid,
        })
    }
}

impl UiaContext {
    pub(super) fn new() -> Result<Self, UiaError> {
        // SAFETY: thread pemanggil sudah CoInitializeEx(MTA) oleh aktor.
        let uia: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| UiaError::Unavailable(format!("CoCreateInstance(CUIAutomation8): {e}")))?;

        // Batas keras agar thread aktor tidak menggantung selamanya pada provider yang hang.
        // Windows < 10 1809 tidak punya IUIAutomation6 → lanjut tanpa; handle tetap punya deadline.
        if let Ok(u6) = uia.cast::<IUIAutomation6>() {
            // SAFETY: setter sederhana.
            unsafe {
                let _ = u6.SetConnectionTimeout(CONNECTION_TIMEOUT_MS);
                let _ = u6.SetTransactionTimeout(TRANSACTION_TIMEOUT_MS);
            }
        }

        let interactive = build_interactive_condition(&uia, &ControlKind::DEFAULT_INTERACTIVE)?;
        let cache = build_cache(&uia)?;
        Ok(Self {
            uia,
            cache,
            interactive,
        })
    }

    /// Satu panggilan lintas-proses mengembalikan semua kontrol interaktif jendela (≤ 500).
    pub(super) fn scan_window(
        &self,
        hwnd: usize,
        deadline: Instant,
        cancel: &AtomicBool,
    ) -> Result<Scan, UiaError> {
        // SAFETY: semua pemanggilan COM pada thread MTA pemilik objek.
        unsafe {
            let root = self
                .uia
                .ElementFromHandle(HWND(hwnd as *mut _))
                .map_err(|e| classify(&e))?;
            let arr = root
                .FindAllBuildCache(TreeScope_Descendants, &self.interactive, &self.cache)
                .map_err(|e| classify(&e))?;
            let len = arr.Length().map_err(|e| classify(&e))?.clamp(0, MAX_ELEMENTS);

            let mut infos = Vec::with_capacity(len as usize);
            let mut elements = Vec::with_capacity(len as usize);
            for i in 0..len {
                if cancel.load(Ordering::Relaxed) {
                    return Err(UiaError::Cancelled);
                }
                if Instant::now() >= deadline {
                    return Err(UiaError::Timeout("scan"));
                }
                let Ok(el) = arr.GetElement(i) else { continue };
                if let Some(info) = read_cached(&el) {
                    infos.push(info);
                    elements.push(el);
                }
            }
            Ok(Scan { infos, elements })
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Revalidasi (pertahanan stale element) — dipanggil tepat sebelum aksi
// ─────────────────────────────────────────────────────────────────────────────

pub(super) fn revalidate(info: &ControlInfo, el: &IUIAutomationElement) -> Result<(), UiaError> {
    const TOL: i64 = 3;
    // SAFETY: getter live (non-cache) pada elemen milik thread ini.
    unsafe {
        let live = el.CurrentBoundingRectangle().map_err(|e| classify(&e))?;
        if el.CurrentIsOffscreen().map_err(|e| classify(&e))?.as_bool() {
            return Err(UiaError::StaleElement);
        }
        if !el.CurrentIsEnabled().map_err(|e| classify(&e))?.as_bool() {
            return Err(UiaError::NotEnabled);
        }
        let moved = (live.left as i64 - info.rect.left as i64).abs() > TOL
            || (live.top as i64 - info.rect.top as i64).abs() > TOL
            || (live.right as i64 - info.rect.right as i64).abs() > TOL
            || (live.bottom as i64 - info.rect.bottom as i64).abs() > TOL;
        if moved {
            return Err(UiaError::StaleElement);
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Eksekusi semantik (hybrid tier-1)
// ─────────────────────────────────────────────────────────────────────────────

pub(super) enum Semantic {
    Done(&'static str),
    /// Timeout di tengah aksi → mungkin sudah terjadi. Pemanggil DILARANG fallback ke klik fisik.
    Unconfirmed(&'static str),
    NotApplicable,
    /// Pattern ada tetapi gagal tanpa efek yang diketahui → boleh fallback fisik.
    Failed,
}

fn pattern<T: Interface>(el: &IUIAutomationElement, id: UIA_PATTERN_ID) -> windows::core::Result<T> {
    // Pattern tidak didukung → GetCurrentPattern mengembalikan pointer null → Err.
    // SAFETY: pemanggilan COM pada thread pemilik elemen.
    unsafe { el.GetCurrentPattern(id)?.cast::<T>() }
}

type Attempt = (&'static str, fn(&IUIAutomationElement) -> windows::core::Result<()>);

fn invoke(el: &IUIAutomationElement) -> windows::core::Result<()> {
    // SAFETY: lihat `pattern`.
    unsafe { pattern::<IUIAutomationInvokePattern>(el, UIA_InvokePatternId)?.Invoke() }
}
fn toggle(el: &IUIAutomationElement) -> windows::core::Result<()> {
    // SAFETY: lihat `pattern`.
    unsafe { pattern::<IUIAutomationTogglePattern>(el, UIA_TogglePatternId)?.Toggle() }
}
fn select(el: &IUIAutomationElement) -> windows::core::Result<()> {
    // SAFETY: lihat `pattern`.
    unsafe { pattern::<IUIAutomationSelectionItemPattern>(el, UIA_SelectionItemPatternId)?.Select() }
}
fn expand(el: &IUIAutomationElement) -> windows::core::Result<()> {
    // SAFETY: lihat `pattern`.
    unsafe { pattern::<IUIAutomationExpandCollapsePattern>(el, UIA_ExpandCollapsePatternId)?.Expand() }
}

const CHAIN_INVOKE: &[Attempt] = &[("invoke", invoke)];
const CHAIN_CHECK: &[Attempt] = &[("toggle", toggle), ("invoke", invoke)];
const CHAIN_SELECT: &[Attempt] = &[("select", select), ("invoke", invoke)];
const CHAIN_LISTITEM: &[Attempt] = &[("invoke", invoke), ("select", select)];
const CHAIN_COMBO: &[Attempt] = &[("expand", expand)];
const CHAIN_NONE: &[Attempt] = &[];

fn chain_for(kind: ControlKind) -> &'static [Attempt] {
    match kind {
        ControlKind::Button | ControlKind::SplitButton | ControlKind::Hyperlink | ControlKind::MenuItem => {
            CHAIN_INVOKE
        }
        ControlKind::CheckBox => CHAIN_CHECK,
        ControlKind::RadioButton | ControlKind::TabItem | ControlKind::TreeItem | ControlKind::DataItem => {
            CHAIN_SELECT
        }
        ControlKind::ListItem => CHAIN_LISTITEM,
        ControlKind::ComboBox => CHAIN_COMBO,
        // Klik pada Edit = taruh caret di titik tertentu → biarkan SendInput.
        ControlKind::Edit => CHAIN_NONE,
    }
}

pub(super) fn activate_semantic(el: &IUIAutomationElement, kind: ControlKind) -> Result<Semantic, UiaError> {
    for (label, run) in chain_for(kind) {
        match run(el) {
            Ok(()) => return Ok(Semantic::Done(label)),
            Err(e) => match classify(&e) {
                UiaError::Timeout(_) => return Ok(Semantic::Unconfirmed(label)),
                fatal @ (UiaError::StaleElement | UiaError::NotEnabled | UiaError::ElevatedTarget) => {
                    return Err(fatal)
                }
                _ => continue, // pattern tidak didukung / gagal generik → coba berikutnya
            },
        }
    }
    Ok(Semantic::NotApplicable)
}

/// `ValuePattern.SetValue` = MENGGANTI seluruh isi (bukan menyisipkan di caret).
/// Dengan verifikasi read-back (dilewati untuk field password agar nilai tidak dibaca).
pub(super) fn set_value_semantic(el: &IUIAutomationElement, text: &str) -> Result<Semantic, UiaError> {
    let Ok(vp) = pattern::<IUIAutomationValuePattern>(el, UIA_ValuePatternId) else {
        return Ok(Semantic::NotApplicable);
    };
    // SAFETY: pemanggilan COM pada thread pemilik elemen.
    unsafe {
        if vp.CurrentIsReadOnly().map(|b| b.as_bool()).unwrap_or(true) {
            return Ok(Semantic::NotApplicable);
        }
        if let Err(e) = vp.SetValue(&BSTR::from(text)) {
            return match classify(&e) {
                UiaError::Timeout(_) => Ok(Semantic::Unconfirmed("value")),
                fatal @ (UiaError::StaleElement | UiaError::NotEnabled | UiaError::ElevatedTarget) => Err(fatal),
                _ => Ok(Semantic::Failed),
            };
        }
        let is_password = el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false);
        if !is_password {
            match vp.CurrentValue() {
                Ok(v) if v.to_string() == text => {}
                _ => return Ok(Semantic::Failed), // aplikasi mengabaikan SetValue (mis. React controlled input)
            }
        }
    }
    Ok(Semantic::Done("value"))
}

/// Pilih kontrol bertipe tertentu saja (mis. Edit/ComboBox untuk set-text).
pub(super) fn pick_among(
    scan: &Scan,
    allowed: &[ControlKind],
    q: &super::snap::SnapQuery,
) -> Option<usize> {
    let idx: Vec<usize> = scan
        .infos
        .iter()
        .enumerate()
        .filter(|(_, c)| allowed.contains(&c.kind))
        .map(|(i, _)| i)
        .collect();
    let subset: Vec<ControlInfo> = idx.iter().filter_map(|&i| scan.infos.get(i).cloned()).collect();
    let m = pick_best(&subset, q)?;
    idx.get(m.index).copied()
}
