// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Selections in canvas coordinates, their rasterization to a [`Mask`], and the magic wand.
//!
//! Every shape samples pixel CENTRES: pixel `(x, y)` is in when `(x + 0.5, y + 0.5)` is.

use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::document::{Document, Mask};

/// A selection on the canvas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// The cells `x..x+w`, `y..y+h`.
    Rect { x: i32, y: i32, w: i32, h: i32 },
    /// The centres inside the axis-aligned ellipse centred on `(cx, cy)` with radii `rx`, `ry`.
    Ellipse { cx: i32, cy: i32, rx: i32, ry: i32 },
    /// A lasso: the polygon through these vertices, filled even-odd.
    Polygon(Vec<(i32, i32)>),
    /// A coverage plane the canvas's size (the magic wand's answer).
    Coverage(Mask),
}

/// `ceil(t)` without libm: truncation, raised by one when it fell below `t`.
fn ceil_i64(t: f64) -> i64 {
    let i = t as i64;
    if (i as f64) < t { i + 1 } else { i }
}

impl Selection {
    /// True when the selection lies inside a `w x h` canvas (the [`Document::validate`] rule).
    pub fn inside(&self, w: u32, h: u32) -> bool {
        let (w, h) = (w as i64, h as i64);
        match self {
            Selection::Rect { x, y, w: rw, h: rh } => {
                let (x, y, rw, rh) = (*x as i64, *y as i64, *rw as i64, *rh as i64);
                rw > 0 && rh > 0 && x >= 0 && y >= 0 && x + rw <= w && y + rh <= h
            }
            Selection::Ellipse { cx, cy, rx, ry } => {
                let (cx, cy, rx, ry) = (*cx as i64, *cy as i64, *rx as i64, *ry as i64);
                rx > 0 && ry > 0 && cx - rx >= 0 && cy - ry >= 0 && cx + rx <= w && cy + ry <= h
            }
            Selection::Polygon(pts) => {
                pts.len() >= 3
                    && pts.iter().all(|(x, y)| (0..=w).contains(&(*x as i64)) && (0..=h).contains(&(*y as i64)))
            }
            Selection::Coverage(m) => {
                m.width as i64 == w && m.height as i64 == h && m.coverage.len() == (w * h) as usize
            }
        }
    }

    /// The selection as a `w x h` coverage plane (255 in, 0 out; a coverage is copied, clipped / zero-extended).
    pub fn rasterize(&self, w: u32, h: u32) -> Mask {
        let mut m = Mask::filled(w, h, 0);
        let (wi, hi) = (w as i64, h as i64);
        let set = |m: &mut Mask, x: i64, y: i64, v: u8| {
            if x >= 0 && y >= 0 && x < wi && y < hi {
                m.coverage[(y * wi + x) as usize] = v;
            }
        };
        match self {
            Selection::Rect { x, y, w: rw, h: rh } => {
                let (x0, y0) = ((*x as i64).max(0), (*y as i64).max(0));
                let (x1, y1) = ((*x as i64 + *rw as i64).min(wi), (*y as i64 + *rh as i64).min(hi));
                for py in y0..y1 {
                    for px in x0..x1 {
                        set(&mut m, px, py, 255);
                    }
                }
            }
            Selection::Ellipse { cx, cy, rx, ry } => {
                let (cx, cy, rx, ry) = (*cx as i64, *cy as i64, *rx as i64, *ry as i64);
                if rx > 0 && ry > 0 {
                    // ((2x + 1 - 2cx) / 2rx)^2 + ((2y + 1 - 2cy) / 2ry)^2 <= 1, cleared of fractions.
                    let lim = 4 * rx * rx * ry * ry;
                    for py in (cy - ry).max(0)..(cy + ry + 1).min(hi) {
                        let dy = 2 * py + 1 - 2 * cy;
                        for px in (cx - rx).max(0)..(cx + rx + 1).min(wi) {
                            let dx = 2 * px + 1 - 2 * cx;
                            if dx * dx * ry * ry + dy * dy * rx * rx <= lim {
                                set(&mut m, px, py, 255);
                            }
                        }
                    }
                }
            }
            Selection::Polygon(pts) => {
                let n = pts.len();
                if n >= 3 {
                    let mut xs: Vec<f64> = Vec::new();
                    for py in 0..hi {
                        let yc = py as f64 + 0.5;
                        xs.clear();
                        for i in 0..n {
                            let (x0, y0) = (pts[i].0 as f64, pts[i].1 as f64);
                            let (x1, y1) = (pts[(i + 1) % n].0 as f64, pts[(i + 1) % n].1 as f64);
                            // Half-open: an edge counts when the centre line separates its two ends.
                            if (y0 <= yc) != (y1 <= yc) {
                                xs.push(x0 + (yc - y0) * (x1 - x0) / (y1 - y0));
                            }
                        }
                        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
                        for pair in xs.chunks_exact(2) {
                            // Centres with l <= px + 0.5 < r.
                            let first = ceil_i64(pair[0] - 0.5).max(0);
                            let end = ceil_i64(pair[1] - 0.5).min(wi);
                            for px in first..end {
                                set(&mut m, px, py, 255);
                            }
                        }
                    }
                }
            }
            Selection::Coverage(c) => {
                for py in 0..hi.min(c.height as i64) {
                    for px in 0..wi.min(c.width as i64) {
                        set(&mut m, px, py, c.at(px, py));
                    }
                }
            }
        }
        m
    }

    /// The magic wand: a 4-connected flood fill over layer `layer` as it lies on the canvas, seeded at canvas pixel
    /// `(x, y)`. A pixel joins when `max |c - seed|` over its four RGBA channels is `<= tolerance`. The answer is a
    /// canvas-sized [`Selection::Coverage`]; canvas pixels the layer does not cover never join.
    pub fn magic_wand(doc: &Document, layer: usize, x: i32, y: i32, tolerance: u8) -> Result<Selection, Error> {
        let l = doc.layer(layer)?;
        let (w, h) = (doc.width as i64, doc.height as i64);
        let (ox, oy) = (l.offset.0 as i64, l.offset.1 as i64);
        let (x, y) = (x as i64, y as i64);
        if x < 0 || y < 0 || x >= w || y >= h {
            return Err(Error::Selection);
        }
        let seed = l.pixel(x - ox, y - oy).ok_or(Error::Selection)?;
        let near = |p: [u8; 4]| (0..4).all(|i| p[i].abs_diff(seed[i]) <= tolerance);
        let mut m = Mask::filled(doc.width, doc.height, 0);
        let mut seen = vec![false; (w * h) as usize];
        let mut stack: Vec<(i64, i64)> = vec![(x, y)];
        seen[(y * w + x) as usize] = true;
        while let Some((px, py)) = stack.pop() {
            m.coverage[(py * w + px) as usize] = 255;
            for (nx, ny) in [(px - 1, py), (px + 1, py), (px, py - 1), (px, py + 1)] {
                if nx < 0 || ny < 0 || nx >= w || ny >= h || seen[(ny * w + nx) as usize] {
                    continue;
                }
                if let Some(p) = l.pixel(nx - ox, ny - oy) {
                    if near(p) {
                        seen[(ny * w + nx) as usize] = true;
                        stack.push((nx, ny));
                    }
                }
            }
        }
        Ok(Selection::Coverage(m))
    }
}
