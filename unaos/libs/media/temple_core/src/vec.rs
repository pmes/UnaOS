// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// TEMPLE (SR147): the 3-vector (positions, directions and linear-light colours alike) and the few transcendental
// helpers the scene needs, all written over `Real` so the Q16.16 and f32 builds trace the same mathematics.

use crate::real::{k, Real};
use core::ops::{Add, Mul, Neg, Sub};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct V3<R> {
    pub x: R,
    pub y: R,
    pub z: R,
}

impl<R: Real> V3<R> {
    #[inline(always)]
    pub fn new(x: R, y: R, z: R) -> Self {
        V3 { x, y, z }
    }
    #[inline(always)]
    pub fn zero() -> Self {
        V3 { x: R::zero(), y: R::zero(), z: R::zero() }
    }
    #[inline(always)]
    pub fn kv(x: f32, y: f32, z: f32) -> Self {
        V3 { x: k(x), y: k(y), z: k(z) }
    }
    #[inline(always)]
    pub fn dot(self, o: Self) -> R {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    #[inline(always)]
    pub fn cross(self, o: Self) -> Self {
        V3 { x: self.y * o.z - self.z * o.y, y: self.z * o.x - self.x * o.z, z: self.x * o.y - self.y * o.x }
    }
    #[inline(always)]
    pub fn len2(self) -> R {
        self.dot(self)
    }
    #[inline(always)]
    pub fn norm(self) -> Self {
        let l = self.len2().sqrt();
        if l <= R::zero() {
            return self;
        }
        let inv = R::one() / l;
        self * inv
    }
    /// Component-wise product (colour filters).
    #[inline(always)]
    pub fn hadamard(self, o: Self) -> Self {
        V3 { x: self.x * o.x, y: self.y * o.y, z: self.z * o.z }
    }
    #[inline(always)]
    pub fn ch(self, i: usize) -> R {
        match i {
            0 => self.x,
            1 => self.y,
            _ => self.z,
        }
    }
}

impl<R: Real> Add for V3<R> {
    type Output = Self;
    #[inline(always)]
    fn add(self, o: Self) -> Self {
        V3 { x: self.x + o.x, y: self.y + o.y, z: self.z + o.z }
    }
}
impl<R: Real> Sub for V3<R> {
    type Output = Self;
    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        V3 { x: self.x - o.x, y: self.y - o.y, z: self.z - o.z }
    }
}
impl<R: Real> Mul<R> for V3<R> {
    type Output = Self;
    #[inline(always)]
    fn mul(self, s: R) -> Self {
        V3 { x: self.x * s, y: self.y * s, z: self.z * s }
    }
}
impl<R: Real> Neg for V3<R> {
    type Output = Self;
    #[inline(always)]
    fn neg(self) -> Self {
        V3 { x: -self.x, y: -self.y, z: -self.z }
    }
}

/// `(sin a, cos a)` for an angle in milliradians (any i32): integer range reduction to [-pi, pi], the symmetry fold
/// to [-pi/2, pi/2], then odd/even Taylor terms to x^9 / x^10 (error < 4e-6 there).
pub fn sin_cos_mrad<R: Real>(mrad: i32) -> (R, R) {
    // 2*pi in micro-radians keeps the reduction exact enough for a slow orbit that runs for hours.
    let two_pi_u: i64 = 6_283_185;
    let mut u = (mrad as i64 * 1000) % two_pi_u;
    if u > two_pi_u / 2 {
        u -= two_pi_u;
    }
    if u < -two_pi_u / 2 {
        u += two_pi_u;
    }
    let half_pi_u = two_pi_u / 4;
    // Fold: sin(pi - x) = sin x, cos(pi - x) = -cos x.
    let (u, cos_sign) = if u > half_pi_u {
        (two_pi_u / 2 - u, -1)
    } else if u < -half_pi_u {
        (-two_pi_u / 2 - u, -1)
    } else {
        (u, 1)
    };
    let x: R = R::from_q16(((u << 16) / 1_000_000) as i32);
    let x2 = x * x;
    let s = x * (R::one() - x2 * (k::<R>(1.0 / 6.0) - x2 * (k::<R>(1.0 / 120.0) - x2 * (k::<R>(1.0 / 5040.0) - x2 * k::<R>(1.0 / 362_880.0)))));
    let c = R::one() - x2 * (k::<R>(0.5) - x2 * (k::<R>(1.0 / 24.0) - x2 * (k::<R>(1.0 / 720.0) - x2 * (k::<R>(1.0 / 40_320.0) - x2 * k::<R>(1.0 / 3_628_800.0)))));
    (s, if cos_sign < 0 { -c } else { c })
}

/// `exp(-x)` for x >= 0, at table-build time only: `exp(-x/16)^16`, the inner term by Taylor to the 6th power.
pub fn exp_neg_slow<R: Real>(x: R) -> R {
    if x <= R::zero() {
        return R::one();
    }
    let y = x * k::<R>(1.0 / 16.0);
    let e = R::one() - y * (R::one() - y * (k::<R>(0.5) - y * (k::<R>(1.0 / 6.0) - y * (k::<R>(1.0 / 24.0) - y * (k::<R>(1.0 / 120.0) - y * k::<R>(1.0 / 720.0))))));
    let e2 = e * e;
    let e4 = e2 * e2;
    let e8 = e4 * e4;
    e8 * e8
}

/// The run-time `exp(-x)`: 257 samples over x in [0, 16) at 1/16 steps, linearly interpolated (x past the end is 0).
pub struct ExpTable<R> {
    t: [R; 257],
}

impl<R: Real> ExpTable<R> {
    pub fn new() -> Self {
        let mut t = [R::zero(); 257];
        let mut i = 0;
        while i < 257 {
            t[i] = exp_neg_slow(R::from_q16((i as i32) << 12));
            i += 1;
        }
        t[256] = R::zero();
        ExpTable { t }
    }
    #[inline(always)]
    pub fn exp_neg(&self, x: R) -> R {
        let q = x.to_q16();
        if q <= 0 {
            return R::one();
        }
        if q >= 16 << 16 {
            return R::zero();
        }
        let i = (q >> 12) as usize;
        let f = R::from_q16((q & 0xFFF) << 4);
        self.t[i] + (self.t[i + 1] - self.t[i]) * f
    }
}

impl<R: Real> Default for ExpTable<R> {
    fn default() -> Self {
        Self::new()
    }
}
