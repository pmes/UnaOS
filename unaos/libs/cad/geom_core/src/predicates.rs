//! Exact orientation predicates: `orient2d`, `orient3d`, `incircle` (after J. R. Shewchuk, "Adaptive Precision
//! Floating-Point Arithmetic and Fast Robust Geometric Predicates", 1997).
//!
//! A predicate that lies is the one thing this crate must not do. Each predicate:
//! 1. refuses a NaN or infinite coordinate ([`Unsure::NonFinite`]);
//! 2. rescales every coordinate by ONE exact power of two so the largest magnitude is in [0.5, 1). The signs are
//!    invariant (the determinants are homogeneous of degree 2, 3 and 4 in the coordinates, and the scale is
//!    positive), and the rescale removes the overflow at 1e300 and the underflow at 1e-300 that make an unscaled
//!    evaluation, Shewchuk's included, return a wrong sign;
//! 3. evaluates Shewchuk's stage-A filter: the floating determinant against `errboundA * permanent`, plus an
//!    absolute term for any subnormal rounding, and returns the sign when the filter is decisive;
//! 4. otherwise evaluates the determinant EXACTLY as a nonoverlapping expansion (two_diff of every difference,
//!    products by scale_expansion, sums by grow_expansion, the shape of Shewchuk's `orient2dexact` /
//!    `orient3dexact` / `incircleexact`) and returns the sign of its most significant component.
//!
//! The exact stage is exact when every intermediate value is an integer multiple of 2^-1074: true when every
//! nonzero rescaled coordinate is at least 2^-T, T = floor(1074 / degree) - 54 (orient2d 483, orient3d 304,
//! incircle 214). When the filter is not decisive and the coordinates span more than that range, the answer is
//! [`Unsure::Range`]: a refusal by name, never a guess. Shewchuk's adaptive stages B-D (the same exact sign,
//! faster) are owed (VUG-GEOM-ADAPT).

use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::vec::{Vec2, Vec3};

/// Why a predicate refused to answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsure {
    /// A coordinate is NaN or infinite.
    NonFinite,
    /// The coordinates span a range the exact stage cannot represent and the filter was not decisive.
    Range,
}

/// The sign of a predicate's determinant, or a named refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Predicate {
    Negative,
    Zero,
    Positive,
    Uncertain(Unsure),
}

impl Predicate {
    /// The sign as an ordering against zero; `None` for [`Predicate::Uncertain`].
    pub fn sign(self) -> Option<Ordering> {
        match self {
            Predicate::Negative => Some(Ordering::Less),
            Predicate::Zero => Some(Ordering::Equal),
            Predicate::Positive => Some(Ordering::Greater),
            Predicate::Uncertain(_) => None,
        }
    }
    fn of(x: f64) -> Predicate {
        if x > 0.0 {
            Predicate::Positive
        } else if x < 0.0 {
            Predicate::Negative
        } else {
            Predicate::Zero
        }
    }
}

/// Positive when `a`, `b`, `c` turn counterclockwise, negative when clockwise, zero when collinear.
pub fn orient2d(a: Vec2, b: Vec2, c: Vec2) -> Predicate {
    let mut v = [a.x, a.y, b.x, b.y, c.x, c.y];
    let wide = match prepare(&mut v, 483) {
        Prep::AllZero => return Predicate::Zero,
        Prep::Refuse(u) => return Predicate::Uncertain(u),
        Prep::Ready { wide } => wide,
    };
    let [ax, ay, bx, by, cx, cy] = v;
    let detleft = (ax - cx) * (by - cy);
    let detright = (ay - cy) * (bx - cx);
    let det = detleft - detright;
    let bound = CCW_ERRBOUND_A * (detleft.abs() + detright.abs()) + ABS_UNDERFLOW;
    if det > bound || -det > bound {
        return Predicate::of(det);
    }
    if wide {
        return Predicate::Uncertain(Unsure::Range);
    }
    let acx = diff(ax, cx);
    let bcy = diff(by, cy);
    let acy = diff(ay, cy);
    let bcx = diff(bx, cx);
    let e = sub(&mul(&acx, &bcy), &mul(&acy, &bcx));
    Predicate::of(sign(&e))
}

/// The determinant of `[a-d; b-d; c-d]`: positive when `d` lies below the plane through `a`, `b`, `c` (the side
/// from which `a`, `b`, `c` appear clockwise), negative above, zero when coplanar (Shewchuk's convention).
pub fn orient3d(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> Predicate {
    let mut v = [a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z, d.x, d.y, d.z];
    let wide = match prepare(&mut v, 304) {
        Prep::AllZero => return Predicate::Zero,
        Prep::Refuse(u) => return Predicate::Uncertain(u),
        Prep::Ready { wide } => wide,
    };
    let [ax, ay, az, bx, by, bz, cx, cy, cz, dx, dy, dz] = v;
    let (adx, ady, adz) = (ax - dx, ay - dy, az - dz);
    let (bdx, bdy, bdz) = (bx - dx, by - dy, bz - dz);
    let (cdx, cdy, cdz) = (cx - dx, cy - dy, cz - dz);
    let (bdxcdy, cdxbdy) = (bdx * cdy, cdx * bdy);
    let (cdxady, adxcdy) = (cdx * ady, adx * cdy);
    let (adxbdy, bdxady) = (adx * bdy, bdx * ady);
    let det = adz * (bdxcdy - cdxbdy) + bdz * (cdxady - adxcdy) + cdz * (adxbdy - bdxady);
    let permanent = (bdxcdy.abs() + cdxbdy.abs()) * adz.abs()
        + (cdxady.abs() + adxcdy.abs()) * bdz.abs()
        + (adxbdy.abs() + bdxady.abs()) * cdz.abs();
    let bound = O3D_ERRBOUND_A * permanent + ABS_UNDERFLOW;
    if det > bound || -det > bound {
        return Predicate::of(det);
    }
    if wide {
        return Predicate::Uncertain(Unsure::Range);
    }
    let (adx, ady, adz) = (diff(ax, dx), diff(ay, dy), diff(az, dz));
    let (bdx, bdy, bdz) = (diff(bx, dx), diff(by, dy), diff(bz, dz));
    let (cdx, cdy, cdz) = (diff(cx, dx), diff(cy, dy), diff(cz, dz));
    let m1 = sub(&mul(&bdx, &cdy), &mul(&cdx, &bdy));
    let m2 = sub(&mul(&cdx, &ady), &mul(&adx, &cdy));
    let m3 = sub(&mul(&adx, &bdy), &mul(&bdx, &ady));
    let e = add(&add(&mul(&adz, &m1), &mul(&bdz, &m2)), &mul(&cdz, &m3));
    Predicate::of(sign(&e))
}

/// Positive when `d` lies inside the circle through `a`, `b`, `c` (given counterclockwise), negative outside, zero
/// when the four are cocircular. For clockwise `a`, `b`, `c` the sign is reversed (Shewchuk's convention).
pub fn incircle(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Predicate {
    let mut v = [a.x, a.y, b.x, b.y, c.x, c.y, d.x, d.y];
    let wide = match prepare(&mut v, 214) {
        Prep::AllZero => return Predicate::Zero,
        Prep::Refuse(u) => return Predicate::Uncertain(u),
        Prep::Ready { wide } => wide,
    };
    let [ax, ay, bx, by, cx, cy, dx, dy] = v;
    let (adx, ady) = (ax - dx, ay - dy);
    let (bdx, bdy) = (bx - dx, by - dy);
    let (cdx, cdy) = (cx - dx, cy - dy);
    let (bdxcdy, cdxbdy) = (bdx * cdy, cdx * bdy);
    let (cdxady, adxcdy) = (cdx * ady, adx * cdy);
    let (adxbdy, bdxady) = (adx * bdy, bdx * ady);
    let alift = adx * adx + ady * ady;
    let blift = bdx * bdx + bdy * bdy;
    let clift = cdx * cdx + cdy * cdy;
    let det = alift * (bdxcdy - cdxbdy) + blift * (cdxady - adxcdy) + clift * (adxbdy - bdxady);
    let permanent = (bdxcdy.abs() + cdxbdy.abs()) * alift
        + (cdxady.abs() + adxcdy.abs()) * blift
        + (adxbdy.abs() + bdxady.abs()) * clift;
    let bound = ICC_ERRBOUND_A * permanent + ABS_UNDERFLOW;
    if det > bound || -det > bound {
        return Predicate::of(det);
    }
    if wide {
        return Predicate::Uncertain(Unsure::Range);
    }
    let (adx, ady) = (diff(ax, dx), diff(ay, dy));
    let (bdx, bdy) = (diff(bx, dx), diff(by, dy));
    let (cdx, cdy) = (diff(cx, dx), diff(cy, dy));
    let alift = add(&mul(&adx, &adx), &mul(&ady, &ady));
    let blift = add(&mul(&bdx, &bdx), &mul(&bdy, &bdy));
    let clift = add(&mul(&cdx, &cdx), &mul(&cdy, &cdy));
    let m1 = sub(&mul(&bdx, &cdy), &mul(&cdx, &bdy));
    let m2 = sub(&mul(&cdx, &ady), &mul(&adx, &cdy));
    let m3 = sub(&mul(&adx, &bdy), &mul(&bdx, &ady));
    let e = add(&add(&mul(&alift, &m1), &mul(&blift, &m2)), &mul(&clift, &m3));
    Predicate::of(sign(&e))
}

// ---- the stage-A filter constants (Shewchuk's exactinit with epsilon = 2^-53) ----

const EPSILON: f64 = 1.1102230246251565e-16; // 2^-53
const CCW_ERRBOUND_A: f64 = (3.0 + 16.0 * EPSILON) * EPSILON;
const O3D_ERRBOUND_A: f64 = (7.0 + 56.0 * EPSILON) * EPSILON;
const ICC_ERRBOUND_A: f64 = (10.0 + 96.0 * EPSILON) * EPSILON;
/// An absolute allowance for subnormal rounding in the filter: each operation can lose at most half of 2^-1074,
/// amplified by factors of at most 8 after the rescale; 1024 * 2^-1074 covers every term of the three filters.
const ABS_UNDERFLOW: f64 = 1024.0 * f64::from_bits(1);
const SPLITTER: f64 = 134217729.0; // 2^27 + 1

// ---- the rescale ----

enum Prep {
    AllZero,
    Refuse(Unsure),
    Ready { wide: bool },
}

/// floor(log2 |x|) for a finite nonzero `x`, subnormals included.
fn exponent(x: f64) -> i32 {
    let bits = x.to_bits() & 0x7fff_ffff_ffff_ffff;
    let biased = (bits >> 52) as i32;
    if biased == 0 { -1074 + (63 - bits.leading_zeros() as i32) } else { biased - 1023 }
}

/// 2^k for k in [-1022, 1023].
fn pow2(k: i32) -> f64 {
    f64::from_bits(((k + 1023) as u64) << 52)
}

/// `x * 2^k`, exact while the result stays a normal number (scaling up never loses a bit).
fn scale2(mut x: f64, mut k: i32) -> f64 {
    while k > 1000 {
        x *= pow2(1000);
        k -= 1000;
    }
    while k < -1000 {
        x *= pow2(-1000);
        k += 1000;
    }
    x * pow2(k)
}

/// Rescales `v` in place so its largest magnitude is in [0.5, 1); `wide` when a nonzero coordinate falls under
/// 2^-t after the rescale (the exact stage would not be exact).
fn prepare(v: &mut [f64], t: i32) -> Prep {
    let mut max = 0.0f64;
    for x in v.iter() {
        if !x.is_finite() {
            return Prep::Refuse(Unsure::NonFinite);
        }
        max = max.max(x.abs());
    }
    if max == 0.0 {
        return Prep::AllZero;
    }
    let k = -(exponent(max) + 1);
    let mut wide = false;
    for x in v.iter_mut() {
        if *x == 0.0 {
            continue;
        }
        let e = exponent(*x) + k;
        if e < -1022 {
            // The rescaled value would be subnormal and lose bits: the inputs themselves would change.
            return Prep::Refuse(Unsure::Range);
        }
        if e < -t {
            wide = true;
        }
        *x = scale2(*x, k);
    }
    Prep::Ready { wide }
}

// ---- expansion arithmetic (Shewchuk §2): nonoverlapping, increasing magnitude, zeros eliminated ----

fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let bv = x - a;
    let av = x - bv;
    let br = b - bv;
    let ar = a - av;
    (x, ar + br)
}

/// Requires |a| >= |b| (or a == 0).
fn fast_two_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let bv = x - a;
    (x, b - bv)
}

fn two_diff(a: f64, b: f64) -> (f64, f64) {
    let x = a - b;
    let bv = a - x;
    let av = x + bv;
    let br = bv - b;
    let ar = a - av;
    (x, ar + br)
}

fn split(a: f64) -> (f64, f64) {
    let c = SPLITTER * a;
    let abig = c - a;
    let hi = c - abig;
    (hi, a - hi)
}

/// `a * b` exactly as `x + y` (Dekker; no fused multiply-add needed).
fn two_product(a: f64, b: f64) -> (f64, f64) {
    let x = a * b;
    let (ahi, alo) = split(a);
    let (bhi, blo) = split(b);
    let err1 = x - ahi * bhi;
    let err2 = err1 - alo * bhi;
    let err3 = err2 - ahi * blo;
    (x, alo * blo - err3)
}

/// `a - b` as an expansion.
fn diff(a: f64, b: f64) -> Vec<f64> {
    let (x, y) = two_diff(a, b);
    let mut e = Vec::with_capacity(2);
    if y != 0.0 {
        e.push(y);
    }
    if x != 0.0 {
        e.push(x);
    }
    e
}

/// GROW-EXPANSION with zero elimination: `e + b`.
fn grow(e: &[f64], b: f64) -> Vec<f64> {
    let mut h = Vec::with_capacity(e.len() + 1);
    let mut q = b;
    for &enow in e {
        let (qn, hh) = two_sum(q, enow);
        if hh != 0.0 {
            h.push(hh);
        }
        q = qn;
    }
    if q != 0.0 {
        h.push(q);
    }
    h
}

/// SCALE-EXPANSION with zero elimination: `e * b`.
fn scale(e: &[f64], b: f64) -> Vec<f64> {
    let mut h = Vec::with_capacity(2 * e.len());
    let Some((&first, rest)) = e.split_first() else { return h };
    let (mut q, hh) = two_product(first, b);
    if hh != 0.0 {
        h.push(hh);
    }
    for &enow in rest {
        let (p1, p0) = two_product(enow, b);
        let (sum, hh) = two_sum(q, p0);
        if hh != 0.0 {
            h.push(hh);
        }
        let (qn, hh) = fast_two_sum(p1, sum);
        if hh != 0.0 {
            h.push(hh);
        }
        q = qn;
    }
    if q != 0.0 {
        h.push(q);
    }
    h
}

fn add(e: &[f64], f: &[f64]) -> Vec<f64> {
    let mut h: Vec<f64> = e.to_vec();
    for &x in f {
        h = grow(&h, x);
    }
    h
}

fn sub(e: &[f64], f: &[f64]) -> Vec<f64> {
    let neg: Vec<f64> = f.iter().map(|x| -x).collect();
    add(e, &neg)
}

fn mul(e: &[f64], f: &[f64]) -> Vec<f64> {
    let mut h = Vec::new();
    for &x in f {
        h = add(&h, &scale(e, x));
    }
    h
}

/// The sign of a zero-eliminated expansion is its most significant (last) component's.
fn sign(e: &[f64]) -> f64 {
    e.last().copied().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(e: &[f64]) -> f64 {
        e.iter().sum()
    }

    #[test]
    fn two_product_is_exact() {
        let a = 1.0 + f64::EPSILON;
        let (x, y) = two_product(a, a);
        // (1 + u)^2 = 1 + 2u + u^2: the u^2 term is the error.
        assert_eq!(x, 1.0 + 2.0 * f64::EPSILON);
        assert_eq!(y, f64::EPSILON * f64::EPSILON);
    }

    #[test]
    fn expansions_carry_what_doubles_drop() {
        let e = add(&diff(1.0, 0.0), &[1e-30]);
        assert_eq!(e, [1e-30, 1.0]);
        let z = sub(&e, &e);
        assert!(z.is_empty());
        // 3 * (1e20 + 1) held exactly: minus 3 * 1e20 leaves exactly 3.
        let m = mul(&diff(3.0, 0.0), &diff(1e20, -1.0));
        assert_eq!(value(&sub(&m, &scale(&[1e20], 3.0))), 3.0);
        assert_eq!(sign(&sub(&[1e-300], &[1.0])), -1.0);
    }

    #[test]
    fn exponent_and_scale() {
        assert_eq!(exponent(1.0), 0);
        assert_eq!(exponent(0.75), -1);
        assert_eq!(exponent(f64::from_bits(1)), -1074);
        assert_eq!(exponent(f64::MAX), 1023);
        assert_eq!(scale2(f64::from_bits(1), 1074), 1.0);
        assert_eq!(scale2(1.0, -1022), f64::MIN_POSITIVE);
    }
}
