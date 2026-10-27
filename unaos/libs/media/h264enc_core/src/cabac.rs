// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENCCABAC (LEDGER SR310): CABAC written (9.3.4): the arithmetic encoder (InitEncoder 9.3.4.1,
//! EncodeDecision 9.3.4.2 with RenormE 9.3.4.3 and PutBit 9.3.4.4, EncodeBypass 9.3.4.4,
//! EncodeTerminate / EncodeFlush 9.3.4.5) and the binarizations (9.3.2) of every syntax element the
//! encoder emits, with the ctxIdxInc derivations (9.3.3.1) of h264_core's decoder (`mb.rs`).
//!
//! CHARTER: Stria (A/V) — seam `shared-core`; the CABAC writer of h264enc_core, the mirror of h264_core's CABAC reader.
//!
//! Shared, never copied: the contexts are initialised by h264_core's own `Cabac::init_contexts`
//! (9.3.1.1 over its `CABAC_INIT_MN`), the state transitions are its `RANGE_TAB_LPS` /
//! `TRANS_IDX_LPS` / `TRANS_IDX_MPS`, coded_block_flag's ctxIdx is its `Cabac::cbf_ctx` and the
//! ctxBlockCatOffsets its `SIG_OFFSET` / `ABS_OFFSET`. The neighbour facts are the encoder's one
//! `MbInfo` grid (`Encoder::dinfo`, recorded by `note_mb` in [`crate::deblock`]), the grid shape the
//! decoder reads for its contexts and its filter alike. The decoder is the oracle: every stream
//! decodes to the encoder's reconstruction byte for byte (tests/intra.rs, pframes.rs, rc.rs).
//!
//! Not written (the encoder emits neither; owed with the partitions / multiple references):
//! sub_mb_type and ref_idx_l0. Not written either: I_PCM. H264ENC8X8 (SR322) adds transform_size_8x8_flag
//! (ctxIdx 399..401) and ctxBlockCat 5 (the 8x8 luma block, no coded_block_flag) over h264_core's
//! `SIG_COEFF_8X8_FRAME` / `LAST_COEFF_8X8`.
//!
//! H264ENCBFRAMES (SR333): B slices — mb_skip_flag at ctxIdx 24..26, the B mb_type bins of Table 9-37
//! (B_Direct_16x16 / B_L0_16x16 / B_L1_16x16 / B_Bi_16x16 at ctxIdx 27..35, h264_core `cabac_mb_type`),
//! and mvd_l1 beside mvd_l0 with the |mvd| neighbour facts kept per list.

use alloc::vec;
use alloc::vec::Vec;

use h264_core::cabac::{Cabac, ABS_OFFSET, CAT_CHROMA_AC, CAT_CHROMA_DC, CAT_LUMA_4X4, CAT_LUMA_8X8, CAT_LUMA_AC, CAT_LUMA_DC, NUM_CTX, SIG_OFFSET};
use h264_core::picture::{blk_at, MbInfo, MbKind, BLK_X, BLK_Y, CBF_CHROMA_AC, CBF_CHROMA_DC, CBF_LUMA_DC};
use h264_core::tables::{LAST_COEFF_8X8, RANGE_TAB_LPS, SIG_COEFF_8X8_FRAME, TRANS_IDX_LPS, TRANS_IDX_MPS};

use crate::nal::BitWriter;
use crate::{intra, mode_at, mode_fields, scan, scan8, Encoder, Kind, MbCode, Pic};

/// ctxIdxOffset (Table 9-34, frame coding) of significant_coeff_flag, last_significant_coeff_flag
/// and coeff_abs_level_minus1 for ctxBlockCat < 5 (the literals of h264_core's `residual_block`).
const SIG_BASE: usize = 105;
const LAST_BASE: usize = 166;
const ABS_BASE: usize = 227;
/// The same three ctxIdxOffsets for ctxBlockCat 5, frame coding (the literals 402 / 417 / 426 of
/// h264_core's `residual_block`; SR322).
const SIG_BASE_8X8: usize = 402;
const LAST_BASE_8X8: usize = 417;
const ABS_BASE_8X8: usize = 426;

/// mvd ctxIdxInc of prefix bins 1..8 (Table 9-39).
const MVD_INC: [usize; 8] = [3, 4, 5, 6, 6, 6, 6, 6];

/// The CABAC writer of one slice: the arithmetic encoder's registers, the context states, and the
/// slice's CABAC-only neighbour facts (|mvd| per macroblock, the previous mb_qp_delta != 0).
/// Every bit goes into the caller's [`BitWriter`], passed to each call.
pub struct Writer {
    /// codILow (10 bits).
    low: u32,
    /// codIRange (9 bits).
    range: u32,
    /// bitsOutstanding.
    outstanding: u32,
    /// firstBitFlag.
    first: bool,
    /// pStateIdx << 1 | valMPS per ctxIdx (h264_core's layout).
    state: [u8; NUM_CTX],
    /// Absolute mvd (x, y) per macroblock and list (SR333: [l0, l1]), capped at 255 as the decoder's
    /// `put_mvd` stores it.
    mvd: Vec<[[u8; 2]; 2]>,
    /// mb_qp_delta of the previous macroblock in decoding order was non-zero (9.3.3.1.1.5).
    prev_qp_nz: bool,
}

impl Writer {
    /// Start the slice data: cabac_alignment_one_bit up to the byte boundary (7.3.4), the contexts
    /// initialised for `column` (0 for I slices, 1 + cabac_init_idc otherwise) at SliceQPY by
    /// h264_core's own `init_contexts`, and the encoder initialised (9.3.4.1). `mbs` is
    /// PicSizeInMbs.
    pub fn new(w: &mut BitWriter, column: usize, slice_qp: i32, mbs: usize) -> Writer {
        while w.bits() % 8 != 0 {
            w.flag(true); // cabac_alignment_one_bit
        }
        let mut init = Cabac::new(&[], 0);
        init.init_contexts(column, slice_qp);
        Writer { low: 0, range: 510, outstanding: 0, first: true, state: *init.contexts(), mvd: vec![[[0; 2]; 2]; mbs], prev_qp_nz: false }
    }

    /// Bits the encoder holds and has not yet put (codILow's 10 and the outstanding ones): added to
    /// the writer's count, an estimate of the slice's bits so far (exact after the flush).
    pub fn pending(&self) -> usize {
        self.outstanding as usize + 10
    }

    /// PutBit (9.3.4.4, Figure 9-9).
    fn put_bit(&mut self, w: &mut BitWriter, b: u32) {
        if self.first {
            self.first = false;
        } else {
            w.u(1, b);
        }
        while self.outstanding > 0 {
            w.u(1, 1 - b);
            self.outstanding -= 1;
        }
    }

    /// RenormE (9.3.4.3, Figure 9-8).
    fn renorm(&mut self, w: &mut BitWriter) {
        while self.range < 256 {
            if self.low < 256 {
                self.put_bit(w, 0);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put_bit(w, 1);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }

    /// EncodeDecision (9.3.4.2, Figure 9-7): `bin` with context `ctx`.
    pub fn decision(&mut self, w: &mut BitWriter, ctx: usize, bin: u32) {
        let s = self.state[ctx];
        let p = (s >> 1) as usize;
        let mps = (s & 1) as u32;
        let lps = RANGE_TAB_LPS[p][((self.range >> 6) & 3) as usize] as u32;
        self.range -= lps;
        if bin != mps {
            self.low += self.range;
            self.range = lps;
            let nmps = if p == 0 { 1 - mps } else { mps };
            self.state[ctx] = (TRANS_IDX_LPS[p] << 1) | nmps as u8;
        } else {
            self.state[ctx] = (TRANS_IDX_MPS[p] << 1) | mps as u8;
        }
        self.renorm(w);
    }

    /// EncodeBypass (9.3.4.4, Figure 9-10).
    pub fn bypass(&mut self, w: &mut BitWriter, bin: u32) {
        self.low <<= 1;
        if bin != 0 {
            self.low += self.range;
        }
        if self.low >= 1024 {
            self.put_bit(w, 1);
            self.low -= 1024;
        } else if self.low < 512 {
            self.put_bit(w, 0);
        } else {
            self.low -= 512;
            self.outstanding += 1;
        }
    }

    /// EncodeTerminate (9.3.4.5, Figure 9-11), ctxIdx 276: end_of_slice_flag (or the I_PCM
    /// announcement). A 1 runs EncodeFlush (Figure 9-12), whose last written bit is the
    /// rbsp_stop_one_bit; the caller then writes only the rbsp_alignment_zero_bits.
    pub fn terminate(&mut self, w: &mut BitWriter, bin: bool) {
        self.range -= 2;
        if bin {
            self.low += self.range;
            self.range = 2;
            self.renorm(w);
            self.put_bit(w, (self.low >> 9) & 1);
            w.u(2, ((self.low >> 7) & 3) | 1);
        } else {
            self.renorm(w);
        }
    }

    /// The k-th order Exp-Golomb suffix of `v` in bypass bins (9.3.2.3, UEGk suffix); the reader is
    /// h264_core's `Cabac::exp_golomb_bypass`.
    pub fn exp_golomb_bypass(&mut self, w: &mut BitWriter, mut v: u32, mut k: u32) {
        while k < 31 && v >= (1 << k) {
            self.bypass(w, 1);
            v -= 1 << k;
            k += 1;
        }
        self.bypass(w, 0);
        while k > 0 {
            k -= 1;
            self.bypass(w, (v >> k) & 1);
        }
    }

    /// residual_block_cabac (7.3.5.3.3) after a coded_block_flag of 1, for ctxBlockCat `cat` < 5,
    /// or for ctxBlockCat 5 (an 8x8 luma block, 64 levels, no coded_block_flag; SR322):
    /// `list[..max]` in scan order, at least one level non-zero. The significance map
    /// (9.3.3.1.3: levelListIdx, chroma DC Min(levelListIdx, 2), cat 5 h264_core's
    /// `SIG_COEFF_8X8_FRAME` / `LAST_COEFF_8X8`), then the levels in reverse scan
    /// order: coeff_abs_level_minus1 (prefix TU cMax 14, bin 0 ctxIdxInc numDecodAbsLevelGt1 != 0 ?
    /// 0 : Min(4, 1 + numDecodAbsLevelEq1), later bins 5 + Min(4 - (cat == 3), numDecodAbsLevelGt1);
    /// UEG0 bypass suffix) and coeff_sign_flag (bypass). The reader is h264_core's
    /// `Cabac::residual_block`.
    pub fn residual_block(&mut self, w: &mut BitWriter, cat: usize, max: usize, list: &[i32]) {
        let (sig_base, last_base, abs_base) = if cat == CAT_LUMA_8X8 { (SIG_BASE_8X8, LAST_BASE_8X8, ABS_BASE_8X8) } else { (SIG_BASE + SIG_OFFSET[cat], LAST_BASE + SIG_OFFSET[cat], ABS_BASE + ABS_OFFSET[cat]) };
        let last = list[..max].iter().rposition(|&v| v != 0).expect("coded_block_flag 1 over a block of zeros");
        for i in 0..max - 1 {
            let (si, li) = match cat {
                CAT_LUMA_8X8 => (SIG_COEFF_8X8_FRAME[i] as usize, LAST_COEFF_8X8[i] as usize),
                CAT_CHROMA_DC => (i.min(2), i.min(2)),
                _ => (i, i),
            };
            let sig = list[i] != 0;
            self.decision(w, sig_base + si, sig as u32);
            if sig {
                self.decision(w, last_base + li, (i == last) as u32);
                if i == last {
                    break;
                }
            }
        }
        let gt1_cap = if cat == CAT_CHROMA_DC { 3 } else { 4 };
        let (mut eq1, mut gt1) = (0u32, 0u32);
        for i in (0..=last).rev() {
            let v = list[i];
            if v == 0 {
                continue;
            }
            let abs1 = v.unsigned_abs() - 1;
            let inc0 = (if gt1 != 0 { 0 } else { (1 + eq1).min(4) }) as usize;
            if abs1 == 0 {
                self.decision(w, abs_base + inc0, 0);
                eq1 += 1;
            } else {
                self.decision(w, abs_base + inc0, 1);
                let ctx = abs_base + 5 + gt1.min(gt1_cap) as usize;
                for _ in 1..abs1.min(14) {
                    self.decision(w, ctx, 1);
                }
                if abs1 < 14 {
                    self.decision(w, ctx, 0);
                } else {
                    self.exp_golomb_bypass(w, abs1 - 14, 0);
                }
                gt1 += 1;
            }
            self.bypass(w, (v < 0) as u32);
        }
    }

    /// mvd_l0[][][comp] (UEG3, signedValFlag 1, uCoff 9; ctxIdx 40..46 / 47..53, 9.3.3.1.1.7) with
    /// `sum` = absMvdComp of the left + the upper neighbour partition.
    pub fn mvd(&mut self, w: &mut BitWriter, comp: usize, sum: u32, v: i32) {
        let base = if comp == 0 { 40 } else { 47 };
        let inc0 = if sum < 3 {
            0
        } else if sum <= 32 {
            1
        } else {
            2
        };
        let a = v.unsigned_abs();
        if a == 0 {
            self.decision(w, base + inc0, 0);
            return;
        }
        self.decision(w, base + inc0, 1);
        for b in 1..a.min(9) {
            self.decision(w, base + MVD_INC[b as usize - 1], 1);
        }
        if a < 9 {
            self.decision(w, base + MVD_INC[a as usize - 1], 0);
        } else {
            self.exp_golomb_bypass(w, a - 9, 3);
        }
        self.bypass(w, (v < 0) as u32);
    }

    /// mb_qp_delta (Table 9-3 mapping, U binarization; ctxIdx 60 + (previous delta != 0), 62, 63;
    /// 9.3.3.1.1.5).
    pub fn qp_delta(&mut self, w: &mut BitWriter, d: i32) {
        let k = (if d > 0 { 2 * d - 1 } else { -2 * d }) as u32;
        self.decision(w, 60 + self.prev_qp_nz as usize, (k != 0) as u32);
        if k != 0 {
            for j in 1..k {
                self.decision(w, if j == 1 { 62 } else { 63 }, 1);
            }
            self.decision(w, if k == 1 { 62 } else { 63 }, 0);
        }
        self.prev_qp_nz = d != 0;
    }
}

impl Encoder {
    /// The neighbouring location (6.4.12) left of / above the block (x, y) (units of a 1/`n`
    /// macroblock; one of x, y is -1) of macroblock (mbx, mby): its macroblock address and the
    /// position inside it. One slice per picture: every macroblock left or above is available.
    fn cabac_nb(&self, mbx: usize, mby: usize, x: isize, y: isize, n: isize) -> Option<(usize, usize, usize)> {
        let nx = mbx as isize + if x < 0 { -1 } else { 0 };
        let ny = mby as isize + if y < 0 { -1 } else { 0 };
        if nx < 0 || ny < 0 {
            return None;
        }
        Some((ny as usize * self.mb_w + nx as usize, x.rem_euclid(n) as usize, y.rem_euclid(n) as usize))
    }

    /// mbAddrA / mbAddrB (6.4.9) when available.
    fn cabac_ab(&self, mbx: usize, mby: usize) -> (Option<usize>, Option<usize>) {
        (self.cabac_nb(mbx, mby, -1, 0, 4).map(|n| n.0), self.cabac_nb(mbx, mby, 0, -1, 4).map(|n| n.0))
    }

    /// condTermFlagN of coded_block_flag (9.3.3.1.1.9), h264_core `mb.rs` `cbf_cond`.
    fn cabac_cbf_cond(&self, m: Option<usize>, cur_intra: bool, trans: impl Fn(&MbInfo) -> Option<bool>) -> usize {
        match m {
            None => cur_intra as usize,
            Some(m) => {
                let i = &self.dinfo[m];
                if i.kind == MbKind::IPcm {
                    1
                } else if i.kind.is_skip() {
                    0
                } else {
                    trans(i).unwrap_or(false) as usize
                }
            }
        }
    }

    /// coded_block_flag ctxIdxInc of luma 4x4 block `blk` (h264_core `cbf_luma_inc`).
    fn cabac_cbf_luma_inc(&self, mbx: usize, mby: usize, blk: usize, cur_intra: bool) -> u32 {
        let (x, y) = (BLK_X[blk] as isize, BLK_Y[blk] as isize);
        let f = |n: Option<(usize, usize, usize)>| {
            let (m, nx, ny) = match n {
                Some(v) => (Some(v.0), v.1, v.2),
                None => (None, 0, 0),
            };
            self.cabac_cbf_cond(m, cur_intra, |i| {
                let b8 = (ny / 2) * 2 + nx / 2;
                if (i.cbp >> b8) & 1 == 0 {
                    None
                } else if i.t8x8 {
                    Some(true)
                } else {
                    Some((i.cbf >> blk_at(nx, ny)) & 1 == 1)
                }
            })
        };
        (f(self.cabac_nb(mbx, mby, x - 1, y, 4)) + 2 * f(self.cabac_nb(mbx, mby, x, y - 1, 4))) as u32
    }

    /// coded_block_flag ctxIdxInc of chroma AC block `blk` of component `c` (h264_core
    /// `cbf_chroma_ac_inc`).
    fn cabac_cbf_chroma_ac_inc(&self, mbx: usize, mby: usize, c: usize, blk: usize, cur_intra: bool) -> u32 {
        let (x, y) = ((blk % 2) as isize, (blk / 2) as isize);
        let f = |n: Option<(usize, usize, usize)>| {
            let (m, nx, ny) = match n {
                Some(v) => (Some(v.0), v.1, v.2),
                None => (None, 0, 0),
            };
            self.cabac_cbf_cond(m, cur_intra, |i| if i.cbp >> 4 == 2 { Some((i.cbf >> (CBF_CHROMA_AC as usize + c * 4 + ny * 2 + nx)) & 1 == 1) } else { None })
        };
        (f(self.cabac_nb(mbx, mby, x - 1, y, 2)) + 2 * f(self.cabac_nb(mbx, mby, x, y - 1, 2))) as u32
    }

    /// mb_type of an I macroblock (Table 9-36) at ctxIdxOffset `off` (3 in I slices; 17 as the
    /// suffix after the prefix 1 in P slices), h264_core `cabac_mb_type_i`.
    fn cabac_mb_type_i(&self, w: &mut BitWriter, c: &mut Writer, mbx: usize, mby: usize, code: &MbCode, off: usize, suffix: bool) {
        let inc0 = if suffix {
            0
        } else {
            let (a, b) = self.cabac_ab(mbx, mby);
            let cond = |m: Option<usize>| m.is_some_and(|m| !matches!(self.dinfo[m].kind, MbKind::I4x4 | MbKind::I8x8)) as usize;
            cond(a) + cond(b)
        };
        if code.kind == Kind::I4 {
            c.decision(w, off + inc0, 0); // I_NxN
            return;
        }
        c.decision(w, off + inc0, 1);
        c.terminate(w, false); // not I_PCM
        let (luma, chroma0, chroma1, pred1_b3, pred0_b3, pred1, pred0) = if suffix { (1, 2, 2, 3, 3, 3, 3) } else { (3, 4, 5, 6, 7, 6, 7) };
        let cc = (code.cbp >> 4) as u32;
        let m = code.i16_mode as u32;
        c.decision(w, off + luma, (code.cbp & 15 != 0) as u32);
        if cc != 0 {
            c.decision(w, off + chroma0, 1);
            c.decision(w, off + chroma1, (cc == 2) as u32);
            c.decision(w, off + pred1_b3, m >> 1);
            c.decision(w, off + pred0_b3, m & 1);
        } else {
            c.decision(w, off + chroma0, 0);
            c.decision(w, off + pred1, m >> 1);
            c.decision(w, off + pred0, m & 1);
        }
    }

    /// One macroblock of a CABAC slice (7.3.4 / 7.3.5): mb_skip_flag in a P / B slice, then
    /// macroblock_layer in h264_core `parse_mb`'s element order, then end_of_slice_flag (a 1 after
    /// the picture's last macroblock flushes the encoder). The CABAC twin of `write_mb`; the
    /// macroblock's facts are already in `dinfo` (`note_mb` runs first).
    pub(crate) fn write_mb_cabac(&mut self, w: &mut BitWriter, c: &mut Writer, mbx: usize, mby: usize, code: &MbCode, slice: Pic) {
        let addr = mby * self.mb_w + mbx;
        let last = addr + 1 == self.mb_w * self.mb_h;
        let (ma, mb) = self.cabac_ab(mbx, mby);
        let p_slice = slice != Pic::Idr;
        c.mvd[addr] = [[0; 2]; 2];
        if p_slice {
            let cond = |m: Option<usize>| m.is_some_and(|m| !self.dinfo[m].kind.is_skip()) as usize;
            let skip = matches!(code.kind, Kind::PSkip | Kind::BSkip);
            let base = if slice == Pic::B { 24 } else { 11 };
            c.decision(w, base + cond(ma) + cond(mb), skip as u32); // mb_skip_flag
            if skip {
                c.prev_qp_nz = false;
                c.terminate(w, last);
                return;
            }
        }
        match code.kind {
            Kind::I4 | Kind::I16 => {
                if p_slice {
                    c.decision(w, 14, 1); // the intra prefix of a P slice's mb_type
                    self.cabac_mb_type_i(w, c, mbx, mby, code, 17, true);
                } else {
                    self.cabac_mb_type_i(w, c, mbx, mby, code, 3, false);
                }
                if code.kind == Kind::I4 {
                    if self.cfg.transform8x8 {
                        self.cabac_t8x8(w, c, mbx, mby, code.t8x8);
                    }
                    let w4 = self.mb_w * 4;
                    for blk in 0..mode_fields(code.t8x8) {
                        let (bx, by) = mode_at(code.t8x8, blk);
                        let (x4, y4) = (mbx * 4 + bx, mby * 4 + by);
                        let pred = intra::predicted_mode(&self.modes, w4, x4, y4);
                        let m = code.modes[blk];
                        if m == pred {
                            c.decision(w, 68, 1); // prev_intra4x4_pred_mode_flag
                        } else {
                            c.decision(w, 68, 0);
                            let r = (if m < pred { m } else { m - 1 }) as u32; // rem_intra4x4_pred_mode, LSB first
                            c.decision(w, 69, r & 1);
                            c.decision(w, 69, (r >> 1) & 1);
                            c.decision(w, 69, (r >> 2) & 1);
                        }
                    }
                }
                // intra_chroma_pred_mode (TU cMax 3; ctxIdx 64..67, 9.3.3.1.1.8)
                let cond = |m: Option<usize>| {
                    m.is_some_and(|m| {
                        let i = &self.dinfo[m];
                        i.kind.is_intra() && i.kind != MbKind::IPcm && i.chroma_mode != 0
                    }) as usize
                };
                let cm = code.chroma_mode;
                c.decision(w, 64 + cond(ma) + cond(mb), (cm != 0) as u32);
                if cm != 0 {
                    c.decision(w, 67, (cm >= 2) as u32);
                    if cm >= 2 {
                        c.decision(w, 67, (cm == 3) as u32);
                    }
                }
            }
            Kind::P16 => {
                // mb_type P_L0_16x16 (Table 9-37: bins 0 0 0 at ctxIdx 14, 15, 16); ref_idx_l0 absent
                // (one active reference).
                c.decision(w, 14, 0);
                c.decision(w, 15, 0);
                c.decision(w, 16, 0);
                self.cabac_mvds(w, c, addr, ma, mb, code);
            }
            Kind::BDirect | Kind::BL0 | Kind::BL1 | Kind::BBi => {
                // B mb_type (Table 9-37, h264_core `cabac_mb_type`): bin 0 at 27 + (A / B available and not
                // B_Skip / B_Direct_16x16): B_Direct_16x16 `0`; B_L0_16x16 `1 0 0`, B_L1_16x16 `1 0 1` (bins at 30,
                // 32); B_Bi_16x16 `1 1 0 0 0 0` (bins at 30, 31, 32, 32, 32). ref_idx absent (one per list).
                let cond = |m: Option<usize>| m.is_some_and(|m| !matches!(self.dinfo[m].kind, MbKind::BSkip | MbKind::BDirect)) as usize;
                c.decision(w, 27 + cond(ma) + cond(mb), (code.kind != Kind::BDirect) as u32);
                match code.kind {
                    Kind::BL0 | Kind::BL1 => {
                        c.decision(w, 30, 0);
                        c.decision(w, 32, (code.kind == Kind::BL1) as u32);
                    }
                    Kind::BBi => {
                        c.decision(w, 30, 1);
                        c.decision(w, 31, 0);
                        for _ in 0..3 {
                            c.decision(w, 32, 0);
                        }
                    }
                    _ => {}
                }
                self.cabac_mvds(w, c, addr, ma, mb, code);
            }
            Kind::PSkip | Kind::BSkip => unreachable!("a skipped macroblock in an I slice"),
        }
        if code.kind != Kind::I16 {
            self.cabac_cbp(w, c, mbx, mby, code);
        }
        if self.cfg.transform8x8 && code.kind.inter_coded() && code.cbp & 15 != 0 {
            self.cabac_t8x8(w, c, mbx, mby, code.t8x8); // B_Direct_16x16 too: direct_8x8_inference_flag 1
        }
        if code.cbp != 0 || code.kind == Kind::I16 {
            c.qp_delta(w, self.qp - self.qp_pred); // QPY,pred moves as h264_core's mb.rs moves it
            self.qp_pred = self.qp;
            self.write_residual_cabac(w, c, mbx, mby, code);
        } else {
            c.prev_qp_nz = false;
        }
        c.terminate(w, last); // end_of_slice_flag
    }

    /// mvd_l0 (x, y) when the macroblock carries it, then mvd_l1 (h264_core `parse_mb_pred_inter`'s order),
    /// each with absMvdComp of the left + the upper macroblock in the same list; the |mvd| kept per list.
    fn cabac_mvds(&self, w: &mut BitWriter, c: &mut Writer, addr: usize, ma: Option<usize>, mb: Option<usize>, code: &MbCode) {
        for (list, used, mvd) in [(0usize, code.kind.l0(), code.mvd), (1, code.kind.l1(), code.mvd1)] {
            if !used {
                continue;
            }
            let abs = |m: Option<usize>, comp: usize| m.map_or(0u32, |m| c.mvd[m][list][comp] as u32);
            let sums = [abs(ma, 0) + abs(mb, 0), abs(ma, 1) + abs(mb, 1)];
            for comp in 0..2 {
                c.mvd(w, comp, sums[comp], mvd[comp]);
            }
            c.mvd[addr][list] = [mvd[0].unsigned_abs().min(255) as u8, mvd[1].unsigned_abs().min(255) as u8];
        }
    }

    /// transform_size_8x8_flag (ctxIdx 399 + condTermFlagA + condTermFlagB, 9.3.3.1.1.10: the neighbour
    /// macroblock is available and carries the flag), h264_core `cabac_t8x8` (SR322).
    fn cabac_t8x8(&self, w: &mut BitWriter, c: &mut Writer, mbx: usize, mby: usize, flag: bool) {
        let (a, b) = self.cabac_ab(mbx, mby);
        let cond = |m: Option<usize>| m.is_some_and(|m| self.dinfo[m].t8x8) as usize;
        c.decision(w, 399 + cond(a) + cond(b), flag as u32);
    }

    /// coded_block_pattern (9.3.2.6: prefix FL 4 bins, suffix TU cMax 2; ctxIdx 73..84,
    /// 9.3.3.1.1.4), h264_core `cabac_cbp`. The current macroblock's own 8x8 bits come from its
    /// `dinfo` entry, the decoder's partial value for every earlier 8x8.
    fn cabac_cbp(&self, w: &mut BitWriter, c: &mut Writer, mbx: usize, mby: usize, code: &MbCode) {
        for b8 in 0..4usize {
            let (bx, by) = (((b8 % 2) * 2) as isize, ((b8 / 2) * 2) as isize);
            let cond = |n: Option<(usize, usize, usize)>| -> usize {
                let Some((m, nx, ny)) = n else { return 0 };
                let b8n = (ny / 2) * 2 + nx / 2;
                let i = &self.dinfo[m];
                if i.kind == MbKind::IPcm {
                    return 0;
                }
                if !i.kind.is_skip() && (i.cbp >> b8n) & 1 != 0 {
                    return 0;
                }
                1
            };
            let a = cond(self.cabac_nb(mbx, mby, bx - 1, by, 4));
            let b = cond(self.cabac_nb(mbx, mby, bx, by - 1, 4));
            c.decision(w, 73 + a + 2 * b, ((code.cbp >> b8) & 1) as u32);
        }
        let cond = |m: Option<usize>, bin: u32| -> usize {
            let Some(m) = m else { return 0 };
            let i = &self.dinfo[m];
            if i.kind == MbKind::IPcm {
                return 1;
            }
            let cc = i.cbp >> 4;
            (!i.kind.is_skip() && if bin == 0 { cc != 0 } else { cc == 2 }) as usize
        };
        let (ma, mb) = self.cabac_ab(mbx, mby);
        let cc = code.cbp >> 4;
        c.decision(w, 77 + cond(ma, 0) + 2 * cond(mb, 0), (cc != 0) as u32);
        if cc != 0 {
            c.decision(w, 77 + 4 + cond(ma, 1) + 2 * cond(mb, 1), (cc == 2) as u32);
        }
    }

    /// residual( 0, 15 ) (7.3.5.3) under CABAC: per block coded_block_flag (ctxIdxInc of
    /// 9.3.3.1.1.9) then residual_block_cabac, in h264_core `parse_residual`'s order.
    fn write_residual_cabac(&self, w: &mut BitWriter, c: &mut Writer, mbx: usize, mby: usize, code: &MbCode) {
        let intra = matches!(code.kind, Kind::I4 | Kind::I16);
        let i16 = code.kind == Kind::I16;
        let (ma, mb) = self.cabac_ab(mbx, mby);
        if i16 {
            let f = |m: Option<usize>| self.cabac_cbf_cond(m, intra, |i| if i.kind == MbKind::I16x16 { Some((i.cbf >> CBF_LUMA_DC) & 1 == 1) } else { None });
            let inc = (f(ma) + 2 * f(mb)) as u32;
            let list = scan(&code.dc, 0);
            let coded = list.iter().any(|&v| v != 0);
            c.decision(w, Cabac::cbf_ctx(CAT_LUMA_DC, inc), coded as u32);
            if coded {
                c.residual_block(w, CAT_LUMA_DC, 16, &list);
            }
        }
        for blk in 0..16 {
            if (code.cbp >> (blk / 4)) & 1 == 0 {
                continue;
            }
            if code.t8x8 {
                // ctxBlockCat 5: one 64-level block per coded 8x8, no coded_block_flag (SR322).
                if blk % 4 == 0 {
                    c.residual_block(w, CAT_LUMA_8X8, 64, &scan8(&code.luma8[blk / 4]));
                }
                continue;
            }
            let inc = self.cabac_cbf_luma_inc(mbx, mby, blk, intra);
            let (cat, max, list) = if i16 { (CAT_LUMA_AC, 15, scan(&code.luma[blk], 1)) } else { (CAT_LUMA_4X4, 16, scan(&code.luma[blk], 0)) };
            let coded = list[..max].iter().any(|&v| v != 0);
            c.decision(w, Cabac::cbf_ctx(cat, inc), coded as u32);
            if coded {
                c.residual_block(w, cat, max, &list);
            }
        }
        let cc = code.cbp >> 4;
        if cc != 0 {
            for comp in 0..2 {
                let f = |m: Option<usize>| self.cabac_cbf_cond(m, intra, |i| if i.cbp >> 4 != 0 { Some((i.cbf >> (CBF_CHROMA_DC as usize + comp)) & 1 == 1) } else { None });
                let inc = (f(ma) + 2 * f(mb)) as u32;
                let list = code.cdc[comp];
                let coded = list.iter().any(|&v| v != 0);
                c.decision(w, Cabac::cbf_ctx(CAT_CHROMA_DC, inc), coded as u32);
                if coded {
                    c.residual_block(w, CAT_CHROMA_DC, 4, &list);
                }
            }
        }
        if cc == 2 {
            for comp in 0..2 {
                for blk in 0..4 {
                    let inc = self.cabac_cbf_chroma_ac_inc(mbx, mby, comp, blk, intra);
                    let list = scan(&code.cac[comp][blk], 1);
                    let coded = list[..15].iter().any(|&v| v != 0);
                    c.decision(w, Cabac::cbf_ctx(CAT_CHROMA_AC, inc), coded as u32);
                    if coded {
                        c.residual_block(w, CAT_CHROMA_AC, 15, &list);
                    }
                }
            }
        }
    }
}
