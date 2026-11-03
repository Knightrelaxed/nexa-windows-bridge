//! Software Rasterizer for NEXA Virtual Cursor Sprite
//!
//! Renders the Aegis-designed tailless delta arrowhead, concentric click ripples,
//! lock-on brackets, and floating "NEXA" telemetry badge into a 32-bit Premultiplied
//! ARGB buffer (BGRA little-endian format for Win32 UpdateLayeredWindow).

use crate::ui::CursorMode;

pub const SPRITE_WIDTH: usize = 256;
pub const SPRITE_HEIGHT: usize = 160;
pub const HOTSPOT_X: i32 = 128;
pub const HOTSPOT_Y: i32 = 64;

/// RGBA color representation (non-premultiplied inputs)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Convert to premultiplied 32-bit pixel value in Win32 0xAARRGGBB order (little-endian BGRA)
    #[inline]
    pub fn to_premultiplied_u32(self) -> u32 {
        if self.a == 0 {
            return 0;
        }
        let a = self.a as u32;
        let r = ((self.r as u32 * a + 127) / 255) as u8;
        let g = ((self.g as u32 * a + 127) / 255) as u8;
        let b = ((self.b as u32 * a + 127) / 255) as u8;
        ((a) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
    }
}

/// Palette mapping for the 9 CursorModes
pub fn get_mode_color(mode: CursorMode) -> Color {
    match mode {
        CursorMode::Idle => Color::rgb(0x22, 0xD3, 0xEE),      // Cyan Neon
        CursorMode::Thinking => Color::rgb(0xA9, 0x8B, 0xFF),  // Violet Cognitive
        CursorMode::Hover => Color::rgb(0x38, 0xBD, 0xF8),     // Sky Lock-on
        CursorMode::Clicking => Color::rgb(0xFF, 0xFF, 0xFF),  // Pure Actuation White
        CursorMode::Dragging => Color::rgb(0x5A, 0xA2, 0xFF),  // Azure Grip
        CursorMode::Typing => Color::rgb(0x8C, 0xF6, 0xFF),    // Bright Cyan I-beam
        CursorMode::Executing => Color::rgb(0xFF, 0xA2, 0x3A), // Amber Activity
        CursorMode::Success => Color::rgb(0x34, 0xE6, 0xA5),   // Cyber Emerald
        CursorMode::Error => Color::rgb(0xFF, 0x4A, 0x68),     // Crimson Alert
    }
}

/// 32-bit BGRA Premultiplied Sprite Canvas
pub struct SpriteCanvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

impl SpriteCanvas {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width * height],
        }
    }

    /// Clear canvas to 100% transparent black
    #[inline]
    pub fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// Blend a premultiplied pixel using Porter-Duff SrcOver
    #[inline]
    pub fn blend_pixel(&mut self, x: i32, y: i32, src_color: Color) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 || src_color.a == 0 {
            return;
        }
        let idx = (y as usize) * self.width + (x as usize);
        if src_color.a == 255 {
            self.pixels[idx] = src_color.to_premultiplied_u32();
            return;
        }

        let dst_raw = self.pixels[idx];
        let da = ((dst_raw >> 24) & 0xFF) as u32;
        let dr = ((dst_raw >> 16) & 0xFF) as u32;
        let dg = ((dst_raw >> 8) & 0xFF) as u32;
        let db = (dst_raw & 0xFF) as u32;

        let sa = src_color.a as u32;
        let sr = (src_color.r as u32 * sa + 127) / 255;
        let sg = (src_color.g as u32 * sa + 127) / 255;
        let sb = (src_color.b as u32 * sa + 127) / 255;

        let inv_sa = 255 - sa;
        let out_r = (sr + (dr * inv_sa + 127) / 255).min(255);
        let out_g = (sg + (dg * inv_sa + 127) / 255).min(255);
        let out_b = (sb + (db * inv_sa + 127) / 255).min(255);
        let out_a = (sa + (da * inv_sa + 127) / 255).min(255);

        self.pixels[idx] = (out_a << 24) | (out_r << 16) | (out_g << 8) | out_b;
    }

    /// Draw a line using Bresenham's algorithm with alpha blending
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let mut cx = x0;
        let mut cy = y0;

        loop {
            self.blend_pixel(cx, cy, color);
            if cx == x1 && cy == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                cx += sx;
            }
            if e2 <= dx {
                err += dx;
                cy += sy;
            }
        }
    }

    /// Draw a line with a specified stroke thickness
    pub fn draw_thick_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, thickness: i32, color: Color) {
        let half = thickness / 2;
        for ox in -half..=half {
            for oy in -half..=half {
                self.draw_line(x0 + ox, y0 + oy, x1 + ox, y1 + oy, color);
            }
        }
    }

    /// Draw a filled triangle using standard scanline interpolation
    pub fn fill_triangle(&mut self, mut p0: (i32, i32), mut p1: (i32, i32), mut p2: (i32, i32), color: Color) {
        // Sort vertices by y ascending (p0.1 <= p1.1 <= p2.1)
        if p0.1 > p1.1 {
            std::mem::swap(&mut p0, &mut p1);
        }
        if p0.1 > p2.1 {
            std::mem::swap(&mut p0, &mut p2);
        }
        if p1.1 > p2.1 {
            std::mem::swap(&mut p1, &mut p2);
        }

        let total_h = p2.1 - p0.1;
        if total_h == 0 {
            return;
        }

        for y in p0.1..=p2.1 {
            let second_half = y > p1.1 || p1.1 == p0.1;
            let segment_h = if second_half { p2.1 - p1.1 } else { p1.1 - p0.1 };
            if segment_h == 0 {
                continue;
            }

            let alpha = (y - p0.1) as f32 / total_h as f32;
            let beta = if second_half {
                (y - p1.1) as f32 / segment_h as f32
            } else {
                (y - p0.1) as f32 / segment_h as f32
            };

            let mut ax = (p0.0 as f32 + (p2.0 - p0.0) as f32 * alpha).round() as i32;
            let mut bx = if second_half {
                (p1.0 as f32 + (p2.0 - p1.0) as f32 * beta).round() as i32
            } else {
                (p0.0 as f32 + (p1.0 - p0.0) as f32 * beta).round() as i32
            };

            if ax > bx {
                std::mem::swap(&mut ax, &mut bx);
            }

            for x in ax..=bx {
                self.blend_pixel(x, y, color);
            }
        }
    }

    /// Draw a circle outline with subpixel distance antialiasing
    pub fn draw_circle_outline(&mut self, cx: i32, cy: i32, radius: f32, thickness: f32, color: Color) {
        let max_r = radius + thickness + 1.0;
        let min_x = (cx as f32 - max_r).floor() as i32;
        let max_x = (cx as f32 + max_r).ceil() as i32;
        let min_y = (cy as f32 - max_r).floor() as i32;
        let max_y = (cy as f32 + max_r).ceil() as i32;

        let half_t = thickness / 2.0;

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let dx = x as f32 - cx as f32;
                let dy = y as f32 - cy as f32;
                let dist = (dx * dx + dy * dy).sqrt();
                let diff = (dist - radius).abs();

                if diff <= half_t + 1.0 {
                    let coverage = (1.0 - (diff - half_t).max(0.0)).clamp(0.0, 1.0);
                    let a = (color.a as f32 * coverage).round() as u8;
                    if a > 0 {
                        self.blend_pixel(x, y, Color::rgba(color.r, color.g, color.b, a));
                    }
                }
            }
        }
    }

    /// Draw a filled rounded rectangle (badge box)
    pub fn fill_rounded_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, bg: Color, border: Color) {
        for py in y..y + h {
            for px in x..x + w {
                // Check corners
                let in_tl = px < x + r && py < y + r;
                let in_tr = px >= x + w - r && py < y + r;
                let in_bl = px < x + r && py >= y + h - r;
                let in_br = px >= x + w - r && py >= y + h - r;

                let inside = if in_tl {
                    let dx = px - (x + r);
                    let dy = py - (y + r);
                    dx * dx + dy * dy <= r * r
                } else if in_tr {
                    let dx = px - (x + w - r - 1);
                    let dy = py - (y + r);
                    dx * dx + dy * dy <= r * r
                } else if in_bl {
                    let dx = px - (x + r);
                    let dy = py - (y + h - r - 1);
                    dx * dx + dy * dy <= r * r
                } else if in_br {
                    let dx = px - (x + w - r - 1);
                    let dy = py - (y + h - r - 1);
                    dx * dx + dy * dy <= r * r
                } else {
                    true
                };

                if inside {
                    let is_border = px == x || px == x + w - 1 || py == y || py == y + h - 1;
                    if is_border && border.a > 0 {
                        self.blend_pixel(px, py, border);
                    } else {
                        self.blend_pixel(px, py, bg);
                    }
                }
            }
        }
    }

    /// 5x7 Monospace Pixel Font Glyph Renderer (Uppercase ASCII & Basic Symbols)
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: Color) {
        let mut cur_x = x;
        for ch in text.chars() {
            let glyph = get_glyph_bitmap(ch);
            for (row, byte) in glyph.iter().enumerate() {
                for col in 0..5 {
                    if (byte & (1 << (4 - col))) != 0 {
                        self.blend_pixel(cur_x + col as i32, y + row as i32, color);
                    }
                }
            }
            cur_x += 6; // 5 px width + 1 px spacing
        }
    }
}

/// 5x7 ASCII Bitmap Glyphs (Full Uppercase A-Z, Digits 0-9, and Common Symbols)
fn get_glyph_bitmap(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        // Uppercase Latin Alphabet (A-Z)
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10010, 0b10100, 0b11000, 0b11100, 0b10110, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10001, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],

        // Digits (0-9)
        '0' => [0b01110, 0b10011, 0b10101, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110],
        '6' => [0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],

        // Punctuation & Symbols
        ' ' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        ':' => [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        ',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100, 0b01000],
        '_' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100],
        '%' => [0b11001, 0b11010, 0b00100, 0b01000, 0b01011, 0b10011, 0b00000],

        // Fallback placeholder box
        _ => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111],
    }
}

/// Render a complete cursor frame into the canvas
pub fn render_cursor_frame(
    canvas: &mut SpriteCanvas,
    mode: CursorMode,
    ripple_progress: Option<f32>,
    badge_override: Option<&str>,
    screen_pos: Option<(i32, i32)>,
) {
    canvas.clear();

    let accent = get_mode_color(mode);
    let hx = HOTSPOT_X;
    let hy = HOTSPOT_Y;

    // ─────────────────────────────────────────────────────────────
    // 1. Concentric Click Ripples (Double concentric rings)
    // ─────────────────────────────────────────────────────────────
    if let Some(p) = ripple_progress {
        let p_clamped = p.clamp(0.0, 1.0);

        // Ring 1
        let r1 = 4.0 + 34.0 * p_clamped;
        let alpha1 = ((1.0 - p_clamped) * (p_clamped * 10.0).min(1.0) * 220.0).round() as u8;
        if alpha1 > 0 {
            canvas.draw_circle_outline(hx, hy, r1, 2.0, Color::rgba(accent.r, accent.g, accent.b, alpha1));
        }

        // Ring 2 (delayed offset)
        if p_clamped > 0.15 {
            let p2 = (p_clamped - 0.15) / 0.85;
            let r2 = 4.0 + 34.0 * p2;
            let alpha2 = ((1.0 - p2) * (p2 * 10.0).min(1.0) * 160.0).round() as u8;
            if alpha2 > 0 {
                canvas.draw_circle_outline(hx, hy, r2, 1.5, Color::rgba(accent.r, accent.g, accent.b, alpha2));
            }
        }
    }

    // ─────────────────────────────────────────────────────────────
    // 2. Hover Lock-on Brackets (Target Reticle)
    // ─────────────────────────────────────────────────────────────
    if mode == CursorMode::Hover {
        let bracket_color = Color::rgba(accent.r, accent.g, accent.b, 200);
        let s = 14;
        let l = 5;
        // Top-left
        canvas.draw_line(hx - s, hy - s, hx - s + l, hy - s, bracket_color);
        canvas.draw_line(hx - s, hy - s, hx - s, hy - s + l, bracket_color);
        // Top-right
        canvas.draw_line(hx + s, hy - s, hx + s - l, hy - s, bracket_color);
        canvas.draw_line(hx + s, hy - s, hx + s, hy - s + l, bracket_color);
        // Bottom-left
        canvas.draw_line(hx - s, hy + s, hx - s + l, hy + s, bracket_color);
        canvas.draw_line(hx - s, hy + s, hx - s, hy + s - l, bracket_color);
        // Bottom-right
        canvas.draw_line(hx + s, hy + s, hx + s - l, hy + s, bracket_color);
        canvas.draw_line(hx + s, hy + s, hx + s, hy + s - l, bracket_color);
    }

    // ─────────────────────────────────────────────────────────────
    // 3. Tailless Delta Arrowhead Pointer (Aegis Dual-Facet)
    // ─────────────────────────────────────────────────────────────
    // Hotspot is exact apex tip: V0 = (hx, hy)
    let v0 = (hx, hy);
    let v1 = (hx + 6, hy + 28);  // Left wing
    let v2 = (hx + 12, hy + 21); // Inward notch
    let v3 = (hx + 22, hy + 18); // Right wing

    // Left facet: Frosted glass translucent fill (32% alpha)
    let glass_color = Color::rgba(accent.r, accent.g, accent.b, 80);
    canvas.fill_triangle(v0, v1, v2, glass_color);

    // Right facet: Solid neon fill (95% alpha)
    let solid_color = Color::rgba(accent.r, accent.g, accent.b, 240);
    canvas.fill_triangle(v0, v2, v3, solid_color);

    // Dark divider line between facets
    let divider_color = Color::rgba(3, 16, 26, 180);
    canvas.draw_line(hx + 2, hy + 7, hx + 10, hy + 20, divider_color);

    // Outer neon rim stroke
    let rim_color = Color::rgba(accent.r.saturating_add(40), accent.g.saturating_add(40), accent.b.saturating_add(40), 255);
    canvas.draw_thick_line(v0.0, v0.1, v1.0, v1.1, 1, rim_color);
    canvas.draw_thick_line(v1.0, v1.1, v2.0, v2.1, 1, rim_color);
    canvas.draw_thick_line(v2.0, v2.1, v3.0, v3.1, 1, rim_color);
    canvas.draw_thick_line(v3.0, v3.1, v0.0, v0.1, 1, rim_color);

    // Core node glow dot at hotspot apex
    canvas.blend_pixel(hx, hy, Color::rgb(255, 255, 255));
    canvas.blend_pixel(hx + 1, hy, Color::rgba(accent.r, accent.g, accent.b, 200));
    canvas.blend_pixel(hx, hy + 1, Color::rgba(accent.r, accent.g, accent.b, 200));

    // ─────────────────────────────────────────────────────────────
    // 4. Floating AI Telemetry Badge ("NEXA // <STATE>")
    // ─────────────────────────────────────────────────────────────
    let badge_text = badge_override.unwrap_or_else(|| match mode {
        CursorMode::Thinking => "NEXA // THK",
        CursorMode::Hover => "NEXA // LCK",
        CursorMode::Clicking => "NEXA // ACT",
        CursorMode::Dragging => "NEXA // DRG",
        CursorMode::Typing => "NEXA // TYP",
        CursorMode::Executing => "NEXA // EXE",
        CursorMode::Success => "NEXA // OK",
        CursorMode::Error => "NEXA // ERR",
        CursorMode::Idle => "NEXA",
    });

    let (flip_x, flip_y) = if let Some((sx, sy)) = screen_pos {
        let (screen_w, screen_h) = {
            let sw = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSCREEN) };
            let sh = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(windows_sys::Win32::UI::WindowsAndMessaging::SM_CYSCREEN) };
            (if sw > 0 { sw } else { 1920 }, if sh > 0 { sh } else { 1080 })
        };
        // Flip badge left if close to right edge of screen; flip up if close to bottom
        (sx + 120 > screen_w, sy + 50 > screen_h)
    } else {
        (false, false)
    };

    let text_chars = badge_text.chars().count();
    let badge_w = (text_chars as i32 * 6 + 12).clamp(42, 100);
    let badge_h = 16;

    let badge_x = if flip_x {
        hx - 24 - badge_w
    } else {
        hx + 24
    };

    let badge_y = if flip_y {
        hy - 18 - badge_h
    } else {
        hy + 18
    };

    let badge_bg = Color::rgba(4, 7, 15, 224); // 88% dark navy glass
    let badge_border = Color::rgba(accent.r, accent.g, accent.b, 160);
    canvas.fill_rounded_rect(badge_x, badge_y, badge_w, badge_h, 4, badge_bg, badge_border);

    // Centered text
    let text_x = badge_x + (badge_w - text_chars as i32 * 6) / 2 + 1;
    let text_y = badge_y + (badge_h - 7) / 2;
    canvas.draw_text(text_x, text_y, badge_text, accent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_premultiplied_color_conversion() {
        let white_translucent = Color::rgba(255, 255, 255, 128);
        let pm = white_translucent.to_premultiplied_u32();
        let a = (pm >> 24) & 0xFF;
        let r = (pm >> 16) & 0xFF;
        let g = (pm >> 8) & 0xFF;
        let b = pm & 0xFF;
        assert_eq!(a, 128);
        assert_eq!(r, 128);
        assert_eq!(g, 128);
        assert_eq!(b, 128);
    }

    #[test]
    fn test_render_cursor_frame_produces_valid_pixels() {
        let mut canvas = SpriteCanvas::new(SPRITE_WIDTH, SPRITE_HEIGHT);
        render_cursor_frame(&mut canvas, CursorMode::Idle, None, None, None);

        // Verify apex pixel is painted with high luminance
        let apex_idx = HOTSPOT_Y as usize * SPRITE_WIDTH + HOTSPOT_X as usize;
        let apex_pixel = canvas.pixels[apex_idx];
        assert_ne!(apex_pixel, 0, "Apex pixel must be rendered");

        // Verify badge area has pixels (unflipped: badge is to the right and below hotspot)
        let badge_idx = (HOTSPOT_Y as usize + 22) * SPRITE_WIDTH + (HOTSPOT_X as usize + 35);
        let badge_pixel = canvas.pixels[badge_idx];
        assert_ne!(badge_pixel, 0, "Badge area must be rendered");

        // Verify premultiplied invariant (R <= A, G <= A, B <= A) across all pixels
        for (i, &p) in canvas.pixels.iter().enumerate() {
            if p != 0 {
                let a = (p >> 24) & 0xFF;
                let r = (p >> 16) & 0xFF;
                let g = (p >> 8) & 0xFF;
                let b = p & 0xFF;
                assert!(r <= a, "Pixel at {} has R ({}) > A ({})", i, r, a);
                assert!(g <= a, "Pixel at {} has G ({}) > A ({})", i, g, a);
                assert!(b <= a, "Pixel at {} has B ({}) > A ({})", i, b, a);
            }
        }
    }

    #[test]
    fn test_concentric_ripples_render_rings() {
        let mut canvas = SpriteCanvas::new(SPRITE_WIDTH, SPRITE_HEIGHT);
        render_cursor_frame(&mut canvas, CursorMode::Clicking, Some(0.4), None, None);

        // Count non-zero pixels with ripple vs without ripple
        let ripple_pixels = canvas.pixels.iter().filter(|&&p| p != 0).count();

        let mut idle_canvas = SpriteCanvas::new(SPRITE_WIDTH, SPRITE_HEIGHT);
        render_cursor_frame(&mut idle_canvas, CursorMode::Idle, None, None, None);
        let idle_pixels = idle_canvas.pixels.iter().filter(|&&p| p != 0).count();

        assert!(
            ripple_pixels > idle_pixels,
            "Ripple frame must have more drawn pixels than idle frame ({} > {})",
            ripple_pixels,
            idle_pixels
        );
    }

    #[test]
    fn test_font_glyph_coverage() {
        let chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!?:.,_+-/ ";
        let fallback = get_glyph_bitmap('#');
        for c in chars.chars() {
            let glyph = get_glyph_bitmap(c);
            assert_ne!(glyph, fallback, "Character '{}' must have a dedicated glyph, not fallback", c);
        }
    }

    #[test]
    fn test_badge_screen_boundary_flip() {
        let mut normal_canvas = SpriteCanvas::new(SPRITE_WIDTH, SPRITE_HEIGHT);
        let mut flipped_canvas = SpriteCanvas::new(SPRITE_WIDTH, SPRITE_HEIGHT);

        // Normal middle of screen: (960, 540)
        render_cursor_frame(&mut normal_canvas, CursorMode::Idle, None, None, Some((960, 540)));
        // Near bottom right screen edge: (1910, 1070)
        render_cursor_frame(&mut flipped_canvas, CursorMode::Idle, None, None, Some((1910, 1070)));

        // Unflipped badge has pixels to the right (x > HOTSPOT_X)
        let right_sample = (HOTSPOT_Y as usize + 22) * SPRITE_WIDTH + (HOTSPOT_X as usize + 35);
        assert_ne!(normal_canvas.pixels[right_sample], 0);

        // Flipped badge has pixels to the left (x < HOTSPOT_X) and above (y < HOTSPOT_Y)
        // badge_x = hx - 24 - badge_w (where hx = 128, badge_w approx 42-60 -> badge_x approx 44..86)
        // badge_y = hy - 18 - 16 = 64 - 34 = 30
        let left_sample = (HOTSPOT_Y as usize - 26) * SPRITE_WIDTH + (HOTSPOT_X as usize - 40);
        assert_ne!(flipped_canvas.pixels[left_sample], 0, "Flipped badge must be rendered to the top-left");
    }
}
