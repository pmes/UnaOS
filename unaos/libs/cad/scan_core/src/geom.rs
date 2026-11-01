// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

// SHIM: geom_core (SR464) not on its branch at build time; the seat swaps this file for re-exports
// (`pub use geom_core::{Vec3, Tol, Plane, Transform};` plus adapters where a real name differs).
//! The f64 geometry scan_core uses: ONLY what the crate needs, named as geom_core names it (euclase's
//! `dot` / `cross` / `mag` / `normalize` / `lerp`), so the swap is this one file.

use core::ops::{Add, Mul, Neg, Sub};

/// A point or vector, millimetres (BP-vug-1: f64 in the CAD cores).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };

    pub const fn new(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x, y, z }
    }

    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }

    pub fn mag_sq(self) -> f64 {
        self.dot(self)
    }

    pub fn mag(self) -> f64 {
        libm::sqrt(self.mag_sq())
    }

    /// The unit vector; the zero vector stays zero (no NaN is manufactured).
    pub fn normalize(self) -> Vec3 {
        let m = self.mag();
        if m > 0.0 { self * (1.0 / m) } else { Vec3::ZERO }
    }

    pub fn lerp(self, o: Vec3, t: f64) -> Vec3 {
        self + (o - self) * t
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
    fn mul(self, s: f64) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// The one tolerance type (BP-vug-2): linear in millimetres, angular in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tol {
    pub lin: f64,
    pub ang: f64,
}

impl Tol {
    pub const DEFAULT: Tol = Tol { lin: 1e-6, ang: 1e-9 };
}

/// The plane `normal · p = d` (`normal` unit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub d: f64,
}

impl Plane {
    /// Signed distance of `p` from the plane, along `normal`.
    pub fn distance(&self, p: Vec3) -> f64 {
        self.normal.dot(p) - self.d
    }
}

/// A rigid transform: `p' = rot · p + trans` (`rot` rows, orthonormal).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub rot: [[f64; 3]; 3],
    pub trans: Vec3,
}

impl Transform {
    pub const IDENTITY: Transform =
        Transform { rot: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], trans: Vec3::ZERO };

    /// The frame whose axes are `x`, `y`, `z` (columns) at `origin`: local → world.
    pub fn from_axes(x: Vec3, y: Vec3, z: Vec3, origin: Vec3) -> Transform {
        Transform { rot: [[x.x, y.x, z.x], [x.y, y.y, z.y], [x.z, y.z, z.z]], trans: origin }
    }

    pub fn apply_vector(&self, v: Vec3) -> Vec3 {
        let r = &self.rot;
        Vec3::new(
            r[0][0] * v.x + r[0][1] * v.y + r[0][2] * v.z,
            r[1][0] * v.x + r[1][1] * v.y + r[1][2] * v.z,
            r[2][0] * v.x + r[2][1] * v.y + r[2][2] * v.z,
        )
    }

    pub fn apply_point(&self, p: Vec3) -> Vec3 {
        self.apply_vector(p) + self.trans
    }

    /// The inverse of a rigid transform (transpose rotation, `-rotᵀ · trans`).
    pub fn inverse(&self) -> Transform {
        let r = &self.rot;
        let rt = [[r[0][0], r[1][0], r[2][0]], [r[0][1], r[1][1], r[2][1]], [r[0][2], r[1][2], r[2][2]]];
        let inv = Transform { rot: rt, trans: Vec3::ZERO };
        let t = inv.apply_vector(self.trans);
        Transform { rot: rt, trans: -t }
    }
}
