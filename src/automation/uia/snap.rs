//! Smart Snap (magnet) — fungsi MURNI tanpa COM/Win32.
//!
//! Aturan pemilihan kandidat:
//! 1. Kontrol yang MENGANDUNG titik menang atas yang hanya "dekat".
//! 2. Di antara yang mengandung titik: area terkecil menang (paling spesifik,
//!    mis. Button di dalam ListItem).
//! 3. Di antara yang tidak mengandung: jarak ke tepi terdekat (≤ radius), lalu area terkecil.
//! 4. Seri penuh → indeks terkecil (deterministik).
//! 5. Jika model memberi box (bukan titik), kandidat > `MAX_AREA_RATIO`× luas box ditolak,
//!    supaya box kecil tidak "ditelan" ListItem/kontainer besar.

use super::types::ControlInfo;
use crate::automation::element_finder::PixelRect;

const MAX_AREA_RATIO: i64 = 6;

#[derive(Debug, Clone, Copy)]
pub struct SnapQuery {
    pub px: i32,
    pub py: i32,
    pub radius_px: i32,
    /// Luas box dari model dalam piksel (None untuk titik tunggal)
    pub hint_area_px: Option<i64>,
    pub screen_w: i32,
    pub screen_h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapMatch {
    pub index: usize,
    pub distance_px: f64,
    pub contained: bool,
}

/// Radius snap default: 4% tinggi layar, dibatasi 24..=96 px.
pub fn default_radius_px(screen_h: i32) -> i32 {
    (((screen_h.max(1) as f64) * 0.04).round() as i32).clamp(24, 96)
}

/// `RECT` Win32 bersifat right/bottom-eksklusif.
pub fn contains(r: &PixelRect, px: i32, py: i32) -> bool {
    px >= r.left && px < r.right && py >= r.top && py < r.bottom
}

/// Jarak Euclidean dari titik ke tepi persegi (0 jika di dalam).
pub fn rect_distance(r: &PixelRect, px: i32, py: i32) -> f64 {
    let dx = if px < r.left {
        r.left as i64 - px as i64
    } else if px >= r.right {
        px as i64 - (r.right as i64 - 1)
    } else {
        0
    };
    let dy = if py < r.top {
        r.top as i64 - py as i64
    } else if py >= r.bottom {
        py as i64 - (r.bottom as i64 - 1)
    } else {
        0
    };
    (dx as f64).hypot(dy as f64)
}

fn is_better(c: &SnapMatch, c_area: i64, b: &SnapMatch, b_area: i64) -> bool {
    match (c.contained, b.contained) {
        (true, false) => true,
        (false, true) => false,
        (true, true) => c_area < b_area,
        (false, false) => {
            let d = c.distance_px - b.distance_px;
            d < -0.5 || (d.abs() <= 0.5 && c_area < b_area)
        }
    }
}

/// Pilih kontrol interaktif terbaik untuk titik estimasi model.
pub fn pick_best(controls: &[ControlInfo], q: &SnapQuery) -> Option<SnapMatch> {
    let mut best: Option<(SnapMatch, i64)> = None;

    for (index, c) in controls.iter().enumerate() {
        // PixelRect::width() memakai pengurangan i32 biasa → bisa overflow pada rect sampah
        // (mis. elemen offscreen dengan koordinat i32::MIN). Saring dulu dengan aritmetika i64.
        let w = c.rect.right as i64 - c.rect.left as i64;
        let h = c.rect.bottom as i64 - c.rect.top as i64;
        if w <= 0 || h <= 0 || w > q.screen_w as i64 * 2 || h > q.screen_h as i64 * 2 {
            continue;
        }
        if !c.rect.is_valid_control(q.screen_w, q.screen_h) {
            continue; // terlalu kecil / terlalu besar / terbalik
        }
        let area = c.area();
        if let Some(hint) = q.hint_area_px {
            if hint > 0 && area > hint.saturating_mul(MAX_AREA_RATIO) {
                continue;
            }
        }

        let contained = contains(&c.rect, q.px, q.py);
        let distance_px = if contained {
            0.0
        } else {
            rect_distance(&c.rect, q.px, q.py)
        };
        if !contained && distance_px > q.radius_px as f64 {
            continue;
        }

        let cand = SnapMatch {
            index,
            distance_px,
            contained,
        };
        let better = match &best {
            None => true,
            Some((b, b_area)) => is_better(&cand, area, b, *b_area),
        };
        if better {
            best = Some((cand, area));
        }
    }

    best.map(|(m, _)| m)
}

// ─────────────────────────────────────────────────────────────────────────────
// Port `TextControlFilter` UFO (tanpa sentence-transformers / ikon)
// ─────────────────────────────────────────────────────────────────────────────

/// Kata kunci dari rencana/instruksi: huruf-angka, lowercase, ≥ 2 karakter.
#[allow(dead_code)]
pub fn plan_keywords(plan: &str) -> Vec<String> {
    plan.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| w.chars().count() >= 2)
        .collect()
}

/// Indeks kontrol yang namanya cocok dengan kata kunci instruksi.
/// Beda dengan UFO: nama kosong / 1 karakter TIDAK dianggap cocok (di UFO `"" in keyword`
/// selalu True sehingga kontrol tanpa nama lolos semua).
#[allow(dead_code)]
pub fn text_filter(controls: &[ControlInfo], plan: &str) -> Vec<usize> {
    let kws = plan_keywords(plan);
    controls
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            let n = c.name.trim().to_lowercase();
            n.chars().count() >= 2
                && kws
                    .iter()
                    .any(|k| n.contains(k.as_str()) || k.contains(n.as_str()))
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::uia::types::ControlKind;

    const SW: i32 = 1920;
    const SH: i32 = 1080;

    fn ctl(kind: ControlKind, l: i32, t: i32, r: i32, b: i32) -> ControlInfo {
        ControlInfo {
            kind,
            name: String::new(),
            automation_id: String::new(),
            rect: PixelRect::new(l, t, r, b),
            pid: 1,
        }
    }

    fn q(px: i32, py: i32) -> SnapQuery {
        SnapQuery {
            px,
            py,
            radius_px: 48,
            hint_area_px: None,
            screen_w: SW,
            screen_h: SH,
        }
    }

    #[test]
    fn smallest_containing_wins() {
        let list = ctl(ControlKind::ListItem, 100, 100, 900, 160);
        let btn = ctl(ControlKind::Button, 800, 110, 880, 150);
        let m = pick_best(&[list, btn], &q(840, 130)).unwrap();
        assert_eq!(m.index, 1);
        assert!(m.contained);
    }

    #[test]
    fn nearest_within_radius() {
        let btn = ctl(ControlKind::Button, 100, 100, 200, 140);
        let m = pick_best(&[btn], &q(215, 120)).unwrap();
        assert!(!m.contained);
        assert!((m.distance_px - 16.0).abs() < 1e-9);
    }

    #[test]
    fn outside_radius_is_none() {
        let btn = ctl(ControlKind::Button, 100, 100, 200, 140);
        assert!(pick_best(&[btn], &q(400, 120)).is_none());
    }

    #[test]
    fn fullscreen_container_rejected() {
        let root = ctl(ControlKind::ListItem, 0, 0, 1920, 1080);
        assert!(pick_best(&[root], &q(500, 500)).is_none());
    }

    #[test]
    fn hint_area_rejects_oversized_candidate() {
        let list = ctl(ControlKind::ListItem, 100, 100, 900, 160); // 48000 px²
        let mut query = q(300, 130);
        query.hint_area_px = Some(1000);
        assert!(pick_best(&[list], &query).is_none());
    }

    #[test]
    fn tie_is_deterministic() {
        let a = ctl(ControlKind::Button, 100, 100, 200, 140);
        let b = ctl(ControlKind::Button, 100, 100, 200, 140);
        assert_eq!(pick_best(&[a, b], &q(150, 120)).unwrap().index, 0);
    }

    #[test]
    fn garbage_rects_do_not_panic() {
        let g = ctl(ControlKind::Button, i32::MIN, i32::MIN, i32::MAX, i32::MAX);
        let _ = pick_best(&[g], &q(10, 10));
    }

    #[test]
    fn distance_to_corner() {
        let r = PixelRect::new(100, 100, 200, 140);
        assert!((rect_distance(&r, 203, 143) - 32f64.sqrt()).abs() < 1e-9);
        assert_eq!(rect_distance(&r, 150, 120), 0.0);
    }

    #[test]
    fn radius_scales_and_clamps() {
        assert_eq!(default_radius_px(1080), 43);
        assert_eq!(default_radius_px(2160), 86);
        assert_eq!(default_radius_px(400), 24);
        assert_eq!(default_radius_px(10_000), 96);
    }

    #[test]
    fn text_filter_matches_names_not_blanks() {
        let mut send = ctl(ControlKind::Button, 0, 0, 50, 20);
        send.name = "Kirim".into();
        let mut cancel = ctl(ControlKind::Button, 60, 0, 110, 20);
        cancel.name = "Batal".into();
        let blank = ctl(ControlKind::Button, 120, 0, 170, 20);
        let hits = text_filter(&[send, cancel, blank], "Klik tombol kirim email.");
        assert_eq!(hits, vec![0]);
    }
}
