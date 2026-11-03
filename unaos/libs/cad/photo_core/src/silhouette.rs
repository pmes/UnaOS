// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Step 4: the silhouette. The seen lattice's bounding box (paper mm) is rectified at `r` px per mm, bilinear
//! through H; a rectified pixel whose photo point is outside the photo is OUT. Rectified pixel (u, v) covers paper
//! [x0 + u / r, x0 + (u + 1) / r) x [y0 + v / r, y0 + (v + 1) / r): v runs UP the paper. Otsu's threshold (IEEE
//! SMC 1979) over the rectified luma (OUT excluded) splits dark from paper. The ink is suppressed by a binary
//! OPENING with a square of side ceil(stroke · r) + 2 (any dark ink thinner than the square goes), then the mask is
//! closed and opened at max(3, round(pitch / 8 · r)) (separable running counts). Dark components are labelled
//! 8-connected (two-pass union-find); the largest is the part ([`Refusal::NoPart`] under (2 · pitch)²;
//! [`Refusal::PartClipped`] when it touches the rectified border or an OUT pixel); the holes are the 4-connected
//! non-part components that touch neither, of at least (pitch / 8)² (the complementary connectivity).

use alloc::vec;
use alloc::vec::Vec;

use crate::edges::Gray;
use crate::homography::{Lattice, apply};
use crate::{Refusal, Vec2};

const NONE: u32 = u32::MAX;

/// The binary silhouette in the rectified frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Silhouette {
    pub x0: f64,
    pub y0: f64,
    /// Rectified px per mm.
    pub r: f64,
    pub w: usize,
    pub h: usize,
    pub part: Vec<bool>,
    /// 0 = not a hole, k = hole k (1-based).
    pub hole: Vec<u32>,
    pub holes: usize,
    /// The Otsu threshold (luma bin; dark is at or below).
    pub threshold: usize,
}

impl Silhouette {
    /// A rectified pixel corner (u, v) in paper mm.
    pub fn corner_mm(&self, u: f64, v: f64) -> Vec2 {
        Vec2::new(self.x0 + u / self.r, self.y0 + v / self.r)
    }
}

/// Otsu's threshold over a 256-bin histogram: the bin maximising the between-class variance (dark = bins ≤ it).
pub fn otsu(hist: &[u64; 256]) -> usize {
    let total: f64 = hist.iter().map(|&c| c as f64).sum();
    let sum: f64 = hist.iter().enumerate().map(|(i, &c)| i as f64 * c as f64).sum();
    let (mut wb, mut sb, mut best, mut var_max) = (0.0, 0.0, 0usize, -1.0);
    for t in 0..256 {
        wb += hist[t] as f64;
        if wb == 0.0 {
            continue;
        }
        let wf = total - wb;
        if wf == 0.0 {
            break;
        }
        sb += t as f64 * hist[t] as f64;
        let (mb, mf) = (sb / wb, (sum - sb) / wf);
        let var = wb * wf * (mb - mf) * (mb - mf);
        if var > var_max {
            var_max = var;
            best = t;
        }
    }
    best
}

fn line_pass(m: &[bool], w: usize, h: usize, rad: usize, erode: bool, horiz: bool) -> Vec<bool> {
    let (lines, len) = if horiz { (h, w) } else { (w, h) };
    let idx = |line: usize, k: usize| if horiz { line * w + k } else { k * w + line };
    let mut out = vec![false; w * h];
    let mut pre = vec![0u32; len + 1];
    for line in 0..lines {
        for k in 0..len {
            pre[k + 1] = pre[k] + m[idx(line, k)] as u32;
        }
        for k in 0..len {
            let lo = k.saturating_sub(rad);
            let hi = (k + rad).min(len - 1);
            let cnt = (pre[hi + 1] - pre[lo]) as usize;
            out[idx(line, k)] = if erode { cnt == hi - lo + 1 } else { cnt > 0 };
        }
    }
    out
}

/// Erode (`erode`) or dilate a mask by the square of side 2 · rad + 1 (the border neither erodes nor dilates).
pub fn morph(m: &[bool], w: usize, h: usize, rad: usize, erode: bool) -> Vec<bool> {
    if rad == 0 {
        return m.to_vec();
    }
    let t = line_pass(m, w, h, rad, erode, true);
    line_pass(&t, w, h, rad, erode, false)
}

pub fn open(m: &[bool], w: usize, h: usize, rad: usize) -> Vec<bool> {
    morph(&morph(m, w, h, rad, true), w, h, rad, false)
}

pub fn close(m: &[bool], w: usize, h: usize, rad: usize) -> Vec<bool> {
    morph(&morph(m, w, h, rad, false), w, h, rad, true)
}

fn find(p: &mut [u32], mut x: u32) -> u32 {
    while p[x as usize] != x {
        p[x as usize] = p[p[x as usize] as usize];
        x = p[x as usize];
    }
    x
}

fn union(p: &mut [u32], a: u32, b: u32) {
    let (ra, rb) = (find(p, a), find(p, b));
    if ra != rb {
        if ra < rb {
            p[rb as usize] = ra;
        } else {
            p[ra as usize] = rb;
        }
    }
}

/// Two-pass union-find labelling: each pixel's component root, NONE outside the mask.
pub fn label(m: &[bool], w: usize, h: usize, eight: bool) -> Vec<u32> {
    let mut p: Vec<u32> = (0..(w * h) as u32).collect();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !m[i] {
                continue;
            }
            if x > 0 && m[i - 1] {
                union(&mut p, i as u32, (i - 1) as u32);
            }
            if y > 0 {
                if m[i - w] {
                    union(&mut p, i as u32, (i - w) as u32);
                }
                if eight && x > 0 && m[i - w - 1] {
                    union(&mut p, i as u32, (i - w - 1) as u32);
                }
                if eight && x + 1 < w && m[i - w + 1] {
                    union(&mut p, i as u32, (i - w + 1) as u32);
                }
            }
        }
    }
    (0..w * h).map(|i| if m[i] { find(&mut p, i as u32) } else { NONE }).collect()
}

/// Rectify, threshold, suppress the ink, take the part and its holes.
pub fn extract(g: &Gray, lat: &Lattice, pitch: f64, line_width_mm: f64, r: f64) -> Result<Silhouette, Refusal> {
    let w = libm::ceil((lat.x1 - lat.x0) * r).max(0.0) as usize;
    let h = libm::ceil((lat.y1 - lat.y0) * r).max(0.0) as usize;
    if w < 3 || h < 3 {
        return Err(Refusal::NoPart);
    }
    let mut luma = vec![0.0f32; w * h];
    let mut out = vec![false; w * h];
    let mut hist = [0u64; 256];
    for v in 0..h {
        for u in 0..w {
            let p = Vec2::new(lat.x0 + (u as f64 + 0.5) / r, lat.y0 + (v as f64 + 0.5) / r);
            let i = v * w + u;
            match apply(&lat.h, p).and_then(|q| g.sample(q.x, q.y)) {
                Some(l) => {
                    luma[i] = l;
                    hist[(l.clamp(0.0, 255.0) as usize).min(255)] += 1;
                }
                None => out[i] = true,
            }
        }
    }
    let threshold = otsu(&hist);
    let dark: Vec<bool> = (0..w * h).map(|i| !out[i] && (luma[i].clamp(0.0, 255.0) as usize) <= threshold).collect();
    let side = libm::ceil(line_width_mm * r).max(0.0) as usize + 2;
    let m = open(&dark, w, h, side / 2);
    let k2 = (libm::round(pitch / 8.0 * r) as usize).max(3);
    let m = open(&close(&m, w, h, k2 / 2), w, h, k2 / 2);
    let lab = label(&m, w, h, true);
    let mut size = vec![0u32; w * h];
    for &l in &lab {
        if l != NONE {
            size[l as usize] += 1;
        }
    }
    let (root, n) = size.iter().enumerate().max_by_key(|(_, s)| **s).map(|(i, s)| (i as u32, *s)).unwrap_or((0, 0));
    let min_part = (2.0 * pitch * r) * (2.0 * pitch * r);
    if (n as f64) < min_part {
        return Err(Refusal::NoPart);
    }
    let part: Vec<bool> = lab.iter().map(|&l| l == root).collect();
    let out_near = |x: usize, y: usize| -> bool {
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h && out[yy as usize * w + xx as usize] {
                    return true;
                }
            }
        }
        false
    };
    for y in 0..h {
        for x in 0..w {
            if part[y * w + x] && (x == 0 || y == 0 || x == w - 1 || y == h - 1 || out_near(x, y)) {
                return Err(Refusal::PartClipped);
            }
        }
    }
    let bg: Vec<bool> = part.iter().map(|&p| !p).collect();
    let blab = label(&bg, w, h, false);
    let mut bsize = vec![0u32; w * h];
    let mut touches = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let l = blab[i];
            if l == NONE {
                continue;
            }
            bsize[l as usize] += 1;
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 || out[i] {
                touches[l as usize] = true;
            }
        }
    }
    let min_hole = ((pitch / 8.0 * r) * (pitch / 8.0 * r)).max(4.0);
    let mut id = vec![0u32; w * h];
    let mut holes = 0u32;
    let mut hole = vec![0u32; w * h];
    for i in 0..w * h {
        let l = blab[i];
        if l == NONE || touches[l as usize] || (bsize[l as usize] as f64) < min_hole {
            continue;
        }
        if id[l as usize] == 0 {
            holes += 1;
            id[l as usize] = holes;
        }
        hole[i] = id[l as usize];
    }
    Ok(Silhouette { x0: lat.x0, y0: lat.y0, r, w, h, part, hole, holes: holes as usize, threshold })
}
