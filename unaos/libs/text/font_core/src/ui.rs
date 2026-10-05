// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! KERNELFONT (rmbp-ledger B359): the desktop text ENGINE, written once for both rings.
//!
//! The kernel's `video::text` is a thin fulfiller over this module: it loads the faces off `/system/fonts/`,
//! holds an [`Engine`] behind its lock and hands it 0RGB surfaces. Everything that decides what a pixel
//! becomes lives here, `no_std`, so the host harness (`tests/kernelfont.rs`) measures the kernel's own paint
//! path against Chromium.
//!
//! * Faces by ROLE ([`Role`]): sans / serif / mono, each regular + bold, and script fallbacks (Noto Sans
//!   Arabic / Hebrew / Devanagari / Thai) tried per grapheme cluster after the primary, the way a browser
//!   falls back ([`crate::shape_fallback`]).
//! * Sizing from the CELL a caller lays out on ([`Engine::fit_size`]): the line box (ascender − descender)
//!   fills the cell height; a mono face's advance also fits the cell width, so a character grid holds.
//! * Glyphs at quarter-pixel x origins (Skia's horizontal-text choice), `RenderMode::SkiaAaa` coverage
//!   (Skia analytic-AA's quarter-row edge snap), then Skia's A8 pre-blend: contrast 0.2, gamma 1.2 — the
//!   FONTCORE oracle's fit of Chromium Linux ([`PREBLEND_DARK`]). Light ink on a dark ground takes the
//!   contrast term alone ([`PREBLEND_LIGHT`]). Hinting off.
//! * A glyph cache bounded in BYTES ([`Engine::new`]); a miss that would pass the bound flushes it, and
//!   every dropped entry is counted in [`Stats::evictions`]. A shaped-run cache (64 runs) so a strip
//!   painter calling once per scanline shapes a string once.
//! * The panel's pixel density from its EDID ([`edid_ppi`]) and the CSS-px → device-px rule ([`device_px`]).

use crate::bidi::Direction;
use crate::cache::{split_position, CacheKey, SUBPIXEL_STEPS};
use crate::fmath::{floor, round};
use crate::raster::{rasterize_glyph_mode, GlyphBitmap, RenderMode};
use crate::shape::{shape_fallback, ShapeOptions};
use crate::Font;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// What a face is FOR. The primary of a [`Style`] is the slot with its role (and weight); [`Role::Script`]
/// slots are fallbacks tried after it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Sans,
    Serif,
    Mono,
    Script,
}

/// One loaded face.
pub struct Slot<'a> {
    pub name: &'static str,
    pub role: Role,
    pub bold: bool,
    pub font: Font<'a>,
}

/// How a run is drawn: role, weight and size in device px.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Style {
    pub role: Role,
    pub bold: bool,
    pub size: f32,
}

/// A shaped glyph in px, relative to the run's pen origin on the baseline (y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub face: u8,
    pub glyph: u16,
    pub x: f32,
    pub y: f32,
    pub adv: f32,
}

struct Run {
    role: Role,
    bold: bool,
    size_64: u32,
    text: Vec<u8>,
    glyphs: Vec<Placed>,
    width: f32,
}

/// Counters the kernel's witness prints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub hits: u64,
    pub misses: u64,
    /// Cache entries dropped by flushes.
    pub evictions: u64,
    pub flushes: u64,
    /// Glyphs composited (each glyph of each drawn run once).
    pub glyphs_drawn: u64,
    pub runs_shaped: u64,
    pub cache_bytes: usize,
    pub cap_bytes: usize,
}

/// Bookkeeping charged per cache entry on top of its coverage bytes (key, map node, bitmap header).
pub const ENTRY_OVERHEAD: usize = 64;
const RUN_CAP: usize = 64;

/// The engine: faces, the bounded glyph cache, the run cache, the counters.
pub struct Engine<'a> {
    slots: Vec<Slot<'a>>,
    cache: BTreeMap<CacheKey, Option<GlyphBitmap>>,
    runs: Vec<Run>,
    run_next: usize,
    pub mode: RenderMode,
    pub stats: Stats,
}

impl<'a> Engine<'a> {
    /// An empty engine whose glyph cache holds at most `cap_bytes`.
    pub fn new(cap_bytes: usize) -> Self {
        Engine {
            slots: Vec::new(),
            cache: BTreeMap::new(),
            runs: Vec::new(),
            run_next: 0,
            mode: RenderMode::SkiaAaa,
            stats: Stats { cap_bytes: cap_bytes.max(4096), ..Stats::default() },
        }
    }

    pub fn add_face(&mut self, name: &'static str, role: Role, bold: bool, font: Font<'a>) {
        if self.slots.len() < 255 {
            self.slots.push(Slot { name, role, bold, font });
            self.runs.clear();
            self.run_next = 0;
        }
    }

    pub fn face_count(&self) -> usize {
        self.slots.len()
    }

    pub fn slots(&self) -> &[Slot<'a>] {
        &self.slots
    }

    /// A regular face of `role` is loaded.
    pub fn has(&self, role: Role) -> bool {
        self.slots.iter().any(|s| s.role == role)
    }

    fn primary(&self, role: Role, bold: bool) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.role == role && s.bold == bold)
            .or_else(|| self.slots.iter().position(|s| s.role == role && !s.bold))
            .or_else(|| self.slots.iter().position(|s| s.role == role))
    }

    /// The fallback stack for a style: its primary, then (for serif/mono) the sans face of the same weight,
    /// then every script face.
    fn stack(&self, role: Role, bold: bool) -> Vec<usize> {
        let mut v = Vec::new();
        if let Some(p) = self.primary(role, bold) {
            v.push(p);
        }
        if role != Role::Sans {
            if let Some(p) = self.primary(Role::Sans, bold) {
                if !v.contains(&p) {
                    v.push(p);
                }
            }
        }
        for (i, s) in self.slots.iter().enumerate() {
            if s.role == Role::Script {
                v.push(i);
            }
        }
        v
    }

    /// (ascent, descent, line height) in px at `size` for the primary regular face of `role` — browser line
    /// metrics ([`Font::line_metrics`]); descent is positive below the baseline.
    pub fn line_metrics(&self, role: Role, size: f32) -> Option<(f32, f32, f32)> {
        let f = &self.slots[self.primary(role, false)?].font;
        let (a, d, g) = f.line_metrics();
        let s = size / f.units_per_em.max(1) as f32;
        Some((a as f32 * s, -(d as f32) * s, (a as f32 - d as f32 + g as f32) * s))
    }

    /// The largest size (quarter-px steps) whose line box (ascender − descender) fits `cell_h`; with `cell_w`,
    /// the face's advance of `'0'` must also fit it (the grid of a character-cell surface).
    pub fn fit_size(&self, role: Role, cell_w: Option<f32>, cell_h: f32) -> Option<f32> {
        let f = &self.slots[self.primary(role, false)?].font;
        let upem = f.units_per_em.max(1) as f32;
        let (a, d, _) = f.line_metrics();
        let em_h = (a as f32 - d as f32) / upem;
        if em_h <= 0.0 {
            return None;
        }
        let mut size = cell_h / em_h;
        if let Some(w) = cell_w {
            let adv = f.advance(f.glyph_index('0')) as f32 / upem;
            if adv > 0.0 {
                size = size.min(w / adv);
            }
        }
        Some(floor(size * 4.0) / 4.0)
    }

    /// The baseline's row inside a `cell_h` cell, the line box centred in it.
    pub fn baseline_in_cell(&self, role: Role, size: f32, cell_h: f32) -> f32 {
        match self.line_metrics(role, size) {
            Some((a, d, _)) => round((cell_h - (a + d)) / 2.0 + a),
            None => round(cell_h * 0.8),
        }
    }

    /// Shape `text` in `style` (bidi, script runs, per-cluster fallback), cached. `None` when no face of the
    /// role (or sans) is loaded.
    fn shape_run(&mut self, text: &[u8], style: Style) -> Option<usize> {
        let size_64 = floor(style.size * 64.0 + 0.5) as u32;
        if let Some(i) = self
            .runs
            .iter()
            .position(|r| r.size_64 == size_64 && r.role == style.role && r.bold == style.bold && r.text == text)
        {
            return Some(i);
        }
        let stack = self.stack(style.role, style.bold);
        if stack.is_empty() {
            return None;
        }
        let s = match core::str::from_utf8(text) {
            Ok(s) => alloc::borrow::Cow::Borrowed(s),
            Err(_) => alloc::borrow::Cow::Owned(text.iter().map(|&b| if b < 0x80 { b as char } else { '?' }).collect()),
        };
        let fonts: Vec<&Font> = stack.iter().map(|&i| &self.slots[i].font).collect();
        let size = size_64 as f32 / 64.0;
        let mut pen = 0f32;
        let mut glyphs = Vec::new();
        for (fi, g) in shape_fallback(&fonts, &s, &ShapeOptions::default(), Direction::Auto) {
            let f = fonts[fi];
            let sc = size / f.units_per_em.max(1) as f32;
            let adv = g.x_advance as f32 * sc;
            glyphs.push(Placed { face: stack[fi] as u8, glyph: g.glyph, x: pen + g.x_offset as f32 * sc, y: -(g.y_offset as f32) * sc, adv });
            pen += adv;
        }
        self.stats.runs_shaped += 1;
        let run = Run { role: style.role, bold: style.bold, size_64, text: text.to_vec(), glyphs, width: pen };
        let i = if self.runs.len() < RUN_CAP {
            self.runs.push(run);
            self.runs.len() - 1
        } else {
            let i = self.run_next % RUN_CAP;
            self.runs[i] = run;
            i
        };
        self.run_next = (i + 1) % RUN_CAP;
        Some(i)
    }

    /// Advance width of `text` in px (unhinted, fractional). 0 when no face can draw it.
    pub fn measure(&mut self, text: &[u8], style: Style) -> f32 {
        match self.shape_run(text, style) {
            Some(i) => self.runs[i].width,
            None => 0.0,
        }
    }

    /// The shaped glyphs of `text` (px, visual order), for callers that place them themselves.
    pub fn placed(&mut self, text: &[u8], style: Style) -> Vec<Placed> {
        match self.shape_run(text, style) {
            Some(i) => self.runs[i].glyphs.clone(),
            None => Vec::new(),
        }
    }

    /// The coverage bitmap of one glyph, through the byte-bounded cache.
    fn glyph(&mut self, face: u8, glyph: u16, size: f32, sub_x: u8) -> Option<&GlyphBitmap> {
        let key = CacheKey { font: face as u32, glyph, size_64: floor(size * 64.0 + 0.5) as u32, sub_x: sub_x % SUBPIXEL_STEPS as u8, sub_y: 0 };
        if self.cache.contains_key(&key) {
            self.stats.hits += 1;
        } else {
            self.stats.misses += 1;
            let slot = self.slots.get(face as usize)?;
            let step = 1.0 / SUBPIXEL_STEPS as f32;
            let bm = rasterize_glyph_mode(&slot.font, glyph, key.size_64 as f32 / 64.0, key.sub_x as f32 * step, 0.0, self.mode);
            let cost = ENTRY_OVERHEAD + bm.as_ref().map_or(0, |b| b.data.len());
            if self.stats.cache_bytes + cost > self.stats.cap_bytes && !self.cache.is_empty() {
                self.stats.evictions += self.cache.len() as u64;
                self.stats.flushes += 1;
                self.cache.clear();
                self.stats.cache_bytes = 0;
            }
            self.stats.cache_bytes += cost;
            self.cache.insert(key, bm);
        }
        self.cache.get(&key).and_then(|b| b.as_ref())
    }

    /// Composite `text` with its pen origin at (`x`, `baseline`) px: `put(px, py, coverage)` for every
    /// covered pixel, coverage already through the pre-blend for `ink`. With `max_x`, the run ENDS at the
    /// first glyph whose advance would pass it (all-or-nothing, the kernel's clip rule). With `row`, only
    /// that pixel row is visited (a strip painter's scanline). Returns the pen advance drawn, in px.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_with(
        &mut self,
        text: &[u8],
        style: Style,
        x: f32,
        baseline: f32,
        max_x: Option<f32>,
        row: Option<i32>,
        ink: u32,
        put: &mut dyn FnMut(i32, i32, u8),
    ) -> f32 {
        let Some(ri) = self.shape_run(text, style) else { return 0.0 };
        let lut = if luma(ink) < 128 { &PREBLEND_DARK } else { &PREBLEND_LIGHT };
        let glyphs = core::mem::take(&mut self.runs[ri].glyphs);
        let iy = round(baseline) as i32;
        let size = style.size;
        let mut pen = 0f32;
        for p in &glyphs {
            if let Some(m) = max_x {
                if x + pen + p.adv > m + 0.01 {
                    break;
                }
            }
            let (ix, sx) = split_position(x + p.x);
            let gy = iy + round(p.y) as i32;
            if row.map_or(true, |r| r == iy) {
                self.stats.glyphs_drawn += 1;
            }
            if let Some(bm) = self.glyph(p.face, p.glyph, size, sx) {
                let (w, h) = (bm.width as i32, bm.height as i32);
                let (r0, r1) = match row {
                    Some(r) => {
                        let rr = r - (gy + bm.top);
                        if rr < 0 || rr >= h {
                            (0, 0)
                        } else {
                            (rr, rr + 1)
                        }
                    }
                    None => (0, h),
                };
                for ry in r0..r1 {
                    let base = (ry * w) as usize;
                    for cx in 0..w {
                        let a = lut[bm.data[base + cx as usize] as usize];
                        if a != 0 {
                            put(ix + bm.left + cx, gy + bm.top + ry, a);
                        }
                    }
                }
            }
            pen += p.adv;
        }
        self.runs[ri].glyphs = glyphs;
        pen
    }

    /// One character without shaping (a character-grid cell: the console). The glyph comes from the first
    /// face of the style's stack whose cmap has it (the primary's .notdef when none does); its pen origin is
    /// (`x`, `baseline`). Pixels go to `put` like [`Engine::draw_with`]. Returns the advance in px.
    pub fn draw_char_with(&mut self, c: char, style: Style, x: f32, baseline: f32, ink: u32, put: &mut dyn FnMut(i32, i32, u8)) -> f32 {
        let stack = self.stack(style.role, style.bold);
        let Some(&first) = stack.first() else { return 0.0 };
        let (face, gid) = stack
            .iter()
            .find_map(|&i| {
                let g = self.slots[i].font.glyph_index(c);
                (g != 0).then_some((i, g))
            })
            .unwrap_or((first, 0));
        let f = &self.slots[face].font;
        let adv = f.advance(gid) as f32 * style.size / f.units_per_em.max(1) as f32;
        let lut = if luma(ink) < 128 { &PREBLEND_DARK } else { &PREBLEND_LIGHT };
        let (ix, sx) = split_position(x);
        let iy = round(baseline) as i32;
        self.stats.glyphs_drawn += 1;
        if let Some(bm) = self.glyph(face as u8, gid, style.size, sx) {
            let w = bm.width as i32;
            for ry in 0..bm.height as i32 {
                for cx in 0..w {
                    let a = lut[bm.data[(ry * w + cx) as usize] as usize];
                    if a != 0 {
                        put(ix + bm.left + cx, iy + bm.top + ry, a);
                    }
                }
            }
        }
        adv
    }

    /// [`Engine::draw_with`] into a 0RGB surface of `stride` pixels, clipped to `clip_w` x `clip_h` (glyphs
    /// end at `clip_w`, all-or-nothing; rows outside `clip_h` are not written). Blends over what is there.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_0rgb(
        &mut self,
        px: &mut [u32],
        stride: usize,
        clip_w: usize,
        clip_h: usize,
        x: f32,
        baseline: f32,
        text: &[u8],
        style: Style,
        ink: u32,
    ) -> f32 {
        let (cw, chh) = (clip_w as i32, clip_h as i32);
        self.draw_with(text, style, x, baseline, Some(clip_w as f32), None, ink, &mut |gx, gy, a| {
            if gx >= 0 && gy >= 0 && gx < cw && gy < chh {
                let i = gy as usize * stride + gx as usize;
                if i < px.len() {
                    px[i] = blend(px[i], ink, a);
                }
            }
        })
    }
}

/// Mix `ink` over `bg` (0x00RRGGBB) at coverage `a`; 0 and 255 are bit-exact (the kernel's `font::blend`).
#[inline]
pub fn blend(bg: u32, ink: u32, a: u8) -> u32 {
    match a {
        0 => bg,
        255 => ink,
        _ => {
            let a = a as i32;
            let ch = |shift: u32| -> u32 {
                let b = ((bg >> shift) & 0xFF) as i32;
                let i = ((ink >> shift) & 0xFF) as i32;
                (b + ((i - b) * a + 127) / 255) as u32
            };
            (ch(16) << 16) | (ch(8) << 8) | ch(0)
        }
    }
}

/// Rec. 601 luma of a 0RGB colour, 0..=255.
#[inline]
pub fn luma(c: u32) -> u32 {
    (((c >> 16) & 0xFF) * 299 + ((c >> 8) & 0xFF) * 587 + (c & 0xFF) * 114) / 1000
}

/// The panel's density from an EDID base block: (horizontal active pixels, image width in mm, ppi). The
/// first detailed timing descriptor (byte 54) gives both in its own units; a descriptor without a size
/// falls back to the base block's centimetre field (byte 21). `None` when neither is present.
pub fn edid_ppi(edid: &[u8]) -> Option<(u32, u32, u32)> {
    if edid.len() < 128 {
        return None;
    }
    let d = &edid[54..72];
    if d[0] == 0 && d[1] == 0 {
        return None; // not a timing descriptor
    }
    let hactive = d[2] as u32 | ((d[4] as u32 >> 4) << 8);
    let mut mm = d[12] as u32 | ((d[14] as u32 >> 4) << 8);
    if mm == 0 {
        mm = edid[21] as u32 * 10;
    }
    if hactive == 0 || mm == 0 {
        return None;
    }
    Some((hactive, mm, (hactive * 254 + mm * 5) / (mm * 10)))
}

/// The UI size in CSS px when no preference is set.
pub const DEFAULT_CSS_PX: i64 = 13;

/// CSS px → device px at `ppi` (96 = 1:1; 0 = unknown, taken as 96).
pub fn device_px(css_px: f32, ppi: u32) -> f32 {
    let ppi = if ppi == 0 { 96 } else { ppi };
    css_px * ppi as f32 / 96.0
}

/// Skia's A8 mask pre-blend for dark ink, `1 − (1 − (a + 0.2·a(1−a)))^(1/1.2)` — Chromium Linux's
/// SK_GAMMA_CONTRAST 0.2 / SK_GAMMA_EXPONENT 1.2, the fit FONTCORE's oracle landed on exactly.
pub static PREBLEND_DARK: [u8; 256] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
    32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 44, 45, 46,
    47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62,
    63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 77,
    78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93,
    94, 95, 96, 97, 98, 99, 100, 101, 101, 102, 103, 104, 105, 106, 107, 108,
    109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 122, 123,
    124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139,
    140, 141, 142, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154,
    155, 156, 157, 158, 159, 160, 161, 162, 162, 163, 164, 165, 166, 167, 168, 169,
    170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180, 181, 182, 183, 184, 185,
    186, 187, 188, 189, 189, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200,
    201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 215, 216,
    218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231, 233, 234,
    235, 236, 237, 238, 239, 241, 242, 243, 244, 246, 247, 248, 250, 251, 253, 255,
];
/// Light ink on a dark ground: the contrast term alone, `a + 0.2·a(1−a)` (gamma 1).
pub static PREBLEND_LIGHT: [u8; 256] = [
    0, 1, 2, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15, 17, 18,
    19, 20, 21, 23, 24, 25, 26, 27, 28, 30, 31, 32, 33, 34, 35, 36,
    38, 39, 40, 41, 42, 43, 44, 46, 47, 48, 49, 50, 51, 52, 54, 55,
    56, 57, 58, 59, 60, 61, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72,
    74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 85, 86, 87, 88, 89, 90,
    91, 92, 93, 94, 95, 96, 97, 98, 100, 101, 102, 103, 104, 105, 106, 107,
    108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 123, 124,
    125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140,
    141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156,
    157, 158, 158, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171,
    172, 173, 174, 175, 176, 177, 178, 179, 179, 180, 181, 182, 183, 184, 185, 186,
    187, 188, 189, 190, 191, 192, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201,
    201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 210, 211, 212, 213, 214, 215,
    216, 217, 217, 218, 219, 220, 221, 222, 223, 223, 224, 225, 226, 227, 228, 229,
    229, 230, 231, 232, 233, 234, 235, 235, 236, 237, 238, 239, 240, 240, 241, 242,
    243, 244, 244, 245, 246, 247, 248, 249, 249, 250, 251, 252, 253, 253, 254, 255,
];
