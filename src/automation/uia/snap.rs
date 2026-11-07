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
/// Selisih minimum (px) antara titik model dan batas bawah pita chrome browser agar titik itu
/// dianggap "di konten halaman" (bukan di tab strip/toolbar).
const CHROME_BAND_MARGIN: i32 = 20;
/// Penalti biaya untuk kontainer baris (ListItem/DataItem) vs kontrol spesifik bernama sama.
const CONTAINER_PENALTY: f32 = 0.05;
/// Hyperlink lebih tinggi dari ini dianggap blok multi-baris (judul + URL + snippet).
const MULTILINE_LINK_HEIGHT: i64 = 64;
/// Offset (px) dari tepi atas blok multi-baris ke baris judul.
const FIRST_LINE_OFFSET: i64 = 20;

#[derive(Debug, Clone, Copy)]
pub struct SnapQuery {
    pub px: i32,
    pub py: i32,
    pub radius_px: i32,
    /// Luas box dari model dalam piksel (None untuk titik tunggal)
    pub hint_area_px: Option<i64>,
    pub screen_w: i32,
    pub screen_h: i32,
    /// Batas bawah (px layar) pita chrome browser (tab strip + address bar + bookmark bar) pada
    /// jendela browser; 0 = nonaktif (aplikasi lain).
    pub chrome_band_bottom: i32,
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

/// Titik klik fisik untuk kontrol hasil snap (piksel layar).
/// - `Image`: bila titik model jatuh di dalam gambar, pakai titik model itu (pusat foto besar
///   jarang sama dengan yang dimaksud).
/// - `Hyperlink` multi-baris (tinggi > 64px, mis. blok hasil Google berisi judul + URL + snippet):
///   bidik baris pertama (judul). Pusat blok sering jatuh di snippet yang BUKAN bagian `<a>`.
/// - Lainnya: pusat kontrol.
pub fn click_point(c: &ControlInfo, px: i32, py: i32) -> (i32, i32) {
    let (cx, cy) = c.center();
    let h = c.rect.bottom as i64 - c.rect.top as i64;
    match c.kind {
        ControlKind::Image if contains(&c.rect, px, py) => (px, py),
        ControlKind::Hyperlink if h > MULTILINE_LINK_HEIGHT => {
            let y = (c.rect.top as i64 + FIRST_LINE_OFFSET).min(c.rect.bottom as i64 - 1);
            (cx, y.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
        }
        _ => (cx, cy),
    }
}

/// Pilih kontrol interaktif terbaik untuk titik estimasi model.
pub fn pick_best(controls: &[ControlInfo], q: &SnapQuery) -> Option<SnapMatch> {
    let mut best: Option<(SnapMatch, i64)> = None;

    for (index, c) in controls.iter().enumerate() {
        // Image hanya relevan untuk klik ber-hint (pick_hinted). Tanpa hint, gambar besar akan
        // "menelan" titik model dan klik berpindah ke pusat gambar.
        if c.kind == ControlKind::Image {
            continue;
        }
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

/// Sinonim lintas-bahasa (Inggris <-> Indonesia) untuk elemen UI umum agar hint bahasa Inggris
/// dari model visi tetap cocok dengan label UIA berbahasa Indonesia (atau sebaliknya).
const UI_SYNONYM_GROUPS: &[&[&str]] = &[
    &["search", "cari", "telusuri", "pencarian", "penelusuran", "temukan", "find"],
    &["download", "downloads", "unduh", "unduhan"],
    &["delete", "clear", "remove", "hapus", "bersihkan", "kosongkan"],
    &["close", "tutup", "keluar", "exit"],
    &["back", "kembali", "mundur"],
    &["next", "forward", "lanjut", "selanjutnya", "berikutnya"],
    &["save", "simpan"],
    &["cancel", "batal", "batalkan"],
    &["settings", "setting", "pengaturan", "setelan"],
    &["home", "beranda"],
    &["all", "semua"],
    &["image", "images", "gambar", "foto", "photo", "photos"],
    &["news", "berita"],
];

fn are_ui_synonyms(w1: &str, w2: &str) -> bool {
    UI_SYNONYM_GROUPS
        .iter()
        .any(|g| g.contains(&w1) && g.contains(&w2))
}

/// Hitung kemiripan antara dua kata individu (Exact, Sinonim UI, Substring, atau Jaro-Winkler Typo)
fn word_sim(hw: &str, cw: &str) -> f32 {
    if hw == cw {
        return 1.0;
    }
    if are_ui_synonyms(hw, cw) {
        return 0.95;
    }
    // Substring match between words (e.g. "oogenesi" in "oogenesi_nurul", "download" in "downloads")
    if hw.contains(cw) || cw.contains(hw) {
        let min_len = hw.len().min(cw.len()) as f32;
        let max_len = hw.len().max(cw.len()) as f32;
        if min_len >= 3.0 && min_len / max_len >= 0.6 {
            return 0.85 + 0.15 * (min_len / max_len);
        }
    }
    // Fuzzy match for typo / transliteration:
    // Hanya relevan jika kedua kata memiliki panjang >= 4
    if hw.len() >= 4 && cw.len() >= 4 {
        let jw = jaro_winkler_similarity(hw, cw);
        // Jika huruf pertama berbeda, pasang ambang ketat (>= 0.88) agar rima pendek tak sengaja
        // (seperti "mode" vs "kode" [jw=0.833] atau "code" vs "kode" [jw=0.833]) TIDAK dianggap cocok.
        let same_first_char = hw.chars().next() == cw.chars().next();
        let threshold = if same_first_char { 0.78 } else { 0.88 };
        if jw >= threshold {
            return jw;
        }
    }
    0.0
}

/// Hitung kemiripan teks kontekstual (Exact, Containment, Jaro-Winkler, dan Token-Coverage)
pub fn text_similarity(hint: &str, candidate: &str) -> f32 {
    let h = normalize_hint(hint);
    let c = normalize_hint(candidate);
    if h.is_empty() || c.is_empty() {
        return 0.0;
    }
    if h == c {
        return 1.0;
    }
    // Substring containment:
    // Jika candidate memuat seluruh hint secara utuh (misal link hasil pencarian memuat judul lengkap)
    if c.contains(&h) {
        return 0.95;
    }
    // Jika hint memuat seluruh candidate (misal hint "Downloads folder" vs candidate "Downloads")
    if h.contains(&c) && (c.len() as f32 / h.len() as f32) >= 0.60 {
        return 0.90;
    }

    let min_len = h.len().min(c.len()) as f32;
    let max_len = h.len().max(c.len()) as f32;
    let len_ratio = min_len / max_len;

    // Asymmetric length scaling: Jaro-Winkler menggelembungkan skor jika salah satu string
    // jauh lebih pendek dari yang lain (misal "mode ai" [7 char] vs "alih kode..." [38 char]).
    // Jika rasio panjang < 0.60 dan bukan containment, skala turunkan full_sim agar token coverage
    // yang menentukan kecocokan semantik.
    let full_sim = if len_ratio >= 0.60 {
        jaro_winkler_similarity(&h, &c)
    } else {
        jaro_winkler_similarity(&h, &c) * len_ratio
    };

    // Stop words yang diabaikan saat menghitung coverage kata kunci
    const STOP_WORDS: &[&str] = &[
        "dan", "di", "ke", "dari", "yang", "pada", "untuk", "dengan", "atau", "ini", "itu",
        "the", "and", "or", "in", "on", "at", "to", "for", "of", "with", "by",
        "buka", "klik", "open", "click", "file", "folder", "tombol", "button", "link",
        "bar", "box", "input", "field", "kolom", "kotak", "current", "query", "profile", "profil",
    ];

    // Deduplikasi token agar kata yang berulang di dalam hint (mis. "kode" x2 dan "bahasa" x2
    // pada "Alih Kode dan Campur Kode Bahasa Arab | Deskripsi Bahasa") tidak menggelembungkan
    // skor ketika kata pembeda penting (seperti "Deskripsi" atau "UGM") tidak ada di kandidat.
    let mut h_words: Vec<&str> = Vec::new();
    for w in h
        .split_whitespace()
        .filter(|w| w.len() >= 3 && !STOP_WORDS.contains(w))
    {
        if !h_words.contains(&w) {
            h_words.push(w);
        }
    }
    if h_words.is_empty() {
        for w in h.split_whitespace().filter(|w| w.len() >= 3) {
            if !h_words.contains(&w) {
                h_words.push(w);
            }
        }
    }
    let mut c_words: Vec<&str> = Vec::new();
    for w in c.split_whitespace().filter(|w| w.len() >= 3) {
        if !c_words.contains(&w) {
            c_words.push(w);
        }
    }

    if h_words.is_empty() || c_words.is_empty() {
        return full_sim;
    }

    // Hitung berapa proporsi kata kunci unik hint yang berhasil ditemukan di candidate (Hint Coverage / Recall)
    let mut hint_matches_sum = 0.0f32;
    for hw in &h_words {
        let mut best_m = 0.0f32;
        for cw in &c_words {
            let s = word_sim(hw, cw);
            if s > best_m {
                best_m = s;
            }
        }
        hint_matches_sum += best_m;
    }
    let hint_coverage = hint_matches_sum / (h_words.len() as f32);

    // Hitung berapa proporsi kata unik candidate yang cocok dengan hint (Candidate Coverage / Precision)
    let mut cand_matches_sum = 0.0f32;
    for cw in &c_words {
        if STOP_WORDS.contains(cw) {
            continue;
        }
        let mut best_m = 0.0f32;
        for hw in &h_words {
            let s = word_sim(hw, cw);
            if s > best_m {
                best_m = s;
            }
        }
        cand_matches_sum += best_m;
    }
    let cand_significant_count = c_words.iter().filter(|w| !STOP_WORDS.contains(w)).count();
    let cand_coverage = if cand_significant_count > 0 {
        cand_matches_sum / (cand_significant_count as f32)
    } else {
        0.0
    };

    // Penalti dilusi paragraf panjang: jika kandidat bukan substring utuh (!c.contains(&h)) dan
    // memiliki jauh lebih banyak kata daripada hint (mis. paragraf 50 kata di AI Overview yang
    // kebetulan memuat kata topik secara tersebar), turunkan skor secara proporsional.
    let penalized_hint_cov = if cand_significant_count > h_words.len() + 4 {
        let dilution =
            ((h_words.len() as f32 + 3.0) / (cand_significant_count as f32 + 3.0)).sqrt();
        hint_coverage * dilution
    } else {
        hint_coverage
    };

    let adjusted_hint_cov =
        (penalized_hint_cov * 0.95 + penalized_hint_cov * cand_coverage * 0.05).min(1.0);
    // Jangan izinkan cand_coverage mengatrol skor ke 90% jika hint_coverage rendah (< 0.65),
    // misalnya judul pendek terpotong milik UIN terhadap hint panjang yang meminta UGM.
    let token_sim = if penalized_hint_cov >= 0.65 {
        adjusted_hint_cov.max(cand_coverage * 0.9)
    } else {
        adjusted_hint_cov
    };
    full_sim.max(token_sim)
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

/// Kind "kontainer baris": sering membungkus kontrol yang lebih spesifik (Hyperlink/Button) dengan
/// nama yang sama, atau berupa paragraf panjang. Diberi penalti kecil agar kontrol spesifik menang
/// saat skornya setara, tetapi tetap terpilih bila ia satu-satunya yang cocok (mis. baris Gmail).
fn is_row_container(kind: ControlKind) -> bool {
    matches!(kind, ControlKind::ListItem | ControlKind::DataItem)
}

#[derive(Debug, Clone)]
struct HintCand {
    index: usize,
    cost: f32,
    sim: f32,
    contained: bool,
    named: bool,
    name: String,
    kind: ControlKind,
    distance: f32,
    area: i64,
}

/// Jarak jangkau semantik (px, ke tepi kontrol) menurut kekuatan kecocokan teks:
/// - sim >= 0.85 : jangkauan lebar (koordinat model bisa meleset ratusan px, mis. sidebar/hasil cari)
/// - sim >= 0.65 : jangkauan menengah (kecocokan parsial TIDAK boleh melompat melintasi layar)
/// - lainnya     : radius geometris ketat
fn reach_for_sim(sim: f32, wide_reach: f32, tight_radius: f32) -> f32 {
    if sim >= 0.85 {
        wide_reach
    } else if sim >= 0.65 {
        wide_reach.min(300.0)
    } else {
        tight_radius
    }
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

    // Wide semantic search reach for high-confidence textual matches (up to 500px, e.g. sidebar navigation)
    let wide_reach = ((q.screen_h as f32) * 0.5).clamp(350.0, 600.0);
    let tight_radius = (q.radius_px as f32).max(16.0);

    let hint_explicitly_button = h
        .split_whitespace()
        .any(|w| matches!(w, "tombol" | "button" | "btn" | "icon" | "ikon" | "suara" | "voice"));
    let hint_wants_input = !hint_explicitly_button
        && h.split_whitespace().any(|w| {
            matches!(
                w,
                "bar" | "box" | "input" | "field" | "kolom" | "kotak" | "search" | "cari"
            )
        });

    let mut candidates: Vec<HintCand> = Vec::new();

    for (i, ctl) in controls.iter().enumerate() {
        let w = ctl.rect.right as i64 - ctl.rect.left as i64;
        let h_px = ctl.rect.bottom as i64 - ctl.rect.top as i64;
        if w <= 0 || h_px <= 0 || w > q.screen_w as i64 * 2 || h_px > q.screen_h as i64 * 2 {
            continue;
        }
        if !ctl.rect.is_valid_control(q.screen_w, q.screen_h) {
            continue;
        }

        // Pita chrome browser (tab strip / address bar / bookmark bar): bila titik model berada
        // jelas di area konten halaman, kontrol yang seluruhnya berada di pita atas itu BUKAN target.
        // Tanpa ini, hint yang sama dengan judul tab (mis. judul artikel yang sedang dibuka) menang
        // "exact" atas elemen halaman dan klik terbang ke tab strip.
        if q.chrome_band_bottom > 0
            && q.py >= q.chrome_band_bottom + CHROME_BAND_MARGIN
            && ctl.rect.bottom <= q.chrome_band_bottom
        {
            continue;
        }

        // Di halaman web browser (chrome_band_bottom > 0), elemen `<li>` (ListItem) yang berisi
        // kalimat/paragraf panjang (mis. butir penjelasan "Ringkasan AI" / AI Overview Google)
        // adalah teks statis, BUKAN tautan yang bisa diklik (tautan web selalu Hyperlink/Button).
        if q.chrome_band_bottom > 0
            && ctl.kind == ControlKind::ListItem
            && ctl.rect.top >= q.chrome_band_bottom
            && (ctl.name.chars().count() > 80
                || ctl.name.contains(": ")
                || ctl.name.contains(". "))
        {
            continue;
        }

        let contained = contains(&ctl.rect, q.px, q.py);
        let d = if contained {
            0.0
        } else {
            rect_distance(&ctl.rect, q.px, q.py) as f32
        };

        let n = normalize_hint(&ctl.name);
        let named = !n.is_empty();
        let sim = if named { text_similarity(&h, &n) } else { 0.0 };

        // Dynamic Reach (lihat `reach_for_sim`).
        let max_allowed_d = reach_for_sim(sim, wide_reach, tight_radius);

        if !contained && d > max_allowed_d {
            continue;
        }

        let spatial = if contained {
            0.0
        } else {
            0.25 + (d / max_allowed_d) * 0.75
        };
        let textual = if named { 1.0 - sim } else { 1.0 };
        // Berikan bobot dominan pada kecocokan teks jika hint ada
        let mut cost = 0.3 * spatial + 0.7 * textual;
        if is_row_container(ctl.kind) {
            cost += CONTAINER_PENALTY;
        }
        if hint_wants_input && !matches!(ctl.kind, ControlKind::Edit | ControlKind::ComboBox) {
            cost += 0.08;
        }

        candidates.push(HintCand {
            index: i,
            cost,
            sim,
            contained,
            named,
            name: ctl.name.clone(),
            kind: ctl.kind,
            distance: d,
            area: ctl.area(),
        });
    }

    // Jika ada kandidat bernama yang cocok secara semantik (sim >= 0.65),
    // BUANG SEMUA kandidat anonim atau kandidat dengan sim < 0.65 (mencegah salah sasaran ke elemen tetangga).
    if candidates.iter().any(|c| c.named && c.sim >= 0.65) {
        candidates.retain(|c| c.named && c.sim >= 0.65);
    }

    // Urut berdasarkan biaya; bila selisih sangat kecil, kontrol berluas LEBIH KECIL (lebih spesifik)
    // menang — bukan sekadar urutan pohon UIA (induk selalu muncul sebelum anak).
    candidates.sort_by(|a, b| {
        if (a.cost - b.cost).abs() < 0.01 {
            a.area.cmp(&b.area).then(a.index.cmp(&b.index))
        } else {
            a.cost.total_cmp(&b.cost)
        }
    });

    match candidates.as_slice() {
        [] => HintedPick::None,
        // Foto/thumbnail tanpa nama yang tepat berada di titik model: hint (mis. judul artikel)
        // tidak bisa dicocokkan ke teks, tetapi titik model memang jatuh di dalam gambar itu.
        [b, ..] if b.sim < 0.65 => {
            let named_neighbour = candidates.iter().any(|c| {
                c.named && c.kind != ControlKind::Image && (c.contained || c.distance <= tight_radius)
            });
            if !named_neighbour {
                if let Some(img) = candidates
                    .iter()
                    .filter(|c| c.kind == ControlKind::Image && c.contained && !c.named)
                    .min_by_key(|c| c.area)
                {
                    return HintedPick::Chosen {
                        index: img.index,
                        sim: 0.0,
                        contained: true,
                    };
                }
            }
            // Jika kandidat terbaik tidak memenuhi ambang batas 65% kecocokan hint
            if b.named {
                HintedPick::Vetoed {
                    best_name: b.name.clone(),
                    best_sim: b.sim,
                }
            } else {
                HintedPick::None
            }
        }
        [b, ..] => {
            // Cek apakah ada kandidat lain yang benar-benar berbeda (competing distinct target)
            // yang skornya berselisih < 0.06.
            // Jika kandidat kedua memiliki nama yang sama (misal '📄 Akses PDF' vs '📄 Akses PDF'),
            // atau keduanya bertumpukan pada titik yang sama (nested element di DOM),
            // maka itu BUKAN ambiguitas — itu elemen yang sama!
            let competing = candidates.iter().skip(1).find(|s| {
                let nb = normalize_hint(&b.name);
                let ns = normalize_hint(&s.name);
                let same_name = nb == ns || nb.contains(&ns) || ns.contains(&nb);
                let overlapping = b.contained && s.contained; // both contain the query point
                !same_name && !overlapping && (s.cost - b.cost).abs() < 0.06
            });

            if competing.is_some() {
                let top3 = candidates
                    .iter()
                    .take(3)
                    .map(|c| AmbiguousCandidate {
                        index: c.index,
                        name: c.name.clone(),
                        kind: c.kind,
                        distance_px: c.distance,
                        score: c.cost,
                    })
                    .collect();
                HintedPick::Ambiguous(top3)
            } else {
                HintedPick::Chosen {
                    index: b.index,
                    sim: b.sim,
                    contained: b.contained,
                }
            }
        }
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
            chrome_band_bottom: 0,
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

        // Veto mismatch when hint is "Print" (both Cancel and Submit have sim < 0.65)
        let veto = pick_hinted(&[btn_cancel, btn_submit], &q(120, 120), "Print");
        match veto {
            HintedPick::Vetoed { best_sim, .. } => {
                assert!(best_sim < 0.65);
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

    #[test]
    fn test_sidebar_download_folder_found_across_large_distance_over_unrelated_item() {
        // Point is at (50, 248) touching "Faqih - Personal"
        let mut personal = ctl(ControlKind::TreeItem, 20, 230, 180, 266);
        personal.name = "Faqih - Personal".into();

        // Target "Downloads" is at (50, 358) (110 px lower in the tree)
        let mut downloads = ctl(ControlKind::TreeItem, 20, 340, 180, 376);
        downloads.name = "Downloads".into();

        // Model called with hint "download" (Indonesian colloquial for folder Downloads)
        let controls = vec![personal, downloads];
        let query = SnapQuery {
            px: 50,
            py: 248,
            radius_px: 22,
            hint_area_px: None,
            screen_w: 1920,
            screen_h: 1080,
            chrome_band_bottom: 0,
        };

        match pick_hinted(&controls, &query, "download") {
            HintedPick::Chosen { index, sim, .. } => {
                // Must pick Downloads (index 1), NOT Faqih - Personal (index 0)
                assert_eq!(index, 1, "Downloads must be chosen even though cursor touched Faqih - Personal");
                assert!(sim >= 0.8, "Similarity of 'download' vs 'Downloads' should be high");
            }
            other => panic!("Expected Downloads to be chosen, got {:?}", other),
        }
    }

    #[test]
    fn test_pick_hinted_duplicate_nested_elements_do_not_trigger_ambiguity() {
        // Bug A: In Chrome, a web button or hyperlink often contains an inner text element
        // with the EXACT SAME accessible name (e.g. '📄 Akses PDF' on <a> and inner <span>).
        // When both elements contain the click point (distance = 0.0), pick_hinted must NOT
        // treat them as competing ambiguous targets!
        let mut hyperlink = ctl(ControlKind::Hyperlink, 1300, 600, 1400, 660);
        hyperlink.name = "📄 Akses PDF".into();
        let mut inner_span = ctl(ControlKind::Button, 1310, 610, 1390, 650);
        inner_span.name = "📄 Akses PDF".into();

        let controls = vec![hyperlink, inner_span];
        let query = q(1344, 630);

        match pick_hinted(&controls, &query, "Akses PDF") {
            HintedPick::Chosen { index, sim, .. } => {
                assert!(index == 0 || index == 1);
                assert!(sim >= 0.85);
            }
            other => panic!("Expected Chosen, but got {:?}", other),
        }
    }

    #[test]
    fn test_text_similarity_token_matching_for_wrapper_phrases() {
        // Bug B: In Chrome profile picker, the button is named "Open Dazatulloh profile",
        // but user says "Dzatullah 2". Token-level fuzzy match must match "dzatullah" with "dazatulloh"
        // and score >= 0.65 so that wide_reach is activated.
        let sim = text_similarity("Dzatullah 2", "Open Dazatulloh profile");
        assert!(sim >= 0.65, "Expected similarity >= 0.65 for 'Dzatullah 2' vs 'Open Dazatulloh profile', got {}", sim);
    }

    #[test]
    fn test_text_similarity_rejects_partial_token_rhymes_like_mode_ai_and_code_tools() {
        // Bug C: In Google Search results, when user asks for "Alih Kode dan Campur Kode Bahasa Arab",
        // neighboring controls like bookmark 'CODE Tools' and filter 'Mode AI' must NOT be matched
        // just because 'code' or 'mode' resembles 'kode'!
        let hint = "Alih Kode dan Campur Kode Bahasa Arab";

        let sim_code_tools = text_similarity(hint, "CODE Tools");
        assert!(
            sim_code_tools < 0.50,
            "Expected 'CODE Tools' to have low similarity (< 0.50), got {}",
            sim_code_tools
        );

        let sim_mode_ai = text_similarity(hint, "Mode AI");
        assert!(
            sim_mode_ai < 0.50,
            "Expected 'Mode AI' to have low similarity (< 0.50), got {}",
            sim_mode_ai
        );

        let real_result = "Alih Kode dan Campur Kode Bahasa Arab Jurnal Universitas Gadjah Mada https://journal.ugm.ac.id › article › view";
        let sim_real = text_similarity(hint, real_result);
        assert!(
            sim_real >= 0.90,
            "Expected real search result to have high similarity (>= 0.90), got {}",
            sim_real
        );
    }

    #[test]
    fn test_pick_hinted_snaps_to_search_result_despite_neighboring_code_tools_and_mode_ai() {
        // Simulation of the exact screen state from user's incident:
        // Query point at (480, 184) in the search box text.
        // Neighboring controls:
        // - 'CODE Tools' bookmark at (200, 95)
        // - 'Mode AI' filter tab at (200, 250)
        // - Real search result link at (480, 420)
        let mut bookmark = ctl(ControlKind::Button, 150, 80, 250, 110);
        bookmark.name = "CODE Tools".into();

        let mut mode_ai = ctl(ControlKind::Button, 150, 235, 250, 265);
        mode_ai.name = "Mode AI".into();

        let mut ugm_link = ctl(ControlKind::Hyperlink, 200, 390, 760, 450);
        ugm_link.name = "Alih Kode dan Campur Kode Bahasa Arab Jurnal Universitas Gadjah Mada https://journal.ugm.ac.id › article › view".into();

        let controls = vec![bookmark, mode_ai, ugm_link];
        let query = SnapQuery {
            px: 480,
            py: 184,
            radius_px: 22,
            hint_area_px: None,
            screen_w: 1920,
            screen_h: 1080,
            chrome_band_bottom: 0,
        };

        match pick_hinted(&controls, &query, "Alih Kode dan Campur Kode Bahasa Arab") {
            HintedPick::Chosen { index, sim, .. } => {
                assert_eq!(index, 2, "Must pick UGM search result (index 2), NOT CODE Tools or Mode AI");
                assert!(sim >= 0.90, "Similarity must be >= 0.90");
            }
            other => panic!("Expected Chosen, but got {:?}", other),
        }
    }

    fn named(kind: ControlKind, name: &str, l: i32, t: i32, r: i32, b: i32) -> ControlInfo {
        let mut c = ctl(kind, l, t, r, b);
        c.name = name.into();
        c
    }

    fn browser_q(px: i32, py: i32) -> SnapQuery {
        SnapQuery {
            px,
            py,
            radius_px: 22,
            hint_area_px: None,
            screen_w: SW,
            screen_h: SH,
            chrome_band_bottom: 150,
        }
    }

    // ── Regression (log 2026-11-06 17:15:25): hint == judul tab yang sedang terbuka ──────────────
    // Titik model di foto halaman (960,432). Satu-satunya elemen bernama sama adalah TabItem di
    // tab strip (y<52). Sebelumnya TabItem menang "exact" lalu klik ditolak/terbang ke tab strip.
    #[test]
    fn browser_tab_with_same_title_does_not_steal_click_from_page_content() {
        let title = "Tim Mahasiswa Sastra Arab UGM Raih Juara 2 Lomba Essay Presentation dalam Ajang Arabic Fair UNS 2025 - Program Studi Sastra Arab FIB UGM";
        let tab = named(ControlKind::TabItem, title, 928, 0, 1249, 52);
        let photo = ctl(ControlKind::Image, 600, 300, 1300, 700);
        let controls = vec![tab, photo];

        // Baseline (perilaku lama): tanpa pita chrome, tab menang "exact".
        let mut no_band = browser_q(960, 432);
        no_band.chrome_band_bottom = 0;
        match pick_hinted(&controls, &no_band, title) {
            HintedPick::Chosen { index, .. } => assert_eq!(index, 0, "baseline: tab menang tanpa pita chrome"),
            other => panic!("baseline expected tab, got {:?}", other),
        }

        // Setelah fix: pita chrome mengecualikan tab → foto (tanpa nama) di titik model dipilih.
        match pick_hinted(&controls, &browser_q(960, 432), title) {
            HintedPick::Chosen { index, .. } => assert_eq!(index, 1, "foto halaman harus dipilih, bukan tab"),
            other => panic!("expected photo, got {:?}", other),
        }
    }

    #[test]
    fn browser_tab_still_clickable_when_model_points_at_tab_strip() {
        let tab = named(ControlKind::TabItem, "PM2+ Monitoring", 928, 0, 1249, 52);
        match pick_hinted(&[tab], &browser_q(1000, 30), "PM2+ Monitoring") {
            HintedPick::Chosen { index, .. } => assert_eq!(index, 0),
            other => panic!("tab must remain clickable near tab strip, got {:?}", other),
        }
    }

    // ── Regression (log 17:14:40): kecocokan parsial tidak boleh melompat ~600px ──────────────────
    #[test]
    fn partial_match_cannot_jump_across_screen() {
        let hint = "Tim Mahasiswa Sastra Arab UGM Raih Juara 2 Lomba Essay";
        let cand = "Universitas Gadjah Mada UNIVERSITAS GADJAH MADA";
        let sim = text_similarity(hint, cand);
        assert!((0.65..0.85).contains(&sim), "precondition: parsial, sim={sim}");

        let far = named(ControlKind::Button, cand, 100, 800, 300, 840);
        // titik model 600px di atas kandidat
        if let HintedPick::Chosen { .. } = pick_hinted(&[far.clone()], &q(200, 200), hint) {
            panic!("kecocokan parsial 600px jauhnya tidak boleh dipilih");
        }
        // dekat (<=300px) tetap boleh
        match pick_hinted(&[far], &q(200, 600), hint) {
            HintedPick::Chosen { index, .. } => assert_eq!(index, 0),
            other => panic!("kecocokan parsial dekat harus dipilih, got {:?}", other),
        }
    }

    // ── Regression: duplikat bersarang — induk (ListItem) muncul lebih dulu di pohon UIA ──────────
    #[test]
    fn nested_duplicate_prefers_specific_control_over_row_container() {
        let li = named(ControlKind::ListItem, "Akses PDF", 1100, 560, 1700, 700);
        let link = named(ControlKind::Hyperlink, "Akses PDF", 1300, 600, 1420, 650);
        match pick_hinted(&[li, link], &q(1350, 625), "Akses PDF") {
            HintedPick::Chosen { index, .. } => assert_eq!(index, 1, "Hyperlink spesifik harus menang atas ListItem pembungkus"),
            other => panic!("expected Chosen, got {:?}", other),
        }
    }

    #[test]
    fn sole_row_container_is_still_chosen_for_row_style_lists() {
        let row = named(
            ControlKind::DataItem,
            "Tokopedia Promo akhir tahun diskon besar-besaran untuk kamu yang berlangganan newsletter kami hari ini saja",
            300,
            400,
            1500,
            440,
        );
        match pick_hinted(&[row], &q(800, 420), "Tokopedia") {
            HintedPick::Chosen { index, .. } => assert_eq!(index, 0),
            other => panic!("baris email harus tetap bisa dipilih, got {:?}", other),
        }
    }

    // ── Foto/thumbnail tanpa nama tepat di titik model (hint tidak bisa dicocokkan) ───────────────
    #[test]
    fn unnamed_image_under_point_is_chosen_when_hint_cannot_match() {
        let photo = ctl(ControlKind::Image, 600, 300, 1300, 700);
        match pick_hinted(&[photo], &browser_q(960, 432), "foto juara") {
            HintedPick::Chosen { index, .. } => assert_eq!(index, 0),
            other => panic!("expected image, got {:?}", other),
        }
    }

    #[test]
    fn unnamed_image_does_not_override_named_neighbour_mismatch() {
        let photo = ctl(ControlKind::Image, 600, 300, 1300, 700);
        let btn = named(ControlKind::Button, "Submit", 900, 400, 1000, 440);
        match pick_hinted(&[photo, btn], &browser_q(950, 420), "Print") {
            HintedPick::Vetoed { .. } => {}
            other => panic!("tombol bertetangga yang tak cocok harus veto, got {:?}", other),
        }
    }

    #[test]
    fn pick_best_without_hint_ignores_images() {
        let photo = ctl(ControlKind::Image, 600, 300, 1300, 700);
        assert!(pick_best(&[photo], &q(960, 432)).is_none());
    }

    // ── Titik klik fisik ───────────────────────────────────────────────────────────────────────
    // Regression (log 07:16:15): blok hasil Google 458x153 → pusat (1405,503) jatuh di snippet.
    #[test]
    fn click_point_targets_first_line_of_multiline_hyperlink() {
        let link = named(ControlKind::Hyperlink, "Hasil", 1176, 427, 1634, 580);
        assert_eq!(click_point(&link, 1344, 108), (1405, 447));
    }

    #[test]
    fn click_point_uses_center_for_single_line_controls() {
        let btn = named(ControlKind::Button, "OK", 100, 100, 200, 140);
        assert_eq!(click_point(&btn, 0, 0), (150, 120));
        let link = named(ControlKind::Hyperlink, "Satu baris", 100, 100, 300, 130);
        assert_eq!(click_point(&link, 0, 0), (200, 115));
    }

    #[test]
    fn click_point_keeps_model_point_inside_image() {
        let photo = ctl(ControlKind::Image, 600, 300, 1300, 700);
        assert_eq!(click_point(&photo, 700, 350), (700, 350));
        // di luar gambar → pusat
        assert_eq!(click_point(&photo, 50, 50), (950, 500));
    }

    // ── Regression (log 2026-11-07 07:50:27..07:50:47 & 07:58:26): "search bar" vs "Cari" ─────────
    #[test]
    fn regression_log_075027_search_bar_hint_matches_indonesian_cari_and_ignores_omnibox() {
        let omnibox = named(ControlKind::Edit, "Address and search bar", 243, 64, 1315, 95);
        let google_cari = named(ControlKind::ComboBox, "Cari", 264, 184, 1088, 247);
        let voice_btn = named(ControlKind::Button, "Telusuri dengan suara", 1140, 195, 1180, 235);
        let controls = vec![omnibox, google_cari, voice_btn];

        // 1. Klik di (804, 208) dengan hint "search bar" harus memilih ComboBox "Cari" (index 1),
        //    BUKAN melompat ke omnibox di atas (index 0) yang tertutup HUD!
        match pick_hinted(&controls, &browser_q(804, 208), "search bar") {
            HintedPick::Chosen { index, .. } => assert_eq!(index, 1, "Harus memilih ComboBox 'Cari', bukan omnibox"),
            other => panic!("Expected Chosen(1), got {:?}", other),
        }

        // 2. Klik di (1056, 237) maupun (1155, 237) dekat tombol suara dengan hint "search bar"
        //    atau "Search" atau "search bar with current query" harus cocok dengan "Cari" (index 1)!
        for &(px, py) in &[(1056, 237), (1155, 237)] {
            for hint in ["search bar", "Search", "search bar with current query", "kotak pencarian"] {
                match pick_hinted(&controls, &browser_q(px, py), hint) {
                    HintedPick::Chosen { index, .. } => {
                        assert_eq!(index, 1, "Hint '{}' di ({},{}) harus memilih 'Cari'", hint, px, py)
                    }
                    other => panic!("Hint '{}' di ({},{}) gagal memilih 'Cari': {:?}", hint, px, py, other),
                }
            }
        }
    }

    // ── Regression (log 2026-11-07 09:28:33..09:29:25): AI Overview ListItem & non-UGM truncated links ──
    #[test]
    fn regression_log_092833_ai_overview_list_items_and_non_ugm_truncated_links_are_rejected() {
        let ai_overview_bullet_1 = named(
            ControlKind::ListItem,
            "Alih Kode (Code-Switching): Peristiwa peralihan dari satu bahasa (atau ragam bahasa) ke bahasa lain secara utuh dalam suatu situasi tutur. Contoh: Berbicara menggunakan bahasa Indonesia, lalu beralih penuh menggunakan bahasa Arab.",
            285,
            581,
            1078,
            806,
        );
        let ai_overview_bullet_2 = named(
            ControlKind::ListItem,
            "Contoh: \"Kita harus syukron dulu atas bantuan yang diberikan\" (mencampurkan kosakata bahasa Arab ke dalam struktur kalimat bahasa Indonesia).",
            313,
            211,
            1078,
            271,
        );
        let uin_truncated_link = named(
            ControlKind::Hyperlink,
            "Alih Kode dan Campur Kode dalam Percakapan Sehari-hari ...",
            1176,
            459,
            1634,
            612,
        );
        let unimal_link = named(
            ControlKind::Hyperlink,
            "Alih Kode dan Campur Kode Arab-Indonesia dalam Vlog ...",
            1176,
            620,
            1634,
            760,
        );
        let controls = vec![
            ai_overview_bullet_1,
            ai_overview_bullet_2,
            uin_truncated_link,
            unimal_link,
        ];

        // 1. Ketika model meminta jurnal UGM "Alih Kode dan Campur Kode Bahasa Arab | Deskripsi Bahasa"
        //    tetapi layar masih di bagian atas (AI Overview, belum digulir ke bawah), JANGAN memilih
        //    ListItem AI Overview ataupun link UIN/Unimal!
        match pick_hinted(
            &controls,
            &browser_q(624, 529),
            "Alih Kode dan Campur Kode Bahasa Arab | Deskripsi Bahasa",
        ) {
            HintedPick::Vetoed { .. } | HintedPick::None => {}
            other => panic!("Tidak boleh memilih ListItem AI Overview atau link non-UGM: {:?}", other),
        }

        // 2. Ketika model memberikan hint panjang yang menyebutkan "Mahasiswa Jurusan Sastra Arab Universitas Gadjah Mada",
        //    link pendek terpotong 'Alih Kode dan Campur Kode dalam Percakapan Sehari-hari ...' (milik UIN)
        //    TIDAK BOLEH dikatrol oleh cand_coverage menjadi 90%!
        match pick_hinted(
            &controls,
            &browser_q(1405, 763),
            "Alih Kode dan Campur Kode dalam Percakapan Sehari-hari Mahasiswa Jurusan Sastra Arab Universitas Gadjah Mada",
        ) {
            HintedPick::Vetoed { .. } | HintedPick::None => {}
            other => panic!("Link UIN terpotong tidak boleh lolos sebagai jurnal UGM: {:?}", other),
        }
    }
}
