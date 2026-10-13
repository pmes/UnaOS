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
//!
//! FONTSCRAFT (rmbp-ledger B640, R124, DESIGN-LANGUAGE §2.2) adds what the language's raster needs:
//! * Variable faces instanced per draw ([`Font::set_variation`]): `wght` from [`Style::weight`] (or
//!   [`Engine::bold_weight`], 600, when `bold` asks a variable slot for a weight it was not loaded at) and `opsz`
//!   set automatically to the size in pt ([`Engine::set_px_per_pt`]).
//! * OpenType features on request ([`Style::features`]: `tnum`, `zero`, `ss01`, `ss02`, `cv01`, `cv05`, `cv08`,
//!   `case`), `kern`/`liga`/`calt` always on; tracking ([`Style::tracking`], em).
//! * Per-role cached ATLASES: the glyph cache is split by (role, bold, weight, size) — one atlas per type role at
//!   the panel's scale — under the one byte bound; an overflow drops whole atlases least-recently-used first, so
//!   drawing a 40-pt clock never flushes the 13-pt body.
//! * The fallback chain primary → [`Role::Fallback`] (Noto Sans, then DejaVu Sans) → [`Role::Script`] faces.
//!
//! FONTS2 (rmbp-ledger B650) reads two more variation tables through [`crate::Font`]: `MVAR` (the line metrics,
//! x-height, cap height and underline of an instance; [`Engine::line_metrics`] takes the regular instance at the
//! size) and GSUB/GPOS FeatureVariations (a feature's lookups at the instance's coordinates, inside the shaping
//! plan). `tnum` for figures that change in place is the KERNEL seam's rule (`video/text.rs`), not the engine's.

use crate::bidi::Direction;
use crate::cache::{split_position, CacheKey, SUBPIXEL_STEPS};
use crate::fmath::{floor, round};
use crate::raster::{rasterize_glyph_mode, GlyphBitmap, RenderMode};
use crate::shape::{shape_fallback_with, ShapeOptions};
use crate::Font;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// What a face is FOR. The primary of a [`Style`] is the slot with its role (and weight); [`Role::Fallback`]
/// slots (FONTSCRAFT: Noto Sans, then DejaVu Sans) are tried after it, then the [`Role::Script`] slots.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Sans,
    Serif,
    Mono,
    Script,
    /// FONTSCRAFT (B640): the general fallback faces, in load order, after the primary and before the scripts —
    /// DESIGN-LANGUAGE §2.2's chain Inter → Noto Sans → DejaVu Sans (→ the kernel's bitmap atlas).
    Fallback,
}

/// One loaded face.
pub struct Slot<'a> {
    pub name: &'static str,
    pub role: Role,
    pub bold: bool,
    pub font: Font<'a>,
}

/// FONTSCRAFT (B640): OpenType features a [`Style`] turns on (bits of [`Style::features`]); `kern`, `liga` and
/// `calt` are always on.
pub const FEAT_TNUM: u8 = 1;
pub const FEAT_ZERO: u8 = 2;
pub const FEAT_SS01: u8 = 4;
pub const FEAT_SS02: u8 = 8;
pub const FEAT_CV01: u8 = 16;
pub const FEAT_CV05: u8 = 32;
pub const FEAT_CV08: u8 = 64;
pub const FEAT_CASE: u8 = 128;

/// The OpenType tags of a [`Style::features`] mask.
pub fn feature_tags(bits: u8) -> Vec<crate::layout::Tag> {
    let all: [(u8, &[u8; 4]); 8] = [
        (FEAT_TNUM, b"tnum"),
        (FEAT_ZERO, b"zero"),
        (FEAT_SS01, b"ss01"),
        (FEAT_SS02, b"ss02"),
        (FEAT_CV01, b"cv01"),
        (FEAT_CV05, b"cv05"),
        (FEAT_CV08, b"cv08"),
        (FEAT_CASE, b"case"),
    ];
    all.iter().filter(|(b, _)| bits & b != 0).map(|(_, t)| **t).collect()
}

/// How a run is drawn: role, weight and size in device px; FONTSCRAFT adds an explicit `wght` (0 = by `bold`),
/// OpenType features ([`FEAT_TNUM`] …) and tracking (em fraction, + looser).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Style {
    pub role: Role,
    pub bold: bool,
    pub size: f32,
    /// `wght` for a variable face (100–900); 0 = the slot's own weight, or [`Engine::bold_weight`] when `bold`
    /// asks for a weight the slot does not have.
    pub weight: u16,
    pub features: u8,
    /// Letter spacing in em (DESIGN-LANGUAGE §2.2's tracking column: +0.5 % = 0.005).
    pub tracking: f32,
}

impl Style {
    /// A style with no weight override, no extra features and no tracking (KERNELFONT's three fields).
    pub const fn new(role: Role, bold: bool, size: f32) -> Self {
        Style { role, bold, size, weight: 0, features: 0, tracking: 0.0 }
    }
    pub const fn with_weight(mut self, w: u16) -> Self {
        self.weight = w;
        self
    }
    pub const fn with_features(mut self, f: u8) -> Self {
        self.features = f;
        self
    }
    pub const fn with_tracking(mut self, t: f32) -> Self {
        self.tracking = t;
        self
    }
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
    style: Style,
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
    /// FONTSCRAFT (B640): live per-role atlases (one per role, weight and size drawn).
    pub atlases: usize,
}

/// Bookkeeping charged per cache entry on top of its coverage bytes (key, map node, bitmap header).
pub const ENTRY_OVERHEAD: usize = 64;
const RUN_CAP: usize = 64;
/// FONTSCRAFT (B640): the most per-role atlases kept at once (the §2.2 ramp has ten roles; a frame draws a few).
pub const ATLAS_MAX: usize = 12;

/// What one atlas holds: every glyph of one (role, bold, weight, size) — one type role at the panel's scale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct AtlasKey {
    role: Role,
    bold: bool,
    weight: u16,
    size_64: u32,
}

struct Atlas {
    key: AtlasKey,
    map: BTreeMap<CacheKey, Option<GlyphBitmap>>,
    bytes: usize,
    used: u64,
}

/// The engine: faces, the per-role atlases (one byte bound over all of them), the run cache, the counters.
pub struct Engine<'a> {
    slots: Vec<Slot<'a>>,
    atlases: Vec<Atlas>,
    tick: u64,
    runs: Vec<Run>,
    run_next: usize,
    pub mode: RenderMode,
    /// Glyph hinting (FONTHINT SR62). `None` (the KERNELFONT default) draws unhinted outlines; `Slight` is what a
    /// Linux desktop's fontconfig `hintslight` gives installed faces — the light auto-hinter on TrueType faces.
    pub hinting: crate::hint::Hinting,
    hinters: BTreeMap<u8, crate::hint::autofit::AutoHinter>,
    /// FONTSCRAFT (B640): device px per pt (the panel's UI scale); a variable face's `opsz` is the size in pt.
    px_per_pt: f32,
    /// FONTSCRAFT: the `wght` a `bold` style instances a variable face at when no bold slot is loaded — 600,
    /// DESIGN-LANGUAGE §2.2's "semibold, not bold, for emphasis below 20 pt".
    pub bold_weight: u16,
    pub stats: Stats,
}

const WGHT: [u8; 4] = *b"wght";
const OPSZ: [u8; 4] = *b"opsz";

impl<'a> Engine<'a> {
    /// An empty engine whose glyph cache holds at most `cap_bytes`.
    pub fn new(cap_bytes: usize) -> Self {
        Engine {
            slots: Vec::new(),
            atlases: Vec::new(),
            tick: 0,
            runs: Vec::new(),
            run_next: 0,
            mode: RenderMode::SkiaAaa,
            hinting: crate::hint::Hinting::None,
            hinters: BTreeMap::new(),
            px_per_pt: 1.0,
            bold_weight: 600,
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

    /// FONTSCRAFT (B640): the panel's device px per pt. A variable face with an `opsz` axis is drawn at
    /// `opsz = size_px / px_per_pt` (Inter: 14–32), the way Segoe UI Variable sets it by default. Changing it
    /// drops every atlas and run.
    pub fn set_px_per_pt(&mut self, k: f32) {
        let k = if k.is_finite() && k > 0.0 { k } else { 1.0 };
        if k != self.px_per_pt {
            self.px_per_pt = k;
            self.flush_all();
            self.runs.clear();
            self.run_next = 0;
        }
    }

    pub fn px_per_pt(&self) -> f32 {
        self.px_per_pt
    }

    fn flush_all(&mut self) {
        for a in self.atlases.drain(..) {
            self.stats.evictions += a.map.len() as u64;
        }
        self.stats.flushes += 1;
        self.stats.cache_bytes = 0;
        self.stats.atlases = 0;
    }

    fn primary(&self, role: Role, bold: bool) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.role == role && s.bold == bold)
            .or_else(|| self.slots.iter().position(|s| s.role == role && !s.bold))
            .or_else(|| self.slots.iter().position(|s| s.role == role))
    }

    /// The fallback stack for a style: its primary, then (for serif/mono) the sans face of the same weight, then
    /// the [`Role::Fallback`] faces of that weight (a variable fallback serves both weights), then every script
    /// face.
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
            if s.role == Role::Fallback && !v.contains(&i) && (s.bold == bold || (!s.bold && s.font.has_axis(WGHT))) {
                v.push(i);
            }
        }
        for (i, s) in self.slots.iter().enumerate() {
            if s.role == Role::Script {
                v.push(i);
            }
        }
        v
    }

    /// The `wght` slot `face` is instanced at for `style` (0 = the face as loaded).
    fn weight_for(&self, face: usize, style: &Style) -> u16 {
        let Some(s) = self.slots.get(face) else { return 0 };
        if !s.font.has_axis(WGHT) {
            return 0;
        }
        if style.weight != 0 {
            style.weight
        } else if style.bold && !s.bold {
            self.bold_weight
        } else {
            0
        }
    }

    /// Slot `face` instanced for `style`: `wght` per [`Engine::weight_for`], `opsz` from the size in pt.
    fn instance(&self, face: usize, style: &Style) -> Font<'a> {
        let s = &self.slots[face];
        let mut f = s.font;
        let w = self.weight_for(face, style);
        if w != 0 {
            f.set_variation(WGHT, w as f32);
        }
        if f.has_axis(OPSZ) {
            f.set_variation(OPSZ, style.size / self.px_per_pt);
        }
        f
    }

    /// The instance a role's primary face draws `style` with — what a witness names (`wght`, `opsz`).
    pub fn primary_instance(&self, style: &Style) -> Option<Font<'a>> {
        Some(self.instance(self.primary(style.role, style.bold)?, style))
    }

    /// The name of the primary face of `role`/`bold` (the slot's wire name).
    pub fn primary_name(&self, role: Role, bold: bool) -> Option<&'static str> {
        Some(self.slots[self.primary(role, bold)?].name)
    }

    /// (ascent, descent, line height) in px at `size` for the primary regular face of `role` — browser line
    /// metrics ([`Font::line_metrics`]); descent is positive below the baseline. FONTS2 (B650): read from the
    /// REGULAR instance at that size (`opsz` from the size, `wght` as loaded), so `MVAR` follows the optical size
    /// while a bold or heavier run never makes its line a different height.
    pub fn line_metrics(&self, role: Role, size: f32) -> Option<(f32, f32, f32)> {
        let p = self.primary(role, false)?;
        let f = self.instance(p, &Style::new(role, false, size));
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

    /// [`Engine::fit_size`] for a PROPORTIONAL face laid on a grid that was built for a mono cell: also caps
    /// the size so the mean advance of `a`..=`z` (of the `bold` face when loaded) fits `cell_w`, so a caller
    /// whose budgets are `len * cell_w` keeps holding on average.
    pub fn fit_size_mean(&self, role: Role, bold: bool, cell_w: f32, cell_h: f32) -> Option<f32> {
        let size = self.fit_size(role, None, cell_h)?;
        let p = self.primary(role, bold)?;
        let f = self.instance(p, &Style::new(role, bold, size));
        let upem = f.units_per_em.max(1) as f32;
        let sum: u32 = ('a'..='z').map(|c| f.advance(f.glyph_index(c)) as u32).sum();
        let mean = sum as f32 / 26.0 / upem;
        if mean <= 0.0 {
            return Some(size);
        }
        Some(floor(size.min(cell_w / mean) * 4.0) / 4.0)
    }

    /// The baseline's row inside a `cell_h` cell, the line box centred in it.
    pub fn baseline_in_cell(&self, role: Role, size: f32, cell_h: f32) -> f32 {
        match self.line_metrics(role, size) {
            Some((a, d, _)) => round((cell_h - (a + d)) / 2.0 + a),
            None => round(cell_h * 0.8),
        }
    }

    /// The baseline inside a line box of `line_h` px (DESIGN-LANGUAGE §2.2's line heights): the face's ascent plus
    /// half the leading, the CSS rule.
    pub fn baseline_in_line(&self, role: Role, size: f32, line_h: f32) -> f32 {
        match self.line_metrics(role, size) {
            Some((a, d, _)) => round((line_h - (a + d)) / 2.0 + a),
            None => round(line_h * 0.8),
        }
    }

    /// Shape `text` in `style` (bidi, script runs, per-cluster fallback, variable instances, features, tracking),
    /// cached. `None` when no face of the role (or sans) is loaded.
    fn shape_run(&mut self, text: &[u8], style: Style) -> Option<usize> {
        let size_64 = floor(style.size * 64.0 + 0.5) as u32;
        if let Some(i) = self.runs.iter().position(|r| {
            r.size_64 == size_64
                && r.style.role == style.role
                && r.style.bold == style.bold
                && r.style.weight == style.weight
                && r.style.features == style.features
                && r.style.tracking == style.tracking
                && r.text == text
        }) {
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
        let size = size_64 as f32 / 64.0;
        let qs = Style { size, ..style };
        let inst: Vec<Font<'a>> = stack.iter().map(|&i| self.instance(i, &qs)).collect();
        let fonts: Vec<&Font> = inst.iter().collect();
        let extra = feature_tags(style.features);
        let track = style.tracking * size;
        let mut pen = 0f32;
        let mut glyphs = Vec::new();
        for (fi, g) in shape_fallback_with(&fonts, &s, &ShapeOptions::default(), Direction::Auto, &extra) {
            let f = fonts[fi];
            let sc = size / f.units_per_em.max(1) as f32;
            let mut adv = g.x_advance as f32 * sc;
            if track != 0.0 && g.x_advance != 0 {
                adv += track;
            }
            glyphs.push(Placed { face: stack[fi] as u8, glyph: g.glyph, x: pen + g.x_offset as f32 * sc, y: -(g.y_offset as f32) * sc, adv });
            pen += adv;
        }
        self.stats.runs_shaped += 1;
        let run = Run { style, size_64, text: text.to_vec(), glyphs, width: pen };
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

    /// The atlas index for `style` (created, or the least recently used one recycled, on a miss).
    fn atlas_for(&mut self, style: &Style) -> usize {
        let key = AtlasKey { role: style.role, bold: style.bold, weight: style.weight, size_64: floor(style.size * 64.0 + 0.5) as u32 };
        self.tick += 1;
        if let Some(i) = self.atlases.iter().position(|a| a.key == key) {
            self.atlases[i].used = self.tick;
            return i;
        }
        let fresh = Atlas { key, map: BTreeMap::new(), bytes: 0, used: self.tick };
        if self.atlases.len() < ATLAS_MAX {
            self.atlases.push(fresh);
            self.stats.atlases = self.atlases.len();
            return self.atlases.len() - 1;
        }
        let lru = (0..self.atlases.len()).min_by_key(|&i| self.atlases[i].used).unwrap_or(0);
        let old = core::mem::replace(&mut self.atlases[lru], fresh);
        self.stats.evictions += old.map.len() as u64;
        self.stats.flushes += 1;
        self.stats.cache_bytes -= old.bytes;
        lru
    }

    /// Make room for `cost` bytes for atlas `keep`: whole atlases go least-recently-used first, `keep` last.
    fn make_room(&mut self, keep: usize, cost: usize) {
        while self.stats.cache_bytes + cost > self.stats.cap_bytes {
            let victim = (0..self.atlases.len())
                .filter(|&i| i != keep && !self.atlases[i].map.is_empty())
                .min_by_key(|&i| self.atlases[i].used)
                .or_else(|| (!self.atlases[keep].map.is_empty()).then_some(keep));
            let Some(v) = victim else { break };
            let a = &mut self.atlases[v];
            self.stats.evictions += a.map.len() as u64;
            self.stats.flushes += 1;
            self.stats.cache_bytes -= a.bytes;
            a.map.clear();
            a.bytes = 0;
        }
    }

    /// The coverage bitmap of one glyph of slot `face` in `style`, through atlas `ai` (byte-bounded overall).
    fn glyph(&mut self, ai: usize, face: u8, glyph: u16, style: &Style, sub_x: u8) -> Option<&GlyphBitmap> {
        let w = self.weight_for(face as usize, style);
        let key = CacheKey {
            font: face as u32 | (w as u32) << 8,
            glyph,
            size_64: floor(style.size * 64.0 + 0.5) as u32,
            sub_x: sub_x % SUBPIXEL_STEPS as u8,
            sub_y: 0,
            hinting: self.hinting,
        };
        if self.atlases[ai].map.contains_key(&key) {
            self.stats.hits += 1;
        } else {
            self.stats.misses += 1;
            if face as usize >= self.slots.len() {
                return None;
            }
            let step = 1.0 / SUBPIXEL_STEPS as f32;
            let (size_q, sx) = (key.size_64 as f32 / 64.0, key.sub_x as f32 * step);
            let font = self.instance(face as usize, &Style { size: size_q, ..*style });
            let bm = match self.hinting {
                crate::hint::Hinting::None => rasterize_glyph_mode(&font, glyph, size_q, sx, 0.0, self.mode),
                h => {
                    let hinter = self.hinters.entry(face).or_insert_with(|| crate::hint::autofit::AutoHinter::new(&font));
                    crate::raster::rasterize_glyph_hinted(&font, hinter, glyph, size_q, sx, 0.0, self.mode, h)
                }
            };
            let cost = ENTRY_OVERHEAD + bm.as_ref().map_or(0, |b| b.data.len());
            self.make_room(ai, cost);
            self.stats.cache_bytes += cost;
            let a = &mut self.atlases[ai];
            a.bytes += cost;
            a.map.insert(key, bm);
        }
        self.atlases[ai].map.get(&key).and_then(|b| b.as_ref())
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
        let ai = self.atlas_for(&style);
        let iy = round(baseline) as i32;
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
            if let Some(bm) = self.glyph(ai, p.face, p.glyph, &style, sx) {
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
        let f = self.instance(face, &style);
        let adv = f.advance(gid) as f32 * style.size / f.units_per_em.max(1) as f32;
        let lut = if luma(ink) < 128 { &PREBLEND_DARK } else { &PREBLEND_LIGHT };
        let (ix, sx) = split_position(x);
        let iy = round(baseline) as i32;
        self.stats.glyphs_drawn += 1;
        let ai = self.atlas_for(&style);
        if let Some(bm) = self.glyph(ai, face as u8, gid, &style, sx) {
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
