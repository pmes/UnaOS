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

// erf / erfc follow FreeBSD msun `s_erf.c` (fdlibm): rational approximations on [0, 0.84375),
// [0.84375, 1.25), [1.25, 1/0.35) and [1/0.35, 28), each with error below 2^-57 relative to the
// function it approximates. The coefficients below are fdlibm's, carried with its notice:
//
//   Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//   Developed at SunPro, a Sun Microsystems, Inc. business.
//   Permission to use, copy, modify, and distribute this software is freely granted, provided
//   that this notice is preserved.
//
// The everywhere-positive series (`erf_series`) and the continued fraction (`erfc_cf`) below are
// independent evaluations; the unit tests hold the rational forms to them on a dense grid.
#[allow(clippy::excessive_precision)]
mod fd {
    pub const ERX: f64 = 8.45062911510467529297e-01;
    pub const PP0: f64 = 1.28379167095512558561e-01;
    pub const PP1: f64 = -3.25042107247001499370e-01;
    pub const PP2: f64 = -2.84817495755985104766e-02;
    pub const PP3: f64 = -5.77027029648944159157e-03;
    pub const PP4: f64 = -2.37630166566501626084e-05;
    pub const QQ1: f64 = 3.97917223959155352819e-01;
    pub const QQ2: f64 = 6.50222499887672944485e-02;
    pub const QQ3: f64 = 5.08130628187576562776e-03;
    pub const QQ4: f64 = 1.32494738004321644526e-04;
    pub const QQ5: f64 = -3.96022827877536812320e-06;
    pub const PA0: f64 = -2.36211856075265944077e-03;
    pub const PA1: f64 = 4.14856118683748331666e-01;
    pub const PA2: f64 = -3.72207876035701323847e-01;
    pub const PA3: f64 = 3.18346619901161753674e-01;
    pub const PA4: f64 = -1.10894694282396677476e-01;
    pub const PA5: f64 = 3.54783043256182359371e-02;
    pub const PA6: f64 = -2.16637559486879084300e-03;
    pub const QA1: f64 = 1.06420880400844228286e-01;
    pub const QA2: f64 = 5.40397917702171048937e-01;
    pub const QA3: f64 = 7.18286544141962662868e-02;
    pub const QA4: f64 = 1.26171219808761642112e-01;
    pub const QA5: f64 = 1.36370839120290507362e-02;
    pub const QA6: f64 = 1.19844998467991074170e-02;
    pub const RA0: f64 = -9.86494403484714822705e-03;
    pub const RA1: f64 = -6.93858572707181764372e-01;
    pub const RA2: f64 = -1.05586262253232909814e+01;
    pub const RA3: f64 = -6.23753324503260060396e+01;
    pub const RA4: f64 = -1.62396669462573470355e+02;
    pub const RA5: f64 = -1.84605092906711035994e+02;
    pub const RA6: f64 = -8.12874355063065934246e+01;
    pub const RA7: f64 = -9.81432934416914548592e+00;
    pub const SA1: f64 = 1.96512716674392571292e+01;
    pub const SA2: f64 = 1.37657754143519042600e+02;
    pub const SA3: f64 = 4.34565877475229228821e+02;
    pub const SA4: f64 = 6.45387271733267880336e+02;
    pub const SA5: f64 = 4.29008140027567833386e+02;
    pub const SA6: f64 = 1.08635005541779435134e+02;
    pub const SA7: f64 = 6.57024977031928170135e+00;
    pub const SA8: f64 = -6.04244152148580987438e-02;
    pub const RB0: f64 = -9.86494292470009928597e-03;
    pub const RB1: f64 = -7.99283237680523006574e-01;
    pub const RB2: f64 = -1.77579549177547519889e+01;
    pub const RB3: f64 = -1.60636384855821916062e+02;
    pub const RB4: f64 = -6.37566443368389627722e+02;
    pub const RB5: f64 = -1.02509513161107724954e+03;
    pub const RB6: f64 = -4.83519191608651397019e+02;
    pub const SB1: f64 = 3.03380607434824582924e+01;
    pub const SB2: f64 = 3.25792512996573918826e+02;
    pub const SB3: f64 = 1.53672958608443695994e+03;
    pub const SB4: f64 = 3.19985821950859553908e+03;
    pub const SB5: f64 = 2.55305040643316442583e+03;
    pub const SB6: f64 = 4.74528541206955367215e+02;
    pub const SB7: f64 = -2.24409524465858183362e+01;
}
use fd::*;

/// The high 32 bits of |x| (fdlibm's interval selector).
fn hi_abs(x: f64) -> u32 {
    ((x.to_bits() >> 32) as u32) & 0x7fff_ffff
}

fn erf_small(x: f64) -> f64 {
    // x·R(x²) for |x| < 0.84375.
    let z = x * x;
    let r = PP0 + z * (PP1 + z * (PP2 + z * (PP3 + z * PP4)));
    let s = 1.0 + z * (QQ1 + z * (QQ2 + z * (QQ3 + z * (QQ4 + z * QQ5))));
    x * (r / s)
}

fn erf_mid(x: f64) -> f64 {
    // P1(s)/Q1(s), s = |x| − 1, for |x| in [0.84375, 1.25).
    let s = x.abs() - 1.0;
    let p = PA0 + s * (PA1 + s * (PA2 + s * (PA3 + s * (PA4 + s * (PA5 + s * PA6)))));
    let q = 1.0 + s * (QA1 + s * (QA2 + s * (QA3 + s * (QA4 + s * (QA5 + s * QA6)))));
    p / q
}

/// erfc(|x|) for |x| in [1.25, 28).
fn erfc_tail(x: f64) -> f64 {
    let x = x.abs();
    let s = 1.0 / (x * x);
    let (r, big_s) = if hi_abs(x) < 0x4006_db6d {
        (
            RA0 + s * (RA1 + s * (RA2 + s * (RA3 + s * (RA4 + s * (RA5 + s * (RA6 + s * RA7)))))),
            1.0 + s * (SA1 + s * (SA2 + s * (SA3 + s * (SA4 + s * (SA5 + s * (SA6 + s * (SA7 + s * SA8))))))),
        )
    } else {
        (
            RB0 + s * (RB1 + s * (RB2 + s * (RB3 + s * (RB4 + s * (RB5 + s * RB6))))),
            1.0 + s * (SB1 + s * (SB2 + s * (SB3 + s * (SB4 + s * (SB5 + s * (SB6 + s * SB7)))))),
        )
    };
    let z = f64::from_bits(x.to_bits() & 0xffff_ffff_0000_0000);
    exp(-z * z - 0.5625) * exp((z - x) * (z + x) + r / big_s) / x
}

/// erf(x) (fdlibm's method; < 1 ulp in f64).
pub fn erf(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let ix = hi_abs(x);
    if ix < 0x3feb_0000 {
        return x + erf_small(x);
    }
    let y = if ix < 0x3ff4_0000 {
        ERX + erf_mid(x)
    } else if ix < 0x4018_0000 {
        1.0 - erfc_tail(x)
    } else {
        1.0
    };
    if x < 0.0 { -y } else { y }
}

/// erfc(x) = 1 − erf(x), accurate in the tails (so GELU of a large negative input is not 0).
pub fn erfc(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let ix = hi_abs(x);
    if ix < 0x3feb_0000 {
        let y = erf_small(x);
        return if x < 0.25 { 1.0 - (x + y) } else { 0.5 - (x - 0.5 + y) };
    }
    if ix < 0x3ff4_0000 {
        let p = erf_mid(x);
        return if x > 0.0 { (1.0 - ERX) - p } else { 1.0 + (ERX + p) };
    }
    if ix < 0x403c_0000 {
        let t = erfc_tail(x);
        return if x > 0.0 { t } else { 2.0 - t };
    }
    if x > 0.0 { 0.0 } else { 2.0 }
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
}
