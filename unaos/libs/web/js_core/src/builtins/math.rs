//! Math (§21.3). `no_std` has no libm, so the elementary functions are computed here: argument reduction and
//! Taylor / Newton iterations in double-double arithmetic (~106-bit significands), then rounded once. This
//! gives results that are correctly rounded in all but astronomically rare cases.

use super::*;
use crate::numconv::libm_floor;

// ------------------------------------------------------------------------------------------------ double-double

#[derive(Clone, Copy, Debug)]
pub struct DD(pub f64, pub f64);

#[inline]
fn two_sum(a: f64, b: f64) -> DD {
    let s = a + b;
    let bb = s - a;
    let e = (a - (s - bb)) + (b - bb);
    DD(s, e)
}
#[inline]
fn quick_two_sum(a: f64, b: f64) -> DD {
    let s = a + b;
    DD(s, b - (s - a))
}
#[inline]
fn split(a: f64) -> (f64, f64) {
    if a.abs() > 6.69692879491417e+299 {
        let a2 = a * 3.7252902984619140625e-09; // 2^-28
        let t = 134217729.0 * a2;
        let hi = t - (t - a2);
        let lo = a2 - hi;
        return (hi * 268435456.0, lo * 268435456.0);
    }
    let t = 134217729.0 * a;
    let hi = t - (t - a);
    (hi, a - hi)
}
#[inline]
fn two_prod(a: f64, b: f64) -> DD {
    let p = a * b;
    let (ah, al) = split(a);
    let (bh, bl) = split(b);
    let e = ((ah * bh - p) + ah * bl + al * bh) + al * bl;
    DD(p, e)
}

impl DD {
    pub fn from(a: f64) -> DD {
        DD(a, 0.0)
    }
    pub fn add(self, o: DD) -> DD {
        let s = two_sum(self.0, o.0);
        let t = two_sum(self.1, o.1);
        let s = quick_two_sum(s.0, s.1 + t.0);
        quick_two_sum(s.0, s.1 + t.1)
    }
    pub fn neg(self) -> DD {
        DD(-self.0, -self.1)
    }
    pub fn sub(self, o: DD) -> DD {
        self.add(o.neg())
    }
    pub fn mul(self, o: DD) -> DD {
        let p = two_prod(self.0, o.0);
        quick_two_sum(p.0, p.1 + (self.0 * o.1 + self.1 * o.0))
    }
    pub fn mulf(self, b: f64) -> DD {
        let p = two_prod(self.0, b);
        quick_two_sum(p.0, p.1 + self.1 * b)
    }
    pub fn div(self, o: DD) -> DD {
        let q1 = self.0 / o.0;
        let r = self.sub(o.mulf(q1));
        let q2 = r.0 / o.0;
        let r = r.sub(o.mulf(q2));
        let q3 = r.0 / o.0;
        quick_two_sum(q1, q2).add(DD(q3, 0.0))
    }
    pub fn sqrt(self) -> DD {
        if self.0 <= 0.0 {
            return DD(sqrt(self.0), 0.0);
        }
        let x = 1.0 / sqrt(self.0);
        let ax = self.0 * x;
        let diff = self.sub(two_prod(ax, ax));
        quick_two_sum(ax, diff.0 * (x * 0.5))
    }
    pub fn scale(self, k: i32) -> DD {
        DD(ldexp(self.0, k), ldexp(self.1, k))
    }
    pub fn val(self) -> f64 {
        self.0 + self.1
    }
}

const LN2: DD = DD(6.931471805599452862e-01, 2.319046813846299558e-17);
const PI: DD = DD(3.141592653589793116e+00, 1.224646799147353207e-16);
const PI_2: DD = DD(1.570796326794896558e+00, 6.123233995736766036e-17);
const LN10: DD = DD(2.302585092994045901e+00, -2.170756223382249351e-16);
// pi/2 as three doubles for argument reduction.
const PIO2_1: f64 = 1.570796326794896558e+00;
const PIO2_2: f64 = 6.123233995736766036e-17;
const PIO2_3: f64 = -1.4973849048591698e-33;

pub fn ldexp(x: f64, k: i32) -> f64 {
    crate::numconv_ldexp(x, k)
}

/// Correctly rounded square root of a non-negative double (exact integer square root of the significand).
pub fn sqrt(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 || x.is_infinite() {
        return x;
    }
    let bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7FF) as i64;
    let mut m = bits & ((1u64 << 52) - 1);
    if e == 0 {
        // subnormal: normalise
        let lz = m.leading_zeros() as i64 - 11;
        m <<= lz;
        e = 1 - lz;
        m &= (1u64 << 52) - 1;
    }
    m |= 1u64 << 52;
    let mut exp = e - 1023;
    // make exponent even
    let mut mm = m as u128;
    if exp & 1 != 0 {
        mm <<= 1;
        exp -= 1;
    }
    // sqrt(mm * 2^exp) = sqrt(mm << 64) * 2^(exp/2 - 32)
    let n = mm << 64;
    let mut r = isqrt_u128(n);
    // r has ~ 59 bits; we want 53 bits + round. Compute with remainder for exact rounding.
    let rem_nonzero = r * r != n;
    // Normalise r to 54 bits (53 + guard) then round half even using the sticky bit.
    let rb = 128 - r.leading_zeros() as i64;
    let shift = rb - 54;
    // x = mm * 2^(exp - 52), so sqrt(x) = r * 2^(-32) * 2^((exp - 52) / 2).
    let mut res_exp = exp / 2 - 26 - 32 + shift;
    let sticky = rem_nonzero || (shift > 0 && (r & ((1u128 << shift) - 1)) != 0);
    if shift > 0 {
        r >>= shift;
    }
    let guard = r & 1;
    let mut q = (r >> 1) as u64;
    res_exp += 1;
    if guard == 1 && (sticky || q & 1 == 1) {
        q += 1;
        if q == 1u64 << 53 {
            q >>= 1;
            res_exp += 1;
        }
    }
    ldexp(q as f64, res_exp as i32)
}

fn isqrt_u128(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = 1u128 << ((128 - n.leading_zeros()).div_ceil(2));
    loop {
        let y = (x + n / x) >> 1;
        if y >= x {
            return x;
        }
        x = y;
    }
}

fn round_half_even_int(x: f64) -> f64 {
    let f = libm_floor(x);
    let d = x - f;
    if d > 0.5 || (d == 0.5 && (f / 2.0) != libm_floor(f / 2.0)) {
        f + 1.0
    } else {
        f
    }
}

/// exp in double-double for |x.0| < ~710.
fn exp_dd(x: DD) -> DD {
    let k = round_half_even_int(x.0 / LN2.0);
    let r = x.sub(LN2.mulf(k));
    // r in [-ln2/2, ln2/2]; scale down by 2^10 and use Taylor, then square.
    let r = r.scale(-10);
    let mut sum = DD::from(1.0);
    let mut term = DD::from(1.0);
    for i in 1..=14 {
        term = term.mul(r).div(DD::from(i as f64));
        sum = sum.add(term);
        if term.0.abs() < 1e-40 {
            break;
        }
    }
    for _ in 0..10 {
        sum = sum.mul(sum);
    }
    sum.scale(k as i32)
}

/// Natural log in double-double for positive finite x (as DD).
fn log_dd(x: DD) -> DD {
    // Initial approximation from the exponent and a short atanh series.
    let (m, e) = frexp(x.0);
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let mut t = s;
    let mut acc = 0.0;
    let mut k = 1.0;
    for _ in 0..30 {
        acc += t / k;
        t *= s2;
        k += 2.0;
    }
    let mut y = DD::from(e as f64 * LN2.0 + 2.0 * acc);
    // Two Newton steps: y += x*exp(-y) - 1
    for _ in 0..2 {
        let ey = exp_dd(y.neg());
        y = y.add(x.mul(ey).sub(DD::from(1.0)));
    }
    y
}

pub fn frexp(x: f64) -> (f64, i32) {
    if x == 0.0 || !x.is_finite() {
        return (x, 0);
    }
    let bits = x.to_bits();
    let e = ((bits >> 52) & 0x7FF) as i32;
    if e == 0 {
        let (m, e2) = frexp(x * 18014398509481984.0); // 2^54
        return (m, e2 - 54);
    }
    let m = f64::from_bits((bits & !(0x7FFu64 << 52)) | (1022u64 << 52));
    (m, e - 1022)
}

pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x > 709.782712893384 {
        return f64::INFINITY;
    }
    if x < -745.1332191019412 {
        return 0.0;
    }
    if x == 0.0 {
        return 1.0;
    }
    exp_dd(DD::from(x)).val()
}

pub fn expm1(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        return x;
    }
    if x > 709.782712893384 {
        return f64::INFINITY;
    }
    if x < -40.0 {
        return -1.0;
    }
    if x.abs() < 1e-5 {
        // Taylor in double-double.
        let xd = DD::from(x);
        let mut sum = xd;
        let mut term = xd;
        for i in 2..=8 {
            term = term.mul(xd).div(DD::from(i as f64));
            sum = sum.add(term);
        }
        return sum.val();
    }
    exp_dd(DD::from(x)).sub(DD::from(1.0)).val()
}

pub fn log(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return x;
    }
    if x == 1.0 {
        return 0.0;
    }
    log_dd(DD::from(x)).val()
}

pub fn log1p(x: f64) -> f64 {
    if x.is_nan() || x < -1.0 {
        return f64::NAN;
    }
    if x == -1.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() || x == 0.0 {
        return x;
    }
    let one_plus = two_sum(1.0, x);
    log_dd(one_plus).val()
}

pub fn log10(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return x;
    }
    if x == 1.0 {
        return 0.0;
    }
    log_dd(DD::from(x)).div(LN10).val()
}

pub fn log2(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return x;
    }
    let (m, e) = frexp(x);
    if m == 0.5 {
        return (e - 1) as f64;
    }
    log_dd(DD::from(x)).div(LN2).val()
}

/// Reduce x by multiples of pi/2: returns (r as DD, quadrant).
fn rem_pio2(x: f64) -> (DD, i64) {
    if x.abs() < 0.7853981633974483 {
        return (DD::from(x), 0);
    }
    if x.abs() < 1e15 {
        let k = round_half_even_int(x / PIO2_1);
        // r = x - k*(p1 + p2 + p3), exactly in double-double steps
        let a = DD::from(x).sub(two_prod(k, PIO2_1));
        let b = a.sub(two_prod(k, PIO2_2));
        let c = b.sub(two_prod(k, PIO2_3));
        return (c, k as i64);
    }
    // Huge arguments: exact reduction with big integers.
    big_rem_pio2(x)
}

fn big_rem_pio2(x: f64) -> (DD, i64) {
    use crate::bignum::BigUint;
    // x = m * 2^e exactly. Compute frac(x * 2/pi) using 2/pi to ~1200 bits: 2/pi = 2 / pi, pi via Machin.
    let bits = 1300u64;
    let pi = machin_pi(bits);
    // two_over_pi_scaled = floor(2^(2*bits) / pi_scaled) where pi_scaled = pi * 2^bits
    let num = BigUint::from_u64(2).shl(2 * bits);
    let (tpo, _) = num.divrem(&pi);
    let b = x.abs().to_bits();
    let e = ((b >> 52) & 0x7FF) as i64 - 1075;
    let m = (b & ((1u64 << 52) - 1)) | (1u64 << 52);
    // y = m * 2^e * tpo / 2^bits; we need y mod 4 and its fraction.
    let prod = BigUint::from_u64(m).mul(&tpo);
    let shift = bits as i64 - e; // y = prod / 2^shift
    let int_part = prod.shr(shift as u64);
    let q = (int_part.low_u64() & 3) as i64;
    // fraction f = (prod mod 2^shift) / 2^shift, take top 120 bits
    let frac = prod.sub(&int_part.shl(shift as u64));
    let top = frac.shr((shift - 120).max(0) as u64);
    let hi = (top.shr(60).low_u64()) as f64 * ldexp(1.0, -60);
    let lo = (top.low_u64() & ((1u64 << 60) - 1)) as f64 * ldexp(1.0, -120);
    let mut f = two_sum(hi, lo);
    let mut q = q;
    if f.0 > 0.5 {
        f = f.sub(DD::from(1.0));
        q += 1;
    }
    // r = f * pi/2
    let r = f.mul(PI_2);
    let r = if x < 0.0 { r.neg() } else { r };
    let q = if x < 0.0 { -q } else { q };
    (r, q)
}

fn machin_pi(bits: u64) -> crate::bignum::BigUint {
    use crate::bignum::BigUint;
    // pi = 16 atan(1/5) - 4 atan(1/239), fixed point with `bits + 32` fraction bits.
    let fb = bits + 32;
    let one = BigUint::from_u64(1).shl(fb);
    let atan_inv = |n: u64| -> BigUint {
        let n2 = BigUint::from_u64(n * n);
        let mut term = one.divrem(&BigUint::from_u64(n)).0;
        let mut sum = term.clone();
        let mut k = 1u64;
        let mut neg = true;
        loop {
            term = term.divrem(&n2).0;
            if term.is_zero() {
                break;
            }
            let t = term.divrem(&BigUint::from_u64(2 * k + 1)).0;
            if neg {
                sum = sum.sub(&t);
            } else {
                sum = sum.add(&t);
            }
            neg = !neg;
            k += 1;
        }
        sum
    };
    let a = atan_inv(5).mul_small(16);
    let b = atan_inv(239).mul_small(4);
    a.sub(&b).shr(32)
}

fn sin_kernel(r: DD) -> DD {
    let r2 = r.mul(r);
    let mut term = r;
    let mut sum = r;
    let mut i = 1.0;
    for _ in 0..14 {
        term = term.mul(r2).div(DD::from((i + 1.0) * (i + 2.0))).neg();
        sum = sum.add(term);
        i += 2.0;
        if term.0.abs() < 1e-40 {
            break;
        }
    }
    sum
}

fn cos_kernel(r: DD) -> DD {
    let r2 = r.mul(r);
    let mut term = DD::from(1.0);
    let mut sum = term;
    let mut i = 0.0;
    for _ in 0..14 {
        term = term.mul(r2).div(DD::from((i + 1.0) * (i + 2.0))).neg();
        sum = sum.add(term);
        i += 2.0;
        if term.0.abs() < 1e-40 {
            break;
        }
    }
    sum
}

pub fn sin(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() {
        return f64::NAN;
    }
    if x == 0.0 {
        return x;
    }
    let (r, q) = rem_pio2(x);
    let v = match q.rem_euclid(4) {
        0 => sin_kernel(r),
        1 => cos_kernel(r),
        2 => sin_kernel(r).neg(),
        _ => cos_kernel(r).neg(),
    };
    v.val()
}

pub fn cos(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() {
        return f64::NAN;
    }
    let (r, q) = rem_pio2(x);
    let v = match q.rem_euclid(4) {
        0 => cos_kernel(r),
        1 => sin_kernel(r).neg(),
        2 => cos_kernel(r).neg(),
        _ => sin_kernel(r),
    };
    v.val()
}

pub fn tan(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() {
        return f64::NAN;
    }
    if x == 0.0 {
        return x;
    }
    let (r, q) = rem_pio2(x);
    let s = sin_kernel(r);
    let c = cos_kernel(r);
    let v = if q.rem_euclid(2) == 0 { s.div(c) } else { c.div(s).neg() };
    v.val()
}

fn atan_dd(x: DD) -> DD {
    // atan for |x| <= 1 via argument halving and Taylor.
    let mut v = x;
    let mut halvings = 0;
    while v.0.abs() > 0.1 {
        // atan(v) = 2 atan(v / (1 + sqrt(1 + v^2)))
        let d = DD::from(1.0).add(DD::from(1.0).add(v.mul(v)).sqrt());
        v = v.div(d);
        halvings += 1;
    }
    let v2 = v.mul(v);
    let mut term = v;
    let mut sum = v;
    let mut k = 1.0;
    for _ in 0..40 {
        term = term.mul(v2).neg();
        k += 2.0;
        let t = term.div(DD::from(k));
        sum = sum.add(t);
        if t.0.abs() < 1e-40 {
            break;
        }
    }
    sum.scale(halvings)
}

fn atan_full(x: DD) -> DD {
    if x.0.abs() <= 1.0 {
        atan_dd(x)
    } else {
        let a = atan_dd(DD::from(1.0).div(x));
        if x.0 > 0.0 {
            PI_2.sub(a)
        } else {
            PI_2.neg().sub(a)
        }
    }
}

pub fn atan(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        return x;
    }
    if x.is_infinite() {
        return if x > 0.0 { PI_2.0 } else { -PI_2.0 };
    }
    atan_full(DD::from(x)).val()
}

pub fn atan2(y: f64, x: f64) -> f64 {
    if y.is_nan() || x.is_nan() {
        return f64::NAN;
    }
    let neg_y = y.is_sign_negative();
    if y == 0.0 {
        if x > 0.0 || (x == 0.0 && !x.is_sign_negative()) {
            return y;
        }
        return if neg_y { -PI.0 } else { PI.0 };
    }
    if x == 0.0 {
        return if y > 0.0 { PI_2.0 } else { -PI_2.0 };
    }
    if x.is_infinite() {
        if y.is_infinite() {
            let v = if x > 0.0 { PI.0 / 4.0 } else { 3.0 * PI.0 / 4.0 };
            return if y > 0.0 { v } else { -v };
        }
        return if x > 0.0 {
            if neg_y {
                -0.0
            } else {
                0.0
            }
        } else if neg_y {
            -PI.0
        } else {
            PI.0
        };
    }
    if y.is_infinite() {
        return if y > 0.0 { PI_2.0 } else { -PI_2.0 };
    }
    let q = DD::from(y).div(DD::from(x));
    let a = atan_full(q);
    let r = if x > 0.0 {
        a
    } else if y >= 0.0 {
        a.add(PI)
    } else {
        a.sub(PI)
    };
    r.val()
}

pub fn asin(x: f64) -> f64 {
    if x.is_nan() || x.abs() > 1.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return x;
    }
    if x.abs() == 1.0 {
        return if x > 0.0 { PI_2.0 } else { -PI_2.0 };
    }
    let xd = DD::from(x);
    let c = DD::from(1.0).sub(xd).mul(DD::from(1.0).add(xd)).sqrt();
    atan2_dd(xd, c).val()
}

pub fn acos(x: f64) -> f64 {
    if x.is_nan() || x.abs() > 1.0 {
        return f64::NAN;
    }
    if x == 1.0 {
        return 0.0;
    }
    if x == -1.0 {
        return PI.0;
    }
    let xd = DD::from(x);
    let s = DD::from(1.0).sub(xd).mul(DD::from(1.0).add(xd)).sqrt();
    atan2_dd(s, xd).val()
}

fn atan2_dd(y: DD, x: DD) -> DD {
    if x.0 == 0.0 {
        return if y.0 > 0.0 { PI_2 } else { PI_2.neg() };
    }
    let a = atan_full(y.div(x));
    if x.0 > 0.0 {
        a
    } else if y.0 >= 0.0 {
        a.add(PI)
    } else {
        a.sub(PI)
    }
}

pub fn sinh(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 || x.is_infinite() {
        return x;
    }
    if x.abs() > 710.5 {
        return if x > 0.0 { f64::INFINITY } else { f64::NEG_INFINITY };
    }
    if x.abs() < 1e-5 {
        let xd = DD::from(x);
        let x2 = xd.mul(xd);
        return xd.add(x2.mul(xd).div(DD::from(6.0))).add(x2.mul(x2).mul(xd).div(DD::from(120.0))).val();
    }
    let a = x.abs();
    let e = exp_dd(DD::from(a - LN2.0)).sub(exp_dd(DD::from(-a - LN2.0)));
    let v = e.val();
    if x < 0.0 {
        -v
    } else {
        v
    }
}

pub fn cosh(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
    if x == 0.0 {
        return 1.0;
    }
    let a = x.abs();
    if a > 710.5 {
        return f64::INFINITY;
    }
    exp_dd(DD::from(a - LN2.0)).add(exp_dd(DD::from(-a - LN2.0))).val()
}

pub fn tanh(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        return x;
    }
    if x > 20.0 {
        return 1.0;
    }
    if x < -20.0 {
        return -1.0;
    }
    if x.abs() < 1e-5 {
        let xd = DD::from(x);
        return xd.sub(xd.mul(xd).mul(xd).div(DD::from(3.0))).val();
    }
    let e = exp_dd(DD::from(2.0 * x));
    e.sub(DD::from(1.0)).div(e.add(DD::from(1.0))).val()
}

pub fn asinh(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 || x.is_infinite() {
        return x;
    }
    let a = x.abs();
    let r = if a > 1e150 {
        log_dd(DD::from(a)).add(LN2)
    } else if a < 1e-5 {
        let ad = DD::from(a);
        ad.sub(ad.mul(ad).mul(ad).div(DD::from(6.0)))
    } else {
        let ad = DD::from(a);
        log_dd(ad.add(ad.mul(ad).add(DD::from(1.0)).sqrt()))
    };
    let v = r.val();
    if x < 0.0 {
        -v
    } else {
        v
    }
}

pub fn acosh(x: f64) -> f64 {
    if x.is_nan() || x < 1.0 {
        return f64::NAN;
    }
    if x == 1.0 {
        return 0.0;
    }
    if x.is_infinite() {
        return x;
    }
    if x > 1e150 {
        return log_dd(DD::from(x)).add(LN2).val();
    }
    let xd = DD::from(x);
    let t = xd.sub(DD::from(1.0)).mul(xd.add(DD::from(1.0))).sqrt();
    log_dd(xd.add(t)).val()
}

pub fn atanh(x: f64) -> f64 {
    if x.is_nan() || x.abs() > 1.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return x;
    }
    if x == 1.0 {
        return f64::INFINITY;
    }
    if x == -1.0 {
        return f64::NEG_INFINITY;
    }
    if x.abs() < 1e-5 {
        let xd = DD::from(x);
        return xd.add(xd.mul(xd).mul(xd).div(DD::from(3.0))).val();
    }
    let xd = DD::from(x);
    let q = DD::from(1.0).add(xd).div(DD::from(1.0).sub(xd));
    log_dd(q).mulf(0.5).val()
}

pub fn cbrt(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 || x.is_infinite() {
        return x;
    }
    let a = x.abs();
    let (m, e) = frexp(a);
    // Initial guess, then Newton in double-double.
    let mut y = DD::from(ldexp(exp(log(m) / 3.0), 0) * ldexp(1.0, e.div_euclid(3)) * match e.rem_euclid(3) {
        0 => 1.0,
        1 => 1.2599210498948732,
        _ => 1.5874010519681994,
    });
    let ad = DD::from(a);
    for _ in 0..3 {
        // y = y - (y^3 - a) / (3 y^2)
        let y2 = y.mul(y);
        y = y.sub(y2.mul(y).sub(ad).div(y2.mulf(3.0)));
    }
    let v = y.val();
    if x < 0.0 {
        -v
    } else {
        v
    }
}

/// Number::exponentiate (§6.1.6.1.3).
pub fn pow(x: f64, y: f64) -> f64 {
    if y.is_nan() {
        return f64::NAN;
    }
    if y == 0.0 {
        return 1.0;
    }
    if x.is_nan() {
        return f64::NAN;
    }
    let ax = x.abs();
    if y.is_infinite() {
        if ax == 1.0 {
            return f64::NAN;
        }
        return if (ax > 1.0) == (y > 0.0) { f64::INFINITY } else { 0.0 };
    }
    let y_int = libm_floor(y) == y;
    let y_odd = y_int && (y.abs() < 9007199254740992.0) && (libm_floor(y / 2.0) * 2.0 != y);
    if x.is_infinite() {
        if x > 0.0 {
            return if y > 0.0 { f64::INFINITY } else { 0.0 };
        }
        return if y > 0.0 {
            if y_odd {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }
        } else if y_odd {
            -0.0
        } else {
            0.0
        };
    }
    if x == 0.0 {
        let neg = x.is_sign_negative();
        return if y > 0.0 {
            if neg && y_odd {
                -0.0
            } else {
                0.0
            }
        } else if neg && y_odd {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    if x < 0.0 && !y_int {
        return f64::NAN;
    }
    if ax == 1.0 {
        return if x < 0.0 && y_odd { -1.0 } else { 1.0 };
    }
    // Exact small integer powers by repeated squaring in double-double.
    let r = if y_int && y.abs() <= 1024.0 {
        let mut n = y.abs() as u64;
        let mut base = DD::from(ax);
        let mut acc = DD::from(1.0);
        while n > 0 {
            if n & 1 == 1 {
                acc = acc.mul(base);
            }
            n >>= 1;
            if n > 0 {
                base = base.mul(base);
            }
            if !acc.0.is_finite() || !base.0.is_finite() {
                break;
            }
        }
        if acc.0.is_finite() && acc.0 != 0.0 && base.0.is_finite() {
            if y < 0.0 {
                DD::from(1.0).div(acc).val()
            } else {
                acc.val()
            }
        } else {
            pow_general(ax, y)
        }
    } else {
        pow_general(ax, y)
    };
    if x < 0.0 && y_odd {
        -r
    } else {
        r
    }
}

fn pow_general(ax: f64, y: f64) -> f64 {
    let l = log_dd(DD::from(ax));
    let t = l.mul(DD::from(y));
    if t.0 > 709.79 {
        return f64::INFINITY;
    }
    if t.0 < -745.2 {
        return 0.0;
    }
    exp_dd(t).val()
}

pub fn hypot2(a: f64, b: f64) -> f64 {
    let a = DD::from(a.abs());
    let b = DD::from(b.abs());
    a.mul(a).add(b.mul(b)).sqrt().val()
}

pub fn round(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    if x.abs() >= 4503599627370496.0 {
        return x;
    }
    if x > 0.0 && x < 0.5 {
        return 0.0;
    }
    if x < 0.0 && x >= -0.5 {
        return -0.0;
    }
    let f = libm_floor(x);
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

pub fn ceil(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let f = -libm_floor(-x);
    if f == 0.0 && x < 0.0 {
        -0.0
    } else {
        f
    }
}

pub fn trunc(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    if x > 0.0 {
        libm_floor(x)
    } else {
        ceil(x)
    }
}

/// Round to IEEE binary16 (ties to even) and back.
pub fn f16round(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() || x == 0.0 {
        return x;
    }
    let a = x.abs();
    let neg = x < 0.0;
    // binary16: 11-bit significand, exponent min -14 (subnormals down to 2^-24), max 65504.
    let r = if a >= 65520.0 {
        f64::INFINITY
    } else {
        let (_, e) = frexp(a); // a = m * 2^e, m in [0.5,1)
        let exp = (e - 1).max(-14); // unbiased exponent of the leading bit, clamped for subnormals
        let ulp = ldexp(1.0, exp - 10);
        let q = a / ulp; // exact (power-of-two scaling)
        let rq = round_half_even_int(q);
        rq * ulp
    };
    if neg {
        -r
    } else {
        r
    }
}

// ------------------------------------------------------------------------------------------------ Math object

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let m = vm.new_object(Some(op));
    for (n, v) in [
        ("E", 2.718281828459045),
        ("LN10", 2.302585092994046),
        ("LN2", 0.6931471805599453),
        ("LOG10E", 0.4342944819032518),
        ("LOG2E", 1.4426950408889634),
        ("PI", 3.141592653589793),
        ("SQRT1_2", 0.7071067811865476),
        ("SQRT2", 1.4142135623730951),
    ] {
        value(vm, m, n, Value::Number(v), 0);
    }
    macro_rules! f1 {
        ($name:expr, $f:expr) => {{
            fn h(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
                let a = vm.arg(ctx, 0);
                let x = vm.to_number(&a)?;
                let f: fn(f64) -> f64 = $f;
                Ok(Value::Number(f(x)))
            }
            method(vm, m, $name, 1, h);
        }};
    }
    f1!("abs", |x| x.abs());
    f1!("acos", acos);
    f1!("acosh", acosh);
    f1!("asin", asin);
    f1!("asinh", asinh);
    f1!("atan", atan);
    f1!("atanh", atanh);
    f1!("cbrt", cbrt);
    f1!("ceil", ceil);
    f1!("cos", cos);
    f1!("cosh", cosh);
    f1!("exp", exp);
    f1!("expm1", expm1);
    f1!("floor", libm_floor);
    f1!("fround", |x| x as f32 as f64);
    f1!("f16round", f16round);
    f1!("log", log);
    f1!("log1p", log1p);
    f1!("log10", log10);
    f1!("log2", log2);
    f1!("round", round);
    f1!("sign", |x| if x.is_nan() || x == 0.0 { x } else if x > 0.0 { 1.0 } else { -1.0 });
    f1!("sin", sin);
    f1!("sinh", sinh);
    f1!("sqrt", sqrt);
    f1!("tan", tan);
    f1!("tanh", tanh);
    f1!("trunc", trunc);
    method(vm, m, "atan2", 2, math_atan2);
    method(vm, m, "clz32", 1, clz32);
    method(vm, m, "hypot", 2, hypot);
    method(vm, m, "imul", 2, imul);
    method(vm, m, "max", 2, max);
    method(vm, m, "min", 2, min);
    method(vm, m, "pow", 2, math_pow);
    method(vm, m, "random", 0, random);
    to_str_tag(vm, m, "Math");
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.math = m;
    global(vm, "Math", Value::Object(m));
}

fn math_atan2(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (a, b) = (vm.arg(ctx, 0), vm.arg(ctx, 1));
    let y = vm.to_number(&a)?;
    let x = vm.to_number(&b)?;
    Ok(Value::Number(atan2(y, x)))
}
fn math_pow(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (a, b) = (vm.arg(ctx, 0), vm.arg(ctx, 1));
    let x = vm.to_number(&a)?;
    let y = vm.to_number(&b)?;
    Ok(Value::Number(pow(x, y)))
}
fn clz32(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let n = vm.to_uint32(&a)?;
    Ok(Value::Number(n.leading_zeros() as f64))
}
fn imul(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (a, b) = (vm.arg(ctx, 0), vm.arg(ctx, 1));
    let x = vm.to_uint32(&a)?;
    let y = vm.to_uint32(&b)?;
    Ok(Value::Number(x.wrapping_mul(y) as i32 as f64))
}
fn hypot(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let mut nums = Vec::with_capacity(ctx.argc);
    for i in 0..ctx.argc {
        let a = vm.arg(ctx, i);
        nums.push(vm.to_number(&a)?);
    }
    if nums.iter().any(|x| x.is_infinite()) {
        return Ok(Value::Number(f64::INFINITY));
    }
    if nums.iter().any(|x| x.is_nan()) {
        return Ok(Value::Number(f64::NAN));
    }
    let max = nums.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    if max == 0.0 {
        return Ok(Value::Number(0.0));
    }
    let mut s = DD::from(0.0);
    for x in &nums {
        let t = DD::from(x.abs() / max);
        s = s.add(t.mul(t));
    }
    Ok(Value::Number(s.sqrt().val() * max))
}
fn max(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let mut r = f64::NEG_INFINITY;
    let mut nan = false;
    for i in 0..ctx.argc {
        let a = vm.arg(ctx, i);
        let x = vm.to_number(&a)?;
        if x.is_nan() {
            nan = true;
        } else if x > r || (x == 0.0 && r == 0.0 && !x.is_sign_negative()) {
            r = x;
        }
    }
    Ok(Value::Number(if nan { f64::NAN } else { r }))
}
fn min(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let mut r = f64::INFINITY;
    let mut nan = false;
    for i in 0..ctx.argc {
        let a = vm.arg(ctx, i);
        let x = vm.to_number(&a)?;
        if x.is_nan() {
            nan = true;
        } else if x < r || (x == 0.0 && r == 0.0 && x.is_sign_negative()) {
            r = x;
        }
    }
    Ok(Value::Number(if nan { f64::NAN } else { r }))
}
fn random(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Number(vm.random()))
}
