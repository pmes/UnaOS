// SPDX-License-Identifier: LGPL-3.0-or-later
//! Levels: input range, gamma, output range, as one 256-entry table; and the fixed-point power it needs.

use alloc::vec::Vec;

use crate::{Error, check, clamp_u8, div_round, map_rgb};

/// Q16: 1.0 = 65536.
pub const Q16: i64 = 1 << 16;

/// `round(2^(2^-k) * 2^30)` for k = 1..=16 (index k - 1).
const EXP2_FRAC: [u64; 16] = [
    1518500250, 1276901417, 1170923762, 1121280436, 1097253708, 1085434106, 1079572136, 1076653033, 1075196443,
    1074468888, 1074105294, 1073923544, 1073832680, 1073787251, 1073764537, 1073753181,
];

/// Levels parameters. `gamma_q8`: 1.0 = 256; it is the exponent (`< 256` lifts the midtones).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Levels {
    pub in_lo: u8,
    pub in_hi: u8,
    pub gamma_q8: u16,
    pub out_lo: u8,
    pub out_hi: u8,
}

impl Levels {
    /// `(0, 255, 1.0, 0, 255)`.
    pub const IDENTITY: Levels = Levels { in_lo: 0, in_hi: 255, gamma_q8: 256, out_lo: 0, out_hi: 255 };
}

/// `log2(x / 65536)` in Q16 for `x` in `1..=65536` (result <= 0).
pub(crate) fn log2_q16(x: u32) -> i64 {
    debug_assert!(x >= 1 && x <= 1 << 16);
    let n = 31 - x.leading_zeros() as i64; // floor(log2 x), 0..=16
    // Mantissa in [1, 2) as Q30.
    let mut m: u64 = ((x as u64) << 30) >> n;
    let mut frac: i64 = 0;
    for bit in (0..16).rev() {
        m = (m * m) >> 30;
        if m >= 2 << 30 {
            m >>= 1;
            frac |= 1 << bit;
        }
    }
    ((n - 16) << 16) + frac
}

/// `2^(y / 65536)` in Q16 for `y <= 0`.
pub(crate) fn exp2_q16(y: i64) -> u32 {
    debug_assert!(y <= 0);
    let ip = y >> 16; // floor, <= 0
    let fp = (y & 0xFFFF) as u32;
    let mut r: u64 = 1 << 30;
    for k in 1..=16u32 {
        if fp & (1 << (16 - k)) != 0 {
            r = (r * EXP2_FRAC[(k - 1) as usize]) >> 30;
        }
    }
    let shift = 14 - ip; // Q30 -> Q16, times 2^ip
    if shift >= 64 { 0 } else { ((r + (1 << (shift - 1))) >> shift) as u32 }
}

/// `t ^ (g_q8 / 256)` for `t` in Q16 `0..=65536`.
pub(crate) fn pow_q16(t: u32, g_q8: u16) -> u32 {
    if t == 0 {
        return 0;
    }
    if g_q8 == 256 || t == 1 << 16 {
        return t;
    }
    let l = log2_q16(t);
    exp2_q16(div_round(l * g_q8 as i64, 256)).min(1 << 16)
}

/// The levels table (built once per call by [`levels`]).
pub fn levels_lut(p: &Levels) -> Result<[u8; 256], Error> {
    if p.in_lo >= p.in_hi || p.gamma_q8 == 0 {
        return Err(Error::BadParam);
    }
    let d = (p.in_hi - p.in_lo) as i64;
    let span = p.out_hi as i64 - p.out_lo as i64;
    let mut lut = [0u8; 256];
    for (v, slot) in lut.iter_mut().enumerate() {
        let t = div_round((v as i64 - p.in_lo as i64) * Q16, d).clamp(0, Q16) as u32;
        let pw = pow_q16(t, p.gamma_q8) as i64;
        *slot = clamp_u8(p.out_lo as i64 + div_round(span * pw, Q16));
    }
    Ok(lut)
}

/// Levels on R, G, B (alpha copied).
pub fn levels(px: &[u8], w: u32, h: u32, p: Levels) -> Result<Vec<u8>, Error> {
    check(px, w, h)?;
    let lut = levels_lut(&p)?;
    Ok(map_rgb(px, [&lut, &lut, &lut]))
}
