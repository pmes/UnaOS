//! The transcendental functions the lossy codecs need, for `no_std` (core has no `f64::sin`), in double
//! precision — they only build tables and gains, never touch a lossless path. MATH-ONE (LEDGER SR362): the
//! transcendentals and sqrt are libm's (the one f64 math, no_std, `default-features = false`); the names and
//! signatures stay (every decoder, `ramp::gain_q16` and the AAC encoder call them), the exact rounding ops,
//! `ldexp` and `bessel_i0` (no libm twin) stay ours. Tested against `std` below.

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

pub fn sqrt(x: f64) -> f64 { libm::sqrt(x) }

/// sin and cos of `x` (radians).
pub fn sincos(x: f64) -> (f64, f64) { libm::sincos(x) }
#[inline]
pub fn sin(x: f64) -> f64 { sincos(x).0 }
#[inline]
pub fn cos(x: f64) -> f64 { sincos(x).1 }

/// e^x.
pub fn exp(x: f64) -> f64 { libm::exp(x) }

/// x * 2^e.
pub fn ldexp(mut x: f64, mut e: i32) -> f64 {
    while e > 1000 { x *= f64::from_bits(0x7E70_0000_0000_0000); e -= 1000; } // 2^1000
    while e < -1000 { x *= f64::from_bits(0x0170_0000_0000_0000); e += 1000; } // 2^-1000
    x * f64::from_bits(((e + 1023) as u64) << 52)
}

/// Natural log.
pub fn ln(x: f64) -> f64 { libm::log(x) }
#[inline]
pub fn log2(x: f64) -> f64 { libm::log2(x) }
#[inline]
pub fn log10(x: f64) -> f64 { libm::log10(x) }
#[inline]
pub fn exp2(x: f64) -> f64 { libm::exp2(x) }

/// x^y (a negative base answers only for an integer y; the codecs never raise a negative base to a fractional power).
pub fn pow(x: f64, y: f64) -> f64 {
    if y == 0.0 { return 1.0; }
    if x == 0.0 { return if y > 0.0 { 0.0 } else { f64::INFINITY }; }
    libm::pow(x, y)
}

/// atan(x).
pub fn atan(x: f64) -> f64 { libm::atan(x) }

/// The modified Bessel function of the first kind, order 0 (NLE4B, SR222: the Kaiser window's), as its power series
/// `sum_k ((x/2)^2)^k / (k!)^2`, summed until a term no longer moves the sum.
pub fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let (mut s, mut term) = (1.0f64, 1.0f64);
    for k in 1..200 {
        term *= q / (k * k) as f64;
        s += term;
        if term <= 1e-17 * s { break; }
    }
    s
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
    #[test]
    fn bessel_i0_is_the_series() {
        // the known values I0(1) = 1.2660658777520082, I0(8) = 427.5641157218048
        let rel = |a: f64, b: f64| (a - b).abs() <= 1e-12 * b;
        assert_eq!(bessel_i0(0.0), 1.0);
        assert!(rel(bessel_i0(1.0), 1.266_065_877_752_008_4), "{}", bessel_i0(1.0));
        assert!(rel(bessel_i0(8.0), 427.564_115_721_804_8), "{}", bessel_i0(8.0));
        assert_eq!(bessel_i0(-8.0), bessel_i0(8.0));
    }

    /// MATH-ONE (LEDGER SR362): the transcendentals and sqrt ARE libm's on a sweep (bit for bit) — the names and
    /// signatures the decoders, `ramp::gain_q16` and the AAC encoder call are unchanged; the pins (mp4one_kat's fnv,
    /// opus md5, `gain_q16_matches_std`) measured unchanged at the retirement.
    #[test]
    fn mathone_equals_libm() {
        let same = |a: f64, b: f64| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan());
        for i in -4000..4000 {
            let x = i as f64 * 0.0173 + 1e-3;
            assert!(same(sin(x), libm::sin(x)) && same(cos(x), libm::cos(x)), "sin/cos {x}");
            assert!(same(atan(x), libm::atan(x)) && same(exp(x / 8.0), libm::exp(x / 8.0)), "atan/exp {x}");
            assert!(same(exp2(x / 8.0), libm::exp2(x / 8.0)), "exp2 {x}");
            let a = x.abs() * 3.1;
            assert!(same(sqrt(a), libm::sqrt(a)) && same(ln(a), libm::log(a)), "sqrt/ln {a}");
            assert!(same(log2(a), libm::log2(a)) && same(log10(a), libm::log10(a)), "log2/log10 {a}");
            assert!(same(pow(a, x / 16.0), libm::pow(a, x / 16.0)), "pow {a} {x}");
        }
        for q in 0..8192 {
            assert!(same(pow(q as f64, 4.0 / 3.0), libm::pow(q as f64, 4.0 / 3.0)), "pow43 {q}");
        }
        assert_eq!((pow(0.0, 2.0), pow(0.0, -1.0), pow(-3.0, 0.0), pow(-2.0, 3.0)), (0.0, f64::INFINITY, 1.0, -8.0));
        assert!(pow(-2.0, 0.5).is_nan());
    }
}
