// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CAVLC (9.2): residual_block_cavlc (7.3.5.3.2) — coeff_token by nC (9.2.1, Table 9-5), the
//! level_prefix / level_suffix decoding of 9.2.2.1, total_zeros (9.2.3, Tables 9-7 / 9-8 / 9-9 a))
//! and run_before (Table 9-10), and the coefficient placement of 9.2.4.

use crate::bits::BitReader;
use crate::tables::{COEFF_TOKEN_CDC, COEFF_TOKEN_NC0, COEFF_TOKEN_NC2, COEFF_TOKEN_NC4, RUN_BEFORE, TOTAL_ZEROS, TOTAL_ZEROS_CDC};
use crate::{Error, Result};

/// Decode one code word of a (length, code, value) table sorted by length.
fn vlc(r: &mut BitReader, table: &[(u8, u16, u8)]) -> Result<u8> {
    let w = r.peek(16);
    for &(len, code, val) in table {
        if (w >> (16 - len as u32)) as u16 == code {
            r.skip(len as usize);
            return Ok(val);
        }
    }
    Err(Error::Invalid("CAVLC code word not in table"))
}

/// coeff_token (9.2.1): (TotalCoeff, TrailingOnes) for the block's nC (-1 = chroma DC 4:2:0).
pub fn coeff_token(r: &mut BitReader, nc: i32) -> Result<(usize, usize)> {
    let v = match nc {
        -1 => vlc(r, &COEFF_TOKEN_CDC)?,
        0..=1 => vlc(r, &COEFF_TOKEN_NC0)?,
        2..=3 => vlc(r, &COEFF_TOKEN_NC2)?,
        4..=7 => vlc(r, &COEFF_TOKEN_NC4)?,
        _ => {
            // 8 <= nC: 6-bit fixed length, TotalCoeff - 1 in the high four bits, TrailingOnes in
            // the low two; 000011 codes (0, 0) (Table 9-5).
            let w = r.u(6);
            if w == 3 {
                0
            } else {
                let tc = (w >> 2) + 1;
                let t1 = w & 3;
                if t1 > tc.min(3) {
                    return Err(Error::Invalid("coeff_token (nC >= 8) TrailingOnes > TotalCoeff"));
                }
                ((tc << 2) | t1) as u8
            }
        }
    };
    Ok(((v >> 2) as usize, (v & 3) as usize))
}

/// residual_block_cavlc (7.3.5.3.2) with startIdx 0 and endIdx `max - 1`: the levels of one
/// block in list order into `coeff[0..max]`. Returns TotalCoeff(coeff_token).
pub fn residual_block(r: &mut BitReader, nc: i32, max: usize, coeff: &mut [i32]) -> Result<u32> {
    for c in coeff[..max].iter_mut() {
        *c = 0;
    }
    let (total, t1s) = coeff_token(r, nc)?;
    if total == 0 {
        return Ok(0);
    }
    if total > max {
        return Err(Error::Invalid("TotalCoeff exceeds the block's coefficient count"));
    }
    let mut level = [0i32; 16];
    // 9.2.2: suffixLength starts at 1 when TotalCoeff > 10 and TrailingOnes < 3.
    let mut suffix_len: u32 = if total > 10 && t1s < 3 { 1 } else { 0 };
    for i in 0..total {
        if i < t1s {
            level[i] = 1 - 2 * r.bit() as i32;
            continue;
        }
        // level_prefix (9.2.2.1): leading zero bits before a one.
        let mut prefix = 0u32;
        while r.bit() == 0 {
            prefix += 1;
            if prefix > 32 || r.overran() {
                return Err(Error::Invalid("level_prefix too long"));
            }
        }
        let mut code: i64 = (prefix.min(15) as i64) << suffix_len;
        let suffix_size = if prefix == 14 && suffix_len == 0 {
            4
        } else if prefix >= 15 {
            prefix - 3
        } else {
            suffix_len
        };
        if suffix_len > 0 || prefix >= 14 {
            if suffix_size > 0 {
                code += r.u(suffix_size) as i64;
            }
        }
        if prefix >= 15 && suffix_len == 0 {
            code += 15;
        }
        if prefix >= 16 {
            code += (1i64 << (prefix - 3)) - 4096;
        }
        if i == t1s && t1s < 3 {
            code += 2;
        }
        let v = if code % 2 == 0 { (code + 2) >> 1 } else { (-code - 1) >> 1 };
        level[i] = v as i32;
        if suffix_len == 0 {
            suffix_len = 1;
        }
        if v.unsigned_abs() > (3u64 << (suffix_len - 1)) && suffix_len < 6 {
            suffix_len += 1;
        }
    }
    // total_zeros (9.2.3) when fewer than all coefficients are coded.
    let mut zeros_left = if total < max {
        let t = if max == 4 { TOTAL_ZEROS_CDC[total - 1] } else { TOTAL_ZEROS[total - 1] };
        vlc(r, t)? as usize
    } else {
        0
    };
    if total + zeros_left > max {
        return Err(Error::Invalid("total_zeros exceeds the block"));
    }
    let mut run = [0usize; 16];
    for i in 0..total - 1 {
        if zeros_left > 0 {
            let rb = vlc(r, RUN_BEFORE[zeros_left.min(7) - 1])? as usize;
            if rb > zeros_left {
                return Err(Error::Invalid("run_before exceeds zerosLeft"));
            }
            run[i] = rb;
            zeros_left -= rb;
        }
    }
    run[total - 1] = zeros_left;
    // 9.2.4: levels are listed from the highest frequency down.
    let mut pos: isize = -1;
    for i in (0..total).rev() {
        pos += run[i] as isize + 1;
        coeff[pos as usize] = level[i];
    }
    Ok(total as u32)
}
