//! FreeType's fixed-point arithmetic, bit-exact (FreeType 2.13 `ftcalc.c`, 64-bit build): the hinters must round
//! exactly as the reference does or points land one 1/64 px off and the per-glyph oracle stops matching.

/// `FT_MulFix`: (a·b + 0x8000 + sign) >> 16 on 32-bit operands (the x86-64 inline form, identical to the C one).
#[inline]
pub fn mul_fix(a: i64, b: i64) -> i64 {
    let ab = (a as i32 as i64) * (b as i32 as i64);
    let ab = ab + 0x8000 + (ab >> 63);
    (ab >> 16) as i32 as i64
}

/// `FT_MulDiv`: sign-magnitude a·b/c rounded half away from zero; c == 0 → 0x7FFFFFFF.
#[inline]
pub fn mul_div(a: i64, b: i64, c: i64) -> i64 {
    let s = (a < 0) ^ (b < 0) ^ (c < 0);
    let (a, b, c) = (a.unsigned_abs(), b.unsigned_abs(), c.unsigned_abs());
    let d = if c > 0 { (a.wrapping_mul(b).wrapping_add(c >> 1)) / c } else { 0x7FFF_FFFF } as i64;
    if s { -d } else { d }
}

/// `FT_MulDiv_No_Round`.
#[inline]
pub fn mul_div_no_round(a: i64, b: i64, c: i64) -> i64 {
    let s = (a < 0) ^ (b < 0) ^ (c < 0);
    let (a, b, c) = (a.unsigned_abs(), b.unsigned_abs(), c.unsigned_abs());
    let d = if c > 0 { a.wrapping_mul(b) / c } else { 0x7FFF_FFFF } as i64;
    if s { -d } else { d }
}

/// `FT_DivFix`: (a << 16) / b rounded, sign-magnitude; b == 0 → 0x7FFFFFFF.
#[inline]
pub fn div_fix(a: i64, b: i64) -> i64 {
    let s = (a < 0) ^ (b < 0);
    let (a, b) = (a.unsigned_abs(), b.unsigned_abs());
    let q = if b > 0 { ((a << 16) + (b >> 1)) / b } else { 0x7FFF_FFFF } as i64;
    if s { -q } else { q }
}

#[inline]
pub fn pix_round(x: i64) -> i64 {
    (x + 32) & !63
}
#[inline]
pub fn pix_floor(x: i64) -> i64 {
    x & !63
}
#[inline]
pub fn pix_ceil(x: i64) -> i64 {
    (x + 63) & !63
}

/// `FT_HYPOT`: the octagonal length estimate max + 3/8 min.
#[inline]
pub fn hypot_approx(x: i64, y: i64) -> i64 {
    let (x, y) = (x.abs(), y.abs());
    if x > y { x + ((3 * y) >> 3) } else { y + ((3 * x) >> 3) }
}

/// `ft_corner_is_flat`.
pub fn corner_is_flat(in_x: i64, in_y: i64, out_x: i64, out_y: i64) -> bool {
    let (ax, ay) = (in_x + out_x, in_y + out_y);
    let d_in = hypot_approx(in_x, in_y);
    let d_out = hypot_approx(out_x, out_y);
    let d_hypot = hypot_approx(ax, ay);
    (d_in + d_out - d_hypot) < (d_hypot >> 4)
}

/// `FT_MSB`: index of the highest set bit (x > 0).
#[inline]
pub fn msb(x: u32) -> i32 {
    31 - x.leading_zeros() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_kats() {
        // FT_MulFix rounds half away from zero.
        assert_eq!(mul_fix(3, 0x8000), 2);
        assert_eq!(mul_fix(-3, 0x8000), -2);
        assert_eq!(mul_fix(1, 0x8000), 1);
        assert_eq!(mul_fix(-1, 0x8000), -1);
        assert_eq!(mul_fix(2048, 0x10000), 2048);
        // 16 px / 2048 upem: FT_DivFix(1024, 2048) = 0x8000.
        assert_eq!(div_fix(16 << 6, 2048), 0x8000);
        assert_eq!(div_fix(12 << 6, 2048), 24576);
        assert_eq!(div_fix(-1, 3), -21845);
        assert_eq!(mul_div(7, 3, 2), 11);
        assert_eq!(mul_div(-7, 3, 2), -11);
        assert_eq!(mul_div(1, 1, 0), 0x7FFF_FFFF);
        assert_eq!(mul_div_no_round(7, 3, 2), 10);
        assert_eq!(pix_round(31), 0);
        assert_eq!(pix_round(32), 64);
        assert_eq!(pix_round(-33), -64);
        assert_eq!(pix_floor(-1), -64);
        assert_eq!(pix_ceil(1), 64);
        assert_eq!(hypot_approx(3, 4), 4 + 1);
        assert!(corner_is_flat(100, 0, 100, 1));
        assert!(!corner_is_flat(100, 0, 0, 100));
        assert_eq!(msb(1), 0);
        assert_eq!(msb(0x8000_0000), 31);
    }
}
