//! The handful of float helpers `core` does not give a `no_std` crate on stable (floor/ceil/round/sqrt).

#[inline]
pub fn floor(x: f32) -> f32 {
    if x.is_nan() || x.abs() >= 8_388_608.0 {
        return x; // already integral (or NaN/inf)
    }
    let t = x as i32 as f32;
    if t > x { t - 1.0 } else { t }
}

#[inline]
pub fn ceil(x: f32) -> f32 {
    -floor(-x)
}

#[inline]
pub fn round(x: f32) -> f32 {
    floor(x + 0.5)
}

/// Newton–Raphson square root (to f32 precision) for non-negative inputs.
pub fn sqrt(x: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return 0.0;
    }
    if !x.is_finite() {
        return x;
    }
    // Initial guess from the exponent bits, then three Newton steps.
    let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1fbd_1df5);
    for _ in 0..4 {
        y = 0.5 * (y + x / y);
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helpers() {
        assert_eq!(floor(-1.5), -2.0);
        assert_eq!(floor(1.5), 1.0);
        assert_eq!(floor(-2.0), -2.0);
        assert_eq!(ceil(1.2), 2.0);
        assert_eq!(ceil(-1.2), -1.0);
        assert_eq!(round(2.5), 3.0);
        assert!((sqrt(2.0) - core::f32::consts::SQRT_2).abs() < 1e-6);
        assert!((sqrt(1e6) - 1000.0).abs() < 1e-3);
    }
}
