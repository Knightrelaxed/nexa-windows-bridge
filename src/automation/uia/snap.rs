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

use super::types::{ControlInfo, ControlKind};
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

/// Radius snap default: 2% tinggi layar, dibatasi 16..=40 px.
pub fn default_radius_px(screen_h: i32) -> i32 {
    (((screen_h.max(1) as f64) * 0.02).round() as i32).clamp(16, 40)
}

/// Pilih kontrol yang paling cocok dengan bounding box dari visi berdasarkan IoU (Intersection-over-Union >= 0.3).
pub fn pick_by_box(controls: &[ControlInfo], bx: &PixelRect, sw: i32, sh: i32) -> Option<usize> {
    let ba = bx.width() as i64 * bx.height() as i64;
    controls.iter().enumerate()
        .filter(|(_, c)| c.rect.is_valid_control(sw, sh))
        .filter_map(|(i, c)| {
            let l = c.rect.left.max(bx.left) as i64;
            let t = c.rect.top.max(bx.top) as i64;
            let r = c.rect.right.min(bx.right) as i64;
            let b = c.rect.bottom.min(bx.bottom) as i64;
            let inter = (r - l).max(0) * (b - t).max(0);
            let uni = ba + c.area() - inter;
            let iou = if uni > 0 { inter as f64 / uni as f64 } else { 0.0 };
            (iou >= 0.3).then_some((i, iou))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
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

// ─────────────────────────────────────────────────────────────────────────────
// Disambiguasi Semantik Lokal (Target Hinting - Claude 4 Audit)
// ─────────────────────────────────────────────────────────────────────────────

/// Bersihkan teks hint dari aksentuasi, shortcut keyboard di akhir (Ctrl+...), tanda &, dll.
pub fn normalize_hint(s: &str) -> String {
    let lower = s.to_lowercase();
    let trimmed = lower.trim();
    let base = if let Some(idx) = trimmed.rfind('(') {
        if trimmed.ends_with(')') {
            &trimmed[..idx]
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    let cleaned = base
        .replace('&', "")
        .replace("...", "")
        .replace('…', "");
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Jaro-Winkler similarity (0.0 ..= 1.0) murni tanpa dependensi eksternal
pub fn jaro_winkler_similarity(s1: &str, s2: &str) -> f32 {
    if s1 == s2 {
        return 1.0;
    }
    if s1.is_empty() || s2.is_empty() {
        return 0.0;
    }

    let c1: Vec<char> = s1.chars().collect();
    let c2: Vec<char> = s2.chars().collect();
    let len1 = c1.len();
    let len2 = c2.len();

    let match_distance = (len1.max(len2) / 2).saturating_sub(1);
    let mut s1_matches = vec![false; len1];
    let mut s2_matches = vec![false; len2];

    let mut matches = 0usize;
    for i in 0..len1 {
        let start = i.saturating_sub(match_distance);
        let end = (i + match_distance + 1).min(len2);
        for j in start..end {
            if s2_matches[j] || c1[i] != c2[j] {
                continue;
            }
            s1_matches[i] = true;
            s2_matches[j] = true;
            matches += 1;
            break;
        }
    }

    if matches == 0 {
        return 0.0;
    }

    let mut transpositions = 0usize;
    let mut k = 0usize;
    for i in 0..len1 {
        if !s1_matches[i] {
            continue;
        }
        while !s2_matches[k] {
            k += 1;
        }
        if c1[i] != c2[k] {
            transpositions += 1;
        }
        k += 1;
    }

    let m = matches as f32;
    let jaro = (m / len1 as f32 + m / len2 as f32 + (m - (transpositions / 2) as f32) / m) / 3.0;

    let prefix = c1
        .iter()
        .zip(c2.iter())
        .take(4)
        .take_while(|(a, b)| a == b)
        .count() as f32;

    jaro + prefix * 0.1 * (1.0 - jaro)
}

/// Hitung kemiripan teks kontekstual (Exact, Containment, atau Jaro-Winkler)
pub fn text_similarity(hint: &str, candidate: &str) -> f32 {
    let h = normalize_hint(hint);
    let c = normalize_hint(candidate);
    if h.is_empty() || c.is_empty() {
        return 0.0;
    }
    if h == c {
        return 1.0;
    }
    if c.contains(&h) || h.contains(&c) {
        return 0.85;
    }
    jaro_winkler_similarity(&h, &c)
}

#[derive(Debug, Clone, PartialEq)]
pub struct AmbiguousCandidate {
    pub index: usize,
    pub name: String,
    pub kind: ControlKind,
    pub distance_px: f32,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HintedPick {
    None,
    Chosen {
        index: usize,
        sim: f32,
        contained: bool,
    },
    Ambiguous(Vec<AmbiguousCandidate>),
    Vetoed {
        best_name: String,
        best_sim: f32,
    },
}

/// Disambiguasi semantik cerdas (Re-Ranker) kontrol interaktif berdasarkan hint teks.
pub fn pick_hinted(controls: &[ControlInfo], q: &SnapQuery, hint: &str) -> HintedPick {
    let h = normalize_hint(hint);
    if h.is_empty() {
        return match pick_best(controls, q) {
            Some(m) => HintedPick::Chosen {
                index: m.index,
                sim: 0.0,
                contained: m.contained,
            },
            None => HintedPick::None,
        };
    }

    let reach = ((q.radius_px as f32) * 2.0).min(160.0);
    // (index, cost, sim, contained, named, name, kind, distance)
    let mut candidates: Vec<(usize, f32, f32, bool, bool, String, ControlKind, f32)> = Vec::new();

    for (i, ctl) in controls.iter().enumerate() {
        let w = ctl.rect.right as i64 - ctl.rect.left as i64;
        let h_px = ctl.rect.bottom as i64 - ctl.rect.top as i64;
        if w <= 0 || h_px <= 0 || w > q.screen_w as i64 * 2 || h_px > q.screen_h as i64 * 2 {
            continue;
        }
        if !ctl.rect.is_valid_control(q.screen_w, q.screen_h) {
            continue;
        }

        let contained = contains(&ctl.rect, q.px, q.py);
        let d = if contained {
            0.0
        } else {
            rect_distance(&ctl.rect, q.px, q.py) as f32
        };
        if d > reach {
            continue;
        }

        let n = normalize_hint(&ctl.name);
        let named = !n.is_empty();
        let sim = if named { text_similarity(&h, &n) } else { 0.0 };

        // Di luar radius asli, hanya nama yang cocok kuat (sim >= 0.5) yang diizinkan menarik titik
        if !contained && sim < 0.5 && d > q.radius_px as f32 {
            continue;
        }

        let spatial = if contained { 0.0 } else { 0.25 + (d / reach) };
        let textual = if named { 1.0 - sim } else { 1.0 };
        let cost = 0.5 * spatial + 0.5 * textual;

        candidates.push((i, cost, sim, contained, named, ctl.name.clone(), ctl.kind, d));
    }

    // Ada kandidat bernama yang layak -> buang yang anonim (tak ada bukti kecocokan dengan hint)
    if candidates.iter().any(|c| c.4 && c.2 >= 0.5) {
        candidates.retain(|c| c.4);
    }

    candidates.sort_by(|a, b| a.1.total_cmp(&b.1));

    match candidates.as_slice() {
        [] => HintedPick::None,
        [b, ..] if b.4 && b.2 < 0.5 => HintedPick::Vetoed {
            best_name: b.5.clone(),
            best_sim: b.2,
        },
        [b, s, ..] if (s.1 - b.1).abs() < 0.08 => {
            let top3 = candidates
                .iter()
                .take(3)
                .map(|c| AmbiguousCandidate {
                    index: c.0,
                    name: c.5.clone(),
                    kind: c.6,
                    distance_px: c.7,
                    score: c.1,
                })
                .collect();
            HintedPick::Ambiguous(top3)
        }
        [b, ..] => HintedPick::Chosen {
            index: b.0,
            sim: b.2,
            contained: b.3,
        },
    }
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
        assert_eq!(default_radius_px(1080), 22);
        assert_eq!(default_radius_px(2160), 40);
        assert_eq!(default_radius_px(400), 16);
        assert_eq!(default_radius_px(10_000), 40);
    }

    #[test]
    fn test_pick_by_box_iou() {
        let btn = ctl(ControlKind::Button, 100, 100, 200, 150);
        let icon = ctl(ControlKind::Button, 105, 105, 125, 125);
        let controls = vec![btn, icon];

        // Query box closely matches btn (100, 100, 200, 150)
        let bx_btn = PixelRect::new(98, 98, 202, 152);
        assert_eq!(pick_by_box(&controls, &bx_btn, SW, SH), Some(0));

        // Query box too far (IoU < 0.3)
        let bx_far = PixelRect::new(50, 50, 80, 80);
        assert_eq!(pick_by_box(&controls, &bx_far, SW, SH), None);
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

    #[test]
    fn test_normalize_hint_and_similarity() {
        assert_eq!(normalize_hint("Search (Ctrl+Shift+F)"), "search");
        assert_eq!(normalize_hint("&File..."), "file");
        assert_eq!(normalize_hint("  Save & As…  "), "save as");

        let sim_exact = text_similarity("search", "search");
        assert_eq!(sim_exact, 1.0);

        let sim_sub = text_similarity("run", "run and debug");
        assert!(sim_sub >= 0.8);

        let sim_diff = text_similarity("login", "search");
        assert!(sim_diff < 0.3);
    }

    #[test]
    fn test_pick_hinted_disambiguation_and_veto() {
        let mut btn_cancel = ctl(ControlKind::Button, 100, 100, 150, 140);
        btn_cancel.name = "Cancel".into();
        let mut btn_submit = ctl(ControlKind::Button, 160, 100, 210, 140);
        btn_submit.name = "Submit".into();

        // Point is closer to cancel (145, 120), but hint is "Submit" -> Submit wins!
        let pick = pick_hinted(&[btn_cancel.clone(), btn_submit.clone()], &q(145, 120), "Submit");
        match pick {
            HintedPick::Chosen { index, sim, .. } => {
                assert_eq!(index, 1);
                assert!(sim >= 0.9);
            }
            _ => panic!("Expected Submit to be chosen"),
        }

        // Veto mismatch when hint is "Print" (both Cancel and Submit have sim < 0.5)
        let veto = pick_hinted(&[btn_cancel, btn_submit], &q(120, 120), "Print");
        match veto {
            HintedPick::Vetoed { best_sim, .. } => {
                assert!(best_sim < 0.5);
            }
            _ => panic!("Expected veto on mismatch"),
        }
    }

    #[test]
    fn unnamed_container_does_not_beat_exact_named_match() {
        let unnamed = ctl(ControlKind::Button, 100, 100, 200, 140); // mengandung titik
        let mut send = ctl(ControlKind::Button, 230, 100, 290, 140); // ±80 px dari titik
        send.name = "Send".into();
        match pick_hinted(&[unnamed, send], &q(150, 120), "Send") {
            HintedPick::Chosen { index, .. } => assert_eq!(index, 1),
            other => panic!("expected Send, got {:?}", other),
        }
    }
}
