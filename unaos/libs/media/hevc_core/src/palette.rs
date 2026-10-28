// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria (A/V) — seam `shared-core` (hevc_core's palette mode).
//!
//! HEVCREXTSCC (LEDGER SR357): the palette mode of the screen content coding extension, written from ITU-T Rec. H.265
//! v4 (12/2016) without the reference software (self-consistent, unconformed until HEVCCONF-SCC): the palette predictor
//! (its initialisation from the PPS / SPS initializers and the update after each palette coding unit, 7.4.9.13; its
//! wavefront / dependent-slice storage is [`crate::ctu`]'s, beside StatCoeff), the traverse scan (6.5.x), the palette
//! binarizations that are not already the engine's (truncated binary, palette_run_prefix with its ctxInc,
//! palette_run_suffix, palette_escape_val) and the decoding of palette samples (8.4.4.2.7). The syntax (7.3.8.13) is
//! parsed in [`crate::ctu`].

use alloc::vec::Vec;

use crate::cabac::Engine;
use crate::picture::Picture;
use crate::ps::{Pps, Sps};
use crate::tables::{LEVEL_SCALE, PALETTE_RUN_PREFIX};

/// PredictorPaletteSize / PredictorPaletteEntries: three rows of one length (a monochrome stream keeps its chroma rows
/// at 0).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PalettePredictor {
    pub entries: [Vec<u16>; 3],
}

impl PalettePredictor {
    /// PredictorPaletteSize.
    pub fn size(&self) -> usize {
        self.entries[0].len()
    }

    /// The initialisation at the start of a slice, a tile or an unsynchronised wavefront row: the PPS initializers
    /// when pps_palette_predictor_initializers_present_flag (possibly none), else the SPS ones, else empty.
    pub fn init(sps: &Sps, pps: &Pps) -> PalettePredictor {
        let mut p = PalettePredictor::default();
        if !sps.scc.palette_mode_enabled_flag {
            return p;
        }
        if let Some(rows) = pps.scc.palette_predictor_initializers.as_ref().or(sps.scc.palette_predictor_initializers.as_ref()) {
            let n = rows[0].len();
            for (dst, src) in p.entries.iter_mut().zip(rows.iter()) {
                *dst = (0..n).map(|i| src.get(i).copied().unwrap_or(0)).collect();
            }
        }
        p
    }

    /// 7.4.9.13: after a palette coding unit the predictor is CurrentPaletteEntries followed by the predictor entries
    /// it did not reuse, up to PaletteMaxPredictorSize `max`.
    pub fn update(&mut self, cur: &[Vec<u16>; 3], reuse: &[bool], max: usize) {
        let mut next = cur.clone();
        for i in 0..self.size() {
            if next[0].len() >= max {
                break;
            }
            if !reuse.get(i).copied().unwrap_or(false) {
                for (row, old) in next.iter_mut().zip(self.entries.iter()) {
                    row.push(old[i]);
                }
            }
        }
        for row in next.iter_mut() {
            row.truncate(max);
        }
        self.entries = next;
    }
}

/// The traverse scan (6.5.x) of a (1 << log2)-square block: rows alternately left to right and right to left;
/// palette_transpose_flag swaps the axes (columns alternately down and up). Position `pos` -> (x, y).
pub fn traverse(log2: u32, pos: usize, transpose: bool) -> (usize, usize) {
    let n = 1usize << log2;
    let (y, i) = (pos >> log2, pos & (n - 1));
    let x = if y & 1 == 1 { n - 1 - i } else { i };
    if transpose { (y, x) } else { (x, y) }
}

/// Truncated binary binarization (TB, 9.3.3.x) in bypass bins with cMax `cmax`: n = cMax + 1 symbols, k = Floor(Log2(n)),
/// u = 2^(k + 1) - n; a value below u is k bins, the rest k + 1 bins of value + u.
pub fn bypass_tb(e: &mut Engine, cmax: u32) -> u32 {
    let n = cmax + 1;
    if n <= 1 {
        return 0;
    }
    let k = 31 - n.leading_zeros();
    let u = (1u32 << (k + 1)) - n;
    let v = e.bypass_bits(k);
    if v < u { v } else { ((v << 1) | e.bypass()) - u }
}

/// ctxInc of palette_run_prefix bin `bin` (< 5): a copy-above run 5, 6, 6, 7, 7; an index run (index < 1 ? 0 : index < 3
/// ? 1 : 2), 3, 3, 4, 4 (SPEC-CHECK).
pub fn run_ctx(bin: u32, copy_above: bool, index: u32) -> usize {
    const ABOVE: [usize; 5] = [5, 6, 6, 7, 7];
    const INDEX: [usize; 5] = [0, 3, 3, 4, 4];
    if copy_above {
        ABOVE[bin as usize]
    } else if bin == 0 {
        if index < 1 { 0 } else if index < 3 { 1 } else { 2 }
    } else {
        INDEX[bin as usize]
    }
}

/// palette_run_prefix: TR with cMax Floor(Log2(PaletteMaxRunMinus1)) + 1 (`max_run_minus1` > 0), bins 0..4 in context
/// ([`run_ctx`]) and the rest bypass.
pub fn run_prefix(e: &mut Engine, max_run_minus1: u32, copy_above: bool, index: u32) -> u32 {
    let cmax = 32 - max_run_minus1.leading_zeros();
    let mut v = 0;
    while v < cmax {
        let b = if v < 5 { e.decision(PALETTE_RUN_PREFIX + run_ctx(v, copy_above, index)) } else { e.bypass() };
        if b == 0 {
            break;
        }
        v += 1;
    }
    v
}

/// PaletteRunMinus1 from palette_run_prefix `prefix` and, when present (prefix > 1 and PaletteMaxRunMinus1 !=
/// 1 << (prefix - 1)), palette_run_suffix: TB with cMax (1 << (prefix - 1)) - 1 below the largest prefix, else
/// PaletteMaxRunMinus1 - (1 << (prefix - 1)).
pub fn run_value(e: &mut Engine, max_run_minus1: u32, prefix: u32) -> u32 {
    if prefix < 2 {
        return prefix;
    }
    let base = 1u32 << (prefix - 1);
    if max_run_minus1 == base {
        return base;
    }
    let cmax = if prefix == 32 - max_run_minus1.leading_zeros() { max_run_minus1.saturating_sub(base) } else { base - 1 };
    base + bypass_tb(e, cmax)
}

/// palette_escape_val (SPEC-CHECK): FL of BitDepth bins in a cu_transquant_bypass coding unit, else EG3; bypass.
pub fn escape_val(e: &mut Engine, bypass_cu: bool, bit_depth: u32) -> u32 {
    if bypass_cu { e.bypass_bits(bit_depth) } else { e.bypass_egk(3).min(1 << 20) }
}

/// 8.4.4.2.7: the samples of the palette coding unit at (x0, y0), nCbS = 1 << log2, of components 0..`ncomp`. `map` is
/// PaletteIndexMap ([y * nCbS + x]); a chroma sample takes the index at its luma position (x * SubWidthC, y *
/// SubHeightC). With `escape` = Some(MaxPaletteIndex) that index is an escape: PaletteEscapeVal (`esc`, at luma
/// positions) as it is in a cu_transquant_bypass coding unit, else scaled by qP (`qp`: Qp'Y, Qp'Cb, Qp'Cr):
/// Clip3(0, (1 << BitDepth) - 1, ((v * levelScale[qP % 6]) << (qP / 6) + 32) >> 6).
pub fn reconstruct(
    pic: &mut Picture,
    x0: usize,
    y0: usize,
    log2: u32,
    cur: &[Vec<u16>; 3],
    map: &[u8],
    esc: &[Vec<u32>; 3],
    escape: Option<u32>,
    bypass: bool,
    qp: [i32; 3],
    bit_depth: [u32; 3],
    ncomp: usize,
) {
    let n = 1usize << log2;
    let (cw, ch) = pic.chroma.sub();
    for c in 0..ncomp {
        let (sw, sh) = if c == 0 { (1, 1) } else { (cw, ch) };
        let max = (1i64 << bit_depth[c]) - 1;
        let q = qp[c].max(0);
        for y in 0..n / sh {
            for x in 0..n / sw {
                let li = y * sh * n + x * sw;
                let idx = map[li] as u32;
                let v = if escape == Some(idx) {
                    let e = esc[c][li] as i64;
                    let s = if bypass { e } else { ((e * LEVEL_SCALE[(q % 6) as usize] as i64) << (q / 6)).saturating_add(32) >> 6 };
                    s.clamp(0, max)
                } else {
                    cur[c].get(idx as usize).copied().unwrap_or(0) as i64
                };
                pic.planes[c].set(x0 / sw + x, y0 / sh + y, v as u16);
            }
        }
    }
}
