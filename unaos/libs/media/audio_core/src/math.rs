//! The transcendental functions the lossy codecs need, for `no_std` (core has no `f64::sin`): double
//! precision, range-reduced series, accurate to a few ulp — they only build tables and gains, never touch
//! a lossless path. Tested against `std` below.

pub const PI: f64 = core::f64::consts::PI;

#[inline]
pub fn floor(x: f64) -> f64 {
    if !(x.abs() < 4503599627370496.0) { return x; }
    let t = x as i64 as f64;
    if t > x { t - 1.0 } else { t }
}
#[inline]
pub fn ceil(x: f64) -> f64 { -floor(-x) }
/// Round half away from zero (C `round`).
#[inline]
pub fn round(x: f64) -> f64 { if x >= 0.0 { floor(x + 0.5) } else { -floor(-x + 0.5) } }
#[inline]
pub fn fabs(x: f64) -> f64 { x.abs() }

pub fn sqrt(x: f64) -> f64 {
    if x <= 0.0 || x.is_nan() || x.is_infinite() { return if x == 0.0 { 0.0 } else if x > 0.0 { x } else { f64::NAN }; }
    // initial guess by halving the exponent, then Newton (quadratic: 6 steps are far more than enough)
    let b = x.to_bits();
    let mut y = f64::from_bits((b >> 1) + 0x1FF8_0000_0000_0000);
    for _ in 0..6 { y = 0.5 * (y + x / y); }
    y
}

/// sin and cos of `x` (radians).
pub fn sincos(x: f64) -> (f64, f64) {
    // reduce to r in [-pi/4, pi/4], quadrant q
    let q = round(x * (2.0 / PI));
    // Cody-Waite with a split pi/2
    const P1: f64 = 1.570_796_326_734_125_6;
    const P2: f64 = 6.077_100_506_506_192e-11;
    const P3: f64 = 2.022_266_248_795_950_7e-21;
    let r = ((x - q * P1) - q * P2) - q * P3;
    let r2 = r * r;
    let s = r * (1.0 + r2 * (-1.0 / 6.0 + r2 * (1.0 / 120.0 + r2 * (-1.0 / 5040.0 + r2 * (1.0 / 362880.0 + r2 * (-1.0 / 39916800.0 + r2 * (1.0 / 6227020800.0 + r2 * (-1.0 / 1307674368000.0))))))));
    let c = 1.0 + r2 * (-0.5 + r2 * (1.0 / 24.0 + r2 * (-1.0 / 720.0 + r2 * (1.0 / 40320.0 + r2 * (-1.0 / 3628800.0 + r2 * (1.0 / 479001600.0 + r2 * (-1.0 / 87178291200.0)))))));
    match (q as i64).rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}
#[inline]
pub fn sin(x: f64) -> f64 { sincos(x).0 }
#[inline]
pub fn cos(x: f64) -> f64 { sincos(x).1 }

/// e^x.
pub fn exp(x: f64) -> f64 {
    if x > 709.0 { return f64::INFINITY; }
    if x < -745.0 { return 0.0; }
    const LN2: f64 = core::f64::consts::LN_2;
    let k = round(x / LN2);
    let r = x - k * LN2; // |r| <= ln2/2
    // e^r by Taylor to r^14 (|r|<0.35: error < 1e-17)
    let mut t = 1.0;
    let mut s = 1.0;
    for i in 1..16 { t *= r / i as f64; s += t; }
    ldexp(s, k as i32)
}

/// x * 2^e.
pub fn ldexp(mut x: f64, mut e: i32) -> f64 {
    while e > 1000 { x *= f64::from_bits(0x7E70_0000_0000_0000); e -= 1000; } // 2^1000
    while e < -1000 { x *= f64::from_bits(0x0170_0000_0000_0000); e += 1000; } // 2^-1000
    x * f64::from_bits(((e + 1023) as u64) << 52)
}

/// Natural log.
pub fn ln(x: f64) -> f64 {
    if x <= 0.0 { return if x == 0.0 { f64::NEG_INFINITY } else { f64::NAN }; }
    if x.is_infinite() { return x; }
    let mut b = x.to_bits();
    let mut e = 0i32;
    if b >> 52 == 0 { // subnormal
        let y = x * f64::from_bits(0x4350_0000_0000_0000); // 2^54
        b = y.to_bits();
        e -= 54;
    }
    e += ((b >> 52) & 0x7FF) as i32 - 1023;
    let mut m = f64::from_bits((b & 0x000F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000); // [1,2)
    if m > core::f64::consts::SQRT_2 { m *= 0.5; e += 1; }
    // ln(m) = 2 atanh((m-1)/(m+1)), |z| <= 0.172
    let z = (m - 1.0) / (m + 1.0);
    let z2 = z * z;
    let mut t = z;
    let mut s = 0.0;
    let mut k = 1.0;
    for _ in 0..12 { s += t / k; t *= z2; k += 2.0; }
    2.0 * s + e as f64 * core::f64::consts::LN_2
}
#[inline]
pub fn log2(x: f64) -> f64 { ln(x) * core::f64::consts::LOG2_E }
#[inline]
pub fn log10(x: f64) -> f64 { ln(x) * core::f64::consts::LOG10_E }
#[inline]
pub fn exp2(x: f64) -> f64 { exp(x * core::f64::consts::LN_2) }

/// x^y for x >= 0 (the codecs never raise a negative base to a fractional power).
pub fn pow(x: f64, y: f64) -> f64 {
    if y == 0.0 { return 1.0; }
    if x == 0.0 { return if y > 0.0 { 0.0 } else { f64::INFINITY }; }
    if x < 0.0 {
        let yi = y as i64;
        if yi as f64 == y { let p = exp(y * ln(-x)); return if yi & 1 == 1 { -p } else { p }; }
        return f64::NAN;
    }
    exp(y * ln(x))
}

/// atan(x).
pub fn atan(x: f64) -> f64 {
    let (neg, a) = if x < 0.0 { (true, -x) } else { (false, x) };
    let (inv, a) = if a > 1.0 { (true, 1.0 / a) } else { (false, a) };
    // two halvings: atan(a) = 2 atan(a / (1 + sqrt(1 + a^2)))
    let a1 = a / (1.0 + sqrt(1.0 + a * a));
    let a2 = a1 / (1.0 + sqrt(1.0 + a1 * a1));
    let z2 = a2 * a2;
    let mut t = a2;
    let mut s = 0.0;
    let mut k = 1.0;
    for i in 0..14 { s += if i & 1 == 0 { t / k } else { -t / k }; t *= z2; k += 2.0; }
    let mut r = 4.0 * s;
    if inv { r = PI / 2.0 - r; }
    if neg { -r } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn close(a: f64, b: f64) -> bool { (a - b).abs() <= 1e-13 * b.abs().max(1.0) }
    #[test]
    fn against_std() {
        let mut x = -40.0f64;
        while x < 40.0 {
            assert!(close(sin(x), std::primitive::f64::sin(x)), "sin {}", x);
            assert!(close(cos(x), std::primitive::f64::cos(x)), "cos {}", x);
            assert!(close(exp(x / 4.0), (x / 4.0).exp()), "exp {}", x);
            assert!(close(atan(x), x.atan()), "atan {}", x);
            if x > 0.0 {
                assert!(close(ln(x), x.ln()), "ln {}", x);
                assert!(close(sqrt(x), x.sqrt()), "sqrt {}", x);
                assert!(close(pow(x, 4.0 / 3.0), x.powf(4.0 / 3.0)), "pow {}", x);
            }
            assert_eq!(floor(x), x.floor());
            assert_eq!(round(x), x.round());
            x += 0.0137;
        }
        assert!(close(ln(1e-310), (1e-310f64).ln()));
        assert!(close(exp2(-30.5), (-30.5f64).exp2()));
    }
}
