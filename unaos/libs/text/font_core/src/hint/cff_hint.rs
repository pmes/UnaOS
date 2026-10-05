//! CFF hinting the way FreeType's Adobe engine applies it (`src/psaux`: `psintrp.c`, `pshints.c`, `psblues.c` —
//! the "cf2" engine Adobe contributed, FreeType's default for CFF since 2.5): what Chromium draws for an installed
//! OpenType/CFF face under fontconfig `hintslight` (`FT_LOAD_TARGET_LIGHT`; the Adobe engine hints lightly by
//! design: vertical only, stem darkening off by default).
//!
//! Model, all in 16.16 fixed point as the engine computes it:
//!
//! - **Blue zones** (`cf2_blues_init`): BlueValues (first pair a bottom zone, the rest top zones) and OtherBlues
//!   (bottom zones), the flat edge snapped to a FamilyBlues edge within one pixel; overshoot suppressed below the
//!   BlueScale size with a boost of up to 0.6 px; each zone's device-space flat edge rounded to the pixel grid.
//! - **Stem hints** (`hstem`/`hstemhm`, `hintmask`, `cntrmask`, ghost hints −20/−21): a hint is captured by a zone
//!   (`cf2_blues_capture`: at the flat edge when suppressing overshoot, a pixel past it when BlueShift requires
//!   overshoot, else rounded) and locked; an initial hint map holds the captured hints (plus a synthetic hint at 0),
//!   and each hint mask builds a map from it (`cf2_hintmap_build`): pairs are centred on their initial-map position,
//!   then `cf2_hintmap_adjustHints` moves each unlocked pair so one edge lands on a pixel boundary, keeping half a
//!   pixel between neighbours (a second, top-down pass retries blocked moves).
//! - **Points** (`cf2_glyphpath_*`): x is scaled; y goes through the hint map piecewise-linearly; a new hint mask
//!   takes effect one path element late, a closing line uses the map the contour started with; output is 16.16
//!   shifted to 26.6 (`>> 10`) through the PostScript builder (a closing point equal to the start is dropped).
//!
//! Not implemented: `seac` accented glyphs (returned as `None`, the caller draws them unhinted), CFF2 blending,
//! the em-box ghost hints of ideographic (LanguageGroup 1) fonts' synthetic heuristic is implemented but untested,
//! and stem darkening (off in FreeType's default configuration).

use super::fixed::{div_fix, mul_div, mul_fix};
use super::{Outline, TAG_CUBIC, TAG_ON};
use crate::cff::{HintPrivate, Index};
use crate::font::Outlines;
use crate::Font;
use alloc::vec::Vec;

type Fixed = i32;

const MAX_HINTS: usize = 96;
const MAX_HINT_EDGES: usize = MAX_HINTS * 2;
const MIN_COUNTER: Fixed = 0x8000; // cf2_doubleToFixed(0.5)
const ICF_TOP: Fixed = 880 << 16;
const ICF_BOTTOM: Fixed = -120 << 16;

const GHOST_BOTTOM: u32 = 0x1;
const GHOST_TOP: u32 = 0x2;
const PAIR_BOTTOM: u32 = 0x4;
const PAIR_TOP: u32 = 0x8;
const LOCKED: u32 = 0x10;
const SYNTHETIC: u32 = 0x20;

#[inline]
fn add(a: Fixed, b: Fixed) -> Fixed {
    a.wrapping_add(b)
}
#[inline]
fn sub(a: Fixed, b: Fixed) -> Fixed {
    a.wrapping_sub(b)
}
#[inline]
fn mulf(a: Fixed, b: Fixed) -> Fixed {
    mul_fix(a as i64, b as i64) as Fixed
}
#[inline]
fn divf(a: Fixed, b: Fixed) -> Fixed {
    div_fix(a as i64, b as i64) as Fixed
}
#[inline]
fn round(x: Fixed) -> Fixed {
    ((x as u32).wrapping_add(0x8000) & 0xFFFF_0000) as Fixed
}
#[inline]
fn floor(x: Fixed) -> Fixed {
    ((x as u32) & 0xFFFF_0000) as Fixed
}
#[inline]
fn int_to_fixed(i: i32) -> Fixed {
    ((i as u32) << 16) as Fixed
}

#[derive(Clone, Copy, Debug, Default)]
struct StemHint {
    min: Fixed,
    max: Fixed,
    used: bool,
    min_ds: Fixed,
    max_ds: Fixed,
}

#[derive(Clone, Copy, Debug, Default)]
struct Hint {
    flags: u32,
    index: usize,
    cs: Fixed,
    ds: Fixed,
    scale: Fixed,
}

impl Hint {
    fn valid(&self) -> bool {
        self.flags != 0
    }
    fn is_pair(&self) -> bool {
        self.flags & (PAIR_BOTTOM | PAIR_TOP) != 0
    }
    fn is_pair_top(&self) -> bool {
        self.flags & PAIR_TOP != 0
    }
    fn is_top(&self) -> bool {
        self.flags & (PAIR_TOP | GHOST_TOP) != 0
    }
    fn is_bottom(&self) -> bool {
        self.flags & (PAIR_BOTTOM | GHOST_BOTTOM) != 0
    }
    fn locked(&self) -> bool {
        self.flags & LOCKED != 0
    }
    fn synthetic(&self) -> bool {
        self.flags & SYNTHETIC != 0
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Zone {
    cs_bottom: Fixed,
    cs_top: Fixed,
    cs_flat: Fixed,
    ds_flat: Fixed,
    bottom: bool,
}

#[derive(Clone, Debug, Default)]
struct Blues {
    blue_scale: Fixed,
    blue_shift: Fixed,
    blue_fuzz: Fixed,
    zones: Vec<Zone>,
    suppress_overshoot: bool,
    boost: Fixed,
    do_em_box: bool,
    em_bottom: Hint,
    em_top: Hint,
}

/// `cf2_blues_init` (no darkening).
fn blues_init(p: &HintPrivate, scale: Fixed) -> Blues {
    let mut b = Blues::default();
    b.blue_scale = divf(p.blue_scale_1000, int_to_fixed(1000));
    b.blue_shift = int_to_fixed(p.blue_shift);
    b.blue_fuzz = int_to_fixed(p.blue_fuzz);
    let bv = &p.blue_values[..p.num_blue_values as usize];
    let ob = &p.other_blues[..p.num_other_blues as usize];
    let fb = &p.family_blues[..p.num_family_blues as usize];
    let fob = &p.family_other_blues[..p.num_family_other_blues as usize];
    let to_f = |v: i32| int_to_fixed(v);
    if p.language_group == 1
        && (bv.is_empty()
            || (bv.len() == 4 && to_f(bv[0]) < ICF_BOTTOM && to_f(bv[1]) < ICF_BOTTOM && to_f(bv[2]) > ICF_TOP && to_f(bv[3]) > ICF_TOP))
    {
        b.em_bottom.cs = ICF_BOTTOM - 1;
        b.em_bottom.ds = sub(round(mulf(b.em_bottom.cs, scale)), MIN_COUNTER);
        b.em_bottom.scale = scale;
        b.em_bottom.flags = GHOST_BOTTOM | LOCKED | SYNTHETIC;
        b.em_top.cs = ICF_TOP + 1;
        b.em_top.ds = add(round(mulf(b.em_top.cs, scale)), MIN_COUNTER);
        b.em_top.scale = scale;
        b.em_top.flags = GHOST_TOP | LOCKED | SYNTHETIC;
        b.do_em_box = true;
        return b;
    }
    let mut max_zone_height = 0;
    let mut i = 0;
    while i + 1 < bv.len() {
        let (bot, top) = (to_f(bv[i]), to_f(bv[i + 1]));
        let h = sub(top, bot);
        if h >= 0 {
            max_zone_height = max_zone_height.max(h);
            let bottom = i == 0;
            b.zones.push(Zone { cs_bottom: bot, cs_top: top, cs_flat: if bottom { top } else { bot }, ds_flat: 0, bottom });
        }
        i += 2;
    }
    let mut i = 0;
    while i + 1 < ob.len() {
        let (bot, top) = (to_f(ob[i]), to_f(ob[i + 1]));
        let h = sub(top, bot);
        if h >= 0 {
            max_zone_height = max_zone_height.max(h);
            b.zones.push(Zone { cs_bottom: bot, cs_top: top, cs_flat: top, ds_flat: 0, bottom: true });
        }
        i += 2;
    }
    let cs_units_per_pixel = divf(int_to_fixed(1), scale);
    for z in b.zones.iter_mut() {
        let flat = z.cs_flat;
        if z.bottom {
            let mut min_diff = Fixed::MAX;
            let mut j = 0;
            while j + 1 < fob.len() {
                let ffe = to_f(fob[j + 1]);
                let diff = sub(flat, ffe).abs();
                if diff < min_diff && diff < cs_units_per_pixel {
                    z.cs_flat = ffe;
                    min_diff = diff;
                    if diff == 0 {
                        break;
                    }
                }
                j += 2;
            }
            if fb.len() >= 2 {
                let ffe = to_f(fb[1]);
                let diff = sub(flat, ffe).abs();
                if diff < min_diff && diff < cs_units_per_pixel {
                    z.cs_flat = ffe;
                }
            }
        } else {
            let mut min_diff = Fixed::MAX;
            let mut j = 2;
            while j < fb.len() {
                let ffe = to_f(fb[j]);
                let diff = sub(flat, ffe).abs();
                if diff < min_diff && diff < cs_units_per_pixel {
                    z.cs_flat = ffe;
                    min_diff = diff;
                    if diff == 0 {
                        break;
                    }
                }
                j += 2;
            }
        }
    }
    if max_zone_height > 0 && b.blue_scale > divf(int_to_fixed(1), max_zone_height) {
        b.blue_scale = divf(int_to_fixed(1), max_zone_height);
    }
    if scale < b.blue_scale {
        b.suppress_overshoot = true;
        let six = (0.6f64 * 65536.0 + 0.5) as i64;
        b.boost = (six - mul_div(six, scale as i64, b.blue_scale as i64)) as Fixed;
        if b.boost > 0x7FFF {
            b.boost = 0x7FFF;
        }
    }
    for z in b.zones.iter_mut() {
        z.ds_flat = if z.bottom { round(mulf(z.cs_flat, scale) - b.boost) } else { round(mulf(z.cs_flat, scale) + b.boost) };
    }
    b
}

/// `cf2_blues_capture`.
fn blues_capture(b: &Blues, bottom: &mut Hint, top: &mut Hint) -> bool {
    let fuzz = b.blue_fuzz;
    let mut ds_move = 0;
    let mut captured = false;
    for z in &b.zones {
        if z.bottom && bottom.is_bottom() {
            if sub(z.cs_bottom, fuzz) <= bottom.cs && bottom.cs <= add(z.cs_top, fuzz) {
                let ds_new = if b.suppress_overshoot {
                    z.ds_flat
                } else if sub(z.cs_top, bottom.cs) >= b.blue_shift {
                    round(bottom.ds).min(sub(z.ds_flat, int_to_fixed(1)))
                } else {
                    round(bottom.ds)
                };
                ds_move = sub(ds_new, bottom.ds);
                captured = true;
                break;
            }
        }
        if !z.bottom && top.is_top() {
            if sub(z.cs_bottom, fuzz) <= top.cs && top.cs <= add(z.cs_top, fuzz) {
                let ds_new = if b.suppress_overshoot {
                    z.ds_flat
                } else if sub(top.cs, z.cs_bottom) >= b.blue_shift {
                    round(top.ds).max(z.ds_flat + int_to_fixed(1))
                } else {
                    round(top.ds)
                };
                ds_move = sub(ds_new, top.ds);
                captured = true;
                break;
            }
        }
    }
    if captured {
        if bottom.valid() {
            bottom.ds = add(bottom.ds, ds_move);
            bottom.flags |= LOCKED;
        }
        if top.valid() {
            top.ds = add(top.ds, ds_move);
            top.flags |= LOCKED;
        }
    }
    captured
}

/// `cf2_hint_init`.
fn hint_init(stems: &[StemHint], i: usize, scale: Fixed, bottom: bool) -> Hint {
    let mut h = Hint::default();
    let s = stems[i];
    let width = sub(s.max, s.min);
    if width == int_to_fixed(-21) {
        if bottom {
            h.cs = s.max;
            h.flags = GHOST_BOTTOM;
        }
    } else if width == int_to_fixed(-20) {
        if !bottom {
            h.cs = s.min;
            h.flags = GHOST_TOP;
        }
    } else if width < 0 {
        if bottom {
            h.cs = s.max;
            h.flags = PAIR_BOTTOM;
        } else {
            h.cs = s.min;
            h.flags = PAIR_TOP;
        }
    } else if bottom {
        h.cs = s.min;
        h.flags = PAIR_BOTTOM;
    } else {
        h.cs = s.max;
        h.flags = PAIR_TOP;
    }
    h.scale = scale;
    h.index = i;
    if h.flags != 0 && s.used {
        h.ds = if h.is_top() { s.max_ds } else { s.min_ds };
        h.flags |= LOCKED;
    } else {
        h.ds = mulf(h.cs, scale);
    }
    h
}

#[derive(Clone, Debug)]
struct HintMap {
    hinted: bool,
    scale: Fixed,
    valid: bool,
    edge: Vec<Hint>,
}

impl HintMap {
    fn new(scale: Fixed) -> Self {
        HintMap { hinted: true, scale, valid: false, edge: Vec::new() }
    }

    /// `cf2_hintmap_map` (the `lastIndex` cache only speeds the search; the result does not depend on it).
    fn map(&self, cs: Fixed) -> Fixed {
        if self.edge.is_empty() || !self.hinted {
            return mulf(cs, self.scale);
        }
        let n = self.edge.len();
        let mut i = 0;
        while i < n - 1 && cs >= self.edge[i + 1].cs {
            i += 1;
        }
        while i > 0 && cs < self.edge[i].cs {
            i -= 1;
        }
        if i == 0 && cs < self.edge[0].cs {
            add(mulf(sub(cs, self.edge[0].cs), self.scale), self.edge[0].ds)
        } else {
            add(mulf(sub(cs, self.edge[i].cs), self.edge[i].scale), self.edge[i].ds)
        }
    }

    /// `cf2_hintmap_adjustHints`.
    fn adjust(&mut self) {
        let mut moves: Vec<(usize, Fixed)> = Vec::new();
        let n = self.edge.len();
        let mut i = 0;
        while i < n {
            let is_pair = self.edge[i].is_pair();
            let j = if is_pair { i + 1 } else { i };
            if j >= n {
                break;
            }
            let ds_i = self.edge[i].ds;
            let ds_j = self.edge[j].ds;
            if !self.edge[i].locked() {
                let frac_down = ds_i - floor(ds_i);
                let frac_up = ds_j - floor(ds_j);
                let down_move_down = -frac_down;
                let up_move_down = -frac_up;
                let down_move_up = if frac_down == 0 { 0 } else { int_to_fixed(1) - frac_down };
                let up_move_up = if frac_up == 0 { 0 } else { int_to_fixed(1) - frac_up };
                let move_up = down_move_up.min(up_move_up);
                let move_down = down_move_down.max(up_move_down);
                let mut save_edge = false;
                let mv;
                if j >= n - 1 || self.edge[j + 1].ds >= add(ds_j, move_up + MIN_COUNTER) {
                    if i == 0 || self.edge[i - 1].ds <= add(ds_i, move_down - MIN_COUNTER) {
                        mv = if -move_down < move_up { move_down } else { move_up };
                    } else {
                        mv = move_up;
                    }
                } else if i == 0 || self.edge[i - 1].ds <= add(ds_i, move_down - MIN_COUNTER) {
                    mv = move_down;
                    save_edge = move_up < -move_down;
                } else {
                    mv = 0;
                    save_edge = true;
                }
                if save_edge && j < n - 1 && !self.edge[j + 1].locked() {
                    moves.push((j, move_up - mv));
                }
                self.edge[i].ds = add(ds_i, mv);
                if is_pair {
                    self.edge[j].ds = add(ds_j, mv);
                }
            }
            if i > 0 && self.edge[i].cs != self.edge[i - 1].cs {
                self.edge[i - 1].scale = divf(sub(self.edge[i].ds, self.edge[i - 1].ds), sub(self.edge[i].cs, self.edge[i - 1].cs));
            }
            if is_pair {
                if self.edge[j].cs != self.edge[j - 1].cs {
                    self.edge[j - 1].scale = divf(sub(self.edge[j].ds, self.edge[j - 1].ds), sub(self.edge[j].cs, self.edge[j - 1].cs));
                }
                i += 1;
            }
            i += 1;
        }
        for &(j, move_up) in moves.iter().rev() {
            if self.edge[j + 1].ds >= add(self.edge[j].ds, move_up + MIN_COUNTER) {
                self.edge[j].ds = add(self.edge[j].ds, move_up);
                if self.edge[j].is_pair() && j > 0 {
                    self.edge[j - 1].ds = add(self.edge[j - 1].ds, move_up);
                }
            }
        }
    }

    /// `cf2_hintmap_insertHint`.
    fn insert(&mut self, initial: Option<&HintMap>, bottom: &Hint, top: &Hint) {
        let mut is_pair = true;
        let (mut first, mut second) = (*bottom, *top);
        if !bottom.valid() {
            first = *top;
            is_pair = false;
        } else if !top.valid() {
            is_pair = false;
        }
        if is_pair && top.cs < bottom.cs {
            return;
        }
        let n = self.edge.len();
        let mut at = 0;
        while at < n && self.edge[at].cs < first.cs {
            at += 1;
        }
        if at < n {
            if self.edge[at].cs == first.cs {
                return;
            }
            if is_pair && self.edge[at].cs <= second.cs {
                return;
            }
            if self.edge[at].is_pair_top() {
                return;
            }
        }
        if let Some(init) = initial {
            if init.valid && !first.locked() {
                if is_pair {
                    let mid = init.map(add(first.cs, sub(second.cs, first.cs) / 2));
                    let half = mulf(sub(second.cs, first.cs) / 2, self.scale);
                    first.ds = sub(mid, half);
                    second.ds = add(mid, half);
                } else {
                    first.ds = init.map(first.cs);
                }
            }
        }
        if at > 0 && first.ds < self.edge[at - 1].ds {
            return;
        }
        if at < n {
            if is_pair {
                if second.ds > self.edge[at].ds {
                    return;
                }
            } else if first.ds > self.edge[at].ds {
                return;
            }
        }
        if n + if is_pair { 2 } else { 1 } > MAX_HINT_EDGES {
            return;
        }
        self.edge.insert(at, first);
        if is_pair {
            self.edge.insert(at + 1, second);
        }
    }

    /// `cf2_hintmap_build` for a non-initial map (`initial` must be valid) or, with `initial == None`, the initial
    /// map itself (captured hints only, plus a synthetic edge at 0).
    fn build(&mut self, initial: Option<&HintMap>, stems: &mut [StemHint], nv: usize, mask: &mut HintMask, blues: &Blues) {
        if !mask.valid {
            // cf2_hintmask_setAll on the caller's mask: once drawing starts without a hintmask, all hints apply
            // and later stem operators are ignored
            mask.set_all(stems.len() + nv);
            if !mask.valid {
                return;
            }
        }
        self.edge.clear();
        let bit_count = stems.len();
        if bit_count > mask.bit_count {
            return;
        }
        if blues.do_em_box {
            let dummy = Hint::default();
            self.insert(initial, &blues.em_bottom, &dummy);
            self.insert(initial, &dummy, &blues.em_top);
        }
        let initial_map = initial.is_none();
        // pass 1 (every map): hints already locked or captured by a blue zone go in first, and leave the mask
        let mut rest = mask.clone();
        for i in 0..bit_count {
            if !mask.bit(i) {
                continue;
            }
            let mut b = hint_init(stems, i, self.scale, true);
            let mut t = hint_init(stems, i, self.scale, false);
            if b.locked() || t.locked() || blues_capture(blues, &mut b, &mut t) {
                self.insert(initial, &b, &t);
                rest.mask[i / 8] &= !(0x80u8 >> (i % 8));
            }
        }
        if initial_map {
            // a synthetic edge at 0 keeps translations from propagating across the origin
            if self.edge.is_empty() || self.edge[0].cs > 0 || self.edge[self.edge.len() - 1].cs < 0 {
                let edge = Hint { flags: GHOST_BOTTOM | LOCKED | SYNTHETIC, scale: self.scale, ..Default::default() };
                self.insert(None, &edge, &Hint::default());
            }
        } else {
            // pass 2: the remaining hints of the mask, centred through the initial map
            for i in 0..bit_count {
                if !rest.bit(i) {
                    continue;
                }
                let b = hint_init(stems, i, self.scale, true);
                let t = hint_init(stems, i, self.scale, false);
                self.insert(initial, &b, &t);
            }
        }
        self.adjust();
        if !initial_map {
            for e in &self.edge {
                if !e.synthetic() {
                    let s = &mut stems[e.index];
                    if e.is_top() {
                        s.max_ds = e.ds;
                    } else {
                        s.min_ds = e.ds;
                    }
                    s.used = true;
                }
            }
        }
        self.valid = true;
        mask.new = false;
    }
}

#[derive(Clone, Debug, Default)]
struct HintMask {
    valid: bool,
    new: bool,
    bit_count: usize,
    mask: [u8; 12],
}

impl HintMask {
    fn set_counts(&mut self, bits: usize) -> bool {
        if bits > MAX_HINTS {
            return false;
        }
        self.bit_count = bits;
        self.valid = true;
        self.new = true;
        bits > 0
    }
    fn set_all(&mut self, bits: usize) {
        if !self.set_counts(bits) {
            return;
        }
        let bytes = bits.div_ceil(8);
        for b in self.mask.iter_mut().take(bytes) {
            *b = 0xFF;
        }
        let m = (1u32 << (bits.wrapping_neg() & 7)) - 1;
        self.mask[bytes - 1] &= !(m as u8);
    }
    fn bit(&self, i: usize) -> bool {
        self.mask[i / 8] & (0x80 >> (i % 8)) != 0
    }
}

/// The PostScript outline builder FreeType's CFF loader feeds (`ps_builder_*`), in 26.6.
#[derive(Default)]
struct Builder {
    out: Outline,
    path_begun: bool,
}

impl Builder {
    fn add_point(&mut self, x: Fixed, y: Fixed, on: bool) {
        self.out.points.push(((x as i64) >> 10, (y as i64) >> 10));
        self.out.tags.push(if on { TAG_ON } else { TAG_CUBIC });
    }
    fn add_contour(&mut self) {
        if !self.out.ends.is_empty() {
            let last = self.out.ends.len() - 1;
            self.out.ends[last] = self.out.points.len().wrapping_sub(1);
        }
        self.out.ends.push(usize::MAX);
    }
    fn start_point(&mut self, p: (Fixed, Fixed)) {
        if !self.path_begun {
            self.path_begun = true;
            self.add_contour();
            self.add_point(p.0, p.1, true);
        }
    }
    fn close_contour(&mut self) {
        let nc = self.out.ends.len();
        let first = if nc <= 1 { 0 } else { self.out.ends[nc - 2].wrapping_add(1) };
        if nc > 0 && first == self.out.points.len() {
            self.out.ends.pop();
            return;
        }
        let np = self.out.points.len();
        if np > 1 && self.out.points[first] == self.out.points[np - 1] && self.out.tags[np - 1] == TAG_ON {
            self.out.points.pop();
            self.out.tags.pop();
        }
        let np = self.out.points.len();
        if nc > 0 {
            if first == np - 1 {
                self.out.ends.pop();
                self.out.points.pop();
                self.out.tags.pop();
            } else {
                self.out.ends[nc - 1] = np - 1;
            }
        }
    }
    fn move_to(&mut self) {
        self.close_contour();
        self.path_begun = false;
    }
    fn line_to(&mut self, p0: (Fixed, Fixed), p1: (Fixed, Fixed)) {
        self.start_point(p0);
        self.add_point(p1.0, p1.1, true);
    }
    fn cube_to(&mut self, p0: (Fixed, Fixed), p1: (Fixed, Fixed), p2: (Fixed, Fixed), p3: (Fixed, Fixed)) {
        self.start_point(p0);
        self.add_point(p1.0, p1.1, false);
        self.add_point(p2.0, p2.1, false);
        self.add_point(p3.0, p3.1, true);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ElemOp {
    Line,
    Cube,
}

/// `CF2_GlyphPath` without darkening (all offsets zero, so no intersections are ever computed).
struct GlyphPath {
    scale_x: Fixed,
    initial: HintMap,
    first: HintMap,
    map: HintMap,
    move_pending: bool,
    path_open: bool,
    path_closing: bool,
    elem_queued: bool,
    prev_op: ElemOp,
    prev: [(Fixed, Fixed); 4],
    current_cs: (Fixed, Fixed),
    current_ds: (Fixed, Fixed),
    start: (Fixed, Fixed),
    offset_start0: (Fixed, Fixed),
    offset_start1: (Fixed, Fixed),
    b: Builder,
    /// Diagnostics: every hint map built, in FreeType's `cf2_hintmap_dump` units.
    trace: Option<Vec<alloc::string::String>>,
}

struct Ctx<'c> {
    /// Horizontal stem hints (the only ones the engine maps; vertical stems only count toward mask bits).
    stems: &'c mut Vec<StemHint>,
    vstems: usize,
    mask: &'c mut HintMask,
    blues: &'c Blues,
}

impl GlyphPath {
    fn hint_point(&self, map: &HintMap, x: Fixed, y: Fixed) -> (Fixed, Fixed) {
        (mulf(self.scale_x, x), map.map(y))
    }

    fn ensure_initial(&mut self, c: &mut Ctx) {
        if !self.initial.valid {
            let mut init = HintMap::new(self.initial.scale);
            init.build(None, c.stems, c.vstems, &mut HintMask::default(), c.blues);
            self.initial = init;
        }
    }

    fn build_map(&mut self, c: &mut Ctx) {
        self.ensure_initial(c);
        let mut m = HintMap::new(self.map.scale);
        m.build(Some(&self.initial), c.stems, c.vstems, c.mask, c.blues);
        if m.valid {
            self.map = m;
        }
        if let Some(t) = self.trace.as_mut() {
            for e in &self.map.edge {
                t.push(alloc::format!(
                    "{:3} {:9.2} {:9.2} {:5} {}{}{}",
                    e.index,
                    e.cs as f64 / 65536.0,
                    e.ds as f64 / e.scale as f64,
                    e.scale,
                    if e.is_pair() { "p" } else { "g" },
                    if e.is_top() { "t" } else { "b" },
                    if e.locked() { "L" } else { "" }
                ));
            }
            t.push(alloc::string::String::from("--"));
        }
    }

    fn push_prev_elem(&mut self, use_map_first: bool, next_p0: (Fixed, Fixed)) {
        let close = use_map_first;
        match self.prev_op {
            ElemOp::Line => {
                let map = if close { &self.first } else { &self.map };
                let p1 = self.hint_point(map, self.prev[1].0, self.prev[1].1);
                if self.current_ds != p1 {
                    self.b.line_to(self.current_ds, p1);
                    self.current_ds = p1;
                }
            }
            ElemOp::Cube => {
                let p1 = self.hint_point(&self.map, self.prev[1].0, self.prev[1].1);
                let p2 = self.hint_point(&self.map, self.prev[2].0, self.prev[2].1);
                let p3 = self.hint_point(&self.map, self.prev[3].0, self.prev[3].1);
                self.b.cube_to(self.current_ds, p1, p2, p3);
                self.current_ds = p3;
            }
        }
        // !useIntersection (always, without darkening)
        let map = if close { &self.first } else { &self.map };
        let p1 = self.hint_point(map, next_p0.0, next_p0.1);
        if p1 != self.current_ds {
            self.b.line_to(self.current_ds, p1);
            self.current_ds = p1;
        }
    }

    fn push_move(&mut self, c: &mut Ctx, start: (Fixed, Fixed)) {
        if !self.map.valid {
            let s = self.start;
            self.move_to(c, s.0, s.1);
        }
        let p1 = self.hint_point(&self.map, start.0, start.1);
        self.b.move_to();
        self.current_ds = p1;
        self.offset_start0 = start;
    }

    fn move_to(&mut self, c: &mut Ctx, x: Fixed, y: Fixed) {
        self.close_open_path(c);
        self.current_cs = (x, y);
        self.start = (x, y);
        self.move_pending = true;
        if !self.map.valid || c.mask.new {
            self.build_map(c);
        }
        self.first = self.map.clone();
    }

    fn line_to(&mut self, c: &mut Ctx, x: Fixed, y: Fixed) {
        let new_map = c.mask.new && !self.path_closing;
        if self.current_cs == (x, y) && !new_map {
            return;
        }
        let p0 = self.current_cs;
        let p1 = (x, y);
        if self.move_pending {
            self.push_move(c, p0);
            self.move_pending = false;
            self.path_open = true;
            self.offset_start1 = p1;
        }
        if self.elem_queued {
            self.push_prev_elem(false, p0);
        }
        self.elem_queued = true;
        self.prev_op = ElemOp::Line;
        self.prev[0] = p0;
        self.prev[1] = p1;
        if new_map {
            self.build_map(c);
        }
        self.current_cs = (x, y);
    }

    #[allow(clippy::too_many_arguments)]
    fn curve_to(&mut self, c: &mut Ctx, x1: Fixed, y1: Fixed, x2: Fixed, y2: Fixed, x3: Fixed, y3: Fixed) {
        let p0 = self.current_cs;
        if self.move_pending {
            self.push_move(c, p0);
            self.move_pending = false;
            self.path_open = true;
            self.offset_start1 = (x1, y1);
        }
        if self.elem_queued {
            self.push_prev_elem(false, p0);
        }
        self.elem_queued = true;
        self.prev_op = ElemOp::Cube;
        self.prev = [p0, (x1, y1), (x2, y2), (x3, y3)];
        if c.mask.new {
            self.build_map(c);
        }
        self.current_cs = (x3, y3);
    }

    fn close_open_path(&mut self, c: &mut Ctx) {
        if self.path_open {
            self.path_closing = true;
            let s = self.start;
            self.line_to(c, s.0, s.1);
            if self.elem_queued {
                let s0 = self.offset_start0;
                self.push_prev_elem(true, s0);
            }
            self.move_pending = true;
            self.path_open = false;
            self.path_closing = false;
            self.elem_queued = false;
        }
    }
}

/// The light-hinted outline of CFF glyph `gid` at `size` px (26.6, y up) as FreeType's Adobe engine produces it.
/// `None` for a missing glyph, a non-CFF face, or a construct not implemented (seac).
pub fn hint_cff(font: &Font, gid: u16, size: f32) -> Option<Outline> {
    hint_cff_traced(font, gid, size, false).0
}

/// [`hint_cff`] plus, with `trace`, every hint map it built (index, csCoord, dsCoord/scale, scale, flags — the
/// columns of FreeType's `cf2_hintmap_dump`, for comparing against a tracing FreeType build).
pub fn hint_cff_traced(font: &Font, gid: u16, size: f32, trace: bool) -> (Option<Outline>, Vec<alloc::string::String>) {
    let mut log = Vec::new();
    let o = hint_cff_inner(font, gid, size, trace.then_some(&mut log));
    (o, log)
}

fn hint_cff_inner(font: &Font, gid: u16, size: f32, log: Option<&mut Vec<alloc::string::String>>) -> Option<Outline> {
    let Outlines::Cff(cff) = &font.outlines else { return None };
    if (cff.font_matrix_scale * font.units_per_em as f32 - 1.0).abs() > 1e-4 {
        return None; // a non-default FontMatrix is applied after the engine; not modelled
    }
    let (cs, subrs, gsubrs, private) = cff.hint_source(gid)?;
    let upem = font.units_per_em.max(1) as i64;
    let size_26_6 = (size * 64.0 + 0.5) as i64;
    let size_scale = div_fix(size_26_6, upem); // FT_Size x_scale (26.6 per unit, 16.16)
    let scale = ((size_scale + 32) / 64) as Fixed; // cf2_getScaleAndHintFlag: px per unit, 16.16
    let blues = blues_init(&private, scale);
    let mut stems: Vec<StemHint> = Vec::new();
    let mut mask = HintMask::default();
    let mut gp = GlyphPath {
        scale_x: scale,
        initial: HintMap::new(scale),
        first: HintMap::new(scale),
        map: HintMap::new(scale),
        move_pending: true,
        path_open: false,
        path_closing: false,
        elem_queued: false,
        prev_op: ElemOp::Line,
        prev: [(0, 0); 4],
        current_cs: (0, 0),
        current_ds: (0, 0),
        start: (0, 0),
        offset_start0: (0, 0),
        offset_start1: (0, 0),
        b: Builder::default(),
        trace: log.as_ref().map(|_| Vec::new()),
    };
    let mut it = Interp { stack: Vec::with_capacity(48), x: 0, y: 0, have_width: false, depth: 0, ops: 0, seac: false };
    let ok = {
        let mut c = Ctx { stems: &mut stems, vstems: 0, mask: &mut mask, blues: &blues };
        let r = it.run(cs, &subrs, &gsubrs, &mut gp, &mut c);
        if r.is_some() && !it.seac {
            gp.close_open_path(&mut c);
        }
        r.is_some() && !it.seac
    };
    if let (Some(l), Some(t)) = (log, gp.trace.take()) {
        *l = t;
    }
    if !ok {
        return None;
    }
    gp.b.close_contour();
    let mut o = gp.b.out;
    // drop a dangling contour record
    while o.ends.last() == Some(&usize::MAX) {
        o.ends.pop();
    }
    Some(o)
}

struct Interp {
    stack: Vec<Fixed>,
    x: Fixed,
    y: Fixed,
    have_width: bool,
    depth: u8,
    ops: u32,
    seac: bool,
}

fn bias(n: usize) -> i32 {
    if n < 1240 {
        107
    } else if n < 33900 {
        1131
    } else {
        32768
    }
}

impl Interp {
    fn arg(&self, i: usize) -> Fixed {
        self.stack.get(i).copied().unwrap_or(0)
    }

    fn do_stems(&mut self, stems: &mut Vec<StemHint>) {
        let count = self.stack.len();
        let has_width = count & 1 == 1;
        let mut pos: Fixed = 0;
        let mut i = if has_width { 1 } else { 0 };
        while i + 1 < count + 1 && i < count {
            let min = add(pos, self.arg(i));
            pos = min;
            let max = add(pos, self.arg(i + 1));
            pos = max;
            stems.push(StemHint { min, max, used: false, min_ds: 0, max_ds: 0 });
            i += 2;
        }
        self.stack.clear();
        self.have_width = true;
    }

    /// `cf2_doStems` into the vertical array: only the count matters here.
    fn do_vstems(&mut self) -> usize {
        let count = self.stack.len();
        let has_width = count & 1 == 1;
        let n = (count - has_width as usize) / 2;
        self.stack.clear();
        self.have_width = true;
        n
    }

    fn flex(&mut self, gp: &mut GlyphPath, c: &mut Ctx, read: &[bool; 12], conditional_last: bool) {
        let mut vals = [0 as Fixed; 14];
        vals[0] = self.x;
        vals[1] = self.y;
        let mut idx = 0;
        let is_hflex = !read[9];
        let top = if is_hflex { 9 } else { 10 };
        for i in 0..top {
            vals[i + 2] = vals[i];
            if read[i] {
                vals[i + 2] = add(vals[i + 2], self.arg(idx));
                idx += 1;
            }
        }
        if is_hflex {
            vals[11] = self.y;
        }
        if conditional_last {
            let last_is_x = sub(vals[10], self.x).abs() > sub(vals[11], self.y).abs();
            let last = self.arg(idx);
            if last_is_x {
                vals[12] = add(vals[10], last);
                vals[13] = self.y;
            } else {
                vals[12] = self.x;
                vals[13] = add(vals[11], last);
            }
        } else {
            vals[12] = if read[10] {
                let v = add(vals[10], self.arg(idx));
                idx += 1;
                v
            } else {
                self.x
            };
            vals[13] = if read[11] { add(vals[11], self.arg(idx)) } else { self.y };
        }
        for j in 0..2 {
            gp.curve_to(c, vals[j * 6 + 2], vals[j * 6 + 3], vals[j * 6 + 4], vals[j * 6 + 5], vals[j * 6 + 6], vals[j * 6 + 7]);
        }
        self.stack.clear();
        self.x = vals[12];
        self.y = vals[13];
    }

    /// `cf2_interpT2CharString` for CFF (version 1) charstrings. Returns `Some(true)` at `endchar`.
    fn run(&mut self, cs: &[u8], subrs: &Index, gsubrs: &Index, gp: &mut GlyphPath, c: &mut Ctx) -> Option<bool> {
        if self.depth > 10 {
            return None;
        }
        let mut p = 0usize;
        loop {
            self.ops += 1;
            if self.ops > 100_000 {
                return None;
            }
            if p >= cs.len() {
                // end of buffer: an implicit return (subroutine) or endchar (top level)
                if self.depth > 0 {
                    return Some(false);
                }
                gp.close_open_path(c);
                return Some(true);
            }
            let op = cs[p];
            p += 1;
            let mut clear = true;
            match op {
                1 | 18 => {
                    if !c.mask.valid {
                        self.do_stems(c.stems);
                    }
                }
                3 | 23 => {
                    if !c.mask.valid {
                        c.vstems += self.do_vstems();
                    }
                }
                4 => {
                    self.have_width = true;
                    let dy = self.stack.last().copied().unwrap_or(0);
                    self.y = add(self.y, dy);
                    gp.move_to(c, self.x, self.y);
                }
                21 => {
                    self.have_width = true;
                    let n = self.stack.len();
                    let (dx, dy) = if n >= 2 { (self.stack[n - 2], self.stack[n - 1]) } else { (0, 0) };
                    self.y = add(self.y, dy);
                    self.x = add(self.x, dx);
                    gp.move_to(c, self.x, self.y);
                }
                22 => {
                    self.have_width = true;
                    let dx = self.stack.last().copied().unwrap_or(0);
                    self.x = add(self.x, dx);
                    gp.move_to(c, self.x, self.y);
                }
                5 => {
                    let n = self.stack.len();
                    let mut i = 0;
                    while i + 1 < n + 1 && i < n {
                        self.x = add(self.x, self.arg(i));
                        self.y = add(self.y, self.arg(i + 1));
                        gp.line_to(c, self.x, self.y);
                        i += 2;
                    }
                }
                6 | 7 => {
                    let mut is_x = op == 6;
                    for i in 0..self.stack.len() {
                        let v = self.stack[i];
                        if is_x {
                            self.x = add(self.x, v);
                        } else {
                            self.y = add(self.y, v);
                        }
                        is_x = !is_x;
                        gp.line_to(c, self.x, self.y);
                    }
                }
                8 | 24 => {
                    let n = self.stack.len();
                    let mut i = 0;
                    while i + 6 <= n {
                        let x1 = add(self.arg(i), self.x);
                        let y1 = add(self.arg(i + 1), self.y);
                        let x2 = add(self.arg(i + 2), x1);
                        let y2 = add(self.arg(i + 3), y1);
                        let x3 = add(self.arg(i + 4), x2);
                        let y3 = add(self.arg(i + 5), y2);
                        gp.curve_to(c, x1, y1, x2, y2, x3, y3);
                        self.x = x3;
                        self.y = y3;
                        i += 6;
                    }
                    if op == 24 {
                        self.x = add(self.x, self.arg(i));
                        self.y = add(self.y, self.arg(i + 1));
                        gp.line_to(c, self.x, self.y);
                    }
                }
                25 => {
                    let n = self.stack.len();
                    let mut i = 0;
                    while i + 6 < n {
                        self.x = add(self.x, self.arg(i));
                        self.y = add(self.y, self.arg(i + 1));
                        gp.line_to(c, self.x, self.y);
                        i += 2;
                    }
                    while i < n {
                        let x1 = add(self.arg(i), self.x);
                        let y1 = add(self.arg(i + 1), self.y);
                        let x2 = add(self.arg(i + 2), x1);
                        let y2 = add(self.arg(i + 3), y1);
                        let x3 = add(self.arg(i + 4), x2);
                        let y3 = add(self.arg(i + 5), y2);
                        gp.curve_to(c, x1, y1, x2, y2, x3, y3);
                        self.x = x3;
                        self.y = y3;
                        i += 6;
                    }
                }
                26 | 27 => {
                    let count1 = self.stack.len();
                    let count = count1 & !2usize;
                    let mut i = count1 - count;
                    while i < count {
                        let (x1, y1, x2, y2, x3, y3);
                        if op == 26 {
                            let xa = if (count - i) & 1 == 1 {
                                let v = add(self.arg(i), self.x);
                                i += 1;
                                v
                            } else {
                                self.x
                            };
                            x1 = xa;
                            y1 = add(self.arg(i), self.y);
                            x2 = add(self.arg(i + 1), x1);
                            y2 = add(self.arg(i + 2), y1);
                            x3 = x2;
                            y3 = add(self.arg(i + 3), y2);
                        } else {
                            let ya = if (count - i) & 1 == 1 {
                                let v = add(self.arg(i), self.y);
                                i += 1;
                                v
                            } else {
                                self.y
                            };
                            y1 = ya;
                            x1 = add(self.arg(i), self.x);
                            x2 = add(self.arg(i + 1), x1);
                            y2 = add(self.arg(i + 2), y1);
                            x3 = add(self.arg(i + 3), x2);
                            y3 = y2;
                        }
                        gp.curve_to(c, x1, y1, x2, y2, x3, y3);
                        self.x = x3;
                        self.y = y3;
                        i += 4;
                    }
                }
                30 | 31 => {
                    let count1 = self.stack.len();
                    let count = count1 & !2usize;
                    let mut i = count1 - count;
                    let mut alternate = op == 31;
                    while i < count {
                        let (x1, y1, x2, y2, x3, y3);
                        if alternate {
                            x1 = add(self.arg(i), self.x);
                            y1 = self.y;
                            x2 = add(self.arg(i + 1), x1);
                            y2 = add(self.arg(i + 2), y1);
                            y3 = add(self.arg(i + 3), y2);
                            if count - i == 5 {
                                x3 = add(self.arg(i + 4), x2);
                                i += 1;
                            } else {
                                x3 = x2;
                            }
                            alternate = false;
                        } else {
                            x1 = self.x;
                            y1 = add(self.arg(i), self.y);
                            x2 = add(self.arg(i + 1), x1);
                            y2 = add(self.arg(i + 2), y1);
                            x3 = add(self.arg(i + 3), x2);
                            if count - i == 5 {
                                y3 = add(self.arg(i + 4), y2);
                                i += 1;
                            } else {
                                y3 = y2;
                            }
                            alternate = true;
                        }
                        gp.curve_to(c, x1, y1, x2, y2, x3, y3);
                        self.x = x3;
                        self.y = y3;
                        i += 4;
                    }
                }
                10 | 29 => {
                    let n = self.stack.pop()? >> 16;
                    let (idx, b) = if op == 10 { (subrs, bias(subrs.len())) } else { (gsubrs, bias(gsubrs.len())) };
                    let i = n.checked_add(b)?;
                    let sub_cs = idx.get(usize::try_from(i).ok()?)?;
                    self.depth += 1;
                    let done = self.run(sub_cs, subrs, gsubrs, gp, c)?;
                    self.depth -= 1;
                    if done {
                        return Some(true);
                    }
                    clear = false;
                }
                11 => return Some(false),
                14 => {
                    self.have_width = true;
                    gp.close_open_path(c);
                    if self.stack.len() > 1 {
                        self.seac = true;
                    }
                    return Some(true);
                }
                19 | 20 => {
                    if self.stack.len() > 1 && c.mask.valid {
                        // FreeType ignores the operator (and so misreads the mask bytes); treat as malformed
                        return None;
                    }
                    c.vstems += self.do_vstems();
                    let bits = c.stems.len() + c.vstems;
                    let bytes = bits.div_ceil(8);
                    let mbytes = cs.get(p..p + bytes)?;
                    p += bytes;
                    if op == 19 {
                        if c.mask.set_counts(bits) {
                            c.mask.mask = [0; 12];
                            c.mask.mask[..bytes].copy_from_slice(mbytes);
                        }
                    } else {
                        // cntrmask: place and lock the stems of this counter group through a temporary map
                        let mut cm = HintMask::default();
                        if cm.set_counts(bits) {
                            cm.mask[..bytes].copy_from_slice(mbytes);
                        }
                        gp.ensure_initial(c);
                        let mut tmp = HintMap::new(gp.map.scale);
                        let initial = gp.initial.clone();
                        tmp.build(Some(&initial), c.stems, c.vstems, &mut cm, c.blues);
                    }
                }
                12 => {
                    let op2 = *cs.get(p)?;
                    p += 1;
                    match op2 {
                        34 => {
                            self.flex(gp, c, &[true, false, true, true, true, false, true, false, true, false, true, false], false);
                            clear = false;
                        }
                        35 => self.flex(gp, c, &[true; 12], false),
                        36 => {
                            self.flex(gp, c, &[true, true, true, true, true, false, true, false, true, true, true, false], false);
                            clear = false;
                        }
                        37 => {
                            self.flex(gp, c, &[true, true, true, true, true, true, true, true, true, true, false, false], true);
                            clear = false;
                        }
                        _ => {} // arithmetic / storage operators: not used by hinting-relevant fonts here
                    }
                }
                28 => {
                    let v = i16::from_be_bytes([*cs.get(p)?, *cs.get(p + 1)?]);
                    p += 2;
                    self.stack.push(int_to_fixed(v as i32));
                    clear = false;
                }
                32..=246 => {
                    self.stack.push(int_to_fixed(op as i32 - 139));
                    clear = false;
                }
                247..=250 => {
                    let v = (op as i32 - 247) * 256 + *cs.get(p)? as i32 + 108;
                    p += 1;
                    self.stack.push(int_to_fixed(v));
                    clear = false;
                }
                251..=254 => {
                    let v = -(op as i32 - 251) * 256 - *cs.get(p)? as i32 - 108;
                    p += 1;
                    self.stack.push(int_to_fixed(v));
                    clear = false;
                }
                255 => {
                    let v = i32::from_be_bytes([*cs.get(p)?, *cs.get(p + 1)?, *cs.get(p + 2)?, *cs.get(p + 3)?]);
                    p += 4;
                    self.stack.push(v);
                    clear = false;
                }
                _ => {}
            }
            if self.stack.len() > 48 {
                return None;
            }
            if clear {
                self.stack.clear();
            }
        }
    }
}
