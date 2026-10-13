// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CABAC (9.3): context initialisation (9.3.1.1), the arithmetic decoding engine (9.3.1.2
//! initialisation; 9.3.3.2.1 DecodeDecision with RenormD 9.3.3.2.2; 9.3.3.2.3 DecodeBypass;
//! 9.3.3.2.2.3 DecodeTerminate), and residual_block_cabac (7.3.5.3.3) with the significance map
//! and level contexts of 9.3.3.1.1.9 / 9.3.3.1.3.
//!
//! The engine is the Recommendation's own model, bit for bit: codIRange is 9 bits, codIOffset is
//! 9 bits read one bit at a time, so the reader's position after a DecodeTerminate that returns 1
//! is exactly the end of the arithmetic code word — which is where pcm_alignment_zero_bit and the
//! I_PCM samples begin (9.3.1.2: the engine is initialised again after the samples).

use crate::tables::{CABAC_INIT_MN, LAST_COEFF_8X8, RANGE_TAB_LPS, SIG_COEFF_8X8_FRAME, TRANS_IDX_LPS, TRANS_IDX_MPS};

/// Contexts this decoder addresses (ctxIdx 0..=459, Table 9-34, frame coding, ChromaArrayType 1).
pub const NUM_CTX: usize = 460;

/// ctxBlockCat (Table 9-42).
pub const CAT_LUMA_DC: usize = 0; // Intra16x16DCLevel
pub const CAT_LUMA_AC: usize = 1; // Intra16x16ACLevel
pub const CAT_LUMA_4X4: usize = 2; // LumaLevel4x4
pub const CAT_CHROMA_DC: usize = 3; // ChromaDCLevel
pub const CAT_CHROMA_AC: usize = 4; // ChromaACLevel
pub const CAT_LUMA_8X8: usize = 5; // LumaLevel8x8

/// ctxBlockCatOffset (Table 9-40) for coded_block_flag, significant / last, coeff_abs_level_minus1.
const CBF_OFFSET: [usize; 5] = [0, 4, 8, 12, 16];
const SIG_OFFSET: [usize; 5] = [0, 15, 29, 44, 47];
const ABS_OFFSET: [usize; 5] = [0, 10, 20, 30, 39];

pub struct Cabac<'a> {
    data: &'a [u8],
    /// Bit position of the next bit to read.
    pos: usize,
    range: u32,
    offset: u32,
    /// pStateIdx << 1 | valMPS per ctxIdx.
    state: [u8; NUM_CTX],
}

impl<'a> Cabac<'a> {
    /// A decoder over `data` whose arithmetic code starts at bit `pos` (byte aligned: the
    /// cabac_alignment_one_bit run has been consumed).
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        let mut c = Cabac { data, pos, range: 510, offset: 0, state: [0; NUM_CTX] };
        c.init_engine();
        c
    }

    /// 9.3.1.1: preCtxState = Clip3(1, 126, ((m * Clip3(0, 51, SliceQPY)) >> 4) + n).
    /// `column` is 0 for I slices, 1 + cabac_init_idc otherwise.
    pub fn init_contexts(&mut self, column: usize, slice_qp: i32) {
        let qp = slice_qp.clamp(0, 51);
        for (i, &(m, n)) in CABAC_INIT_MN[column].iter().enumerate() {
            let pre = (((m as i32 * qp) >> 4) + n as i32).clamp(1, 126);
            self.state[i] = if pre <= 63 { ((63 - pre) << 1) as u8 } else { (((pre - 64) << 1) | 1) as u8 };
        }
    }

    /// 9.3.1.2: codIRange = 510, codIOffset = read_bits(9).
    pub fn init_engine(&mut self) {
        self.range = 510;
        self.offset = 0;
        for _ in 0..9 {
            self.offset = (self.offset << 1) | self.read_bit();
        }
    }

    /// Bit position of the next unread bit.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Re-seat the reader (after I_PCM samples) and initialise the engine there.
    pub fn restart_at(&mut self, pos: usize) {
        self.pos = pos;
        self.init_engine();
    }

    /// True once the engine has read past the end of the slice data.
    pub fn overran(&self) -> bool {
        self.pos > self.data.len() * 8 + 16
    }

    #[inline]
    fn read_bit(&mut self) -> u32 {
        let b = self.data.get(self.pos >> 3).map_or(0, |v| (v >> (7 - (self.pos & 7))) & 1);
        self.pos += 1;
        b as u32
    }

    /// DecodeDecision (9.3.3.2.1) with RenormD (9.3.3.2.2).
    #[inline]
    pub fn decision(&mut self, ctx: usize) -> u32 {
        let s = self.state[ctx];
        let p = (s >> 1) as usize;
        let mps = (s & 1) as u32;
        let q = ((self.range >> 6) & 3) as usize;
        let lps = RANGE_TAB_LPS[p][q] as u32;
        self.range -= lps;
        let bin;
        if self.offset >= self.range {
            bin = 1 - mps;
            self.offset -= self.range;
            self.range = lps;
            let nmps = if p == 0 { 1 - mps } else { mps };
            self.state[ctx] = (TRANS_IDX_LPS[p] << 1) | nmps as u8;
        } else {
            bin = mps;
            self.state[ctx] = (TRANS_IDX_MPS[p] << 1) | mps as u8;
        }
        while self.range < 256 {
            self.range <<= 1;
            self.offset = (self.offset << 1) | self.read_bit();
        }
        bin
    }

    /// DecodeBypass (9.3.3.2.3).
    #[inline]
    pub fn bypass(&mut self) -> u32 {
        self.offset = (self.offset << 1) | self.read_bit();
        if self.offset >= self.range {
            self.offset -= self.range;
            1
        } else {
            0
        }
    }

    /// DecodeTerminate (9.3.3.2.2.3): ctxIdx 276. A 1 ends the slice data or announces I_PCM and
    /// performs no renormalisation.
    pub fn terminate(&mut self) -> u32 {
        self.range -= 2;
        if self.offset >= self.range {
            1
        } else {
            while self.range < 256 {
                self.range <<= 1;
                self.offset = (self.offset << 1) | self.read_bit();
            }
            0
        }
    }

    /// Unary prefix with contexts `ctx(binIdx)`, at most `cmax` ones (TU binarization, 9.3.2.2).
    pub fn tu(&mut self, cmax: u32, mut ctx: impl FnMut(u32) -> usize) -> u32 {
        let mut v = 0;
        while v < cmax && self.decision(ctx(v)) == 1 {
            v += 1;
        }
        v
    }

    /// The k-th order Exp-Golomb suffix in bypass bins (9.3.2.3, UEGk suffix).
    pub fn exp_golomb_bypass(&mut self, mut k: u32) -> u32 {
        let mut v: u32 = 0;
        while self.bypass() == 1 {
            v = v.wrapping_add(1 << k);
            k += 1;
            if k >= 31 {
                return v;
            }
        }
        while k > 0 {
            k -= 1;
            v = v.wrapping_add(self.bypass() << k);
        }
        v
    }

    /// residual_block_cabac (7.3.5.3.3) for one block of ctxBlockCat `cat`, after coded_block_flag
    /// was decoded as 1 (or inferred, for ctxBlockCat 5 with ChromaArrayType != 3). Writes the
    /// levels into `coeff[0..max]` (list order) and returns the number of non-zero levels.
    ///
    /// Significance contexts: levelListIdx for cat 0, 1, 2, 4; Min(levelListIdx / NumC8x8, 2) for
    /// chroma DC (NumC8x8 = 1); Table 9-43 for cat 5 (9.3.3.1.3). Level contexts (9.3.3.1.3):
    /// bin 0 ctxIdxInc = numDecodAbsLevelGt1 != 0 ? 0 : Min(4, 1 + numDecodAbsLevelEq1); later
    /// bins 5 + Min(4 - (cat == 3), numDecodAbsLevelGt1); UEG0 with uCoff 14, then the sign.
    pub fn residual_block(&mut self, cat: usize, max: usize, coeff: &mut [i32]) -> u32 {
        for c in coeff[..max].iter_mut() {
            *c = 0;
        }
        let (sig_base, last_base, abs_base) = if cat == CAT_LUMA_8X8 {
            (402, 417, 426)
        } else {
            (105 + SIG_OFFSET[cat], 166 + SIG_OFFSET[cat], 227 + ABS_OFFSET[cat])
        };
        let mut sig = [false; 64];
        let mut num = max;
        let mut i = 0;
        while i + 1 < num {
            let (si, li) = match cat {
                CAT_LUMA_8X8 => (SIG_COEFF_8X8_FRAME[i] as usize, LAST_COEFF_8X8[i] as usize),
                CAT_CHROMA_DC => (i.min(2), i.min(2)),
                _ => (i, i),
            };
            if self.decision(sig_base + si) == 1 {
                sig[i] = true;
                if self.decision(last_base + li) == 1 {
                    num = i + 1;
                    break;
                }
            }
            i += 1;
        }
        sig[num - 1] = true;
        let gt1_cap = if cat == CAT_CHROMA_DC { 3 } else { 4 };
        let (mut eq1, mut gt1) = (0u32, 0u32);
        let mut nz = 0;
        for i in (0..num).rev() {
            if !sig[i] {
                continue;
            }
            let inc0 = if gt1 != 0 { 0 } else { (1 + eq1).min(4) } as usize;
            let mut level = 1u32;
            if self.decision(abs_base + inc0) == 1 {
                let inc = 5 + gt1.min(gt1_cap) as usize;
                let mut prefix = 1;
                while prefix < 14 && self.decision(abs_base + inc) == 1 {
                    prefix += 1;
                }
                level = if prefix == 14 { 15 + self.exp_golomb_bypass(0) } else { prefix + 1 };
            }
            if level == 1 {
                eq1 += 1;
            } else {
                gt1 += 1;
            }
            let v = level.min(1 << 30) as i32;
            coeff[i] = if self.bypass() == 1 { -v } else { v };
            nz += 1;
        }
        nz
    }

    /// ctxIdx of coded_block_flag for ctxBlockCat `cat` (< 5) with ctxIdxInc `inc` (9.3.3.1.1.9).
    pub fn cbf_ctx(cat: usize, inc: u32) -> usize {
        85 + CBF_OFFSET[cat] + inc as usize
    }
}
