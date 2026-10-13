// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm (DESKTOPCRAFT B631: the Una look's paint — the window chrome and its shadow, the menu bar's
//! VEIL band, the dock's plate and tiles, the procedural wallpaper, and `tests desktopcraft`)
//!
//! DESKTOPCRAFT (rmbp-ledger B631). Peter, 2026-10-14: *"it's giving more a early x-windows vibe opposed to refined
//! mac"* and *"we want people to see our screenshots and be strongly compelled to try unaos"*. Every pixel here is
//! drawn with GLASSKIT (`video::draw`); no geometry is decided here — the boxes, the disc hit boxes, the strip rects
//! and the resize zones are the owners' (`wm`, `dock`, `menubar`), and this module only paints inside them. The
//! design record (every dimension, every colour derivation, the frame cost):
//! `docs/dev/OS/05_USER_EXPERIENCE/DESKTOPCRAFT.md`.
//!
//! Declared at `wm.rs`'s tail with `#[path]` (fbdirect's precedent), so it is `wm::desktop` and reads the window
//! row's fields; `video/mod.rs` does not move. Same gate as `draw` (x86 `wc`, aarch64 `desktop_firmware`).
//!
//! * [`chrome_paint`] — from `wm::paint_window`, the Una arm beside Softy's (one LINE-NEUTRAL fold): the frame band
//!   becomes the wallpaper under the window's soft shadow, the head a rounded title band with the three discs, the
//!   centred title and a hairline. Softy keeps its own painter (`look::is_softy()` decides first).
//! * [`hover_track`] — from `dock::compose` (Una only): the disc group under the pointer, latched; a change damages
//!   the heads involved so their glyphs appear and go.
//! * [`bar_row`] / [`dock_plate_row`] / [`dock_tile_row`] — the menu bar's field and the dock's plate and tiles.
//! * [`wall_gen`] / [`veil_gen`] — the default wallpaper and its VEIL (the blurred copy every translucent strip
//!   samples), built once per wallpaper by `video::wallpaper`.
//! * [`selftest`] — `tests desktopcraft` (R80: a verb, never at boot).
//!
//! THE SHADOW AND THE COMPOSITOR. `wm` paints each window straight into the panel's buffer, inside its own outer
//! box, and repaints only damaged boxes; a pixel outside that box is never repainted when the window presents, so
//! a shadow blended there would darken again on every present. The shadow is therefore drawn INSIDE the box, in
//! the frame band `wm::BORDER` wide that the box already owns: the band shows the wallpaper (never a stale read of
//! the panel) under the analytic falloff of `draw::shadow_rrect`, which reaches zero at the band's outer edge. A
//! shadow wider than the band needs the compositor to damage and re-composite a margin around every box: that is
//! a compositor change, recorded as owed in DESKTOPCRAFT.md, not taken here.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::video::draw::{self, Ink, Radii, Rect, Surf};
use crate::video::text::{self, Face};
use crate::video::theme;

use super::{Ctrl, Window};

// ── the numbers, named once ─────────────────────────────────────────────────────────────────────────────────────

/// The Una look's paint dimensions and strengths. Lengths are LOGICAL (96-ppi) px, scaled by `ui::px` at paint
/// time; strengths are of 255. Proposed values, aligned to DESIGN-LANGUAGE at the fold by editing this one table.
pub struct Craft {
    /// The window's top corners (brief: 10 px at 1x).
    pub win_radius: usize,
    /// The focused window's shadow: blur (logical px) and peak opacity (31 = 12 %).
    pub focus_blur: usize,
    pub focus_alpha: u32,
    /// The unfocused window's shadow: blur and peak (20 = 8 %).
    pub idle_blur: usize,
    pub idle_alpha: u32,
    /// How far the shadow falls below the window (logical px).
    pub drop: usize,
    /// The light title band's top stop: the face this far toward the knob face (a gentle gradient).
    pub title_lift: u32,
    /// An unfocused disc: the face this far toward the ink (a desaturated grey).
    pub idle_disc: u32,
    /// A hovered disc's glyph: the disc's colour this far toward black.
    pub glyph_dark: u32,
    /// A disc's rim: black at this opacity.
    pub disc_rim: u32,
    /// The menu bar's tint over the VEIL (235 = 92 %).
    pub bar_tint: u32,
    /// The dock plate's tint over the VEIL (184 = 72 %).
    pub dock_tint: u32,
    /// The dock plate's corner radius.
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
    win_radius: 10,
    focus_blur: 24,
    focus_alpha: 31,
    idle_blur: 12,
    idle_alpha: 20,
    drop: 1,
    title_lift: 120,
    idle_disc: 56,
    glyph_dark: 140,
    disc_rim: 40,
    bar_tint: 235,
    dock_tint: 184,
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

/// A logical length at the panel's scale, in device px.
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

// ── the window chrome ───────────────────────────────────────────────────────────────────────────────────────────

/// One window's head, as the painter needs it. Every coordinate is BOX-LOCAL device px (the outer box's origin).
pub struct Head<'t> {
    pub bw: usize,
    pub bh: usize,
    pub focused: bool,
    /// The three discs (close, minimise, zoom): `(x, y, diameter)`, from `wm::control_disc` (the hit geometry).
    pub ctrls: [Option<(i32, i32, i32)>; 3],
    /// The pointer is over the disc group: draw the glyphs.
    pub hover: bool,
    pub title: &'t [u8],
}

/// The frame band's width (the box `wm` owns around the head and the content).
#[inline]
fn band() -> usize {
    super::BORDER()
}

/// The shadow of `h`: `(blur, peak, drop)` in device px / of 255. The blur is held inside the band so the falloff
/// reaches zero at the band's outer edge (see the module doc).
fn shadow_of(h: &Head) -> (i32, u32, i32) {
    let b = band() as i32;
    let drop = d(CRAFT.drop).min(b / 3);
    let want = if h.focused { d(CRAFT.focus_blur) } else { d(CRAFT.idle_blur) };
    let blur = want.min(b - drop).max(1);
    (blur, if h.focused { CRAFT.focus_alpha } else { CRAFT.idle_alpha }, drop)
}

/// The window's body (title band + content) in box-local px, and its top radius.
fn body(h: &Head) -> (Rect, i32) {
    let b = band() as i32;
    let r = Rect::new(b, b, h.bw as i32 - 2 * b, h.bh as i32 - 2 * b);
    let rad = d(CRAFT.win_radius).min(r.w / 2).min(super::TITLE_H() as i32);
    (r, rad)
}

/// The title band's two stops: a gentle vertical gradient on Light, flat on Dark.
fn title_stops() -> (u32, u32) {
    let face = theme::chrome_face();
    if theme::is_dark() {
        (face, face)
    } else {
        (draw::blend(face, theme::knob_face(), CRAFT.title_lift), face)
    }
}

/// A disc's fill: the theme's control colour, or the desaturated grey on an unfocused window.
fn disc_fill(which: usize, focused: bool) -> u32 {
    if !focused {
        return draw::blend(theme::chrome_face(), theme::content_text(), CRAFT.idle_disc);
    }
    match which {
        0 => theme::ctrl_close(),
        1 => theme::ctrl_min(),
        _ => theme::ctrl_zoom(),
    }
}

/// The left reserve the title keeps clear of (past the discs) — mirrored on the right so the title stays centred.
fn title_reserve(h: &Head) -> usize {
    let mut r = band() + super::GAP();
    for (x, _, dd) in h.ctrls.iter().flatten() {
        r = r.max((*x + *dd) as usize + super::GAP());
    }
    r
}

/// Compose box-local row `j` of a FULL chrome row (a head row or a bottom band row) into `row[..bw]`.
/// `(bx, by)` is the box's panel origin (the backdrop's coordinates).
pub fn chrome_row(row: &mut [u32], h: &Head, j: usize, bx: usize, by: usize, back: Option<&Back>) {
    let bw = h.bw;
    let row = &mut row[..bw];
    back_sharp(back, row, bx, by + j);
    let (body, rad) = body(h);
    let (blur, peak, drop) = shadow_of(h);
    let jj = j as i32;
    let mut s = Surf::new(row, bw);
    draw::shadow_rrect(&mut s, body.offset(0, drop - jj), rad, blur, peak << 24);
    let t = super::TITLE_H() as i32;
    if jj < body.y || jj >= body.y + t {
        return;
    }
    // The title band: rounded top corners, square at its foot (the content meets it there).
    let band_r = Rect::new(body.x, body.y - jj, body.w, t);
    let radii = Radii::new(rad, rad, 0, 0);
    let (top, bot) = title_stops();
    if top == bot {
        draw::fill_rrect(&mut s, band_r, radii, top);
    } else {
        draw::gradient_rrect_v(&mut s, band_r, radii, top, bot);
    }
    // The outline, one device px at the hairline's opacity, run past the band's foot so its lower arcs never land.
    draw::stroke_rrect(&mut s, Rect::new(body.x, body.y - jj, body.w, t + rad + 2), radii, 1, Ink::argb(theme::hairline()));
    // The hairline between the title band and the content.
    if jj == body.y + t - 1 {
        draw::hairline_h(&mut s, body.x, 0, body.w, Ink::argb(theme::hairline()));
    }
    // The three discs, anti-aliased, with a faint rim; the glyphs while the pointer is over the group.
    for (k, c) in h.ctrls.iter().enumerate() {
        let Some((x, y, dd)) = *c else { continue };
        if jj < y - 1 || jj > y + dd {
            continue;
        }
        let fill = disc_fill(k, h.focused);
        let r = Rect::new(x, y - jj, dd, dd);
        draw::fill_disc(&mut s, r, fill);
        draw::stroke_rrect(&mut s, r, dd / 2, 1, Ink::new(0, CRAFT.disc_rim));
        if h.hover {
            let ink = draw::blend(fill, 0, CRAFT.glyph_dark);
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
                    draw::line_aa(&mut s, cx - g, cy, cx + g, cy, wdt, ink);
                    draw::line_aa(&mut s, cx, cy - g, cx, cy + g, wdt, ink);
                }
            }
        }
    }
    // The title: the UI face, bold, centred on the window (not on the space the discs leave).
    let face = Face::Ui;
    let ch = face.cell_h() as i32;
    let ty = body.y + (t - ch) / 2;
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
    let ink = if h.focused { theme::title_text_active() } else { theme::title_text_inactive() };
    text::draw_row(&mut *s.px, bw, &h.title[..n], tx, (jj - ty) as usize, ink, true, face);
}

/// The side bands' shadow opacity, column by column (`[0, band)` on the left, the right band mirrored into
/// `right`), read off ONE straight row of the kit's own falloff — so a content row costs a table read per pixel
/// and matches the full rows exactly.
fn side_alphas(row: &mut [u32], h: &Head, left: &mut [u8], right: &mut [u8]) {
    let bw = h.bw;
    let b = band();
    let (body, rad) = body(h);
    let (blur, peak, drop) = shadow_of(h);
    let row = &mut row[..bw];
    row.fill(255); // a probe: the blue channel reads `255 - alpha` after the black shadow blends over it
    let mid = (body.y + super::TITLE_H() as i32 + body.bottom()) / 2;
    let mut s = Surf::new(row, bw);
    draw::shadow_rrect(&mut s, body.offset(0, drop - mid), rad, blur, peak << 24);
    for k in 0..b.min(left.len()) {
        left[k] = (255 - (s.px[k] & 255)) as u8;
        right[k] = (255 - (s.px[bw - b + k] & 255)) as u8;
    }
}

/// Paint rows `rows` of a window box through `sink(j, x0, pixels)` (box-local row `j`, first column `x0`): full
/// rows for the head and the bottom band, the two side bands alone for a content row (the content paints its own
/// pixels after this). `row` is a scratch row at least `bw` wide.
pub fn paint_box(h: &Head, bx: usize, by: usize, back: Option<&Back>, row: &mut [u32], rows: core::ops::Range<usize>, mut sink: impl FnMut(usize, usize, &[u32])) {
    let b = band();
    let bw = h.bw;
    if bw < 2 * b + 2 || h.bh < 2 * b + 2 || row.len() < bw {
        return;
    }
    let head = b + super::TITLE_H();
    let mut left = [0u8; 128];
    let mut right = [0u8; 128];
    if b > left.len() {
        return;
    }
    side_alphas(row, h, &mut left, &mut right);
    for j in rows {
        if j >= h.bh {
            break;
        }
        if j < head || j + b >= h.bh {
            chrome_row(row, h, j, bx, by, back);
            sink(j, 0, &row[..bw]);
            continue;
        }
        let py = by + j;
        for k in 0..b {
            let c = back.map_or(theme::DESKTOP_BG, |bk| bk.get(bx + k, py));
            row[k] = draw::blend(c, 0, left[k] as u32);
        }
        sink(j, 0, &row[..b]);
        for k in 0..b {
            let c = back.map_or(theme::DESKTOP_BG, |bk| bk.get(bx + bw - b + k, py));
            row[k] = draw::blend(c, 0, right[k] as u32);
        }
        sink(j, bw - b, &row[..b]);
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
        ctrls: [disc(Ctrl::Close), disc(Ctrl::Minimise), disc(Ctrl::Zoom)],
        hover: ctrls && HOVER.load(Ordering::Relaxed) == r.id,
        title: &r.title[..r.title_len.min(r.title.len())],
    };
    let info = dst.info();
    let (dw, dh) = (info.width, info.height);
    let j0 = oy.saturating_sub(by);
    let j1 = (oy + dh).saturating_sub(by).min(bh);
    if j0 >= j1 {
        return;
    }
    let back = live_back();
    paint_box(&h, bx, by, back.as_ref(), &mut g[..], j0..j1, |j, x0, px| {
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

/// Row `j` of the menu bar's field (`w` x `h` at panel `(x0, y0)`) into `out[..w]`: the face at 92 % over the
/// VEIL, and one hairline where the bar meets the desktop.
pub fn bar_row(out: &mut [u32], x0: usize, y0: usize, w: usize, h: usize, j: usize, back: Option<&Back>) {
    let out = &mut out[..w];
    back_soft(back, out, x0, y0 + j);
    let face = theme::chrome_face();
    for p in out.iter_mut() {
        *p = draw::blend(*p, face, CRAFT.bar_tint);
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
    let outline = theme::title_text_inactive();
    let ink = if percent <= 10 && !charging { theme::error_text() } else { theme::title_text_active() };
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

/// Row `j` of the dock's plate (`w` x `h` at panel `(x0, y0)`) into `out[..w]`: the face at 72 % over the VEIL,
/// rounded at `dock_radius` (the wallpaper itself outside the corners), a hairline round its edge.
pub fn dock_plate_row(out: &mut [u32], x0: usize, y0: usize, w: usize, h: usize, j: usize, back: Option<&Back>) {
    let out = &mut out[..w];
    back_soft(back, out, x0, y0 + j);
    let face = theme::chrome_face();
    for p in out.iter_mut() {
        *p = draw::blend(*p, face, CRAFT.dock_tint);
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
            draw::fill_disc(&mut s, r, Ink::new(theme::content_text(), CRAFT.dot_alpha));
        }
        return;
    }
    if j < y {
        return;
    }
    let r = Rect::new(x as i32, y as i32 - jj, tw as i32, th as i32);
    let rad = draw::icon_radius(tw.min(th)) as i32;
    let face = if pressed { theme::button_face_pressed() } else { theme::button_face() };
    draw::fill_rrect(&mut s, r, rad, face);
    if hover {
        draw::fill_rrect(&mut s, r, rad, Ink::new(theme::content_text(), CRAFT.dock_hover));
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
/// (2880x1800; 1440x900 where memory refuses): the generated wallpaper and its VEIL, the menu bar's field, three
/// windows (Lumen, Settings, Aether, bottom to top, Aether focused) and the dock's plate with nine tiles. It
/// asserts the invariants and prints the paint cost on the running machine against the 60 Hz frame (16 667 us):
///
/// `:: DESKTOPCRAFT: corners=ok shadow=ok hairline=ok black=ok windows=3 surface=<w>x<h> -> PASS ::`
/// `:: DESKTOPCRAFT: cost clock=<tsc|ms> frame_us=<n> budget_us=16667 backdrop_us=<n> chrome_us=<n> bar_us=<n> dock_us=<n> wall_ms=<n> veil_ms=<n> -> UNDER|OVER ::`
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

    // THE FRAME: the backdrop, the bar, three windows, the dock — timed as one.
    let (f0, _) = now_ns();
    frame.copy_from_slice(&wall);
    let (f1, _) = now_ns();
    for j in 0..bar_h {
        bar_row(&mut row, 0, 0, pw, bar_h, j, Some(&back));
        frame[j * pw..(j + 1) * pw].copy_from_slice(&row[..pw]);
    }
    let (f2, _) = now_ns();
    let b = band();
    let t = super::TITLE_H();
    let dd = theme::CONTROL_BOX() as i32;
    let mut chrome_ns = 0u64;
    for fw in wins.iter() {
        let (bw, bh) = (fw.w.min(pw - fw.x), fw.h.min(ph - fw.y));
        let cy = (b + (t.saturating_sub(dd as usize)) / 2) as i32;
        let cx0 = (b + super::GAP()) as i32;
        let g = super::GAP() as i32;
        let h = Head {
            bw,
            bh,
            focused: fw.focused,
            ctrls: [Some((cx0, cy, dd)), Some((cx0 + dd + g, cy, dd)), Some((cx0 + 2 * (dd + g), cy, dd))],
            hover: fw.focused,
            title: fw.title,
        };
        let (c0, _) = now_ns();
        let (fx, fy) = (fw.x, fw.y);
        {
            let fr = &mut frame;
            paint_box(&h, fx, fy, Some(&back), &mut row, 0..bh, |j, x0, px| {
                let o = (fy + j) * pw + fx + x0;
                fr[o..o + px.len()].copy_from_slice(px);
            });
        }
        // The content (a flat document face; the app paints this on the glass).
        let cf = theme::content_fill();
        for y in fy + b + t..fy + bh - b {
            frame[y * pw + fx + b..y * pw + fx + bw - b].fill(cf);
        }
        chrome_ns += now_ns().0.saturating_sub(c0);
        // corners: the top-left body corner pixel is the backdrop (under the shadow only: never the title face).
        let (body_r, rad) = body(&h);
        let (cx, cyy) = (fx + body_r.x as usize, fy + body_r.y as usize);
        let bg = wall[cyy * pw + cx];
        let px = frame[cyy * pw + cx];
        let (_, peak, _) = shadow_of(&h);
        let darker = [16u32, 8, 0].iter().all(|&s| {
            let (p, q) = ((px >> s) & 255, (bg >> s) & 255);
            p <= q && p + 2 >= q - q * peak / 255
        });
        let inside = frame[(cyy + rad as usize) * pw + cx + rad as usize];
        if !darker || inside == px {
            corners = false;
        }
        // shadow: below the body, at mid-width, the darkness (backdrop - pixel, summed over the three channels) is
        // positive next to the body and falls monotonically to (about) nothing at the band's outer edge.
        let mx = fx + bw / 2;
        let dark = |a: u32, b: u32| [16u32, 8, 0].iter().map(|&s| ((a >> s) & 255) as i64 - ((b >> s) & 255) as i64).sum::<i64>();
        let mut prev = i64::MAX;
        let mut first = 0i64;
        for (n, y) in (fy + bh - b..fy + bh).enumerate() {
            let dk = dark(wall[y * pw + mx], frame[y * pw + mx]);
            if n == 0 {
                first = dk;
            }
            if dk > prev + 3 || dk < 0 {
                shadow = false;
            }
            prev = dk;
        }
        if first <= 0 || prev > 3 {
            shadow = false;
        }
        // The title's text band is a glyph zone for the black census.
        let ch = Face::Ui.cell_h();
        glyph_rects.push((fx, fy + b + t.saturating_sub(ch) / 2, bw, ch));
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
    serial_println!(
        ":: DESKTOPCRAFT: cost clock={} frame_us={} budget_us={} backdrop_us={} chrome_us={} bar_us={} dock_us={} wall_ms={} veil_ms={} black_runs={} -> {} ::",
        clock,
        frame_us,
        BUDGET_US,
        f1.saturating_sub(f0) / 1000,
        chrome_ns / 1000,
        f2.saturating_sub(f1) / 1000,
        f4.saturating_sub(f3) / 1000,
        t1.saturating_sub(t0) / 1_000_000,
        t2.saturating_sub(t1) / 1_000_000,
        runs,
        if frame_us <= BUDGET_US { "UNDER" } else { "OVER" }
    );
}
