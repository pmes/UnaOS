//! The float functions `core` does not give a `no_std` crate, in f64 (the shader lanes are f32; computing the
//! builtins in f64 and rounding once keeps them inside GLSL ES 1.00 §4.5.2's highp relative error, 2^-16,
//! with margin). The same shape as svg_core's `fmath` (Cody–Waite reduction, series on a reduced range).

use core::f64::consts::{FRAC_PI_2, LN_2, PI, SQRT_2};

pub fn trunc(x: f64) -> f64 {
    if x.is_nan() || x.abs() >= 4_503_599_627_370_496.0 {
        return x;
    }
    x as i64 as f64
}

pub fn floor(x: f64) -> f64 {
    let t = trunc(x);
    if t > x { t - 1.0 } else { t }
}

pub fn ceil(x: f64) -> f64 {
    -floor(-x)
}

/// Round half away from zero.
pub fn round(x: f64) -> f64 {
    if x < 0.0 { -floor(-x + 0.5) } else { floor(x + 0.5) }
}

pub fn fract(x: f64) -> f64 {
    x - floor(x)
}

pub fn sqrt(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let mut y = f64::from_bits((x.to_bits() >> 1) + 0x1ff7_a3be_a91d_9b1b);
    for _ in 0..5 {
        y = 0.5 * (y + x / y);
    }
    y
}

/// sin and cos with the argument reduced to [-π/4, π/4] by quadrant (two-part Cody–Waite π/2).
pub fn sin_cos(x: f64) -> (f64, f64) {
    if !x.is_finite() {
        return (f64::NAN, f64::NAN);
    }
    let q = round(x / FRAC_PI_2);
    const P1: f64 = 1.570_796_326_734_125_6;
    const P2: f64 = 6.077_100_506_506_192e-11;
    let r = (x - q * P1) - q * P2;
    let r2 = r * r;
    let (mut s, mut t, mut c, mut u) = (r, r, 1.0, 1.0);
    for k in 1..=9 {
        t *= -r2 / ((2 * k) as f64 * (2 * k + 1) as f64);
        s += t;
        u *= -r2 / ((2 * k - 1) as f64 * (2 * k) as f64);
        c += u;
    }
    match (q as i64).rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

pub fn sin(x: f64) -> f64 {
    sin_cos(x).0
}

pub fn cos(x: f64) -> f64 {
    sin_cos(x).1
}

pub fn tan(x: f64) -> f64 {
    let (s, c) = sin_cos(x);
    s / c
}

fn atan_unit(x: f64) -> f64 {
    let x1 = x / (1.0 + sqrt(1.0 + x * x));
    let x2 = x1 / (1.0 + sqrt(1.0 + x1 * x1));
    let z = x2 * x2;
    let (mut term, mut s) = (x2, x2);
    for k in 1..=14 {
        term *= -z;
        s += term / (2 * k + 1) as f64;
    }
    4.0 * s
}

pub fn atan(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let a = x.abs();
    let r = if a <= 1.0 { atan_unit(a) } else { FRAC_PI_2 - atan_unit(1.0 / a) };
    if x < 0.0 { -r } else { r }
}

/// GLSL ES 1.00 §8.1 `atan(y, x)`: the angle in [-π, π]; undefined when both are 0 (we answer 0).
pub fn atan2(y: f64, x: f64) -> f64 {
    if x == 0.0 {
        return if y > 0.0 {
            FRAC_PI_2
        } else if y < 0.0 {
            -FRAC_PI_2
        } else {
            0.0
        };
    }
    let a = atan(y / x);
    if x > 0.0 {
        a
    } else if y >= 0.0 {
        a + PI
    } else {
        a - PI
    }
}

pub fn asin(x: f64) -> f64 {
    if !(-1.0..=1.0).contains(&x) {
        return f64::NAN;
    }
    atan2(x, sqrt(1.0 - x * x))
}

pub fn acos(x: f64) -> f64 {
    if !(-1.0..=1.0).contains(&x) {
        return f64::NAN;
    }
    atan2(sqrt(1.0 - x * x), x)
}

/// Natural logarithm: frexp, then the atanh series on m ∈ [√½, √2].
pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if !x.is_finite() {
        return x;
    }
    let bits = x.to_bits();
    let e0 = ((bits >> 52) & 0x7ff) as i64;
    if e0 == 0 {
        return ln(x * 4_503_599_627_370_496.0) - 52.0 * LN_2;
    }
    let mut e = e0 - 1023;
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    if m > SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let (mut term, mut sum) = (s, s);
    for k in 1..=20 {
        term *= s2;
        sum += term / (2 * k + 1) as f64;
    }
    2.0 * sum + e as f64 * LN_2
}

pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x > 709.0 {
        return f64::INFINITY;
    }
    if x < -745.0 {
        return 0.0;
    }
    let k = round(x / LN_2);
    let r = x - k * LN_2;
    let (mut term, mut sum) = (1.0, 1.0);
    for i in 1..=20 {
        term *= r / i as f64;
        sum += term;
    }
    let ki = k as i64;
    let half = ki / 2;
    let p1 = f64::from_bits(((half + 1023) as u64) << 52);
    let p2 = f64::from_bits(((ki - half + 1023) as u64) << 52);
    sum * p1 * p2
}

pub fn exp2(x: f64) -> f64 {
    exp(x * LN_2)
}

pub fn log2(x: f64) -> f64 {
    ln(x) / LN_2
}

/// GLSL ES 1.00 §8.2 `pow`: undefined for x < 0 or x = 0 with y ≤ 0 (we answer NaN / 0 / inf the C way).
pub fn pow(x: f64, y: f64) -> f64 {
    if y == 0.0 {
        return 1.0;
    }
    if x == 0.0 {
        return if y > 0.0 { 0.0 } else { f64::INFINITY };
    }
    if x < 0.0 {
        return f64::NAN;
    }
    exp(y * ln(x))
}
