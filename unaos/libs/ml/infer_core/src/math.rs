// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The transcendental functions the encoder needs, ours (`core` has no `exp`/`erf`/`sqrt` without
//! `std`, and `std`'s defer to the platform libm, which differs between machines). All are
//! evaluated in f64 by fixed algorithms; the f32 results are therefore identical everywhere.

/// e^x. Cody–Waite reduction x = k·ln2 + r (|r| ≤ ln2/2), a degree-13 Taylor polynomial for e^r
/// (truncation < 2e-17 relative), and 2^k assembled from bits.
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
    const LN2_HI: f64 = 6.931_471_803_691_238_164_90e-1;
    const LN2_LO: f64 = 1.908_214_929_270_587_700_02e-10;
    const INV_LN2: f64 = core::f64::consts::LOG2_E;
    let kf = round_half_even(x * INV_LN2);
    let k = kf as i64;
    let r = (x - kf * LN2_HI) - kf * LN2_LO;
    // Horner, highest degree first.
    let mut p = 1.0 / 6_227_020_800.0; // 1/13!
    for d in (1..13).rev() {
        p = p * r + 1.0 / factorial(d);
    }
    let er = p * r + 1.0;
    scale2(er, k)
}

const fn factorial(n: u32) -> f64 {
    let mut f = 1.0;
    let mut i = 2;
    while i <= n {
        f *= i as f64;
        i += 1;
    }
    f
}

fn round_half_even(x: f64) -> f64 {
    // |x| < 2^52 here, so adding and subtracting 2^52 rounds to an integer (ties to even).
    const BIG: f64 = 4_503_599_627_370_496.0;
    if x >= 0.0 { (x + BIG) - BIG } else { (x - BIG) + BIG }
}

/// x · 2^k for normal results, with a two-step scale so subnormal results stay exact enough.
fn scale2(x: f64, k: i64) -> f64 {
    let pow = |e: i64| f64::from_bits(((e + 1023) as u64) << 52);
    if k > 1023 {
        x * pow(1023) * pow(k - 1023)
    } else if k < -1022 {
        x * pow(-1022) * pow((k + 1022).max(-1022))
    } else {
        x * pow(k)
    }
}

/// √x for x ≥ 0 (NaN for x < 0): Newton–Raphson in f64 from a bit-level estimate, then a final
/// correction step so the result is the correctly rounded f64 square root for normal inputs.
pub fn sqrt(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 || x == f64::INFINITY {
        return x;
    }
    // Normalise subnormals.
    let (m, adj) = if x < f64::MIN_POSITIVE { (x * 18_014_398_509_481_984.0 /* 2^54 */, 27) } else { (x, 0) };
    let mut y = f64::from_bits((m.to_bits() >> 1) + (1023u64 << 51));
    for _ in 0..6 {
        y = 0.5 * (y + m / y);
    }
    // One exact-residual correction: among y and its neighbours take the one whose square is
    // closest to m, the squares formed exactly (Dekker's two-product).
    let lo = f64::from_bits(y.to_bits() - 1);
    let hi = f64::from_bits(y.to_bits() + 1);
    let resid = |c: f64| {
        let (p, e) = two_prod(c, c);
        ((p - m) + e).abs()
    };
    let mut best = y;
    for c in [lo, hi] {
        if resid(c) < resid(best) {
            best = c;
        }
    }
    if adj != 0 { best / 134_217_728.0 /* 2^27 */ } else { best }
}

/// a·b = p + e exactly (Veltkamp split; no FMA needed).
fn two_prod(a: f64, b: f64) -> (f64, f64) {
    let split = |x: f64| {
        let t = 134_217_729.0 * x; // 2^27 + 1
        let hi = t - (t - x);
        (hi, x - hi)
    };
    let p = a * b;
    let (ah, al) = split(a);
    let (bh, bl) = split(b);
    (p, ((ah * bh - p) + ah * bl + al * bh) + al * bl)
}

/// erf(x), |error| < 1e-15: for |x| < 3 the everywhere-positive series
/// erf(x) = (2/√π)·e^{-x²}·Σ 2ⁿx^{2n+1}/(1·3·…·(2n+1)); beyond, 1 − erfc(x) from the continued
/// fraction (see [`erfc`]).
pub fn erf(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let a = x.abs();
    let v = if a < 3.0 { erf_series(a) } else { 1.0 - erfc_cf(a) };
    if x < 0.0 { -v } else { v }
}

/// erfc(x) = 1 − erf(x), accurate in the tails (so GELU of a large negative input is not 0).
pub fn erfc(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x < 0.0 {
        return 2.0 - erfc(-x);
    }
    if x < 3.0 { 1.0 - erf_series(x) } else { erfc_cf(x) }
}

const TWO_OVER_SQRT_PI: f64 = core::f64::consts::FRAC_2_SQRT_PI;

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
fn erfc_cf(x: f64) -> f64 {
    let mut f = x;
    for k in (1..=60).rev() {
        f = x + (k as f64 * 0.5) / f;
    }
    exp(-x * x) / (f * SQRT_PI)
}

/// √π.
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
    fn gelu_known_answers() {
        assert_eq!(gelu(0.0), 0.0);
        assert!((gelu(1.0) - 0.841_344_74).abs() < 1e-7);
        assert!((gelu(-1.0) + 0.158_655_25).abs() < 1e-7);
        assert!(gelu(-10.0) < 0.0 && gelu(-10.0) > -1e-20);
    }
}
