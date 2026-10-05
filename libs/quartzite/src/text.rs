// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Quartzite's own text (QUARTZFONT, LEDGER SR64): the chrome's strings measured and painted by UnaOS's
//! `font_core` through the shared host font stack (`text_host`) — the same discovery, `shape_fallback` and
//! Skia-mode rasterizer Aether paints page text with. No third-party rasterizer (ab_glyph is gone).
//!
//! - [`ui_style`] / [`parse_font_name`]: the desktop's UI font (a Pango font description such as GTK's
//!   `gtk-font-name`, "Sans 10") as a [`TextStyle`] in px.
//! - [`draw_text`] / [`measure_text_height`]: word-wrapped text into a 32-bit software buffer.
//! - The GTK face's [`crate::platforms::gtk::glyph`] label and entry draw with [`TextStyle::shape`] and
//!   [`Line::paint`].

pub use text_host::line::{Line, PixelOrder, TextStyle};

/// The UI font when the platform names none: fontconfig's `sans` at GTK's default "Sans 10" (10 pt at
/// 96 dpi = 13⅓ px).
pub fn ui_style() -> TextStyle {
    TextStyle::new(&["sans"], 10.0 * 96.0 / 72.0)
}

/// A Pango font description (`[FAMILY-LIST] [STYLE-OPTIONS] [SIZE]`, e.g. "Cantarell 11", "Sans Bold
/// Italic 10", "DejaVu Sans, Noto Sans 12px") as a [`TextStyle`]; points become px at `dpi`. None when it
/// names no family.
pub fn parse_font_name(desc: &str, dpi: f32) -> Option<TextStyle> {
    let mut words: Vec<&str> = desc.split_whitespace().collect();
    let mut size = 10.0 * dpi / 72.0;
    if let Some(last) = words.last() {
        if let Some(px) = last.strip_suffix("px").and_then(|n| n.parse::<f32>().ok()) {
            size = px;
            words.pop();
        } else if let Ok(pt) = last.parse::<f32>() {
            size = pt * dpi / 72.0;
            words.pop();
        }
    }
    let (mut weight, mut italic) = (400u16, false);
    while let Some(w) = words.last() {
        let lw = w.to_ascii_lowercase();
        let wt = match lw.as_str() {
            "thin" => Some(100),
            "ultra-light" | "extra-light" => Some(200),
            "light" => Some(300),
            "semi-light" | "demi-light" => Some(350),
            "book" => Some(380),
            "normal" | "regular" | "roman" => Some(400),
            "medium" => Some(500),
            "semi-bold" | "demi-bold" => Some(600),
            "bold" => Some(700),
            "ultra-bold" | "extra-bold" => Some(800),
            "heavy" | "black" => Some(900),
            "ultra-black" | "extra-black" => Some(1000),
            _ => None,
        };
        if let Some(v) = wt {
            if lw != "normal" && lw != "roman" {
                weight = v;
            }
        } else if lw == "italic" || lw == "oblique" {
            italic = true;
        } else if !matches!(
            lw.as_str(),
            "small-caps" | "ultra-condensed" | "extra-condensed" | "condensed" | "semi-condensed" | "semi-expanded"
                | "expanded" | "extra-expanded" | "ultra-expanded"
        ) {
            break;
        }
        words.pop();
    }
    let families: Vec<String> =
        words.join(" ").split(',').map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).collect();
    if families.is_empty() || !(size > 0.0) {
        return None;
    }
    Some(TextStyle { families, size, weight, italic })
}

/// Lines of `text` wrapped to `max_w` px: paragraphs at '\n', breaks at spaces, a word wider than a line
/// broken at grapheme boundaries.
pub fn wrap(text: &str, style: &TextStyle, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if style.width(&cand) <= max_w || cand.is_empty() {
                line = cand;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if style.width(word) <= max_w {
                line = word.to_string();
                continue;
            }
            // a giant word: as many graphemes per line as fit (at least one)
            for (a, b) in text_host::font_core::grapheme::clusters(word) {
                let g = &word[a..b];
                let cand = format!("{line}{g}");
                if !line.is_empty() && style.width(&cand) > max_w {
                    out.push(std::mem::replace(&mut line, g.to_string()));
                } else {
                    line = cand;
                }
            }
        }
        out.push(line);
    }
    out
}

/// Draws `text` wrapped inside `start_x..width` into a `width`×`height` buffer of little-endian 0xAARRGGBB
/// pixels, the first line's top at `start_y`, ink `color` (0xRRGGBB). Returns the y below the last line.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    text: &str,
    start_x: i32,
    start_y: i32,
    color: u32,
    style: &TextStyle,
) -> i32 {
    let ink = [(color >> 16) as u8, (color >> 8) as u8, color as u8, 255];
    let max_w = (width as i32 - start_x).max(1) as f32;
    let mut bytes: Vec<u8> = buffer.iter().flat_map(|p| p.to_le_bytes()).collect();
    let mut y = start_y as f32;
    for l in wrap(text, style, max_w) {
        let line = style.shape(&l);
        let lh = line.height().max(1.0);
        line.paint(
            &mut bytes,
            width,
            height,
            width as usize * 4,
            PixelOrder::Bgra,
            start_x as f32,
            y + line.ascent,
            ink,
            (start_x, width as i32),
        );
        y += lh;
    }
    for (p, c) in buffer.iter_mut().zip(bytes.chunks_exact(4)) {
        *p = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
    }
    y as i32
}

/// The height [`draw_text`] would use for `text` inside `width` px (from y = 0).
pub fn measure_text_height(width: u32, text: &str, style: &TextStyle) -> i32 {
    let lh = style.metrics();
    let n = wrap(text, style, width.max(1) as f32).len() as f32;
    (n * (lh.0 + lh.1 + lh.2).max(1.0)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pango_font_names_kat() {
        let s = parse_font_name("Sans 10", 96.0).unwrap();
        assert_eq!(s.families, vec!["Sans"]);
        assert!((s.size - 13.333_333).abs() < 1e-4);
        assert_eq!((s.weight, s.italic), (400, false));
        let s = parse_font_name("DejaVu Sans Bold Italic 12", 72.0).unwrap();
        assert_eq!((s.families[0].as_str(), s.size, s.weight, s.italic), ("DejaVu Sans", 12.0, 700, true));
        let s = parse_font_name("Cantarell, Noto Sans Semi-Bold 15px", 96.0).unwrap();
        assert_eq!(s.families, vec!["Cantarell", "Noto Sans"]);
        assert_eq!((s.size, s.weight), (15.0, 600));
        let s = parse_font_name("Monospace", 96.0).unwrap();
        assert_eq!(s.families, vec!["Monospace"]);
        assert!((s.size - 13.333_333).abs() < 1e-4, "no size: Pango's 10 pt default");
        assert!(parse_font_name("Bold 10", 96.0).is_none());
        assert!(parse_font_name("", 96.0).is_none());
    }

    #[test]
    fn wrap_and_draw() {
        let st = TextStyle::new(&["DejaVu Sans"], 13.0);
        if st.primary().map(|f| f.family.as_str()) != Some("DejaVu Sans") {
            return;
        }
        let max = st.width("The quick").max(st.width("brown fox")) + 1.0;
        assert!(st.width("The quick brown") > max);
        let lines = wrap("The quick brown fox\njumps", &st, max);
        assert_eq!(lines, vec!["The quick", "brown fox", "jumps"]);
        let giant = wrap("abcdefghij", &st, st.width("abcd") + 0.5);
        assert_eq!(giant.concat(), "abcdefghij");
        assert_eq!(giant[0], "abcd");
        let (w, h) = (120u32, 60u32);
        let mut buf = vec![0xFFFF_FFFFu32; (w * h) as usize];
        let end = draw_text(&mut buf, w, h, "Enter URL...", 4, 2, 0x000000, &st);
        assert_eq!(end, 2 + 15, "one line of 12 + 3 px");
        assert_eq!(measure_text_height(w, "Enter URL...", &st), 15);
        assert!(buf.iter().filter(|&&p| p & 0xFF < 0x80).count() > 30);
        assert!(buf.iter().all(|&p| p >> 24 == 0xFF));
    }
}
