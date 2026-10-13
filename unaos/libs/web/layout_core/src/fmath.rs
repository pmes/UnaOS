//! The float rounding `core` does not give a `no_std` crate (floor/ceil/round/trunc), bit-exact with std's:
//! the host's layout numbers must not move when its engine moves into this crate. Methods carry std's names, so
//! the ported code reads unchanged; where std is in the crate graph its inherent methods take precedence and
//! compute the same values.

/// `copysign` on bits.
#[inline]
fn copysign(mag: f32, sign: f32) -> f32 {
    f32::from_bits((mag.to_bits() & 0x7fff_ffff) | (sign.to_bits() & 0x8000_0000))
}

/// Toward zero; |x| >= 2^23 (and NaN/inf) is already integral.
#[inline]
pub fn trunc(x: f32) -> f32 {
    if x.is_nan() || x.abs() >= 8_388_608.0 {
        return x;
    }
    copysign(x as i32 as f32, x)
}

#[inline]
pub fn floor(x: f32) -> f32 {
    let t = trunc(x);
    if t > x { t - 1.0 } else { t }
}

#[inline]
pub fn ceil(x: f32) -> f32 {
    let t = trunc(x);
    if t < x { t + 1.0 } else { t }
}

/// Half away from zero (std's `round`). `x - trunc(x)` is exact below 2^23.
#[inline]
pub fn round(x: f32) -> f32 {
    let t = trunc(x);
    if x.is_nan() || x.abs() >= 8_388_608.0 {
        return x;
    }
    let d = x - t;
    if d >= 0.5 {
        t + 1.0
    } else if d <= -0.5 {
        t - 1.0
    } else {
        t
    }
}

/// std's method names over the helpers above.
pub trait FloatExt {
    fn floor(self) -> f32;
    fn ceil(self) -> f32;
    fn round(self) -> f32;
    fn trunc(self) -> f32;
}

impl FloatExt for f32 {
    #[inline]
    fn floor(self) -> f32 {
        floor(self)
    }
    #[inline]
    fn ceil(self) -> f32 {
        ceil(self)
    }
    #[inline]
    fn round(self) -> f32 {
        round(self)
    }
    #[inline]
    fn trunc(self) -> f32 {
        trunc(self)
    }
}
