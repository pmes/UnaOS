//! The float functions `core` does not give a `no_std` crate (sqrt, trig, pow), in f64. Accuracy is a few ulp
//! over the ranges SVG geometry uses, which is far below what the 8-bit canvas can show.

use core::f64::consts::{FRAC_PI_2, PI};

pub fn floor(x: f64) -> f64 {
    if x.is_nan() || x.abs() >= 4_503_599_627_370_496.0 {
        return x;
    }
    let t = x as i64 as f64;
    if t > x { t - 1.0 } else { t }
}

pub fn ceil(x: f64) -> f64 {
    -floor(-x)
}

pub fn round(x: f64) -> f64 {
    floor(x + 0.5)
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

pub fn hypot(x: f64, y: f64) -> f64 {
    let (ax, ay) = (x.abs(), y.abs());
    let m = ax.max(ay);
    if m == 0.0 || !m.is_finite() {
        return m;
    }
    let (a, b) = (ax / m, ay / m);
    m * sqrt(a * a + b * b)
}

/// sin and cos for |x| reduced to [-π/4, π/4] by quadrant.
pub fn sin_cos(x: f64) -> (f64, f64) {
    if !x.is_finite() {
        return (f64::NAN, f64::NAN);
    }
    let q = round(x / FRAC_PI_2);
    // Two-part Cody–Waite reduction of π/2.
    const P1: f64 = 1.570_796_326_734_125_6;
    const P2: f64 = 6.077_100_506_506_192e-11;
    let r = (x - q * P1) - q * P2;
    let r2 = r * r;
    // Taylor to r^17 / r^16 — |r| ≤ π/4 makes the tail below 1e-17.
    let mut s = r;
    let mut t = r;
    let mut c = 1.0;
    let mut u = 1.0;
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

/// atan on [0, 1] by argument halving and a series.
fn atan_unit(x: f64) -> f64 {
    // atan(x) = 2 atan(x / (1 + sqrt(1 + x²))), twice, brings x below 0.2.
    let x1 = x / (1.0 + sqrt(1.0 + x * x));
    let x2 = x1 / (1.0 + sqrt(1.0 + x1 * x1));
    let z = x2 * x2;
    let mut term = x2;
    let mut s = x2;
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

pub fn acos(x: f64) -> f64 {
    let x = x.clamp(-1.0, 1.0);
    atan2(sqrt(1.0 - x * x), x)
}

/// Natural log (x > 0) via frexp + atanh series.
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
    let mut e = ((bits >> 52) & 0x7ff) as i64;
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    if e == 0 {
        // subnormal
        let y = x * 4_503_599_627_370_496.0;
        return ln(y) - 52.0 * core::f64::consts::LN_2;
    }
    e -= 1023;
    if m > core::f64::consts::SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let mut term = s;
    let mut sum = s;
    for k in 1..=20 {
        term *= s2;
        sum += term / (2 * k + 1) as f64;
    }
    2.0 * sum + e as f64 * core::f64::consts::LN_2
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
    let k = round(x / core::f64::consts::LN_2);
    let r = x - k * core::f64::consts::LN_2;
    let mut term = 1.0;
    let mut sum = 1.0;
    for i in 1..=20 {
        term *= r / i as f64;
        sum += term;
    }
    let ki = k as i64;
    // 2^k by exponent bits (k within the normal range here, split to stay finite).
    let half = ki / 2;
    let p1 = f64::from_bits(((half + 1023) as u64) << 52);
    let p2 = f64::from_bits(((ki - half + 1023) as u64) << 52);
    sum * p1 * p2
}

pub fn powf(x: f64, y: f64) -> f64 {
    if y == 0.0 {
        return 1.0;
    }
    if x == 0.0 {
        return 0.0;
    }
    if x < 0.0 {
        return f64::NAN;
    }
    exp(y * ln(x))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn functions() {
        for i in -200..200 {
            let x = i as f64 * 0.137;
            let (s, c) = sin_cos(x);
            assert!((s * s + c * c - 1.0).abs() < 1e-14);
        }
        assert!((sin(PI / 6.0) - 0.5).abs() < 1e-15);
        assert!((cos(PI / 3.0) - 0.5).abs() < 1e-15);
        assert!((atan2(1.0, 1.0) - PI / 4.0).abs() < 1e-15);
        assert!((atan2(1.0, -1.0) - 3.0 * PI / 4.0).abs() < 1e-15);
        assert!((atan2(-1.0, -1.0) + 3.0 * PI / 4.0).abs() < 1e-15);
        assert!((acos(0.5) - PI / 3.0).abs() < 1e-14);
        assert!((sqrt(2.0) - core::f64::consts::SQRT_2).abs() < 1e-15);
        assert!((ln(10.0) - core::f64::consts::LN_10).abs() < 1e-14);
        assert!((exp(1.0) - core::f64::consts::E).abs() < 1e-14);
        assert!((powf(2.0, 0.5) - core::f64::consts::SQRT_2).abs() < 1e-14);
        assert!((tan(0.3) - sin(0.3) / cos(0.3)).abs() < 1e-15);
    }
}
