//! The float functions `core` does not give a `no_std` crate, in f64 (the shader lanes are f32; computing the
//! builtins in f64 and rounding once keeps them inside GLSL ES 1.00 §4.5.2's highp relative error, 2^-16,
//! with margin). MATH-ONE (LEDGER SR362): the transcendentals and sqrt are libm's (the one f64 math, no_std,
//! `default-features = false`); this file keeps only GLSL's domain answers and the exact rounding ops.

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
    libm::sqrt(x)
}

/// sin and cos (libm's `sincos`; a non-finite argument answers NaN, GLSL's undefined).
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

/// GLSL ES 1.00 §8.1 `atan(y, x)`: the angle in [-π, π]; undefined when both are 0 (we answer 0).
pub fn atan2(y: f64, x: f64) -> f64 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    libm::atan2(y, x)
}

pub fn asin(x: f64) -> f64 {
    if !(-1.0..=1.0).contains(&x) {
        return f64::NAN;
    }
    libm::asin(x)
}

pub fn acos(x: f64) -> f64 {
    if !(-1.0..=1.0).contains(&x) {
        return f64::NAN;
    }
    libm::acos(x)
}

/// Natural logarithm.
pub fn ln(x: f64) -> f64 {
    libm::log(x)
}

pub fn exp(x: f64) -> f64 {
    libm::exp(x)
}

pub fn exp2(x: f64) -> f64 {
    libm::exp2(x)
}

pub fn log2(x: f64) -> f64 {
    libm::log2(x)
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
    libm::pow(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: f64, b: f64) -> bool {
        a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
    }

    /// MATH-ONE (LEDGER SR362): every builtin here IS libm's on a sweep (bit for bit), and GLSL's domain answers stay.
    #[test]
    fn mathone_equals_libm() {
        for i in -4000..4000 {
            let x = i as f64 * 0.0173 + 1e-3;
            let (s, c) = sin_cos(x);
            assert!(same(s, libm::sin(x)) && same(c, libm::cos(x)), "sin_cos {x}");
            assert!(same(tan(x), libm::tan(x)), "tan {x}");
            assert!(same(atan(x), libm::atan(x)), "atan {x}");
            assert!(same(atan2(x, 0.37 - x), libm::atan2(x, 0.37 - x)), "atan2 {x}");
            assert!(same(exp(x / 8.0), libm::exp(x / 8.0)), "exp {x}");
            assert!(same(exp2(x / 8.0), libm::exp2(x / 8.0)), "exp2 {x}");
            let a = x.abs() * 3.1;
            assert!(same(sqrt(a), libm::sqrt(a)), "sqrt {a}");
            assert!(same(ln(a), libm::log(a)), "ln {a}");
            assert!(same(log2(a), libm::log2(a)), "log2 {a}");
            assert!(same(pow(a, x / 16.0), libm::pow(a, x / 16.0)), "pow {a} {x}");
            let u = (i as f64 / 4000.0).clamp(-1.0, 1.0);
            assert!(same(asin(u), libm::asin(u)) && same(acos(u), libm::acos(u)), "asin/acos {u}");
        }
        // GLSL ES 1.00 §8.1 / §8.2 undefined cases keep their answers.
        assert_eq!(atan2(0.0, 0.0), 0.0);
        assert!(pow(-2.0, 0.5).is_nan() && pow(-2.0, 2.0).is_nan());
        assert_eq!((pow(0.0, 2.0), pow(0.0, -1.0), pow(5.0, 0.0)), (0.0, f64::INFINITY, 1.0));
        assert!(sin_cos(f64::INFINITY).0.is_nan() && asin(1.5).is_nan() && acos(-1.5).is_nan());
    }
}
