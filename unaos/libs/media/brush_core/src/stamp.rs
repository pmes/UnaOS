// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A dab's coverage mask. Geometry is 8-bit fixed point (1/256 px); the root is an integer Newton iteration.
//!
//! The radial profile `g(t)` is 1 for `t <= ri` (`ri = r * hardness / 255`), `(r - t) / (r - ri)` between (linear in
//! distance), 0 for `t >= r`. A pixel whose centre lies at distance `d` takes `g` box-filtered over one pixel of
//! distance, `G(d + 1/2) - G(d - 1/2)` with `G` the integral of `g` — the rim's anti-aliasing by sub-pixel distance.
//! For a hard dab (`ri = r`) that is `clamp(1/2 + r - d, 0, 1)`; the filter keeps the profile's area, and `g`
//! non-increasing makes the coverage non-increasing from the centre.

use alloc::vec;
use alloc::vec::Vec;

use crate::stroke::Dab;
use crate::{floor, round};

/// Radii above this are refused as an empty stamp (a `(2 * 4096 + 2)²` mask is ~67 MB; nothing larger is allocated).
pub const MAX_RADIUS: f32 = 4096.0;

/// One fixed-point unit: 1/256 px.
const ONE: i64 = 256;
const HALF: i64 = 128;

/// A dab's coverage over the canvas pixels `x0..x0+w`, `y0..y0+h` (row-major, 0 = none, 255 = full), with the dab's
/// flow carried for [`crate::apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub x0: i32,
    pub y0: i32,
    pub w: u32,
    pub h: u32,
    pub coverage: Vec<u8>,
    pub flow: u8,
}

impl Stamp {
    /// Coverage at canvas pixel `(x, y)`; 0 outside the stamp.
    pub fn at(&self, x: i64, y: i64) -> u8 {
        let (lx, ly) = (x - self.x0 as i64, y - self.y0 as i64);
        if lx < 0 || ly < 0 || lx >= self.w as i64 || ly >= self.h as i64 {
            return 0;
        }
        self.coverage[(ly * self.w as i64 + lx) as usize]
    }
}

/// `floor(sqrt(n))` by Newton's iteration on integers.
pub fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    // A start at or above the root: 2^ceil(bits / 2).
    let mut x = 1u64 << (64 - n.leading_zeros()).div_ceil(2);
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// `G(s)`: the integral of the profile from 0 to `s` (fixed point; `g = 1` below 0 too, so `G(s) = s` there).
#[inline]
fn integral(s: i64, ri: i64, r: i64) -> i64 {
    if s <= ri {
        s
    } else if s >= r {
        ri + (r - ri) / 2
    } else {
        let u = s - ri;
        ri + u - u * u / (2 * (r - ri))
    }
}

/// The coverage mask of `dab` (its `hardness`: 0 = soft from the centre, 255 = hard edge).
pub fn stamp(dab: &Dab) -> Stamp {
    let r = dab.radius;
    let empty = Stamp { x0: 0, y0: 0, w: 0, h: 0, coverage: Vec::new(), flow: dab.flow };
    if !(r > 0.0) || r > MAX_RADIUS || !dab.x.is_finite() || !dab.y.is_finite() {
        return empty;
    }
    if !(dab.x > -1.0e9 && dab.x < 1.0e9 && dab.y > -1.0e9 && dab.y < 1.0e9) {
        return empty; // keeps every box coordinate in i32 range
    }
    let rf = round(r * 256.0);
    if rf <= 0 {
        return empty;
    }
    let ri = rf * dab.hardness as i64 / 255;
    let (cx, cy) = (round(dab.x * 256.0), round(dab.y * 256.0));
    let x0 = floor(dab.x - r - 0.5);
    let y0 = floor(dab.y - r - 0.5);
    let x1 = floor(dab.x + r + 0.5) + 1;
    let y1 = floor(dab.y + r + 0.5) + 1;
    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let mut coverage = vec![0u8; w as usize * h as usize];
    let reach = (rf + HALF) * (rf + HALF);
    for py in 0..h as i64 {
        let dy = (y0 as i64 + py) * ONE + HALF - cy;
        for px in 0..w as i64 {
            let dx = (x0 as i64 + px) * ONE + HALF - cx;
            let d2 = dx * dx + dy * dy;
            if d2 >= reach {
                continue;
            }
            let d = isqrt(d2 as u64) as i64;
            let a = integral(d + HALF, ri, rf) - integral(d - HALF, ri, rf); // 0..=256
            coverage[(py * w as i64 + px) as usize] = ((a.clamp(0, ONE) * 255 + HALF) / ONE) as u8;
        }
    }
    Stamp { x0, y0, w, h, coverage, flow: dab.flow }
}
