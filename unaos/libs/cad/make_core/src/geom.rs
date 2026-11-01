// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! THE GEOM SEAM — the one file through which make_core touches geometry.
//!
//! SHIM until VUGGEOM (LEDGER SR464) folds: `geom_core` was unfolded when make_core was cut, so this file carries the
//! minimum make_core uses under geom_core's names (`Tol { lin, ang }` with `Tol::DEFAULT`, `Vec2`, `Vec3`, `Aabb3`)
//! and four adapters. The seat replaces the bodies with `pub use geom_core::{Tol, Vec2, Vec3, Aabb3}` and adapters
//! calling geom_core's intersections and polygon area; nothing outside this file writes vector math (BP-vug-2).

use core::ops::{Add, Mul, Neg, Sub};
use libm::fabs;

/// The tolerance policy (BP-vug-2): `lin` in millimetres (or the program's unit), `ang` in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tol {
    pub lin: f64,
    pub ang: f64,
}

impl Tol {
    /// geom_core's default (1e-6 mm, 1e-9 rad).
    pub const DEFAULT: Tol = Tol { lin: 1e-6, ang: 1e-9 };
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Vec2 { x, y }
    }
    pub fn dot(self, o: Vec2) -> f64 {
        self.x * o.x + self.y * o.y
    }
    /// The z of the 3D cross product (positive: `o` turns left of `self`).
    pub fn cross(self, o: Vec2) -> f64 {
        self.x * o.y - self.y * o.x
    }
    pub fn length(self) -> f64 {
        libm::hypot(self.x, self.y)
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f64) -> Vec2 {
        Vec2::new(self.x * k, self.y * k)
    }
}
impl Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3 { x, y, z }
    }
    pub fn length(self) -> f64 {
        libm::sqrt(self.x * self.x + self.y * self.y + self.z * self.z)
    }
    pub fn xy(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f64> for Vec3 {
    type Output = Vec3;
    fn mul(self, k: f64) -> Vec3 {
        Vec3::new(self.x * k, self.y * k, self.z * k)
    }
}

/// An axis-aligned box, `min <= max` per axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb3 {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb3 {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Aabb3 { min, max }
    }
    pub fn from_point(p: Vec3) -> Self {
        Aabb3 { min: p, max: p }
    }
    pub fn contains(&self, p: Vec3, tol: Tol) -> bool {
        p.x >= self.min.x - tol.lin
            && p.x <= self.max.x + tol.lin
            && p.y >= self.min.y - tol.lin
            && p.y <= self.max.y + tol.lin
            && p.z >= self.min.z - tol.lin
            && p.z <= self.max.z + tol.lin
    }
    pub fn intersects(&self, o: &Aabb3) -> bool {
        self.min.x <= o.max.x
            && o.min.x <= self.max.x
            && self.min.y <= o.max.y
            && o.min.y <= self.max.y
            && self.min.z <= o.max.z
            && o.min.z <= self.max.z
    }
    pub fn union(&self, o: &Aabb3) -> Aabb3 {
        Aabb3 {
            min: Vec3::new(self.min.x.min(o.min.x), self.min.y.min(o.min.y), self.min.z.min(o.min.z)),
            max: Vec3::new(self.max.x.max(o.max.x), self.max.y.max(o.max.y), self.max.z.max(o.max.z)),
        }
    }
    /// Grown by `d` on every side.
    pub fn expand(&self, d: f64) -> Aabb3 {
        Aabb3 { min: self.min - Vec3::new(d, d, d), max: self.max + Vec3::new(d, d, d) }
    }
}

/// The intersection of the lines `p + t·d` and `q + s·e`; `None` when they are parallel within `tol.ang`.
pub fn line_line2(p: Vec2, d: Vec2, q: Vec2, e: Vec2, tol: Tol) -> Option<Vec2> {
    let den = d.cross(e);
    let (ld, le) = (d.length(), e.length());
    if ld <= 0.0 || le <= 0.0 || fabs(den) <= tol.ang * ld * le {
        return None;
    }
    let t = (q - p).cross(e) / den;
    Some(p + d * t)
}

/// Whether the closed segments `ab` and `cd` meet (touching within `tol.lin` counts).
pub fn seg_seg2(a: Vec2, b: Vec2, c: Vec2, d: Vec2, tol: Tol) -> bool {
    let side = |p: Vec2, q: Vec2, r: Vec2| -> i8 {
        let l = (q - p).length();
        let s = (q - p).cross(r - p);
        if l <= 0.0 || fabs(s) <= tol.lin * l {
            0
        } else if s > 0.0 {
            1
        } else {
            -1
        }
    };
    let on = |p: Vec2, q: Vec2, r: Vec2| -> bool {
        r.x >= p.x.min(q.x) - tol.lin
            && r.x <= p.x.max(q.x) + tol.lin
            && r.y >= p.y.min(q.y) - tol.lin
            && r.y <= p.y.max(q.y) + tol.lin
    };
    let (d1, d2, d3, d4) = (side(a, b, c), side(a, b, d), side(c, d, a), side(c, d, b));
    if d1 * d2 < 0 && d3 * d4 < 0 {
        return true;
    }
    (d1 == 0 && on(a, b, c)) || (d2 == 0 && on(a, b, d)) || (d3 == 0 && on(c, d, a)) || (d4 == 0 && on(c, d, b))
}

/// The signed area of a closed polygon (positive: counter-clockwise).
pub fn signed_area2(pts: &[Vec2]) -> f64 {
    let n = pts.len();
    let mut s = 0.0;
    for i in 0..n {
        s += pts[i].cross(pts[(i + 1) % n]);
    }
    s * 0.5
}

/// Whether the segment `ab` meets the box (the slab test, clipped to `t ∈ [0, 1]`).
pub fn seg_aabb3(a: Vec3, b: Vec3, bx: &Aabb3) -> bool {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let axes = [(a.x, d.x, bx.min.x, bx.max.x), (a.y, d.y, bx.min.y, bx.max.y), (a.z, d.z, bx.min.z, bx.max.z)];
    for (o, dir, lo, hi) in axes {
        if dir == 0.0 {
            if o < lo || o > hi {
                return false;
            }
        } else {
            let (mut ta, mut tb) = ((lo - o) / dir, (hi - o) / dir);
            if ta > tb {
                core::mem::swap(&mut ta, &mut tb);
            }
            t0 = t0.max(ta);
            t1 = t1.min(tb);
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}
