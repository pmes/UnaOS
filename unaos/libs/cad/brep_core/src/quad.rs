//! Gauss-Legendre quadrature: 16 points (exact for polynomials of degree <= 31) and 4 points (degree <= 7) on [-1, 1].
//! mass.rs integrates trigonometric integrands on pieces of at most π/4, where 16 points leave an error far under
//! 1e-13 relative.

const N16: [f64; 8] = [
    0.095_012_509_837_637_440_185,
    0.281_603_550_779_258_913_230,
    0.458_016_777_657_227_386_342,
    0.617_876_244_402_643_748_447,
    0.755_404_408_355_003_033_895,
    0.865_631_202_387_831_743_880,
    0.944_575_023_073_232_576_078,
    0.989_400_934_991_649_932_596,
];
const W16: [f64; 8] = [
    0.189_450_610_455_068_496_285,
    0.182_603_415_044_923_588_867,
    0.169_156_519_395_002_538_189,
    0.149_595_988_816_576_732_081,
    0.124_628_971_255_533_872_052,
    0.095_158_511_682_492_784_810,
    0.062_253_523_938_647_892_863,
    0.027_152_459_411_754_094_852,
];
const N4: [f64; 2] = [0.339_981_043_584_856_264_803, 0.861_136_311_594_052_575_224];
const W4: [f64; 2] = [0.652_145_154_862_546_142_627, 0.347_854_845_137_453_857_373];

/// The (node, weight) pairs of the 16-point rule mapped to [a, b] (b < a gives negative weights: an oriented integral).
pub(crate) fn nodes16(a: f64, b: f64) -> impl Iterator<Item = (f64, f64)> {
    let (m, h) = ((a + b) * 0.5, (b - a) * 0.5);
    (0..16).map(move |i| {
        let (x, w) = (N16[i % 8], W16[i % 8]);
        let x = if i < 8 { -x } else { x };
        (m + h * x, w * h)
    })
}

/// The (node, weight) pairs of the 4-point rule mapped to [a, b].
pub(crate) fn nodes4(a: f64, b: f64) -> impl Iterator<Item = (f64, f64)> {
    let (m, h) = ((a + b) * 0.5, (b - a) * 0.5);
    (0..4).map(move |i| {
        let (x, w) = (N4[i % 2], W4[i % 2]);
        let x = if i < 2 { -x } else { x };
        (m + h * x, w * h)
    })
}

/// [a, b] split into ceil(|b - a| / max_piece) equal pieces (at least one), as (start, end) pairs.
pub(crate) fn pieces(a: f64, b: f64, max_piece: f64) -> impl Iterator<Item = (f64, f64)> {
    let n = libm::ceil(libm::fabs(b - a) / max_piece).max(1.0) as usize;
    let step = (b - a) / n as f64;
    (0..n).map(move |k| (a + step * k as f64, if k + 1 == n { b } else { a + step * (k + 1) as f64 }))
}

/// 16-point Gauss-Legendre of `f` over [a, b] split into pieces of at most `max_piece` (the unit tests' witness of the
/// tables; mass.rs walks the nodes itself).
#[cfg(test)]
pub(crate) fn integrate(a: f64, b: f64, max_piece: f64, mut f: impl FnMut(f64) -> f64) -> f64 {
    let mut s = 0.0;
    for (p, q) in pieces(a, b, max_piece) {
        for (x, w) in nodes16(p, q) {
            s += w * f(x);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gl16_is_exact_to_degree_31() {
        let got = integrate(-1.0, 1.0, 10.0, |x| libm::pow(x, 30.0));
        assert!(libm::fabs(got - 2.0 / 31.0) < 1e-15, "{got}");
        let w: f64 = nodes16(-1.0, 1.0).map(|(_, w)| w).sum();
        assert!(libm::fabs(w - 2.0) < 1e-15);
    }

    #[test]
    fn gl4_is_exact_to_degree_7() {
        let got: f64 = nodes4(-1.0, 1.0).map(|(x, w)| w * libm::pow(x, 6.0)).sum();
        assert!(libm::fabs(got - 2.0 / 7.0) < 1e-15, "{got}");
    }

    #[test]
    fn trig_on_pieces_is_tight() {
        // ∫_0^{2π} cos² = π, on π/4 pieces.
        let got = integrate(0.0, 2.0 * core::f64::consts::PI, core::f64::consts::FRAC_PI_4, |t| {
            let c = libm::cos(t);
            c * c
        });
        assert!(libm::fabs(got - core::f64::consts::PI) < 1e-14);
    }
}
