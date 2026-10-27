// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CAVLC written (9.2, residual_block_cavlc 7.3.5.3.2): coeff_token by nC (Table 9-5),
//! trailing_ones_sign_flag, level_prefix / level_suffix with the suffixLength adaptation of 9.2.2,
//! total_zeros (Tables 9-7 / 9-8 / 9-9 a)) and run_before (Table 9-10).
//!
//! The code words come from h264_core's own tables (`tables::COEFF_TOKEN_*`, `TOTAL_ZEROS*`,
//! `RUN_BEFORE`): each is a list of (length, code, value) rows the decoder matches by code, so the
//! writer finds the row BY VALUE — one copy of every table, read from both sides.
//!
//! [`Nnz`] keeps TotalCoeff per 4x4 block of the picture for nC (9.2.1), with the decoder's
//! neighbour rule for a single slice: a block left of or above the current one is available when
//! it is inside the picture (every such block is earlier in decoding order), and nC is the rounded
//! mean of the available ones (h264_core `mb.rs` `nc_luma` / `nc_chroma` / `combine_nc`).

use alloc::vec;
use alloc::vec::Vec;

use h264_core::tables::{COEFF_TOKEN_CDC, COEFF_TOKEN_NC0, COEFF_TOKEN_NC2, COEFF_TOKEN_NC4, RUN_BEFORE, TOTAL_ZEROS, TOTAL_ZEROS_CDC};

use crate::nal::BitWriter;

/// Write the row of a (length, code, value) table whose value is `v`.
fn put(w: &mut BitWriter, table: &[(u8, u16, u8)], v: u8) {
    let &(len, code, _) = table.iter().find(|r| r.2 == v).expect("CAVLC value outside its table");
    w.u(len as u32, code as u32);
}

/// level_prefix / level_suffix (9.2.2.1) for levelCode `code` at `suffix_len`.
fn put_level(w: &mut BitWriter, code: u32, suffix_len: u32) {
    let code = code as i64;
    if suffix_len == 0 && code < 14 {
        w.u(code as u32 + 1, 1);
        return;
    }
    if suffix_len == 0 && code < 30 {
        w.u(15, 1); // level_prefix 14
        w.u(4, (code - 14) as u32);
        return;
    }
    if suffix_len > 0 && code < (15 << suffix_len) {
        w.u((code >> suffix_len) as u32 + 1, 1);
        w.u(suffix_len, (code & ((1 << suffix_len) - 1)) as u32);
        return;
    }
    // level_prefix >= 15: levelCode = (15 << suffixLength) + level_suffix (+ 15 when
    // suffixLength is 0) (+ (1 << (level_prefix - 3)) - 4096 from level_prefix 16).
    let base = (15i64 << suffix_len) + if suffix_len == 0 { 15 } else { 0 };
    let mut prefix = 15u32;
    loop {
        let extra = if prefix >= 16 { (1i64 << (prefix - 3)) - 4096 } else { 0 };
        let s = code - base - extra;
        if s >= 0 && s < (1i64 << (prefix - 3)) {
            for _ in 0..prefix {
                w.flag(false);
            }
            w.flag(true);
            w.u(prefix - 3, s as u32);
            return;
        }
        prefix += 1;
    }
}

/// residual_block_cavlc of one block: `list` holds the levels in scan order (`max` of them: 16 for
/// a 4x4 / Intra16x16DC block, 15 for an AC block, 4 for chroma DC with nC = -1). Returns
/// TotalCoeff(coeff_token).
pub fn write_block(w: &mut BitWriter, nc: i32, max: usize, list: &[i32]) -> u8 {
    let list = &list[..max];
    let mut nz = [0usize; 16]; // positions of the non-zero levels, lowest frequency first
    let mut total = 0usize;
    for (i, &v) in list.iter().enumerate() {
        if v != 0 {
            nz[total] = i;
            total += 1;
        }
    }
    // TrailingOnes: up to three +-1 levels at the high-frequency end.
    let mut t1s = 0usize;
    while t1s < total.min(3) && list[nz[total - 1 - t1s]].abs() == 1 {
        t1s += 1;
    }
    let token = ((total << 2) | t1s) as u8;
    match nc {
        -1 => put(w, &COEFF_TOKEN_CDC, token),
        0..=1 => put(w, &COEFF_TOKEN_NC0, token),
        2..=3 => put(w, &COEFF_TOKEN_NC2, token),
        4..=7 => put(w, &COEFF_TOKEN_NC4, token),
        _ => w.u(6, if total == 0 { 3 } else { (((total - 1) << 2) | t1s) as u32 }),
    }
    if total == 0 {
        return 0;
    }
    // Levels from the highest frequency down: trailing-one signs, then the rest.
    let mut suffix_len: u32 = if total > 10 && t1s < 3 { 1 } else { 0 };
    for i in 0..total {
        let v = list[nz[total - 1 - i]];
        if i < t1s {
            w.flag(v < 0);
            continue;
        }
        let mut code = if v > 0 { 2 * v as i64 - 2 } else { -2 * v as i64 - 1 };
        if i == t1s && t1s < 3 {
            code -= 2;
        }
        put_level(w, code as u32, suffix_len);
        if suffix_len == 0 {
            suffix_len = 1;
        }
        if v.unsigned_abs() > (3u32 << (suffix_len - 1)) && suffix_len < 6 {
            suffix_len += 1;
        }
    }
    // total_zeros: the zeros below the highest-frequency level.
    let last = nz[total - 1];
    let mut zeros_left = last + 1 - total;
    if total < max {
        let t = if max == 4 { TOTAL_ZEROS_CDC[total - 1] } else { TOTAL_ZEROS[total - 1] };
        put(w, t, zeros_left as u8);
    }
    // run_before of each level but the lowest-frequency one, while zeros remain.
    for i in 0..total - 1 {
        if zeros_left == 0 {
            break;
        }
        let k = total - 1 - i;
        let run = nz[k] - nz[k - 1] - 1;
        put(w, RUN_BEFORE[zeros_left.min(7) - 1], run as u8);
        zeros_left -= run;
    }
    total as u8
}

/// TotalCoeff per 4x4 block of the picture: luma on a (4 * mb_w) x (4 * mb_h) grid, each chroma
/// component on a (2 * mb_w) x (2 * mb_h) grid. Skipped macroblocks and blocks not coded keep 0.
pub struct Nnz {
    w4: usize,
    luma: Vec<u8>,
    w2: usize,
    chroma: [Vec<u8>; 2],
}

impl Nnz {
    pub fn new(mb_w: usize, mb_h: usize) -> Nnz {
        Nnz { w4: mb_w * 4, luma: vec![0; mb_w * mb_h * 16], w2: mb_w * 2, chroma: [vec![0; mb_w * mb_h * 4], vec![0; mb_w * mb_h * 4]] }
    }

    /// Every block back to 0 (a new picture).
    pub fn reset(&mut self) {
        self.luma.fill(0);
        self.chroma[0].fill(0);
        self.chroma[1].fill(0);
    }

    fn combine(a: Option<u8>, b: Option<u8>) -> i32 {
        match (a, b) {
            (Some(a), Some(b)) => (a as i32 + b as i32 + 1) >> 1,
            (Some(a), None) => a as i32,
            (None, Some(b)) => b as i32,
            (None, None) => 0,
        }
    }

    /// nC of the luma 4x4 block at picture 4x4 position (x4, y4).
    pub fn nc_luma(&self, x4: usize, y4: usize) -> i32 {
        let a = (x4 > 0).then(|| self.luma[y4 * self.w4 + x4 - 1]);
        let b = (y4 > 0).then(|| self.luma[(y4 - 1) * self.w4 + x4]);
        Self::combine(a, b)
    }

    /// nC of the chroma AC block of component `c` at picture chroma-4x4 position (x2, y2).
    pub fn nc_chroma(&self, c: usize, x2: usize, y2: usize) -> i32 {
        let a = (x2 > 0).then(|| self.chroma[c][y2 * self.w2 + x2 - 1]);
        let b = (y2 > 0).then(|| self.chroma[c][(y2 - 1) * self.w2 + x2]);
        Self::combine(a, b)
    }

    pub fn set_luma(&mut self, x4: usize, y4: usize, tc: u8) {
        self.luma[y4 * self.w4 + x4] = tc;
    }

    pub fn set_chroma(&mut self, c: usize, x2: usize, y2: usize, tc: u8) {
        self.chroma[c][y2 * self.w2 + x2] = tc;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use h264_core::bits::BitReader;
    use h264_core::cavlc::residual_block;

    /// Every block written reads back through h264_core's own CAVLC reader, at every nC class and
    /// block size, including large levels (the escape prefixes) and long runs.
    #[test]
    fn write_block_round_trips_through_the_decoder() {
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut blocks: Vec<(i32, usize, [i32; 16])> = Vec::new();
        for n in 0..3000 {
            let max = [16usize, 15, 4][n % 3];
            let nc = if max == 4 { -1 } else { [0, 1, 2, 3, 4, 7, 8, 16][(rnd() % 8) as usize] };
            let mut b = [0i32; 16];
            let density = rnd() % 17;
            for v in b[..max].iter_mut() {
                if rnd() % 16 < density {
                    let mag = match rnd() % 10 {
                        0..=4 => 1,
                        5..=7 => 1 + (rnd() % 4) as i32,
                        8 => 1 + (rnd() % 64) as i32,
                        _ => 1 + (rnd() % 3000) as i32,
                    };
                    *v = if rnd() & 1 == 1 { -mag } else { mag };
                }
            }
            blocks.push((nc, max, b));
        }
        let mut w = BitWriter::new();
        let mut totals = Vec::new();
        for (nc, max, b) in &blocks {
            totals.push(write_block(&mut w, *nc, *max, b));
        }
        w.trailing();
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        let mut out = [0i32; 16];
        for (k, (nc, max, b)) in blocks.iter().enumerate() {
            let t = residual_block(&mut r, *nc, *max, &mut out).unwrap();
            assert_eq!(t as u8, totals[k], "block {k}");
            assert_eq!(&out[..*max], &b[..*max], "block {k} nc {nc} max {max}");
        }
        assert!(!r.more_rbsp_data());
    }
}
