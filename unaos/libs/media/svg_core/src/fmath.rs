//! The float functions `core` does not give a `no_std` crate (sqrt, trig, pow), in f64. MATH-ONE (LEDGER SR362):
//! the transcendentals, sqrt and hypot are libm's (the one f64 math, no_std, `default-features = false`); this
//! file keeps svg_core's domain answers and the exact rounding ops.

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
    libm::sqrt(x)
}

pub fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

/// sin and cos (libm's `sincos`; a non-finite argument answers NaN).
pub fn sin_cos(x: f64) -> (f64, f64) {
    if !x.is_finite() {
        return (f64::NAN, f64::NAN);
    }
    libm::sincos(x)
}

pub fn sin(x: f64) -> f64 {
    sin_cos(x).0
}
pub fn cos(x: f64) -> f64 {
    sin_cos(x).1
}
pub fn tan(x: f64) -> f64 {
    libm::tan(x)
}

pub fn atan(x: f64) -> f64 {
    libm::atan(x)
}

pub fn atan2(y: f64, x: f64) -> f64 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    libm::atan2(y, x)
}

pub fn acos(x: f64) -> f64 {
    libm::acos(x.clamp(-1.0, 1.0))
}

/// Natural log.
pub fn ln(x: f64) -> f64 {
    libm::log(x)
}

pub fn exp(x: f64) -> f64 {
    libm::exp(x)
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
    libm::pow(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::PI;
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

    /// MATH-ONE (LEDGER SR362): the transcendentals, sqrt and hypot ARE libm's on a sweep (bit for bit); the domain
    /// answers stay.
    #[test]
    fn mathone_equals_libm() {
        let same = |a: f64, b: f64| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan());
        for i in -4000..4000 {
            let x = i as f64 * 0.0173 + 1e-3;
            let (s, c) = sin_cos(x);
            assert!(same(s, libm::sin(x)) && same(c, libm::cos(x)), "sin_cos {x}");
            assert!(same(tan(x), libm::tan(x)) && same(atan(x), libm::atan(x)), "tan/atan {x}");
            assert!(same(atan2(x, 0.37 - x), libm::atan2(x, 0.37 - x)), "atan2 {x}");
            assert!(same(hypot(x, 0.37 - x), libm::hypot(x, 0.37 - x)), "hypot {x}");
            assert!(same(exp(x / 8.0), libm::exp(x / 8.0)), "exp {x}");
            let a = x.abs() * 3.1;
            assert!(same(sqrt(a), libm::sqrt(a)) && same(ln(a), libm::log(a)), "sqrt/ln {a}");
            assert!(same(powf(a, x / 16.0), libm::pow(a, x / 16.0)), "powf {a} {x}");
            let u = i as f64 / 4000.0;
            assert!(same(acos(u), libm::acos(u)), "acos {u}");
        }
        assert_eq!((atan2(0.0, 0.0), acos(2.0), powf(0.0, -1.0), powf(3.0, 0.0)), (0.0, 0.0, 0.0, 1.0));
        assert!(powf(-2.0, 2.0).is_nan() && sin_cos(f64::NAN).1.is_nan());
    }
}
