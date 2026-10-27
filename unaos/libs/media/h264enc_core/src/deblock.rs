// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENC2 (LEDGER SR211): the in-loop deblocking filter (8.7) over the encoder's reconstruction.
//!
//! CHARTER: Stria (A/V) — seam `shared-core`; the facts h264_core's `deblock::deblock_picture` reads, recorded per macroblock.
//!
//! The filter is h264_core's own, called as the decoder calls it once a picture is decoded: this
//! module only records, per macroblock as it is coded, the facts that fn reads — `MbInfo` (slice,
//! kind, QPY, transform size, the non-zero map of the luma 4x4 blocks) and the per-4x4 `Motion`
//! (mvL0, refIdxL0, the reference's id) — in the decoder's shape. With [`crate::Config::deblock`]
//! off both fns return at once and the slices carry disable_deblocking_filter_idc 1.
//!
//! H264ENCCABAC (SR310): the same `MbInfo` grid is the CABAC writer's neighbour record, as the
//! decoder's one `info` grid serves both its contexts and its filter: under
//! [`crate::Entropy::Cabac`] `note_mb` runs with the filter off too and also records
//! `chroma_mode` and `cbf` (coded_block_flag per block, h264_core's `CBF_*` bit layout).
//!
//! H264ENC8X8 (SR322): `t8x8` (the filter's transform_size_8x8 edge rule: h264_core's
//! `deblock_picture` skips the odd internal edges of such a macroblock), kind I8x8, and an 8x8's
//! level marking all four of its 4x4s in `nz` and `cbf`, as h264_core's `parse_residual` records them.
//!
//! H264ENCBFRAMES (SR333): the B kinds (BSkip, BDirect, Inter) and both lists' motion per 4x4: refIdx
//! per list and the reference's id, 0 the past anchor (L0) and 1 the future one (L1); h264_core's
//! `boundary_strength` applies the bi-predicted / mixed-reference rules over those ids itself.

use h264_core::deblock::{deblock_picture, SliceDeblock};
use h264_core::picture::{MbKind, Mv, BLK_X, BLK_Y, CBF_CHROMA_AC, CBF_CHROMA_DC, CBF_LUMA_DC};

use crate::{Encoder, Entropy, Kind, MbCode};

impl Encoder {
    /// Record the deblocking facts of the coded macroblock (mbx, mby) (its motion is already in
    /// `self.motion` / `self.motion1`).
    pub(crate) fn note_mb(&mut self, mbx: usize, mby: usize, code: &MbCode) {
        if !self.cfg.deblock && self.cfg.entropy == Entropy::Cavlc {
            return;
        }
        let i = mby * self.mb_w + mbx;
        let kind = match code.kind {
            Kind::I4 => {
                if code.t8x8 { MbKind::I8x8 } else { MbKind::I4x4 }
            }
            Kind::I16 => MbKind::I16x16,
            Kind::PSkip => MbKind::PSkip,
            Kind::P16 | Kind::BL0 | Kind::BL1 | Kind::BBi => MbKind::Inter,
            Kind::BSkip => MbKind::BSkip,
            Kind::BDirect => MbKind::BDirect,
        };
        let intra = kind.is_intra();
        // The decoder's nz: TotalCoeff > 0 per luma 4x4 block (by luma4x4BlkIdx). Intra edges are
        // bS 3 / 4 before nz is read, and a P_Skip / B_Skip has no levels.
        let mut nz = 0u16;
        if code.kind.inter_coded() {
            for blk in 0..16 {
                if luma_coded(code, blk) {
                    nz |= 1 << blk;
                }
            }
        }
        let info = &mut self.dinfo[i];
        info.slice = 0;
        info.kind = kind;
        info.cbp = code.cbp;
        info.t8x8 = code.t8x8;
        info.qp = (if code.cbp != 0 || code.kind == Kind::I16 { self.qp } else { self.qp_pred }) as u8; // QPY: a MB without mb_qp_delta keeps QPY,pred
        info.nz = nz;
        info.chroma_mode = code.chroma_mode;
        info.cbf = cbf_bits(code);
        let (m, m1) = (self.motion[i], self.motion1[i]);
        let w4 = self.dmo.w4;
        for blk in 0..16 {
            let g = (mby * 4 + BLK_Y[blk]) * w4 + mbx * 4 + BLK_X[blk];
            if intra {
                self.dmo.mv[g] = [Mv::ZERO; 2];
                self.dmo.ref_idx[g] = [-1, -1];
                self.dmo.ref_id[g] = [-1, -1];
            } else {
                // A P macroblock: [mvL0, 0], [0, -1], [0, -1] (motion1 is NONE outside B pictures).
                let used = |r: i32| r >= 0;
                self.dmo.mv[g] = [if used(m.r) { m.mv } else { Mv::ZERO }, if used(m1.r) { m1.mv } else { Mv::ZERO }];
                self.dmo.ref_idx[g] = [m.r.max(-1) as i8, m1.r.max(-1) as i8];
                self.dmo.ref_id[g] = [if used(m.r) { 0 } else { -1 }, if used(m1.r) { 1 } else { -1 }];
            }
        }
        self.dmo.mb_intra[i] = intra;
    }

    /// Filter the finished reconstruction with h264_core's deblocking filter (one slice,
    /// FilterOffsetA / B 0, chroma_qp_index_offset 0), before it is cropped and becomes the
    /// reference.
    pub(crate) fn deblock(&mut self) {
        if !self.cfg.deblock {
            return;
        }
        let slices = [SliceDeblock { disable: 0, alpha_offset: 0, beta_offset: 0 }];
        deblock_picture(&mut self.rec, &self.dmo, &self.dinfo, &slices, self.mb_w, self.mb_h, [0, 0]);
    }
}

/// coded_block_flag of every block of a coded macroblock as h264_core's `parse_residual` records it
/// in `MbInfo::cbf`: set where the block is coded (by coded_block_pattern) and has a non-zero level,
/// which is exactly where the CABAC writer writes coded_block_flag 1.
pub(crate) fn cbf_bits(code: &MbCode) -> u32 {
    let mut cbf = 0u32;
    for blk in 0..16 {
        if (code.cbp >> (blk / 4)) & 1 == 1 && luma_coded(code, blk) {
            cbf |= 1 << blk;
        }
    }
    if code.kind == Kind::I16 && code.dc.iter().any(|&v| v != 0) {
        cbf |= 1 << CBF_LUMA_DC;
    }
    let cc = code.cbp >> 4;
    for c in 0..2 {
        if cc != 0 && code.cdc[c].iter().any(|&v| v != 0) {
            cbf |= 1 << (CBF_CHROMA_DC as usize + c);
        }
        for blk in 0..4 {
            if cc == 2 && code.cac[c][blk].iter().any(|&v| v != 0) {
                cbf |= 1 << (CBF_CHROMA_AC as usize + c * 4 + blk);
            }
        }
    }
    cbf
}

/// Does luma 4x4 block `blk` carry a level: its own, or under the 8x8 transform its 8x8's (SR322)?
fn luma_coded(code: &MbCode, blk: usize) -> bool {
    if code.t8x8 { code.luma8[blk / 4].iter().any(|&v| v != 0) } else { code.luma[blk].iter().any(|&v| v != 0) }
}
