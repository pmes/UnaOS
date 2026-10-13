// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The macroblock layer (7.3.5): macroblock_layer, mb_pred (7.3.5.1), sub_mb_pred (7.3.5.2) and
//! residual (7.3.5.3) for both entropy coders, with the semantics of 7.4.5 (Tables 7-11, 7-13,
//! 7-14, 7-17, 7-18), the CAVLC nC of 9.2.1 and every CABAC binarization (9.3.2) and ctxIdxInc
//! (9.3.3.1.1.1 .. 9.3.3.1.1.10, 9.3.3.1.2) the slice data of a frame reaches.
//!
//! Parsing writes, as it goes, the facts later syntax elements' contexts read from neighbours
//! (the [`MbInfo`] of the macroblock, ref_idx and |mvd| per 4x4 block, Intra4x4/8x8PredMode), so
//! neighbour partitions inside the current macroblock are seen exactly as the Recommendation's
//! "already decoded" rule (6.4.11.7) has them.

use crate::bits::BitReader;
use crate::cabac::{self, Cabac};
use crate::cavlc;
use crate::picture::{blk_at, MbInfo, MbKind, Mv, PicState, Picture, BLK_X, BLK_Y, CBF_CHROMA_AC, CBF_CHROMA_DC, CBF_LUMA_DC};
use crate::ps::{Pps, SliceHeader, SliceType, Sps};
use crate::tables::{CBP_INTER, CBP_INTRA, ZIGZAG4, ZIGZAG8};
use crate::{Error, Result};

/// The entropy decoder of a slice (entropy_coding_mode_flag).
pub enum Entropy<'a> {
    Cavlc(BitReader<'a>),
    Cabac(Cabac<'a>),
}

/// Partition shapes of a macroblock type (Tables 7-13, 7-14): 16x16, 16x8, 8x16, 8x8.
pub const SHAPE_16X16: u8 = 0;
pub const SHAPE_16X8: u8 = 1;
pub const SHAPE_8X16: u8 = 2;
pub const SHAPE_8X8: u8 = 3;

/// Prediction list use of a partition: bit 0 = Pred_L0, bit 1 = Pred_L1 (3 = BiPred).
pub type PredFlags = u8;

/// B macroblock types 1..=21 (Table 7-14): (shape, pred of partition 0, pred of partition 1).
const B_PART: [(u8, u8, u8); 22] = [
    (0, 0, 0), // B_Direct_16x16 (unused here)
    (0, 1, 0),
    (0, 2, 0),
    (0, 3, 0),
    (1, 1, 1),
    (2, 1, 1),
    (1, 2, 2),
    (2, 2, 2),
    (1, 1, 2),
    (2, 1, 2),
    (1, 2, 1),
    (2, 2, 1),
    (1, 1, 3),
    (2, 1, 3),
    (1, 2, 3),
    (2, 2, 3),
    (1, 3, 1),
    (2, 3, 1),
    (1, 3, 2),
    (2, 3, 2),
    (1, 3, 3),
    (2, 3, 3),
];

/// Partition shape and per-partition prediction flags of an inter macroblock type.
pub fn mb_partitions(slice_type: SliceType, mb_type: u32) -> (u8, [PredFlags; 2]) {
    match slice_type {
        SliceType::P => match mb_type {
            0 => (SHAPE_16X16, [1, 0]),
            1 => (SHAPE_16X8, [1, 1]),
            2 => (SHAPE_8X16, [1, 1]),
            _ => (SHAPE_8X8, [1, 1]),
        },
        _ => {
            if mb_type == 22 {
                (SHAPE_8X8, [3, 3])
            } else {
                let (s, a, b) = B_PART[mb_type as usize];
                (s, [a, b])
            }
        }
    }
}

/// Number of partitions and the (x, y, w, h) of partition `i` in 4x4 units.
pub fn part_rect(shape: u8, i: usize) -> (usize, usize, usize, usize) {
    match shape {
        SHAPE_16X16 => (0, 0, 4, 4),
        SHAPE_16X8 => (0, 2 * i, 4, 2),
        SHAPE_8X16 => (2 * i, 0, 2, 4),
        _ => (2 * (i % 2), 2 * (i / 2), 2, 2),
    }
}

pub fn num_parts(shape: u8) -> usize {
    match shape {
        SHAPE_16X16 => 1,
        SHAPE_8X8 => 4,
        _ => 2,
    }
}

/// A sub-macroblock type (Tables 7-17, 7-18): (NumSubMbPart, width, height in 4x4 units, pred
/// flags); B_Direct_8x8 has pred flags 0.
pub fn sub_partitions(slice_type: SliceType, t: u8) -> (usize, usize, usize, PredFlags) {
    if slice_type == SliceType::P {
        match t {
            0 => (1, 2, 2, 1),
            1 => (2, 2, 1, 1),
            2 => (2, 1, 2, 1),
            _ => (4, 1, 1, 1),
        }
    } else {
        match t {
            0 => (4, 1, 1, 0),
            1 => (1, 2, 2, 1),
            2 => (1, 2, 2, 2),
            3 => (1, 2, 2, 3),
            4 => (2, 2, 1, 1),
            5 => (2, 1, 2, 1),
            6 => (2, 2, 1, 2),
            7 => (2, 1, 2, 2),
            8 => (2, 2, 1, 3),
            9 => (2, 1, 2, 3),
            10 => (4, 1, 1, 1),
            11 => (4, 1, 1, 2),
            _ => (4, 1, 1, 3),
        }
    }
}

/// Position (x, y) in 4x4 units, within its 8x8, of sub-macroblock partition `j`.
pub fn sub_pos(w: usize, h: usize, j: usize) -> (usize, usize) {
    match (w, h) {
        (2, 2) => (0, 0),
        (2, 1) => (0, j),
        (1, 2) => (j, 0),
        _ => (j % 2, j / 2),
    }
}

/// The parsed syntax of one macroblock, coefficients already placed by the inverse scans (8.5.6,
/// 8.5.7) into raster order within their blocks.
pub struct Mb {
    pub kind: MbKind,
    /// mb_type within its slice type's table (Table 7-11 for intra types).
    pub mb_type: u32,
    pub pcm: [u8; 384],
    /// Intra16x16PredMode (Table 7-11).
    pub i16_mode: u8,
    pub chroma_mode: u8,
    /// Intra4x4PredMode by luma4x4BlkIdx, or Intra8x8PredMode in [0..4].
    pub modes: [u8; 16],
    pub sub: [u8; 4],
    /// ref_idx_lX per mbPartIdx; -1 when the list is not used.
    pub ref_idx: [[i8; 4]; 2],
    /// mvd_lX[mbPartIdx][subMbPartIdx][comp].
    pub mvd: [[[[i32; 2]; 4]; 4]; 2],
    pub cbp: u8,
    pub t8x8: bool,
    pub qp: i32,
    /// Intra16x16DCLevel (raster 4x4).
    pub dc: [i32; 16],
    /// 4x4 blocks by luma4x4BlkIdx (Intra16x16 AC with [0] = 0).
    pub luma: [[i32; 16]; 16],
    pub luma8: [[i32; 64]; 4],
    /// ChromaDCLevel per iCbCr (c00, c01, c10, c11).
    pub cdc: [[i32; 4]; 2],
    /// ChromaACLevel per iCbCr and chroma4x4BlkIdx ([0] = 0).
    pub cac: [[[i32; 16]; 4]; 2],
}

impl Mb {
    pub fn new() -> Mb {
        Mb {
            kind: MbKind::PSkip,
            mb_type: 0,
            pcm: [0; 384],
            i16_mode: 0,
            chroma_mode: 0,
            modes: [2; 16],
            sub: [0; 4],
            ref_idx: [[-1; 4]; 2],
            mvd: [[[[0; 2]; 4]; 4]; 2],
            cbp: 0,
            t8x8: false,
            qp: 0,
            dc: [0; 16],
            luma: [[0; 16]; 16],
            luma8: [[0; 64]; 4],
            cdc: [[0; 4]; 2],
            cac: [[[0; 16]; 4]; 2],
        }
    }
}

impl Default for Mb {
    fn default() -> Self {
        Mb::new()
    }
}

/// Everything decoding the macroblocks of one slice reads and writes.
pub struct SliceCtx<'a> {
    pub sps: &'a Sps,
    pub pps: &'a Pps,
    pub sh: &'a SliceHeader,
    /// Slice number within the picture (neighbour availability, 6.4.4).
    pub slice: u32,
    pub mb_w: usize,
    pub mb_h: usize,
    pub st: &'a mut PicState,
    /// The picture under decoding (samples and motion field).
    pub pic: &'a mut Picture,
    /// The DPB the reference lists index.
    pub dpb: &'a [Picture],
    /// RefPicList0 / RefPicList1 as DPB indices (`usize::MAX` = "no reference picture").
    pub lists: [&'a [usize]; 2],
    /// QPY of the previous macroblock in decoding order (QPY,PRED, 7.4.5).
    pub qp: i32,
    /// mb_qp_delta of the previous macroblock in the slice was non-zero (9.3.3.1.1.5).
    pub prev_qp_nz: bool,
}

impl<'a> SliceCtx<'a> {
    pub fn slice_type(&self) -> SliceType {
        self.sh.slice_type
    }

    /// Neighbouring location (6.4.12): the block at (x, y) in units of `n`ths of a macroblock
    /// (4 for luma 4x4 blocks, 2 for chroma 4x4 blocks), relative to macroblock `addr`. Returns the
    /// macroblock holding it and the position inside that macroblock, or `None` when the location
    /// is outside the picture, right of the current macroblock below its top row, below it, or in
    /// a macroblock of another slice (or not yet decoded).
    pub fn nb(&self, addr: usize, x: isize, y: isize, n: isize) -> Option<(usize, usize, usize)> {
        let dx = if x < 0 { -1 } else if x >= n { 1 } else { 0 };
        let dy = if y < 0 { -1 } else if y >= n { 1 } else { 0 };
        if dy > 0 || (dy == 0 && dx > 0) {
            return None;
        }
        if dx == 0 && dy == 0 {
            return Some((addr, x as usize, y as usize));
        }
        let mbx = (addr % self.mb_w) as isize + dx;
        let mby = (addr / self.mb_w) as isize + dy;
        if mbx < 0 || mby < 0 || mbx >= self.mb_w as isize {
            return None;
        }
        let m = mby as usize * self.mb_w + mbx as usize;
        if self.st.info[m].slice != self.slice {
            return None;
        }
        Some((m, x.rem_euclid(n) as usize, y.rem_euclid(n) as usize))
    }

    /// mbAddrA / mbAddrB (6.4.9) when available.
    pub fn mb_a(&self, addr: usize) -> Option<usize> {
        self.nb(addr, -1, 0, 4).map(|n| n.0)
    }
    pub fn mb_b(&self, addr: usize) -> Option<usize> {
        self.nb(addr, 0, -1, 4).map(|n| n.0)
    }

    /// Index of the 4x4 block (x, y) of macroblock `m` in the picture-wide 4x4 arrays.
    #[inline]
    pub fn g4(&self, m: usize, x: usize, y: usize) -> usize {
        ((m / self.mb_w) * 4 + y) * self.mb_w * 4 + (m % self.mb_w) * 4 + x
    }

    /// Reset the per-4x4 state of macroblock `addr` before it is parsed.
    pub fn begin_mb(&mut self, addr: usize) {
        self.st.info[addr] = MbInfo { slice: self.slice, qp: self.qp as u8, ..MbInfo::default() };
        for y in 0..4 {
            for x in 0..4 {
                let g = self.g4(addr, x, y);
                self.st.mvd[g] = [[0; 2]; 2];
                self.st.intra_modes[g] = 2;
                self.pic.motion.ref_idx[g] = [-1; 2];
                self.pic.motion.ref_id[g] = [-1; 2];
                self.pic.motion.mv[g] = [Mv::ZERO; 2];
            }
        }
    }

    /// A skipped macroblock (P_Skip / B_Skip, 7.4.4): no syntax beyond the skip indication.
    pub fn skip_mb(&mut self, addr: usize) -> Mb {
        self.begin_mb(addr);
        let kind = if self.slice_type() == SliceType::B { MbKind::BSkip } else { MbKind::PSkip };
        let i = &mut self.st.info[addr];
        i.kind = kind;
        i.direct8 = if kind == MbKind::BSkip { 0xf } else { 0 };
        self.prev_qp_nz = false;
        let mut mb = Mb::new();
        mb.kind = kind;
        mb.qp = self.qp;
        mb
    }

    // -----------------------------------------------------------------------------------------
    // CABAC syntax elements
    // -----------------------------------------------------------------------------------------

    /// mb_skip_flag (ctxIdx 11..13 P, 24..26 B; 9.3.3.1.1.1).
    pub fn cabac_skip(&self, c: &mut Cabac, addr: usize) -> bool {
        let cond = |m: Option<usize>| m.is_some_and(|m| !self.st.info[m].kind.is_skip()) as usize;
        let base = if self.slice_type() == SliceType::B { 24 } else { 11 };
        c.decision(base + cond(self.mb_a(addr)) + cond(self.mb_b(addr))) == 1
    }

    /// mb_type of an I macroblock (Table 9-36) as prefix-free bins at ctxIdxOffset `off` (3 in I
    /// slices, 17 / 32 as the suffix in P / B slices, 9.3.3.1.2).
    fn cabac_mb_type_i(&self, c: &mut Cabac, addr: usize, off: usize, suffix: bool) -> u32 {
        let inc0 = if suffix {
            0
        } else {
            let cond = |m: Option<usize>| m.is_some_and(|m| !matches!(self.st.info[m].kind, MbKind::I4x4 | MbKind::I8x8)) as usize;
            cond(self.mb_a(addr)) + cond(self.mb_b(addr))
        };
        if c.decision(off + inc0) == 0 {
            return 0; // I_NxN
        }
        if c.terminate() == 1 {
            return 25; // I_PCM
        }
        let (luma, chroma0, chroma1, pred1_b3, pred0_b3, pred1, pred0) =
            if suffix { (1, 2, 2, 3, 3, 3, 3) } else { (3, 4, 5, 6, 7, 6, 7) };
        let mut t = 1 + 12 * c.decision(off + luma);
        if c.decision(off + chroma0) == 1 {
            t += 4 + 4 * c.decision(off + chroma1);
            t += 2 * c.decision(off + pred1_b3);
            t += c.decision(off + pred0_b3);
        } else {
            t += 2 * c.decision(off + pred1);
            t += c.decision(off + pred0);
        }
        t
    }

    /// mb_type (9.3.2.5, 9.3.3.1.2) in the slice type's numbering (P: intra types are 5 + I type;
    /// B: 23 + I type).
    fn cabac_mb_type(&self, c: &mut Cabac, addr: usize) -> u32 {
        match self.slice_type() {
            SliceType::I => self.cabac_mb_type_i(c, addr, 3, false),
            SliceType::P => {
                if c.decision(14) == 0 {
                    if c.decision(15) == 0 { 3 * c.decision(16) } else { 2 - c.decision(17) }
                } else {
                    5 + self.cabac_mb_type_i(c, addr, 17, true)
                }
            }
            SliceType::B => {
                let cond = |m: Option<usize>| m.is_some_and(|m| !matches!(self.st.info[m].kind, MbKind::BSkip | MbKind::BDirect)) as usize;
                if c.decision(27 + cond(self.mb_a(addr)) + cond(self.mb_b(addr))) == 0 {
                    return 0;
                }
                if c.decision(27 + 3) == 0 {
                    return 1 + c.decision(27 + 5);
                }
                let mut bits = c.decision(27 + 4) << 3;
                bits |= c.decision(27 + 5) << 2;
                bits |= c.decision(27 + 5) << 1;
                bits |= c.decision(27 + 5);
                match bits {
                    0..=7 => bits + 3,
                    13 => 23 + self.cabac_mb_type_i(c, addr, 32, true),
                    14 => 11,
                    15 => 22,
                    _ => ((bits << 1) | c.decision(27 + 5)) - 4,
                }
            }
        }
    }

    /// sub_mb_type (Table 9-38, ctxIdx 21..23 P, 36..39 B).
    fn cabac_sub_mb_type(&self, c: &mut Cabac) -> u8 {
        if self.slice_type() == SliceType::P {
            if c.decision(21) == 1 {
                0
            } else if c.decision(22) == 0 {
                1
            } else if c.decision(23) == 1 {
                2
            } else {
                3
            }
        } else {
            if c.decision(36) == 0 {
                return 0;
            }
            if c.decision(37) == 0 {
                return 1 + c.decision(39) as u8;
            }
            let mut t = 3u8;
            if c.decision(38) == 1 {
                if c.decision(39) == 1 {
                    return 11 + c.decision(39) as u8;
                }
                t += 4;
            }
            t += 2 * c.decision(39) as u8;
            t += c.decision(39) as u8;
            t
        }
    }

    /// Is the 4x4 block (x, y) of macroblock `m` predicted in direct mode (B_Skip, B_Direct_16x16,
    /// B_Direct_8x8)?
    fn is_direct(&self, m: usize, x: usize, y: usize) -> bool {
        let i = &self.st.info[m];
        (i.direct8 >> ((y / 2) * 2 + x / 2)) & 1 == 1
    }

    /// ref_idx_lX (U binarization, ctxIdx 54..59; 9.3.3.1.1.6) for the partition whose top-left
    /// 4x4 block is (x, y).
    fn cabac_ref_idx(&self, c: &mut Cabac, addr: usize, list: usize, x: usize, y: usize) -> u32 {
        let cond = |n: Option<(usize, usize, usize)>| -> usize {
            let Some((m, nx, ny)) = n else { return 0 };
            let i = &self.st.info[m];
            if i.kind.is_skip() || i.kind.is_intra() || self.is_direct(m, nx, ny) {
                return 0;
            }
            (self.pic.motion.ref_idx[self.g4(m, nx, ny)][list] > 0) as usize
        };
        let inc = cond(self.nb(addr, x as isize - 1, y as isize, 4)) + 2 * cond(self.nb(addr, x as isize, y as isize - 1, 4));
        if c.decision(54 + inc) == 0 {
            return 0;
        }
        let mut v = 1;
        if c.decision(54 + 4) == 0 {
            return v;
        }
        v += 1;
        while c.decision(54 + 5) == 1 {
            v += 1;
            if v > 32 {
                break;
            }
        }
        v
    }

    /// mvd_lX[][][comp] (UEG3, signedValFlag 1, uCoff 9; ctxIdx 40..46 / 47..53, 9.3.3.1.1.7).
    fn cabac_mvd(&self, c: &mut Cabac, addr: usize, list: usize, comp: usize, x: usize, y: usize) -> i32 {
        let abs = |n: Option<(usize, usize, usize)>| n.map_or(0u32, |(m, nx, ny)| self.st.mvd[self.g4(m, nx, ny)][list][comp] as u32);
        let sum = abs(self.nb(addr, x as isize - 1, y as isize, 4)) + abs(self.nb(addr, x as isize, y as isize - 1, 4));
        let base = if comp == 0 { 40 } else { 47 };
        let inc0 = if sum < 3 {
            0
        } else if sum <= 32 {
            1
        } else {
            2
        };
        if c.decision(base + inc0) == 0 {
            return 0;
        }
        let mut v: u32 = 1;
        while v < 9 && c.decision(base + [3, 4, 5, 6, 6, 6, 6, 6][v as usize - 1]) == 1 {
            v += 1;
        }
        if v >= 9 {
            v = v.wrapping_add(c.exp_golomb_bypass(3));
        }
        let v = v.min(1 << 20) as i32;
        if c.bypass() == 1 { -v } else { v }
    }

    /// coded_block_pattern (9.3.2.6 prefix FL / suffix TU; ctxIdx 73..84, 9.3.3.1.1.4).
    fn cabac_cbp(&self, c: &mut Cabac, addr: usize) -> u8 {
        let mut luma = 0u8;
        for b8 in 0..4usize {
            let (bx, by) = ((b8 % 2) * 2, (b8 / 2) * 2);
            let cond = |n: Option<(usize, usize, usize)>, luma: u8| -> usize {
                let Some((m, nx, ny)) = n else { return 0 };
                let b8n = (ny / 2) * 2 + nx / 2;
                if m == addr {
                    return ((luma >> b8n) & 1 == 0) as usize;
                }
                let i = &self.st.info[m];
                if i.kind == MbKind::IPcm {
                    return 0;
                }
                if !i.kind.is_skip() && (i.cbp >> b8n) & 1 != 0 {
                    return 0;
                }
                1
            };
            let a = cond(self.nb(addr, bx as isize - 1, by as isize, 4), luma);
            let b = cond(self.nb(addr, bx as isize, by as isize - 1, 4), luma);
            luma |= (c.decision(73 + a + 2 * b) as u8) << b8;
        }
        let cond = |m: Option<usize>, bin: u32| -> usize {
            let Some(m) = m else { return 0 };
            let i = &self.st.info[m];
            if i.kind == MbKind::IPcm {
                return 1;
            }
            let cc = i.cbp >> 4;
            (!i.kind.is_skip() && if bin == 0 { cc != 0 } else { cc == 2 }) as usize
        };
        let (ma, mb) = (self.mb_a(addr), self.mb_b(addr));
        let mut chroma = 0u8;
        if c.decision(77 + cond(ma, 0) + 2 * cond(mb, 0)) == 1 {
            chroma = 1 + c.decision(77 + 4 + cond(ma, 1) + 2 * cond(mb, 1)) as u8;
        }
        luma | (chroma << 4)
    }

    /// intra_chroma_pred_mode (TU cMax 3; ctxIdx 64..67, 9.3.3.1.1.8).
    fn cabac_chroma_mode(&self, c: &mut Cabac, addr: usize) -> u8 {
        let cond = |m: Option<usize>| {
            m.is_some_and(|m| {
                let i = &self.st.info[m];
                i.kind.is_intra() && i.kind != MbKind::IPcm && i.chroma_mode != 0
            }) as usize
        };
        if c.decision(64 + cond(self.mb_a(addr)) + cond(self.mb_b(addr))) == 0 {
            return 0;
        }
        if c.decision(67) == 0 {
            return 1;
        }
        if c.decision(67) == 0 { 2 } else { 3 }
    }

    /// mb_qp_delta (Table 9-3 mapping, U; ctxIdx 60..63, 9.3.3.1.1.5).
    fn cabac_qp_delta(&self, c: &mut Cabac) -> Result<i32> {
        if c.decision(60 + self.prev_qp_nz as usize) == 0 {
            return Ok(0);
        }
        let mut k = 1i32;
        while c.decision(if k == 1 { 62 } else { 63 }) == 1 {
            k += 1;
            if k > 104 {
                return Err(Error::Invalid("mb_qp_delta out of range"));
            }
        }
        Ok(if k & 1 == 1 { (k + 1) / 2 } else { -(k / 2) })
    }

    /// transform_size_8x8_flag (ctxIdx 399..401, 9.3.3.1.1.10).
    fn cabac_t8x8(&self, c: &mut Cabac, addr: usize) -> bool {
        let cond = |m: Option<usize>| m.is_some_and(|m| self.st.info[m].t8x8) as usize;
        c.decision(399 + cond(self.mb_a(addr)) + cond(self.mb_b(addr))) == 1
    }

    // -----------------------------------------------------------------------------------------
    // Entropy-independent element readers
    // -----------------------------------------------------------------------------------------

    fn read_ref(&self, e: &mut Entropy, addr: usize, list: usize, x: usize, y: usize) -> Result<i8> {
        let n = self.sh.num_ref_idx_active[list];
        if n <= 1 {
            return Ok(0);
        }
        let v = match e {
            Entropy::Cavlc(r) => r.te(n - 1),
            Entropy::Cabac(c) => self.cabac_ref_idx(c, addr, list, x, y),
        };
        if v >= n {
            return Err(Error::Invalid("ref_idx beyond num_ref_idx_active"));
        }
        Ok(v as i8)
    }

    fn read_mvd(&self, e: &mut Entropy, addr: usize, list: usize, x: usize, y: usize) -> Result<[i32; 2]> {
        let v = match e {
            Entropy::Cavlc(r) => [r.se(), r.se()],
            Entropy::Cabac(c) => [self.cabac_mvd(c, addr, list, 0, x, y), self.cabac_mvd(c, addr, list, 1, x, y)],
        };
        if v[0].abs() > 16384 || v[1].abs() > 16384 {
            return Err(Error::Invalid("mvd out of range"));
        }
        Ok(v)
    }

    /// Record ref_idx_lX for the 4x4 blocks of a rectangle (for later context derivations).
    fn put_ref(&mut self, addr: usize, list: usize, (x, y, w, h): (usize, usize, usize, usize), r: i8) {
        for yy in y..y + h {
            for xx in x..x + w {
                let g = self.g4(addr, xx, yy);
                self.pic.motion.ref_idx[g][list] = r;
            }
        }
    }

    fn put_mvd(&mut self, addr: usize, list: usize, (x, y, w, h): (usize, usize, usize, usize), v: [i32; 2]) {
        let a = [v[0].unsigned_abs().min(255) as u8, v[1].unsigned_abs().min(255) as u8];
        for yy in y..y + h {
            for xx in x..x + w {
                let g = self.g4(addr, xx, yy);
                self.st.mvd[g][list] = a;
            }
        }
    }

    /// Intra4x4PredMode / Intra8x8PredMode prediction (8.3.1.1 / 8.3.2.1) for the block whose
    /// top-left 4x4 block is (x, y): Min of the modes left and above, DC (2) when either neighbour
    /// is unavailable or is inter under constrained_intra_pred_flag (dcPredModePredictedFlag), and
    /// 2 for a neighbour not coded in Intra_4x4 / Intra_8x8.
    fn predicted_intra_mode(&self, addr: usize, x: usize, y: usize) -> u8 {
        let mode = |n: Option<(usize, usize, usize)>| -> Option<i8> {
            let (m, nx, ny) = n?;
            let i = &self.st.info[m];
            if !i.kind.is_intra() && self.pps.constrained_intra_pred {
                return None;
            }
            Some(if matches!(i.kind, MbKind::I4x4 | MbKind::I8x8) { self.st.intra_modes[self.g4(m, nx, ny)] } else { 2 })
        };
        match (mode(self.nb(addr, x as isize - 1, y as isize, 4)), mode(self.nb(addr, x as isize, y as isize - 1, 4))) {
            (Some(a), Some(b)) => a.min(b) as u8,
            _ => 2,
        }
    }

    fn read_intra_mode(&self, e: &mut Entropy, pred: u8) -> u8 {
        let (prev, rem) = match e {
            Entropy::Cavlc(r) => {
                if r.flag() {
                    (true, 0)
                } else {
                    (false, r.u(3) as u8)
                }
            }
            Entropy::Cabac(c) => {
                if c.decision(68) == 1 {
                    (true, 0)
                } else {
                    let v = c.decision(69) | (c.decision(69) << 1) | (c.decision(69) << 2);
                    (false, v as u8)
                }
            }
        };
        if prev {
            pred
        } else if rem < pred {
            rem
        } else {
            rem + 1
        }
    }

    // -----------------------------------------------------------------------------------------
    // macroblock_layer
    // -----------------------------------------------------------------------------------------

    /// Parse macroblock_layer() of macroblock `addr` (not skipped).
    pub fn parse_mb(&mut self, e: &mut Entropy, addr: usize) -> Result<Mb> {
        self.begin_mb(addr);
        let mut mb = Mb::new();
        let raw = match e {
            Entropy::Cavlc(r) => r.ue(),
            Entropy::Cabac(c) => self.cabac_mb_type(c, addr),
        };
        let st = self.slice_type();
        let (intra_type, inter_type) = match st {
            SliceType::I => (Some(raw), None),
            SliceType::P => {
                if raw < 5 {
                    (None, Some(raw))
                } else {
                    (Some(raw - 5), None)
                }
            }
            SliceType::B => {
                if raw < 23 {
                    (None, Some(raw))
                } else {
                    (Some(raw - 23), None)
                }
            }
        };
        if let Some(it) = intra_type {
            if it > 25 {
                return Err(Error::Invalid("mb_type out of range"));
            }
            mb.mb_type = it;
            if it == 25 {
                return self.parse_pcm(e, addr, mb);
            }
            mb.kind = if it == 0 { MbKind::I4x4 } else { MbKind::I16x16 };
        } else {
            mb.mb_type = inter_type.unwrap_or(0);
            mb.kind = if st == SliceType::B && mb.mb_type == 0 { MbKind::BDirect } else { MbKind::Inter };
        }
        self.st.info[addr].kind = mb.kind;

        let mut no_sub_lt8x8 = true;
        match mb.kind {
            MbKind::I4x4 | MbKind::I16x16 => {
                if mb.kind == MbKind::I4x4 && self.pps.transform_8x8_mode {
                    mb.t8x8 = match e {
                        Entropy::Cavlc(r) => r.flag(),
                        Entropy::Cabac(c) => self.cabac_t8x8(c, addr),
                    };
                    if mb.t8x8 {
                        mb.kind = MbKind::I8x8;
                        self.st.info[addr].kind = MbKind::I8x8;
                        self.st.info[addr].t8x8 = true;
                    }
                }
                match mb.kind {
                    MbKind::I4x4 => {
                        for blk in 0..16 {
                            let (x, y) = (BLK_X[blk], BLK_Y[blk]);
                            let pred = self.predicted_intra_mode(addr, x, y);
                            let m = self.read_intra_mode(e, pred);
                            mb.modes[blk] = m;
                            let g = self.g4(addr, x, y);
                            self.st.intra_modes[g] = m as i8;
                        }
                    }
                    MbKind::I8x8 => {
                        for b8 in 0..4 {
                            let (x, y) = ((b8 % 2) * 2, (b8 / 2) * 2);
                            let pred = self.predicted_intra_mode(addr, x, y);
                            let m = self.read_intra_mode(e, pred);
                            mb.modes[b8] = m;
                            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                                let g = self.g4(addr, x + dx, y + dy);
                                self.st.intra_modes[g] = m as i8;
                            }
                        }
                    }
                    _ => {
                        let t = mb.mb_type - 1;
                        mb.i16_mode = (t % 4) as u8;
                        mb.cbp = (((t / 4) % 3) << 4) as u8 | if t >= 12 { 15 } else { 0 };
                    }
                }
                mb.chroma_mode = match e {
                    Entropy::Cavlc(r) => r.ue() as u8,
                    Entropy::Cabac(c) => self.cabac_chroma_mode(c, addr),
                };
                if mb.chroma_mode > 3 {
                    return Err(Error::Invalid("intra_chroma_pred_mode > 3"));
                }
                self.st.info[addr].chroma_mode = mb.chroma_mode;
            }
            MbKind::BDirect => {
                self.st.info[addr].direct8 = 0xf;
                no_sub_lt8x8 = self.sps.direct_8x8_inference;
            }
            _ => {
                let (shape, pf) = mb_partitions(st, mb.mb_type);
                if shape == SHAPE_8X8 {
                    no_sub_lt8x8 = self.parse_sub_mb_pred(e, addr, &mut mb)?;
                } else {
                    self.parse_mb_pred_inter(e, addr, &mut mb, shape, pf)?;
                }
            }
        }

        if mb.kind != MbKind::I16x16 {
            mb.cbp = match e {
                Entropy::Cavlc(r) => {
                    let k = r.ue() as usize;
                    if k > 47 {
                        return Err(Error::Invalid("coded_block_pattern codeNum > 47"));
                    }
                    if mb.kind.is_intra() { CBP_INTRA[k] } else { CBP_INTER[k] }
                }
                Entropy::Cabac(c) => self.cabac_cbp(c, addr),
            };
            self.st.info[addr].cbp = mb.cbp;
            if mb.cbp & 15 != 0 && self.pps.transform_8x8_mode && !mb.kind.is_intra() && no_sub_lt8x8 {
                mb.t8x8 = match e {
                    Entropy::Cavlc(r) => r.flag(),
                    Entropy::Cabac(c) => self.cabac_t8x8(c, addr),
                };
                self.st.info[addr].t8x8 = mb.t8x8;
            }
        }
        self.st.info[addr].cbp = mb.cbp;

        let mut delta = 0;
        if mb.cbp != 0 || mb.kind == MbKind::I16x16 {
            delta = match e {
                Entropy::Cavlc(r) => r.se(),
                Entropy::Cabac(c) => self.cabac_qp_delta(c)?,
            };
            if !(-26..=25).contains(&delta) {
                return Err(Error::Invalid("mb_qp_delta outside -26..25"));
            }
            self.qp = (self.qp + delta + 52) % 52;
        }
        self.prev_qp_nz = delta != 0;
        mb.qp = self.qp;
        self.st.info[addr].qp = self.qp as u8;
        if mb.cbp != 0 || mb.kind == MbKind::I16x16 {
            self.parse_residual(e, addr, &mut mb)?;
        }
        if let Entropy::Cavlc(r) = e {
            if r.overran() {
                return Err(Error::Truncated);
            }
        }
        Ok(mb)
    }

    fn parse_pcm(&mut self, e: &mut Entropy, addr: usize, mut mb: Mb) -> Result<Mb> {
        mb.kind = MbKind::IPcm;
        match e {
            Entropy::Cavlc(r) => {
                while !r.byte_aligned() {
                    r.bit();
                }
                for v in mb.pcm.iter_mut() {
                    *v = r.u(8) as u8;
                }
                if r.overran() {
                    return Err(Error::Truncated);
                }
            }
            Entropy::Cabac(c) => {
                let start = c.pos().div_ceil(8);
                let d = c.data();
                if start + 384 > d.len() {
                    return Err(Error::Truncated);
                }
                mb.pcm.copy_from_slice(&d[start..start + 384]);
                c.restart_at((start + 384) * 8);
            }
        }
        let i = &mut self.st.info[addr];
        i.kind = MbKind::IPcm;
        i.cbp = 0x2f;
        i.cbf = u32::MAX;
        i.tc = [16; 24];
        i.nz = 0xffff;
        i.qp = self.qp as u8;
        self.prev_qp_nz = false;
        mb.qp = self.qp;
        Ok(mb)
    }

    /// mb_pred() of an inter macroblock that is not 8x8 (7.3.5.1): every ref_idx_l0, every
    /// ref_idx_l1, every mvd_l0, every mvd_l1.
    fn parse_mb_pred_inter(&mut self, e: &mut Entropy, addr: usize, mb: &mut Mb, shape: u8, pf: [PredFlags; 2]) -> Result<()> {
        let n = num_parts(shape);
        for list in 0..2 {
            for p in 0..n {
                let rect = part_rect(shape, p);
                if pf[p] >> list & 1 == 1 {
                    let r = self.read_ref(e, addr, list, rect.0, rect.1)?;
                    mb.ref_idx[list][p] = r;
                    self.put_ref(addr, list, rect, r);
                }
            }
        }
        for list in 0..2 {
            for p in 0..n {
                let rect = part_rect(shape, p);
                if pf[p] >> list & 1 == 1 {
                    let v = self.read_mvd(e, addr, list, rect.0, rect.1)?;
                    mb.mvd[list][p][0] = v;
                    self.put_mvd(addr, list, rect, v);
                }
            }
        }
        Ok(())
    }

    /// sub_mb_pred() (7.3.5.2). Returns noSubMbPartSizeLessThan8x8Flag.
    fn parse_sub_mb_pred(&mut self, e: &mut Entropy, addr: usize, mb: &mut Mb) -> Result<bool> {
        let st = self.slice_type();
        let mut no_lt8x8 = true;
        for i in 0..4 {
            let t = match e {
                Entropy::Cavlc(r) => r.ue(),
                Entropy::Cabac(c) => self.cabac_sub_mb_type(c) as u32,
            };
            if (st == SliceType::P && t > 3) || t > 12 {
                return Err(Error::Invalid("sub_mb_type out of range"));
            }
            mb.sub[i] = t as u8;
            let (np, _, _, _) = sub_partitions(st, t as u8);
            if st == SliceType::B && t == 0 {
                self.st.info[addr].direct8 |= 1 << i;
                if !self.sps.direct_8x8_inference {
                    no_lt8x8 = false;
                }
            } else if np > 1 {
                no_lt8x8 = false;
            }
        }
        let ref0 = st == SliceType::P && mb.mb_type == 4; // P_8x8ref0
        for list in 0..2 {
            for i in 0..4 {
                let (_, _, _, pf) = sub_partitions(st, mb.sub[i]);
                if pf >> list & 1 == 1 {
                    let rect = part_rect(SHAPE_8X8, i);
                    let r = if ref0 { 0 } else { self.read_ref(e, addr, list, rect.0, rect.1)? };
                    mb.ref_idx[list][i] = r;
                    self.put_ref(addr, list, rect, r);
                }
            }
        }
        for list in 0..2 {
            for i in 0..4 {
                let (np, w, h, pf) = sub_partitions(st, mb.sub[i]);
                if pf >> list & 1 == 1 {
                    let (bx, by, _, _) = part_rect(SHAPE_8X8, i);
                    for j in 0..np {
                        let (sx, sy) = sub_pos(w, h, j);
                        let rect = (bx + sx, by + sy, w, h);
                        let v = self.read_mvd(e, addr, list, rect.0, rect.1)?;
                        mb.mvd[list][i][j] = v;
                        self.put_mvd(addr, list, rect, v);
                    }
                }
            }
        }
        Ok(no_lt8x8)
    }

    // -----------------------------------------------------------------------------------------
    // residual
    // -----------------------------------------------------------------------------------------

    /// TotalCoeff of the luma (chroma: `Some(iCbCr)`) 4x4 block at a neighbouring location
    /// (9.2.1): 0 for a skipped macroblock, 16 for I_PCM.
    fn tc_at(&self, n: (usize, usize, usize), chroma: Option<usize>) -> i32 {
        let (m, x, y) = n;
        let i = &self.st.info[m];
        if i.kind.is_skip() {
            return 0;
        }
        if i.kind == MbKind::IPcm {
            return 16;
        }
        match chroma {
            None => i.tc[blk_at(x, y)] as i32,
            Some(c) => i.tc[16 + c * 4 + y * 2 + x] as i32,
        }
    }

    /// nC of a luma 4x4 block (9.2.1).
    fn nc_luma(&self, addr: usize, blk: usize) -> i32 {
        let (x, y) = (BLK_X[blk] as isize, BLK_Y[blk] as isize);
        let a = self.nb(addr, x - 1, y, 4).map(|n| self.tc_at(n, None));
        let b = self.nb(addr, x, y - 1, 4).map(|n| self.tc_at(n, None));
        combine_nc(a, b)
    }

    /// nC of a chroma AC block (9.2.1, chroma4x4BlkIdx in a 2x2 grid).
    fn nc_chroma(&self, addr: usize, c: usize, blk: usize) -> i32 {
        let (x, y) = ((blk % 2) as isize, (blk / 2) as isize);
        let a = self.nb(addr, x - 1, y, 2).map(|n| self.tc_at(n, Some(c)));
        let b = self.nb(addr, x, y - 1, 2).map(|n| self.tc_at(n, Some(c)));
        combine_nc(a, b)
    }

    /// condTermFlagN of coded_block_flag (9.3.3.1.1.9) for a neighbour macroblock with
    /// `trans` = Some(coded_block_flag of transBlockN) or None when transBlockN is not available.
    fn cbf_cond(&self, m: Option<usize>, cur_intra: bool, trans: impl Fn(&MbInfo) -> Option<bool>) -> u32 {
        match m {
            None => cur_intra as u32,
            Some(m) => {
                let i = &self.st.info[m];
                if i.kind == MbKind::IPcm {
                    1
                } else if i.kind.is_skip() {
                    0
                } else {
                    trans(i).unwrap_or(false) as u32
                }
            }
        }
    }

    fn cbf_luma_inc(&self, addr: usize, blk: usize, cur_intra: bool) -> u32 {
        let (x, y) = (BLK_X[blk] as isize, BLK_Y[blk] as isize);
        let f = |n: Option<(usize, usize, usize)>| {
            let (m, nx, ny) = match n {
                Some(v) => (Some(v.0), v.1, v.2),
                None => (None, 0, 0),
            };
            self.cbf_cond(m, cur_intra, |i| {
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
        f(self.nb(addr, x - 1, y, 4)) + 2 * f(self.nb(addr, x, y - 1, 4))
    }

    fn cbf_chroma_ac_inc(&self, addr: usize, c: usize, blk: usize, cur_intra: bool) -> u32 {
        let (x, y) = ((blk % 2) as isize, (blk / 2) as isize);
        let f = |n: Option<(usize, usize, usize)>| {
            let (m, nx, ny) = match n {
                Some(v) => (Some(v.0), v.1, v.2),
                None => (None, 0, 0),
            };
            self.cbf_cond(m, cur_intra, |i| if i.cbp >> 4 == 2 { Some((i.cbf >> (CBF_CHROMA_AC as usize + c * 4 + ny * 2 + nx)) & 1 == 1) } else { None })
        };
        f(self.nb(addr, x - 1, y, 2)) + 2 * f(self.nb(addr, x, y - 1, 2))
    }

    /// residual( 0, 15 ) (7.3.5.3) into `mb`, recording TotalCoeff, coded_block_flag and the
    /// non-zero map in the macroblock's [`MbInfo`].
    fn parse_residual(&mut self, e: &mut Entropy, addr: usize, mb: &mut Mb) -> Result<()> {
        let cur_intra = mb.kind.is_intra();
        let i16 = mb.kind == MbKind::I16x16;
        let mut buf = [0i32; 64];
        let mut nz: u16 = 0;
        // Intra16x16DCLevel
        if i16 {
            let n = match e {
                Entropy::Cavlc(r) => {
                    let nc = self.nc_luma(addr, 0);
                    cavlc::residual_block(r, nc, 16, &mut buf)?
                }
                Entropy::Cabac(c) => {
                    let f = |m: Option<usize>| self.cbf_cond(m, cur_intra, |i| if i.kind == MbKind::I16x16 { Some((i.cbf >> CBF_LUMA_DC) & 1 == 1) } else { None });
                    let inc = f(self.mb_a(addr)) + 2 * f(self.mb_b(addr));
                    if c.decision(Cabac::cbf_ctx(cabac::CAT_LUMA_DC, inc)) == 1 {
                        self.st.info[addr].cbf |= 1 << CBF_LUMA_DC;
                        c.residual_block(cabac::CAT_LUMA_DC, 16, &mut buf).max(1)
                    } else {
                        0
                    }
                }
            };
            if n > 0 {
                for k in 0..16 {
                    mb.dc[ZIGZAG4[k] as usize] = buf[k];
                }
            }
        }
        for b8 in 0..4 {
            let coded = (mb.cbp >> b8) & 1 == 1;
            if mb.t8x8 {
                if !coded {
                    continue;
                }
                match e {
                    Entropy::Cavlc(r) => {
                        // Four interleaved 4x4 blocks (7.3.5.3.2: lumaLevel8x8[4 * i + i4x4]).
                        for sub in 0..4 {
                            let blk = b8 * 4 + sub;
                            let nc = self.nc_luma(addr, blk);
                            let n = cavlc::residual_block(r, nc, 16, &mut buf)?;
                            self.st.info[addr].tc[blk] = n as u8;
                            for k in 0..16 {
                                mb.luma8[b8][ZIGZAG8[4 * k + sub] as usize] = buf[k];
                            }
                        }
                    }
                    Entropy::Cabac(c) => {
                        c.residual_block(cabac::CAT_LUMA_8X8, 64, &mut buf);
                        for k in 0..64 {
                            mb.luma8[b8][ZIGZAG8[k] as usize] = buf[k];
                        }
                        for sub in 0..4 {
                            self.st.info[addr].cbf |= 1 << (b8 * 4 + sub);
                        }
                    }
                }
                if mb.luma8[b8].iter().any(|&v| v != 0) {
                    nz |= 0xf << (b8 * 4);
                }
                continue;
            }
            for sub in 0..4 {
                let blk = b8 * 4 + sub;
                if !coded {
                    continue;
                }
                let max = if i16 { 15 } else { 16 };
                let n = match e {
                    Entropy::Cavlc(r) => {
                        let nc = self.nc_luma(addr, blk);
                        cavlc::residual_block(r, nc, max, &mut buf)?
                    }
                    Entropy::Cabac(c) => {
                        let inc = self.cbf_luma_inc(addr, blk, cur_intra);
                        let cat = if i16 { cabac::CAT_LUMA_AC } else { cabac::CAT_LUMA_4X4 };
                        if c.decision(Cabac::cbf_ctx(cat, inc)) == 1 {
                            self.st.info[addr].cbf |= 1 << blk;
                            c.residual_block(cat, max, &mut buf).max(1)
                        } else {
                            0
                        }
                    }
                };
                self.st.info[addr].tc[blk] = n as u8;
                if n > 0 {
                    nz |= 1 << blk;
                    let off = if i16 { 1 } else { 0 };
                    for k in 0..max {
                        mb.luma[blk][ZIGZAG4[k + off] as usize] = buf[k];
                    }
                }
            }
        }
        // Chroma (ChromaArrayType 1: NumC8x8 = 1).
        let cc = mb.cbp >> 4;
        if cc != 0 {
            for c in 0..2 {
                let n = match e {
                    Entropy::Cavlc(r) => cavlc::residual_block(r, -1, 4, &mut buf)?,
                    Entropy::Cabac(cb) => {
                        let f = |m: Option<usize>| self.cbf_cond(m, cur_intra, |i| if i.cbp >> 4 != 0 { Some((i.cbf >> (CBF_CHROMA_DC as usize + c)) & 1 == 1) } else { None });
                        let inc = f(self.mb_a(addr)) + 2 * f(self.mb_b(addr));
                        if cb.decision(Cabac::cbf_ctx(cabac::CAT_CHROMA_DC, inc)) == 1 {
                            self.st.info[addr].cbf |= 1 << (CBF_CHROMA_DC as usize + c);
                            cb.residual_block(cabac::CAT_CHROMA_DC, 4, &mut buf).max(1)
                        } else {
                            0
                        }
                    }
                };
                if n > 0 {
                    mb.cdc[c].copy_from_slice(&buf[..4]);
                }
            }
        }
        if cc == 2 {
            for c in 0..2 {
                for blk in 0..4 {
                    let n = match e {
                        Entropy::Cavlc(r) => {
                            let nc = self.nc_chroma(addr, c, blk);
                            cavlc::residual_block(r, nc, 15, &mut buf)?
                        }
                        Entropy::Cabac(cb) => {
                            let inc = self.cbf_chroma_ac_inc(addr, c, blk, cur_intra);
                            if cb.decision(Cabac::cbf_ctx(cabac::CAT_CHROMA_AC, inc)) == 1 {
                                self.st.info[addr].cbf |= 1 << (CBF_CHROMA_AC as usize + c * 4 + blk);
                                cb.residual_block(cabac::CAT_CHROMA_AC, 15, &mut buf).max(1)
                            } else {
                                0
                            }
                        }
                    };
                    self.st.info[addr].tc[16 + c * 4 + blk] = n as u8;
                    if n > 0 {
                        for k in 0..15 {
                            mb.cac[c][blk][ZIGZAG4[k + 1] as usize] = buf[k];
                        }
                    }
                }
            }
        }
        self.st.info[addr].nz = nz;
        Ok(())
    }
}

/// nC from the neighbours' TotalCoeff (9.2.1): the rounded mean of two, either one, or 0.
fn combine_nc(a: Option<i32>, b: Option<i32>) -> i32 {
    match (a, b) {
        (Some(a), Some(b)) => (a + b + 1) >> 1,
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => 0,
    }
}
