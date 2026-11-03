// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Step 1: luma, gradient, edgels. Luma is ITU-R BT.601's Y' = 0.299 R + 0.587 G + 0.114 B on the stored values
//! (0..255). The gradient is Sobel's 3x3 pair divided by 8 (luma units per pixel). Edgels are Canny's non-maximum
//! suppression along the gradient quantised to four directions (Canny, IEEE PAMI 1986), placed to sub-pixel by the
//! parabola through the three magnitudes, kept above a quarter of the 98th-percentile magnitude. Pixel (i, j) covers
//! [i, i + 1) x [j, j + 1): its centre is (i + 0.5, j + 0.5); y runs down the photo.

use alloc::vec;
use alloc::vec::Vec;

use crate::{Image, Vec2};

/// A luma plane.
#[derive(Clone, Debug, PartialEq)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
}

impl Gray {
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.v[y * self.w + x]
    }

    /// Bilinear at a point in pixel coordinates (centres at i + 0.5); None outside the centres' hull.
    pub fn sample(&self, x: f64, y: f64) -> Option<f32> {
        let fx = x - 0.5;
        let fy = y - 0.5;
        if !(fx >= 0.0 && fy >= 0.0) {
            return None;
        }
        let i0 = libm::floor(fx) as usize;
        let j0 = libm::floor(fy) as usize;
        if i0 + 1 >= self.w || j0 + 1 >= self.h {
            return None;
        }
        let ax = (fx - i0 as f64) as f32;
        let ay = (fy - j0 as f64) as f32;
        let a = self.at(i0, j0);
        let b = self.at(i0 + 1, j0);
        let c = self.at(i0, j0 + 1);
        let d = self.at(i0 + 1, j0 + 1);
        Some((a * (1.0 - ax) + b * ax) * (1.0 - ay) + (c * (1.0 - ax) + d * ax) * ay)
    }
}

/// BT.601 luma; None when the RGBA buffer is not `width * height * 4` bytes or the image is empty.
pub fn luma(img: &Image) -> Option<Gray> {
    let (w, h) = (img.width as usize, img.height as usize);
    if w < 3 || h < 3 || img.rgba.len() != w * h * 4 {
        return None;
    }
    let v = img
        .rgba
        .chunks_exact(4)
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect();
    Some(Gray { w, h, v })
}

/// The Sobel gradient (/ 8), zero on the one-pixel border.
#[derive(Clone, Debug, PartialEq)]
pub struct Grad {
    pub w: usize,
    pub h: usize,
    pub gx: Vec<f32>,
    pub gy: Vec<f32>,
}

pub fn sobel(g: &Gray) -> Grad {
    let (w, h) = (g.w, g.h);
    let mut gx = vec![0.0f32; w * h];
    let mut gy = vec![0.0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let p = |dx: isize, dy: isize| g.at((x as isize + dx) as usize, (y as isize + dy) as usize);
            let sx = (p(1, -1) + 2.0 * p(1, 0) + p(1, 1)) - (p(-1, -1) + 2.0 * p(-1, 0) + p(-1, 1));
            let sy = (p(-1, 1) + 2.0 * p(0, 1) + p(1, 1)) - (p(-1, -1) + 2.0 * p(0, -1) + p(1, -1));
            gx[y * w + x] = sx / 8.0;
            gy[y * w + x] = sy / 8.0;
        }
    }
    Grad { w, h, gx, gy }
}

/// An edge point: its sub-pixel position (photo px), the gradient direction (radians, atan2(gy, gx): toward the
/// brighter side) and the gradient magnitude.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edgel {
    pub p: Vec2,
    pub theta: f64,
    pub mag: f64,
}

/// The fraction of the 98th-percentile magnitude an edgel must reach.
pub const EDGE_FRACTION: f32 = 0.25;

/// The edgels of a luma plane.
pub fn edgels(g: &Gray) -> Vec<Edgel> {
    let gr = sobel(g);
    let (w, h) = (g.w, g.h);
    let mag: Vec<f32> = gr.gx.iter().zip(&gr.gy).map(|(a, b)| libm::sqrtf(a * a + b * b)).collect();
    let mut nz: Vec<f32> = mag.iter().copied().filter(|m| *m > 0.0).collect();
    if nz.is_empty() {
        return Vec::new();
    }
    let k = ((nz.len() as f64 * 0.98) as usize).min(nz.len() - 1);
    let (_, p98, _) = nz.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
    let thr = *p98 * EDGE_FRACTION;
    if !(thr > 0.0) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            let i = y * w + x;
            let m = mag[i];
            if m < thr {
                continue;
            }
            let (ax, ay) = (gr.gx[i], gr.gy[i]);
            // The gradient's direction quantised to 0 / 45 / 90 / 135 degrees (y down).
            let ang = libm::atan2f(ay, ax);
            let mut deg = ang * (180.0 / core::f32::consts::PI);
            if deg < 0.0 {
                deg += 180.0;
            }
            let (dx, dy): (isize, isize) = if !(22.5..157.5).contains(&deg) {
                (1, 0)
            } else if deg < 67.5 {
                (1, 1)
            } else if deg < 112.5 {
                (0, 1)
            } else {
                (-1, 1)
            };
            let m1 = mag[(y as isize - dy) as usize * w + (x as isize - dx) as usize];
            let m2 = mag[(y as isize + dy) as usize * w + (x as isize + dx) as usize];
            if !(m > m1 && m >= m2) {
                continue;
            }
            let den = m1 - 2.0 * m + m2;
            let d = if den < 0.0 { ((m1 - m2) / (2.0 * den)).clamp(-0.5, 0.5) } else { 0.0 };
            let p = Vec2::new(x as f64 + 0.5 + d as f64 * dx as f64, y as f64 + 0.5 + d as f64 * dy as f64);
            out.push(Edgel { p, theta: ang as f64, mag: m as f64 });
        }
    }
    out
}
