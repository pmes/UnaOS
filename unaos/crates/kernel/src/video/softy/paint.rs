// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm (SOFTY B629 / SOFTY2 B641: the look's drawing FACADE over GLASSKIT's `video::draw`)
//!
//! SOFTY's one door to pixels. Every Softy painter (the taskbar's rows, the flyouts' surfaces, the window chrome's
//! title band) draws through these few functions and never through a bare loop. SOFTY2 (B641) REPOINTED the bodies
//! at GLASSKIT's kit (`video::draw`, B628): coverage-anti-aliased rounded rectangles with per-corner radii, the 1 px
//! stroke, round-capped anti-aliased lines, discs, and — the one Softy could not draw before — the analytic soft
//! drop SHADOW ([`shadow`], DESIGN-LANGUAGE §2.4 `elev.2`, the flyouts'). The names and arguments are the ones the
//! painters always called, so no painter changed shape.
//!
//! THE SURFACE. A [`Surf`] maps SCENE coordinates (physical px, `usize`) onto a buffer `w` wide and `h` rows tall
//! through an origin: a whole surface ([`Surf::whole`], origin 0), ONE scanline of a strip ([`Surf::row`]: scene
//! row `j` is buffer row 0), or a FRAMED surface ([`Surf::framed`]: the body's scene `(0, 0)` sits `m` px inside
//! the buffer, and the band around it holds the body's shadow over what lies beneath, [`under`]). Every op is
//! clipped to the buffer by the kit. Colours are the glass's logical `0x00RRGGBB`; `a` is a straight alpha
//! `0..=255`. No heap, no lock, no I/O.

use crate::video::draw::{self as kit, Ink, Radii};

/// A window onto a buffer. See the module doc.
pub struct Surf<'a> {
    pub px: &'a mut [u32],
    /// The buffer's stride (px).
    pub w: usize,
    /// The buffer's rows.
    pub h: usize,
    /// Scene `(x, y)` is buffer `(x + ox, y + oy)`.
    ox: i32,
    oy: i32,
}

/// `(x, y, w, h)` in scene px.
pub type Rect = (usize, usize, usize, usize);

impl<'a> Surf<'a> {
    /// The whole surface `px` (`w` x `h`).
    pub fn whole(px: &'a mut [u32], w: usize, h: usize) -> Self {
        let h = h.min(px.len() / w.max(1));
        Surf { px, w, h, ox: 0, oy: 0 }
    }
    /// Scanline `j` of a strip `w` wide (`out` holds at least `w` words).
    pub fn row(out: &'a mut [u32], w: usize, j: usize) -> Self {
        let h = (out.len() / w.max(1)).min(1);
        Surf { px: out, w, h, ox: 0, oy: -(j as i32) }
    }
    /// A `w` x `h` buffer whose body starts `m` px in from every edge (a flyout with its shadow band).
    pub fn framed(px: &'a mut [u32], w: usize, h: usize, m: usize) -> Self {
        let h = h.min(px.len() / w.max(1));
        Surf { px, w, h, ox: m as i32, oy: m as i32 }
    }
    /// The kit's view of the same buffer.
    #[inline]
    fn kit(&mut self) -> kit::Surf<'_> {
        let n = (self.w * self.h).min(self.px.len());
        kit::Surf::new(&mut self.px[..n], self.w)
    }
    /// A scene rect in the kit's (buffer) coordinates.
    #[inline]
    fn r(&self, r: Rect) -> kit::Rect {
        kit::Rect::new(r.0 as i32 + self.ox, r.1 as i32 + self.oy, r.2 as i32, r.3 as i32)
    }
    /// The buffer row of scene row `y`, when the buffer holds it.
    #[inline]
    fn brow(&self, y: usize) -> Option<usize> {
        let b = y as i64 + self.oy as i64;
        if b < 0 || b >= self.h as i64 { None } else { Some(b as usize) }
    }
    /// The scene rows `y .. y + h` this buffer holds.
    #[inline]
    fn rows(&self, y: usize, h: usize) -> core::ops::Range<usize> {
        let lo = (-(self.oy as i64)).max(0) as usize;
        let hi = (self.h as i64 - self.oy as i64).max(0) as usize;
        let a = y.max(lo);
        let b = (y + h).min(hi);
        a..b.max(a)
    }
    /// Mix `c` at alpha `a` into scene pixel `(x, y)` (clipped).
    #[inline]
    fn mix(&mut self, x: usize, y: usize, c: u32, a: u8) {
        let bx = x as i64 + self.ox as i64;
        let Some(by) = self.brow(y) else { return };
        if a == 0 || bx < 0 || bx as usize >= self.w {
            return;
        }
        let i = by * self.w + bx as usize;
        if let Some(d) = self.px.get_mut(i) {
            *d = blend(*d, c, a);
        }
    }
}

/// `src` over `dst` at straight alpha `a` (the kit's arithmetic: exact at 0 and 255).
#[inline]
pub fn blend(dst: u32, src: u32, a: u8) -> u32 {
    kit::blend(dst, src, a as u32)
}

/// A solid rectangle.
pub fn fill(s: &mut Surf, r: Rect, c: u32, a: u8) {
    let rr = s.r(r);
    kit::fill(&mut s.kit(), rr, Ink::new(c, a as u32));
}

/// A rounded rectangle, every corner `rad`, coverage-anti-aliased (the kit's `fill_rrect`).
pub fn rrect(s: &mut Surf, r: Rect, rad: usize, c: u32, a: u8) {
    let rr = s.r(r);
    kit::fill_rrect(&mut s.kit(), rr, rad as i32, Ink::new(c, a as u32));
}

/// A rounded rectangle with only its TOP corners rounded (a window's head, a flyout's lip).
pub fn rrect_top(s: &mut Surf, r: Rect, rad: usize, c: u32, a: u8) {
    let rr = s.r(r);
    let rad = rad as i32;
    kit::fill_rrect(&mut s.kit(), rr, Radii::new(rad, rad, 0, 0), Ink::new(c, a as u32));
}

/// The 1 px outline of a rounded rectangle (the hairline every Softy surface carries), anti-aliased.
pub fn stroke_rrect(s: &mut Surf, r: Rect, rad: usize, c: u32, a: u8) {
    let rr = s.r(r);
    kit::stroke_rrect(&mut s.kit(), rr, rad as i32, 1, Ink::new(c, a as u32));
}

/// Sub-samples (of 16) of corner pixel `(i, j)` INSIDE the quarter disc of radius `rad` centred `(rad, rad)`.
#[inline]
fn corner_cover(i: usize, j: usize, rad: usize) -> u32 {
    let (c, r2) = (8 * rad as i64, (8 * rad as i64) * (8 * rad as i64));
    let mut n = 0;
    for sy in 0..4i64 {
        for sx in 0..4i64 {
            let dx = c - (8 * i as i64 + 2 * sx + 1);
            let dy = c - (8 * j as i64 + 2 * sy + 1);
            if dx <= 0 || dy <= 0 || dx * dx + dy * dy <= r2 {
                n += 1;
            }
        }
    }
    n
}

/// Paint `c` over the pixels OUTSIDE the top corners' arcs of `r` (a window head's corners are the desktop's: the
/// compositor's own rule — a window row has no under-sample — and the one op the kit has no "outside" form of).
pub fn cut_top_corners(s: &mut Surf, r: Rect, rad: usize, c: u32) {
    let (x, y, w, h) = r;
    let rad = rad.min(w / 2).min(h);
    for yy in s.rows(y, rad) {
        let j = yy - y;
        for i in 0..rad {
            let n = 16 - corner_cover(i, j, rad);
            let a = ((255 * n + 8) / 16) as u8;
            s.mix(x + i, yy, c, a);
            s.mix(x + w - 1 - i, yy, c, a);
        }
    }
}

/// A horizontal hairline (`w` long, 1 px).
pub fn hairline_h(s: &mut Surf, x: usize, y: usize, w: usize, c: u32, a: u8) {
    let rr = s.r((x, y, w, 1));
    kit::hairline_h(&mut s.kit(), rr.x, rr.y, rr.w, Ink::new(c, a as u32));
}

/// A vertical hairline (`h` long, 1 px).
pub fn hairline_v(s: &mut Surf, x: usize, y: usize, h: usize, c: u32, a: u8) {
    let rr = s.r((x, y, 1, h));
    kit::hairline_v(&mut s.kit(), rr.x, rr.y, rr.h, Ink::new(c, a as u32));
}

/// A filled disc of diameter `d` in the box at `(x, y)`, anti-aliased.
pub fn disc(s: &mut Surf, x: usize, y: usize, d: usize, c: u32, a: u8) {
    let rr = s.r((x, y, d, d));
    kit::fill_disc(&mut s.kit(), rr, Ink::new(c, a as u32));
}

/// A round-capped anti-aliased stroke of width `t` from pixel `(x0, y0)` to pixel `(x1, y1)` (centre to centre):
/// the glyph primitive (the close cross, the chevrons, the power symbol's stem).
#[allow(clippy::too_many_arguments)]
pub fn line(s: &mut Surf, x0: usize, y0: usize, x1: usize, y1: usize, t: usize, c: u32, a: u8) {
    let q = kit::SUB;
    let (ox, oy) = (s.ox, s.oy);
    let at = |v: usize, o: i32| (v as i32 + o) * q + q / 2;
    kit::line_aa(&mut s.kit(), at(x0, ox), at(y0, oy), at(x1, ox), at(y1, oy), t.max(1) as i32 * q, Ink::new(c, a as u32));
}

/// THE SOFT DROP SHADOW of the rounded body `r` (radius `rad`) at elevation level 2 — DESIGN-LANGUAGE §2.4 `elev.2`
/// (menus and popovers: `0 0 2 pt` + `0 8 24 pt`; light 12 % / 18 %, dark 30 % / 45 %, [`super::palette::elev2`]) as
/// two of the kit's analytic falloffs (`draw::shadow_rrect`). The far layer's drop and blur are held inside `m` px
/// (the framed surface's band), so the shadow never leaves the buffer it is drawn into. Drawn BEFORE the body (the
/// caster covers its own footprint — the kit's contract).
pub fn shadow(s: &mut Surf, r: Rect, rad: usize, m: usize) {
    let (near, far) = super::palette::elev2();
    let rr = s.r(r);
    let m = m as i32;
    let px = |n: usize| super::palette::px(n) as i32;
    let drop = (px(8) / 2).min(m / 3).max(0);
    let blur = (px(24) / 2).min(m - drop).max(1);
    let mut ks = s.kit();
    kit::shadow_rrect(&mut ks, rr.offset(0, drop), rad as i32, blur, far);
    kit::shadow_rrect(&mut ks, rr, rad as i32, px(2).min(m).max(1), near);
}

/// What lies beneath a framed surface: the wallpaper's own pixels from panel `(px0, py0)` (the compositor has no
/// under-sample of WINDOWS, so a flyout's corners and shadow fall on the desktop — DESKTOPCRAFT's toast makes the
/// same call), else the desktop colour. Fills the whole buffer; the body is painted over it.
pub fn under(s: &mut Surf, px0: usize, py0: usize) {
    let back = crate::video::wm::desktop::live_back();
    let w = s.w;
    for y in 0..s.h {
        let Some(row) = s.px.get_mut(y * w..(y + 1) * w) else { break };
        match back.as_ref() {
            Some(b) => b.sharp(row, px0, py0 + y),
            None => row.fill(super::palette::DESKTOP),
        }
    }
}

/// SOFTYDARK (B694): one band row's FACE, `w` px from panel `(px0, py)` into `out`: `face` at straight alpha `a` over
/// the wallpaper's VEIL (blurred once per picture, `wm::desktop::Back::soft`; the desktop colour with no wallpaper);
/// at `a == 255` a plain fill, no sample taken. The band's signature carries the wallpaper generation (`strip::seal`).
pub fn veil_row(out: &mut [u32], w: usize, px0: usize, py: usize, face: u32, a: u8) {
    let n = w.min(out.len());
    let row = &mut out[..n];
    if a == 255 {
        row.fill(face);
        return;
    }
    match crate::video::wm::desktop::live_back() {
        Some(b) => b.soft(row, px0, py),
        None => row.fill(super::palette::DESKTOP),
    }
    for p in row.iter_mut() {
        *p = blend(*p, face, a);
    }
}

/// Text with its line TOP at `(x, y)`, clipped at `x + max_w`; returns the pen x. Each covered pixel blends `ink`
/// over what the surface already holds (the glass's text faces, `video::text`).
#[allow(clippy::too_many_arguments)]
pub fn text(s: &mut Surf, x: usize, y: usize, max_w: usize, t: &[u8], ink: u32, bold: bool, face: super::super::text::Face) -> usize {
    let n = super::super::text::fit(t, bold, face, max_w);
    let t = &t[..n];
    let ch = face.cell_h();
    let mut pen = x + super::super::text::advance(t, bold, face);
    let bx = (x as i64 + s.ox as i64).max(0) as usize;
    for yy in s.rows(y, ch) {
        let Some(by) = s.brow(yy) else { continue };
        let w = s.w;
        let Some(row) = s.px.get_mut(by * w..(by + 1) * w) else { continue };
        super::super::text::draw_row(row, w, t, bx, yy - y, ink, bold, face);
    }
    if n == 0 {
        pen = x;
    }
    pen
}

/// The width `t` takes in `face` (physical px).
pub fn text_w(t: &[u8], bold: bool, face: super::super::text::Face) -> usize {
    super::super::text::advance(t, bold, face)
}

/// An app's icon from the registrar (`appres`), `size` square at `(x, y)`; `false` when the registrar has no icon
/// for `key` (the caller draws its monogram). Row by row (the registrar's own face).
pub fn app_icon(s: &mut Surf, key: &str, x: usize, y: usize, size: usize) -> bool {
    // `dock_tile_row` draws 4/5 of the box it is handed, centred; hand it the box whose 4/5 is `size`.
    let bx = size * 5 / 4;
    let off = (bx - size) / 2;
    // The box's x is the buffer's (the row's columns); its y stays the scene's, with `yy` (the registrar reads only
    // the row's offset into the box).
    let tx = (x as i64 + s.ox as i64 - off as i64).max(0) as usize;
    let ty = y.saturating_sub(off);
    let mut drew = true;
    for yy in s.rows(y, size) {
        let Some(by) = s.brow(yy) else { continue };
        let w = s.w;
        let Some(row) = s.px.get_mut(by * w..(by + 1) * w) else { continue };
        if !crate::fs::appres::dock_tile_row(row, w, key, tx, ty, bx, bx, yy) {
            drew = false;
            break;
        }
    }
    drew
}
