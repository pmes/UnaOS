//! The light auto-hinter: FreeType's "autofit" latin writing system in `FT_RENDER_MODE_LIGHT`, which is what
//! Chromium on Linux gets for every TrueType face when fontconfig says `hintslight` (see [`super`]).
//!
//! Method (FreeType 2.13.2 `src/autofit`, `aflatin.c` / `afhints.c` / `afglobal.c`, reimplemented here; the
//! per-script data — blue strings, reference characters, Unicode ranges, style order — is generated into
//! `autofit_tables.rs` by `oracle/gen_autofit.py`):
//!
//! 1. **Style coverage** (`af_face_globals_compute_style_coverage`): every glyph reachable through the Unicode cmap
//!    from a script's ranges is assigned that script's default style, first style in table order wins; glyphs of
//!    a script's non-base ranges are flagged so they skip blue zones; everything else falls to the unhinted
//!    `none` style.
//! 2. **Global metrics per style** (`af_latin_metrics_init`): standard stem widths from the first available
//!    reference character (`o O 0` for Latin) — segments linked into stems, widths sorted and clustered — and the
//!    blue zones: for each blue string, every character's extreme point, classified flat or round from its
//!    neighbourhood, the median flat height as the reference and the median round one as the overshoot.
//! 3. **Scaling** (`af_latin_metrics_scale_dim`): the vertical scale is nudged so the x-height overshoot lands on
//!    the pixel grid (threshold 40/64, abandoned if it moves anything by 2 px or more); blue zones under 3/4 px
//!    tall become active with their reference rounded to the grid.
//! 4. **Per glyph** (`af_latin_hints_apply`, vertical dimension only in light mode): points and their in/out
//!    directions with near-point merging and weak-point detection, segments along the major direction, stem /
//!    serif linking scored by overlap and width, edges clustered from segments, edges snapped to the closest blue
//!    zone, then stems positioned (the anchor stem centred with the 32/26/38 offsets, later stems relative to it,
//!    widths kept unrounded — light mode never adjusts stem widths), serifs and lone edges interpolated, and
//!    finally the outline's points: edge points to their edge, strong points interpolated between edges, weak
//!    points (IUP-style) between touched neighbours. x is only scaled.
//!
//! Not implemented (the honest ceiling): the CJK and Indic writing systems (their glyphs go unhinted where
//! FreeType hints them), the HarfBuzz-driven OpenType-feature coverage (small caps, superscripts, ligature glyphs
//! not in the cmap stay in the `none` style), and HarfBuzz shaping of multi-character blue clusters.

use super::autofit_tables::{SCRIPTS, STYLES, STYLE_NONE_DFLT};
use super::fixed::{corner_is_flat, div_fix, msb, mul_div, mul_fix, pix_round};
use super::{Outline, TAG_CONIC, TAG_CUBIC, TAG_ON};
use crate::font::Outlines;
use crate::Font;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

/// A writing system of FreeType's autofitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WritingSystem {
    Latin,
    Cjk,
    Indic,
    Dummy,
}

/// How a style's glyphs are found: the script's Unicode ranges, or the given OpenType features.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    Default,
    Features(&'static [&'static str]),
}

/// One script of `afscript.h` with its `afranges.c` ranges.
#[derive(Debug)]
pub struct ScriptRec {
    pub name: &'static str,
    pub top_to_bottom: bool,
    pub standard: &'static str,
    pub ot_tags: &'static [&'static str],
    pub ranges: &'static [(u32, u32)],
    pub nonbase: &'static [(u32, u32)],
}

/// One style of `afstyles.h`: writing system, script, coverage and its blue strings with their properties.
#[derive(Debug)]
pub struct StyleRec {
    pub name: &'static str,
    pub ws: WritingSystem,
    pub script: usize,
    pub coverage: Coverage,
    pub blues: &'static [(&'static str, u8)],
}

const STYLE_MASK: u16 = 0x3FFF;
const STYLE_UNASSIGNED: u16 = STYLE_MASK;
const NONBASE: u16 = 0x4000;

const PROP_TOP: u8 = 1;
const PROP_SUB_TOP: u8 = 2;
const PROP_NEUTRAL: u8 = 4;
const PROP_X_HEIGHT: u8 = 8;
const PROP_LONG: u8 = 16;

const BLUE_ACTIVE: u32 = 1 << 0;
const BLUE_TOP: u32 = 1 << 1;
const BLUE_SUB_TOP: u32 = 1 << 2;
const BLUE_NEUTRAL: u32 = 1 << 3;
const BLUE_ADJUSTMENT: u32 = 1 << 4;

const DIR_NONE: i8 = 4;
const DIR_RIGHT: i8 = 1;
const DIR_LEFT: i8 = -1;
const DIR_UP: i8 = 2;
const DIR_DOWN: i8 = -2;

const FLAG_CONIC: u16 = 1 << 0;
const FLAG_CUBIC: u16 = 1 << 1;
const FLAG_CONTROL: u16 = FLAG_CONIC | FLAG_CUBIC;
const FLAG_TOUCH_X: u16 = 1 << 2;
const FLAG_TOUCH_Y: u16 = 1 << 3;
const FLAG_WEAK: u16 = 1 << 4;
const FLAG_NEAR: u16 = 1 << 5;

const EDGE_ROUND: u8 = 1 << 0;
const EDGE_SERIF: u8 = 1 << 1;
const EDGE_DONE: u8 = 1 << 2;
const EDGE_NEUTRAL: u8 = 1 << 3;

const HORZ: usize = 0;
const VERT: usize = 1;

const MAX_WIDTHS: usize = 16;

#[derive(Clone, Copy, Debug, Default)]
struct Width {
    org: i64,
    cur: i64,
    fit: i64,
}

#[derive(Clone, Copy, Debug, Default)]
struct Blue {
    rf: Width,
    shoot: Width,
    ascender: i64,
    descender: i64,
    flags: u32,
}

#[derive(Clone, Debug, Default)]
struct LatinAxis {
    widths: Vec<Width>,
    edge_distance_threshold: i64,
    standard_width: i64,
    extra_light: bool,
    blues: Vec<Blue>,
    org_scale: i64,
    org_delta: i64,
    scale: i64,
    delta: i64,
}

#[derive(Clone, Debug)]
struct LatinMetrics {
    upem: i64,
    style: usize,
    axis: [LatinAxis; 2],
}

#[derive(Clone, Copy, Debug, Default)]
struct Point {
    fx: i64,
    fy: i64,
    ox: i64,
    oy: i64,
    x: i64,
    y: i64,
    u: i64,
    v: i64,
    flags: u16,
    in_dir: i8,
    out_dir: i8,
    next: usize,
    prev: usize,
}

#[derive(Clone, Copy, Debug)]
struct Segment {
    flags: u8,
    dir: i8,
    pos: i64,
    delta: i64,
    min_coord: i64,
    max_coord: i64,
    height: i64,
    score: i64,
    link: Option<usize>,
    serif: Option<usize>,
    edge: Option<usize>,
    edge_next: usize,
    first: usize,
    last: usize,
}

const SEG0: Segment = Segment {
    flags: 0,
    dir: 0,
    pos: 0,
    delta: 0,
    min_coord: 0,
    max_coord: 0,
    height: 0,
    score: 32000,
    link: None,
    serif: None,
    edge: None,
    edge_next: 0,
    first: 0,
    last: 0,
};

#[derive(Clone, Copy, Debug)]
struct Edge {
    fpos: i64,
    opos: i64,
    pos: i64,
    flags: u8,
    dir: i8,
    scale: i64,
    /// (blue index, overshoot?) — FreeType's `blue_edge` pointer to a blue's `ref` or `shoot` width.
    blue: Option<(usize, bool)>,
    link: Option<usize>,
    serif: Option<usize>,
    first: usize,
    last: usize,
}

#[derive(Clone, Debug, Default)]
struct AxisHints {
    segments: Vec<Segment>,
    edges: Vec<Edge>,
    major_dir: i8,
}

#[derive(Clone, Debug, Default)]
struct Hints {
    points: Vec<Point>,
    contours: Vec<usize>,
    axis: [AxisHints; 2],
    x_scale: i64,
    y_scale: i64,
    x_delta: i64,
    y_delta: i64,
    upem: i64,
}

enum Slot {
    Pending,
    Latin(Box<LatinMetrics>),
    /// Hinted as the dummy writing system (scaled only).
    Dummy,
}

/// Per-face auto-hinter state: the glyph → style map and the lazily computed per-style global metrics.
/// Build it once per face with [`AutoHinter::new`] and keep it next to the face.
pub struct AutoHinter {
    styles: Vec<u16>,
    slots: Vec<Slot>,
    upem: i64,
    /// False for faces the auto-hinter does not handle (no `glyf` outlines): those are returned unhinted.
    pub truetype: bool,
}

impl AutoHinter {
    /// Assign every glyph of `font` to a style (`af_face_globals_compute_style_coverage`, cmap part).
    pub fn new(font: &Font) -> Self {
        let n = font.num_glyphs as usize;
        let mut styles = vec![STYLE_UNASSIGNED; n];
        let map = font.unicode_map();
        let lookup = |lo: u32, hi: u32| -> &[(u32, u16)] {
            let a = map.partition_point(|e| e.0 < lo);
            let b = map.partition_point(|e| e.0 <= hi);
            &map[a..b.max(a)]
        };
        let mut dflt = None;
        for (ss, st) in STYLES.iter().enumerate() {
            if st.coverage != Coverage::Default {
                gsub_coverage(font, ss, &mut styles, false);
                continue;
            }
            if st.script == super::autofit_tables::SCRIPT_LATN {
                dflt = Some(ss);
            }
            let sc = &SCRIPTS[st.script];
            for &(lo, hi) in sc.ranges {
                for &(_, g) in lookup(lo, hi) {
                    let g = g as usize;
                    if g != 0 && g < n && styles[g] & STYLE_MASK == STYLE_UNASSIGNED {
                        styles[g] = ss as u16;
                    }
                }
            }
            for &(lo, hi) in sc.nonbase {
                for &(_, g) in lookup(lo, hi) {
                    let g = g as usize;
                    if g != 0 && g < n && styles[g] & STYLE_MASK == ss as u16 {
                        styles[g] |= NONBASE;
                    }
                }
            }
        }
        for (ss, st) in STYLES.iter().enumerate() {
            if st.coverage == Coverage::Default {
                gsub_coverage(font, ss, &mut styles, false);
            }
        }
        if let Some(d) = dflt {
            gsub_coverage(font, d, &mut styles, true);
        }
        for s in styles.iter_mut() {
            if *s & STYLE_MASK == STYLE_UNASSIGNED {
                *s = (*s & !STYLE_MASK) | STYLE_NONE_DFLT as u16;
            }
        }
        let slots = (0..STYLES.len()).map(|_| Slot::Pending).collect();
        AutoHinter { styles, slots, upem: font.units_per_em.max(1) as i64, truetype: matches!(font.outlines, Outlines::Glyf(_)) }
    }

    /// The style name a glyph is hinted with (`latn_dflt`, `cyrl_dflt`, `none_dflt`, …).
    pub fn style_name(&mut self, font: &Font, gid: u16) -> &'static str {
        let s = self.resolve(font, gid);
        STYLES[s].name
    }

    fn resolve(&mut self, font: &Font, gid: u16) -> usize {
        loop {
            let style = (self.styles.get(gid as usize).copied().unwrap_or(STYLE_NONE_DFLT as u16) & STYLE_MASK) as usize;
            if let Slot::Pending = self.slots[style] {
                let st = &STYLES[style];
                self.slots[style] = match st.ws {
                    WritingSystem::Latin => match latin_metrics_init(font, style, self.upem) {
                        Some(m) => Slot::Latin(Box::new(m)),
                        None => {
                            // no blue zones: this style's glyphs fall back to `none` (af_latin_metrics_init_blues)
                            for s in self.styles.iter_mut() {
                                if (*s & STYLE_MASK) as usize == style {
                                    *s = STYLE_NONE_DFLT as u16;
                                }
                            }
                            Slot::Dummy
                        }
                    },
                    _ => Slot::Dummy,
                };
                continue;
            }
            return style;
        }
    }

    /// Diagnostics: the style's blue zones (reference, overshoot, flags) and vertical standard widths in font
    /// units, and the adjusted vertical scale at `size` px.
    pub fn debug_metrics(&mut self, font: &Font, gid: u16, size: f32) -> (Vec<(i64, i64, u32)>, Vec<i64>, i64) {
        let style = self.resolve(font, gid);
        let scale = div_fix((size * 64.0 + 0.5) as i64, self.upem);
        match &mut self.slots[style] {
            Slot::Latin(m) => {
                latin_metrics_scale(m, scale, 0, scale, 0);
                (
                    m.axis[VERT].blues.iter().map(|b| (b.rf.org, b.shoot.org, b.flags)).collect(),
                    m.axis[VERT].widths.iter().map(|w| w.org).collect(),
                    m.axis[VERT].scale,
                )
            }
            _ => (Vec::new(), Vec::new(), scale),
        }
    }

    /// The light-hinted outline of `gid` at `size` px per em (26.6, y up, origin at the pen position), as
    /// `FT_Load_Glyph(FT_LOAD_TARGET_LIGHT)` returns it for a TrueType face. `None` for a missing glyph; an
    /// empty outline for a glyph without contours.
    pub fn hint(&mut self, font: &Font, gid: u16, size: f32) -> Option<Outline> {
        let mut o = raw_outline(font, gid)?;
        if !(size > 0.0 && size < 16384.0) {
            return None;
        }
        // FT_Set_Char_Size(size·64, 72 dpi): x_scale = y_scale = FT_DivFix(size_26_6, units_per_EM).
        let size_26_6 = (size * 64.0 + 0.5) as i64;
        let scale = div_fix(size_26_6, self.upem);
        if o.points.is_empty() {
            return Some(o);
        }
        let style = self.resolve(font, gid);
        let nonbase = self.styles.get(gid as usize).map(|s| s & NONBASE != 0).unwrap_or(false);
        match &mut self.slots[style] {
            Slot::Latin(m) => {
                latin_metrics_scale(m, scale, 0, scale, 0);
                let mut h = Hints { upem: self.upem, ..Default::default() };
                h.x_scale = m.axis[HORZ].scale;
                h.x_delta = m.axis[HORZ].delta;
                h.y_scale = m.axis[VERT].scale;
                h.y_delta = m.axis[VERT].delta;
                h.reload(&o);
                let top_to_bottom = SCRIPTS[STYLES[style].script].top_to_bottom;
                // AF_HINTS_DO_VERTICAL only: light mode disables horizontal hinting.
                h.compute_segments(VERT);
                let widths = m.axis[VERT].widths.clone();
                h.link_segments(&widths, VERT);
                h.compute_edges(VERT, m.axis[VERT].edge_distance_threshold, top_to_bottom);
                if !nonbase {
                    h.compute_blue_edges(m);
                }
                h.hint_edges(VERT, m, top_to_bottom);
                h.align_edge_points(VERT);
                h.align_strong_points(VERT);
                h.align_weak_points(VERT);
                for (i, p) in h.points.iter().enumerate() {
                    o.points[i] = (p.x, p.y);
                }
            }
            _ => {
                for p in o.points.iter_mut() {
                    *p = (mul_fix(p.0, scale), mul_fix(p.1, scale));
                }
            }
        }
        Some(o)
    }
}

/// `af_shaper_get_coverage`: glyphs reachable through the style's GSUB lookups (all features of the script
/// for a default style, the style's features otherwise) join the style when still unassigned.
fn gsub_coverage(font: &Font, style: usize, styles: &mut [u16], default_script: bool) {
    let Some(gsub) = font.gsub else { return };
    let t = &gsub.0;
    let st = &STYLES[style];
    let mut tags: Vec<[u8; 4]> = SCRIPTS[st.script].ot_tags.iter().filter_map(|s| s.as_bytes().try_into().ok()).collect();
    if default_script {
        tags.push(*b"DFLT");
    } else if tags.is_empty() {
        return; // HarfBuzz maps the script to DFLT, which only the default script handles
    }
    let feats: Vec<[u8; 4]> = match st.coverage {
        Coverage::Default => Vec::new(),
        Coverage::Features(f) => f.iter().filter_map(|s| s.as_bytes().try_into().ok()).collect(),
    };
    let lookups = t.collect_lookups(&tags, if feats.is_empty() { None } else { Some(&feats) });
    if lookups.is_empty() {
        return;
    }
    let mut glyphs = super::coverage::gsub_outputs(t, &lookups);
    if st.coverage != Coverage::Default {
        // only if the feature substitutes at least one blue character
        let found = st.blues.iter().any(|(s, _)| {
            s.chars().any(|ch| {
                let g = font.glyph_index(ch);
                lookups.iter().any(|&l| super::coverage::would_substitute(t, l, g))
            })
        });
        if !found {
            return;
        }
        if let Some(gpos) = font.gpos {
            let pl = gpos.0.collect_lookups(&tags, Some(&feats));
            let pin = super::coverage::gpos_inputs(&gpos.0, &pl);
            glyphs.retain(|g| !pin.contains(g));
        }
    }
    for g in glyphs {
        let g = g as usize;
        if g < styles.len() && styles[g] == STYLE_UNASSIGNED {
            styles[g] = style as u16;
        }
    }
}

/// The glyphs one blue (or standard) character gives in `style` (`af_shaper_get_cluster`): the cmap glyph for a
/// default style; for a feature style the glyph(s) after the feature's lookups, or nothing when the feature does
/// not change it.
fn cluster_glyphs(font: &Font, style: usize, ch: char) -> Vec<u16> {
    let g = font.glyph_index(ch);
    let st = &STYLES[style];
    let Coverage::Features(feats) = st.coverage else {
        return if g != 0 { vec![g] } else { Vec::new() };
    };
    let Some(gsub) = font.gsub else { return Vec::new() };
    let tags: Vec<[u8; 4]> = SCRIPTS[st.script].ot_tags.iter().filter_map(|s| s.as_bytes().try_into().ok()).collect();
    let mut lookups = Vec::new();
    for f in feats {
        let Ok(f): Result<[u8; 4], _> = f.as_bytes().try_into() else { continue };
        if let Some(l) = gsub.0.feature_lookups(&tags, f) {
            lookups.extend(l);
        }
    }
    lookups.sort_unstable();
    lookups.dedup();
    let out = super::coverage::apply_single(&gsub.0, &lookups, g);
    if out.len() == 1 && out[0] == g {
        Vec::new()
    } else {
        out
    }
}

/// The `FT_LOAD_NO_SCALE` outline the autofitter starts from: raw TrueType points, translated by −pp1.x
/// (`xMin − lsb`, almost always 0).
fn raw_outline(font: &Font, gid: u16) -> Option<Outline> {
    let Outlines::Glyf(g) = &font.outlines else { return None };
    let mut o = g.raw_outline(gid)?;
    // pp1 = xMin − lsb of the glyph — or of its last component when that one is flagged USE_MY_METRICS (each
    // component without the flag restores the composite's own phantom points), recursively.
    let mut mg = gid;
    for _ in 0..8 {
        let Some(cs) = g.components(mg) else { break };
        match cs.last() {
            Some(c) if c.flags & 0x0200 != 0 => mg = c.glyph,
            _ => break,
        }
    }
    if let Some([xmin, _, _, _]) = g.bbox(mg) {
        let pp1 = xmin as i64 - font.lsb(mg) as i64;
        if pp1 != 0 {
            for p in o.points.iter_mut() {
                p.0 -= pp1;
            }
        }
    }
    Some(o)
}

fn chars_of(s: &str) -> impl Iterator<Item = Option<char>> + '_ {
    // af_shaper_get_cluster without HarfBuzz: space-separated clusters; a cluster of more than one character
    // yields nothing.
    s.split(' ').filter(|c| !c.is_empty()).map(|c| {
        let mut it = c.chars();
        let first = it.next();
        if it.next().is_some() { None } else { first }
    })
}

fn latin_metrics_init(font: &Font, style: usize, upem: i64) -> Option<LatinMetrics> {
    let mut m = LatinMetrics { upem, style, axis: [LatinAxis::default(), LatinAxis::default()] };
    if font.unicode_map().is_empty() {
        // FT_Select_Charmap failed: metrics stay empty, which in FreeType still "succeeds" with no blues → fails.
        return None;
    }
    init_widths(font, &mut m);
    if !init_blues(font, &mut m) {
        return None;
    }
    Some(m)
}

fn latin_constant(upem: i64, c: i64) -> i64 {
    c * upem / 2048
}

fn init_widths(font: &Font, m: &mut LatinMetrics) {
    let sc = &SCRIPTS[STYLES[m.style].script];
    let mut gid = 0u16;
    for ch in chars_of(sc.standard) {
        let Some(ch) = ch else { continue };
        let g = cluster_glyphs(font, m.style, ch);
        if g.len() > 1 {
            continue;
        }
        gid = g.first().copied().unwrap_or(0);
        if gid != 0 {
            break;
        }
    }
    let mut counts = [0usize; 2];
    let mut widths: [Vec<Width>; 2] = [Vec::new(), Vec::new()];
    if gid != 0 {
        if let Some(o) = raw_outline(font, gid) {
            if !o.points.is_empty() {
                let mut h = Hints { upem: m.upem, x_scale: 0x10000, y_scale: 0x10000, ..Default::default() };
                h.reload(&o);
                for dim in [HORZ, VERT] {
                    h.compute_segments(dim);
                    h.link_segments(&[], dim);
                    let segs = &h.axis[dim].segments;
                    let mut w = Vec::new();
                    for (i, seg) in segs.iter().enumerate() {
                        if let Some(l) = seg.link {
                            if segs[l].link == Some(i) && l > i {
                                let d = (seg.pos - segs[l].pos).abs();
                                if w.len() < MAX_WIDTHS {
                                    w.push(Width { org: d, cur: 0, fit: 0 });
                                }
                            }
                        }
                    }
                    sort_and_quantize_widths(&mut w, m.upem / 100);
                    counts[dim] = w.len();
                    widths[dim] = w;
                }
            }
        }
    }
    for dim in [HORZ, VERT] {
        let ax = &mut m.axis[dim];
        ax.widths = core::mem::take(&mut widths[dim]);
        let stdw = if counts[dim] > 0 { ax.widths[0].org } else { latin_constant(m.upem, 50) };
        ax.edge_distance_threshold = stdw / 5;
        ax.standard_width = stdw;
        ax.extra_light = false;
    }
}

fn sort_and_quantize_widths(t: &mut Vec<Width>, threshold: i64) {
    let count = t.len();
    if count <= 1 {
        return;
    }
    for i in 1..count {
        let mut j = i;
        while j > 0 {
            if t[j].org >= t[j - 1].org {
                break;
            }
            t.swap(j, j - 1);
            j -= 1;
        }
    }
    let mut cur_idx = 0usize;
    let mut cur_val = t[0].org;
    let mut i = 1usize;
    while i < count {
        if t[i].org - cur_val > threshold || i == count - 1 {
            let mut sum = 0i64;
            if t[i].org - cur_val <= threshold && i == count - 1 {
                i += 1;
            }
            let mut j = cur_idx;
            while j < i {
                sum += t[j].org;
                t[j].org = 0;
                j += 1;
            }
            t[cur_idx].org = sum / j as i64;
            if i < count - 1 {
                cur_idx = i + 1;
                cur_val = t[cur_idx].org;
            }
        }
        i += 1;
    }
    let mut w = 1usize;
    for i in 1..count {
        if t[i].org != 0 {
            t[w] = t[i];
            w += 1;
        }
    }
    t.truncate(w);
}

fn init_blues(font: &Font, m: &mut LatinMetrics) -> bool {
    let upem = m.upem;
    let flat_threshold = upem / 14;
    let st = &STYLES[m.style];
    let mut blues: Vec<Blue> = Vec::new();
    for &(string, props) in st.blues {
        let is_top = props & PROP_TOP != 0;
        let is_sub_top = props & PROP_SUB_TOP != 0;
        let is_neutral = props & PROP_NEUTRAL != 0;
        let is_long = props & PROP_LONG != 0;
        let mut flats: Vec<i64> = Vec::new();
        let mut rounds: Vec<i64> = Vec::new();
        let mut ascender = 0i64;
        let mut descender = 0i64;
        for ch in chars_of(string) {
            let Some(ch) = ch else { continue };
            let mut best_y_extremum = if is_top { i64::from(i32::MIN) } else { i64::from(i32::MAX) };
            let mut best_round = false;
            for gid in cluster_glyphs(font, m.style, ch) {
            if gid == 0 {
                continue;
            }
            let Some(o) = raw_outline(font, gid) else { continue };
            if o.points.len() <= 2 {
                continue;
            }
            let pts = &o.points;
            let tags = &o.tags;
            let on = |i: usize| tags[i] == TAG_ON;
            let mut best_point: i64 = -1;
            let mut best_contour_first: i64 = -1;
            let mut best_contour_last: i64 = -1;
            let mut best_y = 0i64;
            let mut last: i64 = -1;
            for &end in &o.ends {
                let first = last + 1;
                last = end as i64;
                if last <= first {
                    continue;
                }
                if is_top || is_sub_top {
                    for pp in first..=last {
                        let y = pts[pp as usize].1;
                        if best_point < 0 || y > best_y {
                            best_point = pp;
                            best_y = y;
                            ascender = ascender.max(best_y);
                        } else {
                            descender = descender.min(y);
                        }
                    }
                } else {
                    for pp in first..=last {
                        let y = pts[pp as usize].1;
                        if best_point < 0 || y < best_y {
                            best_point = pp;
                            best_y = y;
                            descender = descender.min(best_y);
                        } else {
                            ascender = ascender.max(y);
                        }
                    }
                }
                if best_point > best_contour_last {
                    best_contour_first = first;
                    best_contour_last = last;
                }
            }
            let mut round = false;
            if best_point >= 0 {
                let bp = best_point as usize;
                let (bcf, bcl) = (best_contour_first as usize, best_contour_last as usize);
                let best_x = pts[bp].0;
                let mut best_segment_first = bp;
                let mut best_segment_last = bp;
                let (mut best_on_point_first, mut best_on_point_last): (i64, i64) =
                    if on(bp) { (bp as i64, bp as i64) } else { (-1, -1) };
                let mut prev = bp;
                let mut next = bp;
                loop {
                    prev = if prev > bcf { prev - 1 } else { bcl };
                    let dist = (pts[prev].1 - best_y).abs();
                    if dist > 5 && (pts[prev].0 - best_x).abs() <= 20 * dist {
                        break;
                    }
                    best_segment_first = prev;
                    if on(prev) {
                        best_on_point_first = prev as i64;
                        if best_on_point_last < 0 {
                            best_on_point_last = prev as i64;
                        }
                    }
                    if prev == bp {
                        break;
                    }
                }
                loop {
                    next = if next < bcl { next + 1 } else { bcf };
                    let dist = (pts[next].1 - best_y).abs();
                    if dist > 5 && (pts[next].0 - best_x).abs() <= 20 * dist {
                        break;
                    }
                    best_segment_last = next;
                    if on(next) {
                        best_on_point_last = next as i64;
                        if best_on_point_first < 0 {
                            best_on_point_first = next as i64;
                        }
                    }
                    if next == bp {
                        break;
                    }
                }
                if is_long {
                    let length_threshold = upem / 25;
                    let dist = (pts[best_segment_last].0 - pts[best_segment_first].0).abs();
                    if dist < length_threshold
                        && best_segment_last as i64 - best_segment_first as i64 + 2 <= bcl as i64 - bcf as i64
                    {
                        let height_threshold = upem / 4;
                        let mut p_first: i64 = 0;
                        let mut p_last: i64 = 0;
                        let mut pv = bp;
                        loop {
                            pv = if pv > bcf { pv - 1 } else { bcl };
                            if pts[pv].0 != best_x || pv == bp {
                                break;
                            }
                        }
                        if pv == bp {
                            continue; // degenerate: skip this glyph
                        }
                        let left2right = pts[pv].0 < pts[bp].0;
                        let mut first = best_segment_last;
                        let mut lst = first;
                        let mut hit = false;
                        loop {
                            if !hit {
                                first = lst;
                                if on(first) {
                                    p_first = first as i64;
                                    p_last = first as i64;
                                } else {
                                    p_first = -1;
                                    p_last = -1;
                                }
                                hit = true;
                            }
                            lst = if lst < bcl { lst + 1 } else { bcf };
                            let mut cont = false;
                            if (best_y - pts[first].1).abs() > height_threshold {
                                hit = false;
                                cont = true;
                            }
                            if !cont {
                                let dist = (pts[lst].1 - pts[first].1).abs();
                                if dist > 5 && (pts[lst].0 - pts[first].0).abs() <= 20 * dist {
                                    hit = false;
                                    cont = true;
                                }
                                if !cont {
                                    if on(lst) {
                                        p_last = lst as i64;
                                        if p_first < 0 {
                                            p_first = lst as i64;
                                        }
                                    }
                                    let l2r = pts[first].0 < pts[lst].0;
                                    let d = (pts[lst].0 - pts[first].0).abs();
                                    if l2r == left2right && d >= length_threshold {
                                        loop {
                                            lst = if lst < bcl { lst + 1 } else { bcf };
                                            let d = (pts[lst].1 - pts[first].1).abs();
                                            if d > 5 && (pts[next].0 - pts[first].0).abs() <= 20 * dist {
                                                lst = if lst > bcf { lst - 1 } else { bcl };
                                                break;
                                            }
                                            p_last = lst as i64;
                                            if on(lst) {
                                                p_last = lst as i64;
                                                if p_first < 0 {
                                                    p_first = lst as i64;
                                                }
                                            }
                                            if lst == best_segment_first {
                                                break;
                                            }
                                        }
                                        best_y = pts[first].1;
                                        best_segment_first = first;
                                        best_segment_last = lst;
                                        best_on_point_first = p_first;
                                        best_on_point_last = p_last;
                                        break;
                                    }
                                }
                            }
                            if lst == best_segment_first {
                                break;
                            }
                        }
                    }
                }
                round = if best_on_point_first >= 0
                    && best_on_point_last >= 0
                    && (pts[best_on_point_last as usize].0 - pts[best_on_point_first as usize].0).abs() > flat_threshold
                {
                    false
                } else {
                    !on(best_segment_first) || !on(best_segment_last)
                };
                if round && is_neutral {
                    continue;
                }
            }
            if is_top {
                if best_y > best_y_extremum {
                    best_y_extremum = best_y;
                    best_round = round;
                }
            } else if best_y < best_y_extremum {
                best_y_extremum = best_y;
                best_round = round;
            }
            }
            if !(best_y_extremum == i64::from(i32::MIN) || best_y_extremum == i64::from(i32::MAX)) {
                if best_round {
                    rounds.push(best_y_extremum);
                } else {
                    flats.push(best_y_extremum);
                }
            }
        }
        if flats.is_empty() && rounds.is_empty() {
            continue;
        }
        sort_pos(&mut rounds);
        sort_pos(&mut flats);
        let mut b = Blue::default();
        if flats.is_empty() {
            b.rf.org = rounds[rounds.len() / 2];
            b.shoot.org = b.rf.org;
        } else if rounds.is_empty() {
            b.rf.org = flats[flats.len() / 2];
            b.shoot.org = b.rf.org;
        } else {
            b.rf.org = flats[flats.len() / 2];
            b.shoot.org = rounds[rounds.len() / 2];
        }
        if b.shoot.org != b.rf.org {
            let (r, s) = (b.rf.org, b.shoot.org);
            let over_ref = s > r;
            if (is_top || is_sub_top) ^ over_ref {
                b.rf.org = (s + r) / 2;
                b.shoot.org = b.rf.org;
            }
        }
        b.ascender = ascender;
        b.descender = descender;
        if is_top {
            b.flags |= BLUE_TOP;
        }
        if is_sub_top {
            b.flags |= BLUE_SUB_TOP;
        }
        if is_neutral {
            b.flags |= BLUE_NEUTRAL;
        }
        if props & PROP_X_HEIGHT != 0 {
            b.flags |= BLUE_ADJUSTMENT;
        }
        blues.push(b);
    }
    if blues.is_empty() {
        return false;
    }
    // sort bottoms of blue zones, then make sure tops never overlap the next zone
    let mut order: Vec<usize> = (0..blues.len()).collect();
    let key = |b: &Blue| if b.flags & (BLUE_TOP | BLUE_SUB_TOP) != 0 { b.rf.org } else { b.shoot.org };
    for i in 1..order.len() {
        let mut j = i;
        while j > 0 {
            let (a, b) = (key(&blues[order[j - 1]]), key(&blues[order[j]]));
            if b >= a {
                break;
            }
            order.swap(j, j - 1);
            j -= 1;
        }
    }
    for i in 0..order.len() - 1 {
        let (bi, bn) = (order[i], order[i + 1]);
        let b_val = if blues[bn].flags & (BLUE_TOP | BLUE_SUB_TOP) != 0 { blues[bn].shoot.org } else { blues[bn].rf.org };
        let top_i = blues[bi].flags & (BLUE_TOP | BLUE_SUB_TOP) != 0;
        let a = if top_i { &mut blues[bi].shoot.org } else { &mut blues[bi].rf.org };
        if *a > b_val {
            *a = b_val;
        }
    }
    m.axis[VERT].blues = blues;
    true
}

fn sort_pos(t: &mut [i64]) {
    for i in 1..t.len() {
        let mut j = i;
        while j > 0 {
            if t[j] >= t[j - 1] {
                break;
            }
            t.swap(j, j - 1);
            j -= 1;
        }
    }
}

fn latin_metrics_scale(m: &mut LatinMetrics, x_scale: i64, x_delta: i64, y_scale: i64, y_delta: i64) {
    scale_dim(m, HORZ, x_scale, x_delta);
    scale_dim(m, VERT, y_scale, y_delta);
}

fn scale_dim(m: &mut LatinMetrics, dim: usize, scale_in: i64, delta: i64) {
    let mut scale = scale_in;
    if m.axis[dim].org_scale == scale && m.axis[dim].org_delta == delta {
        return;
    }
    m.axis[dim].org_scale = scale;
    m.axis[dim].org_delta = delta;
    // x-height alignment (vertical only; the horizontal branch is compiled out in FreeType)
    if let Some(blue) = m.axis[VERT].blues.iter().find(|b| b.flags & BLUE_ADJUSTMENT != 0).copied() {
        let scaled = mul_fix(blue.shoot.org, scale);
        let threshold = 40; // increase-x-height is off by default
        let fitted = (scaled + threshold) & !63;
        if scaled != fitted && dim == VERT {
            let new_scale = mul_div(scale, fitted, scaled);
            let mut max_height = m.upem;
            for b in &m.axis[VERT].blues {
                max_height = max_height.max(b.ascender);
                max_height = max_height.max(-b.descender);
            }
            let mut dist = mul_fix(max_height, new_scale - scale).abs();
            dist &= !127;
            if dist == 0 {
                scale = new_scale;
            }
        }
    }
    let ax = &mut m.axis[dim];
    ax.scale = scale;
    ax.delta = delta;
    for w in ax.widths.iter_mut() {
        w.cur = mul_fix(w.org, scale);
        w.fit = w.cur;
    }
    ax.extra_light = mul_fix(ax.standard_width, scale) < 32 + 8;
    if dim == VERT {
        for b in ax.blues.iter_mut() {
            b.rf.cur = mul_fix(b.rf.org, scale) + delta;
            b.rf.fit = b.rf.cur;
            b.shoot.cur = mul_fix(b.shoot.org, scale) + delta;
            b.shoot.fit = b.shoot.cur;
            b.flags &= !BLUE_ACTIVE;
            let dist = mul_fix(b.rf.org - b.shoot.org, scale);
            if (-48..=48).contains(&dist) {
                let mut d2 = dist.abs();
                d2 = if d2 < 32 {
                    0
                } else if d2 < 48 {
                    32
                } else {
                    64
                };
                if dist < 0 {
                    d2 = -d2;
                }
                b.rf.fit = pix_round(b.rf.cur);
                b.shoot.fit = b.rf.fit - d2;
                b.flags |= BLUE_ACTIVE;
            }
        }
        // a sub-top zone is used only when it does not overlap another active zone
        let n = ax.blues.len();
        for i in 0..n {
            let b = ax.blues[i];
            if b.flags & BLUE_SUB_TOP == 0 || b.flags & BLUE_ACTIVE == 0 {
                continue;
            }
            for k in 0..n {
                let o = ax.blues[k];
                if o.flags & BLUE_SUB_TOP != 0 || o.flags & BLUE_ACTIVE == 0 {
                    continue;
                }
                if o.rf.fit <= b.shoot.fit && o.shoot.fit >= b.rf.fit {
                    ax.blues[i].flags &= !BLUE_ACTIVE;
                    break;
                }
            }
        }
    }
}

/// `af_direction_compute`.
fn direction_compute(dx: i64, dy: i64) -> i8 {
    let (dir, ll, ss) = if dy >= dx {
        if dy >= -dx { (DIR_UP, dy, dx) } else { (DIR_LEFT, -dx, dy) }
    } else if dy >= -dx {
        (DIR_RIGHT, dx, dy)
    } else {
        (DIR_DOWN, -dy, dx)
    };
    if ll <= 14 * ss.abs() { DIR_NONE } else { dir }
}

/// `FT_Outline_Get_Orientation`: true for PostScript (counter-clockwise outer) orientation.
fn is_postscript_orientation(o: &Outline) -> bool {
    if o.points.is_empty() {
        return false;
    }
    let (mut xmin, mut xmax, mut ymin, mut ymax) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
    for &(x, y) in &o.points {
        xmin = xmin.min(x);
        xmax = xmax.max(x);
        ymin = ymin.min(y);
        ymax = ymax.max(y);
    }
    if xmin == xmax || ymin == ymax {
        return false;
    }
    if xmin < -0x100_0000 || ymin < -0x100_0000 || xmax > 0x100_0000 || ymax > 0x100_0000 {
        return false;
    }
    let xshift = (msb((xmax.abs() | xmin.abs()) as u32) - 14).max(0);
    let yshift = (msb((ymax - ymin) as u32) - 14).max(0);
    let mut area = 0i64;
    let mut first = 0usize;
    for &last in &o.ends {
        if last >= o.points.len() || last < first {
            break;
        }
        let mut prev = (o.points[last].0 >> xshift, o.points[last].1 >> yshift);
        for n in first..=last {
            let cur = (o.points[n].0 >> xshift, o.points[n].1 >> yshift);
            area += (cur.1 - prev.1) * (cur.0 + prev.0);
            prev = cur;
        }
        first = last + 1;
    }
    area > 0
}

impl Hints {
    /// `af_glyph_hints_reload`.
    fn reload(&mut self, o: &Outline) {
        let n = o.points.len();
        self.axis[0] = AxisHints::default();
        self.axis[1] = AxisHints::default();
        self.axis[HORZ].major_dir = DIR_UP;
        self.axis[VERT].major_dir = DIR_LEFT;
        if is_postscript_orientation(o) {
            self.axis[HORZ].major_dir = DIR_DOWN;
            self.axis[VERT].major_dir = DIR_RIGHT;
        }
        self.points = vec![Point::default(); n];
        self.contours.clear();
        if n == 0 || o.ends.is_empty() {
            return;
        }
        let near_limit = 20 * self.upem / 2048;
        let pts = &mut self.points;
        let mut contour_index = 0usize;
        let mut endpoint = o.ends[0];
        let mut prev = endpoint;
        for i in 0..n {
            let (vx, vy) = o.points[i];
            let p = &mut pts[i];
            p.in_dir = DIR_NONE;
            p.out_dir = DIR_NONE;
            p.fx = vx as i16 as i64;
            p.fy = vy as i16 as i64;
            p.ox = mul_fix(vx, self.x_scale) + self.x_delta;
            p.x = p.ox;
            p.oy = mul_fix(vy, self.y_scale) + self.y_delta;
            p.y = p.oy;
            p.flags = match o.tags[i] {
                TAG_CONIC => FLAG_CONIC,
                TAG_CUBIC => FLAG_CUBIC,
                _ => 0,
            };
            // `end->fx = outline->points[endpoint].x` — the contour's end point gets its coordinates early so the
            // first point's `prev` delta is right.
            pts[endpoint].fx = o.points[endpoint].0 as i16 as i64;
            pts[endpoint].fy = o.points[endpoint].1 as i16 as i64;
            let (ox, oy) = (pts[i].fx - pts[prev].fx, pts[i].fy - pts[prev].fy);
            if ox.abs() + oy.abs() < near_limit {
                pts[prev].flags |= FLAG_NEAR;
            }
            pts[i].prev = prev;
            pts[prev].next = i;
            prev = i;
            if i == endpoint {
                contour_index += 1;
                if contour_index < o.ends.len() {
                    endpoint = o.ends[contour_index].min(n - 1);
                    prev = endpoint;
                }
            }
        }
        let mut idx = 0usize;
        for &e in &o.ends {
            self.contours.push(idx);
            idx = e + 1;
        }
        let near_limit2 = 2 * near_limit - 1;
        let at = |i: usize, d: i64| (i as i64 + d) as usize;
        for c in 0..self.contours.len() {
            let pts = &mut self.points;
            let first0 = self.contours[c];
            let mut point = first0;
            let mut prev = pts[first0].prev;
            while prev != first0 {
                let (ox, oy) = (pts[point].fx - pts[prev].fx, pts[point].fy - pts[prev].fy);
                if ox.abs() + oy.abs() >= near_limit2 {
                    break;
                }
                point = prev;
                prev = pts[prev].prev;
            }
            let first = point;
            let mut curr = first;
            pts[curr].u = first as i64 - curr as i64;
            pts[first].v = -pts[curr].u;
            let (mut ox, mut oy) = (0i64, 0i64);
            let mut next = first;
            loop {
                let point = next;
                next = pts[point].next;
                ox += pts[next].fx - pts[point].fx;
                oy += pts[next].fy - pts[point].fy;
                if ox.abs() + oy.abs() < near_limit {
                    pts[next].flags |= FLAG_WEAK;
                    if next == first {
                        break;
                    }
                    continue;
                }
                pts[curr].u = next as i64 - curr as i64;
                pts[next].v = -pts[curr].u;
                let out_dir = direction_compute(ox, oy);
                pts[curr].out_dir = out_dir;
                curr = pts[curr].next;
                while curr != next {
                    pts[curr].in_dir = out_dir;
                    pts[curr].out_dir = out_dir;
                    curr = pts[curr].next;
                }
                pts[next].in_dir = out_dir;
                pts[curr].u = first as i64 - curr as i64;
                pts[first].v = -pts[curr].u;
                ox = 0;
                oy = 0;
                if next == first {
                    break;
                }
            }
        }
        let pts = &mut self.points;
        for i in 0..n {
            if pts[i].flags & FLAG_WEAK != 0 {
                continue;
            }
            if pts[i].in_dir == DIR_NONE && pts[i].out_dir == DIR_NONE {
                let nu = at(i, pts[i].u);
                let pv = at(i, pts[i].v);
                let (ix, iy) = (pts[i].fx - pts[pv].fx, pts[i].fy - pts[pv].fy);
                let (ox, oy) = (pts[nu].fx - pts[i].fx, pts[nu].fy - pts[i].fy);
                if (ix ^ ox) >= 0 && (iy ^ oy) >= 0 {
                    pts[i].flags |= FLAG_WEAK;
                    pts[pv].u = nu as i64 - pv as i64;
                    pts[nu].v = -pts[pv].u;
                }
            }
        }
        for i in 0..n {
            if pts[i].flags & FLAG_WEAK != 0 {
                continue;
            }
            let mut weak = false;
            if pts[i].flags & FLAG_CONTROL != 0 {
                weak = true;
            } else if pts[i].out_dir == pts[i].in_dir {
                if pts[i].out_dir != DIR_NONE {
                    weak = true;
                } else {
                    let nu = at(i, pts[i].u);
                    let pv = at(i, pts[i].v);
                    if corner_is_flat(
                        pts[i].fx - pts[pv].fx,
                        pts[i].fy - pts[pv].fy,
                        pts[nu].fx - pts[i].fx,
                        pts[nu].fy - pts[i].fy,
                    ) {
                        pts[pv].u = nu as i64 - pv as i64;
                        pts[nu].v = -pts[pv].u;
                        weak = true;
                    }
                }
            } else if pts[i].in_dir == -pts[i].out_dir {
                weak = true;
            }
            if weak {
                pts[i].flags |= FLAG_WEAK;
            }
        }
    }

    /// `af_latin_hints_compute_segments`.
    fn compute_segments(&mut self, dim: usize) {
        let flat_threshold = self.upem / 14;
        let major_dir = self.axis[dim].major_dir.abs();
        let mut segment_dir = major_dir;
        let mut segs: Vec<Segment> = Vec::new();
        let pts = &mut self.points;
        for p in pts.iter_mut() {
            if dim == HORZ {
                p.u = p.fx;
                p.v = p.fy;
            } else {
                p.u = p.fy;
                p.v = p.fx;
            }
        }
        for &c0 in &self.contours {
            let mut point = c0;
            let mut last = pts[point].prev;
            let mut on_edge = false;
            let (mut min_pos, mut max_pos) = (32000i64, -32000i64);
            let (mut min_coord, mut max_coord) = (32000i64, -32000i64);
            let (mut min_flags, mut max_flags) = (0u16, 0u16);
            let (mut min_on, mut max_on) = (32000i64, -32000i64);
            let mut prev_seg: Option<usize> = None;
            let (mut p_min_pos, mut p_max_pos) = (min_pos, max_pos);
            let (mut p_min_coord, mut p_max_coord) = (min_coord, max_coord);
            let (mut p_min_flags, mut p_max_flags) = (min_flags, max_flags);
            let (mut p_min_on, mut p_max_on) = (min_on, max_on);
            let mut seg: Option<usize> = None;
            if pts[last].out_dir.abs() == major_dir && pts[point].out_dir.abs() == major_dir {
                last = point;
                loop {
                    point = pts[point].prev;
                    if pts[point].out_dir.abs() != major_dir {
                        point = pts[point].next;
                        break;
                    }
                    if point == last {
                        break;
                    }
                }
            }
            last = point;
            let mut passed = false;
            loop {
                if on_edge {
                    let u = pts[point].u;
                    min_pos = min_pos.min(u);
                    max_pos = max_pos.max(u);
                    let v = pts[point].v;
                    if v < min_coord {
                        min_coord = v;
                        min_flags = pts[point].flags;
                    }
                    if v > max_coord {
                        max_coord = v;
                        max_flags = pts[point].flags;
                    }
                    if pts[point].flags & FLAG_CONTROL == 0 {
                        min_on = min_on.min(v);
                        max_on = max_on.max(v);
                    }
                    if pts[point].out_dir != segment_dir || point == last {
                        let s = seg.unwrap();
                        let same_point = match prev_seg {
                            Some(ps) => segs[s].first == segs[ps].last,
                            None => false,
                        };
                        if !same_point {
                            let sg = &mut segs[s];
                            sg.last = point;
                            sg.pos = ((min_pos + max_pos) >> 1) as i16 as i64;
                            sg.delta = ((max_pos - min_pos) >> 1) as i16 as i64;
                            if (min_flags | max_flags) & FLAG_CONTROL != 0 && (max_on - min_on) < flat_threshold {
                                sg.flags |= EDGE_ROUND;
                            }
                            sg.min_coord = min_coord as i16 as i64;
                            sg.max_coord = max_coord as i16 as i64;
                            sg.height = sg.max_coord - sg.min_coord;
                            prev_seg = Some(s);
                            p_min_pos = min_pos;
                            p_max_pos = max_pos;
                            p_min_coord = min_coord;
                            p_max_coord = max_coord;
                            p_min_flags = min_flags;
                            p_max_flags = max_flags;
                            p_min_on = min_on;
                            p_max_on = max_on;
                        } else {
                            let ps = prev_seg.unwrap();
                            if pts[segs[ps].last].in_dir == pts[point].in_dir {
                                if p_min_pos < min_pos {
                                    min_pos = p_min_pos;
                                }
                                if p_max_pos > max_pos {
                                    max_pos = p_max_pos;
                                }
                                if p_min_coord < min_coord {
                                    min_coord = p_min_coord;
                                    min_flags = p_min_flags;
                                }
                                if p_max_coord > max_coord {
                                    max_coord = p_max_coord;
                                    max_flags = p_max_flags;
                                }
                                if p_min_on < min_on {
                                    min_on = p_min_on;
                                }
                                if p_max_on > max_on {
                                    max_on = p_max_on;
                                }
                                let sg = &mut segs[ps];
                                sg.last = point;
                                sg.pos = ((min_pos + max_pos) >> 1) as i16 as i64;
                                sg.delta = ((max_pos - min_pos) >> 1) as i16 as i64;
                                if (min_flags | max_flags) & FLAG_CONTROL != 0 && (max_on - min_on) < flat_threshold {
                                    sg.flags |= EDGE_ROUND;
                                } else {
                                    sg.flags &= !EDGE_ROUND;
                                }
                                sg.min_coord = min_coord as i16 as i64;
                                sg.max_coord = max_coord as i16 as i64;
                                sg.height = sg.max_coord - sg.min_coord;
                            } else if (p_max_coord - p_min_coord).abs() > (max_coord - min_coord).abs() {
                                if min_pos < p_min_pos {
                                    p_min_pos = min_pos;
                                }
                                if max_pos > p_max_pos {
                                    p_max_pos = max_pos;
                                }
                                let sg = &mut segs[ps];
                                sg.last = point;
                                sg.pos = ((p_min_pos + p_max_pos) >> 1) as i16 as i64;
                                sg.delta = ((p_max_pos - p_min_pos) >> 1) as i16 as i64;
                            } else {
                                if p_min_pos < min_pos {
                                    min_pos = p_min_pos;
                                }
                                if p_max_pos > max_pos {
                                    max_pos = p_max_pos;
                                }
                                {
                                    let sg = &mut segs[s];
                                    sg.last = point;
                                    sg.pos = ((min_pos + max_pos) >> 1) as i16 as i64;
                                    sg.delta = ((max_pos - min_pos) >> 1) as i16 as i64;
                                    if (min_flags | max_flags) & FLAG_CONTROL != 0 && (max_on - min_on) < flat_threshold {
                                        sg.flags |= EDGE_ROUND;
                                    }
                                    sg.min_coord = min_coord as i16 as i64;
                                    sg.max_coord = max_coord as i16 as i64;
                                    sg.height = sg.max_coord - sg.min_coord;
                                }
                                segs[ps] = segs[s];
                                p_min_pos = min_pos;
                                p_max_pos = max_pos;
                                p_min_coord = min_coord;
                                p_max_coord = max_coord;
                                p_min_flags = min_flags;
                                p_max_flags = max_flags;
                                p_min_on = min_on;
                                p_max_on = max_on;
                            }
                            segs.pop(); // axis->num_segments--
                        }
                        on_edge = false;
                        seg = None;
                    }
                }
                if point == last {
                    if passed {
                        break;
                    }
                    passed = true;
                }
                if !on_edge && (pts[point].out_dir.abs() == major_dir || point == pts[point].prev) {
                    if segs.len() > 1000 {
                        self.axis[dim].segments.clear();
                        return;
                    }
                    segment_dir = pts[point].out_dir;
                    let mut sg = SEG0;
                    sg.dir = segment_dir;
                    sg.first = point;
                    sg.last = point;
                    segs.push(sg);
                    let s = segs.len() - 1;
                    seg = Some(s);
                    min_pos = pts[point].u;
                    max_pos = min_pos;
                    min_coord = pts[point].v;
                    max_coord = min_coord;
                    min_flags = pts[point].flags;
                    max_flags = min_flags;
                    if pts[point].flags & FLAG_CONTROL != 0 {
                        min_on = 32000;
                        max_on = -32000;
                    } else {
                        min_on = pts[point].v;
                        max_on = min_on;
                    }
                    on_edge = true;
                    if point == pts[point].prev {
                        let sg = &mut segs[s];
                        sg.pos = min_pos as i16 as i64;
                        if pts[point].flags & FLAG_CONTROL != 0 {
                            sg.flags |= EDGE_ROUND;
                        }
                        sg.min_coord = pts[point].v as i16 as i64;
                        sg.max_coord = sg.min_coord;
                        sg.height = 0;
                        on_edge = false;
                        seg = None;
                    }
                }
                point = pts[point].next;
            }
        }
        for sg in segs.iter_mut() {
            let (first, last) = (sg.first, sg.last);
            let (fv, lv) = (pts[first].v, pts[last].v);
            if fv < lv {
                let p = pts[first].prev;
                if pts[p].v < fv {
                    sg.height = (sg.height + ((fv - pts[p].v) >> 1)) as i16 as i64;
                }
                let p = pts[last].next;
                if pts[p].v > lv {
                    sg.height = (sg.height + ((pts[p].v - lv) >> 1)) as i16 as i64;
                }
            } else {
                let p = pts[first].prev;
                if pts[p].v > fv {
                    sg.height = (sg.height + ((pts[p].v - fv) >> 1)) as i16 as i64;
                }
                let p = pts[last].next;
                if pts[p].v < lv {
                    sg.height = (sg.height + ((lv - pts[p].v) >> 1)) as i16 as i64;
                }
            }
        }
        self.axis[dim].segments = segs;
    }

    /// `af_latin_hints_link_segments`.
    fn link_segments(&mut self, widths: &[Width], dim: usize) {
        let major = self.axis[dim].major_dir;
        let segs = &mut self.axis[dim].segments;
        let max_width = widths.last().map(|w| w.org).unwrap_or(0);
        let mut len_threshold = latin_constant(self.upem, 8);
        if len_threshold == 0 {
            len_threshold = 1;
        }
        let len_score = latin_constant(self.upem, 6000);
        let dist_score = 3000i64;
        let n = segs.len();
        for i in 0..n {
            if segs[i].dir != major {
                continue;
            }
            for j in 0..n {
                let (pos1, pos2) = (segs[i].pos, segs[j].pos);
                if segs[i].dir as i32 + segs[j].dir as i32 == 0 && pos2 > pos1 {
                    let min = segs[i].min_coord.max(segs[j].min_coord);
                    let max = segs[i].max_coord.min(segs[j].max_coord);
                    let len = max - min;
                    if len >= len_threshold {
                        let dist = pos2 - pos1;
                        let dist_demerit = if max_width != 0 {
                            let delta = (dist << 10) / max_width - (1 << 10);
                            if delta > 10000 {
                                32000
                            } else if delta > 0 {
                                delta * delta / dist_score
                            } else {
                                0
                            }
                        } else {
                            dist
                        };
                        let score = dist_demerit + len_score / len;
                        if score < segs[i].score {
                            segs[i].score = score;
                            segs[i].link = Some(j);
                        }
                        if score < segs[j].score {
                            segs[j].score = score;
                            segs[j].link = Some(i);
                        }
                    }
                }
            }
        }
        for i in 0..n {
            if let Some(l) = segs[i].link {
                if segs[l].link != Some(i) {
                    segs[i].link = None;
                    segs[i].serif = segs[l].link;
                }
            }
        }
    }

    /// `af_latin_hints_compute_edges`.
    fn compute_edges(&mut self, dim: usize, laxis_edge_threshold: i64, top_to_bottom: bool) {
        let scale = if dim == HORZ { self.x_scale } else { self.y_scale };
        let top_to_bottom = dim == VERT && top_to_bottom;
        let segment_length_threshold = if dim == HORZ { div_fix(64, self.y_scale) } else { 0 };
        let segment_width_threshold = div_fix(32, scale);
        let mut edge_distance_threshold = mul_fix(laxis_edge_threshold, scale);
        if edge_distance_threshold > 64 / 4 {
            edge_distance_threshold = 64 / 4;
        }
        let edge_distance_threshold = div_fix(edge_distance_threshold, scale);
        let ax = &mut self.axis[dim];
        let major = ax.major_dir;
        let segs = &mut ax.segments;
        let edges = &mut ax.edges;
        edges.clear();
        for s in 0..segs.len() {
            let sg = segs[s];
            if sg.height < segment_length_threshold || sg.delta > segment_width_threshold || sg.dir == DIR_NONE {
                continue;
            }
            if sg.serif.is_some() && 2 * sg.height < 3 * segment_length_threshold {
                continue;
            }
            let mut found = None;
            for (ee, e) in edges.iter().enumerate() {
                let dist = (sg.pos - e.fpos).abs();
                if dist < edge_distance_threshold && e.dir == sg.dir {
                    found = Some(ee);
                    break;
                }
            }
            match found {
                None => {
                    // af_axis_hints_new_edge: sorted insert (same position: minor direction first)
                    let mut at = edges.len();
                    while at > 0 {
                        let prev = edges[at - 1].fpos;
                        if if top_to_bottom { prev > sg.pos } else { prev < sg.pos } {
                            break;
                        }
                        if prev == sg.pos && sg.dir == major {
                            break;
                        }
                        at -= 1;
                    }
                    let opos = mul_fix(sg.pos, scale);
                    edges.insert(
                        at,
                        Edge {
                            fpos: sg.pos,
                            opos,
                            pos: opos,
                            flags: 0,
                            dir: sg.dir,
                            scale: 0,
                            blue: None,
                            link: None,
                            serif: None,
                            first: s,
                            last: s,
                        },
                    );
                    segs[s].edge_next = s;
                }
                Some(e) => {
                    segs[s].edge_next = edges[e].first;
                    let l = edges[e].last;
                    segs[l].edge_next = s;
                    edges[e].last = s;
                }
            }
        }
        for s in 0..segs.len() {
            if segs[s].dir != DIR_NONE {
                continue;
            }
            let mut found = None;
            for (ee, e) in edges.iter().enumerate() {
                if (segs[s].pos - e.fpos).abs() < edge_distance_threshold {
                    found = Some(ee);
                    break;
                }
            }
            if let Some(e) = found {
                segs[s].edge_next = edges[e].first;
                let l = edges[e].last;
                segs[l].edge_next = s;
                edges[e].last = s;
            }
        }
        for e in 0..edges.len() {
            let first = edges[e].first;
            let mut s = first;
            loop {
                segs[s].edge = Some(e);
                s = segs[s].edge_next;
                if s == first {
                    break;
                }
            }
        }
        for e in 0..edges.len() {
            let mut is_round = 0;
            let mut is_straight = 0;
            let first = edges[e].first;
            let mut s = first;
            loop {
                if segs[s].flags & EDGE_ROUND != 0 {
                    is_round += 1;
                } else {
                    is_straight += 1;
                }
                let is_serif = match segs[s].serif {
                    Some(sr) => matches!(segs[sr].edge, Some(se) if se != e),
                    None => false,
                };
                let link_has_edge = matches!(segs[s].link, Some(l) if segs[l].edge.is_some());
                if link_has_edge || is_serif {
                    let (mut edge2, seg2) = if is_serif { (edges[e].serif, segs[s].serif.unwrap()) } else { (edges[e].link, segs[s].link.unwrap()) };
                    match edge2 {
                        Some(e2) => {
                            let edge_delta = (edges[e].fpos - edges[e2].fpos).abs();
                            let seg_delta = (segs[s].pos - segs[seg2].pos).abs();
                            if seg_delta < edge_delta {
                                edge2 = segs[seg2].edge;
                            }
                        }
                        None => edge2 = segs[seg2].edge,
                    }
                    if is_serif {
                        edges[e].serif = edge2;
                        if let Some(e2) = edge2 {
                            edges[e2].flags |= EDGE_SERIF;
                        }
                    } else {
                        edges[e].link = edge2;
                    }
                }
                s = segs[s].edge_next;
                if s == first {
                    break;
                }
            }
            edges[e].flags = 0;
            if is_round > 0 && is_round >= is_straight {
                edges[e].flags |= EDGE_ROUND;
            }
            if edges[e].serif.is_some() && edges[e].link.is_some() {
                edges[e].serif = None;
            }
        }
    }

    /// `af_latin_hints_compute_blue_edges`.
    fn compute_blue_edges(&mut self, m: &LatinMetrics) {
        let latin = &m.axis[VERT];
        let scale = latin.scale;
        let ax = &mut self.axis[VERT];
        let major = ax.major_dir;
        for edge in ax.edges.iter_mut() {
            let mut best: Option<(usize, bool)> = None;
            let mut best_is_neutral = false;
            let mut best_dist = mul_fix(m.upem / 40, scale);
            if best_dist > 64 / 2 {
                best_dist = 64 / 2;
            }
            for (bb, blue) in latin.blues.iter().enumerate() {
                if blue.flags & BLUE_ACTIVE == 0 {
                    continue;
                }
                let is_top = blue.flags & (BLUE_TOP | BLUE_SUB_TOP) != 0;
                let is_neutral = blue.flags & BLUE_NEUTRAL != 0;
                let is_major = edge.dir == major;
                if (is_top ^ is_major) || is_neutral {
                    let mut dist = mul_fix((edge.fpos - blue.rf.org).abs(), scale);
                    if dist < best_dist {
                        best_dist = dist;
                        best = Some((bb, false));
                        best_is_neutral = is_neutral;
                    }
                    if edge.flags & EDGE_ROUND != 0 && dist != 0 && !is_neutral {
                        let is_under_ref = edge.fpos < blue.rf.org;
                        if is_top ^ is_under_ref {
                            dist = mul_fix((edge.fpos - blue.shoot.org).abs(), scale);
                            if dist < best_dist {
                                best_dist = dist;
                                best = Some((bb, true));
                                best_is_neutral = is_neutral;
                            }
                        }
                    }
                }
            }
            if let Some(b) = best {
                edge.blue = Some(b);
                if best_is_neutral {
                    edge.flags |= EDGE_NEUTRAL;
                }
            }
        }
    }

    /// `af_latin_hint_edges` in light mode (stem widths are never adjusted: `af_latin_compute_stem_width` returns
    /// the width unchanged without AF_LATIN_HINTS_STEM_ADJUST).
    fn hint_edges(&mut self, dim: usize, m: &LatinMetrics, top_to_bottom: bool) {
        let top_to_bottom = dim == VERT && top_to_bottom;
        let blues = &m.axis[VERT].blues;
        let fit = |b: (usize, bool)| if b.1 { blues[b.0].shoot.fit } else { blues[b.0].rf.fit };
        let edges = &mut self.axis[dim].edges;
        let n = edges.len();
        let mut anchor: Option<usize> = None;
        let mut has_serifs = 0;
        let align_linked = |edges: &mut Vec<Edge>, base: usize, stem: usize| {
            let dist = edges[stem].opos - edges[base].opos;
            edges[stem].pos = edges[base].pos + dist;
        };
        if dim == VERT {
            for e in 0..n {
                if edges[e].flags & EDGE_DONE != 0 {
                    continue;
                }
                let mut edge1 = None;
                let mut edge2 = edges[e].link;
                if let Some(e2) = edge2 {
                    if edges[e].blue.is_some() && edges[e2].blue.is_some() {
                        let neutral = edges[e].flags & EDGE_NEUTRAL != 0;
                        let neutral2 = edges[e2].flags & EDGE_NEUTRAL != 0;
                        if neutral2 {
                            edges[e2].blue = None;
                            edges[e2].flags &= !EDGE_NEUTRAL;
                        } else if neutral {
                            edges[e].blue = None;
                            edges[e].flags &= !EDGE_NEUTRAL;
                        }
                    }
                }
                let mut blue = edges[e].blue;
                if blue.is_some() {
                    edge1 = Some(e);
                } else if let Some(e2) = edge2 {
                    if edges[e2].blue.is_some() {
                        blue = edges[e2].blue;
                        edge1 = Some(e2);
                        edge2 = Some(e);
                    }
                }
                let Some(e1) = edge1 else { continue };
                edges[e1].pos = fit(blue.unwrap());
                edges[e1].flags |= EDGE_DONE;
                if let Some(e2) = edge2 {
                    if edges[e2].blue.is_none() {
                        align_linked(edges, e1, e2);
                        edges[e2].flags |= EDGE_DONE;
                    }
                }
                if anchor.is_none() {
                    anchor = Some(e);
                }
            }
        }
        for e in 0..n {
            if edges[e].flags & EDGE_DONE != 0 {
                continue;
            }
            let Some(e2) = edges[e].link else {
                has_serifs += 1;
                continue;
            };
            if edges[e2].blue.is_some() {
                align_linked(edges, e2, e);
                edges[e].flags |= EDGE_DONE;
                continue;
            }
            match anchor {
                None => {
                    let org_len = edges[e2].opos - edges[e].opos;
                    let cur_len = org_len;
                    let (u_off, d_off) = if cur_len <= 64 { (32, 32) } else { (38, 26) };
                    if cur_len < 96 {
                        let org_center = edges[e].opos + (org_len >> 1);
                        let mut cur_pos1 = pix_round(org_center);
                        let error1 = (org_center - (cur_pos1 - u_off)).abs();
                        let error2 = (org_center - (cur_pos1 + d_off)).abs();
                        if error1 < error2 {
                            cur_pos1 -= u_off;
                        } else {
                            cur_pos1 += d_off;
                        }
                        edges[e].pos = cur_pos1 - cur_len / 2;
                        edges[e2].pos = edges[e].pos + cur_len;
                    } else {
                        edges[e].pos = pix_round(edges[e].opos);
                    }
                    anchor = Some(e);
                    edges[e].flags |= EDGE_DONE;
                    align_linked(edges, e, e2);
                }
                Some(a) => {
                    let org_pos = edges[a].pos + (edges[e].opos - edges[a].opos);
                    let org_len = edges[e2].opos - edges[e].opos;
                    let org_center = org_pos + (org_len >> 1);
                    let cur_len = org_len;
                    if edges[e2].flags & EDGE_DONE != 0 {
                        edges[e].pos = edges[e2].pos - cur_len;
                    } else if cur_len < 96 {
                        let mut cur_pos1 = pix_round(org_center);
                        let (u_off, d_off) = if cur_len <= 64 { (32, 32) } else { (38, 26) };
                        let delta1 = (org_center - (cur_pos1 - u_off)).abs();
                        let delta2 = (org_center - (cur_pos1 + d_off)).abs();
                        if delta1 < delta2 {
                            cur_pos1 -= u_off;
                        } else {
                            cur_pos1 += d_off;
                        }
                        edges[e].pos = cur_pos1 - cur_len / 2;
                        edges[e2].pos = cur_pos1 + cur_len / 2;
                    } else {
                        let cur_pos1 = pix_round(org_pos);
                        let delta1 = (cur_pos1 + (cur_len >> 1) - org_center).abs();
                        let cur_pos2 = pix_round(org_pos + org_len) - cur_len;
                        let delta2 = (cur_pos2 + (cur_len >> 1) - org_center).abs();
                        edges[e].pos = if delta1 < delta2 { cur_pos1 } else { cur_pos2 };
                        edges[e2].pos = edges[e].pos + cur_len;
                    }
                    edges[e].flags |= EDGE_DONE;
                    edges[e2].flags |= EDGE_DONE;
                    if e > 0 && (if top_to_bottom { edges[e].pos > edges[e - 1].pos } else { edges[e].pos < edges[e - 1].pos }) {
                        if let Some(l) = edges[e].link {
                            if (edges[l].pos - edges[e - 1].pos).abs() > 16 {
                                edges[e].pos = edges[e - 1].pos;
                            }
                        }
                    }
                }
            }
        }
        // (the lowercase-m symmetry pass is horizontal only)
        if has_serifs > 0 || anchor.is_none() {
            for e in 0..n {
                if edges[e].flags & EDGE_DONE != 0 {
                    continue;
                }
                let mut delta = 1000i64;
                if let Some(s) = edges[e].serif {
                    delta = (edges[s].opos - edges[e].opos).abs();
                }
                if delta < 64 + 16 {
                    let s = edges[e].serif.unwrap();
                    edges[e].pos = edges[s].pos + (edges[e].opos - edges[s].opos);
                } else if anchor.is_none() {
                    edges[e].pos = pix_round(edges[e].opos);
                    anchor = Some(e);
                } else {
                    let a = anchor.unwrap();
                    let before = (0..e).rev().find(|&b| edges[b].flags & EDGE_DONE != 0);
                    let after = (e + 1..n).find(|&b| edges[b].flags & EDGE_DONE != 0);
                    match (before, after) {
                        (Some(b), Some(af)) => {
                            if edges[af].opos == edges[b].opos {
                                edges[e].pos = edges[b].pos;
                            } else {
                                edges[e].pos = edges[b].pos
                                    + mul_div(
                                        edges[e].opos - edges[b].opos,
                                        edges[af].pos - edges[b].pos,
                                        edges[af].opos - edges[b].opos,
                                    );
                            }
                        }
                        _ => {
                            edges[e].pos = edges[a].pos + ((edges[e].opos - edges[a].opos + 16) & !31);
                        }
                    }
                }
                edges[e].flags |= EDGE_DONE;
                // the two "don't move if the stem would disappear" checks need `edge->link`, which serif and
                // lone edges never have here
            }
        }
    }

    fn align_edge_points(&mut self, dim: usize) {
        let ax = &self.axis[dim];
        for sg in &ax.segments {
            let Some(e) = sg.edge else { continue };
            let pos = ax.edges[e].pos;
            let mut p = sg.first;
            loop {
                if dim == HORZ {
                    self.points[p].x = pos;
                    self.points[p].flags |= FLAG_TOUCH_X;
                } else {
                    self.points[p].y = pos;
                    self.points[p].flags |= FLAG_TOUCH_Y;
                }
                if p == sg.last {
                    break;
                }
                p = self.points[p].next;
            }
        }
    }

    fn align_strong_points(&mut self, dim: usize) {
        let touch = if dim == HORZ { FLAG_TOUCH_X } else { FLAG_TOUCH_Y };
        let edges = &mut self.axis[dim].edges;
        if edges.is_empty() {
            return;
        }
        let n = edges.len();
        for p in self.points.iter_mut() {
            if p.flags & touch != 0 || p.flags & FLAG_WEAK != 0 {
                continue;
            }
            let (mut u, ou) = if dim == VERT { (p.fy, p.oy) } else { (p.fx, p.ox) };
            let fu = u;
            'store: {
                let e0 = &edges[0];
                if e0.fpos - u >= 0 {
                    u = e0.pos - (e0.opos - ou);
                    break 'store;
                }
                let el = &edges[n - 1];
                if u - el.fpos >= 0 {
                    u = el.pos + (ou - el.opos);
                    break 'store;
                }
                let mut min = 0usize;
                let mut max = n;
                if max <= 8 {
                    let mut nn = 0;
                    while nn < max {
                        if edges[nn].fpos >= u {
                            break;
                        }
                        nn += 1;
                    }
                    if edges[nn].fpos == u {
                        u = edges[nn].pos;
                        break 'store;
                    }
                    min = nn;
                } else {
                    while min < max {
                        let mid = (max + min) >> 1;
                        let fpos = edges[mid].fpos;
                        if u < fpos {
                            max = mid;
                        } else if u > fpos {
                            min = mid + 1;
                        } else {
                            u = edges[mid].pos;
                            break 'store;
                        }
                    }
                }
                let (b, a) = (min - 1, min);
                if edges[b].scale == 0 {
                    edges[b].scale = div_fix(edges[a].pos - edges[b].pos, edges[a].fpos - edges[b].fpos);
                }
                u = edges[b].pos + mul_fix(fu - edges[b].fpos, edges[b].scale);
            }
            if dim == HORZ {
                p.x = u;
            } else {
                p.y = u;
            }
            p.flags |= touch;
        }
    }

    fn align_weak_points(&mut self, dim: usize) {
        let touch = if dim == HORZ { FLAG_TOUCH_X } else { FLAG_TOUCH_Y };
        let pts = &mut self.points;
        for p in pts.iter_mut() {
            if dim == HORZ {
                p.u = p.x;
                p.v = p.ox;
            } else {
                p.u = p.y;
                p.v = p.oy;
            }
        }
        for &c0 in &self.contours {
            let mut point = c0;
            let end_point = pts[point].prev;
            let first_point = point;
            loop {
                if point > end_point {
                    break;
                }
                if pts[point].flags & touch != 0 {
                    break;
                }
                point += 1;
            }
            if point > end_point {
                continue;
            }
            let first_touched = point;
            let last_touched;
            loop {
                while point < end_point && pts[point + 1].flags & touch != 0 {
                    point += 1;
                }
                let lt = point;
                point += 1;
                let mut end = false;
                loop {
                    if point > end_point {
                        end = true;
                        break;
                    }
                    if pts[point].flags & touch != 0 {
                        break;
                    }
                    point += 1;
                }
                if end {
                    last_touched = lt;
                    break;
                }
                iup_interp(pts, lt + 1, point - 1, lt, point);
            }
            if last_touched == first_touched {
                iup_shift(pts, first_point, end_point, first_touched);
            } else {
                if last_touched < end_point {
                    iup_interp(pts, last_touched + 1, end_point, last_touched, first_touched);
                }
                // FreeType compares against the glyph's first point here, not the contour's
                if first_touched > 0 {
                    iup_interp(pts, first_point, first_touched.wrapping_sub(1), last_touched, first_touched);
                }
            }
        }
        for p in pts.iter_mut() {
            if dim == HORZ {
                p.x = p.u;
            } else {
                p.y = p.u;
            }
        }
    }
}

fn iup_shift(pts: &mut [Point], p1: usize, p2: usize, r: usize) {
    let delta = pts[r].u - pts[r].v;
    if delta == 0 {
        return;
    }
    for p in p1..r {
        pts[p].u = pts[p].v + delta;
    }
    for p in r + 1..=p2 {
        pts[p].u = pts[p].v + delta;
    }
}

fn iup_interp(pts: &mut [Point], p1: usize, p2: usize, ref1: usize, ref2: usize) {
    if p1 > p2 || p2 == usize::MAX {
        return;
    }
    let (r1, r2) = if pts[ref1].v > pts[ref2].v { (ref2, ref1) } else { (ref1, ref2) };
    let (v1, v2, u1, u2) = (pts[r1].v, pts[r2].v, pts[r1].u, pts[r2].u);
    let (d1, d2) = (u1 - v1, u2 - v2);
    if u1 == u2 || v1 == v2 {
        for p in p1..=p2 {
            let mut u = pts[p].v;
            if u <= v1 {
                u += d1;
            } else if u >= v2 {
                u += d2;
            } else {
                u = u1;
            }
            pts[p].u = u;
        }
    } else {
        let scale = div_fix(u2 - u1, v2 - v1);
        for p in p1..=p2 {
            let mut u = pts[p].v;
            if u <= v1 {
                u += d1;
            } else if u >= v2 {
                u += d2;
            } else {
                u = u1 + mul_fix(u - v1, scale);
            }
            pts[p].u = u;
        }
    }
}
