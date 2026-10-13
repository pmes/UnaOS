// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// TEMPLE (SR147): the scalar the renderer is written over. Two backends, one scene:
//
//   `Fx`  — Q16.16 in an i32, every op integer (i64 products, saturating). The x86 ring-3 profile is
//           `+soft-float,-sse` (unaos/x86_64-unknown-none.json) and the kernel clears CR4.OSFXSR for every UnaOS
//           ring-3 task (crates/kernel/src/arch/x86_64/linuxabi/fpu.rs:20), so an f32 there is a compiler_builtins
//           call per op. Fx is what the vessel runs on x86; it is bit-exact on every machine, so the host's 64x64
//           checksum is the ring-3 program's checksum.
//   `f32` — hardware float where the program owns an FPU (the host; aarch64 ring 3 is hard-float, aarch64-base.json).
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
        Fx(((self.0 as u64) << 16).isqrt() as i32)
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
