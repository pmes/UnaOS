// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Decoded pictures and the per-macroblock / per-4x4 state the decoding process reads back:
//! sample planes (8-bit, sized to whole macroblocks), the motion field every later picture's
//! direct prediction and the deblocking filter consult, and the macroblock facts the CABAC context
//! derivations (9.3.3.1.1), CAVLC nC (9.2.1), intra mode prediction (8.3.1.1) and the deblocking
//! filter (8.7.2) need from neighbours.

use alloc::vec;
use alloc::vec::Vec;

/// x / y of luma4x4BlkIdx in 4x4 units (6.4.3, inverse 4x4 luma block scanning).
pub const BLK_X: [usize; 16] = [0, 1, 0, 1, 2, 3, 2, 3, 0, 1, 0, 1, 2, 3, 2, 3];
pub const BLK_Y: [usize; 16] = [0, 0, 1, 1, 0, 0, 1, 1, 2, 2, 3, 3, 2, 2, 3, 3];

/// luma4x4BlkIdx of the 4x4 block at (x, y) in 4x4 units (6.4.13.1).
#[inline]
pub fn blk_at(x: usize, y: usize) -> usize {
    (y / 2) * 8 + (x / 2) * 4 + (y % 2) * 2 + (x % 2)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MbKind {
    I4x4,
    I8x8,
    I16x16,
    IPcm,
    PSkip,
    BSkip,
    /// B_Direct_16x16.
    BDirect,
    /// Every other P or B macroblock type.
    Inter,
}

impl MbKind {
    pub fn is_intra(self) -> bool {
        matches!(self, MbKind::I4x4 | MbKind::I8x8 | MbKind::I16x16 | MbKind::IPcm)
    }
    pub fn is_skip(self) -> bool {
        matches!(self, MbKind::PSkip | MbKind::BSkip)
    }
}

/// coded_block_flag bits of [`MbInfo::cbf`]: luma 4x4 blocks 0..15 by luma4x4BlkIdx, then:
pub const CBF_LUMA_DC: u32 = 16;
pub const CBF_CHROMA_DC: u32 = 17; // + iCbCr
pub const CBF_CHROMA_AC: u32 = 19; // + iCbCr * 4 + chroma4x4BlkIdx

/// What later macroblocks need to know about a decoded macroblock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MbInfo {
    /// Slice number within the picture; `u32::MAX` until decoded.
    pub slice: u32,
    pub kind: MbKind,
    /// CodedBlockPatternLuma | CodedBlockPatternChroma << 4.
    pub cbp: u8,
    pub t8x8: bool,
    /// QPY (I_PCM keeps QPY,PRED here; the deblocking filter uses 0 for it, 8.7.2.2).
    pub qp: u8,
    pub chroma_mode: u8,
    pub cbf: u32,
    /// TotalCoeff(coeff_token) per luma 4x4 block (by luma4x4BlkIdx), then Cb, Cr AC blocks.
    pub tc: [u8; 24],
    /// Luma 4x4 blocks (by luma4x4BlkIdx) whose transform block has non-zero coefficients.
    pub nz: u16,
    /// Sub-macroblocks predicted in direct mode (bit per mbPartIdx; 0xf for B_Skip / B_Direct).
    pub direct8: u8,
}

impl Default for MbInfo {
    fn default() -> Self {
        MbInfo { slice: u32::MAX, kind: MbKind::PSkip, cbp: 0, t8x8: false, qp: 0, chroma_mode: 0, cbf: 0, tc: [0; 24], nz: 0, direct8: 0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mv {
    pub x: i16,
    pub y: i16,
}

impl Mv {
    pub const ZERO: Mv = Mv { x: 0, y: 0 };
    pub fn new(x: i32, y: i32) -> Mv {
        Mv { x: x as i16, y: y as i16 }
    }
}

#[derive(Clone, Debug)]
pub struct Plane {
    pub data: Vec<u8>,
    pub w: usize,
    pub h: usize,
}

impl Plane {
    pub fn new(w: usize, h: usize, fill: u8) -> Plane {
        Plane { data: vec![fill; w * h], w, h }
    }

    /// The sample at (x, y) with the coordinates clamped into the plane (8.4.2.2.1 / 8.4.2.2.2
    /// reference sample padding).
    #[inline]
    pub fn at(&self, x: i32, y: i32) -> u8 {
        let xc = x.clamp(0, self.w as i32 - 1) as usize;
        let yc = y.clamp(0, self.h as i32 - 1) as usize;
        self.data[yc * self.w + xc]
    }
}

/// The motion field of a picture, per 4x4 luma block, for direct prediction of later pictures
/// (8.4.1.2) and for the deblocking filter (8.7.2.1).
#[derive(Clone, Debug)]
pub struct Motion {
    /// 4x4 blocks per row.
    pub w4: usize,
    pub mv: Vec<[Mv; 2]>,
    /// refIdxL0 / refIdxL1; -1 when the list is not used (or the block is intra).
    pub ref_idx: Vec<[i8; 2]>,
    /// The DPB id of the picture each list references; -1 when unused.
    pub ref_id: Vec<[i32; 2]>,
    /// Per macroblock: coded in an intra mode.
    pub mb_intra: Vec<bool>,
}

impl Motion {
    pub fn new(mb_w: usize, mb_h: usize) -> Motion {
        let n = mb_w * mb_h * 16;
        Motion { w4: mb_w * 4, mv: vec![[Mv::ZERO; 2]; n], ref_idx: vec![[-1; 2]; n], ref_id: vec![[-1; 2]; n], mb_intra: vec![true; mb_w * mb_h] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefState {
    Unused,
    Short,
    Long,
}

/// A decoded frame and its decoding-process state.
#[derive(Clone, Debug)]
pub struct Picture {
    pub planes: [Plane; 3],
    pub mb_w: usize,
    pub mb_h: usize,
    pub motion: Motion,
    /// Unique within the decoder's lifetime; what [`Motion::ref_id`] stores.
    pub id: i32,
    pub top_poc: i32,
    pub bottom_poc: i32,
    /// PicOrderCnt(frame) = Min(TopFieldOrderCnt, BottomFieldOrderCnt) (8.2.1).
    pub poc: i32,
    pub frame_num: u32,
    pub frame_num_wrap: i32,
    pub long_term_frame_idx: u32,
    pub reference: RefState,
    pub output_needed: bool,
    /// A frame inferred by the frame_num gap process (8.2.5.2): never output.
    pub non_existing: bool,
    pub idr: bool,
    pub mmco5: bool,
    /// The caller's tag (a presentation time) for the access unit the picture came from.
    pub tag: i64,
    /// Cropping rectangle (x, y, width, height).
    pub crop: (u32, u32, u32, u32),
    pub full_range: bool,
}

impl Picture {
    pub fn new(mb_w: usize, mb_h: usize, id: i32) -> Picture {
        let (w, h) = (mb_w * 16, mb_h * 16);
        Picture {
            planes: [Plane::new(w, h, 0), Plane::new(w / 2, h / 2, 128), Plane::new(w / 2, h / 2, 128)],
            mb_w,
            mb_h,
            motion: Motion::new(mb_w, mb_h),
            id,
            top_poc: 0,
            bottom_poc: 0,
            poc: 0,
            frame_num: 0,
            frame_num_wrap: 0,
            long_term_frame_idx: 0,
            reference: RefState::Unused,
            output_needed: false,
            non_existing: false,
            idr: false,
            mmco5: false,
            tag: 0,
            crop: (0, 0, w as u32, h as u32),
            full_range: false,
        }
    }

    pub fn is_ref(&self) -> bool {
        self.reference != RefState::Unused
    }
    pub fn is_long(&self) -> bool {
        self.reference == RefState::Long
    }
}

/// Per-picture decoding state that does not outlive the picture.
pub struct PicState {
    pub info: Vec<MbInfo>,
    /// Intra4x4PredMode / Intra8x8PredMode per 4x4 block (2 = DC for any other macroblock).
    pub intra_modes: Vec<i8>,
    /// Absolute mvd per 4x4 block, [list][component], saturated (9.3.3.1.1.7 only compares the
    /// sum with 3 and 32).
    pub mvd: Vec<[[u8; 2]; 2]>,
}

impl PicState {
    pub fn new(mb_w: usize, mb_h: usize) -> PicState {
        let n = mb_w * mb_h * 16;
        PicState { info: vec![MbInfo::default(); mb_w * mb_h], intra_modes: vec![2; n], mvd: vec![[[0; 2]; 2]; n] }
    }
}
