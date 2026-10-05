//! The shaper for Latin, Greek and Cyrillic: script itemization, cmap lookup, GSUB single + ligature
//! substitution for the default features (`ccmp`, `locl`, `rlig`, `liga`, `clig`, `calt` — only their type
//! 1/4 lookups; contextual types 5/6 are owed), GPOS pair kerning (`kern`) with the legacy `kern` table as the
//! fallback exactly when GPOS has no `kern` feature for the script (HarfBuzz's rule), measuring, UAX #14
//! line layout and drawing through the glyph cache.

use crate::cache::{split_position, GlyphCache};
use crate::layout::{ligature_subst, pair_adjust, single_subst, Gdef, LayoutTable, Tag};
use crate::linebreak::{breaks, Break};
use crate::Font;
use alloc::vec::Vec;

/// A positioned glyph, in font units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphPos {
    pub glyph: u16,
    /// Byte offset of the first source character this glyph represents.
    pub cluster: usize,
    pub x_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct ShapeOptions {
    pub kerning: bool,
    pub ligatures: bool,
}

impl Default for ShapeOptions {
    fn default() -> Self {
        ShapeOptions { kerning: true, ligatures: true }
    }
}

/// OpenType script of a character (`None` for script-neutral characters: spaces, digits, punctuation).
pub fn char_script(c: char) -> Option<Tag> {
    let cp = c as u32;
    match cp {
        0x0370..=0x03FF | 0x1F00..=0x1FFF => Some(*b"grek"),
        0x0400..=0x052F | 0x1C80..=0x1C8F | 0x2DE0..=0x2DFF | 0xA640..=0xA69F => Some(*b"cyrl"),
        0x41..=0x5A | 0x61..=0x7A | 0xAA | 0xBA | 0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x24F | 0x1E00..=0x1EFF
        | 0xA720..=0xA7FF | 0xFB00..=0xFB06 => Some(*b"latn"),
        _ => None,
    }
}

/// Split text into (byte range, script) runs; neutral characters join the run before them (or after, at start).
pub fn itemize(text: &str) -> Vec<(usize, usize, Tag)> {
    let mut runs: Vec<(usize, usize, Tag)> = Vec::new();
    let mut cur: Option<Tag> = None;
    let mut run_start = 0usize;
    for (i, c) in text.char_indices() {
        if let Some(s) = char_script(c) {
            match cur {
                None => cur = Some(s), // leading neutrals join the first run (run_start stays 0)
                Some(t) if t != s => {
                    runs.push((run_start, i, t));
                    cur = Some(s);
                    run_start = i;
                }
                _ => {}
            }
        }
    }
    runs.push((run_start, text.len(), cur.unwrap_or(*b"latn")));
    if text.is_empty() {
        runs.clear();
    }
    runs
}

const GSUB_FEATURES: [Tag; 6] = [*b"ccmp", *b"locl", *b"rlig", *b"liga", *b"clig", *b"calt"];
const GSUB_FEATURES_NOLIGA: [Tag; 4] = [*b"ccmp", *b"locl", *b"rlig", *b"calt"];

fn skipped(gdef: Option<&Gdef>, flag: u16, g: u16) -> bool {
    gdef.is_some_and(|d| d.skips(flag, g))
}

fn apply_gsub(font: &Font, t: &LayoutTable, script: Tag, opts: &ShapeOptions, buf: &mut Vec<GlyphPos>) {
    let feats: &[Tag] = if opts.ligatures { &GSUB_FEATURES } else { &GSUB_FEATURES_NOLIGA };
    let gdef = font.gdef.as_ref();
    for li in t.lookups_for(script, feats) {
        let Some(l) = t.lookup(li) else { continue };
        match l.kind {
            1 => {
                for gp in buf.iter_mut() {
                    if skipped(gdef, l.flag, gp.glyph) {
                        continue;
                    }
                    for st in &l.subtables {
                        if let Some(g) = single_subst(st, gp.glyph) {
                            gp.glyph = g;
                            break;
                        }
                    }
                }
            }
            4 => {
                let mut i = 0;
                while i < buf.len() {
                    if skipped(gdef, l.flag, buf[i].glyph) {
                        i += 1;
                        continue;
                    }
                    // Positions of the following non-skipped glyphs.
                    let mut follow: Vec<usize> = Vec::new();
                    let mut k = i + 1;
                    while k < buf.len() && follow.len() < 16 {
                        if !skipped(gdef, l.flag, buf[k].glyph) {
                            follow.push(k);
                        }
                        k += 1;
                    }
                    let mut hit = None;
                    for st in &l.subtables {
                        let r = ligature_subst(st, buf[i].glyph, |j| follow.get(j - 1).map(|&p| buf[p].glyph));
                        if r.is_some() {
                            hit = r;
                            break;
                        }
                    }
                    if let Some((lig, n)) = hit {
                        buf[i].glyph = lig;
                        // Remove the consumed components (back to front keeps indices valid).
                        for &p in follow[..n - 1].iter().rev() {
                            buf.remove(p);
                        }
                    }
                    i += 1;
                }
            }
            _ => {} // contextual / multiple / alternate substitutions: owed
        }
    }
}

fn apply_kerning(font: &Font, script: Tag, buf: &mut [GlyphPos]) {
    let gdef = font.gdef.as_ref();
    let gpos_kern = font.gpos.map(|g| g.0.lookups_for(script, &[*b"kern"])).unwrap_or_default();
    if !gpos_kern.is_empty() {
        let t = font.gpos.unwrap().0;
        for li in gpos_kern {
            let Some(l) = t.lookup(li) else { continue };
            if l.kind != 2 {
                continue; // only pair adjustment is implemented
            }
            let mut i = 0;
            while i < buf.len() {
                if skipped(gdef, l.flag, buf[i].glyph) {
                    i += 1;
                    continue;
                }
                let mut j = i + 1;
                while j < buf.len() && skipped(gdef, l.flag, buf[j].glyph) {
                    j += 1;
                }
                if j >= buf.len() {
                    break;
                }
                let mut next = j;
                for st in &l.subtables {
                    if let Some((v1, v2, has2)) = pair_adjust(st, buf[i].glyph, buf[j].glyph) {
                        buf[i].x_advance += v1.x_advance as i32;
                        buf[i].x_offset += v1.x_placement as i32;
                        buf[i].y_offset += v1.y_placement as i32;
                        buf[j].x_advance += v2.x_advance as i32;
                        buf[j].x_offset += v2.x_placement as i32;
                        buf[j].y_offset += v2.y_placement as i32;
                        if has2 {
                            next = j + 1;
                        }
                        break;
                    }
                }
                i = next;
            }
        }
    } else if let Some(k) = font.kern {
        for i in 0..buf.len().saturating_sub(1) {
            buf[i].x_advance += k.pair(buf[i].glyph, buf[i + 1].glyph) as i32;
        }
    }
}

/// Shape `text` into positioned glyphs (font units).
pub fn shape(font: &Font, text: &str, opts: &ShapeOptions) -> Vec<GlyphPos> {
    let mut out = Vec::new();
    for (s, e, script) in itemize(text) {
        let mut buf: Vec<GlyphPos> = text[s..e]
            .char_indices()
            .map(|(i, c)| GlyphPos { glyph: font.glyph_index(c), cluster: s + i, x_advance: 0, x_offset: 0, y_offset: 0 })
            .collect();
        if let Some(g) = font.gsub {
            apply_gsub(font, &g.0, script, opts, &mut buf);
        }
        for gp in buf.iter_mut() {
            gp.x_advance = font.advance(gp.glyph) as i32;
        }
        if opts.kerning {
            apply_kerning(font, script, &mut buf);
        }
        out.extend(buf);
    }
    out
}

/// Advance width of `text` at `size` px (fractional, unhinted — what Chromium's measureText reports with
/// subpixel positioning).
pub fn measure(font: &Font, text: &str, size: f32, opts: &ShapeOptions) -> f32 {
    let units: i64 = shape(font, text, opts).iter().map(|g| g.x_advance as i64).sum();
    units as f32 * size / font.units_per_em as f32
}

/// One laid-out line: byte range into the source and its width in px (trailing spaces excluded).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub start: usize,
    pub end: usize,
    pub width: f32,
}

fn trim_end_width(font: &Font, text: &str, size: f32, opts: &ShapeOptions) -> f32 {
    measure(font, text.trim_end_matches([' ', '\t', '\n', '\r', '\u{2028}', '\u{2029}', '\u{0085}', '\u{000B}', '\u{000C}']), size, opts)
}

/// Greedy line layout on UAX #14 opportunities: each line takes as many break-delimited segments as fit
/// `max_width`; a mandatory break ends a line; a single segment wider than the line overflows on its own.
pub fn layout_lines(font: &Font, text: &str, size: f32, max_width: f32, opts: &ShapeOptions) -> Vec<Line> {
    let b = breaks(text);
    let offs: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(core::iter::once(text.len())).collect();
    let mut lines = Vec::new();
    let mut start = 0usize; // byte
    let mut last_fit: Option<usize> = None; // byte offset of the last opportunity that fits
    for (ci, &br) in b.iter().enumerate().skip(1) {
        if br == Break::None {
            continue;
        }
        let pos = offs[ci];
        let w = trim_end_width(font, &text[start..pos], size, opts);
        if w <= max_width || last_fit.is_none() {
            last_fit = Some(pos);
            if br == Break::Mandatory {
                lines.push(Line { start, end: pos, width: w });
                start = pos;
                last_fit = None;
            }
            continue;
        }
        // Overflow: end the line at the last fitting opportunity and retry this one on the new line.
        let end = last_fit.unwrap();
        lines.push(Line { start, end, width: trim_end_width(font, &text[start..end], size, opts) });
        start = end;
        let w2 = trim_end_width(font, &text[start..pos], size, opts);
        last_fit = Some(pos);
        if br == Break::Mandatory {
            lines.push(Line { start, end: pos, width: w2 });
            start = pos;
            last_fit = None;
        }
    }
    if start < text.len() && lines.last().map_or(true, |l: &Line| l.end < text.len()) {
        lines.push(Line { start, end: text.len(), width: trim_end_width(font, &text[start..], size, opts) });
    }
    lines
}

/// An 8-bit coverage canvas (row-major, `width * height`).
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        Canvas { width, height, data: alloc::vec![0; width * height] }
    }
}

/// Draw shaped `text` with its baseline origin at (`x`, `y`) px (y down) into `canvas`, compositing glyph
/// coverage with source-over (`1 - (1-a)(1-b)`). Glyph origins snap to the cache's subpixel grid in x and to
/// whole pixels in y (as Skia does for horizontal text). Returns the pen advance in px.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    cache: &mut GlyphCache,
    font_id: u32,
    font: &Font,
    text: &str,
    size: f32,
    x: f32,
    y: f32,
    canvas: &mut Canvas,
    opts: &ShapeOptions,
) -> f32 {
    let scale = size / font.units_per_em as f32;
    let mut pen = x;
    for g in shape(font, text, opts) {
        let gx = pen + g.x_offset as f32 * scale;
        let gy = y - g.y_offset as f32 * scale;
        let (ix, sx) = split_position(gx);
        let iy = crate::fmath::round(gy) as i32;
        if let Some(bm) = cache.get(font_id, font, g.glyph, size, sx, 0) {
            for row in 0..bm.height as i32 {
                let cy = iy + bm.top + row;
                if cy < 0 || cy >= canvas.height as i32 {
                    continue;
                }
                for col in 0..bm.width as i32 {
                    let cx = ix + bm.left + col;
                    if cx < 0 || cx >= canvas.width as i32 {
                        continue;
                    }
                    let a = bm.data[(row * bm.width as i32 + col) as usize] as u32;
                    let d = &mut canvas.data[cy as usize * canvas.width + cx as usize];
                    let b = *d as u32;
                    *d = (a + b - (a * b + 127) / 255).min(255) as u8;
                }
            }
        }
        pen += g.x_advance as f32 * scale;
    }
    pen - x
}
