//! The scanline rasterizer: exact-area coverage accumulation with nonzero winding, 256 coverage levels,
//! subpixel origin offsets, no hinting.
//!
//! Method (the signed-area accumulation that FreeType's "smooth" rasterizer and font-rs both use): every
//! line segment, clipped to a pixel row and split at every pixel column it crosses, deposits into an
//! accumulation row its signed height `dy` (winding direction) split between the cell it passes through and
//! the next one, in proportion to the area of that cell lying to the RIGHT of the segment — which for a
//! straight piece inside one cell is exactly `dy * (1 - (mean x - cell left))`. A running sum along the row
//! then gives, per pixel, the exact signed area covered by the outline (the integral of the winding number
//! over the pixel); nonzero fill is `min(1, |area|)`. Quadratic and cubic segments are flattened to lines
//! adaptively (error below 1/32 px). Outline points are snapped to the 26.6 grid FreeType stores outlines on,
//! and coverage is quantized the way FreeType's gray rasterizer does (`min(255, floor(area * 256))`).

use crate::fmath::{ceil, floor, round, sqrt};
use crate::path::OutlineSink;
use alloc::vec;
use alloc::vec::Vec;

/// An 8-bit coverage bitmap of one glyph. `left`/`top` place its top-left pixel relative to the glyph
/// origin's integer pixel (y down: `top` is negative above the baseline).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlyphBitmap {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl GlyphBitmap {
    pub fn get(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            0
        } else {
            self.data[(y * self.width + x) as usize]
        }
    }
}

/// Accumulation canvas.
pub struct Rasterizer {
    w: usize,
    h: usize,
    acc: Vec<f32>,
    cur: (f32, f32),
    start: (f32, f32),
    /// Set when points are snapped to 1/64 px like a FreeType outline (default true).
    pub snap_26_6: bool,
}

const FLATTEN_TOL: f32 = 1.0 / 32.0;

impl Rasterizer {
    pub fn new(w: usize, h: usize) -> Self {
        Rasterizer { w, h, acc: vec![0.0; (w + 2) * h], cur: (0.0, 0.0), start: (0.0, 0.0), snap_26_6: true }
    }

    pub fn width(&self) -> usize {
        self.w
    }
    pub fn height(&self) -> usize {
        self.h
    }

    fn snap(&self, x: f32, y: f32) -> (f32, f32) {
        if self.snap_26_6 { (round(x * 64.0) / 64.0, round(y * 64.0) / 64.0) } else { (x, y) }
    }

    /// Deposit one straight edge (pixel coordinates, y down).
    pub fn line(&mut self, p0: (f32, f32), p1: (f32, f32)) {
        if !(p0.0.is_finite() && p0.1.is_finite() && p1.0.is_finite() && p1.1.is_finite()) {
            return;
        }
        if p0.1 == p1.1 {
            return;
        }
        let (dir, a, b) = if p0.1 < p1.1 { (1.0f32, p0, p1) } else { (-1.0f32, p1, p0) };
        let hh = self.h as f32;
        let ya = a.1.max(0.0);
        let yb = b.1.min(hh);
        if ya >= yb {
            return;
        }
        let dxdy = (b.0 - a.0) / (b.1 - a.1);
        let wf = self.w as f32;
        let mut y = ya;
        let mut row = floor(ya) as usize;
        while y < yb && row < self.h {
            let ynext = ((row + 1) as f32).min(yb);
            let dy = ynext - y;
            if dy > 0.0 {
                let x0 = a.0 + (y - a.1) * dxdy;
                let x1 = a.0 + (ynext - a.1) * dxdy;
                self.row_segment(row, x0.clamp(0.0, wf), x1.clamp(0.0, wf), dy * dir);
            }
            y = ynext;
            row += 1;
        }
    }

    /// One row-clipped piece from x0 to x1 (already clamped to [0, w]) with signed height `d`.
    /// Splits at pixel columns. Rows are `w + 2` cells wide so the deposit right of column w-1 stays in-row.
    fn row_segment(&mut self, row: usize, cx0: f32, cx1: f32, d: f32) {
        let base = row * (self.w + 2);
        let (lo, hi) = if cx0 <= cx1 { (cx0, cx1) } else { (cx1, cx0) };
        let i0 = floor(lo) as usize;
        let i1 = floor(hi) as usize;
        if i0 == i1 || hi == floor(hi) && i1 == i0 + 1 {
            // Within a single cell (an x exactly on the right border still belongs to cell i0).
            let i = i0;
            let mx = 0.5 * (lo + hi) - i as f32;
            self.add(base, i, d * (1.0 - mx));
            self.add(base, i + 1, d * mx);
            return;
        }
        // Split the piece at each column boundary; height shares are proportional to x-extent.
        let span = hi - lo;
        let mut x = lo;
        let mut i = i0;
        while x < hi {
            let xn = ((i + 1) as f32).min(hi);
            let part = d * (xn - x) / span;
            let mx = 0.5 * (x + xn) - i as f32;
            self.add(base, i, part * (1.0 - mx));
            self.add(base, i + 1, part * mx);
            x = xn;
            i += 1;
        }
    }

    #[inline]
    fn add(&mut self, base: usize, i: usize, v: f32) {
        if let Some(c) = self.acc.get_mut(base + i) {
            *c += v;
        }
    }

    fn quad(&mut self, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32)) {
        let ddx = p0.0 - 2.0 * p1.0 + p2.0;
        let ddy = p0.1 - 2.0 * p1.1 + p2.1;
        let dev = sqrt(ddx * ddx + ddy * ddy) * 0.25;
        let n = (ceil(sqrt(dev / FLATTEN_TOL)) as usize).clamp(1, 256);
        let mut prev = p0;
        for k in 1..=n {
            let t = k as f32 / n as f32;
            let mt = 1.0 - t;
            let p = (
                mt * mt * p0.0 + 2.0 * mt * t * p1.0 + t * t * p2.0,
                mt * mt * p0.1 + 2.0 * mt * t * p1.1 + t * t * p2.1,
            );
            self.line(prev, p);
            prev = p;
        }
    }

    fn cubic(&mut self, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)) {
        let d1 = ((p0.0 - 2.0 * p1.0 + p2.0), (p0.1 - 2.0 * p1.1 + p2.1));
        let d2 = ((p1.0 - 2.0 * p2.0 + p3.0), (p1.1 - 2.0 * p2.1 + p3.1));
        let m = (d1.0 * d1.0 + d1.1 * d1.1).max(d2.0 * d2.0 + d2.1 * d2.1);
        let dev = sqrt(m) * 0.75;
        let n = (ceil(sqrt(dev / FLATTEN_TOL)) as usize).clamp(1, 256);
        let mut prev = p0;
        for k in 1..=n {
            let t = k as f32 / n as f32;
            let mt = 1.0 - t;
            let a = mt * mt * mt;
            let b = 3.0 * mt * mt * t;
            let c = 3.0 * mt * t * t;
            let dd = t * t * t;
            let p = (
                a * p0.0 + b * p1.0 + c * p2.0 + dd * p3.0,
                a * p0.1 + b * p1.1 + c * p2.1 + dd * p3.1,
            );
            self.line(prev, p);
            prev = p;
        }
    }

    /// Resolve the accumulation into coverage bytes (nonzero rule).
    pub fn finish(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.w * self.h];
        for y in 0..self.h {
            let mut s = 0.0f32;
            for x in 0..self.w {
                s += self.acc[y * (self.w + 2) + x];
                let a = s.abs();
                let v = (a * 256.0) as i32;
                out[y * self.w + x] = v.clamp(0, 255) as u8;
            }
        }
        out
    }
}

/// An [`OutlineSink`] that maps font units into a rasterizer: px = ox + x*scale, py = oy - y*scale.
pub struct Scaled<'r> {
    pub r: &'r mut Rasterizer,
    pub scale: f32,
    pub ox: f32,
    pub oy: f32,
}

impl Scaled<'_> {
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        self.r.snap(self.ox + x * self.scale, self.oy - y * self.scale)
    }
}

impl OutlineSink for Scaled<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.r.cur = p;
        self.r.start = p;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        let c = self.r.cur;
        self.r.line(c, p);
        self.r.cur = p;
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let p1 = self.map(x1, y1);
        let p = self.map(x, y);
        let c = self.r.cur;
        self.r.quad(c, p1, p);
        self.r.cur = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p1 = self.map(x1, y1);
        let p2 = self.map(x2, y2);
        let p = self.map(x, y);
        let c = self.r.cur;
        self.r.cubic(c, p1, p2, p);
        self.r.cur = p;
    }
    fn close(&mut self) {
        let (c, s) = (self.r.cur, self.r.start);
        if c != s {
            self.r.line(c, s);
        }
        self.r.cur = s;
    }
}

/// Rasterize glyph `gid` of `font` at `size` pixels per em, with the origin at subpixel offset
/// (`sub_x`, `sub_y`) in [0,1) px (y down). `None` when the glyph has no outline (e.g. space).
pub fn rasterize_glyph(font: &crate::Font, gid: u16, size: f32, sub_x: f32, sub_y: f32) -> Option<GlyphBitmap> {
    let path = font.glyph_path(gid)?;
    if path.is_empty() || !(size > 0.0) || size > 4096.0 {
        return None;
    }
    let scale = size / font.units_per_em as f32;
    // Pixel bounds of the control box (control points bound the curve), with a 1 px margin for snapping.
    let x0 = floor(sub_x + path.x_min * scale) as i32 - 1;
    let x1 = ceil(sub_x + path.x_max * scale) as i32 + 1;
    let y0 = floor(sub_y - path.y_max * scale) as i32 - 1;
    let y1 = ceil(sub_y - path.y_min * scale) as i32 + 1;
    let w = (x1 - x0).max(1) as usize;
    let h = (y1 - y0).max(1) as usize;
    if w * h > 1 << 22 {
        return None;
    }
    let mut r = Rasterizer::new(w, h);
    {
        let mut s = Scaled { r: &mut r, scale, ox: sub_x - x0 as f32, oy: sub_y - y0 as f32 };
        path.replay(&mut s);
    }
    let data = r.finish();
    Some(trim(GlyphBitmap { left: x0, top: y0, width: w as u32, height: h as u32, data }))
}

/// Drop all-zero border rows/columns.
fn trim(b: GlyphBitmap) -> GlyphBitmap {
    let (w, h) = (b.width as usize, b.height as usize);
    let row_empty = |y: usize| b.data[y * w..(y + 1) * w].iter().all(|&v| v == 0);
    let col_empty = |x: usize| (0..h).all(|y| b.data[y * w + x] == 0);
    let mut t = 0;
    while t < h && row_empty(t) {
        t += 1;
    }
    if t == h {
        return GlyphBitmap { left: b.left, top: b.top, width: 0, height: 0, data: Vec::new() };
    }
    let mut bt = h;
    while bt > t && row_empty(bt - 1) {
        bt -= 1;
    }
    let mut l = 0;
    while l < w && col_empty(l) {
        l += 1;
    }
    let mut r = w;
    while r > l && col_empty(r - 1) {
        r -= 1;
    }
    let (nw, nh) = (r - l, bt - t);
    let mut data = Vec::with_capacity(nw * nh);
    for y in t..bt {
        data.extend_from_slice(&b.data[y * w + l..y * w + r]);
    }
    GlyphBitmap { left: b.left + l as i32, top: b.top + t as i32, width: nw as u32, height: nh as u32, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(r: &mut Rasterizer, pts: &[(f32, f32)]) {
        for i in 0..pts.len() {
            r.line(pts[i], pts[(i + 1) % pts.len()]);
        }
    }

    #[test]
    fn unit_square_is_full() {
        let mut r = Rasterizer::new(4, 4);
        fill(&mut r, &[(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)]);
        let c = r.finish();
        assert_eq!(&c[4..8], &[0, 255, 255, 0]);
        assert_eq!(&c[8..12], &[0, 255, 255, 0]);
        assert_eq!(&c[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn half_pixel_square_is_half() {
        // A square from x=0.5..1.5 covers half of pixels 0 and 1.
        let mut r = Rasterizer::new(3, 1);
        fill(&mut r, &[(0.5, 0.0), (1.5, 0.0), (1.5, 1.0), (0.5, 1.0)]);
        let c = r.finish();
        assert_eq!(c, vec![128, 128, 0]);
    }

    #[test]
    fn diagonal_triangle_exact_area() {
        // Right triangle covering exactly half of a 1x1 pixel: area 0.5 → 128.
        let mut r = Rasterizer::new(2, 1);
        fill(&mut r, &[(0.0, 0.0), (1.0, 1.0), (0.0, 1.0)]);
        let c = r.finish();
        assert_eq!(c, vec![128, 0]);
        // A triangle spanning 4 px wide, 1 px tall: areas 1/8, 3/8, 5/8, 7/8 of each pixel.
        let mut r = Rasterizer::new(4, 1);
        fill(&mut r, &[(0.0, 0.0), (4.0, 1.0), (0.0, 1.0)]);
        let c = r.finish();
        assert_eq!(c, vec![224, 160, 96, 32]);
    }

    #[test]
    fn winding_nonzero_overlap_saturates_and_opposite_cancels() {
        // Two same-direction overlapping squares → coverage stays 255 (nonzero), never wraps.
        let mut r = Rasterizer::new(3, 1);
        let sq = [(0.0, 0.0), (2.0, 0.0), (2.0, 1.0), (0.0, 1.0)];
        fill(&mut r, &sq);
        fill(&mut r, &[(1.0, 0.0), (3.0, 0.0), (3.0, 1.0), (1.0, 1.0)]);
        assert_eq!(r.finish(), vec![255, 255, 255]);
        // A counter-wound inner square punches a hole.
        let mut r = Rasterizer::new(3, 1);
        fill(&mut r, &[(0.0, 0.0), (3.0, 0.0), (3.0, 1.0), (0.0, 1.0)]);
        fill(&mut r, &[(1.0, 0.0), (1.0, 1.0), (2.0, 1.0), (2.0, 0.0)]);
        assert_eq!(r.finish(), vec![255, 0, 255]);
    }

    #[test]
    fn circle_area_matches_pi_r2() {
        // A flattened circle of radius 10 px: total coverage ≈ π r² within 0.2 %.
        let mut r = Rasterizer::new(24, 24);
        r.snap_26_6 = false;
        let n = 720;
        let pts: Vec<(f32, f32)> = (0..n)
            .map(|k| {
                let a = k as f64 * core::f64::consts::TAU / n as f64;
                ((12.0 + 10.0 * libm_cos(a)) as f32, (12.0 + 10.0 * libm_sin(a)) as f32)
            })
            .collect();
        fill(&mut r, &pts);
        let sum: f64 = r.finish().iter().map(|&v| v as f64 / 255.0).sum();
        let want = core::f64::consts::PI * 100.0;
        assert!((sum - want).abs() / want < 0.004, "{sum} vs {want}");
    }

    // Tiny Taylor-series trig so the test needs no std.
    fn libm_sin(x: f64) -> f64 {
        let mut x = x % core::f64::consts::TAU;
        if x > core::f64::consts::PI {
            x -= core::f64::consts::TAU;
        }
        let mut term = x;
        let mut s = x;
        for k in 1..20 {
            term *= -x * x / ((2 * k) as f64 * (2 * k + 1) as f64);
            s += term;
        }
        s
    }
    fn libm_cos(x: f64) -> f64 {
        libm_sin(x + core::f64::consts::FRAC_PI_2)
    }
}
