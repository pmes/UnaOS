//! The canvas: premultiplied RGBA8 pixmaps, coverage masks, and path filling.
//!
//! Filling goes through **font_core's exact-area scanline rasterizer** (`font_core::raster::Rasterizer`, the
//! same accumulation FONTCORE draws glyphs with — one rasterizer, two users): each flattened edge deposits its
//! signed area into an accumulation row; the running sum is the exact integral of the winding number over each
//! pixel. Nonzero coverage is `min(1, |a|)`; even-odd folds it, `a mod 2` reflected about 1 — the rule
//! FreeType's gray rasterizer uses. Curves are flattened here in device space to 1/20 px.

use crate::fmath::{ceil, floor};
use crate::geom::{Path, Rect};
use alloc::vec;
use alloc::vec::Vec;
use font_core::raster::Rasterizer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

/// Premultiplied RGBA, 8 bits per channel, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Pixmap {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

/// A coverage region: `w × h` bytes placed at (`x`, `y`) on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct Coverage {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

/// A full-canvas alpha mask (clip paths, masks).
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

pub const FLATTEN_TOL: f64 = 0.05;

#[inline]
pub fn div255(v: u32) -> u32 {
    // Exact round(v / 255) for v ≤ 255*255.
    let t = v + 128;
    (t + (t >> 8)) >> 8
}

impl Pixmap {
    pub fn new(w: usize, h: usize) -> Pixmap {
        Pixmap { w, h, data: vec![0; w * h * 4] }
    }

    /// Source-over a premultiplied float colour (each 0..=1) scaled by `k` (0..=1) onto pixel (x, y).
    #[inline]
    pub fn blend(&mut self, x: usize, y: usize, c: [f32; 4], k: f32) {
        let i = (y * self.w + x) * 4;
        let sa = c[3] * k;
        if sa <= 0.0 {
            return;
        }
        let inv = 1.0 - sa;
        let d = &mut self.data[i..i + 4];
        for ch in 0..4 {
            let v = c[ch] * k * 255.0 + d[ch] as f32 * inv;
            d[ch] = (v + 0.5).clamp(0.0, 255.0) as u8;
        }
    }

    /// Composite `src` onto `self` (source-over), each source pixel scaled by `opacity` and by `mask` if given.
    pub fn draw_pixmap(&mut self, src: &Pixmap, opacity: f32, mask: Option<&Mask>) {
        let op = (opacity.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        for p in 0..self.w * self.h {
            let i = p * 4;
            let mut k = op;
            if let Some(m) = mask {
                k = div255(k * m.data[p] as u32);
            }
            if k == 0 {
                continue;
            }
            let s = &src.data[i..i + 4];
            if s[3] == 0 {
                continue;
            }
            let sa = div255(s[3] as u32 * k);
            let inv = 255 - sa;
            let d = &mut self.data[i..i + 4];
            for ch in 0..4 {
                let sv = div255(s[ch] as u32 * k);
                d[ch] = (sv + div255(d[ch] as u32 * inv)).min(255) as u8;
            }
        }
    }

    /// Multiply every pixel by a mask (in place).
    pub fn apply_mask(&mut self, m: &Mask) {
        for p in 0..self.w * self.h {
            let k = m.data[p] as u32;
            if k == 255 {
                continue;
            }
            let d = &mut self.data[p * 4..p * 4 + 4];
            for ch in d.iter_mut() {
                *ch = div255(*ch as u32 * k) as u8;
            }
        }
    }

    /// Straight (non-premultiplied) RGBA bytes.
    pub fn to_straight(&self) -> Vec<u8> {
        let mut out = self.data.clone();
        for px in out.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a == 0 {
                px[0] = 0;
                px[1] = 0;
                px[2] = 0;
            } else if a < 255 {
                for ch in 0..3 {
                    px[ch] = ((px[ch] as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        out
    }
}

impl Mask {
    pub fn new(w: usize, h: usize, fill: u8) -> Mask {
        Mask { w, h, data: vec![fill; w * h] }
    }
    pub fn multiply(&mut self, o: &Mask) {
        for (a, b) in self.data.iter_mut().zip(o.data.iter()) {
            *a = div255(*a as u32 * *b as u32) as u8;
        }
    }
    /// Union a coverage region in: a = a + c − a·c.
    pub fn union_coverage(&mut self, c: &Coverage) {
        for yy in 0..c.h {
            for xx in 0..c.w {
                let v = c.data[yy * c.w + xx] as u32;
                if v == 0 {
                    continue;
                }
                let i = (c.y + yy) * self.w + c.x + xx;
                let a = self.data[i] as u32;
                self.data[i] = (a + v - div255(a * v)).min(255) as u8;
            }
        }
    }
    /// Keep only the inside of an axis-aligned device rectangle (anti-aliased edges).
    pub fn clip_rect(&mut self, r: &Rect) {
        let mut p = Path::new();
        p.move_to(r.x, r.y);
        p.line_to(r.right(), r.y);
        p.line_to(r.right(), r.bottom());
        p.line_to(r.x, r.bottom());
        p.close();
        let mut m = Mask::new(self.w, self.h, 0);
        if let Some(c) = fill_coverage(&p, FillRule::NonZero, self.w, self.h, true) {
            m.union_coverage(&c);
        }
        self.multiply(&m);
    }
}

/// Rasterize a device-space path to coverage, clipped to the `w × h` canvas. `None` if nothing is covered.
pub fn fill_coverage(path: &Path, rule: FillRule, w: usize, h: usize, aa: bool) -> Option<Coverage> {
    let polys = path.flatten(FLATTEN_TOL);
    fill_polys(&polys, rule, w, h, aa)
}

pub fn fill_polys(polys: &[(Vec<(f64, f64)>, bool)], rule: FillRule, w: usize, h: usize, aa: bool) -> Option<Coverage> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (pts, _) in polys {
        for p in pts {
            if !(p.0.is_finite() && p.1.is_finite()) {
                return None;
            }
            x0 = x0.min(p.0);
            y0 = y0.min(p.1);
            x1 = x1.max(p.0);
            y1 = y1.max(p.1);
        }
    }
    if x0 > x1 {
        return None;
    }
    // Left of the canvas still matters for winding (edges clamp onto column 0), so x starts at 0 at least.
    let bx0 = floor(x0).max(0.0) as i64;
    let by0 = floor(y0).max(0.0) as i64;
    let bx1 = (ceil(x1) as i64 + 1).min(w as i64);
    let by1 = (ceil(y1) as i64 + 1).min(h as i64);
    if bx1 <= bx0 || by1 <= by0 || bx0 >= w as i64 || by0 >= h as i64 {
        return None;
    }
    let (cw, ch) = ((bx1 - bx0) as usize, (by1 - by0) as usize);
    let mut r = Rasterizer::new(cw, ch);
    r.snap_26_6 = false;
    let (ox, oy) = (bx0 as f64, by0 as f64);
    for (pts, _) in polys {
        if pts.len() < 2 {
            continue;
        }
        for k in 0..pts.len() {
            let a = pts[k];
            let b = pts[(k + 1) % pts.len()];
            r.line(((a.0 - ox) as f32, (a.1 - oy) as f32), ((b.0 - ox) as f32, (b.1 - oy) as f32));
        }
    }
    let area = r.signed_area();
    let mut data = vec![0u8; cw * ch];
    let mut any = false;
    for (i, &a) in area.iter().enumerate() {
        let mut v = a.abs();
        if rule == FillRule::EvenOdd {
            v %= 2.0;
            if v > 1.0 {
                v = 2.0 - v;
            }
        } else if v > 1.0 {
            v = 1.0;
        }
        let b = if aa { (v * 255.0 + 0.5) as u8 } else if v >= 0.5 { 255 } else { 0 };
        if b != 0 {
            any = true;
        }
        data[i] = b;
    }
    if !any {
        return None;
    }
    Some(Coverage { x: bx0 as usize, y: by0 as usize, w: cw, h: ch, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64, cw: bool) -> Path {
        let mut p = Path::new();
        p.move_to(x, y);
        if cw {
            p.line_to(x + w, y);
            p.line_to(x + w, y + h);
            p.line_to(x, y + h);
        } else {
            p.line_to(x, y + h);
            p.line_to(x + w, y + h);
            p.line_to(x + w, y);
        }
        p.close();
        p
    }

    #[test]
    fn rules_and_clipping() {
        let mut p = rect(-5.0, 0.0, 8.0, 2.0, true);
        p.extend(&rect(1.0, 0.0, 1.0, 2.0, true));
        let nz = fill_coverage(&p, FillRule::NonZero, 4, 2, true).unwrap();
        assert_eq!((nz.x, nz.w), (0, 4));
        assert_eq!(&nz.data[0..4], &[255, 255, 255, 0]);
        let eo = fill_coverage(&p, FillRule::EvenOdd, 4, 2, true).unwrap();
        assert_eq!(&eo.data[0..4], &[255, 0, 255, 0]);
        // Half-pixel edge → 128.
        let h = fill_coverage(&rect(0.5, 0.0, 1.0, 1.0, false), FillRule::NonZero, 3, 1, true).unwrap();
        assert_eq!(h.data, vec![128, 128, 0]);
        assert!(fill_coverage(&rect(10.0, 10.0, 1.0, 1.0, true), FillRule::NonZero, 4, 4, true).is_none());
    }

    #[test]
    fn blending() {
        let mut pm = Pixmap::new(1, 1);
        pm.blend(0, 0, [0.5, 0.0, 0.0, 0.5], 1.0);
        assert_eq!(pm.data, vec![128, 0, 0, 128]);
        pm.blend(0, 0, [0.0, 0.0, 1.0, 1.0], 0.5);
        assert_eq!(pm.data, vec![64, 0, 128, 192]);
        assert_eq!(pm.to_straight(), vec![85, 0, 170, 192]);
        assert_eq!(div255(255 * 255), 255);
        assert_eq!(div255(128 * 255), 128);
    }
}
