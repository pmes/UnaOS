// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm (SOFTY B629: the look's drawing FACADE — rounded rects, hairlines, the glyph strokes, text)
//!
//! SOFTY's one door to pixels. Every Softy painter (the taskbar's rows, the Start menu's and the calendar's
//! surfaces, the window chrome's title band) draws through these few functions and never through a bare loop, so
//! GLASSKIT's `video::draw` can replace the bodies at the fold with no other change (SOFTY.md, "the facade").
//!
//! THE SURFACE. A [`Surf`] is a window of rows `y0 .. y0 + h` of a scene `w` px wide: a whole surface (`y0 = 0`)
//! or ONE scanline of a strip (`h = 1`, `y0 = j`), so the row-composed taskbar and the surface-composed Start
//! menu share every op. Coordinates are the scene's, physical px; every op clips to the window. Colours are the
//! glass's logical `0x00RRGGBB`; `a` is a straight alpha `0..=255` of the op's colour over what is there.
//! No heap, no lock, no I/O: pure integer arithmetic on the caller's slice.
//!
//! ANTI-ALIASING. Edges (corner arcs, discs, the slanted strokes of the close glyph) are covered by a 4x4
//! sub-sample per edge pixel; interiors are whole spans. That is the whole AA model until GLASSKIT's coverage
//! rasteriser lands behind the same names.

/// A window of rows onto a `w`-wide scene. See the module doc.
pub struct Surf<'a> {
    pub px: &'a mut [u32],
    pub w: usize,
    pub y0: usize,
    pub h: usize,
}

/// `(x, y, w, h)` in scene px.
pub type Rect = (usize, usize, usize, usize);

impl<'a> Surf<'a> {
    /// The whole surface `px` (`w` x `h`).
    pub fn whole(px: &'a mut [u32], w: usize, h: usize) -> Self {
        Surf { px, w, y0: 0, h }
    }
    /// Scanline `j` of a strip `w` wide (`out` holds at least `w` words).
    pub fn row(out: &'a mut [u32], w: usize, j: usize) -> Self {
        Surf { px: out, w, y0: j, h: 1 }
    }
    /// The scene rows of `r` this window holds, as a range.
    #[inline]
    fn rows(&self, y: usize, h: usize) -> core::ops::Range<usize> {
        let a = y.max(self.y0);
        let b = (y + h).min(self.y0 + self.h);
        a..b.max(a)
    }
    /// Mix `c` at alpha `a` into scene pixel `(x, y)` (clipped).
    #[inline]
    pub fn mix(&mut self, x: usize, y: usize, c: u32, a: u8) {
        if a == 0 || x >= self.w || y < self.y0 || y >= self.y0 + self.h {
            return;
        }
        let i = (y - self.y0) * self.w + x;
        if let Some(d) = self.px.get_mut(i) {
            *d = blend(*d, c, a);
        }
    }
    #[inline]
    fn span(&mut self, x: usize, y: usize, w: usize, c: u32, a: u8) {
        if a == 0 || x >= self.w || y < self.y0 || y >= self.y0 + self.h {
            return;
        }
        let x1 = (x + w).min(self.w);
        let base = (y - self.y0) * self.w;
        let Some(row) = self.px.get_mut(base + x..base + x1) else { return };
        if a == 255 {
            row.fill(c);
        } else {
            for d in row.iter_mut() {
                *d = blend(*d, c, a);
            }
        }
    }
}

/// `src` over `dst` at straight alpha `a` (`video::font::blend`'s arithmetic: exact at 0 and 255).
#[inline]
pub fn blend(dst: u32, src: u32, a: u8) -> u32 {
    super::super::font::blend(dst, src, a)
}

/// Scale an alpha by a coverage `0..=16` (sixteen sub-samples).
#[inline]
fn cov(a: u8, n: u32) -> u8 {
    ((a as u32 * n + 8) / 16) as u8
}

/// A solid rectangle.
pub fn fill(s: &mut Surf, r: Rect, c: u32, a: u8) {
    for y in s.rows(r.1, r.3) {
        s.span(r.0, y, r.2, c, a);
    }
}

/// Sub-samples (of 16) of corner pixel `(i, j)` that lie INSIDE the quarter disc of radius `rad` whose centre is
/// `(rad, rad)` — the top-left corner's arc; the other three corners mirror into it.
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

/// A rounded rectangle, every corner `rad`, anti-aliased on the arcs.
pub fn rrect(s: &mut Surf, r: Rect, rad: usize, c: u32, a: u8) {
    let (x, y, w, h) = r;
    let rad = rad.min(w / 2).min(h / 2);
    for yy in s.rows(y, h) {
        let j = yy - y;
        let jj = if j < rad { Some(j) } else if j >= h - rad { Some(h - 1 - j) } else { None };
        match jj {
            None => s.span(x, yy, w, c, a),
            Some(jj) => {
                for i in 0..rad {
                    let n = corner_cover(i, jj, rad);
                    s.mix(x + i, yy, c, cov(a, n));
                    s.mix(x + w - 1 - i, yy, c, cov(a, n));
                }
                s.span(x + rad, yy, w - 2 * rad, c, a);
            }
        }
    }
}

/// A rounded rectangle with only its TOP corners rounded (a window's head, a flyout's lip).
pub fn rrect_top(s: &mut Surf, r: Rect, rad: usize, c: u32, a: u8) {
    let (x, y, w, h) = r;
    let rad = rad.min(w / 2).min(h);
    for yy in s.rows(y, h) {
        let j = yy - y;
        if j >= rad {
            s.span(x, yy, w, c, a);
            continue;
        }
        for i in 0..rad {
            let n = corner_cover(i, j, rad);
            s.mix(x + i, yy, c, cov(a, n));
            s.mix(x + w - 1 - i, yy, c, cov(a, n));
        }
        s.span(x + rad, yy, w - 2 * rad, c, a);
    }
}

/// The 1 px outline of a rounded rectangle (the hairline every Softy surface carries), anti-aliased.
pub fn stroke_rrect(s: &mut Surf, r: Rect, rad: usize, c: u32, a: u8) {
    let (x, y, w, h) = r;
    if w < 2 || h < 2 {
        return;
    }
    let rad = rad.min(w / 2).min(h / 2);
    let ri = rad.saturating_sub(1);
    for yy in s.rows(y, h) {
        let j = yy - y;
        if j == 0 || j == h - 1 {
            s.span(x + rad, yy, w - 2 * rad, c, a);
        } else {
            s.mix(x, yy, c, if j >= rad && j < h - rad { a } else { 0 });
            s.mix(x + w - 1, yy, c, if j >= rad && j < h - rad { a } else { 0 });
        }
        let jj = if j < rad { Some(j) } else if j >= h - rad { Some(h - 1 - j) } else { None };
        if let Some(jj) = jj {
            for i in 0..rad {
                let outer = corner_cover(i, jj, rad);
                let inner = if i >= 1 && jj >= 1 && ri > 0 { corner_cover(i - 1, jj - 1, ri) } else { 0 };
                let n = outer.saturating_sub(inner);
                s.mix(x + i, yy, c, cov(a, n));
                s.mix(x + w - 1 - i, yy, c, cov(a, n));
            }
        }
    }
}

/// Paint `c` over the pixels OUTSIDE the top corners' arcs of `r` (the window head's corners are the desktop's).
pub fn cut_top_corners(s: &mut Surf, r: Rect, rad: usize, c: u32) {
    let (x, y, w, h) = r;
    let rad = rad.min(w / 2).min(h);
    for yy in s.rows(y, rad) {
        let j = yy - y;
        for i in 0..rad {
            let n = 16 - corner_cover(i, j, rad);
            s.mix(x + i, yy, c, cov(255, n));
            s.mix(x + w - 1 - i, yy, c, cov(255, n));
        }
    }
}

/// A horizontal hairline (`w` long, 1 px).
pub fn hairline_h(s: &mut Surf, x: usize, y: usize, w: usize, c: u32, a: u8) {
    s.span(x, y, w, c, a);
}

/// A vertical hairline (`h` long, 1 px).
pub fn hairline_v(s: &mut Surf, x: usize, y: usize, h: usize, c: u32, a: u8) {
    for yy in s.rows(y, h) {
        s.mix(x, yy, c, a);
    }
}

/// A filled disc of diameter `d` in the box at `(x, y)`, anti-aliased.
pub fn disc(s: &mut Surf, x: usize, y: usize, d: usize, c: u32, a: u8) {
    rrect(s, (x, y, d, d), d / 2, c, a);
}

/// A stroke of width `t` from `(x0, y0)` to `(x1, y1)` (scene px, 1/8 px precision via the sub-samples):
/// the glyph primitive (the close cross, the chevrons, the power symbol's stem).
pub fn line(s: &mut Surf, x0: usize, y0: usize, x1: usize, y1: usize, t: usize, c: u32, a: u8) {
    let (ax, ay, bx, by) = (8 * x0 as i64 + 4, 8 * y0 as i64 + 4, 8 * x1 as i64 + 4, 8 * y1 as i64 + 4);
    let (dx, dy) = (bx - ax, by - ay);
    let ll = (dx * dx + dy * dy).max(1);
    let half = 4 * t.max(1) as i64;
    let pad = t.max(1) + 1;
    let (lx, hx) = (x0.min(x1).saturating_sub(pad), x0.max(x1) + pad);
    let (ly, hy) = (y0.min(y1).saturating_sub(pad), y0.max(y1) + pad);
    for yy in s.rows(ly, hy - ly + 1) {
        for xx in lx..=hx {
            let mut n = 0;
            for sy in 0..4i64 {
                for sx in 0..4i64 {
                    let (px, py) = (8 * xx as i64 + 2 * sx + 1, 8 * yy as i64 + 2 * sy + 1);
                    let tq = ((px - ax) * dx + (py - ay) * dy).clamp(0, ll);
                    let (qx, qy) = (ax + dx * tq / ll, ay + dy * tq / ll);
                    let (ex, ey) = (px - qx, py - qy);
                    if ex * ex + ey * ey <= half * half {
                        n += 1;
                    }
                }
            }
            s.mix(xx, yy, c, cov(a, n));
        }
    }
}

/// Text with its line TOP at `(x, y)`, clipped at `x + max_w`; returns the pen x. Each covered pixel blends `ink`
/// over what the surface already holds (the glass's text faces, `video::text`).
pub fn text(s: &mut Surf, x: usize, y: usize, max_w: usize, t: &[u8], ink: u32, bold: bool, face: super::super::text::Face) -> usize {
    let n = super::super::text::fit(t, bold, face, max_w);
    let t = &t[..n];
    let ch = face.cell_h();
    let mut pen = x + super::super::text::advance(t, bold, face);
    for yy in s.rows(y, ch) {
        let base = (yy - s.y0) * s.w;
        let Some(row) = s.px.get_mut(base..base + s.w) else { continue };
        super::super::text::draw_row(row, s.w, t, x, yy - y, ink, bold, face);
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

/// Row `j` (scene) of an app's icon from the registrar (`appres`), `size` square at `(x, y)`; `false` when the
/// registrar has no icon for `key` (the caller draws its monogram). Rows only: a whole surface asks per row.
pub fn app_icon(s: &mut Surf, key: &str, x: usize, y: usize, size: usize) -> bool {
    // `dock_tile_row` draws 4/5 of the box it is handed, centred; hand it the box whose 4/5 is `size`.
    let bx = size * 5 / 4;
    let off = (bx - size) / 2;
    let mut drew = true;
    for yy in s.rows(y, size) {
        let base = (yy - s.y0) * s.w;
        let Some(row) = s.px.get_mut(base..base + s.w) else { continue };
        if !crate::fs::appres::dock_tile_row(row, s.w, key, x.saturating_sub(off), y.saturating_sub(off), bx, bx, yy) {
            drew = false;
            break;
        }
    }
    drew
}
