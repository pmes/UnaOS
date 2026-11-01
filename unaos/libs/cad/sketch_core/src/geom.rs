// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! SHIM (VUGSKETCH SR465) for `geom_core` (VUG-GEOM SR464, cut beside and not yet committed when this crate was
//! built). It holds ONLY the four names sketch_core takes from geom_core — `Tol`, `Vec2`, `Arc2`, `area` — spelled as
//! VUGGEOM's launch names them. The fold swaps it out: `mod geom;` -> `use geom_core as geom;` in lib.rs, the
//! geom_core path dependency in Cargo.toml, and this file deleted. Nothing else in the crate names geom_core.

/// The one tolerance policy (BP-vug-2): linear in millimetres, angular in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tol {
    pub lin: f64,
    pub ang: f64,
}

impl Tol {
    /// 1e-6 mm and 1e-9 rad (geom_core states the reasoning; the shim only carries the values).
    pub const DEFAULT: Tol = Tol { lin: 1e-6, ang: 1e-9 };
}

/// A 2D point or vector, f64 (BP-vug-1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Vec2 { x, y }
    }
}

/// A circular arc: centre, radius, start angle and signed sweep (radians; positive = CCW).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc2 {
    pub centre: Vec2,
    pub radius: f64,
    pub start: f64,
    pub sweep: f64,
}

/// The signed area of a closed polygon (shoelace; CCW positive).
pub fn area(pts: &[Vec2]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        s += a.x * b.y - b.x * a.y;
    }
    0.5 * s
}
