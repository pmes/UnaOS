// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The transcendental functions the encoder needs (`core` has no `exp`/`erf`/`sqrt` without `std`, and `std`'s
//! defer to the platform libm, which differs between machines). MATH-ONE (LEDGER SR362): they are the `libm`
//! crate's (pure Rust, `default-features = false`: the same software algorithms on every target), evaluated in
//! f64; the f32 results are therefore identical everywhere. The series and the continued fraction below stay as
//! the independent checks of erf / erfc.

/// e^x (libm).
pub fn exp(x: f64) -> f64 {
    libm::exp(x)
}

/// √x for x ≥ 0 (NaN for x < 0), correctly rounded (libm).
pub fn sqrt(x: f64) -> f64 {
    libm::sqrt(x)
}

/// erf(x) (libm: fdlibm's rational forms, < 1 ulp in f64).
pub fn erf(x: f64) -> f64 {
    libm::erf(x)
}

/// erfc(x) = 1 − erf(x), accurate in the tails (so GELU of a large negative input is not 0) (libm).
pub fn erfc(x: f64) -> f64 {
    libm::erfc(x)
}

#[cfg(test)]
const TWO_OVER_SQRT_PI: f64 = core::f64::consts::FRAC_2_SQRT_PI;

/// erf by the everywhere-positive series (2/√π)·e^{-x²}·Σ 2ⁿx^{2n+1}/(1·3·…·(2n+1)) (|x| < 3):
/// the independent check of the rational forms.
#[cfg(test)]
fn erf_series(a: f64) -> f64 {
    let x2 = a * a;
    let mut term = a;
    let mut sum = a;
    let mut n = 0u32;
    while n < 200 {
        n += 1;
        term *= 2.0 * x2 / (2 * n + 1) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    TWO_OVER_SQRT_PI * exp(-x2) * sum
}

/// erfc for x ≥ 3 by the continued fraction e^{-x²}/√π · 1/(x + (1/2)/(x + 1/(x + (3/2)/(x + …)))),
/// evaluated bottom-up with a fixed depth (60 levels: converged to < 1e-17 relative for x ≥ 3).
#[cfg(test)]
fn erfc_cf(x: f64) -> f64 {
    let mut f = x;
    for k in (1..=60).rev() {
        f = x + (k as f64 * 0.5) / f;
    }
    exp(-x * x) / (f * SQRT_PI)
}

/// √π.
#[cfg(test)]
const SQRT_PI: f64 = 1.772_453_850_905_516_027_3;

/// GELU, the exact (erf) form BERT uses: x·Φ(x) = ½·x·erfc(−x/√2), rounded once to f32.
pub fn gelu(x: f32) -> f32 {
    let x = x as f64;
    (0.5 * x * erfc(-x * core::f64::consts::FRAC_1_SQRT_2)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_known_answers() {
        assert_eq!(exp(0.0), 1.0);
        for (x, want) in [(1.0, core::f64::consts::E), (-1.0, 0.367_879_441_171_442_33), (10.0, 22_026.465_794_806_718), (-20.0, 2.061_153_622_438_557_8e-9), (0.5, 1.648_721_270_700_128_1)] {
            assert!(((exp(x) - want) / want).abs() < 4e-16, "exp({x}) = {}", exp(x));
        }
        assert!(exp(-740.0) > 0.0 && exp(-740.0) < 1e-320);
    }

    #[test]
    fn sqrt_is_correctly_rounded_on_squares_and_known_values() {
        for i in 1..2000u64 {
            let f = i as f64;
            assert_eq!(sqrt(f * f), f);
        }
        assert_eq!(sqrt(2.0), core::f64::consts::SQRT_2);
        let t = sqrt(1e-310);
        assert!((t * t / 1e-310 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn erf_known_answers() {
        // Reference values (Abramowitz & Stegun table 7.1 / high-precision).
        for (x, want) in [(0.1, 0.112_462_916_018_284_9), (0.5, 0.520_499_877_813_046_5), (1.0, 0.842_700_792_949_714_9), (2.0, 0.995_322_265_018_952_7), (2.9, 0.999_958_902_121_900_3), (3.5, 0.999_999_256_901_627_7)] {
            assert!((erf(x) - want).abs() < 2e-15, "erf({x}) = {}", erf(x));
            assert!((erf(-x) + want).abs() < 2e-15);
        }
        assert!((erfc(5.0) - 1.537_459_794_428_034_8e-12).abs() < 1e-24);
        assert!((erfc(3.0) - 2.209_049_699_858_544e-5).abs() < 1e-18);
        assert!((erfc(2.99) - 2.352_560_308_064_019_5e-5).abs() < 1e-15);
        assert!((erfc(3.01) - 2.073_896_363_713_263e-5).abs() < 1e-18);
    }

    #[test]
    fn rational_forms_agree_with_the_series_and_the_continued_fraction() {
        let mut worst = 0.0f64;
        for i in 0..30000 {
            let x = i as f64 * 1e-4; // [0, 3)
            let d = (erf(x) - erf_series(x)).abs();
            worst = worst.max(d);
            assert!(d < 2e-15, "erf({x}): {} vs series {}", erf(x), erf_series(x));
            assert!((erfc(-x) - (1.0 + erf_series(x))).abs() < 4e-15);
        }
        for i in 0..2300 {
            let x = 3.0 + i as f64 * 1e-2; // [3, 26): erfc stays a normal f64 (≥ 1e-296)
            let (a, b) = (erfc(x), erfc_cf(x));
            // The continued fraction's own e^{-x²} carries the rounding of x² (≈ x²·2^-53 relative).
            assert!(((a - b) / b).abs() < 2e-16 * (1.0 + x * x), "erfc({x}): {a} vs cf {b}");
        }
        assert!(worst < 2e-15);
    }

    #[test]
    fn gelu_known_answers() {
        assert_eq!(gelu(0.0), 0.0);
        assert!((gelu(1.0) - 0.841_344_74).abs() < 1e-7);
        assert!((gelu(-1.0) + 0.158_655_25).abs() < 1e-7);
        assert!(gelu(-10.0) < 0.0 && gelu(-10.0) > -1e-20);
    }

    /// MATH-ONE (LEDGER SR362): exp / sqrt / erf / erfc ARE libm's on a sweep (bit for bit); GELU rounds the same erfc.
    #[test]
    fn mathone_equals_libm() {
        let same = |a: f64, b: f64| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan());
        for i in -6000..6000 {
            let x = i as f64 * 0.00731;
            assert!(same(exp(x * 8.0), libm::exp(x * 8.0)), "exp {x}");
            assert!(same(sqrt(x.abs() * 7.0), libm::sqrt(x.abs() * 7.0)), "sqrt {x}");
            assert!(same(erf(x), libm::erf(x)) && same(erfc(x), libm::erfc(x)), "erf/erfc {x}");
            let g = x as f32;
            assert_eq!(gelu(g).to_bits(), ((0.5 * g as f64 * libm::erfc(-(g as f64) * core::f64::consts::FRAC_1_SQRT_2)) as f32).to_bits());
        }
        assert!(sqrt(-1.0).is_nan() && exp(f64::NAN).is_nan());
    }
}
