// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm (DESKTOPCRAFT B631: the Una look's paint — the window chrome, the menu bar's VEIL band, the
//! dock's plate and tiles, the procedural wallpaper, and `tests desktopcraft`; DESIGNAPPLY B639: the window SHADOW
//! outside the box — the elevation levels, the pre-rendered 9-slice tiles and the casts the compositor shades)
//!
//! DESKTOPCRAFT (rmbp-ledger B631). Peter, 2026-10-14: *"it's giving more a early x-windows vibe opposed to refined
//! mac"* and *"we want people to see our screenshots and be strongly compelled to try unaos"*. Every pixel here is
//! drawn with GLASSKIT (`video::draw`); no geometry is decided here — the boxes, the disc hit boxes, the strip rects
//! and the resize zones are the owners' (`wm`, `dock`, `menubar`), and this module only paints inside them. The
//! design record: `docs/dev/OS/05_USER_EXPERIENCE/DESKTOPCRAFT.md`; DESIGNAPPLY's (the frame's removal, the shadow
//! margin math, the tiles): `docs/dev/evidence/designapply/DESIGNAPPLY.md`.
//!
//! Declared at `wm.rs`'s tail with `#[path]` (fbdirect's precedent), so it is `wm::desktop` and reads the window
//! row's fields; `video/mod.rs` does not move. Same gate as `draw` (x86 `wc`, aarch64 `desktop_firmware`).
//!
//! * [`chrome_paint`] — from `wm::paint_window`, the Una arm beside Softy's (one LINE-NEUTRAL fold): the head, a flat
//!   `surface.window` title band with rounded top corners (square when zoomed), the head's stretch of the EDGE
//!   ([`edge_ink`]), the three discs, the centred title and the title rule. The corner pixels outside the arc are the
//!   desktop's (the wallpaper under every shadow that reaches them).
//! * [`edge_paint`] — from `wm::paint_window`'s tail, after the content (WINEDGE B659): the window's EDGE below the
//!   head — a 1-device-px hairline down both sides and along the foot, [`edge_ink`], inside the box, so the edge is
//!   one line all the way round and the shadow lies beyond it.
//!   Softy keeps its own painter (`look::is_softy()` decides first).
//! * [`hover_track`] — from `dock::compose` (Una only): the disc group under the pointer, latched; a change damages
//!   the heads involved so their glyphs appear and go.
//! * [`track`] / [`shade_bytes`] / [`shade_words`] — THE SHADOW, both looks (below).
//! * [`bar_row`] / [`dock_plate_row`] / [`dock_tile_row`] — the menu bar's field and the dock's plate and tiles.
//! * [`wall_gen`] / [`veil_gen`] — the default wallpaper and its VEIL (the blurred copy every translucent strip
//!   samples), built once per wallpaper by `video::wallpaper`. STARFIELD (B661, R128): the default is the scene —
//!   dark matter on ultra black, lensed stars, the zircon and the beam — and its sparse SHIMMER, re-tinted per tick by
//!   [`shimmer_tick`] from [`track`]'s tail; `tests starfield` ([`starfield_selftest`]).
//! * [`VeilCache`] — VEILCOST (rmbp-ledger B690, BP-bar-23): each veiled strip's composed field (the menu bar's,
//!   the dock's plate), kept once per picture x theme epoch x rect, so a repaint is a row copy.
//! * [`selftest`] — `tests desktopcraft` (R80: a verb, never at boot).
//!
//! THE SHADOW AND THE COMPOSITOR (DESIGNAPPLY B639, R122, DESIGN-LANGUAGE §2.4). The 5 pt frame and the bevel are
//! gone (`wm::BORDER` is 0): a window's box is its head and its content, and its shadow lies OUTSIDE the box, in a
//! MARGIN `top / side / bottom` device px wide (the elevation level's extent: `side = max blur`, `top = max(blur -
//! dy)`, `bottom = max(blur + dy)` over its layers). `wm` paints each window straight into the panel, inside its box,
//! and repaints only damaged boxes, so a shadow can never be blended over what is already there (it would darken
//! again on every present). It is instead part of the DESKTOP: the background fill (`wm::stage_fill`, the erase path
//! that paints the wallpaper wherever no window box is) multiplies each pixel it writes by every window's shadow at
//! that pixel — computed from scratch from the wallpaper, so a pixel is the same however often it is painted. The
//! compositor's side ([`track`], at the head of every composite pass): the windows that cast (used, above the shell,
//! not compat, not zoomed) are snapshotted with their level (focused = `elev.3`, else `elev.3i`); any window whose
//! box, level, theme or scale moved, appeared or went has the union of its old and new shadow extents queued for
//! that fill (`wm::defer_erase`), which the same pass drains (clipped by every window box and the furniture) and
//! whose `damage_intersecting` repaints the windows the extent crosses. `wm::occluders` reports the extents, so the
//! desktop layer's own presents never paint over a shadow. A shadow falls on the desktop, not on a lower window
//! (the fill is clipped by every box): the 1-px EDGE and the shadow's edge on the desktop carry the depth.
//!
//! THE EDGE (WINEDGE B659; DESIGN-LANGUAGE §2.9's 2026-10-13 amendment). Peter, flight 35: *"new prefs panel looking
//! very nice. new borders not so much."* — DESIGNAPPLY's frameless box with only the outside shadow did not read as a
//! window edge to him. A window now has a VISIBLE EDGE: a 1-device-px hairline (`line.strong` on Light,
//! `line.subtle` on Dark — [`edge_ink`], never a literal) round the whole box at every scale. The head draws its
//! stretch (the rounded top, in [`chrome_row`]); the rest is drawn over the content's outermost pixels after the
//! content by [`edge_paint`], because with `wm::BORDER()` = 0 the content reaches the box's sides and foot and would
//! paint over anything laid before it (DESIGNAPPLY's own outline ran only down the head for that reason). The edge is
//! inside the box, so the shadow's margin, the resize band and the occluder extents do not move.
//!
//! THE TILES. A shadow is never computed per frame (§2.4). Per (radius, level, scale, theme) one [`Tiles`] is
//! rendered once and kept (bounded: [`TILE_SLOTS`]): the top and bottom CORNER tiles (left; the right mirrors) and
//! the three straight RAMPS (top, bottom, side) — a 9-slice of alpha. Each pixel is `1 - prod(1 - A_k F(sd_k))`
//! over the level's layers, `sd_k` the signed distance (quarter px) to the rounded box shifted down `dy_k`, `F` the
//! GLASSKIT smoothstep over `[-blur, +blur]` (half the peak on the edge). A shaded pixel is then one table read per
//! cast that reaches it and one multiply per channel.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::video::draw::{self, Ink, Radii, Rect, Surf};
use crate::video::text::{self, Face};
use crate::video::theme;

use super::{Ctrl, Window};

// ── the numbers, named once ─────────────────────────────────────────────────────────────────────────────────────

/// One shadow layer (§2.4's `x y blur` notation, x always 0): black, `dy` pt down, `blur` pt, at `alpha` of 255.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layer {
    pub dy: usize,
    pub blur: usize,
    pub alpha: u32,
}

const fn ly(dy: usize, blur: usize, alpha: u32) -> Layer {
    Layer { dy, blur, alpha }
}
const NO_LAYER: Layer = ly(0, 0, 0);

/// The elevation levels (§2.4), indices into [`Craft::elev`]. `elev.0` (flat) casts nothing and has no row;
/// `elev.4` (modal) is [`ELEV_WINDOW`] with the scrim (`theme::SCRIM_ALPHA`) behind.
pub const ELEV_RAISED: usize = 0;
pub const ELEV_OVERLAY: usize = 1;
pub const ELEV_WINDOW: usize = 2;
pub const ELEV_IDLE: usize = 3;
pub const ELEV_NAMES: [&str; 4] = ["elev.1", "elev.2", "elev.3", "elev.3i"];

/// The Una look's paint dimensions and strengths. Lengths are pt (DESIGN-LANGUAGE §2.1: one pt is `video::dpi`'s
/// scale in device px, `ui::px`); strengths are of 255. DESIGNAPPLY (B639) aligned this table to the language.
pub struct Craft {
    /// `radius.window` (§2.5): Una 12, Softy 8. The head's top corners; square when zoomed.
    pub win_radius: usize,
    pub win_radius_softy: usize,
    /// The elevation levels: `[level][dark as usize][layer]` (§2.4's table, alphas of 255).
    pub elev: [[[Layer; 2]; 2]; 4],
    /// A hovered disc's glyph: black at this opacity (50 %, §3.1).
    pub glyph_alpha: u32,
    /// The VEIL's tint (§2.4): `surface.window` at 72 % (Light) / 78 % (Dark) — the menu bar and the dock.
    pub veil_tint: (u32, u32),
    /// The dock plate's corner radius (`radius.dock`, §2.5).
    pub dock_radius: usize,
    /// A hovered dock tile: the ink over the tile at this opacity (10 = 4 %).
    pub dock_hover: u32,
    /// The running indicator: its diameter and its opacity (153 = 60 %).
    pub dot: usize,
    pub dot_alpha: u32,
}

pub const CRAFT: Craft = Craft {
    win_radius: 12,
    win_radius_softy: 8,
    elev: [
        // elev.1 raised (knobs, the segmented thumb): Light 0 1 2 @ 14 % + 0 0 1 @ 10 %; Dark 0 1 2 @ 40 %
        [[ly(1, 2, 36), ly(0, 1, 26)], [ly(1, 2, 102), NO_LAYER]],
        // elev.2 overlay (menus, popovers, toasts): 0 0 2 @ 12 % + 0 8 24 @ 18 %; Dark @ 30 % / 45 % (+ rim.dark)
        [[ly(0, 2, 31), ly(8, 24, 46)], [ly(0, 2, 77), ly(8, 24, 115)]],
        // elev.3 window, focused: 0 0 2 @ 12 % + 0 16 40 @ 26 %; Dark @ 40 % / 55 % (+ the edge, `edge_ink`, both)
        [[ly(0, 2, 31), ly(16, 40, 66)], [ly(0, 2, 102), ly(16, 40, 140)]],
        // elev.3i window, unfocused: 0 0 2 @ 8 % + 0 8 20 @ 14 %; Dark @ 30 % / 35 %
        [[ly(0, 2, 20), ly(8, 20, 36)], [ly(0, 2, 77), ly(8, 20, 89)]],
    ],
    glyph_alpha: 128,
    veil_tint: (184, 199),
    dock_radius: 18,
    dock_hover: 10,
    dot: 4,
    dot_alpha: 153,
};

/// A pt length at the panel's scale, in device px.
#[inline]
fn d(n: usize) -> i32 {
    crate::ui::px(n) as i32
}

/// The widest row any painter here composes (the strip primitive's own bound).
const MAX_ROW: usize = crate::video::strip::MAX_STRIP_W;

/// Composed rows (try-locked, never waited on: the compositor paints masked, possibly on several staging cores).
static ROWS: [crate::sync::Mutex<[u32; MAX_ROW]>; 4] = [const { crate::sync::Mutex::new([0u32; MAX_ROW]) }; 4];

// ── the backdrop ────────────────────────────────────────────────────────────────────────────────────────────────

/// The VEIL's cell: the wallpaper box-averaged over `VEIL_CELL` x `VEIL_CELL` px, then a [1 2 1] pass each way.
pub const VEIL_SHIFT: usize = 3;
pub const VEIL_CELL: usize = 1 << VEIL_SHIFT;

/// What lies under the furniture: the wallpaper (sharp) and its VEIL (blurred, sampled bilinearly).
#[derive(Clone, Copy)]
pub struct Back<'a> {
    pub px: &'a [u32],
    pub w: usize,
    pub h: usize,
    pub veil: &'a [u32],
    pub vw: usize,
    pub vh: usize,
}

impl Back<'_> {
    /// The wallpaper's pixel at panel `(x, y)` (`DESKTOP_BG` off the picture).
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u32 {
        if x < self.w && y < self.h {
            self.px.get(y * self.w + x).copied().unwrap_or(theme::DESKTOP_BG)
        } else {
            theme::DESKTOP_BG
        }
    }
    /// The wallpaper's row `y` from column `x0` into `out`.
    pub fn sharp(&self, out: &mut [u32], x0: usize, y: usize) {
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.get(x0 + i, y);
        }
    }
    /// The VEIL's row `y` from column `x0` into `out`: bilinear between cell centres.
    pub fn soft(&self, out: &mut [u32], x0: usize, y: usize) {
        if self.vw == 0 || self.vh == 0 || self.veil.len() < self.vw * self.vh {
            out.fill(theme::DESKTOP_BG);
            return;
        }
        let v = y.saturating_sub(VEIL_CELL / 2);
        let j0 = (v >> VEIL_SHIFT).min(self.vh - 1);
        let j1 = (j0 + 1).min(self.vh - 1);
        let fy = (v & (VEIL_CELL - 1)) as u32;
        let (r0, r1) = (&self.veil[j0 * self.vw..(j0 + 1) * self.vw], &self.veil[j1 * self.vw..(j1 + 1) * self.vw]);
        for (i, o) in out.iter_mut().enumerate() {
            let u = (x0 + i).saturating_sub(VEIL_CELL / 2);
            let i0 = (u >> VEIL_SHIFT).min(self.vw - 1);
            let i1 = (i0 + 1).min(self.vw - 1);
            let fx = (u & (VEIL_CELL - 1)) as u32;
            let c = VEIL_CELL as u32;
            let w00 = (c - fx) * (c - fy);
            let w10 = fx * (c - fy);
            let w01 = (c - fx) * fy;
            let w11 = fx * fy;
            let (a, b, e, f) = (r0[i0], r0[i1], r1[i0], r1[i1]);
            let sh = 2 * VEIL_SHIFT as u32;
            let rb = (((a & 0x00FF_00FF) * w00 + (b & 0x00FF_00FF) * w10 + (e & 0x00FF_00FF) * w01 + (f & 0x00FF_00FF) * w11) >> sh) & 0x00FF_00FF;
            let g = (((a & 0x0000_FF00) * w00 + (b & 0x0000_FF00) * w10 + (e & 0x0000_FF00) * w01 + (f & 0x0000_FF00) * w11) >> sh) & 0x0000_FF00;
            *o = rb | g;
        }
    }
}

/// The live backdrop, when a wallpaper is installed (`video::wallpaper`'s published snapshot).
#[cfg(feature = "facet")]
#[inline]
pub fn live_back() -> Option<Back<'static>> {
    crate::video::wallpaper::back()
}
#[cfg(not(feature = "facet"))]
#[inline]
pub fn live_back() -> Option<Back<'static>> {
    None
}

/// Row `y` of the sharp backdrop (the flat desktop colour where there is no wallpaper).
#[inline]
fn back_sharp(b: Option<&Back>, out: &mut [u32], x0: usize, y: usize) {
    match b {
        Some(b) => b.sharp(out, x0, y),
        None => out.fill(theme::DESKTOP_BG),
    }
}

/// Row `y` of the VEIL (the flat desktop colour where there is no wallpaper).
#[inline]
fn back_soft(b: Option<&Back>, out: &mut [u32], x0: usize, y: usize) {
    match b {
        Some(b) => b.soft(out, x0, y),
        None => out.fill(theme::DESKTOP_BG),
    }
}

// ── the VEIL cache (VEILCOST B690, BP-bar-23: beauty costs no more than a solid fill) ───────────────────────────────

/// What a veiled field's pixels depend on: the picture (its buffers, size and a 32-cell fingerprint of its VEIL, so a
/// new picture allocated where an old one lay is still a new key), [`veil_cache_invalidate`]'s generation (the
/// wallpaper install), the theme epoch and the live face/tint/hairline (set, accent, contrast), and the strip's rect
/// plus one strip word (`extra`: the dock's corner radius, which follows the scale).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VeilKey {
    wgen: u32,
    pic: (usize, usize, usize, usize, u64),
    epoch: u32,
    look: (u32, u32, u32),
    rect: (usize, usize, usize, usize),
    extra: usize,
}

/// Bumped by [`veil_cache_invalidate`] (the wallpaper install): every cached field is stale.
static VEIL_GEN: AtomicU32 = AtomicU32::new(0);

/// Drop every cached veiled field (the next repaint of each strip composes once and keeps it).
pub fn veil_cache_invalidate() {
    VEIL_GEN.fetch_add(1, Ordering::AcqRel);
}

fn veil_key(b: Option<&Back>, rect: (usize, usize, usize, usize), extra: usize) -> VeilKey {
    let pic = match b {
        None => (0, 0, 0, 0, 0),
        Some(b) => {
            let n = b.veil.len();
            let mut f = 0xcbf2_9ce4_8422_2325u64 ^ (((b.w as u64) << 32) | b.h as u64);
            if n > 0 {
                for i in 0..32 {
                    f = (f ^ b.veil[i * (n - 1) / 31] as u64).wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            (b.px.as_ptr() as usize, b.veil.as_ptr() as usize, b.vw, b.vh, f)
        }
    };
    VeilKey { wgen: VEIL_GEN.load(Ordering::Acquire), pic, epoch: theme::epoch(), look: (theme::surface_window(), veil_tint(), theme::hairline()), rect, extra }
}

/// One strip's composed veiled field, `w` x `h` (`rows`, row-major) with a filled flag per row. A miss composes the
/// row and keeps it; a hit is one row copy. The buffer is `try_reserve`d (a refusal paints uncached, never fails).
pub struct VeilCache {
    key: Option<VeilKey>,
    w: usize,
    h: usize,
    rows: alloc::vec::Vec<u32>,
    have: alloc::vec::Vec<bool>,
}

impl VeilCache {
    pub const fn new() -> Self {
        Self { key: None, w: 0, h: 0, rows: alloc::vec::Vec::new(), have: alloc::vec::Vec::new() }
    }

    /// The bytes this cache holds (its field and its row flags).
    pub fn bytes(&self) -> usize {
        self.rows.capacity() * 4 + self.have.capacity()
    }

    fn reset(&mut self, key: VeilKey, w: usize, h: usize) -> bool {
        self.key = None;
        let n = w * h;
        if self.rows.len() != n {
            self.rows = alloc::vec::Vec::new();
            if self.rows.try_reserve_exact(n).is_err() {
                return false;
            }
            self.rows.resize(n, 0);
        }
        self.have.clear();
        if self.have.try_reserve_exact(h).is_err() {
            return false;
        }
        self.have.resize(h, false);
        (self.key, self.w, self.h) = (Some(key), w, h);
        true
    }

    /// Row `j` of the field keyed `key` into `out[..w]`: the kept row (returns `true`), else `paint(out)` and keep it.
    fn row(&mut self, key: VeilKey, out: &mut [u32], w: usize, h: usize, j: usize, paint: impl FnOnce(&mut [u32])) -> bool {
        if j >= h || out.len() < w || ((self.key != Some(key) || self.w != w || self.h != h) && !self.reset(key, w, h)) {
            paint(out);
            return false;
        }
        let o = j * w;
        if self.have[j] {
            out[..w].copy_from_slice(&self.rows[o..o + w]);
            return true;
        }
        paint(out);
        self.rows[o..o + w].copy_from_slice(&out[..w]);
        self.have[j] = true;
        false
    }
}

/// The live menu bar's and dock plate's fields (try-locked: a contended pass paints uncached, never waits).
static BAR_VEIL: crate::sync::Mutex<VeilCache> = crate::sync::Mutex::new(VeilCache::new());
static DOCK_VEIL: crate::sync::Mutex<VeilCache> = crate::sync::Mutex::new(VeilCache::new());

/// The bytes the live veiled strips' caches hold: `(bar, dock)`.
pub fn veil_cache_bytes() -> (usize, usize) {
    (BAR_VEIL.try_lock().map_or(0, |c| c.bytes()), DOCK_VEIL.try_lock().map_or(0, |c| c.bytes()))
}

/// [`bar_row`] through `c`: `true` when the row was the kept one (a hit).
#[allow(clippy::too_many_arguments)]
pub fn bar_row_cached(c: &mut VeilCache, out: &mut [u32], x0: usize, y0: usize, w: usize, h: usize, j: usize, back: Option<&Back>) -> bool {
    let k = veil_key(back, (x0, y0, w, h), 0);
    c.row(k, out, w, h, j, |o| bar_row(o, x0, y0, w, h, j, back))
}

/// [`dock_plate_row`] through `c`: `true` on a hit.
#[allow(clippy::too_many_arguments)]
pub fn dock_plate_row_cached(c: &mut VeilCache, out: &mut [u32], x0: usize, y0: usize, w: usize, h: usize, j: usize, back: Option<&Back>) -> bool {
    let k = veil_key(back, (x0, y0, w, h), d(CRAFT.dock_radius) as usize);
    c.row(k, out, w, h, j, |o| dock_plate_row(o, x0, y0, w, h, j, back))
}

// ── the shadow (DESIGNAPPLY B639, §2.4): the margins, the tiles, the casts ──────────────────────────────────────────

/// A level's layers in device px at scale `s2`: `(dy, blur, alpha)`, an empty layer `(0, 0, 0)`.
fn layers_px(level: usize, dark: bool, s2: u32) -> [(i32, i32, u32); 2] {
    let mut out = [(0i32, 0i32, 0u32); 2];
    for (k, l) in CRAFT.elev[level.min(3)][dark as usize].iter().enumerate() {
        if l.alpha == 0 || l.blur == 0 {
            continue;
        }
        let px = |n: usize| crate::video::dpi::px_at(n, s2) as i32;
        out[k] = (px(l.dy), px(l.blur).max(1), l.alpha.min(255));
    }
    out
}

/// A level's MARGIN outside the box, device px: `(top, side, bottom)` — the farthest any layer reaches
/// (`side = max blur`, `top = max(blur - dy)`, `bottom = max(blur + dy)`). Pure arithmetic: the occluder list
/// and the tiles both read it, so the box the desktop layer leaves alone is the box the shadow fills.
pub fn margins(level: usize, dark: bool, s2: u32) -> (i32, i32, i32) {
    let (mut t, mut s, mut b) = (0i32, 0i32, 0i32);
    for &(dy, bl, a) in layers_px(level, dark, s2).iter() {
        if a == 0 {
            continue;
        }
        s = s.max(bl);
        t = t.max(bl - dy);
        b = b.max(bl + dy);
    }
    (t, s, b)
}

/// The pre-rendered 9-slice of one (radius, level, scale, theme): two corner tiles (left; the right mirrors) and
/// three ramps, alpha of 255 (black). Built once by [`tiles_for`], leaked, kept in [`TILES`].
pub struct Tiles {
    /// The margin outside the box (device px).
    pub top: i32,
    pub side: i32,
    pub bot: i32,
    /// The corner radius (device px) and the level's largest drop.
    pub r: i32,
    pub dymax: i32,
    /// Corner-tile width (`side + r`), top-tile height (`top + r + dymax`), bottom-tile height (`r + bot`).
    pub cw: i32,
    pub th: i32,
    pub bh: i32,
    /// Row-major `th x cw` (rows from `box.top - top`, columns from `box.left - side`).
    top_c: &'static [u8],
    /// Row-major `bh x cw` (rows from `box.bottom - r`).
    bot_c: &'static [u8],
    /// `cw` (columns from `box.left - side`), `th` (rows from `box.top - top`), `bh` (rows from `box.bottom - r`).
    side_r: &'static [u8],
    top_r: &'static [u8],
    bot_r: &'static [u8],
    /// The key it was built for.
    key: (u16, u8, u8, bool),
    /// Bytes held (the witness's).
    pub bytes: usize,
}

/// How many tile sets may be kept: 2 levels x 2 themes x the Settings scale steps, with room — bounded, so a
/// session that cycles every scale and theme holds at most this many (each ~25 KiB at 2.0).
pub const TILE_SLOTS: usize = 24;
static TILES: crate::sync::Mutex<[Option<&'static Tiles>; TILE_SLOTS]> = crate::sync::Mutex::new([None; TILE_SLOTS]);
/// Microseconds the last tile build took (the cost line's `tiles_us=`), and how many sets are held.
static TILE_US: AtomicU32 = AtomicU32::new(0);

/// The GLASSKIT falloff for one layer at signed distance `sd4` (quarter px; positive = outside): the peak inside
/// `blur`, half on the edge, zero `blur` outside (3t^2 - 2t^3), of 255.
#[inline]
fn fall(sd4: i64, blur: i32, alpha: u32) -> u32 {
    let b4 = 4 * blur as i64;
    let t = (((sd4 + b4).clamp(0, 2 * b4) << 16) / (2 * b4).max(1)) as u64; // 0 ..= 65536
    let ss = (t * t >> 16) * (3 * 65536 - 2 * t) >> 16;
    ((alpha as u64 * (65536 - ss.min(65536)) + 32768) >> 16) as u32
}

/// The rounded box's signed distance (quarter px) for a point whose outward offsets from the corner circle's
/// centre are `(qx, qy)` (quarter px, positive = outside on that axis), corner radius `r4` (quarter px).
#[inline]
fn sd_corner(qx: i64, qy: i64, r4: i64) -> i64 {
    let (ox, oy) = (qx.max(0), qy.max(0));
    let out = draw::isqrt((ox * ox + oy * oy) as u64) as i64;
    out + qx.max(qy).min(0) - r4
}

/// Combine the layers' alphas at one point: `1 - prod(1 - a_k)`.
#[inline]
fn combine(ls: &[(i32, i32, u32); 2], mut each: impl FnMut(i32, i32) -> i64) -> u8 {
    let mut keep = 255u32;
    for &(dy, bl, a) in ls.iter() {
        if a == 0 {
            continue;
        }
        let ak = fall(each(dy, bl), bl, a);
        keep = keep * (255 - ak.min(255)) / 255;
    }
    (255 - keep) as u8
}

fn leak_bytes(n: usize) -> Option<&'static mut [u8]> {
    let mut v: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if v.try_reserve_exact(n.max(1)).is_err() {
        return None;
    }
    v.resize(n.max(1), 0);
    Some(alloc::vec::Vec::leak(v))
}

/// Render one tile set. Integer only; about `(top + r + dymax + r + bot) x (side + r)` square roots, once.
fn build_tiles(level: usize, dark: bool, s2: u32, r: i32) -> Option<Tiles> {
    let ls = layers_px(level, dark, s2);
    let (top, side, bot) = margins(level, dark, s2);
    let dymax = ls.iter().map(|l| l.0).max().unwrap_or(0);
    let r = r.max(0);
    let (cw, th, bh) = (side + r, top + r + dymax, r + bot);
    if cw <= 0 || th <= 0 || bh <= 0 || cw > 1024 || th > 1024 || bh > 1024 {
        return None;
    }
    let (cwu, thu, bhu) = (cw as usize, th as usize, bh as usize);
    let top_c = leak_bytes(thu * cwu)?;
    let bot_c = leak_bytes(bhu * cwu)?;
    let side_r = leak_bytes(cwu)?;
    let top_r = leak_bytes(thu)?;
    let bot_r = leak_bytes(bhu)?;
    let r4 = 4 * r as i64;
    // Tile coordinates: the box's left edge at x = side; the top tile's box top at y = top; the bottom tile's box
    // bottom at y = r. A pixel's centre is (4i + 2) quarter px.
    let (x0, yt, yb) = (4 * side as i64, 4 * top as i64, 4 * r as i64);
    for v in 0..thu {
        let py = 4 * v as i64 + 2;
        for u in 0..cwu {
            let px = 4 * u as i64 + 2;
            top_c[v * cwu + u] = combine(&ls, |dy, _| {
                let (cx, cy) = (x0 + r4, yt + 4 * dy as i64 + r4);
                sd_corner(cx - px, cy - py, r4)
            });
        }
        top_r[v] = combine(&ls, |dy, _| yt + 4 * dy as i64 - py);
    }
    for w in 0..bhu {
        let py = 4 * w as i64 + 2;
        for u in 0..cwu {
            let px = 4 * u as i64 + 2;
            bot_c[w * cwu + u] = combine(&ls, |dy, _| {
                let (cx, cy) = (x0 + r4, yb + 4 * dy as i64 - r4);
                sd_corner(cx - px, py - cy, r4)
            });
        }
        bot_r[w] = combine(&ls, |dy, _| py - (yb + 4 * dy as i64));
    }
    for u in 0..cwu {
        let px = 4 * u as i64 + 2;
        side_r[u] = combine(&ls, |_, _| x0 - px);
    }
    let bytes = thu * cwu + bhu * cwu + cwu + thu + bhu;
    Some(Tiles {
        top,
        side,
        bot,
        r,
        dymax,
        cw,
        th,
        bh,
        top_c,
        bot_c,
        side_r,
        top_r,
        bot_r,
        key: (r as u16, level as u8, s2 as u8, dark),
        bytes,
    })
}

/// The tile set for (radius `r` device px, `level`, the live scale, `dark`): the kept one, else rendered now and
/// kept. `None` without memory or with the table full of other keys (that window then casts nothing).
pub fn tiles_for(level: usize, dark: bool, r: i32) -> Option<&'static Tiles> {
    let s2 = crate::video::dpi::s2();
    let key = (r.max(0) as u16, level as u8, s2 as u8, dark);
    let mut g = TILES.lock();
    if let Some(t) = g.iter().flatten().find(|t| t.key == key) {
        return Some(*t);
    }
    let slot = g.iter().position(|t| t.is_none())?;
    let t0 = crate::arch::now_cycles();
    let t = build_tiles(level, dark, s2, r)?;
    let us = cycles_us(crate::arch::now_cycles().saturating_sub(t0));
    TILE_US.store(us as u32, Ordering::Relaxed);
    let t: &'static Tiles = alloc::boxed::Box::leak(alloc::boxed::Box::new(t));
    g[slot] = Some(t);
    Some(t)
}

/// Cycles to microseconds (the TSC where it is calibrated; 0 where the clock is unknown).
fn cycles_us(c: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let hz = crate::arch::x86_64::apic::tsc_hz();
        if hz >= 1_000_000 {
            return c / (hz / 1_000_000);
        }
    }
    let _ = c;
    0
}

/// The radius a window's head (and so its shadow) is drawn with, device px: §2.5's `radius.window` for the live
/// look, 0 when zoomed (§3.1: maximised and snapped windows are square).
fn win_radius(zoomed: bool) -> i32 {
    if zoomed {
        return 0;
    }
    if crate::video::look::is_softy() {
        return d(CRAFT.win_radius_softy);
    }
    d(CRAFT.win_radius)
}

/// One window's shadow as the compositor shades it: the box (panel px, half-open) and its tiles.
#[derive(Clone, Copy)]
pub struct Cast {
    pub id: super::WinId,
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub t: &'static Tiles,
}

impl Cast {
    /// The extent the shadow can touch (panel px, half-open): the box grown by the margin.
    pub fn extent(&self) -> (i32, i32, i32, i32) {
        (self.x0 - self.t.side, self.y0 - self.t.top, self.x1 + self.t.side, self.y1 + self.t.bot)
    }
    /// The shadow's alpha (of 255, black) along panel row `y`, columns `a .. b`: `f(x, alpha)` for every column the
    /// shadow can reach (alpha may be 0). The row's tile row is chosen once; the two corner spans read it (the right
    /// one mirrored), the straight span between them is one ramp value (zero beside the box: inside it).
    #[inline]
    pub fn for_each_alpha(&self, y: i32, a: i32, b: i32, mut f: impl FnMut(i32, u32)) {
        let t = self.t;
        if y < self.y0 - t.top || y >= self.y1 + t.bot {
            return;
        }
        let cw = t.cw as usize;
        let (tile_row, ramp): (&[u8], u32) = if y < self.y0 - t.top + t.th {
            let ri = (y - (self.y0 - t.top)) as usize;
            (t.top_c.get(ri * cw..(ri + 1) * cw).unwrap_or(&[]), t.top_r.get(ri).copied().unwrap_or(0) as u32)
        } else if y >= self.y1 - t.r {
            let ri = (y - (self.y1 - t.r)) as usize;
            (t.bot_c.get(ri * cw..(ri + 1) * cw).unwrap_or(&[]), t.bot_r.get(ri).copied().unwrap_or(0) as u32)
        } else {
            (t.side_r, 0)
        };
        if tile_row.len() < cw {
            return;
        }
        let (lx0, rx1) = (self.x0 - t.side, self.x1 + t.side);
        let (lx1, rx0) = (lx0 + t.cw, rx1 - t.cw);
        for x in a.max(lx0)..b.min(lx1) {
            f(x, tile_row[(x - lx0) as usize] as u32);
        }
        if ramp != 0 {
            for x in a.max(lx1)..b.min(rx0) {
                f(x, ramp);
            }
        }
        for x in a.max(rx0).max(lx1)..b.min(rx1) {
            f(x, tile_row[(rx1 - 1 - x) as usize] as u32);
        }
    }
}

/// The windows that cast, at most this many (the topmost by z; a deeper window casts nothing).
pub const MAX_CASTS: usize = 24;

/// The published casts (written by [`track`] at the head of each composite pass, read by the fills).
#[derive(Clone, Copy)]
pub struct Casts {
    pub n: usize,
    pub c: [Option<Cast>; MAX_CASTS],
}

impl Casts {
    pub const EMPTY: Casts = Casts { n: 0, c: [None; MAX_CASTS] };
    pub fn iter(&self) -> impl Iterator<Item = &Cast> {
        self.c[..self.n.min(MAX_CASTS)].iter().flatten()
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    fn push(&mut self, c: Cast) {
        if self.n < MAX_CASTS {
            self.c[self.n] = Some(c);
            self.n += 1;
        }
    }
    fn find(&self, id: super::WinId) -> Option<&Cast> {
        self.iter().find(|c| c.id == id)
    }
}

static CASTS: crate::sync::Mutex<Casts> = crate::sync::Mutex::new(Casts::EMPTY);
/// The next [`track`] treats every cast as new (a wallpaper reload repainted the desktop layer under them).
static STALE: AtomicBool = AtomicBool::new(false);
/// Composite passes whose shadow pass queued at least one fill, since boot (the cost line's `track_fills=`).
static TRACK_FILLS: AtomicU32 = AtomicU32::new(0);

/// Does row `r` cast a shadow? Used, drawn above the shell, not a full-screen compat surface, not zoomed (§3.1),
/// with a box.
fn casts(r: &Window, shell: u32) -> bool {
    r.used && !r.compat && super::above_shell(r, shell) && r.zoom_saved.is_none() && r.w != 0 && r.h != 0
}

/// The box the desktop layer must leave alone for row `r`: its shadow extent when it casts (at the live focus,
/// theme and scale — the same margins the tiles are built with), else the box itself. For `wm::occluders`.
pub(super) fn occluder_box(r: &Window, shell: u32, focus: u64) -> (usize, usize, usize, usize) {
    let b = super::outer_box(r);
    if !casts(r, shell) {
        return b;
    }
    let level = if focus != 0 && focus == r.owner_asid { ELEV_WINDOW } else { ELEV_IDLE };
    let (t, s, bo) = margins(level, theme::is_dark(), crate::video::dpi::s2());
    let (x0, y0) = (b.0.saturating_sub(s as usize), b.1.saturating_sub(t as usize));
    (x0, y0, b.0 + b.2 + s as usize - x0, b.1 + b.3 + bo as usize - y0)
}

/// A copy of the published casts (one short hold, IRQs masked so a fill on an interrupt path cannot spin on a
/// holder it preempted).
pub fn casts_now() -> Casts {
    let _irq = crate::arch::IrqMask::new();
    *CASTS.lock()
}

/// A wallpaper reload repainted the desktop layer: every shadow is re-filled on the next pass (and one is asked for).
pub fn shadows_stale() {
    STALE.store(true, Ordering::Release);
    let _ = super::damage_intersecting(0, 0, 1 << 16, 1 << 16);
}

/// **The compositor's shadow pass** (`wm::composite_inner`'s head, before the erase drain): snapshot the windows
/// that cast, publish them, and queue a background fill over every extent that changed — the union of a window's
/// old and new extent when its box, level, theme or scale moved, its extent when it appeared or went. The same pass
/// drains the queue (`drain_deferred`), so the shadow lands with the move that caused it.
pub(super) fn track() {
    shim_note_pass(); // SHIMMERIDLE2 (B830): every pass counts, so the idle seat can tell a busy glass from a stranded one
    let Some(pi) = crate::video::panel_info_nonblocking() else { return };
    let (pw, ph) = (pi.width as i32, pi.height as i32);
    if pw == 0 || ph == 0 {
        return;
    }
    // 1. The casting rows, topmost first (z, id, box, focused, zoomed) — under the table, nothing else held.
    let mut rows: [(u32, super::WinId, (usize, usize, usize, usize), bool); MAX_CASTS] = [(0, 0, (0, 0, 0, 0), false); MAX_CASTS];
    let mut nr = 0usize;
    let (mut occ, mut no, mut occ_full) = ([(0usize, 0usize, 0usize, 0usize); SHIM_OCC], 0usize, false);
    {
        let shell = super::shell_z();
        let focus = super::focus_asid();
        let t = super::table();
        for r in t.rows.iter() {
            if r.used && super::above_shell(r, shell) {
                // STARFIELD (B661): every visible window's occluder box (its shadow extent) — the shimmer skips them.
                let b = occluder_box(r, shell, focus);
                if b.2 != 0 && b.3 != 0 {
                    if no < SHIM_OCC {
                        occ[no] = b;
                        no += 1;
                    } else {
                        occ_full = true;
                    }
                }
            }
            if !casts(r, shell) {
                continue;
            }
            let e = (r.z, r.id, super::outer_box(r), focus != 0 && focus == r.owner_asid);
            if nr < MAX_CASTS {
                rows[nr] = e;
                nr += 1;
            } else {
                let k = (0..MAX_CASTS).min_by_key(|&k| rows[k].0).unwrap_or(0);
                if rows[k].0 < e.0 {
                    rows[k] = e;
                }
            }
        }
    }
    // 2. Their tiles (rendered on first use, outside the table lock).
    let dark = theme::is_dark();
    let rad = win_radius(false);
    let (mut tf, mut ti): (Option<Option<&'static Tiles>>, Option<Option<&'static Tiles>>) = (None, None);
    let mut now = Casts::EMPTY;
    for &(_, id, b, focused) in rows[..nr].iter() {
        let slot = if focused { &mut tf } else { &mut ti };
        let level = if focused { ELEV_WINDOW } else { ELEV_IDLE };
        let Some(t) = *slot.get_or_insert_with(|| tiles_for(level, dark, rad)) else { continue };
        let (x0, y0) = (b.0 as i32, b.1 as i32);
        now.push(Cast { id, x0, y0, x1: x0 + b.2 as i32, y1: y0 + b.3 as i32, t });
    }
    // 3. Diff against the published set and publish.
    let mut fills: [(i32, i32, i32, i32); 2 * MAX_CASTS] = [(0, 0, 0, 0); 2 * MAX_CASTS];
    let mut nf = 0usize;
    let mut add = |e: (i32, i32, i32, i32)| {
        if nf < fills.len() {
            fills[nf] = e;
            nf += 1;
        }
    };
    let union = |a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3));
    {
        let _irq = crate::arch::IrqMask::new();
        let mut g = CASTS.lock();
        let stale = STALE.swap(false, Ordering::AcqRel);
        for c in now.iter() {
            match g.find(c.id) {
                Some(o) if !stale && o.x0 == c.x0 && o.y0 == c.y0 && o.x1 == c.x1 && o.y1 == c.y1 && core::ptr::eq(o.t, c.t) => {}
                Some(o) => add(union(o.extent(), c.extent())),
                None => add(c.extent()),
            }
        }
        for o in g.iter() {
            if now.find(o.id).is_none() {
                add(o.extent());
            }
        }
        *g = now;
    }
    if nf == 0 {
        if !occ_full {
            shimmer_live(&occ[..no], pw as usize, ph as usize); // STARFIELD (B661): the shimmer rides an idle pass only
        }
        return;
    }
    TRACK_FILLS.fetch_add(1, Ordering::Relaxed);
    // 4. Queue the fills, clipped to the panel (the drain paints them through `stage_fill`, which shades).
    for &(x0, y0, x1, y1) in fills[..nf].iter() {
        let (x0, y0, x1, y1) = (x0.clamp(0, pw), y0.clamp(0, ph), x1.clamp(0, pw), y1.clamp(0, ph));
        if x1 > x0 && y1 > y0 {
            super::defer_erase(x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize, super::DEFER_ROUTE, false);
        }
    }
}

/// `c` with every channel scaled by `keep` of 256.
#[inline]
fn keep_rgb(c: u32, keep: u32) -> u32 {
    let rb = ((c & 0x00FF_00FF) * keep >> 8) & 0x00FF_00FF;
    let g = ((c & 0x0000_FF00) * keep >> 8) & 0x0000_FF00;
    rb | g
}

/// `256 - a` with `a` of 255 mapped onto 256 (so 255 is fully black, 0 untouched).
#[inline]
fn keep_of(a: u32) -> u32 {
    256 - (a + (a >> 7)).min(256)
}

/// Darken panel row `py` (pixels `0x00RRGGBB`, the first at panel column `x0`) by every cast that reaches it.
pub fn shade_words(casts: &Casts, row: &mut [u32], py: i32, x0: i32) {
    let n = row.len() as i32;
    for c in casts.iter() {
        let (ex0, ey0, ex1, ey1) = c.extent();
        if py < ey0 || py >= ey1 {
            continue;
        }
        c.for_each_alpha(py, ex0.max(x0), ex1.min(x0 + n), |x, al| {
            if al != 0 {
                let p = &mut row[(x - x0) as usize];
                *p = keep_rgb(*p, keep_of(al));
            }
        });
    }
}

/// [`shade_words`] over a staged row of `bpp`-byte pixels in the PANEL's format (the fill's stage). Only the
/// 4-byte formats are shaded (every panel this compositor drives): a uniform multiply of the three colour bytes is
/// the same whichever order they are in, and the fourth byte is padding.
pub fn shade_bytes(casts: &Casts, row: &mut [u8], bpp: usize, py: i32, x0: i32) {
    if bpp != 4 {
        return;
    }
    let n = (row.len() / 4) as i32;
    for c in casts.iter() {
        let (ex0, ey0, ex1, ey1) = c.extent();
        if py < ey0 || py >= ey1 {
            continue;
        }
        c.for_each_alpha(py, ex0.max(x0), ex1.min(x0 + n), |x, al| {
            if al != 0 {
                let k = keep_of(al);
                let o = (x - x0) as usize * 4;
                for ch in &mut row[o..o + 3] {
                    *ch = ((*ch as u32 * k) >> 8) as u8;
                }
            }
        });
    }
}

// ── the window chrome ───────────────────────────────────────────────────────────────────────────────────────────

/// One window's head, as the painter needs it. Every coordinate is BOX-LOCAL device px (the box's origin).
pub struct Head<'t> {
    pub bw: usize,
    pub bh: usize,
    pub focused: bool,
    /// Square corners (zoomed: §3.1).
    pub square: bool,
    /// The three discs (close, minimise, zoom): `(x, y, diameter)`, from `wm::control_disc` (the hit geometry).
    pub ctrls: [Option<(i32, i32, i32)>; 3],
    /// The pointer is over the disc group: draw the glyphs.
    pub hover: bool,
    pub title: &'t [u8],
}

/// The head's top radius (device px) for `h`.
fn head_radius(h: &Head) -> i32 {
    win_radius(h.square).min(h.bw as i32 / 2).min(super::TITLE_H() as i32)
}

/// A disc's fill: the control colour, or `line.strong` on an unfocused window (§3.1).
fn disc_fill(which: usize, focused: bool) -> u32 {
    if !focused {
        return theme::line_strong();
    }
    match which {
        0 => theme::ctrl_close(),
        1 => theme::ctrl_min(),
        _ => theme::ctrl_zoom(),
    }
}

/// The left reserve the title keeps clear of (past the discs) — mirrored on the right so the title stays centred.
fn title_reserve(h: &Head) -> usize {
    let mut r = super::GAP();
    for (x, _, dd) in h.ctrls.iter().flatten() {
        r = r.max((*x + *dd) as usize + super::GAP());
    }
    r
}

/// Compose box-local head row `j` into `row[..bw]`. `(bx, by)` is the box's panel origin (the backdrop's and the
/// casts' coordinates). The corner pixels outside the arc are the desktop: the wallpaper under every cast's shadow.
pub fn chrome_row(row: &mut [u32], h: &Head, j: usize, bx: usize, by: usize, back: Option<&Back>, casts: &Casts) {
    let bw = h.bw;
    let t = super::TITLE_H() as i32;
    let jj = j as i32;
    if jj >= t || bw == 0 {
        return;
    }
    let row = &mut row[..bw];
    let rad = head_radius(h);
    // The desktop under the two corner squares (the title band below covers the rest of the row).
    if jj < rad {
        let k = (rad as usize).min(bw);
        for (x0, len) in [(0usize, k), (bw - k, k)] {
            let seg = &mut row[x0..x0 + len];
            back_sharp(back, seg, bx + x0, by + j);
            shade_words(casts, seg, (by + j) as i32, (bx + x0) as i32);
        }
    }
    let mut s = Surf::new(row, bw);
    let band_r = Rect::new(0, -jj, bw as i32, t);
    let radii = Radii::new(rad, rad, 0, 0);
    draw::fill_rrect(&mut s, band_r, radii, theme::surface_window());
    // The head's stretch of the EDGE (WINEDGE B659, §2.9 amended): one device px, opaque [`edge_ink`], run past the
    // band's foot so its lower arcs never land; [`edge_paint`] carries it on down the sides and along the foot. It was
    // `line.strong` @ 60 % (Light) / `rim.dark` (Dark) until flight 35 — too faint to read as an edge (Peter).
    draw::stroke_rrect(&mut s, Rect::new(0, -jj, bw as i32, t + rad + 2), radii, 1, Ink::solid(edge_ink()));
    // The rule between the title band and the content, between the two edge columns (the edge stays one line).
    if jj == t - 1 {
        draw::hairline_h(&mut s, 1, 0, bw as i32 - 2, theme::line_subtle());
    }
    // The three discs, anti-aliased; the glyphs (black @ 50 %) while the pointer is over the group.
    for (k, c) in h.ctrls.iter().enumerate() {
        let Some((x, y, dd)) = *c else { continue };
        if jj < y - 1 || jj > y + dd {
            continue;
        }
        let r = Rect::new(x, y - jj, dd, dd);
        draw::fill_disc(&mut s, r, disc_fill(k, h.focused));
        if h.hover {
            let ink = Ink::new(0, CRAFT.glyph_alpha);
            let sub = draw::SUB;
            let (cx, cy) = (x * sub + dd * sub / 2, (y - jj) * sub + dd * sub / 2);
            let g = dd * sub * 3 / 10;
            let wdt = (d(1) * sub * 5 / 4).max(sub);
            match k {
                0 => {
                    draw::line_aa(&mut s, cx - g, cy - g, cx + g, cy + g, wdt, ink);
                    draw::line_aa(&mut s, cx + g, cy - g, cx - g, cy + g, wdt, ink);
                }
                1 => draw::line_aa(&mut s, cx - g, cy, cx + g, cy, wdt, ink),
                _ => {
                    // Two diagonal arrowheads (zoom): one in the top-left quarter, one in the bottom-right.
                    let a = g * 3 / 4;
                    draw::line_aa(&mut s, cx - g, cy - g, cx - g + a, cy - g, wdt, ink);
                    draw::line_aa(&mut s, cx - g, cy - g, cx - g, cy - g + a, wdt, ink);
                    draw::line_aa(&mut s, cx + g, cy + g, cx + g - a, cy + g, wdt, ink);
                    draw::line_aa(&mut s, cx + g, cy + g, cx + g, cy + g - a, wdt, ink);
                }
            }
        }
    }
    // The title: the UI face, bold, centred on the window (not on the space the discs leave).
    let face = Face::Ui;
    let ch = face.cell_h() as i32;
    let ty = (t - ch) / 2;
    if jj < ty || jj >= ty + ch {
        return;
    }
    let reserve = title_reserve(h);
    let max_w = bw.saturating_sub(2 * reserve);
    let n = text::fit(h.title, true, face, max_w);
    if n == 0 {
        return;
    }
    let adv = text::advance(&h.title[..n], true, face);
    let tx = (bw.saturating_sub(adv) / 2).max(reserve);
    let ink = if h.focused { theme::text_primary() } else { theme::text_tertiary() };
    text::draw_row(&mut *s.px, bw, &h.title[..n], tx, (jj - ty) as usize, ink, true, face);
}

/// Paint head rows `rows` of a window box through `sink(j, x0, pixels)` (box-local row `j`, first column `x0`).
/// With no frame there is nothing to paint below the head: the content paints its own pixels. `row` is a scratch
/// row at least `bw` wide.
#[allow(clippy::too_many_arguments)]
pub fn paint_box(h: &Head, bx: usize, by: usize, back: Option<&Back>, casts: &Casts, row: &mut [u32], rows: core::ops::Range<usize>, mut sink: impl FnMut(usize, usize, &[u32])) {
    let bw = h.bw;
    if bw < 2 || h.bh < 2 || row.len() < bw {
        return;
    }
    let head = super::TITLE_H().min(h.bh);
    for j in rows {
        if j >= head {
            break;
        }
        chrome_row(row, h, j, bx, by, back, casts);
        sink(j, 0, &row[..bw]);
    }
}

/// WINEDGE (B659) — the window EDGE's ink: `line.strong` on Light, `line.subtle` on Dark (DESIGN-LANGUAGE §2.3's
/// roles; §2.9 as amended 2026-10-13). One function, read by the head's stroke ([`chrome_row`]), the body's
/// ([`edge_paint`]) and the fixture's `edge=` clause, so the three can never disagree.
pub fn edge_ink() -> u32 {
    if theme::is_dark() { theme::line_subtle() } else { theme::line_strong() }
}

/// WINEDGE (B659) — the box-local column spans `(x0, len)` of the EDGE on box row `j` of a `bw x bh` box, BELOW the
/// head (the head's rows are [`chrome_row`]'s stroke): the two side columns on every content row, the whole row on
/// the foot. Empty above the head, outside the box, or for a box too small to have an inside.
pub fn edge_spans(bw: usize, bh: usize, j: usize) -> [(usize, usize); 2] {
    let head = super::TITLE_H().min(bh);
    if bw < 2 || bh < 2 || j < head || j >= bh {
        return [(0, 0); 2];
    }
    if j + 1 == bh {
        return [(0, bw), (0, 0)];
    }
    [(0, 1), (bw - 1, 1)]
}

/// **The window EDGE below the head** (WINEDGE B659) into `dst` (a band whose origin is panel `(ox, oy)`): from
/// `wm::paint_window`'s TAIL, after the content upscale — the content reaches the box's sides and foot now that
/// `wm::BORDER()` is 0, so an edge laid before it (the keyline's old seat) would be painted over on every present.
/// Inside the box (the box's outermost column each side and its last row), so nothing outside the box moves: the
/// shadow still starts on the first pixel past it, the resize band is still outside it. Una only: Softy's painter
/// draws its own 1 px border (`softy::chrome`), and a compat row has no chrome. Cost: two pixels per content row and
/// one row at the foot, per compose of the band that holds them.
pub(super) fn edge_paint(dst: &crate::video::FrameBuffer, r: &Window, ox: usize, oy: usize) {
    if r.compat || crate::video::look::is_softy() {
        return;
    }
    let (bx, by, bw, bh) = super::outer_box(r);
    let Some(lx) = bx.checked_sub(ox) else { return };
    let ink = edge_ink();
    let dh = dst.info().height;
    let j0 = oy.saturating_sub(by).max(super::TITLE_H().min(bh));
    let j1 = (oy + dh).saturating_sub(by).min(bh);
    for j in j0..j1 {
        let y = by + j - oy;
        for (x0, len) in edge_spans(bw, bh, j) {
            for i in 0..len {
                dst.put_pixel(lx + x0 + i, y, ink);
            }
        }
    }
}

/// The window whose disc group is under the pointer (`0` = none).
static HOVER: AtomicU32 = AtomicU32::new(0);

/// **The Una chrome** of row `r` into `dst` (a band whose origin is panel `(ox, oy)`): from `wm::paint_window`,
/// after the stock chrome and before the content, exactly where Softy's painter sits. `ctrls` = the row draws a
/// control cluster (`wm::controls`). A row too wide for the scratch, or no free scratch row, keeps the stock chrome
/// for this pass (never a wait: the compositor paints masked).
pub(super) fn chrome_paint(dst: &crate::video::FrameBuffer, r: &Window, ox: usize, oy: usize, focused: bool, ctrls: bool) {
    let (bx, by, bw, bh) = super::outer_box(r);
    if bw == 0 || bh == 0 || bw > MAX_ROW {
        return;
    }
    let Some(mut g) = ROWS.iter().find_map(|m| m.try_lock()) else { return };
    let disc = |c: Ctrl| -> Option<(i32, i32, i32)> {
        if !ctrls {
            return None;
        }
        super::control_disc(r, c).map(|(x, y, dd)| (x as i32 - bx as i32, y as i32 - by as i32, dd as i32))
    };
    let h = Head {
        bw,
        bh,
        focused,
        square: r.zoom_saved.is_some(),
        ctrls: [disc(Ctrl::Close), disc(Ctrl::Minimise), disc(Ctrl::Zoom)],
        hover: ctrls && HOVER.load(Ordering::Relaxed) == r.id,
        title: &r.title[..r.title_len.min(r.title.len())],
    };
    let info = dst.info();
    let (dw, dh) = (info.width, info.height);
    let j0 = oy.saturating_sub(by);
    let j1 = (oy + dh).saturating_sub(by).min(bh).min(super::TITLE_H());
    if j0 >= j1 {
        return;
    }
    let back = live_back();
    let casts = casts_now();
    paint_box(&h, bx, by, back.as_ref(), &casts, &mut g[..], j0..j1, |j, x0, px| {
        let ly = by + j - oy;
        for (i, &c) in px.iter().enumerate() {
            let x = bx + x0 + i;
            if x >= ox && x - ox < dw {
                dst.put_pixel(x - ox, ly, c);
            }
        }
    });
}

/// The pointer's disc group, latched for [`chrome_paint`]; a change damages the heads it left and entered. Called
/// once per composite pass from `dock::compose` (Una only).
pub fn hover_track() {
    let Some(pi) = crate::video::panel_info_nonblocking() else { return };
    let (cx, cy) = crate::pal::cursor::pos(pi.width as i32, pi.height as i32);
    let now = match super::hit_test(cx, cy) {
        Some((win, _, _)) if super::control_hit(win, cx, cy).is_some() => win,
        _ => 0,
    };
    let was = HOVER.swap(now, Ordering::Relaxed);
    if was != now {
        damage_head(was);
        damage_head(now);
    }
}

fn damage_head(id: super::WinId) {
    if id == 0 {
        return;
    }
    if let Some(i) = super::info(id) {
        let (b, t) = (super::BORDER(), super::TITLE_H());
        let _ = super::damage_intersecting(i.x.saturating_sub(b), i.y.saturating_sub(t + b), i.w * i.scale + 2 * b, t + b);
    }
}

// ── the menu bar ────────────────────────────────────────────────────────────────────────────────────────────────

/// The VEIL's tint for the live set (§2.4: `surface.window` at 72 % Light / 78 % Dark).
#[inline]
fn veil_tint() -> u32 {
    if theme::is_dark() { CRAFT.veil_tint.1 } else { CRAFT.veil_tint.0 }
}

/// Row `j` of the menu bar's field (`w` x `h` at panel `(x0, y0)`) into `out[..w]`: `surface.window` at the VEIL's
/// tint over the VEIL, and one hairline where the bar meets the desktop.
pub fn bar_row(out: &mut [u32], x0: usize, y0: usize, w: usize, h: usize, j: usize, back: Option<&Back>) {
    let out = &mut out[..w];
    back_soft(back, out, x0, y0 + j);
    let face = theme::surface_window();
    for p in out.iter_mut() {
        *p = draw::blend(*p, face, veil_tint());
    }
    if j + 1 == h {
        let ink = Ink::argb(theme::hairline());
        for p in out.iter_mut() {
            *p = draw::blend(*p, ink.rgb, ink.a);
        }
    }
}

/// VEILCOST (B696): [`bar_row`] through the live cache when it is free (STAGEVERB B690's row copy), else uncached —
/// what the menu bar's compose costs, for the cost line's sample.
pub fn bar_row_live(out: &mut [u32], x0: usize, y0: usize, w: usize, h: usize, j: usize, back: Option<&Back>) {
    match BAR_VEIL.try_lock() {
        Some(mut c) => { bar_row_cached(&mut c, out, x0, y0, w, h, j, back); }
        None => bar_row(out, x0, y0, w, h, j, back),
    }
}

/// [`bar_row`] over the live wallpaper (the menu bar's compose).
pub fn bar_field_row(out: &mut [u32], x0: usize, y0: usize, w: usize, h: usize, j: usize) {
    let back = live_back();
    match BAR_VEIL.try_lock() {
        Some(mut c) => {
            bar_row_cached(&mut c, out, x0, y0, w, h, j, back.as_ref());
        }
        None => bar_row(out, x0, y0, w, h, j, back.as_ref()),
    }
}

/// Row `j` of the battery item at `x0`: a rounded outline with a fill level and a nub, centred in a bar `h`
/// tall; `(bw, bh)` the body, `(cw, ch)` the nub. Returns `true` (it drew).
#[allow(clippy::too_many_arguments)]
pub fn battery_row(out: &mut [u32], w: usize, h: usize, j: usize, x0: usize, body: (usize, usize), nub: (usize, usize), percent: u8, charging: bool) -> bool {
    let (bw, bh) = (body.0 as i32, body.1 as i32);
    let y0 = (h as i32 - bh) / 2 - j as i32;
    let x0 = x0 as i32;
    let mut s = Surf::new(&mut out[..w], w);
    let outline = theme::text_secondary();
    let ink = if percent <= 10 && !charging { theme::ink_critical() } else { theme::text_primary() };
    let r = Rect::new(x0, y0, bw, bh);
    let rad = (bh / 4).max(1);
    draw::stroke_rrect(&mut s, r, rad, 1, Ink::new(outline, 200));
    let inner = r.inset(2);
    let fill_w = inner.w * percent.min(100) as i32 / 100;
    if fill_w > 0 {
        draw::fill_rrect(&mut s, Rect::new(inner.x, inner.y, fill_w, inner.h), (rad - 1).max(1), ink);
    }
    let (cw, chh) = (nub.0 as i32, nub.1 as i32);
    draw::fill_rrect(&mut s, Rect::new(x0 + bw, y0 + (bh - chh) / 2, cw.max(1), chh), Radii::new(0, (cw / 2).max(1), (cw / 2).max(1), 0), Ink::new(outline, 200));
    if charging {
        let sub = draw::SUB;
        let (cx, cy) = (x0 * sub + bw * sub / 2, y0 * sub + bh * sub / 2);
        let (u, v) = (bw * sub / 8, bh * sub * 3 / 8);
        let wdt = sub.max(d(1) * sub);
        let bolt = theme::knob_face();
        draw::line_aa(&mut s, cx + u, cy - v, cx - u, cy, wdt, bolt);
        draw::line_aa(&mut s, cx - u, cy, cx + u, cy, wdt, bolt);
        draw::line_aa(&mut s, cx + u, cy, cx - u, cy + v, wdt, bolt);
    }
    true
}

// ── the dock ────────────────────────────────────────────────────────────────────────────────────────────────────

/// Row `j` of the dock's plate (`w` x `h` at panel `(x0, y0)`) into `out[..w]`: the VEIL's tint over the VEIL,
/// rounded at `dock_radius` (the wallpaper itself outside the corners), a hairline round its edge.
pub fn dock_plate_row(out: &mut [u32], x0: usize, y0: usize, w: usize, h: usize, j: usize, back: Option<&Back>) {
    let out = &mut out[..w];
    back_soft(back, out, x0, y0 + j);
    let face = theme::surface_window();
    for p in out.iter_mut() {
        *p = draw::blend(*p, face, veil_tint());
    }
    let rad = (d(CRAFT.dock_radius) as usize).min(w / 2).min(h / 2);
    let corner = j < rad || j + rad >= h;
    if corner && rad > 0 && rad <= 128 {
        // The corners: coverage of the plate's arc, read off the kit (blue channel of a zero row), and the sharp
        // wallpaper outside it.
        let mut cov = [0u32; 128];
        let mut cs = Surf::new(&mut cov[..rad], rad);
        draw::fill_rrect(&mut cs, Rect::new(0, -(j as i32), 2 * rad as i32 + 2, h as i32), rad as i32, 255u32);
        for k in 0..rad {
            let a = cov[k] & 255;
            if a == 255 {
                continue;
            }
            let l = draw::blend(back.map_or(theme::DESKTOP_BG, |b| b.get(x0 + k, y0 + j)), out[k], a);
            let rx = w - 1 - k;
            let r = draw::blend(back.map_or(theme::DESKTOP_BG, |b| b.get(x0 + rx, y0 + j)), out[rx], a);
            out[k] = l;
            out[rx] = r;
        }
    }
    let mut s = Surf::new(out, w);
    draw::stroke_rrect(&mut s, Rect::new(0, -(j as i32), w as i32, h as i32), rad as i32, 1, Ink::argb(theme::hairline()));
}

/// [`dock_plate_row`] over the live wallpaper (the dock's compose).
pub fn dock_field_row(out: &mut [u32], x0: usize, y0: usize, w: usize, h: usize, j: usize) {
    let back = live_back();
    match DOCK_VEIL.try_lock() {
        Some(mut c) => {
            dock_plate_row_cached(&mut c, out, x0, y0, w, h, j, back.as_ref());
        }
        None => dock_plate_row(out, x0, y0, w, h, j, back.as_ref()),
    }
}

/// Row `j` of one dock tile at strip-local `(x, y)`, `tw` x `th`, below which `pad` px hold the running dot:
/// a rounded square (the kit's icon radius), tinted 4 % while hovered, the pressed face while pressed; the 2 px
/// running dot centred under it.
#[allow(clippy::too_many_arguments)]
pub fn dock_tile_row(out: &mut [u32], w: usize, j: usize, x: usize, y: usize, tw: usize, th: usize, pad: usize, hover: bool, pressed: bool, running: bool) {
    let mut s = Surf::new(&mut out[..w], w);
    let jj = j as i32;
    if j >= y + th {
        if running && j < y + th + pad {
            let dd = d(CRAFT.dot).max(2);
            let r = Rect::new((x + tw / 2) as i32 - dd / 2, (y + th) as i32 + (pad as i32 - dd) / 2 - jj, dd, dd);
            draw::fill_disc(&mut s, r, Ink::new(theme::text_primary(), CRAFT.dot_alpha));
        }
        return;
    }
    if j < y {
        return;
    }
    let r = Rect::new(x as i32, y as i32 - jj, tw as i32, th as i32);
    let rad = draw::icon_radius(tw.min(th)) as i32;
    let face = if pressed { theme::surface_sunken() } else { theme::surface_raised() };
    draw::fill_rrect(&mut s, r, rad, face);
    if hover {
        draw::fill_rrect(&mut s, r, rad, Ink::new(theme::text_primary(), CRAFT.dock_hover));
    }
    draw::stroke_rrect(&mut s, r, rad, 1, Ink::argb(theme::hairline()));
}

// ── the wallpaper ───────────────────────────────────────────────────────────────────────────────────────────────

/// The 4x4 ordered-dither matrix in 1/256 of a step (`draw`'s own values).
const BAYER: [u32; 16] = [8, 136, 40, 168, 200, 72, 232, 104, 56, 184, 24, 152, 248, 120, 216, 88];

// ── STARFIELD (rmbp-ledger B661, R128) — the generated default: dark matter, lensed stars, the zircon, the beam ──────
//
// Peter (R128): the start screen SHIMMERS; the crystal less Picasso; the Starry Night flow read as DARK MATTER on ULTRA
// BLACK; a beam striking a temple-sized ZIRCON in the FOREGROUND; the background stars GRAVITATIONALLY LENSED through
// the flow's mass. The design record (written before this code, R106) is `docs/dev/OS/05_USER_EXPERIENCE/STARFIELD.md`:
// §3 the composition, §4 the field, §5 the lens math, §6 the zircon, §7 the beam, §8 the shimmer. Integer only. The
// picture is painted ONCE per install on the worker core (`wallpaper::generate`); per frame only the sparse SHIMMER runs
// ([`shimmer_tick`], seated at [`track`]'s tail). Every colour is a `theme::starfield` pair (`palette_core`).

/// The scene's palette, resolved once per generation from the live set (Dark is the scene, Light the quieter one).
#[derive(Clone, Copy)]
struct SfPal {
    void: u32,
    deep: u32,
    mid: u32,
    hi: u32,
    star: u32,
    warm: u32,
    zd: u32,
    zm: u32,
    zl: u32,
    fw: u32,
    fc: u32,
    core: u32,
    glow: u32,
}

impl SfPal {
    /// One look's set: [`wall_gen`] passes the live look; WINEDGE2 (B668)'s `shadow_dark=` probe draws both.
    fn of(dark: bool) -> SfPal {
        use crate::video::theme::starfield as sf;
        let p = |t: (u32, u32)| if dark { t.1 } else { t.0 };
        SfPal {
            void: p(sf::VOID),
            deep: p(sf::MATTER_DEEP),
            mid: p(sf::MATTER_MID),
            hi: p(sf::MATTER_HI),
            star: p(sf::STAR),
            warm: p(sf::STAR_WARM),
            zd: p(sf::ZIRCON_DEEP),
            zm: p(sf::ZIRCON_MID),
            zl: p(sf::ZIRCON_LIGHT),
            fw: p(sf::FIRE_WARM),
            fc: p(sf::FIRE_COOL),
            core: p(sf::BEAM_CORE),
            glow: p(sf::BEAM_GLOW),
        }
    }
}

/// `dst` toward `src` by `a` of 256 (exact at both ends; no lane carries into the next).
#[inline]
fn sf_mix(dst: u32, src: u32, a: u32) -> u32 {
    let a = a.min(256);
    let na = 256 - a;
    let rb = (((src & 0x00FF_00FF) * a + (dst & 0x00FF_00FF) * na) >> 8) & 0x00FF_00FF;
    let g = (((src & 0x0000_FF00) * a + (dst & 0x0000_FF00) * na) >> 8) & 0x0000_FF00;
    rb | g
}

/// The sum of the three channels' absolute differences.
#[inline]
fn sf_dist(a: u32, b: u32) -> u32 {
    [16u32, 8, 0].iter().map(|&s| (((a >> s) & 255) as i32 - ((b >> s) & 255) as i32).unsigned_abs()).sum()
}

/// A 64-bit mix of a lattice point and a seed (splitmix's finaliser).
#[inline]
fn sf_hash(a: i64, b: i64, seed: u64) -> u64 {
    let mut z = (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (b as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ seed.wrapping_mul(0x1656_67B1_9E37_79F9);
    z ^= z >> 30;
    z = z.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z ^= z >> 27;
    z = z.wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Value noise at pixel `(x, y)` over a lattice `l` px wide: Q12 (0..=4095), smoothstep-bilinear.
fn sf_vn(x: i64, y: i64, l: i64, seed: u64) -> i64 {
    let l = l.max(1);
    let (fx, fy) = ((x * 4096).div_euclid(l), (y * 4096).div_euclid(l));
    let (ix, iy) = (fx >> 12, fy >> 12);
    let (tx, ty) = (fx & 4095, fy & 4095);
    let sm = |t: i64| (t * t * (3 * 4096 - 2 * t)) >> 24;
    let (sx, sy) = (sm(tx), sm(ty));
    let hv = |a: i64, b: i64| (sf_hash(a, b, seed) >> 52) as i64;
    let (a, b, c, d) = (hv(ix, iy), hv(ix + 1, iy), hv(ix, iy + 1), hv(ix + 1, iy + 1));
    let top = a + (((b - a) * sx) >> 12);
    let bot = c + (((d - c) * sx) >> 12);
    top + (((bot - top) * sy) >> 12)
}

/// `sin(deg)` in Q12 (Bhaskara I's approximation; under 0.2 % off).
fn sf_sin(deg: i64) -> i64 {
    let d = deg.rem_euclid(360);
    let (x, s) = if d >= 180 { (d - 180, -1) } else { (d, 1) };
    let p = x * (180 - x);
    s * (4 * p * 4096) / (40500 - p)
}

fn sf_cos(deg: i64) -> i64 {
    sf_sin(deg + 90)
}

#[inline]
fn sf_isqrt(v: i64) -> i64 {
    draw::isqrt(v.max(0) as u64) as i64
}

/// A zeroed `i32` table, or `None` without memory.
fn sf_vec(n: usize) -> Option<alloc::vec::Vec<i32>> {
    let mut v = alloc::vec::Vec::new();
    v.try_reserve_exact(n).ok()?;
    v.resize(n, 0);
    Some(v)
}

/// Blend `col` into pixel `(x, y)` at `a` of 256 (bounds-checked).
#[inline]
fn sf_plot(px: &mut [u32], w: usize, h: usize, x: i64, y: i64, col: u32, a: u32) {
    if a == 0 || x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
        return;
    }
    let o = y as usize * w + x as usize;
    px[o] = sf_mix(px[o], col, a);
}

/// Blend `col` at the Q8 sub-pixel point `(x, y)`, spread bilinearly over its four pixels.
#[inline]
fn sf_splat(px: &mut [u32], w: usize, h: usize, x: i64, y: i64, col: u32, a: u32) {
    sf_splat_with(&mut |x, y, c, a| sf_plot(px, w, h, x, y, c, a), x, y, col, a);
}

/// STARTSCENE (B662): [`sf_splat`] through any plot sink `plot(x, y, colour, alpha of 256)` — the picture's buffer, an
/// offscreen buffer, or the write-only GOP framebuffer before the heap ([`stars_onto`]).
#[inline]
fn sf_splat_with(plot: &mut impl FnMut(i64, i64, u32, u32), x: i64, y: i64, col: u32, a: u32) {
    if a == 0 || x < 0 || y < 0 {
        return;
    }
    let (ix, iy) = (x >> 8, y >> 8);
    let (fx, fy) = ((x & 255) as u32, (y & 255) as u32);
    let ws = [(256 - fx) * (256 - fy), fx * (256 - fy), (256 - fx) * fy, fx * fy];
    for (k, &wt) in ws.iter().enumerate() {
        plot(ix + (k & 1) as i64, iy + (k >> 1) as i64, col, (a.min(256) * wt) >> 16);
    }
}

/// THE FIELD (STARFIELD.md §4): the dark-matter density, its smoothed potential, the flow (the curl of the stream
/// function) and the lens gradient, on a node grid of `c` px (node `i` at pixel `(i - 1) c`).
struct SfField {
    c: i64,
    gw: usize,
    gh: usize,
    dens: alloc::vec::Vec<i32>,
    phi: alloc::vec::Vec<i32>,
    ux: alloc::vec::Vec<i32>,
    uy: alloc::vec::Vec<i32>,
    gx: alloc::vec::Vec<i32>,
    gy: alloc::vec::Vec<i32>,
    gmax: i64,
}

/// The three Starry Night vortices: centre (permille of W, H), radius (permille of H), strength per px of radius.
const SF_VORTEX: [(i64, i64, i64, i64); 3] = [(340, 300, 200, 60), (620, 160, 110, -50), (140, 620, 130, 55)];

impl SfField {
    fn build(w: usize, h: usize) -> Option<SfField> {
        let (c, gw, gh) = sf_field_dims(w, h);
        let n = gw * gh;
        let (mut dens, mut phi, mut tmp) = (sf_vec(n)?, sf_vec(n)?, sf_vec(n)?);
        let (mut ux, mut uy, mut gx, mut gy) = (sf_vec(n)?, sf_vec(n)?, sf_vec(n)?, sf_vec(n)?);
        let gmax = sf_field_fill(w, h, (c, gw, gh), &mut dens, &mut phi, &mut tmp, Some((&mut ux[..], &mut uy[..])), &mut gx, &mut gy);
        Some(SfField { c, gw, gh, dens, phi, ux, uy, gx, gy, gmax })
    }

    /// STARTSCENE (B662): the lens view the stars read (the gradient tables and their maximum).
    fn lens(&self) -> SfLens<'_> {
        SfLens { c: self.c, gw: self.gw, gh: self.gh, gx: &self.gx, gy: &self.gy, gmax: self.gmax }
    }

    /// Table `v` bilinearly at the Q8 pixel point `(xq, yq)`.
    #[inline]
    fn at(&self, v: &[i32], xq: i64, yq: i64) -> i64 {
        sf_at(self.c, self.gw, self.gh, v, xq, yq)
    }
}

/// STARTSCENE (B662): the field's node grid for a `w` x `h` panel — `(cell px, columns, rows)`.
fn sf_field_dims(w: usize, h: usize) -> (i64, usize, usize) {
    let c = (h as i64 / 72).max(8);
    (c, w / c as usize + 4, h / c as usize + 4)
}

/// STARTSCENE (B662): THE FIELD into caller-owned tables of `gw * gh` nodes — the heap's for the picture
/// ([`SfField::build`]), a static scratch before the heap exists ([`stars_into`] / [`stars_onto`], the boot splash's GOP
/// stage). One body, so the lens and therefore every star lands on the same pixel either way. `flow` (the stream
/// function's curl, which only the strokes read) is skipped when `None`; `tmp` is scratch. Returns `Gmax` (at least 1).
#[allow(clippy::too_many_arguments)]
fn sf_field_fill(w: usize, h: usize, (c, gw, gh): (i64, usize, usize), dens: &mut [i32], phi: &mut [i32], tmp: &mut [i32], mut flow: Option<(&mut [i32], &mut [i32])>, gx: &mut [i32], gy: &mut [i32]) -> i64 {
    let (ww, hh) = (w as i64, h as i64);
    let l1 = (hh * 10 / 25).max(16);
    let (l2, l3, l0) = ((l1 * 10 / 21).max(8), (l1 * 10 / 44).max(4), (hh * 10 / 16).max(16));
    let ridge = |v: i64| {
        let r = 4095 - (2 * v - 4095).abs();
        let r = (r * r) >> 12;
        (r * r) >> 12
    };
    let node = |i: usize, j: usize| ((i as i64 - 1) * c, (j as i64 - 1) * c);
    for j in 0..gh {
        for i in 0..gw {
            let (x, y) = node(i, j);
            let (r1, r2, r3) = (ridge(sf_vn(x, y, l1, 1)), ridge(sf_vn(x, y, l2, 2)), ridge(sf_vn(x, y, l3, 3)));
            let d = (5 * r1 + 3 * ((r2 * (r1 + 1024)) >> 12) + 2 * ((r3 * (r2 + 1024)) >> 12)) / 10;
            dens[j * gw + i] = d.clamp(0, 4095) as i32;
        }
    }
    // The potential: the density blurred four times by [1 2 1] each way (the lens sees a smoother mass).
    phi.copy_from_slice(dens);
    for _ in 0..4 {
        for j in 0..gh {
            for i in 0..gw {
                let (l, r) = (i.saturating_sub(1), (i + 1).min(gw - 1));
                tmp[j * gw + i] = (phi[j * gw + l] + 2 * phi[j * gw + i] + phi[j * gw + r]) / 4;
            }
        }
        for j in 0..gh {
            let (u, d) = (j.saturating_sub(1), (j + 1).min(gh - 1));
            for i in 0..gw {
                phi[j * gw + i] = (tmp[u * gw + i] + 2 * tmp[j * gw + i] + tmp[d * gw + i]) / 4;
            }
        }
    }
    // The stream function: the potential, a coarse drift and the vortices (the strokes' input only).
    let rows = if flow.is_some() { gh } else { 0 };
    for j in 0..rows {
        for i in 0..gw {
            let (x, y) = node(i, j);
            let mut p = phi[j * gw + i] as i64 + (sf_vn(x, y, l0, 4) - 2048) / 2;
            for &(vx, vy, vr, va) in SF_VORTEX.iter() {
                let (cx, cy, rr) = (ww * vx / 1000, hh * vy / 1000, (hh * vr / 1000).max(1));
                let (dx, dy) = (x - cx, y - cy);
                let r2 = dx * dx + dy * dy;
                if r2 < rr * rr {
                    let q = 4096 - r2 * 4096 / (rr * rr);
                    p += va * rr * q / 4096 * q / 4096;
                }
            }
            tmp[j * gw + i] = p as i32;
        }
    }
    // The flow (the curl, normalised to Q12) and the lens gradient (per cell).
    let mut gm = 0i64;
    for j in 0..gh {
        let (u, d) = (j.saturating_sub(1), (j + 1).min(gh - 1));
        for i in 0..gw {
            let (l, r) = (i.saturating_sub(1), (i + 1).min(gw - 1));
            let k = j * gw + i;
            if let Some((ux, uy)) = flow.as_mut() {
                let (dpx, dpy) = ((tmp[j * gw + r] - tmp[j * gw + l]) as i64, (tmp[d * gw + i] - tmp[u * gw + i]) as i64);
                let (vx, vy) = (dpy, -dpx);
                let m = sf_isqrt(vx * vx + vy * vy);
                if m > 0 {
                    ux[k] = (vx * 4096 / m) as i32;
                    uy[k] = (vy * 4096 / m) as i32;
                } else {
                    ux[k] = 4096;
                    uy[k] = 0;
                }
            }
            let (a, b) = (((phi[j * gw + r] - phi[j * gw + l]) / 2) as i64, ((phi[d * gw + i] - phi[u * gw + i]) / 2) as i64);
            gx[k] = a as i32;
            gy[k] = b as i32;
            if i > 0 && j > 0 && i + 1 < gw && j + 1 < gh {
                gm = gm.max(sf_isqrt(a * a + b * b));
            }
        }
    }
    gm.max(1)
}

/// STARTSCENE (B662): what the stars read of the field — the lens gradient and its maximum (borrowed, so the GOP
/// stage's static tables and the picture's heap tables serve the same star pass).
struct SfLens<'a> {
    c: i64,
    gw: usize,
    gh: usize,
    gx: &'a [i32],
    gy: &'a [i32],
    gmax: i64,
}

impl SfLens<'_> {
    #[inline]
    fn at(&self, v: &[i32], xq: i64, yq: i64) -> i64 {
        sf_at(self.c, self.gw, self.gh, v, xq, yq)
    }
}

/// Table `v` (a `gw` x `gh` node grid of `c` px) bilinearly at the Q8 pixel point `(xq, yq)`.
#[inline]
fn sf_at(c: i64, gw: usize, gh: usize, v: &[i32], xq: i64, yq: i64) -> i64 {
    let cq = c << 8;
    let (u, s) = (xq + cq, yq + cq);
    let i = (u.max(0) / cq).min(gw as i64 - 2);
    let j = (s.max(0) / cq).min(gh as i64 - 2);
    let fx = ((u - i * cq) * 256 / cq).clamp(0, 256);
    let fy = ((s - j * cq) * 256 / cq).clamp(0, 256);
    let (i, j, g) = (i as usize, j as usize, gw);
    let (a, b, c, d) = (v[j * g + i] as i64, v[j * g + i + 1] as i64, v[(j + 1) * g + i] as i64, v[(j + 1) * g + i + 1] as i64);
    let t = a * 256 + (b - a) * fx;
    let bo = c * 256 + (d - c) * fx;
    (t * 256 + (bo - t) * fy) >> 16
}

/// 1. THE GROUND AND THE HAZE: ultra black, the dark-matter web as a faint glow (`v^2`, `v = (3 D + Phi) / 4`), a
/// vignette on the added light, ordered-dithered. Per pixel: two bilinear reads off per-column and per-row tables.
fn sf_haze(px: &mut [u32], w: usize, h: usize, f: &SfField, p: &SfPal) -> Option<()> {
    let cq = f.c;
    let mut cols: alloc::vec::Vec<(u32, i32)> = alloc::vec::Vec::new();
    cols.try_reserve_exact(w).ok()?;
    for x in 0..w {
        let u = x as i64 + cq;
        let i = (u / cq).min(f.gw as i64 - 2);
        cols.push((i as u32, ((u - i * cq) * 256 / cq) as i32));
    }
    let ch = |c: u32| [((c >> 16) & 255) as i32, ((c >> 8) & 255) as i32, (c & 255) as i32];
    let (v0, v1, v2) = (ch(p.void), ch(p.deep), ch(p.mid));
    let (cx, cy) = (w as i64 / 2, h as i64 / 2);
    let inv = (110i64 << 32) / (cx * cx + cy * cy).max(1);
    let gw = f.gw;
    for y in 0..h {
        let s = y as i64 + cq;
        let j = (s / cq).min(f.gh as i64 - 2) as usize;
        let fy = ((s - j as i64 * cq) * 256 / cq) as i32;
        let dy2 = (y as i64 - cy) * (y as i64 - cy);
        let (r0, r1) = (j * gw, (j + 1) * gw);
        let row = &mut px[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            let (i, fx) = cols[x];
            let i = i as usize;
            let bil = |v: &[i32]| {
                let t = v[r0 + i] * 256 + (v[r0 + i + 1] - v[r0 + i]) * fx;
                let b = v[r1 + i] * 256 + (v[r1 + i + 1] - v[r1 + i]) * fx;
                (t * 256 + (b - t) * fy) >> 16
            };
            let vv = ((3 * bil(&f.dens) + bil(&f.phi)) >> 2).clamp(0, 4095);
            let dx = x as i64 - cx;
            let vig = 256 - (((dx * dx + dy2) * inv) >> 32) as i32;
            let a = ((((vv * vv) >> 16) * vig) >> 8) * 150 >> 8;
            let hi = if vv > 3000 { ((vv - 3000) * a) >> 11 } else { 0 };
            let dth = BAYER[((y & 3) << 2) | (x & 3)] as i32;
            let q = |k: usize| ((((v0[k] << 8) + (v1[k] - v0[k]) * a + (v2[k] - v1[k]) * hi) + dth) >> 8).clamp(0, 255) as u32;
            *o = (q(0) << 16) | (q(1) << 8) | q(2);
        }
    }
    Some(())
}

/// 2. THE FLOW (Van Gogh strokes): streamlines of the curl from a jittered seed grid, integrated both ways by the
/// midpoint rule one px a step, tapered at the tips; luminance = the density at the seed. Every 7th step of a bright
/// stroke is a shimmer candidate `(x, y, phase along the stroke)`. Returns the strokes drawn.
fn sf_strokes(px: &mut [u32], w: usize, h: usize, f: &SfField, p: &SfPal, cand: &mut alloc::vec::Vec<(u16, u16, u16)>) -> u32 {
    let (ww, hh) = (w as i64, h as i64);
    let sp = (hh / 60).max(6);
    let (lmin, lvar) = ((hh * 18 / 1800).max(6), (hh * 90 / 1800).max(8));
    let mut n = 0u32;
    for gy in 0..hh / sp + 1 {
        for gx in 0..ww / sp + 1 {
            let r = sf_hash(gx, gy, 11);
            let (x0, y0) = (gx * sp + (r % sp as u64) as i64, gy * sp + ((r >> 16) % sp as u64) as i64);
            if x0 >= ww || y0 >= hh {
                continue;
            }
            let (xq, yq) = ((x0 << 8) + 128, (y0 << 8) + 128);
            let d0 = f.at(&f.dens, xq, yq).clamp(0, 4095);
            if d0 < 400 && (r >> 40) & 3 != 0 {
                continue; // the voids keep one faint stroke in four
            }
            let half = (lmin + ((r >> 24) % lvar as u64) as i64) / 2;
            let tone = (d0 + ((r >> 44) & 511) as i64 - 256).clamp(0, 4095) as u32;
            let col = if tone < 2048 { sf_mix(p.deep, p.mid, tone >> 3) } else { sf_mix(p.mid, p.hi, (tone - 2048) >> 3) };
            let ab = 24 + d0 * 200 / 4096;
            let wid = 1 + (d0 > 1800) as i64 + (d0 > 3200) as i64;
            let bright = ab > 120;
            n += 1;
            for dir in [1i64, -1] {
                let (mut x, mut y) = (xq, yq);
                for st in 0..half {
                    let (ux, uy) = (f.at(&f.ux, x, y), f.at(&f.uy, x, y));
                    let (mx, my) = (x + dir * ux / 32, y + dir * uy / 32);
                    let (vx, vy) = (f.at(&f.ux, mx, my), f.at(&f.uy, mx, my));
                    x += dir * vx / 16;
                    y += dir * vy / 16;
                    if x < 0 || y < 0 || x >= ww << 8 || y >= hh << 8 {
                        break;
                    }
                    let tip = st * 256 / half.max(1);
                    let a = (ab * (256 - ((tip * tip) >> 8)) >> 8) as u32;
                    sf_splat(px, w, h, x, y, col, a);
                    for k in 1..wid {
                        sf_splat(px, w, h, x - vy * k / 16, y + vx * k / 16, col, a * 3 / 4);
                    }
                    if bright && st % 7 == 3 && cand.len() < 60_000 && cand.try_reserve(1).is_ok() {
                        cand.push(((x >> 8) as u16, (y >> 8) as u16, ((st * dir * 1200) as u64).wrapping_add(r) as u16));
                    }
                }
            }
        }
    }
    n
}

/// A drawn star: its source `b` and image `t` (Q8), the lens gradient there, brightness, class, stretch.
#[derive(Clone, Copy)]
struct SfStar {
    bq: (i64, i64),
    tq: (i64, i64),
    g: (i64, i64),
    b: u32,
    class: u8,
    stretched: bool,
}

/// One lens probe the fixture checks: the source, the image (Q8 px) and the gradient there.
#[derive(Clone, Copy, Default)]
pub struct SfProbe {
    pub bq: (i64, i64),
    pub tq: (i64, i64),
    pub g: (i64, i64),
}

/// 3. THE STARS, LENSED (STARFIELD.md §5): source `b` by hash; `delta = -G Dmax / Gmax` (away from the mass); image
/// `t = b + delta`; stretched into an arc `1 + |G| Smax / Gmax` px along the tangent `(-Gy, Gx)`; magnified by
/// `1 + 0.75 |G| / Gmax`. Returns `(drawn, displaced by >= 1 px)`.
fn sf_stars(px: &mut [u32], w: usize, h: usize, f: &SfField, p: &SfPal, out: &mut alloc::vec::Vec<SfStar>) -> Option<(u32, u32)> {
    out.try_reserve_exact(sf_nstars(w, h)).ok()?;
    Some(sf_stars_core(w, h, &f.lens(), p, STAR_SEED, &mut |x, y, c, a| sf_plot(px, w, h, x, y, c, a), &mut |s| out.push(s)))
}

/// STARTSCENE (B662): the stars' seed — the picture's and the GOP stage's are the same stars.
pub const STAR_SEED: u64 = 21;

/// How many star sources a `w` x `h` panel draws (about 3000 at 2880x1800).
fn sf_nstars(w: usize, h: usize) -> usize {
    ((3000 * w as i64 * h as i64) / (2880 * 1800)).max(300) as usize
}

/// STARTSCENE (B662): the star pass itself, allocation-free — every pixel through `plot(x, y, colour, alpha of 256)`,
/// every drawn star to `each`. [`sf_stars`] (the picture) and [`stars_into`] / [`stars_onto`] (the GOP stage, the
/// fixture) call it with the same field and seed. Returns `(drawn, displaced by >= 1 px)`.
fn sf_stars_core(w: usize, h: usize, f: &SfLens, p: &SfPal, seed: u64, plot: &mut impl FnMut(i64, i64, u32, u32), each: &mut impl FnMut(SfStar)) -> (u32, u32) {
    let (ww, hh) = (w as i64, h as i64);
    let n = sf_nstars(w, h);
    let (dmax, smax) = ((hh / 100).max(2), (hh / 225).max(1));
    let (rh, sk) = ((hh * 4 / 1800).max(2), (hh * 10 / 1800).max(3));
    let (mut drawn, mut lensed) = (0u32, 0u32);
    for k in 0..n as i64 {
        let r = sf_hash(k, 7, seed);
        let bq = (((r % ww as u64) as i64) << 8 | 128, (((r >> 24) % hh as u64) as i64) << 8 | 128);
        let (gx, gy) = (f.at(&f.gx, bq.0, bq.1), f.at(&f.gy, bq.0, bq.1));
        let gm = sf_isqrt(gx * gx + gy * gy);
        let (dx, dy) = (-gx * dmax * 256 / f.gmax, -gy * dmax * 256 / f.gmax);
        let tq = (bq.0 + dx, bq.1 + dy);
        if tq.0 < 0 || tq.1 < 0 || tq.0 >= ww << 8 || tq.1 >= hh << 8 {
            continue;
        }
        if dx * dx + dy * dy >= 256 * 256 {
            lensed += 1;
        }
        let u = ((r >> 48) & 4095) as i64;
        let class: u8 = if u > 3972 { 2 } else if u > 3480 { 1 } else { 0 };
        let mag = (((u * u) >> 12) * u) >> 12;
        let mu = 256 + gm * 192 / f.gmax;
        let b = (((24 + mag * 200 / 4095) * mu) >> 8).min(256) as u32;
        let col = if class == 2 && (r >> 8) % 3 == 0 { p.warm } else { p.star };
        let l = 1 + gm * smax / f.gmax;
        let stretched = l >= 2;
        if stretched {
            let (tx, ty) = (-gy * 256 / gm.max(1), gx * 256 / gm.max(1));
            let steps = l * 2;
            for i in -steps..=steps {
                let e = 256 - i.abs() * 256 / (steps + 1);
                sf_splat_with(plot, tq.0 + tx * i / 2, tq.1 + ty * i / 2, col, ((b as i64 * e) >> 8) as u32 * 3 / 4);
            }
        } else {
            sf_splat_with(plot, tq.0, tq.1, col, b);
            if class >= 1 {
                for (ox, oy) in [(256i64, 0i64), (-256, 0), (0, 256), (0, -256)] {
                    sf_splat_with(plot, tq.0 + ox, tq.1 + oy, col, b / 3);
                }
            }
        }
        if class == 2 {
            let (cx, cy) = (tq.0 >> 8, tq.1 >> 8);
            for oy in -rh..=rh {
                for ox in -rh..=rh {
                    let d2 = ox * ox + oy * oy;
                    if d2 <= rh * rh {
                        let q = 256 - d2 * 256 / (rh * rh + 1);
                        plot(cx + ox, cy + oy, col, ((b as i64 * q * q) >> 16) as u32 / 3);
                    }
                }
            }
            for i in 1..=sk {
                let a = (b as i64 * (sk + 1 - i) / (sk + 1)) as u32 / 2;
                for (sx, sy) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                    plot(cx + sx * i, cy + sy * i, col, a);
                }
            }
        }
        drawn += 1;
        each(SfStar { bq, tq, g: (gx, gy), b, class, stretched });
    }
    (drawn, lensed)
}

/// The zircon as drawn: its silhouette's per-column top and bottom (`i32::MAX` / -1 where it is absent), its box,
/// the apex, and how many of its facets face the camera.
struct SfGem {
    top: alloc::vec::Vec<i32>,
    bot: alloc::vec::Vec<i32>,
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
    apex: (i64, i64),
    front: u32,
}

/// The zircon's facet count: 8 prism quads, 8 crown triangles, the base.
pub const ZIRCON_FACETS: u32 = 17;

/// 4. THE ZIRCON (STARFIELD.md §6): an octagonal prism under an octagonal pyramid and a base, tilted -15 degrees,
/// projected in perspective to 45 % of the panel's height at (0.78 W, base 0.965 H); front facets only (convex: they
/// tile the silhouette), each flat-shaded from the beam's light with a specular, a Fresnel rim and the crown's fire,
/// silhouette edges anti-aliased, interior edges a faint highlight line.
fn sf_zircon(px: &mut [u32], w: usize, h: usize, p: &SfPal) -> Option<SfGem> {
    const N: usize = 8;
    let (ww, hh) = (w as i64, h as i64);
    let (yb, yt, ya) = (-7168i64, 1229i64, 6349i64);
    let mut v = [[0i64; 3]; 17];
    for k in 0..N {
        let a = 20 + 45 * k as i64;
        let (c, s) = (sf_cos(a), sf_sin(a));
        v[k] = [c, yt, s];
        v[N + k] = [c, yb, s];
    }
    v[16] = [0, ya, 0];
    // Faces: vertex ring, vertex count, the neighbour face across each edge (edge e runs vertex e -> e + 1).
    let mut faces = [([0usize; 8], 0usize, [0usize; 8]); 17];
    for k in 0..N {
        let (k1, km) = ((k + 1) % N, (k + N - 1) % N);
        faces[k] = ([N + k, N + k1, k1, k, 0, 0, 0, 0], 4, [16, k1, 8 + k, km, 0, 0, 0, 0]);
        faces[8 + k] = ([k, k1, 16, 0, 0, 0, 0, 0], 3, [k, 8 + k1, 8 + km, 0, 0, 0, 0, 0]);
    }
    let (mut bv, mut bn) = ([0usize; 8], [0usize; 8]);
    for e in 0..N {
        let j = N - 1 - e; // edge B_j -> B_(j-1), shared with prism j-1
        bv[e] = N + j;
        bn[e] = (j + N - 1) % N;
    }
    faces[16] = (bv, 8, bn);
    // The tilt (top toward the camera), then the camera's depth.
    let (ct, st) = (sf_cos(-15), sf_sin(-15));
    let tilt = |q: [i64; 3]| [q[0], (q[1] * ct - q[2] * st) >> 12, (q[1] * st + q[2] * ct) >> 12];
    let zc = 7 * 4096;
    let mut cv = [[0i64; 3]; 17];
    for k in 0..17 {
        let t = tilt(v[k]);
        cv[k] = [t[0], t[1], t[2] + zc];
    }
    let centre = [0i64, (yb + ya) / 2, 0];
    let norm = |q: [i64; 3]| {
        let m = sf_isqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).max(1);
        [q[0] * 4096 / m, q[1] * 4096 / m, q[2] * 4096 / m]
    };
    let dot = |a: [i64; 3], b: [i64; 3]| (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) >> 12;
    let mut nrm = [[0i64; 3]; 17];
    let mut cen = [[0i64; 3]; 17];
    let mut front = [false; 17];
    for (fi, f) in faces.iter().enumerate() {
        let (a, b, c) = (v[f.0[0]], v[f.0[1]], v[f.0[2]]);
        let (e1, e2) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let mut n = norm([(e1[1] * e2[2] - e1[2] * e2[1]) >> 12, (e1[2] * e2[0] - e1[0] * e2[2]) >> 12, (e1[0] * e2[1] - e1[1] * e2[0]) >> 12]);
        let mut m = [0i64; 3];
        let mut mc = [0i64; 3];
        for &vi in f.0[..f.1].iter() {
            for d in 0..3 {
                m[d] += v[vi][d];
                mc[d] += cv[vi][d];
            }
        }
        let k = f.1 as i64;
        let (m, mc) = ([m[0] / k, m[1] / k, m[2] / k], [mc[0] / k, mc[1] / k, mc[2] / k]);
        if dot(n, [m[0] - centre[0], m[1] - centre[1], m[2] - centre[2]]) < 0 {
            n = [-n[0], -n[1], -n[2]];
        }
        let n = tilt(n);
        nrm[fi] = n;
        cen[fi] = mc;
        front[fi] = dot(n, mc) < 0;
    }
    // Projection, then one scale and shift: 45 % of the panel's height, base at 0.965 H, centre at 0.78 W.
    let mut sn = [(0i64, 0i64); 17];
    let (mut nx0, mut nx1, mut ny0, mut ny1) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
    for k in 0..17 {
        let z = cv[k][2].max(1);
        sn[k] = (cv[k][0] * 65536 / z, -cv[k][1] * 65536 / z);
        nx0 = nx0.min(sn[k].0);
        nx1 = nx1.max(sn[k].0);
        ny0 = ny0.min(sn[k].1);
        ny1 = ny1.max(sn[k].1);
    }
    let span = (ny1 - ny0).max(1);
    let hg = hh * 45 / 100;
    let (ax, by) = ((ww * 78 / 100) << 8, (hh * 965 / 1000) << 8);
    let midx = (nx0 + nx1) / 2;
    let mut sp = [(0i64, 0i64); 17];
    for k in 0..17 {
        sp[k] = ((sn[k].0 - midx) * hg * 256 / span + ax, (sn[k].1 - ny1) * hg * 256 / span + by);
    }
    let (gx0, gx1) = (sp.iter().map(|q| q.0).min().unwrap_or(0) >> 8, sp.iter().map(|q| q.0).max().unwrap_or(0) >> 8);
    let (gy0, gy1) = (sp.iter().map(|q| q.1).min().unwrap_or(0) >> 8, sp.iter().map(|q| q.1).max().unwrap_or(0) >> 8);
    let gh_px = (gy1 - gy0).max(1);
    let (mut top, mut bot) = (sf_vec(w)?, sf_vec(w)?);
    top.fill(i32::MAX);
    bot.fill(-1);
    // The light: the beam's, from above, a little left and toward the camera.
    let lgt = norm([-901, 3809, -1229]);
    let mut nfront = 0u32;
    for (fi, f) in faces.iter().enumerate() {
        if !front[fi] {
            continue;
        }
        nfront += 1;
        let n = nrm[fi];
        let vv = norm([-cen[fi][0], -cen[fi][1], -cen[fi][2]]);
        let ndl = dot(n, lgt).max(0);
        let ndv = dot(n, vv).max(0);
        let fr = ((4096 - ndv) * (4096 - ndv) >> 12) * (4096 - ndv) >> 12;
        let hv = norm([lgt[0] + vv[0], lgt[1] + vv[1], lgt[2] + vv[2]]);
        let mut spc = dot(n, hv).max(0);
        for _ in 0..5 {
            spc = (spc * spc) >> 12;
        }
        let mut c = sf_mix(p.zd, p.zm, (ndl * 256 >> 12) as u32);
        c = sf_mix(c, p.zl, ((spc * 220) >> 12) as u32);
        c = sf_mix(c, p.zl, ((fr * 150) >> 12) as u32);
        if (8..16).contains(&fi) && ndl > 2400 {
            c = sf_mix(c, if fi % 2 == 0 { p.fw } else { p.fc }, 40);
        }
        let alpha = 210 + ((fr * 46) >> 12);
        // The face's edges: Q8 endpoints, the inside sign, length, silhouette-ness.
        let nv = f.1;
        let mut area = 0i64;
        for e in 0..nv {
            let (a, b) = (sp[f.0[e]], sp[f.0[(e + 1) % nv]]);
            area += a.0 * b.1 - b.0 * a.1;
        }
        let sg = if area >= 0 { 1i64 } else { -1 };
        let mut ed = [(0i64, 0i64, 0i64, 0i64, 0i64, false); 8];
        let (mut fx0, mut fx1, mut fy0, mut fy1) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
        for e in 0..nv {
            let (a, b) = (sp[f.0[e]], sp[f.0[(e + 1) % nv]]);
            let len = sf_isqrt((b.0 - a.0) * (b.0 - a.0) + (b.1 - a.1) * (b.1 - a.1)).max(1);
            ed[e] = (a.0, a.1, b.0 - a.0, b.1 - a.1, len, !front[f.2[e]]);
            fx0 = fx0.min(a.0);
            fx1 = fx1.max(a.0);
            fy0 = fy0.min(a.1);
            fy1 = fy1.max(a.1);
        }
        let (bx0, bx1) = (((fx0 >> 8) - 1).clamp(0, ww), ((fx1 >> 8) + 2).clamp(0, ww));
        let (by0, by1) = (((fy0 >> 8) - 1).clamp(0, hh), ((fy1 >> 8) + 2).clamp(0, hh));
        let mut d = [0i64; 8];
        let mut stp = [0i64; 8];
        for e in 0..nv {
            stp[e] = -sg * ed[e].3 * 65536 / ed[e].4;
        }
        for y in by0..by1 {
            let pyc = (y << 8) + 128;
            let pxc = (bx0 << 8) + 128;
            for e in 0..nv {
                let (x0, y0, ex, ey, len, _) = ed[e];
                d[e] = sg * (ex * (pyc - y0) - ey * (pxc - x0)) * 256 / len;
            }
            let shade = ((y - gy0) * 72 / gh_px).clamp(0, 72) as u32;
            let cy = sf_mix(c, p.zd, shade);
            for x in bx0..bx1 {
                let (mut cov, mut inside, mut ein, mut esil) = (256i64, true, i64::MAX, i64::MAX);
                for e in 0..nv {
                    let dq = d[e] >> 8;
                    if ed[e].5 {
                        cov = cov.min((dq + 128).clamp(0, 256));
                        esil = esil.min(dq);
                    } else {
                        if dq < 0 {
                            inside = false;
                        }
                        ein = ein.min(dq);
                    }
                    d[e] += stp[e];
                }
                if !inside || cov <= 0 {
                    continue;
                }
                let mut cc = cy;
                if ein < 154 {
                    cc = sf_mix(cc, p.zl, ((154 - ein) * 50 / 154) as u32);
                }
                if esil < 384 {
                    cc = sf_mix(cc, p.zl, ((384 - esil.max(0)) * 70 / 384) as u32);
                }
                let o = y as usize * w + x as usize;
                px[o] = sf_mix(px[o], cc, ((cov * alpha) >> 8) as u32);
                let xi = x as usize;
                top[xi] = top[xi].min(y as i32);
                bot[xi] = bot[xi].max(y as i32);
            }
        }
    }
    Some(SfGem { top, bot, x0: gx0, y0: gy0, x1: gx1, y1: gy1, apex: (sp[16].0 >> 8, sp[16].1 >> 8), front: nfront })
}

/// The beam as drawn: its column, the landing row, its glow half-width and the landing glow's radius.
#[derive(Clone, Copy)]
struct SfBeam {
    x: i64,
    land: i64,
    gw: i64,
    rg: i64,
}

/// 5. THE BEAM (STARFIELD.md §7): a column from the panel's top onto the crown's upper-left facet, brightening toward
/// the landing; dust along it; the landing glow, its hot core and a horizontal flare; the three-ray fan inside.
fn sf_beam(px: &mut [u32], w: usize, h: usize, p: &SfPal, g: &SfGem) -> SfBeam {
    let (ww, hh) = (w as i64, h as i64);
    let bx = (g.apex.0 - (g.x1 - g.x0) * 16 / 100).clamp(1, ww - 2);
    let t = g.top[bx as usize];
    let land = (if t == i32::MAX { g.apex.1 } else { t as i64 }).clamp(1, hh - 1);
    let (cw, gwd, rg, rc) = ((hh / 600).max(1), (hh / 45).max(3), (hh / 13).max(6), (hh / 200).max(2));
    for y in 0..land {
        let fade = 96 + 160 * y / land;
        for dx in -gwd..=gwd {
            let x = bx + dx;
            if x < 0 || x >= ww {
                continue;
            }
            let o = y as usize * w + x as usize;
            let ad = dx.abs();
            let ag = (((gwd - ad) * (gwd - ad) * 90 / (gwd * gwd)) * fade) >> 8;
            let mut c = sf_mix(px[o], p.glow, ag as u32);
            if ad <= cw {
                let ac = ((256 - ad * ad * 256 / ((cw + 1) * (cw + 1))) * fade) >> 8;
                c = sf_mix(c, p.core, ac as u32);
            }
            px[o] = c;
        }
        let r = sf_hash(y, bx, 31);
        if r % 23 == 0 {
            let x = bx + ((r >> 8) % (2 * gwd as u64 + 1)) as i64 - gwd;
            sf_plot(px, w, h, x, y, p.glow, 60 + ((r >> 20) % 80) as u32);
        }
    }
    for oy in -rg..=rg {
        for ox in -rg..=rg {
            let d2 = ox * ox + oy * oy;
            if d2 > rg * rg {
                continue;
            }
            let d = sf_isqrt(d2);
            let q = rg - d;
            sf_plot(px, w, h, bx + ox, land + oy, p.glow, (q * q * q * 200 / (rg * rg * rg)) as u32);
            if d <= rc {
                sf_plot(px, w, h, bx + ox, land + oy, p.core, ((rc - d + 1) * 230 / (rc + 1)) as u32);
            }
        }
    }
    let fl = rg * 26 / 10;
    for oy in -1i64..=1 {
        for ox in -fl..=fl {
            let q = fl - ox.abs();
            let a = (q * q * 120 / (fl * fl) * if oy == 0 { 256 } else { 110 }) >> 8;
            sf_plot(px, w, h, bx + ox, land + oy, p.core, a as u32);
        }
    }
    let len = ((g.y1 - land) * 6 / 10).max(1);
    for (deg, col) in [(64i64, p.fw), (68, p.zl), (72, p.fc)] {
        let (dxq, dyq) = (sf_cos(deg), sf_sin(deg));
        for t in 0..len {
            let (x, y) = (bx + dxq * t / 4096, land + dyq * t / 4096);
            if x < 0 || x >= ww {
                continue;
            }
            let (tp, bt) = (g.top[x as usize], g.bot[x as usize]);
            if tp == i32::MAX || y < tp as i64 || y > bt as i64 {
                continue;
            }
            let a = (90 * (len - t) / len) as u32;
            sf_plot(px, w, h, x, y, col, a);
            sf_plot(px, w, h, x + 1, y, col, a / 2);
        }
    }
    SfBeam { x: bx, land, gw: gwd, rg }
}

/// One shimmering pixel: `out = mix(rest, tint, amp * smooth(triangle(t * rate * 8 + phase)) / 256)` — at the wave's
/// zero it is exactly the generated picture (STARFIELD.md §8).
#[derive(Clone, Copy)]
pub struct Glint {
    pub x: u16,
    pub y: u16,
    pub rest: u32,
    pub tint: u32,
    pub phase: u16,
    pub rate: u8,
    pub amp: u8,
    grp: u32,
}

/// A run of glints re-tinted together, and the box it repaints (`x1`, `y1` exclusive).
#[derive(Clone, Copy)]
pub struct Group {
    pub start: u32,
    pub len: u32,
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
}

/// The picture's shimmer: its glints sorted by group, the groups (the beam's first), the coprime stride the star
/// cells are visited in, and the picture's size.
pub struct Shimmer {
    pub glints: alloc::vec::Vec<Glint>,
    pub groups: alloc::vec::Vec<Group>,
    pub beam_groups: usize,
    pub stride: usize,
    pub w: usize,
    pub h: usize,
}

/// What one generation drew, for the witness lines and `tests starfield`.
pub struct Scene {
    pub shimmer: Shimmer,
    pub stars: u32,
    pub lensed: u32,
    pub streams: u32,
    pub front: u32,
    /// The zircon's box `(x, y, w, h)` and the beam's `(column, landing row, glow half-width)`.
    pub gem: (usize, usize, usize, usize),
    pub beam: (usize, usize, usize),
    pub probes: [SfProbe; 3],
    pub nprobes: usize,
    /// The lens's largest displacement, px (`H/100`).
    pub dmax: i64,
    /// STARTSCENE (B662): up to 16 star pixels (bright, compact, evenly spaced over the drawn stars; each the pixel its
    /// splat weighs most) — the fixture checks the GOP stage lit the same ones.
    pub sample: [(u16, u16); 16],
    pub nsample: usize,
}

/// The shimmer's caps (STARFIELD.md §8): all glints, the beam's, the twinkles', the lensed-drift entries (pairs).
pub const SHIMMER_MAX: usize = 6000;
const SF_CAP_BEAM: usize = 1600;
const SF_CAP_TWINKLE: usize = 2000;
const SF_CAP_DRIFT: usize = 800;
/// The beam's groups: six column segments and the landing.
const SF_BEAM_SEGS: u32 = 6;
const SF_BEAM_GROUPS: u32 = SF_BEAM_SEGS + 1;

/// Up to `cap` items of `v`, evenly spaced (so a cap never favours the top of the panel).
fn sf_thin<T: Copy>(v: &[T], cap: usize, mut each: impl FnMut(T)) {
    let n = v.len();
    if n == 0 || cap == 0 {
        return;
    }
    let take = n.min(cap);
    for i in 0..take {
        each(v[i * n / take]);
    }
}

/// 6. THE SHIMMER LIST, from the FINAL picture: the beam's centre line and landing; the stars (a lensed compact star
/// with `|G| > Gmax/4` is a drift PAIR, every other star a twinkle toward the ground 3 px away); the streams'
/// travelling glints. No star or stream entry on or beside the foreground. Sorted into groups: the beam's, then cells.
#[allow(clippy::too_many_arguments)]
fn sf_shimmer(px: &[u32], w: usize, h: usize, p: &SfPal, g: &SfGem, b: &SfBeam, stars: &[SfStar], cand: &[(u16, u16, u16)], gmax: i64) -> Option<Shimmer> {
    let (ww, hh) = (w as i64, h as i64);
    let fg = |x: i64, y: i64| -> bool {
        if x < 0 || y < 0 || x >= ww || y >= hh {
            return true;
        }
        if x >= g.x0 - 6 && x <= g.x1 + 6 {
            for xx in [x - 3, x, x + 3] {
                if xx >= 0 && xx < ww {
                    let (t, bt) = (g.top[xx as usize], g.bot[xx as usize]);
                    if t != i32::MAX && y >= t as i64 - 3 && y <= bt as i64 + 3 {
                        return true;
                    }
                }
            }
        }
        let (dx, dy) = (x - b.x, y - b.land);
        (dx.abs() <= b.gw + 3 && y <= b.land) || dx * dx + dy * dy <= (b.rg + 3) * (b.rg + 3) || (dy.abs() <= 3 && dx.abs() <= b.rg * 26 / 10 + 3)
    };
    let cs = (hh / 19).max(32);
    let ncx = (ww + cs - 1) / cs;
    let cell = |x: i64, y: i64| SF_BEAM_GROUPS + ((y / cs) * ncx + x / cs) as u32;
    let at = |x: i64, y: i64| px[y as usize * w + x as usize];
    let mut gl: alloc::vec::Vec<Glint> = alloc::vec::Vec::new();
    gl.try_reserve_exact(SHIMMER_MAX + 2).ok()?;
    let push = |gl: &mut alloc::vec::Vec<Glint>, x: i64, y: i64, tint: u32, phase: u16, rate: u8, amp: u8, grp: u32| {
        if gl.len() < SHIMMER_MAX && x >= 0 && y >= 0 && x < ww && y < hh {
            gl.push(Glint { x: x as u16, y: y as u16, rest: at(x, y), tint, phase, rate, amp, grp });
        }
    };
    // The beam: its centre line (a pulse runs down it into the stone) and its landing.
    let step = if b.land as usize > SF_CAP_BEAM - 300 { 2 } else { 1 };
    let kq = 65536 * 9 / (2 * hh).max(1);
    let mut y = 0i64;
    while y < b.land {
        let seg = ((y * SF_BEAM_SEGS as i64) / b.land.max(1)).min(SF_BEAM_SEGS as i64 - 1) as u32;
        push(&mut gl, b.x, y, p.core, (65536 - (y * kq) % 65536) as u16, 2, 100, seg);
        y += step;
    }
    let rh = hh / 200 + 4;
    for oy in -rh..=rh {
        for ox in -rh..=rh {
            if ox * ox + oy * oy <= rh * rh && (ox + oy) & 1 == 0 {
                push(&mut gl, b.x + ox, b.land + oy, p.core, 0, 2, 120, SF_BEAM_SEGS);
            }
        }
    }
    // The stars: drift pairs (lensed, compact), then twinkles.
    let mut drift = 0usize;
    let mut tw: alloc::vec::Vec<(i64, i64, i64, i64, u64)> = alloc::vec::Vec::new();
    tw.try_reserve_exact(stars.len()).ok()?;
    for (k, s) in stars.iter().enumerate() {
        let (x, y) = (s.tq.0 >> 8, s.tq.1 >> 8);
        let r = sf_hash(k as i64, 3, 41);
        let gm = sf_isqrt(s.g.0 * s.g.0 + s.g.1 * s.g.1);
        if !s.stretched && s.class < 2 && gm * 4 > gmax && drift + 2 <= SF_CAP_DRIFT {
            let (qx, qy) = if s.g.0.abs() >= s.g.1.abs() { (x - s.g.0.signum(), y) } else { (x, y - s.g.1.signum()) };
            if !fg(x, y) && !fg(qx, qy) && qx >= 0 && qy >= 0 && qx < ww && qy < hh {
                let (rp, rq) = (at(x, y), at(qx, qy));
                let grp = cell(x, y);
                let ph = r as u16;
                push(&mut gl, x, y, rq, ph, 1, 128, grp);
                push(&mut gl, qx, qy, rp, ph, 1, 128, grp);
                drift += 2;
                continue;
            }
        }
        let (bx, by) = if x + 3 < ww { (x + 3, y) } else { (x - 3, y) };
        if !fg(x, y) && !fg(bx, by) {
            tw.push((x, y, bx, by, r));
        }
    }
    sf_thin(&tw, SF_CAP_TWINKLE, |(x, y, bx, by, r)| {
        let tint = at(bx, by);
        push(&mut gl, x, y, tint, r as u16, 2 + ((r >> 16) % 5) as u8, 140 + ((r >> 24) % 61) as u8, cell(x, y));
    });
    // The streams: the rest of the budget, evenly over the candidates clear of the foreground.
    let room = SHIMMER_MAX.saturating_sub(gl.len());
    let mut sc: alloc::vec::Vec<(u16, u16, u16)> = alloc::vec::Vec::new();
    sc.try_reserve_exact(cand.len()).ok()?;
    sc.extend(cand.iter().copied().filter(|&(x, y, _)| !fg(x as i64, y as i64)));
    sf_thin(&sc, room, |(x, y, ph)| {
        let (x, y) = (x as i64, y as i64);
        push(&mut gl, x, y, p.hi, ph, 3, 140, cell(x, y));
    });
    // Sort by group, then position; one entry per pixel within a group.
    gl.sort_unstable_by_key(|e| ((e.grp as u64) << 32) | ((e.y as u64) << 16) | e.x as u64);
    gl.dedup_by_key(|e| (e.grp, e.x, e.y));
    let mut groups: alloc::vec::Vec<Group> = alloc::vec::Vec::new();
    let mut beam_groups = 0usize;
    let mut i = 0usize;
    while i < gl.len() {
        let gid = gl[i].grp;
        let start = i;
        let (mut x0, mut y0, mut x1, mut y1) = (u16::MAX, u16::MAX, 0u16, 0u16);
        while i < gl.len() && gl[i].grp == gid {
            x0 = x0.min(gl[i].x);
            y0 = y0.min(gl[i].y);
            x1 = x1.max(gl[i].x + 1);
            y1 = y1.max(gl[i].y + 1);
            i += 1;
        }
        if groups.try_reserve(1).is_err() {
            return None;
        }
        groups.push(Group { start: start as u32, len: (i - start) as u32, x0, y0, x1, y1 });
        if gid < SF_BEAM_GROUPS {
            beam_groups += 1;
        }
    }
    let ncells = groups.len() - beam_groups;
    let mut stride = (ncells * 618 / 1000).max(1);
    let gcd = |mut a: usize, mut b: usize| {
        while b != 0 {
            let t = a % b;
            a = b;
            b = t;
        }
        a
    };
    while ncells > 1 && gcd(stride, ncells) != 1 {
        stride += 1;
    }
    Some(Shimmer { glints: gl, groups, beam_groups, stride, w, h })
}

/// **The default wallpaper** into `px` (`w` x `h`): STARFIELD (B661, R128) — the ultra-black ground and the
/// dark-matter haze, the flow's Van Gogh strokes, the lensed stars, the zircon in the foreground and the beam striking
/// it, every colour a `theme::starfield` pair; and the shimmer list built from the finished picture. Integer only.
/// `None` when `w`/`h` are out of range or memory refused a table (the picture may then be partly drawn; the caller
/// installs it without a shimmer).
pub fn wall_gen(px: &mut [u32], w: usize, h: usize) -> Option<Scene> {
    wall_gen_look(px, w, h, theme::is_dark())
}

/// [`wall_gen`] in a named look's set (`dark`), whatever the live one is (WINEDGE2 B668's `shadow_dark=` probe).
fn wall_gen_look(px: &mut [u32], w: usize, h: usize, dark: bool) -> Option<Scene> {
    if w < 64 || h < 64 || w > 16_384 || h > 16_384 || px.len() < w * h {
        return None;
    }
    let p = SfPal::of(dark);
    let f = SfField::build(w, h)?;
    sf_haze(px, w, h, &f, &p)?;
    let mut cand: alloc::vec::Vec<(u16, u16, u16)> = alloc::vec::Vec::new();
    let streams = sf_strokes(px, w, h, &f, &p, &mut cand);
    let mut stars: alloc::vec::Vec<SfStar> = alloc::vec::Vec::new();
    let (nstars, lensed) = sf_stars(px, w, h, &f, &p, &mut stars)?;
    let g = sf_zircon(px, w, h, &p)?;
    let b = sf_beam(px, w, h, &p, &g);
    let shimmer = sf_shimmer(px, w, h, &p, &g, &b, &stars, &cand, f.gmax)?;
    // The lens probes: the three largest displacements among the bright compact stars clear of the foreground.
    let (ww, hh) = (w as i64, h as i64);
    let mut probes = [SfProbe::default(); 3];
    let mut best = [-1i64; 3];
    for s in stars.iter() {
        let (x, y) = (s.tq.0 >> 8, s.tq.1 >> 8);
        if s.stretched || s.b < 100 || x < 8 || y < 8 || x >= ww - 8 || y >= hh - 8 {
            continue;
        }
        if (x - b.x).abs() <= b.gw + 8 || (x >= g.x0 - 8 && x <= g.x1 + 8 && y >= g.y0 - 8) || ((x - b.x).pow(2) + (y - b.land).pow(2)) <= (b.rg + 8).pow(2) {
            continue;
        }
        let (dx, dy) = (s.tq.0 - s.bq.0, s.tq.1 - s.bq.1);
        let m = dx * dx + dy * dy;
        let mut k = 3usize;
        while k > 0 && m > best[k - 1] {
            k -= 1;
        }
        if k < 3 {
            for j in (k + 1..3).rev() {
                best[j] = best[j - 1];
                probes[j] = probes[j - 1];
            }
            best[k] = m;
            probes[k] = SfProbe { bq: s.bq, tq: s.tq, g: s.g };
        }
    }
    let nprobes = best.iter().filter(|&&m| m >= 0).count();
    // STARTSCENE (B662): the star sample the GOP stage must reproduce.
    let elig = |s: &&SfStar| s.class >= 1 && !s.stretched && s.b >= 100;
    let step = (stars.iter().filter(elig).count() / 16).max(1);
    let (mut sample, mut nsample) = ([(0u16, 0u16); 16], 0usize);
    for s in stars.iter().filter(elig).step_by(step) {
        if nsample < 16 {
            sample[nsample] = ((((s.tq.0 + 128) >> 8).clamp(0, ww - 1)) as u16, (((s.tq.1 + 128) >> 8).clamp(0, hh - 1)) as u16);
            nsample += 1;
        }
    }
    Some(Scene {
        shimmer,
        stars: nstars,
        lensed,
        streams,
        front: g.front,
        gem: (g.x0.max(0) as usize, g.y0.max(0) as usize, (g.x1 - g.x0).max(0) as usize, (g.y1 - g.y0).max(0) as usize),
        beam: (b.x as usize, b.land as usize, b.gw as usize),
        probes,
        nprobes,
        dmax: (hh / 100).max(2),
        sample,
        nsample,
    })
}

// ── STARTSCENE (rmbp-ledger B662, R128) — the scene's GROUND before the heap ──────────────────────────────────────
//
// The boot splash's GOP stage (`splash::gop_stage`, pre-heap, allocation-free) cannot run the generator, so it paints
// the scene's ground only: `VOID` and the SAME lensed stars (the field computed into this static scratch by the
// picture's own `sf_field_fill`, the stars by its own `sf_stars_core`, the same seed). The web, the gem and the beam
// arrive with the hold's full paint (`splash::hold_prepare` -> `wallpaper::generate_splash`): a fade-in, not a cut.
// Design record: `docs/dev/OS/05_USER_EXPERIENCE/STARFIELD.md` §9.

/// The static field scratch's capacity in nodes (2880x1800 needs 119 x 76 = 9044; 3840x2160 needs 10032).
const SF_SCRATCH_NODES: usize = 12_288;
/// Five tables (density, potential, scratch, the gradient's two components) — no heap.
static SF_SCRATCH: crate::sync::Mutex<[i32; 5 * SF_SCRATCH_NODES]> = crate::sync::Mutex::new([0; 5 * SF_SCRATCH_NODES]);
/// [`stars_onto`]'s read cache over a write-only target: `(pixel index << 32) | value written`, direct-mapped.
static SF_ONTO: crate::sync::Mutex<[u64; 4096]> = crate::sync::Mutex::new([0; 4096]);

/// The scene's ground colour in the live set (`theme::starfield::VOID`).
pub fn scene_void() -> u32 {
    SfPal::of(crate::video::theme::is_dark()).void
}

/// The star pass over the static scratch field. `None` off range or when the grid outgrows the scratch.
fn sf_stars_static(w: usize, h: usize, seed: u64, plot: &mut impl FnMut(i64, i64, u32, u32)) -> Option<u32> {
    if w < 64 || h < 64 || w > 16_384 || h > 16_384 {
        return None;
    }
    let dims = sf_field_dims(w, h);
    let n = dims.1 * dims.2;
    if n > SF_SCRATCH_NODES {
        return None;
    }
    let mut g = SF_SCRATCH.lock();
    let (dens, rest) = g.split_at_mut(SF_SCRATCH_NODES);
    let (phi, rest) = rest.split_at_mut(SF_SCRATCH_NODES);
    let (tmp, rest) = rest.split_at_mut(SF_SCRATCH_NODES);
    let (gx, gy) = rest.split_at_mut(SF_SCRATCH_NODES);
    let (gx, gy) = (&mut gx[..n], &mut gy[..n]);
    let gmax = sf_field_fill(w, h, dims, &mut dens[..n], &mut phi[..n], &mut tmp[..n], None, gx, gy);
    let lens = SfLens { c: dims.0, gw: dims.1, gh: dims.2, gx, gy, gmax };
    let (drawn, _) = sf_stars_core(w, h, &lens, &SfPal::of(crate::video::theme::is_dark()), seed, plot, &mut |_| {});
    Some(drawn)
}

/// STARTSCENE (B662): **the stars alone**, allocation-free, blended into `px` (`w` x `h`) exactly where the generated
/// picture draws them (`seed` = [`STAR_SEED`] for the picture's stars). Returns the stars drawn.
pub fn stars_into(px: &mut [u32], w: usize, h: usize, seed: u64) -> Option<u32> {
    if px.len() < w * h {
        return None;
    }
    sf_stars_static(w, h, seed, &mut |x, y, c, a| sf_plot(px, w, h, x, y, c, a))
}

/// STARTSCENE (B662): [`stars_into`] onto a WRITE-ONLY target (the GOP framebuffer: VRAM is never read back — see
/// `FrameBuffer::get_pixel`) already filled with [`scene_void`]: each pixel's last written value is kept in a small
/// direct-mapped cache so overlapping splats accumulate as they do in the picture (an evicted pixel blends over
/// `VOID` again). `put(x, y, colour)` writes; it is only called in bounds. Returns the stars drawn.
pub fn stars_onto(w: usize, h: usize, mut put: impl FnMut(usize, usize, u32)) -> Option<u32> {
    let void = scene_void();
    let mut cache = SF_ONTO.lock();
    cache.fill(u64::MAX);
    sf_stars_static(w, h, STAR_SEED, &mut |x, y, col, a| {
        if a == 0 || x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            return;
        }
        let idx = (y as u64) * w as u64 + x as u64;
        let slot = (idx.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 52) as usize;
        let e = cache[slot];
        let v = sf_mix(if e >> 32 == idx { e as u32 } else { void }, col, a);
        cache[slot] = (idx << 32) | v as u64;
        put(x as usize, y as usize, v);
    })
}

// ── THE SHIMMER, per frame ──────────────────────────────────────────────────────────────────────────────────────

/// Star cells re-tinted per tick (the beam's seven groups are re-tinted every tick).
pub const SHIMMER_CELLS: usize = 4;
/// The shimmer's floor between ticks (30 Hz at most).
pub const SHIMMER_MS: u64 = 33;

/// `smooth(triangle(th))`: 0 at 0, 255 at half a turn, 0 again at a full turn.
#[inline]
fn sf_wave(th: u16) -> u32 {
    let p = th as u32;
    let tri = if p < 32768 { p } else { 65535 - p };
    let x = tri >> 7;
    (x * x * (768 - 2 * x)) >> 16
}

/// Re-tint group `g` at time `t` through `put(x, y, colour)`; returns the glints written.
fn sf_retint(s: &Shimmer, g: &Group, t: u32, put: &mut impl FnMut(usize, usize, u32)) -> u32 {
    let (a, b) = (g.start as usize, (g.start + g.len) as usize);
    for e in s.glints[a.min(s.glints.len())..b.min(s.glints.len())].iter() {
        let th = t.wrapping_mul(e.rate as u32 * 8).wrapping_add(e.phase as u32) as u16;
        let k = (e.amp as u32 * sf_wave(th)) >> 8;
        put(e.x as usize, e.y as usize, sf_mix(e.rest, e.tint, k));
    }
    g.len
}

/// **One shimmer tick** (STARFIELD.md §8): every beam group, then the next [`SHIMMER_CELLS`] star cells in the
/// coprime-stride order from `cursor`; a group whose box meets an occluder box `(x, y, w, h)` (a visible window and
/// its shadow extent) is skipped — the shimmer stops where a window covers it — and every other is re-tinted through
/// `put` and its box handed to `queue` (the live seat queues it on the compositor's erase route). Returns
/// `(next cursor, glints written, boxes queued, px in the boxes)`. Pure: the fixture runs it on an offscreen picture.
pub fn shimmer_tick(s: &Shimmer, now_ms: u64, occ: &[(usize, usize, usize, usize)], cursor: u32, mut put: impl FnMut(usize, usize, u32), mut queue: impl FnMut(usize, usize, usize, usize)) -> (u32, u32, u32, u64) {
    let t = now_ms as u32;
    let hit = |g: &Group| occ.iter().any(|&(x, y, w, h)| (g.x0 as usize) < x + w && x < g.x1 as usize && (g.y0 as usize) < y + h && y < g.y1 as usize);
    let (mut n, mut nb, mut area) = (0u32, 0u32, 0u64);
    let mut run = |g: &Group, n: &mut u32, nb: &mut u32, area: &mut u64| {
        if g.len == 0 || g.x1 <= g.x0 || g.y1 <= g.y0 || hit(g) {
            return;
        }
        *n += sf_retint(s, g, t, &mut put);
        let (bw, bh) = ((g.x1 - g.x0) as usize, (g.y1 - g.y0) as usize);
        queue(g.x0 as usize, g.y0 as usize, bw, bh);
        *nb += 1;
        *area += (bw * bh) as u64;
    };
    for g in s.groups[..s.beam_groups.min(s.groups.len())].iter() {
        run(g, &mut n, &mut nb, &mut area);
    }
    let ncells = s.groups.len().saturating_sub(s.beam_groups);
    let mut cur = cursor;
    if ncells > 0 {
        for _ in 0..SHIMMER_CELLS.min(ncells) {
            let i = s.beam_groups + ((cur as usize % ncells) * s.stride) % ncells;
            cur = cur.wrapping_add(1);
            run(&s.groups[i], &mut n, &mut nb, &mut area);
        }
    }
    (cur, n, nb, area)
}

/// The visible windows' occluder boxes [`track`] gathers for the shimmer (more than this and the shimmer skips the pass).
const SHIM_OCC: usize = 32;
/// The last tick (ms), the star-cell cursor, ticks run, and a tick in flight (passes may run on several cores).
static SHIM_LAST: AtomicU64 = AtomicU64::new(0);
static SHIM_CURSOR: AtomicU32 = AtomicU32::new(0);
static SHIM_TICKS: AtomicU32 = AtomicU32::new(0);
static SHIM_BUSY: AtomicBool = AtomicBool::new(false);
/// SHIMMERIDLE (B663): the last tick's cost (us, re-tint + queue) and the boxes it queued.
#[cfg(feature = "facet")]
static SHIM_TICK_US: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "facet")]
static SHIM_TICK_NB: AtomicU32 = AtomicU32::new(0);
/// SHIMMERIDLE2 (B830): the last tick's glints written and px in its boxes (the `[shimmer] tick` line).
#[cfg(feature = "facet")]
static SHIM_TICK_GL: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "facet")]
static SHIM_TICK_PX: AtomicU64 = AtomicU64::new(0);

/// The live seat (from [`track`], at the head of the composite pass, before the erase drain): at most once per
/// [`SHIMMER_MS`], only when the erase queue is empty and the pass queued no shadow fill, only while the desktop scene
/// owns the backdrop and the installed picture is the generated one at the panel's size. The re-tinted pixels are
/// written into the installed picture (`wallpaper::shimmer_with`) and each box goes on `defer_erase`, so the SAME
/// pass's drain paints it through `stage_fill` (the wallpaper, every window shadow, the window and furniture clip, the
/// cursor bracket).
#[cfg(feature = "facet")]
fn shimmer_live(occ: &[(usize, usize, usize, usize)], pw: usize, ph: usize) {
    if super::DEFER_N.load(Ordering::Relaxed) != 0 || !crate::video::wallpaper::owns_backdrop() { // STARTSCENE (B662): the scene owns the backdrop from the splash on (x86 never sets the REALDESK latch)
        return;
    }
    let now = crate::arch::ms();
    if now.wrapping_sub(SHIM_LAST.load(Ordering::Relaxed)) < SHIMMER_MS || SHIM_BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    SHIM_LAST.store(now, Ordering::Relaxed);
    let cur = SHIM_CURSOR.load(Ordering::Relaxed);
    let t0 = now_ns().0;
    let next = crate::video::wallpaper::shimmer_with(|s, buf, w, h| {
        if w != pw || h != ph || s.w != w || s.h != h {
            return (cur, 0, 0, 0);
        }
        let (c, gl, nb, px) = shimmer_tick(
            s,
            now,
            occ,
            cur,
            // SAFETY: `buf` is the installed picture's `w * h` buffer (published with its shimmer, freed only by the
            // install after next — see `wallpaper::Snap`); `(x, y)` lies inside it (a glint is a pixel of that
            // picture). Painters read it concurrently; an aligned word store is never torn, and the pixel's box is
            // repainted by this pass's drain.
            |x, y, c| unsafe { core::ptr::write_volatile(buf.add(y * w + x), c) },
            |x, y, bw, bh| super::defer_erase(x, y, bw, bh, super::DEFER_ROUTE, false),
        );
        (c, nb, gl, px)
    });
    if let Some((c, nb, gl, px)) = next {
        SHIM_CURSOR.store(c, Ordering::Relaxed);
        // SHIMMERIDLE (B663): this tick's cost (re-tint + queue) and its boxes, read by the idle seat.
        SHIM_TICK_US.store(now_ns().0.saturating_sub(t0) / 1000, Ordering::Relaxed);
        SHIM_TICK_NB.store(nb, Ordering::Relaxed);
        SHIM_TICK_GL.store(gl, Ordering::Relaxed);
        SHIM_TICK_PX.store(px, Ordering::Relaxed);
        SHIM_TICKS.fetch_add(1, Ordering::Relaxed);
    }
    SHIM_BUSY.store(false, Ordering::Release);
}
#[cfg(not(feature = "facet"))]
fn shimmer_live(_occ: &[(usize, usize, usize, usize)], _pw: usize, _ph: usize) {}

/// Shimmer ticks run live since boot.
pub fn shimmer_ticks() -> u32 {
    SHIM_TICKS.load(Ordering::Relaxed)
}

// ── SHIMMERIDLE (rmbp-ledger B663): the twinkle on an idle desktop ───────────────────────────────────────────────
//
// The live seat ([`shimmer_live`]) rides [`track`], and `track` runs only inside a composite pass: on a desktop where
// nothing draws no pass runs, so the stars stood still. The TIMER is the device-service pass `x86_usb_pump` runs at
// ~1 kHz (`desktop_uefi::desktop_app_service`, beside `pace_service` and `splash::hold_service`, which already make an
// unmasked one-pass `composite` from this lane). [`shimmer_idle`] is called there on every pass and costs one clock
// read and one load until a tick is due. When one is due and the glass is idle it runs ONE ordinary `wm::composite`:
// the pass's own `track` runs the tick (fresh occluders, the `nf == 0` "no window moved since the last pass" rule,
// the empty-erase-queue rule), and the SAME pass's drain presents its boxes through `stage_fill` — the cheapest
// present there is, the one the live seat already uses. On the service core that pass is a ONE-PASS holder
// (GATESTEAL B657, `wm.rs` `masked = … || service_cpu()`): no re-run rounds, no re-acquire.
//
// It never contends for the gate: it runs only when `COMP_GATE` is free and nothing is owed (`COMP_PENDING`, the erase
// queue, a damaged row, the menu), so it never declines, never parks and never steals; any real work makes the pass
// that follows it carry the shimmer instead. It does not run while the splash hold covers the panel (`hold_service`
// ticks the hold store then) or while reduce motion is set (DESIGN-LANGUAGE §2.6). A tick that queued no box (the
// desktop covered, or the picture not the panel's) backs the idle floor off to [`SHIM_IDLE_BACKOFF`] x the floor.

/// The idle seat's floor multiplier after a tick that queued nothing (a covered desktop: ~4 probes a second).
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
const SHIM_IDLE_BACKOFF: u64 = 8;
/// The idle seat's last attempt (ms), whether its last pass queued nothing, its ticks, its worst tick (us), its worst
/// whole pass (us, tick + present), the attempts it skipped as not idle, and the fixture's reduce-motion force.
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_IDLE_LAST: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_IDLE_EMPTY: AtomicBool = AtomicBool::new(false);
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_IDLE_TICKS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_IDLE_MAX_US: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_IDLE_PASS_US: AtomicU64 = AtomicU64::new(0);
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_IDLE_SKIPS: AtomicU32 = AtomicU32::new(0);
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_RM_FORCE: AtomicBool = AtomicBool::new(false);

/// THE GATE CHECK: the compositor gate is free and nothing is owed — no declined pass (`COMP_PENDING`), no queued
/// erase box, no menu paint, no damaged row (a window that moved or drew since the last pass). Read-only; the window
/// table is taken once, for the scan. SHIMMERIDLE2 (B830): returns the refusing terms, one bit each (`WHY_*`, file
/// tail); 0 = the glass is idle.
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
fn idle_why() -> u8 {
    (if super::COMP_GATE.load(Ordering::Acquire) { WHY_GATE } else { 0 })
        | (if super::COMP_PENDING.load(Ordering::Acquire) { WHY_PENDING } else { 0 })
        | (if super::DEFER_N.load(Ordering::Relaxed) != 0 { WHY_ERASE } else { 0 })
        | (if super::menu_paint_owed() { WHY_MENU } else { 0 })
        | (if super::any_damaged() { WHY_DAMAGED } else { 0 })
}

/// **The idle tick** (SHIMMERIDLE, B663) — from the device-service pass on every iteration. See the block above.
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
pub fn shimmer_idle() {
    let now = crate::arch::ms();
    let floor = if SHIM_IDLE_EMPTY.load(Ordering::Relaxed) { SHIMMER_MS * SHIM_IDLE_BACKOFF } else { SHIMMER_MS };
    if now.wrapping_sub(SHIM_LAST.load(Ordering::Relaxed)) < SHIMMER_MS || now.wrapping_sub(SHIM_IDLE_LAST.load(Ordering::Relaxed)) < floor {
        return;
    }
    if crate::video::settings::reduce_motion() || SHIM_RM_FORCE.load(Ordering::Relaxed) || crate::splash::holding() || !crate::video::wallpaper::owns_backdrop() {
        return;
    }
    SHIM_IDLE_LAST.store(now, Ordering::Relaxed);
    let why = idle_why();
    if why != 0 {
        SHIM_IDLE_SKIPS.fetch_add(1, Ordering::Relaxed);
        if !shim_stranded(why) {
            return;
        }
    } else {
        SHIM_OWED_AT.store(0, Ordering::Relaxed);
    }
    let carry = why != 0; // SHIMMERIDLE2 (B830): a carried pass services owed work; its tick-less outcome is no backoff
    let n0 = SHIM_TICKS.load(Ordering::Relaxed);
    let t0 = now_ns().0;
    let drained = if carry { super::composite(); false } else { super::present_drain() == Some(true) }; // SHIMMERPRESENT (B840): the idle tick presents its boxes through the drain (`wm::present_drain`); a carried pass is the whole composite
    let pass_us = now_ns().0.saturating_sub(t0) / 1000;
    let ticked = SHIM_TICKS.load(Ordering::Relaxed) != n0;
    if !carry || ticked {
        SHIM_IDLE_EMPTY.store(!ticked || SHIM_TICK_NB.load(Ordering::Relaxed) == 0, Ordering::Relaxed);
    }
    if ticked {
        SHIM_IDLE_TICKS.fetch_add(1, Ordering::Relaxed);
        SHIM_IDLE_MAX_US.fetch_max(SHIM_TICK_US.load(Ordering::Relaxed), Ordering::Relaxed);
        SHIM_IDLE_PASS_US.fetch_max(pass_us, Ordering::Relaxed); shim_present_note(drained, pass_us);
        shim_wire_once();
    }
}

/// **`tests shimmeridle`** (R80: a verb, never at boot). Two 200 ms windows with the fixture's own task asleep (it
/// draws nothing), after a settle of at most 1 s for the first tick (SHIMMERIDLE2 B830): (1) the shimmer's ticks in the
/// window (all seats) must be 3..=7 — the 33 ms floor caps it at 7, the idle seat must lift it to at least 3 — each idle tick under STARFIELD's bound (800 us), and no gate steal;
/// (2) with reduce motion forced, the idle seat ticks zero times.
/// `:: SHIMMERIDLE: detail idle_ticks=<n> pass_max_us=<n> skips=<n> backdrop=<0|1> why=<terms|none> carried=<n> settle_ms=<n> present=<drain|pass> pass_us=<n> ::`
/// `:: SHIMMERIDLE: idle_ms=<n> ticks=<n> max_us=<n> reduce_motion_ticks=<n> gate_steals=<n> -> PASS|FAIL ::`
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
pub fn shimmeridle_selftest() {
    const IDLE_MS: u64 = 200;
    if crate::video::settings::reduce_motion() {
        serial_println!(":: SHIMMERIDLE: reduce motion is set (the idle tick is off by design) -> SKIP ::");
        return;
    }
    // SHIMMERIDLE2 (B830): SETTLE first — the premise is an idle desktop; wait (<= 1 s) for any shimmer tick.
    let (s0, n0) = (crate::arch::ms(), SHIM_TICKS.load(Ordering::Relaxed));
    while SHIM_TICKS.load(Ordering::Relaxed) == n0 && crate::arch::ms().saturating_sub(s0) < 1000 {
        crate::arch::sched::sleep_ms(5);
    }
    let settle_ms = crate::arch::ms().saturating_sub(s0);
    SHIM_IDLE_WHY.store(0, Ordering::Relaxed);
    let carried0 = SHIM_IDLE_CARRIED.load(Ordering::Relaxed);
    let steal0 = super::comp_steal_gen();
    let (skip0, i0) = (SHIM_IDLE_SKIPS.load(Ordering::Relaxed), SHIM_IDLE_TICKS.load(Ordering::Relaxed));
    SHIM_IDLE_MAX_US.store(0, Ordering::Relaxed);
    SHIM_IDLE_PASS_US.store(0, Ordering::Relaxed);
    let (t0, a0) = (crate::arch::ms(), SHIM_TICKS.load(Ordering::Relaxed));
    crate::arch::sched::sleep_ms(IDLE_MS);
    let idle_ms = crate::arch::ms().saturating_sub(t0);
    let ticks = SHIM_TICKS.load(Ordering::Relaxed).wrapping_sub(a0);
    let idle_ticks = SHIM_IDLE_TICKS.load(Ordering::Relaxed).wrapping_sub(i0);
    let (max_us, pass_us) = (SHIM_IDLE_MAX_US.load(Ordering::Relaxed), SHIM_IDLE_PASS_US.load(Ordering::Relaxed));
    SHIM_RM_FORCE.store(true, Ordering::Relaxed);
    let r0 = SHIM_IDLE_TICKS.load(Ordering::Relaxed);
    crate::arch::sched::sleep_ms(IDLE_MS);
    let rm_ticks = SHIM_IDLE_TICKS.load(Ordering::Relaxed).wrapping_sub(r0);
    SHIM_RM_FORCE.store(false, Ordering::Relaxed);
    let steals = super::comp_steal_gen().wrapping_sub(steal0);
    let ok = (3..=7).contains(&ticks) && idle_ticks > 0 && max_us <= 800 && rm_ticks == 0 && steals == 0;
    shim_wire();
    serial_println!(
        ":: SHIMMERIDLE: detail idle_ticks={} pass_max_us={} skips={} backdrop={} why={} carried={} settle_ms={} present={} pass_us={} ::",
        idle_ticks, pass_us, SHIM_IDLE_SKIPS.load(Ordering::Relaxed).wrapping_sub(skip0), crate::video::wallpaper::owns_backdrop() as u8,
        WhyLabel(SHIM_IDLE_WHY.load(Ordering::Relaxed)), SHIM_IDLE_CARRIED.load(Ordering::Relaxed).wrapping_sub(carried0), settle_ms, shim_present_label(), SHIM_PRESENT_US.load(Ordering::Relaxed)
    );
    serial_println!(
        ":: SHIMMERIDLE: idle_ms={} ticks={} max_us={} reduce_motion_ticks={} gate_steals={} -> {} ::",
        idle_ms, ticks, max_us, rm_ticks, steals, if ok { "PASS" } else { "FAIL" }
    );
}

/// The shimmer's cost on an offscreen picture: `ticks` ticks 7 x [`SHIMMER_MS`] apart, each re-tint plus the drain's
/// fill modelled as the wallpaper rows of every queued box copied into `frame`. Returns `(worst tick us, mean us,
/// boxes, px changed in wall)` — `wall` is changed in place.
fn shimmer_cost(s: &Shimmer, wall: &mut [u32], frame: &mut [u32], pw: usize, occ: &[(usize, usize, usize, usize)], ticks: u32) -> (u64, u64, u32) {
    let (mut cur, mut worst, mut total, mut nbox) = (0u32, 0u64, 0u64, 0u32);
    for k in 0..ticks {
        let now = 1000 + k as u64 * SHIMMER_MS * 7;
        let mut boxes = [(0usize, 0usize, 0usize, 0usize); 16];
        let mut nb = 0usize;
        let (t0, _) = now_ns();
        let (c, _, _, _) = shimmer_tick(s, now, occ, cur, |x, y, c| wall[y * pw + x] = c, |x, y, w, h| {
            if nb < boxes.len() {
                boxes[nb] = (x, y, w, h);
                nb += 1;
            }
        });
        for &(x, y, w, h) in boxes[..nb].iter() {
            for yy in y..y + h {
                let o = yy * pw + x;
                if o + w <= frame.len() && o + w <= wall.len() {
                    frame[o..o + w].copy_from_slice(&wall[o..o + w]);
                }
            }
        }
        let dt = now_ns().0.saturating_sub(t0);
        worst = worst.max(dt);
        total += dt;
        nbox += nb as u32;
        cur = c;
    }
    (worst / 1000, total / ticks.max(1) as u64 / 1000, nbox)
}

/// The DESKTOPCRAFT fixture's three window boxes, grown by their shadow margins — the occluders the shimmer sees.
fn fixture_occluders(pw: usize, ph: usize) -> [(usize, usize, usize, usize); 3] {
    let k = |v: usize| v * pw / 1440;
    let dark = theme::is_dark();
    let s2 = crate::video::dpi::s2();
    let wins = [(k(1010), k(250), k(380), k(520), false), (k(640), k(170), k(760), k(520), false), (k(120), k(110), k(900), k(600), true)];
    let mut out = [(0usize, 0usize, 0usize, 0usize); 3];
    for (i, &(x, y, w, h, focused)) in wins.iter().enumerate() {
        let (t, s, b) = margins(if focused { ELEV_WINDOW } else { ELEV_IDLE }, dark, s2);
        let (w, h) = (w.min(pw.saturating_sub(x)), h.min(ph.saturating_sub(y)));
        let (x0, y0) = (x.saturating_sub(s as usize), y.saturating_sub(t as usize));
        out[i] = (x0, y0, x + w + s as usize - x0, y + h + b as usize - y0);
    }
    out
}

/// **`tests starfield`** (R80: a verb, never at boot) — generates the scene offscreen at the rMBP panel's size
/// (2880x1800; 1440x900 where memory refuses), times it against 400 ms, checks the lens on three known stars, the
/// zircon, the beam, the shimmer list's cap and that no pure-black run exists, then runs 30 shimmer ticks with the
/// DESKTOPCRAFT windows as occluders: the worst tick against 800 us, and no pixel under a window changed.
///
/// `:: STARFIELD: detail surface=<w>x<h> glints=<n> groups=<n> zircon_box=<x>,<y>,<w>,<h> front=<n> beam_land=<x>,<y> lens=ok(<dx>,<dy>) x3 covered_changed=0 changed=<n> boxes=<n> shimmer_max_us=<n> literals=0 ::`
/// `:: STARFIELD: gen_ms=<n> stars=<n> lensed=ok streams=<n> zircon=ok(facets=17) beam=ok shimmer_us=<n> -> PASS ::`
pub fn starfield_selftest() {
    let mut dims = [(2880usize, 1800usize), (1440, 900)].into_iter();
    let (pw, ph, mut wall, mut frame) = loop {
        let Some((w, h)) = dims.next() else {
            serial_println!(":: STARFIELD: no-surface -> FAIL ::");
            return;
        };
        let (mut a, mut b) = (alloc::vec::Vec::new(), alloc::vec::Vec::new());
        if a.try_reserve_exact(w * h).is_ok() && b.try_reserve_exact(w * h).is_ok() {
            a.resize(w * h, 0u32);
            b.resize(w * h, 0u32);
            break (w, h, a, b);
        }
    };
    let (t0, _) = now_ns();
    let scene = wall_gen(&mut wall, pw, ph);
    let gen_ms = now_ns().0.saturating_sub(t0) / 1_000_000;
    let Some(sc) = scene else {
        serial_println!(":: STARFIELD: gen_ms={} no-scene -> FAIL ::", gen_ms);
        return;
    };
    let at = |x: usize, y: usize| wall[y.min(ph - 1) * pw + x.min(pw - 1)];
    // The lens, on three known stars: displaced >= 1 px, at most Dmax (+1), AWAY from the mass (delta . G < 0), and a
    // star is drawn at the image (it differs from the mean of the ring 5 px round it).
    let mut lens_ok = sc.nprobes == 3;
    let mut lens_txt: [(bool, i64, i64); 3] = [(false, 0, 0); 3];
    for (i, pr) in sc.probes[..sc.nprobes.min(3)].iter().enumerate() {
        let (dx, dy) = (pr.tq.0 - pr.bq.0, pr.tq.1 - pr.bq.1);
        let m2 = dx * dx + dy * dy;
        let away = dx * pr.g.0 + dy * pr.g.1 < 0;
        let within = m2 >= 256 * 256 && m2 <= ((sc.dmax + 1) * 256).pow(2);
        let (x, y) = ((pr.tq.0 >> 8) as usize, (pr.tq.1 >> 8) as usize);
        let mut acc = [0u32; 3];
        for (ox, oy) in [(5i64, 0i64), (-5, 0), (0, 5), (0, -5), (4, 4), (-4, 4), (4, -4), (-4, -4)] {
            let c = at((x as i64 + ox).max(0) as usize, (y as i64 + oy).max(0) as usize);
            for (k, s) in [16u32, 8, 0].iter().enumerate() {
                acc[k] += (c >> s) & 255;
            }
        }
        let ring = ((acc[0] / 8) << 16) | ((acc[1] / 8) << 8) | (acc[2] / 8);
        let drawn = sf_dist(at(x, y), ring) > 6;
        let ok = away && within && drawn;
        lens_ok &= ok;
        lens_txt[i] = (ok, (dx + 128) >> 8, (dy + 128) >> 8);
    }
    // The zircon: 17 facets, 5..=11 facing the camera, 40..=50 % of the panel tall, on the panel.
    let (gx, gy, gw, gh) = sc.gem;
    let gem_ok = sc.front >= 5 && sc.front <= 11 && gh * 1000 >= ph * 400 && gh * 1000 <= ph * 500 && gx + gw <= pw && gy + gh <= ph;
    // The beam: its column mid-way down differs from the field three glow-widths aside; it lands on the stone.
    let (bx, land, bgw) = sc.beam;
    let side = if bx + 3 * bgw < pw { bx + 3 * bgw } else { bx.saturating_sub(3 * bgw) };
    let beam_ok = land > 0 && land >= gy && land <= gy + gh && sf_dist(at(bx, land / 2), at(side, land / 2)) > 12;
    // No pure-black run (the ground is ultra black, never #000000).
    let mut runs = 0usize;
    for y in 0..ph {
        let mut run = 0usize;
        for x in 0..pw {
            if wall[y * pw + x] & 0x00FF_FFFF == 0 {
                run += 1;
                if run == 3 {
                    runs += 1;
                }
            } else {
                run = 0;
            }
        }
    }
    // The shimmer: 30 ticks, the DESKTOPCRAFT windows as occluders.
    let occ = fixture_occluders(pw, ph);
    let mut before: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    if before.try_reserve_exact(pw * ph).is_err() {
        serial_println!(":: STARFIELD: gen_ms={} no-snapshot -> FAIL ::", gen_ms);
        return;
    }
    before.extend_from_slice(&wall);
    frame.copy_from_slice(&wall);
    let (worst, _mean, boxes) = shimmer_cost(&sc.shimmer, &mut wall, &mut frame, pw, &occ, 30);
    let (mut changed, mut covered) = (0u64, 0u64);
    for y in 0..ph {
        for x in 0..pw {
            if wall[y * pw + x] != before[y * pw + x] {
                changed += 1;
                if occ.iter().any(|&(ox, oy, ow, oh)| x >= ox && x < ox + ow && y >= oy && y < oy + oh) {
                    covered += 1;
                }
            }
        }
    }
    let nglint = sc.shimmer.glints.len();
    let lit = theme::AUDIT_LITERALS_OUTSIDE;
    let shim_ok = worst <= 800 && nglint <= SHIMMER_MAX && covered == 0 && changed > 0;
    let ok = gen_ms <= 400 && sc.stars > 0 && lens_ok && sc.streams > 0 && gem_ok && beam_ok && runs == 0 && shim_ok && lit == 0;
    let lf = |i: usize| if lens_txt[i].0 { "ok" } else { "FAIL" };
    serial_println!(
        ":: STARFIELD: detail surface={}x{} glints={} groups={} zircon_box={},{},{},{} front={} beam_land={},{} lens={}({},{}) lens={}({},{}) lens={}({},{}) covered_changed={} changed={} boxes={} shimmer_max_us={} black_runs={} literals={} ::",
        pw, ph, nglint, sc.shimmer.groups.len(), gx, gy, gw, gh, sc.front, bx, land,
        lf(0), lens_txt[0].1, lens_txt[0].2, lf(1), lens_txt[1].1, lens_txt[1].2, lf(2), lens_txt[2].1, lens_txt[2].2,
        covered, changed, boxes, worst, runs, lit
    );
    serial_println!(
        ":: STARFIELD: gen_ms={} stars={} lensed={} streams={} zircon={}(facets={}) beam={} shimmer_us={} -> {} ::",
        gen_ms,
        sc.stars,
        if lens_ok { "ok" } else { "FAIL" },
        sc.streams,
        if gem_ok { "ok" } else { "FAIL" },
        ZIRCON_FACETS,
        if beam_ok { "ok" } else { "FAIL" },
        worst,
        if ok { "PASS" } else { "FAIL" }
    );
}

/// **The VEIL** of `px` (`w` x `h`): each `VEIL_CELL`-square cell's mean, then one [1 2 1] pass each way over the
/// cells — a blur of about three cells, built once per wallpaper. Returns `(cells, vw, vh)`, or `None` without memory.
pub fn veil_gen(px: &[u32], w: usize, h: usize) -> Option<(alloc::vec::Vec<u32>, usize, usize)> {
    if w == 0 || h == 0 || px.len() < w * h {
        return None;
    }
    let (vw, vh) = (w.div_ceil(VEIL_CELL), h.div_ceil(VEIL_CELL));
    let mut sum = alloc::vec::Vec::new();
    if sum.try_reserve_exact(vw * vh * 4).is_err() {
        return None;
    }
    sum.resize(vw * vh * 4, 0u32);
    for y in 0..h {
        let cy = y >> VEIL_SHIFT;
        let row = &px[y * w..(y + 1) * w];
        for (x, &p) in row.iter().enumerate() {
            let o = (cy * vw + (x >> VEIL_SHIFT)) * 4;
            sum[o] += (p >> 16) & 255;
            sum[o + 1] += (p >> 8) & 255;
            sum[o + 2] += p & 255;
            sum[o + 3] += 1;
        }
    }
    // Means, as three channels per cell.
    let mut c3 = alloc::vec::Vec::new();
    if c3.try_reserve_exact(vw * vh * 3).is_err() {
        return None;
    }
    for i in 0..vw * vh {
        let n = sum[i * 4 + 3].max(1);
        c3.push(sum[i * 4] / n);
        c3.push(sum[i * 4 + 1] / n);
        c3.push(sum[i * 4 + 2] / n);
    }
    drop(sum);
    // [1 2 1] across, then down (edges clamp).
    let mut tmp = c3.clone();
    for y in 0..vh {
        for x in 0..vw {
            let (l, r) = (x.saturating_sub(1), (x + 1).min(vw - 1));
            for k in 0..3 {
                tmp[(y * vw + x) * 3 + k] = (c3[(y * vw + l) * 3 + k] + 2 * c3[(y * vw + x) * 3 + k] + c3[(y * vw + r) * 3 + k] + 2) / 4;
            }
        }
    }
    let mut out = alloc::vec::Vec::new();
    if out.try_reserve_exact(vw * vh).is_err() {
        return None;
    }
    for y in 0..vh {
        let (u, dn) = (y.saturating_sub(1), (y + 1).min(vh - 1));
        for x in 0..vw {
            let v = |k: usize| (tmp[(u * vw + x) * 3 + k] + 2 * tmp[(y * vw + x) * 3 + k] + tmp[(dn * vw + x) * 3 + k] + 2) / 4;
            out.push((v(0).min(255) << 16) | (v(1).min(255) << 8) | v(2).min(255));
        }
    }
    Some((out, vw, vh))
}

// ── tests desktopcraft ──────────────────────────────────────────────────────────────────────────────────────────

/// Monotonic nanoseconds and the clock's name (the TSC where it is calibrated, else the millisecond tick).
fn now_ns() -> (u64, &'static str) {
    #[cfg(target_arch = "x86_64")]
    {
        let hz = crate::arch::x86_64::apic::tsc_hz();
        if hz >= 1_000_000 {
            return (crate::arch::now_cycles().saturating_mul(1000) / (hz / 1_000_000), "tsc");
        }
    }
    (crate::arch::ms().saturating_mul(1_000_000), "ms")
}

/// One staged window of the fixture: its box (panel px) and its head.
struct FixWin {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    focused: bool,
    title: &'static [u8],
}

/// WINEDGE2 (B668) — which base each desktop painter lays on THIS boot, `(desk, fill)`: true = the wallpaper
/// picture, false = the flat `DESKTOP_BG`. The fill under a shadow (`wm::stage_fill` → `bg_row`) lays the picture
/// whenever one is installed (`wallpaper::active`); the desktop layer beside it (`Screen`'s back buffer, through
/// `wm::wp_paint`) only while the scene owns the backdrop (`video::desktop_scene_owns_backdrop`). Flight 36 (x86,
/// no `[realdesk]` line) ran `(false, true)`: the margin was the only place the picture reached the glass. With no
/// picture installed both are read over the generated one (the design case).
fn live_bases() -> (bool, bool) {
    #[cfg(feature = "facet")]
    if crate::video::wallpaper::active() {
        return (crate::video::desktop_scene_owns_backdrop(), true);
    }
    (true, true)
}

/// One look's `shadow_dark=` reading: the margin's outermost pixel equals the backdrop under it (`outer_eq`), and
/// `inner` — the first pixel outside the box minus the backdrop beside it, mean of three channels, the weaker of the
/// bottom and left sides (negative = darker; positive = the flight-36 flaw, a margin LIGHTER than the backdrop).
struct DarkRead {
    outer_eq: bool,
    inner: i64,
    ok: bool,
}

/// WINEDGE2 (B668): a focused window (`elev.3`) over the generated wallpaper drawn in look `dark` into `buf`
/// (`w` x `h`), its shadow shaded as the fill shades it over the fill's base, read against the desktop layer's base
/// (`bases`, [`live_bases`]) at the bottom margin (mid-width) and the left margin (mid-height). `None` when the
/// picture, the tiles or the margin do not fit.
fn shadow_dark_look(buf: &mut [u32], w: usize, h: usize, dark: bool, bases: (bool, bool)) -> Option<DarkRead> {
    if buf.len() < w * h {
        return None;
    }
    buf[..w * h].fill(theme::DESKTOP_BG);
    wall_gen_look(buf, w, h, dark)?;
    let t = tiles_for(ELEV_WINDOW, dark, win_radius(false))?;
    let (x0, y0) = (w as i32 / 6, h as i32 / 8);
    let (x1, y1) = (x0 + w as i32 / 2, y0 + h as i32 * 5 / 12);
    if x0 - t.side < 0 || y1 + t.bot > h as i32 || t.bot <= 0 || t.side <= 0 {
        return None;
    }
    let mut one = Casts::EMPTY;
    one.push(Cast { id: 0xD5E0, x0, y0, x1, y1, t });
    let buf = &buf[..w * h];
    let desk = |x: usize, y: usize| if bases.0 { buf[y * w + x] } else { theme::DESKTOP_BG };
    let fill = |x: usize, y: usize| {
        let mut p = [if bases.1 { buf[y * w + x] } else { theme::DESKTOP_BG }];
        shade_words(&one, &mut p, y as i32, x as i32);
        p[0]
    };
    let ch = |c: u32, s: u32| ((c >> s) & 255) as i64;
    let (mx, my) = (((x0 + x1) / 2) as usize, ((y0 + y1) / 2) as usize);
    // (inner, outer): the first pixel outside the box, the margin's last pixel — below, then to the left.
    let pairs = [((mx, y1 as usize), (mx, (y1 + t.bot - 1) as usize)), (((x0 - 1) as usize, my), ((x0 - t.side) as usize, my))];
    let mut r = DarkRead { outer_eq: true, inner: i64::MIN, ok: true };
    for &((ix, iy), (ox, oy)) in pairs.iter() {
        if fill(ox, oy) != desk(ox, oy) {
            r.outer_eq = false;
        }
        let (b, m) = (desk(ix, iy), fill(ix, iy));
        let lighter = [16u32, 8, 0].iter().any(|&s| ch(m, s) > ch(b, s));
        let d = [16u32, 8, 0].iter().map(|&s| ch(m, s) - ch(b, s)).sum::<i64>() / 3;
        if lighter || d >= 0 {
            r.ok = false;
        }
        r.inner = r.inner.max(d);
    }
    r.ok &= r.outer_eq;
    Some(r)
}

/// **`tests desktopcraft`** — paints the default desktop into an offscreen surface the size of the rMBP panel
/// (2880x1800; 1440x900 where memory refuses) the way the compositor does: the generated wallpaper and its VEIL,
/// the desktop under every window's shadow (the fill's own shading, [`shade_words`], over the pre-rendered tiles),
/// the menu bar's field, three windows (Lumen, Settings, Aether, bottom to top, Aether focused) painted over it, and
/// the dock's plate with nine tiles. It asserts the invariants and prints the paint cost on the running machine
/// against the 60 Hz frame (16 667 us):
///
/// `:: DESKTOPCRAFT: corners=ok shadow=ok edge=ok(1px) shadow_dark=ok(outer=eq inner=-<n>) hairline=ok black=ok veil=ok(<veil_us>/<fill_us>) windows=3 surface=<w>x<h> -> PASS ::`
/// `:: DESKTOPCRAFT: cost clock=<tsc|ms> frame_us=<n> budget_us=16667 backdrop_us=<n> chrome_us=<n> bar_us=<n> dock_us=<n> wall_ms=<n> veil_ms=<n> shadow_us=<n> tiles_us=<n> tiles_kib=<n> margin=<t>/<s>/<b> idle_margin=<t>/<s>/<b> ramp_a=<first>/<last> ramp_dark=<first>/<last> frame=<n> track_fills=<n> black_runs=<n> shimmer_us=<n> veil_us=<n> fill_us=<n> veil_cache=<hit|miss> -> UNDER|OVER ::`
///
/// * **corners** — the focused window's top-left box pixel (outside the rounded head) is the desktop under the
///   shadows (exactly the fill's value there), and the head's face one radius in is not;
/// * **shadow** — WINEDGE (B659) re-derived it against what the tiles CAST. Below the focused window at mid-width,
///   over the bottom margin's `bot` rows: the cast's own alpha there (its bottom ramp, read through
///   [`Cast::for_each_alpha`] — the reader the fill uses) is positive on the first row outside the box, never rises
///   row to row (no tolerance: the ramp is integer tile data, not a dithered measurement), and is at most 2 of 255 on
///   the margin's last row; and every one of those panel pixels is EXACTLY the wallpaper under every cast's shade
///   there (the fill's value, as `corners` reads it). The darkness (wallpaper minus panel, three channels) on the
///   first and last row is printed on the cost line (`ramp_dark=`) beside the alpha (`ramp_a=`), not asserted: the
///   wallpaper is ordered-dithered, so its row-to-row step is the dither's, not the shadow's. The clause it replaces
///   (DESIGNAPPLY's) started `prev` at `i64::MAX` and tested `dk > prev + 3`: in the `--release` kernel the add
///   wraps to `i64::MIN + 2`, so the FIRST row always failed — flight 35's `shadow=FAIL` at 2.0, and every other
///   scale (`docs/dev/evidence/winedge/edgecheck.out`);
/// * **edge** — WINEDGE (B659): the focused window's EDGE is one device px of [`edge_ink`] — the box's outermost
///   column at mid-content on both sides, its last row at mid-width, and the head's side one pixel below the corner
///   arc are the ink; one pixel further in is the content (or the head's face), so it is ONE px; and one pixel
///   outside, on the left and below, is the shaded desktop (the edge is inside the box, the shadow outside it).
///   Printed `edge=ok(1px)`;
/// * **shadow_dark** — WINEDGE2 (B668, flight 36: the margin painted as a pale box over a dark desktop): on BOTH
///   looks, a focused window over the generated wallpaper ([`shadow_dark_look`]), the margin as the fill lays it and
///   the backdrop beside it as the desktop layer lays it on this boot ([`live_bases`]): the margin's outermost pixel
///   EQUALS the backdrop under it and the first pixel outside the box is darker than the backdrop (no channel
///   lighter). Printed `shadow_dark=ok(outer=eq inner=-<n>)` (`n`, the weaker look's mean channel darkening), else
///   `shadow_dark=FAIL(look=<light|dark> outer=<eq|ne> inner=<+-n> bases=<wall|flat>/<wall|flat>)`;
/// * **hairline** — the dock plate's top edge is a line over the plate;
/// * **black** — no run of pure `#000000` longer than 2 px outside a glyph zone;
/// * **veil** — VEILCOST (rmbp-ledger B690, BP-bar-23: beauty costs no more than a solid fill): the menu bar's box
///   repainted as a strip repaints it (each row composed into the scratch row, then copied out — `strip::paint`'s
///   shape, here into the staged frame) once over the VEIL through a [`VeilCache`] ([`bar_row_cached`], the live
///   bar's path) and once as a solid `surface.window` fill; best of 3, interleaved (the first veiled pass fills the
///   cache, the later ones copy it). Asserted `veil_us <= fill_us x 1.05 + 50` (in ns); printed `veil=ok(<veil>/<fill>)`
///   or `veil=OVER(<veil>/<fill>)`, and both on the cost line with `veil_cache=hit` when the last pass was all kept rows.
pub fn selftest() {
    let mut dims = [(2880usize, 1800usize), (1440, 900)].into_iter();
    let (pw, ph, mut wall, mut frame) = loop {
        let Some((w, h)) = dims.next() else {
            serial_println!(":: DESKTOPCRAFT: no-surface -> FAIL ::");
            return;
        };
        let (mut a, mut b) = (alloc::vec::Vec::new(), alloc::vec::Vec::new());
        if a.try_reserve_exact(w * h).is_ok() && b.try_reserve_exact(w * h).is_ok() {
            a.resize(w * h, 0u32);
            b.resize(w * h, 0u32);
            break (w, h, a, b);
        }
    };
    let (t0, clock) = now_ns();
    let scene = wall_gen(&mut wall, pw, ph); // STARFIELD (B661): the scene; its shimmer is costed after every check
    let (t1, _) = now_ns();
    let Some((veil, vw, vh)) = veil_gen(&wall, pw, ph) else {
        serial_println!(":: DESKTOPCRAFT: no-veil -> FAIL ::");
        return;
    };
    let (t2, _) = now_ns();
    let back = Back { px: &wall, w: pw, h: ph, veil: &veil, vw, vh };
    // The scale the layout is drawn at: the panel's own (logical 1440 wide at 2x on the rMBP).
    let k = |v: usize| v * pw / 1440;
    let bar_h = crate::ui::px(28).max(k(28)).min(ph / 8);
    let wins = [
        FixWin { x: k(1010), y: k(250), w: k(380), h: k(520), focused: false, title: b"Lumen" },
        FixWin { x: k(640), y: k(170), w: k(760), h: k(520), focused: false, title: b"Settings" },
        FixWin { x: k(120), y: k(110), w: k(900), h: k(600), focused: true, title: b"Aether" },
    ];
    let mut row = alloc::vec::Vec::new();
    if row.try_reserve_exact(pw).is_err() {
        serial_println!(":: DESKTOPCRAFT: no-row -> FAIL ::");
        return;
    }
    row.resize(pw, 0u32);
    let (mut corners, mut shadow, mut edge, mut hairline, mut black) = (true, true, true, true, true);
    // WINEDGE (B659): the focused window's bottom ramp, first/last row — the cast's alpha and the darkness it paints.
    let (mut ramp_a, mut ramp_dark) = ((0u32, 0u32), (0i64, 0i64));
    let mut glyph_rects: alloc::vec::Vec<(usize, usize, usize, usize)> = alloc::vec::Vec::new();
    // The casts the compositor would publish for these three boxes (the live theme, scale and look).
    let dark = theme::is_dark();
    let rad = win_radius(false);
    let (tk0, _) = now_ns();
    let mut casts = Casts::EMPTY;
    for (i, fw) in wins.iter().enumerate() {
        let (bw, bh) = (fw.w.min(pw - fw.x), fw.h.min(ph - fw.y));
        let level = if fw.focused { ELEV_WINDOW } else { ELEV_IDLE };
        let Some(t) = tiles_for(level, dark, rad) else {
            serial_println!(":: DESKTOPCRAFT: no-tiles level={} -> FAIL ::", ELEV_NAMES[level]);
            return;
        };
        casts.push(Cast { id: 0xD5C0 + i as super::WinId, x0: fw.x as i32, y0: fw.y as i32, x1: (fw.x + bw) as i32, y1: (fw.y + bh) as i32, t });
    }
    let tiles_ns = now_ns().0.saturating_sub(tk0);
    let (mut tb, mut seen, mut ns) = (0usize, [core::ptr::null::<Tiles>(); 2], 0usize);
    for c in casts.iter() {
        let p = c.t as *const Tiles;
        if ns < 2 && !seen[..ns].contains(&p) {
            seen[ns] = p;
            ns += 1;
            tb += c.t.bytes;
        }
    }
    let tiles_kib = tb.div_ceil(1024);

    // THE FRAME: the backdrop, the shadows on it, the bar, three windows, the dock — timed as one.
    let (f0, _) = now_ns();
    frame.copy_from_slice(&wall);
    let (f1, _) = now_ns();
    // The desktop under the shadows: what `wm::stage_fill` writes over each cast's extent (the windows paint over
    // their own boxes next, exactly as the compositor clips the fill).
    let mut ext = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for c in casts.iter() {
        let e = c.extent();
        ext = (ext.0.min(e.0), ext.1.min(e.1), ext.2.max(e.2), ext.3.max(e.3));
    }
    let (ey0, ey1) = (ext.1.max(0) as usize, (ext.3.max(0) as usize).min(ph));
    let (ex0, ex1) = (ext.0.max(0) as usize, (ext.2.max(0) as usize).min(pw));
    let mut spans: [(usize, usize); 8] = [(0, 0); 8];
    for y in ey0..ey1 {
        // The row's spans with every window box cut out — the fill's own clip (`erase_clip`): it never writes a box.
        let mut ns = 1usize;
        spans[0] = (ex0, ex1);
        for c in casts.iter() {
            if (y as i32) < c.y0 || (y as i32) >= c.y1 {
                continue;
            }
            let (bx0, bx1) = (c.x0.max(0) as usize, c.x1.max(0) as usize);
            let mut out = [(0usize, 0usize); 8];
            let mut no = 0usize;
            for &(a, b) in spans[..ns].iter() {
                for piece in [(a, b.min(bx0)), (a.max(bx1), b)] {
                    if piece.1 > piece.0 && no < out.len() {
                        out[no] = piece;
                        no += 1;
                    }
                }
            }
            spans = out;
            ns = no;
        }
        for &(a, b) in spans[..ns].iter() {
            shade_words(&casts, &mut frame[y * pw + a..y * pw + b], y as i32, a as i32);
        }
    }
    let (fs, _) = now_ns();
    for j in 0..bar_h {
        bar_row(&mut row, 0, 0, pw, bar_h, j, Some(&back));
        frame[j * pw..(j + 1) * pw].copy_from_slice(&row[..pw]);
    }
    let (f2, _) = now_ns();
    let t = super::TITLE_H();
    let dd = theme::CONTROL_BOX() as i32;
    let mut chrome_ns = 0u64;
    for fw in wins.iter() {
        let (bw, bh) = (fw.w.min(pw - fw.x), fw.h.min(ph - fw.y));
        let cy = ((t.saturating_sub(dd as usize)) / 2) as i32;
        let cx0 = super::GAP() as i32;
        let g = super::GAP() as i32;
        let h = Head {
            bw,
            bh,
            focused: fw.focused,
            square: false,
            ctrls: [Some((cx0, cy, dd)), Some((cx0 + dd + g, cy, dd)), Some((cx0 + 2 * (dd + g), cy, dd))],
            hover: fw.focused,
            title: fw.title,
        };
        let (c0, _) = now_ns();
        let (fx, fy) = (fw.x, fw.y);
        {
            let fr = &mut frame;
            paint_box(&h, fx, fy, Some(&back), &casts, &mut row, 0..bh, |j, x0, px| {
                let o = (fy + j) * pw + fx + x0;
                fr[o..o + px.len()].copy_from_slice(px);
            });
        }
        // The content (a flat document face; the app paints this on the glass).
        let cf = theme::surface_content();
        for y in fy + t..fy + bh {
            frame[y * pw + fx..y * pw + fx + bw].fill(cf);
        }
        // The EDGE below the head, after the content — `edge_paint`'s spans and ink, as `wm::paint_window`'s tail.
        let ink = edge_ink();
        for j in t.min(bh)..bh {
            for (x0, len) in edge_spans(bw, bh, j) {
                let o = (fy + j) * pw + fx + x0;
                frame[o..o + len].fill(ink);
            }
        }
        chrome_ns += now_ns().0.saturating_sub(c0);
        if fw.focused {
            // corners: the box's top-left pixel is the desktop under the shadows, exactly the fill's value there.
            let rr = head_radius(&h) as usize;
            let mut want = [wall[fy * pw + fx]];
            shade_words(&casts, &mut want, fy as i32, fx as i32);
            let px = frame[fy * pw + fx];
            let inside = frame[(fy + rr.max(1)) * pw + fx + bw - 1 - rr.max(1)];
            if rr == 0 || px != want[0] || inside == px || inside != theme::surface_window() { // `inside`: one radius in from the top-RIGHT corner (the discs sit top-left)
                corners = false;
            }
            // shadow (WINEDGE B659): below the box at mid-width, the CAST's ramp — positive next to the box, never
            // rising, <= 2 on the margin's last row — and each panel pixel exactly the wallpaper under every cast there.
            let mx = fx + bw / 2;
            let dark_of = |a: u32, b: u32| [16u32, 8, 0].iter().map(|&s| ((a >> s) & 255) as i64 - ((b >> s) & 255) as i64).sum::<i64>();
            let me = casts.iter().find(|c| c.x0 == fx as i32 && c.y0 == fy as i32).copied();
            let bot = me.map_or(0, |c| c.t.bot) as usize;
            let mut prev: Option<u32> = None;
            let (mut a_first, mut a_last, mut d_first, mut d_last) = (0u32, 0u32, 0i64, 0i64);
            for (n, y) in (fy + bh..(fy + bh + bot).min(ph)).enumerate() {
                let mut a = 0u32;
                if let Some(c) = me {
                    c.for_each_alpha(y as i32, mx as i32, mx as i32 + 1, |_, al| a = al);
                }
                let mut want = [wall[y * pw + mx]];
                shade_words(&casts, &mut want, y as i32, mx as i32);
                let dk = dark_of(wall[y * pw + mx], frame[y * pw + mx]);
                if n == 0 {
                    (a_first, d_first) = (a, dk);
                }
                if prev.is_some_and(|p| a > p) || frame[y * pw + mx] != want[0] {
                    shadow = false;
                }
                prev = Some(a);
                (a_last, d_last) = (a, dk);
            }
            if bot == 0 || a_first == 0 || a_last > 2 || prev.is_none() {
                shadow = false;
            }
            (ramp_a, ramp_dark) = ((a_first, a_last), (d_first, d_last));
            // edge (WINEDGE B659): one device px of `edge_ink` round the box, inside it, the shaded desktop outside.
            let ym = fy + (t + bh) / 2;
            let ylast = fy + bh - 1;
            let jh = rr.max(1) + 1;
            let desk = |x: usize, y: usize| {
                let mut w = [wall[y * pw + x]];
                shade_words(&casts, &mut w, y as i32, x as i32);
                w[0]
            };
            if bh <= t + 2 || bw < 4 || ym == 0 {
                edge = false;
            } else {
                let on = frame[ym * pw + fx] == ink && frame[ym * pw + fx + bw - 1] == ink && frame[ylast * pw + mx] == ink;
                let thin = frame[ym * pw + fx + 1] == cf && frame[ym * pw + fx + bw - 2] == cf && frame[(ylast - 1) * pw + mx] == cf;
                let head = jh >= t || (frame[(fy + jh) * pw + fx] == ink && frame[(fy + jh) * pw + fx + 1] != ink);
                let out = fx == 0 || (frame[ym * pw + fx - 1] == desk(fx - 1, ym) && frame[ym * pw + fx - 1] != ink);
                let below = ylast + 1 >= ph || frame[(ylast + 1) * pw + mx] == desk(mx, ylast + 1);
                edge = on && thin && head && out && below;
            }
        }
        // The title's text band is a glyph zone for the black census.
        let ch = Face::Ui.cell_h();
        glyph_rects.push((fx, fy + t.saturating_sub(ch) / 2, bw, ch));
    }
    let (f3, _) = now_ns();
    // The dock: nine tiles, floating 8 above the edge, bottom centre.
    let (cell, pad) = (k(56), k(6));
    let (tile, dot_pad) = (k(48), k(6));
    let dw = 9 * cell + 2 * pad;
    let dh = cell + 2 * pad;
    let (dx, dy) = ((pw - dw) / 2, ph - dh - k(8));
    for j in 0..dh {
        dock_plate_row(&mut row, dx, dy, dw, dh, j, Some(&back));
        for i in 0..9 {
            let tx = pad + i * cell + (cell - tile) / 2;
            let ty = pad + (cell - tile - dot_pad) / 2;
            dock_tile_row(&mut row, dw, j, tx, ty, tile, tile, dot_pad, i == 4, false, i < 3);
        }
        frame[(dy + j) * pw + dx..(dy + j) * pw + dx + dw].copy_from_slice(&row[..dw]);
    }
    let (f4, _) = now_ns();
    // hairline: the plate's top edge at its centre differs from the plate two rows in.
    let cxd = dx + dw / 2;
    if frame[dy * pw + cxd] == frame[(dy + 2) * pw + cxd] || frame[dy * pw + cxd] == wall[dy * pw + cxd] {
        hairline = false;
    }
    // black: no run of pure #000000 longer than 2 px anywhere outside a glyph zone.
    let mut runs = 0usize;
    for y in 0..ph {
        let mut run = 0usize;
        for x in 0..pw {
            let p = frame[y * pw + x] & 0x00FF_FFFF;
            let zone = glyph_rects.iter().any(|&(gx, gy, gw, gh)| x >= gx && x < gx + gw && y >= gy && y < gy + gh);
            if p == 0 && !zone {
                run += 1;
                if run == 3 {
                    runs += 1;
                }
            } else {
                run = 0;
            }
        }
    }
    if runs > 0 {
        black = false;
    }
    // veil (VEILCOST B690): one veiled strip repaint of the bar's box against a solid fill of the same box; the
    // bar's rows end veiled, as the frame had them.
    let face = theme::surface_window();
    let (mut veil_ns, mut fill_ns) = (u64::MAX, u64::MAX);
    let mut vc = VeilCache::new();
    let mut vc_hit = false;
    for _ in 0..3 {
        let (a, _) = now_ns();
        for j in 0..bar_h {
            row[..pw].fill(face);
            frame[j * pw..(j + 1) * pw].copy_from_slice(&row[..pw]);
        }
        let (b, _) = now_ns();
        let mut hit = true;
        for j in 0..bar_h {
            hit &= bar_row_cached(&mut vc, &mut row, 0, 0, pw, bar_h, j, Some(&back));
            frame[j * pw..(j + 1) * pw].copy_from_slice(&row[..pw]);
        }
        let (c, _) = now_ns();
        vc_hit = hit;
        fill_ns = fill_ns.min(b.saturating_sub(a));
        veil_ns = veil_ns.min(c.saturating_sub(b));
    }
    let veil_ok = veil_ns.saturating_mul(100) <= fill_ns.saturating_mul(105).saturating_add(5_000_000);
    drop(vc);
    let veil_s = alloc::format!("{}({}/{})", if veil_ok { "ok" } else { "OVER" }, veil_ns / 1000, fill_ns / 1000);
    // shadow_dark (WINEDGE2 B668): both looks, the margin against the backdrop beside it, on this boot's two bases.
    let bases = live_bases();
    let (sw, sh) = (720usize, 480usize);
    let mut sbuf: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    let (mut sd_n, mut sd_fail): (i64, Option<(bool, Option<DarkRead>)>) = (i64::MIN, None);
    if sbuf.try_reserve_exact(sw * sh).is_ok() {
        sbuf.resize(sw * sh, 0);
        for look_dark in [false, true] {
            match shadow_dark_look(&mut sbuf, sw, sh, look_dark, bases) {
                Some(r) => {
                    sd_n = sd_n.max(r.inner);
                    if !r.ok && sd_fail.is_none() {
                        sd_fail = Some((look_dark, Some(r)));
                    }
                }
                None => {
                    if sd_fail.is_none() {
                        sd_fail = Some((look_dark, None));
                    }
                }
            }
        }
    } else {
        sd_fail = Some((dark, None));
    }
    drop(sbuf);
    let wf = |b: bool| if b { "wall" } else { "flat" };
    let lk = |d: bool| if d { "dark" } else { "light" };
    let shadow_dark_s = match &sd_fail {
        None => alloc::format!("ok(outer=eq inner={:+})", sd_n),
        Some((d, Some(r))) => alloc::format!(
            "FAIL(look={} outer={} inner={:+} bases={}/{})",
            lk(*d), if r.outer_eq { "eq" } else { "ne" }, r.inner, wf(bases.0), wf(bases.1)
        ),
        Some((d, None)) => alloc::format!("FAIL(look={} no-probe)", lk(*d)),
    };
    let ok = corners && shadow && edge && sd_fail.is_none() && hairline && black && veil_ok;
    let fl = |b: bool| if b { "ok" } else { "FAIL" };
    serial_println!(
        ":: DESKTOPCRAFT: corners={} shadow={} edge={} shadow_dark={} hairline={} black={} veil={} windows={} surface={}x{} -> {} ::",
        fl(corners), fl(shadow), if edge { "ok(1px)" } else { "FAIL" }, shadow_dark_s, fl(hairline), fl(black), veil_s, wins.len(), pw, ph, if ok { "PASS" } else { "FAIL" }
    );
    let frame_us = f4.saturating_sub(f0) / 1000;
    // STARFIELD (B661): the shimmer's worst tick over these windows (after every check: it re-tints `wall`).
    let shim_us = scene.as_ref().map_or(0, |sc| shimmer_cost(&sc.shimmer, &mut wall, &mut frame, pw, &fixture_occluders(pw, ph), 30).0);
    const BUDGET_US: u64 = 16_667;
    let s2 = crate::video::dpi::s2();
    let (mt, ms, mb) = margins(ELEV_WINDOW, dark, s2);
    let (it, is, ib) = margins(ELEV_IDLE, dark, s2);
    serial_println!(
        ":: DESKTOPCRAFT: cost clock={} frame_us={} budget_us={} backdrop_us={} chrome_us={} bar_us={} dock_us={} wall_ms={} veil_ms={} shadow_us={} tiles_us={} tiles_kib={} margin={}/{}/{} idle_margin={}/{}/{} ramp_a={}/{} ramp_dark={}/{} frame={} track_fills={} black_runs={} shimmer_us={} veil_us={} fill_us={} veil_cache={} -> {} ::",
        clock,
        frame_us,
        BUDGET_US,
        f1.saturating_sub(f0) / 1000,
        chrome_ns / 1000,
        f2.saturating_sub(fs) / 1000,
        f4.saturating_sub(f3) / 1000,
        t1.saturating_sub(t0) / 1_000_000,
        t2.saturating_sub(t1) / 1_000_000,
        fs.saturating_sub(f1) / 1000,
        (tiles_ns / 1000).max(TILE_US.load(Ordering::Relaxed) as u64),
        tiles_kib,
        mt,
        ms,
        mb,
        it,
        is,
        ib,
        ramp_a.0,
        ramp_a.1,
        ramp_dark.0,
        ramp_dark.1,
        super::BORDER(),
        TRACK_FILLS.load(Ordering::Relaxed),
        runs,
        shim_us,
        veil_ns / 1000,
        fill_ns / 1000,
        if vc_hit { "hit" } else { "miss" },
        if frame_us <= BUDGET_US { "UNDER" } else { "OVER" }
    );
}

// ── SHIMMERIDLE2 (rmbp-ledger B830): the idle tick named, never starved by stranded work, its cost read as the tick ──
//
// Handler: Kernel — wm (the shimmer's idle seat, `shimmer_idle`, extended; no second tick source, no new present).
// Flight 37 boot 1: `skips=6 ticks=0` — the idle seat came due every 33 ms and `idle_why` refused all six with no pass
// between them. The refusal is now NAMED (`why=` on the fixture's detail line), and owed work the gate is free to
// service but no pass is servicing (only ERASE / MENU / DAMAGED, at two consecutive due attempts with no composite
// pass between — [`track`] counts them) is CARRIED: the idle seat runs its one ordinary `wm::composite` anyway, which
// services that work; the next due attempt finds the glass idle and ticks. A busy glass (passes running) still skips.
// The source: the device-service pass (`usb_pump`'s 1-tick nap, a sleeper the tickless idle arms its one-shot to).

/// The refusing terms of [`idle_why`], one bit each.
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
const WHY_GATE: u8 = 1;
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
const WHY_PENDING: u8 = 2;
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
const WHY_ERASE: u8 = 4;
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
const WHY_MENU: u8 = 8;
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
const WHY_DAMAGED: u8 = 16;

/// Composite passes since boot (one relaxed increment at [`track`]'s head).
static TRACK_PASSES: AtomicU32 = AtomicU32::new(0);

/// [`track`]'s head: count the pass.
fn shim_note_pass() {
    TRACK_PASSES.fetch_add(1, Ordering::Relaxed);
}

/// The refusals the idle seat met (OR of `WHY_*`, reset by the fixture), the pass count at the last owed refusal + 1
/// (0 = none), the passes it carried, and the `[shimmer] tick` line said once.
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_IDLE_WHY: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_OWED_AT: AtomicU32 = AtomicU32::new(0);
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_IDLE_CARRIED: AtomicU32 = AtomicU32::new(0);
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_WIRE_SAID: AtomicBool = AtomicBool::new(false);

/// A refusal by `why`: note it, and answer whether the owed work is STRANDED — the gate free (a standing PENDING included, B840), and no
/// composite pass since the previous due attempt that also found work owed. Counts a carry when it is.
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
fn shim_stranded(why: u8) -> bool {
    SHIM_IDLE_WHY.fetch_or(why, Ordering::Relaxed);
    if why & WHY_GATE != 0 { // SHIMMERPRESENT (B840, the seat's ruling): a PENDING with the gate free is strandable — a one-pass holder's COMP_PENDING has only `service_damage` as taker
        SHIM_OWED_AT.store(0, Ordering::Relaxed);
        return false;
    }
    let mark = TRACK_PASSES.load(Ordering::Relaxed).wrapping_add(1).max(1);
    if SHIM_OWED_AT.swap(mark, Ordering::Relaxed) != mark {
        return false;
    }
    SHIM_OWED_AT.store(0, Ordering::Relaxed);
    SHIM_IDLE_CARRIED.fetch_add(1, Ordering::Relaxed);
    true
}

/// `[shimmer] tick src=timer lane=device-service period_ms=<n> backoff_ms=<n> glints=<n> boxes=<n> px=<n> tick_us=<n>`
/// — the named source and the last tick's size and cost.
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
fn shim_wire() {
    serial_println!(
        "[shimmer] tick src=timer lane=device-service period_ms={} backoff_ms={} glints={} boxes={} px={} tick_us={}",
        SHIMMER_MS,
        SHIMMER_MS * SHIM_IDLE_BACKOFF,
        SHIM_TICK_GL.load(Ordering::Relaxed),
        SHIM_TICK_NB.load(Ordering::Relaxed),
        SHIM_TICK_PX.load(Ordering::Relaxed),
        SHIM_TICK_US.load(Ordering::Relaxed)
    );
}

/// The line once per boot, on the idle seat's first tick.
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
fn shim_wire_once() {
    if !SHIM_WIRE_SAID.swap(true, Ordering::Relaxed) {
        shim_wire();
    }
}

/// The last shimmer tick's own cost (us: re-tint + queue) — VEILCOST's `shimmer_idle_us` reads it.
#[cfg(feature = "facet")]
pub fn shimmer_tick_us() -> u64 {
    SHIM_TICK_US.load(Ordering::Relaxed)
}

/// `why=` — the refusing terms joined by `+`, or `none`.
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
struct WhyLabel(u8);
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
impl core::fmt::Display for WhyLabel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == 0 {
            return f.write_str("none");
        }
        let mut sep = "";
        for (b, n) in [(WHY_GATE, "gate"), (WHY_PENDING, "pending"), (WHY_ERASE, "erase"), (WHY_MENU, "menu"), (WHY_DAMAGED, "damaged")] {
            if self.0 & b != 0 {
                f.write_str(sep)?;
                f.write_str(n)?;
                sep = "+";
            }
        }
        Ok(())
    }
}

// ── SHIMMERPRESENT (rmbp-ledger B840): the idle tick's present, named ─────────────────────────────────────────────
//
// Handler: Kernel — wm. `shimmer_idle` presents an idle tick through `wm::present_drain` (the tick, the erase drain and the
// cursor tail under COMP_GATE; the whole pass whenever anything else is owed). The fixture's detail line names the last
// ticked idle present (`present=drain|pass`, `-` before any) and its whole-call cost (`pass_us=`); VEILCOST reads drain
// frames through [`shimmer_present_drain`].

/// The last ticked idle present (0 none, 1 drain, 2 pass) and its whole-call cost (us).
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_PRESENT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
static SHIM_PRESENT_US: AtomicU64 = AtomicU64::new(0);

/// [`shimmer_idle`]'s ticked arm: record which present carried the tick and what the call cost.
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
fn shim_present_note(drained: bool, us: u64) {
    SHIM_PRESENT.store(if drained { 1 } else { 2 }, Ordering::Relaxed);
    SHIM_PRESENT_US.store(us, Ordering::Relaxed);
}

/// Was the last ticked idle present the drain present (VEILCOST's `shimmer_idle_us` reads only those)?
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
pub fn shimmer_present_drain() -> bool {
    SHIM_PRESENT.load(Ordering::Relaxed) == 1
}

/// `present=` — `drain`, `pass`, or `-` before any ticked idle present.
#[cfg(all(target_arch = "x86_64", feature = "facet"))]
fn shim_present_label() -> &'static str {
    match SHIM_PRESENT.load(Ordering::Relaxed) {
        1 => "drain",
        2 => "pass",
        _ => "-",
    }
}

// A11YFILTER (rmbp-ledger B857; BP-video-25, video.md §5 A11YSHADOW B816's owed STALE-only setter) — TAIL-APPENDED so no
// line above moves. The epoch re-lay (`wm::a11yshadow::track`) takes `shadows_relay`: a colour-filter change's own call
// (`a11yfilter::changed`) damages every window right after its bump, so the re-lay of THAT epoch only sets STALE; any
// other epoch move (CONTRAST bumps with no damage of its own and leans on this one) keeps `shadows_stale`'s damage-all.

/// The epoch a colour-filter change's own damage-all covers (`u32::MAX` = none yet).
static COVERED: AtomicU32 = AtomicU32::new(u32::MAX);
/// Re-lays that took the damage-all (`shadows_stale`) since boot (`tests a11yfilters`' `extra_pass=`).
static RELAY_WALKS: AtomicU32 = AtomicU32::new(0);

/// `a11yfilter::changed`: the epoch its bump will make, stored BEFORE the bump (Release), so a pass whose Acquire read
/// of the epoch sees the bump sees this too. A concurrent bump makes it miss: the re-lay then damages (conservative).
pub fn shadows_covered(e: u32) {
    COVERED.store(e, Ordering::Release);
}

/// The epoch re-lay at epoch `e`: STALE alone when a filter change's damage covers `e`, else [`shadows_stale`].
pub(super) fn shadows_relay(e: u32) {
    if COVERED.load(Ordering::Acquire) == e {
        STALE.store(true, Ordering::Release);
    } else {
        RELAY_WALKS.fetch_add(1, Ordering::Relaxed);
        shadows_stale();
    }
}

/// Re-lays that took the damage-all since boot.
pub fn relay_walks() -> u32 {
    RELAY_WALKS.load(Ordering::Relaxed)
}
