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
//!   `surface.window` title band with rounded top corners (square when zoomed), the 1-device-px outline
//!   (`line.strong` @ 60 % on Light, `rim.dark` on Dark), the three discs, the centred title and the title rule. The
//!   corner pixels outside the arc are the desktop's (the wallpaper under every shadow that reaches them).
//!   Softy keeps its own painter (`look::is_softy()` decides first).
//! * [`hover_track`] — from `dock::compose` (Una only): the disc group under the pointer, latched; a change damages
//!   the heads involved so their glyphs appear and go.
//! * [`track`] / [`shade_bytes`] / [`shade_words`] — THE SHADOW, both looks (below).
//! * [`bar_row`] / [`dock_plate_row`] / [`dock_tile_row`] — the menu bar's field and the dock's plate and tiles.
//! * [`wall_gen`] / [`veil_gen`] — the default wallpaper and its VEIL (the blurred copy every translucent strip
//!   samples), built once per wallpaper by `video::wallpaper`.
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
//! (the fill is clipped by every box): the 1-px outline and the shadow's edge on the desktop carry the depth.
//!
//! THE TILES. A shadow is never computed per frame (§2.4). Per (radius, level, scale, theme) one [`Tiles`] is
//! rendered once and kept (bounded: [`TILE_SLOTS`]): the top and bottom CORNER tiles (left; the right mirrors) and
//! the three straight RAMPS (top, bottom, side) — a 9-slice of alpha. Each pixel is `1 - prod(1 - A_k F(sd_k))`
//! over the level's layers, `sd_k` the signed distance (quarter px) to the rounded box shifted down `dy_k`, `F` the
//! GLASSKIT smoothstep over `[-blur, +blur]` (half the peak on the edge). A shaded pixel is then one table read per
//! cast that reaches it and one multiply per channel.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
    /// A window's outline on Light: `line.strong` at this opacity (60 %).
    pub outline_alpha: u32,
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
    /// The wallpaper: the dark tone's darkening of `DESKTOP_BG`, the light tone's pull toward the accent, the glow's
    /// peak, its radius in permille of the panel's height and its centre in permille of the panel.
    pub wall_dark: u32,
    pub wall_tint: u32,
    pub glow_alpha: u32,
    pub glow_radius_pm: usize,
    pub glow_x_pm: usize,
    pub glow_y_pm: usize,
}

pub const CRAFT: Craft = Craft {
    win_radius: 12,
    win_radius_softy: 8,
    elev: [
        // elev.1 raised (knobs, the segmented thumb): Light 0 1 2 @ 14 % + 0 0 1 @ 10 %; Dark 0 1 2 @ 40 %
        [[ly(1, 2, 36), ly(0, 1, 26)], [ly(1, 2, 102), NO_LAYER]],
        // elev.2 overlay (menus, popovers, toasts): 0 0 2 @ 12 % + 0 8 24 @ 18 %; Dark @ 30 % / 45 % (+ rim.dark)
        [[ly(0, 2, 31), ly(8, 24, 46)], [ly(0, 2, 77), ly(8, 24, 115)]],
        // elev.3 window, focused: 0 0 2 @ 12 % + 0 16 40 @ 26 % (+ outline); Dark @ 40 % / 55 % (+ rim.dark)
        [[ly(0, 2, 31), ly(16, 40, 66)], [ly(0, 2, 102), ly(16, 40, 140)]],
        // elev.3i window, unfocused: 0 0 2 @ 8 % + 0 8 20 @ 14 %; Dark @ 30 % / 35 %
        [[ly(0, 2, 20), ly(8, 20, 36)], [ly(0, 2, 77), ly(8, 20, 89)]],
    ],
    outline_alpha: 153,
    glyph_alpha: 128,
    veil_tint: (184, 199),
    dock_radius: 18,
    dock_hover: 10,
    dot: 4,
    dot_alpha: 153,
    wall_dark: 90,
    wall_tint: 96,
    glow_alpha: 150,
    glow_radius_pm: 620,
    glow_x_pm: 300,
    glow_y_pm: 760,
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
    let Some(pi) = crate::video::panel_info_nonblocking() else { return };
    let (pw, ph) = (pi.width as i32, pi.height as i32);
    if pw == 0 || ph == 0 {
        return;
    }
    // 1. The casting rows, topmost first (z, id, box, focused, zoomed) — under the table, nothing else held.
    let mut rows: [(u32, super::WinId, (usize, usize, usize, usize), bool); MAX_CASTS] = [(0, 0, (0, 0, 0, 0), false); MAX_CASTS];
    let mut nr = 0usize;
    {
        let shell = super::shell_z();
        let focus = super::focus_asid();
        let t = super::table();
        for r in t.rows.iter() {
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
    // The outline, one device px, run past the band's foot so its lower arcs never land: `line.strong` @ 60 % on
    // Light, `rim.dark` (white @ 8 %) on Dark (§3.1).
    let edge = if theme::is_dark() { Ink::new(theme::RIM_DARK, theme::RIM_DARK_ALPHA) } else { Ink::new(theme::line_strong(), CRAFT.outline_alpha) };
    draw::stroke_rrect(&mut s, Rect::new(0, -jj, bw as i32, t + rad + 2), radii, 1, edge);
    // The rule between the title band and the content.
    if jj == t - 1 {
        draw::hairline_h(&mut s, 0, 0, bw as i32, theme::line_subtle());
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

/// [`bar_row`] over the live wallpaper (the menu bar's compose).
pub fn bar_field_row(out: &mut [u32], x0: usize, y0: usize, w: usize, h: usize, j: usize) {
    let back = live_back();
    bar_row(out, x0, y0, w, h, j, back.as_ref());
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
    dock_plate_row(out, x0, y0, w, h, j, back.as_ref());
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

/// **The default wallpaper** into `px` (`w` x `h`): a soft two-tone diagonal from `DESKTOP_BG` darkened (top left)
/// to `DESKTOP_BG` pulled toward the accent (bottom right), and a large soft disc of the accent low and left of
/// centre — every colour derived from the live tokens, ordered-dithered so an 8-bit panel shows no banding.
/// Integer only; one table of `w` words.
pub fn wall_gen(px: &mut [u32], w: usize, h: usize) {
    if w == 0 || h == 0 || px.len() < w * h {
        return;
    }
    let base = theme::DESKTOP_BG;
    let acc = theme::accent();
    let t0 = draw::blend(base, 0, CRAFT.wall_dark);
    let t1 = draw::blend(base, acc, CRAFT.wall_tint);
    let glow = draw::blend(acc, theme::knob_face(), 48);
    let ch = |c: u32, s: u32| ((c >> s) & 255) as i64;
    let (a0, a1) = ([ch(t0, 16), ch(t0, 8), ch(t0, 0)], [ch(t1, 16), ch(t1, 8), ch(t1, 0)]);
    let gl = [ch(glow, 16), ch(glow, 8), ch(glow, 0)];
    // The diagonal parameter, Q15 per axis (so x + y spans 0 ..= 65536).
    let mut tx = alloc::vec::Vec::new();
    if tx.try_reserve_exact(w).is_err() {
        return;
    }
    tx.extend((0..w).map(|x| ((x as u64 * 32768) / w.max(2) as u64) as i64));
    let rad = (h * CRAFT.glow_radius_pm / 1000).max(1) as i64;
    let (gx, gy) = ((w * CRAFT.glow_x_pm / 1000) as i64, (h * CRAFT.glow_y_pm / 1000) as i64);
    let r2 = rad * rad;
    let inv = (1i64 << 40) / r2.max(1);
    for y in 0..h {
        let ty = (y as i64 * 32768) / h.max(2) as i64;
        let dy = y as i64 - gy;
        let dy2 = dy * dy;
        let in_rows = dy2 < r2;
        let row = &mut px[y * w..(y + 1) * w];
        for (x, p) in row.iter_mut().enumerate() {
            let t = tx[x] + ty; // 0 ..= 65536
            let mut c = [0i64; 3];
            for k in 0..3 {
                c[k] = (a0[k] << 8) + ((a1[k] - a0[k]) * t >> 8);
            }
            if in_rows {
                let dx = x as i64 - gx;
                let d2 = dx * dx + dy2;
                if d2 < r2 {
                    let q = 256 - ((d2 * inv) >> 32).min(256); // 256 at the centre, 0 at the rim
                    let f = q * q * q >> 16; // a smooth falloff, 0 ..= 256
                    let a = f * CRAFT.glow_alpha as i64 >> 8; // 0 ..= alpha (of 256)
                    for k in 0..3 {
                        c[k] += ((gl[k] << 8) - c[k]) * a >> 8;
                    }
                }
            }
            let dth = BAYER[((y & 3) << 2) | (x & 3)] as i64;
            let q = |v: i64| (((v + dth) >> 8).clamp(0, 255)) as u32;
            *p = (q(c[0]) << 16) | (q(c[1]) << 8) | q(c[2]);
        }
    }
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

/// **`tests desktopcraft`** — paints the default desktop into an offscreen surface the size of the rMBP panel
/// (2880x1800; 1440x900 where memory refuses) the way the compositor does: the generated wallpaper and its VEIL,
/// the desktop under every window's shadow (the fill's own shading, [`shade_words`], over the pre-rendered tiles),
/// the menu bar's field, three windows (Lumen, Settings, Aether, bottom to top, Aether focused) painted over it, and
/// the dock's plate with nine tiles. It asserts the invariants and prints the paint cost on the running machine
/// against the 60 Hz frame (16 667 us):
///
/// `:: DESKTOPCRAFT: corners=ok shadow=ok hairline=ok black=ok windows=3 surface=<w>x<h> -> PASS ::`
/// `:: DESKTOPCRAFT: cost clock=<tsc|ms> frame_us=<n> budget_us=16667 backdrop_us=<n> chrome_us=<n> bar_us=<n> dock_us=<n> wall_ms=<n> veil_ms=<n> shadow_us=<n> tiles_us=<n> tiles_kib=<n> margin=<t>/<s>/<b> idle_margin=<t>/<s>/<b> frame=<n> track_fills=<n> black_runs=<n> -> UNDER|OVER ::`
///
/// * **corners** — the focused window's top-left box pixel (outside the rounded head) is the desktop under the
///   shadows (exactly the fill's value there), and the head's face one radius in is not;
/// * **shadow** — below the focused window at mid-width, the darkness (wallpaper minus panel, three channels) is
///   positive on the first row outside the box, never grows going down (within 3), and is (about) nothing at the
///   margin's last row — the shadow lies OUTSIDE the box and fades to zero inside its margin (DESIGNAPPLY B639);
/// * **hairline** — the dock plate's top edge is a line over the plate;
/// * **black** — no run of pure `#000000` longer than 2 px outside a glyph zone.
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
    wall_gen(&mut wall, pw, ph);
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
    let (mut corners, mut shadow, mut hairline, mut black) = (true, true, true, true);
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
            // shadow: below the box at mid-width, the darkness is positive next to the box and falls monotonically
            // to (about) nothing at the margin's last row.
            let mx = fx + bw / 2;
            let dark_of = |a: u32, b: u32| [16u32, 8, 0].iter().map(|&s| ((a >> s) & 255) as i64 - ((b >> s) & 255) as i64).sum::<i64>();
            let bot = casts.iter().find(|c| c.x0 == fx as i32 && c.y0 == fy as i32).map_or(0, |c| c.t.bot) as usize;
            let mut prev = i64::MAX;
            let mut first = 0i64;
            for (n, y) in (fy + bh..(fy + bh + bot).min(ph)).enumerate() {
                let dk = dark_of(wall[y * pw + mx], frame[y * pw + mx]);
                if n == 0 {
                    first = dk;
                }
                if dk > prev + 3 || dk < 0 {
                    shadow = false;
                }
                prev = dk;
            }
            if bot == 0 || first <= 0 || prev > 3 {
                shadow = false;
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
    let ok = corners && shadow && hairline && black;
    let fl = |b: bool| if b { "ok" } else { "FAIL" };
    serial_println!(
        ":: DESKTOPCRAFT: corners={} shadow={} hairline={} black={} windows={} surface={}x{} -> {} ::",
        fl(corners), fl(shadow), fl(hairline), fl(black), wins.len(), pw, ph, if ok { "PASS" } else { "FAIL" }
    );
    let frame_us = f4.saturating_sub(f0) / 1000;
    const BUDGET_US: u64 = 16_667;
    let s2 = crate::video::dpi::s2();
    let (mt, ms, mb) = margins(ELEV_WINDOW, dark, s2);
    let (it, is, ib) = margins(ELEV_IDLE, dark, s2);
    serial_println!(
        ":: DESKTOPCRAFT: cost clock={} frame_us={} budget_us={} backdrop_us={} chrome_us={} bar_us={} dock_us={} wall_ms={} veil_ms={} shadow_us={} tiles_us={} tiles_kib={} margin={}/{}/{} idle_margin={}/{}/{} frame={} track_fills={} black_runs={} -> {} ::",
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
        super::BORDER(),
        TRACK_FILLS.load(Ordering::Relaxed),
        runs,
        if frame_us <= BUDGET_US { "UNDER" } else { "OVER" }
    );
}
