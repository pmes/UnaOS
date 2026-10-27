// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// TEMPLE (SR147): the scalar the renderer is written over. Two backends, one scene:
//
//   `Fx`  — Q16.16 in an i32, every op integer (i64 products, saturating). It is the bit-exact reference: the
//           same bits on every machine, so the host's 64x64 checksum is any build's checksum, and it is the scalar
//           of the soft-float profile (the kernel's x86_64-unaos.json, `+soft-float`). Its square root takes the
//           exact SSE2 path where the target has SSE2 and `u64::isqrt` where it does not: bit-identical (B876).
//   `f32` — hardware float where the program owns an FPU: the host, x86 ring 3 (B666: unaos/x86_64-unknown-none.json
//           is `+sse,+sse2`, the FPU context saved eagerly per switch, so f32 is the vessel's scalar on x86), and
//           aarch64 ring 3 (hard-float, aarch64-base.json).
//
// Constants enter through `k(f32)`: on the soft-float target the literal folds at compile time (LLVM folds the
// IR-level fmul/fptosi before soft-float lowering), so no float op survives into the Fx program's hot paths.

use core::ops::{Add, Div, Mul, Neg, Sub};

pub trait Real: Copy + PartialOrd + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self> + Neg<Output = Self> {
    const NAME: &'static str;
    /// A constant.
    fn k(x: f32) -> Self;
    /// `q / 65536`.
    fn from_q16(q: i32) -> Self;
    /// `self * 65536`, saturating.
    fn to_q16(self) -> i32;
    /// The square root (0 for a non-positive input).
    fn sqrt(self) -> Self;
    /// `a * b > c * e`, computed exactly where the representation allows (the polyhedron clip compares fractions
    /// instead of dividing per plane).
    fn prod_gt(a: Self, b: Self, c: Self, e: Self) -> bool;
    /// The floor as an integer (the values it is used on are small).
    fn floor_i32(self) -> i32;

    #[inline(always)]
    fn zero() -> Self {
        Self::from_q16(0)
    }
    #[inline(always)]
    fn one() -> Self {
        Self::from_q16(65536)
    }
    #[inline(always)]
    fn from_i32(i: i32) -> Self {
        Self::from_q16(i << 16)
    }
    #[inline(always)]
    fn abs(self) -> Self {
        if self < Self::zero() { -self } else { self }
    }
    #[inline(always)]
    fn min(self, o: Self) -> Self {
        if o < self { o } else { self }
    }
    #[inline(always)]
    fn max(self, o: Self) -> Self {
        if o > self { o } else { self }
    }
    #[inline(always)]
    fn clamp01(self) -> Self {
        self.max(Self::zero()).min(Self::one())
    }
}

/// Shorthand for a constant of the scalar in use.
#[inline(always)]
pub fn k<R: Real>(x: f32) -> R {
    R::k(x)
}

// ---- Q16.16 ---------------------------------------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Fx(pub i32);

#[inline(always)]
fn sat(v: i64) -> i32 {
    if v > i32::MAX as i64 {
        i32::MAX
    } else if v < -(i32::MAX as i64) {
        -i32::MAX
    } else {
        v as i32
    }
}

impl Add for Fx {
    type Output = Fx;
    #[inline(always)]
    fn add(self, o: Fx) -> Fx {
        Fx(self.0.saturating_add(o.0))
    }
}
impl Sub for Fx {
    type Output = Fx;
    #[inline(always)]
    fn sub(self, o: Fx) -> Fx {
        Fx(self.0.saturating_sub(o.0))
    }
}
impl Mul for Fx {
    type Output = Fx;
    #[inline(always)]
    fn mul(self, o: Fx) -> Fx {
        Fx(sat((self.0 as i64 * o.0 as i64) >> 16))
    }
}
impl Div for Fx {
    type Output = Fx;
    #[inline(always)]
    fn div(self, o: Fx) -> Fx {
        if o.0 == 0 {
            return Fx(if self.0 >= 0 { i32::MAX } else { -i32::MAX });
        }
        Fx(sat(((self.0 as i64) << 16) / o.0 as i64))
    }
}
impl Neg for Fx {
    type Output = Fx;
    #[inline(always)]
    fn neg(self) -> Fx {
        Fx(self.0.saturating_neg())
    }
}

impl Real for Fx {
    const NAME: &'static str = "fx16";
    #[inline(always)]
    fn k(x: f32) -> Fx {
        Fx((x * 65536.0) as i32)
    }
    #[inline(always)]
    fn from_q16(q: i32) -> Fx {
        Fx(q)
    }
    #[inline(always)]
    fn to_q16(self) -> i32 {
        self.0
    }
    #[inline(always)]
    fn sqrt(self) -> Fx {
        if self.0 <= 0 {
            return Fx(0);
        }
        Fx(isqrt_q32((self.0 as u64) << 16) as i32)
    }
    #[inline(always)]
    fn prod_gt(a: Fx, b: Fx, c: Fx, e: Fx) -> bool {
        (a.0 as i64 * b.0 as i64) > (c.0 as i64 * e.0 as i64)
    }
    #[inline(always)]
    fn floor_i32(self) -> i32 {
        self.0 >> 16
    }
}

// ---- f32 ------------------------------------------------------------------------------------------------------------

impl Real for f32 {
    const NAME: &'static str = "f32";
    #[inline(always)]
    fn k(x: f32) -> f32 {
        x
    }
    #[inline(always)]
    fn from_q16(q: i32) -> f32 {
        q as f32 * (1.0 / 65536.0)
    }
    #[inline(always)]
    fn to_q16(self) -> i32 {
        (self * 65536.0) as i32
    }
    #[inline(always)]
    fn sqrt(self) -> f32 {
        if self <= 0.0 {
            return 0.0;
        }
        sqrt_f32(self)
    }
    #[inline(always)]
    fn prod_gt(a: f32, b: f32, c: f32, e: f32) -> bool {
        a * b > c * e
    }
    #[inline(always)]
    fn floor_i32(self) -> i32 {
        let i = self as i32;
        if (i as f32) > self { i - 1 } else { i }
    }
}

#[cfg(all(target_arch = "x86_64", target_feature = "sse"))]
#[inline(always)]
#[allow(unused_unsafe)]
fn sqrt_f32(x: f32) -> f32 {
    use core::arch::x86_64::{_mm_cvtss_f32, _mm_set_ss, _mm_sqrt_ss};
    unsafe { _mm_cvtss_f32(_mm_sqrt_ss(_mm_set_ss(x))) }
}

/// Without an SSE unit: the bit-level estimate and three Newton steps (aarch64's hard-float build lands here too;
/// it is the measurement's reference path only on the host).
#[cfg(not(all(target_arch = "x86_64", target_feature = "sse")))]
#[inline(always)]
fn sqrt_f32(x: f32) -> f32 {
    let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1FC0_0000);
    y = 0.5 * (y + x / y);
    y = 0.5 * (y + x / y);
    0.5 * (y + x / y)
}

/// `floor(sqrt(n))` for the Q16.16 root's `n = q << 16` (q a positive i32, so n < 2^47). With SSE2 (B876): n is
/// exact as an f64 (< 2^53), `sqrtsd` is correctly rounded, so the truncated root is the true floor or one above it
/// (rounding up past an integer); the one downward correction makes it `u64::isqrt` bit for bit, at ~1/5 the cost.
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
#[inline(always)]
#[allow(unused_unsafe)]
fn isqrt_q32(n: u64) -> u64 {
    use core::arch::x86_64::{_mm_cvtsd_f64, _mm_set_sd, _mm_setzero_pd, _mm_sqrt_sd};
    let mut r = unsafe { _mm_cvtsd_f64(_mm_sqrt_sd(_mm_setzero_pd(), _mm_set_sd(n as f64))) } as u64;
    if r * r > n {
        r -= 1;
    }
    r
}

/// Without SSE2 (the soft-float kernel profile, aarch64): core's integer square root, unchanged.
#[cfg(not(all(target_arch = "x86_64", target_feature = "sse2")))]
#[inline(always)]
fn isqrt_q32(n: u64) -> u64 {
    n.isqrt()
}

#[cfg(test)]
mod tests {
    use super::isqrt_q32;

    /// The fast root is `u64::isqrt` bit for bit over every input `Fx::sqrt` can hand it: a stride sweep of all
    /// positive i32 (shifted to Q32), both ends of the range, and the edges 0, 1, 2^47 - 1.
    #[test]
    fn isqrt_q32_is_isqrt() {
        let check = |n: u64| assert_eq!(isqrt_q32(n), n.isqrt(), "isqrt_q32({})", n);
        for n in [0u64, 1, 2, 3, 4, (1u64 << 47) - 1, ((i32::MAX as u64) << 16)] {
            check(n);
        }
        let mut q = 1u64;
        while q <= i32::MAX as u64 {
            check(q << 16);
            q += 997;
        }
        for q in (1u64..=200_000).chain((i32::MAX as u64 - 200_000)..=i32::MAX as u64) {
            check(q << 16);
        }
        // Every n just below and at a perfect square near the top of the range, where rounding up bites.
        for r in ((1u64 << 23) - 4096)..(1u64 << 23) + 64 {
            let sq = r * r;
            if sq < (1u64 << 47) {
                check(sq - 1);
                check(sq);
            }
        }
    }
}
