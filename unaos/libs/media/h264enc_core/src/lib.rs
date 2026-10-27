// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENC (LEDGER SR197): an H.264 / AVC encoder (ITU-T Rec. H.264), `no_std` + `alloc`, whose
//! one dependency is h264_core — the editor's export leg (the NLE line's first encoder).
//!
//! CHARTER: Stria (A/V) — seam `shared-core` (a `no_std` lib both rings link, beside h264_core).
//!
//! What it writes: Constrained Baseline (profile_idc 66, constraint_set0/1), CAVLC, or Main
//! (profile_idc 77, constraint_set1) with CABAC ([`Config::entropy`], H264ENCCABAC SR310), frames only,
//! one slice per picture, constant QP (16..=40) or rate-controlled ([`Config::rate`], H264ENCRC SR218). An IDR every `gop` frames, P frames between them
//! against the previous reconstruction (one reference frame). Macroblocks: Intra_4x4 and
//! Intra_16x16 (the SAD-cheapest mode of each, chroma likewise) in I slices; P_Skip and
//! P_L0_16x16 by an integer-sample full search in a +-8 window in P slices, optionally refined to
//! half / quarter samples (H264ENC2, SR211). With [`Config::deblock`] the slices carry
//! disable_deblocking_filter_idc 0 and the reconstruction passes h264_core's in-loop filter
//! before it becomes the reference (else idc 1, unfiltered). Output as Annex B byte stream or as
//! avcC samples with the AVCDecoderConfigurationRecord ([`Encoder::avcc`]). With [`Config::bframes`]
//! (H264ENCBFRAMES SR333) up to 4 B pictures sit between anchors: Main profile, POC type 0, two
//! reference frames (L0 the past anchor, L1 the future one), B_Skip / B_Direct_16x16 (spatial direct) /
//! B_L0 / B_L1 / B_Bi_16x16, coded in decoding order through [`Encoder::push`] / [`Encoder::flush`].
//!
//! | [`Config`] field | range         | default | effect                                                  |
//! |------------------|---------------|---------|---------------------------------------------------------|
//! | `width`, `height`| even, 2..=16384 | (set) | the picture size; cropped from whole macroblocks        |
//! | `qp`             | 16..=40       | (set)   | constant QPY of every macroblock                        |
//! | `gop`            | >= 1          | (set)   | an IDR every `gop` frames                               |
//! | `deblock`        | bool          | false   | idc 0 + h264_core's `deblock_picture` over the recon    |
//! | `subpel`         | 0 / 1 / 2     | 0       | motion refinement: integer / half / quarter (>2 = 2)    |
//! | `rate`           | `Option<RateControl>` | None | CBR / VBR: `qp` starts, [`rc`] picks each picture's and row's QP |
//! | `entropy`        | [`Entropy`]   | Cavlc   | Cabac: Main profile, entropy_coding_mode_flag 1, [`cabac`] writes the slice data |
//! | `transform8x8`   | bool          | false   | High (profile_idc 100), transform_8x8_mode_flag: Intra_8x8 in I macroblocks (SR322) |
//! | `bframes`        | 0..=4         | 0       | B pictures between anchors (SR333): push / flush, VUI reorder 1 (>4 = 4) |
//!
//! | module          | Recommendation                                                          |
//! |-----------------|-------------------------------------------------------------------------|
//! | [`nal`]         | 7.2 RBSP writing, 9.1 ue / se, 7.3.1 NAL units, emulation prevention, avcC |
//! | [`sps`]         | 7.3.2.1 SPS (+ VUI E.1.1), 7.3.2.2 PPS, 7.3.3 slice header                |
//! | [`transform`]   | 8.5 forward: 4x4 / 8x8 core transforms, DC Hadamards, quantisers, SATD   |
//! | [`intra`]       | 8.3 mode decision over h264_core's predictors, 8.3.1.1 mode coding       |
//! | [`cavlc`]       | 9.2 residual_block_cavlc written, 9.2.1 nC                               |
//! | [`cabac`]       | 9.3.4 the arithmetic encoder, 9.3.2 binarizations, 9.3.3.1 ctxIdxInc      |
//! | [`me`]          | 8.4.1 MV prediction and P_Skip, integer search + sub-sample refinement   |
//! | [`yuv`]         | RGBA -> I420 input, BT.601 limited range (Rec. ITU-R BT.601), NLE5B      |
//! | [`deblock`]     | 8.7 the facts h264_core's deblocking filter reads, recorded per MB       |
//! | [`rc`]          | rate control: picture QP (slice_qp_delta), row QP (7.4.5 mb_qp_delta)   |
//!
//! Shared, never copied: h264_core's `transform` (`LevelScale`, `scale4`, `luma_dc`, `chroma_dc`,
//! `idct4_add`) is the encoder's reconstruction; its `intra` (`pred_nxn`, `pred_16x16`,
//! `pred_chroma`, `Edges`) its prediction; its `tables` (coeff_token, total_zeros, run_before,
//! coded_block_pattern, QPc, zig-zag) its code words; its `inter` (`luma_mc`, `chroma_mc`) and
//! `picture` (`Plane`, `Mv`) its motion compensation; its `deblock` (`deblock_picture`) its in-loop
//! filter. The reconstruction loop is the decoder's
//! arithmetic in the decoder's order, so `h264_core::Decoder` decodes the output to exactly
//! [`Encoder::reconstruction`] (the KAT). The neighbour rules (availability, predicted intra mode,
//! nC, mvp) are decoder-private methods there, so they are mirrored here for one slice per
//! picture; making them shared free fns is owed (H264ENC-NBSHARE).
//!
//! Owed (handlers.md §5): two-pass / scene cuts / activity masking (inside H264ENC-RC), scaling matrices (inside H264ENC-CABAC; High's 8x8 transform is SR322's), the
//! 16x8 / 8x16 / 8x8 partitions (inside H264ENC-SUBPEL), per-slice filter offsets (inside
//! H264ENC-DEBLOCK), pyramid / temporal-direct / weighted B (inside H264ENC-BFRAMES), hardware encode (H264ENC-HW).

#![no_std]
#![forbid(unsafe_code)]
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

extern crate alloc;

pub mod cabac;
pub mod cavlc;
pub mod deblock;
pub mod intra;
pub mod me;
pub mod nal;
pub mod rc;
pub mod sps;
pub mod transform;
pub mod yuv;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use h264_core::picture::{MbInfo, Motion, Plane, BLK_X, BLK_Y};
use h264_core::ps::ScalingMatrices;
use h264_core::tables::{CBP_INTER, CBP_INTRA, QPC, ZIGZAG4, ZIGZAG8};
use h264_core::transform::LevelScale;

use crate::cavlc::Nnz;
use crate::me::MbMotion;
use crate::nal::BitWriter;
pub use crate::rc::{RateControl, RcMode};
use crate::sps::SeqParams;

/// Every way the encoder refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Width or height zero, odd (4:2:0 cropping is in units of 2) or above 16384.
    Geometry,
    /// [`Encoder::encode`] under [`Config::bframes`] above 0: a reordered stream is coded through
    /// [`Encoder::push`] / [`Encoder::flush`] (SR333).
    Reorder,
    /// QP outside 16..=40.
    Qp,
    /// A GOP length of 0.
    Gop,
    /// A frame whose size or planes do not match the configuration.
    FrameSize,
    /// A rate control with a zero bit rate or frame rate, or a VBR max below its mean.
    Rate,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Error::Geometry => "h264enc: width / height not even and in 2..=16384",
            Error::Qp => "h264enc: QP outside 16..=40",
            Error::Gop => "h264enc: GOP length 0",
            Error::FrameSize => "h264enc: frame does not match the configured size",
            Error::Rate => "h264enc: rate control with a zero bit rate / frame rate, or a VBR max below its mean",
            Error::Reorder => "h264enc: B pictures are reordered: code through push / flush, not encode",
        })
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// The stream's fixed parameters. `width`, `height`, `qp` and `gop` must be set; the coding tools
/// after them default off (`..Default::default()` keeps SR197's bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Config {
    pub width: u32,
    pub height: u32,
    /// Constant QP (QPY) of every macroblock, 16..=40.
    pub qp: u8,
    /// An IDR picture every `gop` frames (1 = every frame intra).
    pub gop: u32,
    /// The in-loop deblocking filter (8.7): slices carry disable_deblocking_filter_idc 0 and the
    /// reconstruction is filtered before it becomes the reference. false = idc 1, unfiltered.
    pub deblock: bool,
    /// Motion refinement after the integer search: 0 integer, 1 half, 2 quarter samples (above 2
    /// is read as 2).
    pub subpel: u8,
    /// Rate control (H264ENCRC, SR218): None codes every macroblock at `qp`; Some starts at `qp`
    /// and [`rc`] chooses each picture's QP from the bit budget and each macroblock row's from the
    /// running bit count.
    pub rate: Option<RateControl>,
    /// The entropy coder (H264ENCCABAC, SR310): Cavlc (Constrained Baseline, SR197's bytes) or
    /// Cabac (Main profile).
    pub entropy: Entropy,
    /// The 8x8 transform (H264ENC8X8, SR322): High profile (profile_idc 100) with
    /// transform_8x8_mode_flag 1 under either entropy coder; I macroblocks may take Intra_8x8 (chosen
    /// against Intra_4x4 by SATD). P macroblocks keep the 4x4 transform (transform_size_8x8_flag 0;
    /// their 8x8 choice by real bits is owed). false keeps every pinned byte.
    pub transform8x8: bool,
    /// B pictures between anchors (H264ENCBFRAMES, SR333): 0 keeps every pinned byte (and [`Encoder::encode`]);
    /// 1..=4 (above 4 read as 4) codes Main profile with POC type 0 through [`Encoder::push`] / [`Encoder::flush`].
    pub bframes: u8,
}

/// The coding type of one picture: an IDR (I slice), a P anchor, or a non-reference B picture (SR333).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pic {
    Idr,
    P,
    B,
}

/// SliceQPY of a B picture above its picture QP under constant QP (SR333): slice_qp_delta 2.
pub const B_QP_OFFSET: i32 = 2;

/// entropy_coding_mode_flag (7.4.2.2): CAVLC (9.2) or CABAC (9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Entropy {
    #[default]
    Cavlc,
    Cabac,
}

/// One 8-bit 4:2:0 planar input frame, borrowed: the field names of gneiss_pal's
/// `dsp::video::Pixels::I420` plus the frame size.
#[derive(Debug, Clone, Copy)]
pub struct Yuv420<'a> {
    pub width: u32,
    pub height: u32,
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub y_stride: usize,
    pub uv_stride: usize,
}

/// One encoded access unit: its NAL units in decoding order (header byte first, no start code;
/// an IDR's carries its SPS and PPS ahead of the slice).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    /// An IDR picture (the container's sync sample).
    pub keyframe: bool,
    pub nals: Vec<Vec<u8>>,
    /// Presentation time in frames (SR333): the frame index, or under [`Config::bframes`] the display
    /// index + 1 (the one-picture reorder delay, so `dts <= pts`).
    pub pts: u64,
    /// Decoding time in frames: the access unit's index in decoding order.
    pub dts: u64,
}

impl Access {
    /// The Annex B byte stream form (B.1): each NAL unit after a 4-byte start code.
    pub fn annexb(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.nals.iter().map(|n| n.len() + 4).sum());
        for n in &self.nals {
            v.extend_from_slice(&[0, 0, 0, 1]);
            v.extend_from_slice(n);
        }
        v
    }

    /// The avcC sample form (ISO/IEC 14496-15 §5.3.2): the VCL NAL units each after its 4-byte
    /// big-endian length; the parameter sets live in [`Encoder::avcc`].
    pub fn avcc_sample(&self) -> Vec<u8> {
        let mut v = Vec::new();
        for n in self.nals.iter().filter(|n| matches!(n[0] & 0x1f, nal::NAL_SLICE | nal::NAL_IDR)) {
            v.extend_from_slice(&(n.len() as u32).to_be_bytes());
            v.extend_from_slice(n);
        }
        v
    }
}

/// The intra 4x4 penalty in bits: Intra_4x4 is chosen over Intra_16x16 only when its SAD beats
/// Intra_16x16's by more than this many bits' worth ([`lambda`]) — the 16 mode fields it spends.
pub const I4_BIAS: u32 = 24;

/// The intra 8x8 penalty in bits (SR322): the 4 mode fields Intra_8x8 spends, at [`I4_BIAS`]'s rate.
pub const I8_BIAS: u32 = 6;

/// The SAD cost of one bit at QP `qp`: about 2^((qp - 12) / 6), the growth of the quantiser step.
pub fn lambda(qp: i32) -> u32 {
    1 << ((qp - 12).max(0) / 6)
}

/// Macroblock types the encoder writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    I4,
    I16,
    PSkip,
    P16,
    /// B pictures (SR333): B_Skip, B_Direct_16x16 (spatial direct), B_L0_16x16, B_L1_16x16, B_Bi_16x16.
    BSkip,
    BDirect,
    BL0,
    BL1,
    BBi,
}

impl Kind {
    /// An inter macroblock with mb_type and residual syntax (not skipped, not intra).
    pub(crate) fn inter_coded(self) -> bool {
        matches!(self, Kind::P16 | Kind::BDirect | Kind::BL0 | Kind::BL1 | Kind::BBi)
    }
    /// Carries mvd_l0.
    pub(crate) fn l0(self) -> bool {
        matches!(self, Kind::P16 | Kind::BL0 | Kind::BBi)
    }
    /// Carries mvd_l1.
    pub(crate) fn l1(self) -> bool {
        matches!(self, Kind::BL1 | Kind::BBi)
    }
}

/// The decided syntax of one macroblock: modes, motion and levels (raster within each block, as
/// h264_core's `mb::Mb` holds them).
pub(crate) struct MbCode {
    pub kind: Kind,
    /// Intra4x4PredMode by luma4x4BlkIdx, or Intra8x8PredMode in [0..4] when `t8x8`.
    pub modes: [u8; 16],
    pub i16_mode: u8,
    pub chroma_mode: u8,
    /// mvd_l0 of P_L0_16x16 / B_L0 / B_Bi (quarter samples).
    pub mvd: [i32; 2],
    /// mvd_l1 of B_L1_16x16 / B_Bi_16x16 (SR333).
    pub mvd1: [i32; 2],
    /// CodedBlockPatternLuma | CodedBlockPatternChroma << 4.
    pub cbp: u8,
    /// Intra16x16DCLevel (raster 4x4).
    pub dc: [i32; 16],
    /// 4x4 blocks by luma4x4BlkIdx (Intra_16x16: AC, [0] = 0).
    pub luma: [[i32; 16]; 16],
    /// ChromaDCLevel per iCbCr (c0..c3).
    pub cdc: [[i32; 4]; 2],
    /// ChromaACLevel per iCbCr and chroma4x4BlkIdx (raster, [0] = 0).
    pub cac: [[[i32; 16]; 4]; 2],
    /// transform_size_8x8_flag (SR322): the luma levels are `luma8` (an I4 kind is then I_NxN coded
    /// Intra_8x8).
    pub t8x8: bool,
    /// The 8x8 luma blocks by luma8x8BlkIdx (raster) when `t8x8`.
    pub luma8: [[i32; 64]; 4],
}

impl MbCode {
    pub fn new(kind: Kind) -> MbCode {
        MbCode { kind, modes: [2; 16], i16_mode: 0, chroma_mode: 0, mvd: [0; 2], mvd1: [0; 2], cbp: 0, dc: [0; 16], luma: [[0; 16]; 16], cdc: [[0; 4]; 2], cac: [[[0; 16]; 4]; 2], t8x8: false, luma8: [[0; 64]; 4] }
    }

    /// CodedBlockPatternLuma: bit b8 when a block of that 8x8 (or the 8x8 itself, `t8x8`) has a level.
    pub fn luma_cbp(&self) -> u8 {
        let mut cbp = 0u8;
        for blk in 0..16 {
            let coded = if self.t8x8 { self.luma8[blk / 4].iter().any(|&v| v != 0) } else { self.luma[blk].iter().any(|&v| v != 0) };
            if coded {
                cbp |= 1 << (blk / 4);
            }
        }
        cbp
    }

    /// CodedBlockPatternChroma: 2 with an AC level, 1 with only DC levels, else 0.
    pub fn chroma_cbp(&self) -> u8 {
        if self.cac.iter().flatten().flatten().any(|&v| v != 0) {
            2
        } else if self.cdc.iter().flatten().any(|&v| v != 0) {
            1
        } else {
            0
        }
    }
}

/// An H.264 encoder for one stream. Every piece of state (the padded source, the reconstruction
/// and its reference, the per-block neighbour grids) is owned here and changed only through
/// `&mut self`.
pub struct Encoder {
    pub(crate) cfg: Config,
    pub(crate) mb_w: usize,
    pub(crate) mb_h: usize,
    /// QPY of the macroblocks being coded (the constant QP without rate control).
    pub(crate) qp: i32,
    /// QPc of `qp` (chroma_qp_index_offset 0, Table 8-15).
    pub(crate) qpc: i32,
    /// QPY,pred (7.4.5): the QPY of the previous macroblock of the slice, as h264_core's decoder
    /// tracks it (moved only by a written mb_qp_delta).
    pub(crate) qp_pred: i32,
    /// The rate controller ([`Config::rate`]).
    pub(crate) rc: Option<rc::Rc>,
    /// SliceQPY of the last encoded picture.
    last_qp: i32,
    log2_max_frame_num: u32,
    sps: Vec<u8>,
    pps: Vec<u8>,
    pub(crate) ls: Box<LevelScale>,
    /// The input, padded to whole macroblocks by edge replication.
    pub(crate) src: [Plane; 3],
    /// The picture being reconstructed.
    pub(crate) rec: [Plane; 3],
    /// The previous reconstruction: the reference of a P picture (under `bframes` the past anchor, L0).
    pub(crate) refp: [Plane; 3],
    /// The future anchor, a B picture's L1 reference (SR333; unused without `bframes`).
    pub(crate) fwd: [Plane; 3],
    crop: [Vec<u8>; 3],
    pub(crate) nnz: Nnz,
    /// Intra4x4PredMode per 4x4 block of the picture (2 for any block not coded Intra_4x4).
    pub(crate) modes: Vec<u8>,
    /// Motion per macroblock of the current P picture (list 0 of a B picture).
    pub(crate) motion: Vec<MbMotion>,
    /// List-1 motion per macroblock of the current B picture (SR333).
    pub(crate) motion1: Vec<MbMotion>,
    /// The last anchor's motion: the co-located macroblocks of spatial direct (NONE after an IDR).
    pub(crate) col: Vec<MbMotion>,
    /// The deblocking filter's per-macroblock facts of the current picture ([`deblock`]).
    pub(crate) dinfo: Vec<MbInfo>,
    /// The deblocking filter's per-4x4 motion of the current picture ([`deblock`]).
    pub(crate) dmo: Motion,
    frames: u64,
    idr_count: u32,
    frame_num: u32,
    /// The padded sources held for B coding, with their display indices (SR333).
    ahead: Vec<([Plane; 3], u64)>,
    /// The cropped reconstruction of each access unit the last push / flush returned.
    recons: Vec<[Vec<u8>; 3]>,
    /// Access units coded so far (the next one's dts).
    coded: u64,
    /// Display index of the last IDR (POC 0).
    idr_display: u64,
    /// log2_max_pic_order_cnt_lsb (POC type 0, under `bframes` only).
    log2_max_poc_lsb: u32,
}

impl Encoder {
    pub fn new(cfg: Config) -> Result<Encoder> {
        if cfg.width == 0 || cfg.height == 0 || cfg.width % 2 != 0 || cfg.height % 2 != 0 || cfg.width > 16384 || cfg.height > 16384 {
            return Err(Error::Geometry);
        }
        if !(16..=40).contains(&cfg.qp) {
            return Err(Error::Qp);
        }
        if cfg.gop == 0 {
            return Err(Error::Gop);
        }
        let rc = match cfg.rate {
            Some(r) => Some(rc::Rc::new(r, cfg.gop, cfg.qp as i32)?.with_bframes(cfg.bframes.min(4))),
            None => None,
        };
        let (mb_w, mb_h) = (cfg.width.div_ceil(16) as usize, cfg.height.div_ceil(16) as usize);
        let qp = cfg.qp as i32;
        let mut log2_max_frame_num = 4;
        while log2_max_frame_num < 16 && (1u64 << log2_max_frame_num) < cfg.gop as u64 {
            log2_max_frame_num += 1;
        }
        let mut log2_max_poc_lsb = 4;
        while log2_max_poc_lsb < 16 && (1u64 << log2_max_poc_lsb) < 4 * cfg.gop as u64 {
            log2_max_poc_lsb += 1;
        }
        let cabac = cfg.entropy == Entropy::Cabac;
        let high = cfg.transform8x8;
        let reorder = cfg.bframes > 0;
        let p = SeqParams { width: cfg.width, height: cfg.height, mb_w: mb_w as u32, mb_h: mb_h as u32, log2_max_frame_num, qp, cabac, high, reorder, log2_max_poc_lsb };
        let sps = nal::nal_unit(3, nal::NAL_SPS, &sps::write_sps(&p));
        let pps = nal::nal_unit(3, nal::NAL_PPS, &sps::write_pps(qp, cabac, high));
        let planes = || [Plane::new(mb_w * 16, mb_h * 16, 0), Plane::new(mb_w * 8, mb_h * 8, 128), Plane::new(mb_w * 8, mb_h * 8, 128)];
        let (w, h) = (cfg.width as usize, cfg.height as usize);
        Ok(Encoder {
            cfg,
            mb_w,
            mb_h,
            qp,
            qpc: QPC[qp as usize] as i32,
            qp_pred: qp,
            rc,
            last_qp: qp,
            log2_max_frame_num,
            sps,
            pps,
            ls: Box::new(LevelScale::new(&ScalingMatrices::FLAT)),
            src: planes(),
            rec: planes(),
            refp: planes(),
            fwd: planes(),
            crop: [vec![0; w * h], vec![128; w * h / 4], vec![128; w * h / 4]],
            nnz: Nnz::new(mb_w, mb_h),
            modes: vec![2; mb_w * mb_h * 16],
            motion: vec![MbMotion::NONE; mb_w * mb_h],
            motion1: vec![MbMotion::NONE; mb_w * mb_h],
            col: vec![MbMotion::NONE; mb_w * mb_h],
            dinfo: vec![MbInfo::default(); mb_w * mb_h],
            dmo: Motion::new(mb_w, mb_h),
            frames: 0,
            idr_count: 0,
            frame_num: 0,
            ahead: Vec::new(),
            recons: Vec::new(),
            coded: 0,
            idr_display: 0,
            log2_max_poc_lsb,
        })
    }

    pub fn config(&self) -> Config {
        self.cfg
    }

    /// SliceQPY of the last encoded picture (the constant QP without rate control).
    pub fn picture_qp(&self) -> u8 {
        self.last_qp as u8
    }

    /// The SPS NAL unit (header byte first).
    pub fn sps_nal(&self) -> &[u8] {
        &self.sps
    }

    /// The PPS NAL unit (header byte first).
    pub fn pps_nal(&self) -> &[u8] {
        &self.pps
    }

    /// The AVCDecoderConfigurationRecord of the stream (MP4 `avcC`, Matroska CodecPrivate).
    pub fn avcc(&self) -> Vec<u8> {
        nal::avcc_record(&self.sps, &self.pps)
    }

    /// The reconstruction of the last encoded frame, cropped to the configured size: Y, Cb, Cr,
    /// row-major without padding — what a decoder outputs for it.
    pub fn reconstruction(&self) -> [&[u8]; 3] {
        [&self.crop[0], &self.crop[1], &self.crop[2]]
    }

    /// The cropped reconstruction of each access unit the last [`Encoder::push`] / [`Encoder::flush`]
    /// returned, in the same (decoding) order (SR333).
    pub fn reconstructions(&self) -> &[[Vec<u8>; 3]] {
        &self.recons
    }

    /// Encode the next frame: an IDR when the frame index is a multiple of the GOP, else a P frame.
    /// Under [`Config::bframes`] above 0 it refuses ([`Error::Reorder`]): see [`Encoder::push`].
    pub fn encode(&mut self, f: &Yuv420) -> Result<Access> {
        if self.cfg.bframes > 0 {
            return Err(Error::Reorder);
        }
        self.load(f)?;
        let idr = self.frames.is_multiple_of(self.cfg.gop as u64);
        let au = self.code(if idr { Pic::Idr } else { Pic::P }, self.frames);
        self.frames += 1;
        Ok(au)
    }

    /// Take the next frame in output order (SR333) and return the access units it releases, in decoding
    /// order. Without `bframes` that is [`Encoder::encode`]'s one unit. With `bframes`, display index
    /// k = frame % gop: k 0 is an IDR; k a multiple of bframes + 1, or the last frame before the next IDR,
    /// is a P anchor, coded at once and followed by the B pictures held since the previous anchor; any
    /// other frame is held (no units). [`Encoder::reconstructions`] holds the units' reconstructions.
    pub fn push(&mut self, f: &Yuv420) -> Result<Vec<Access>> {
        self.recons.clear();
        if self.cfg.bframes == 0 {
            let au = self.encode(f)?;
            self.recons.push(self.crop.clone());
            return Ok(vec![au]);
        }
        self.load(f)?;
        let (b, gop) = (self.cfg.bframes.min(4) as u64, self.cfg.gop as u64);
        let (display, k) = (self.frames, self.frames % gop);
        self.frames += 1;
        if k != 0 && k % (b + 1) != 0 && k + 1 != gop {
            self.ahead.push((self.src.clone(), display));
            return Ok(Vec::new());
        }
        Ok(self.group(if k == 0 { Pic::Idr } else { Pic::P }, display))
    }

    /// Drain the held frames at the end of the stream (SR333): the last becomes a P anchor, the rest B.
    pub fn flush(&mut self) -> Result<Vec<Access>> {
        self.recons.clear();
        let Some((planes, display)) = self.ahead.pop() else { return Ok(Vec::new()) };
        self.src = planes;
        Ok(self.group(Pic::P, display))
    }

    /// Code the anchor in `src`, then each held frame as a B picture between the previous anchor (L0)
    /// and it (L1); the new anchor then becomes L0.
    fn group(&mut self, pic: Pic, display: u64) -> Vec<Access> {
        let mut out = vec![self.code(pic, display)];
        self.recons.push(self.crop.clone());
        for (planes, d) in core::mem::take(&mut self.ahead) {
            self.src = planes;
            out.push(self.code(Pic::B, d));
            self.recons.push(self.crop.clone());
        }
        core::mem::swap(&mut self.refp, &mut self.fwd);
        out
    }

    /// Code the picture in `src` as `pic` at display index `display`: one slice, its reconstruction
    /// and (an anchor) the reference update.
    fn code(&mut self, pic: Pic, display: u64) -> Access {
        let idr = pic == Pic::Idr;
        let bpic = pic == Pic::B;
        // frame_num: PrevRefFrameNum + 1; a B picture (nal_ref_idc 0) does not move PrevRefFrameNum.
        let frame_num = if idr { 0 } else { (self.frame_num + 1) % (1 << self.log2_max_frame_num) };
        if !bpic {
            self.frame_num = frame_num;
        }
        if idr {
            self.idr_display = display;
        }
        let slice_qp = self.rc_picture(pic);
        self.set_qp(slice_qp);
        self.qp_pred = slice_qp;
        self.nnz.reset();
        self.modes.fill(2);
        self.motion.fill(MbMotion::NONE);
        self.motion1.fill(MbMotion::NONE);
        let mut w = BitWriter::new();
        let is_cabac = self.cfg.entropy == Entropy::Cabac;
        let poc = (self.cfg.bframes > 0).then(|| ((2 * (display - self.idr_display) % (1u64 << self.log2_max_poc_lsb)) as u32, self.log2_max_poc_lsb));
        sps::write_slice_header(&mut w, pic, frame_num, self.log2_max_frame_num, self.idr_count & 0xffff, poc, self.cfg.deblock, slice_qp - self.cfg.qp as i32, is_cabac);
        let hdr = w.bits();
        // CABAC (SR310): the slice data's writer, contexts for I (column 0) or P / B with cabac_init_idc 0.
        let mut cw = is_cabac.then(|| cabac::Writer::new(&mut w, if idr { 0 } else { 1 }, slice_qp, self.mb_w * self.mb_h));
        let mut run = 0u32;
        for mby in 0..self.mb_h {
            self.rc_row(mby, w.bits() + cw.as_ref().map_or(0, cabac::Writer::pending) - hdr);
            for mbx in 0..self.mb_w {
                if idr {
                    let code = self.intra_mb(mbx, mby);
                    self.note_mb(mbx, mby, &code);
                    match &mut cw {
                        Some(c) => self.write_mb_cabac(&mut w, c, mbx, mby, &code, pic),
                        None => self.write_mb(&mut w, mbx, mby, &code, false),
                    }
                } else {
                    let code = if bpic { self.b_mb(mbx, mby) } else { self.inter_mb(mbx, mby) };
                    self.note_mb(mbx, mby, &code);
                    if let Some(c) = &mut cw {
                        self.write_mb_cabac(&mut w, c, mbx, mby, &code, pic);
                    } else if matches!(code.kind, Kind::PSkip | Kind::BSkip) {
                        run += 1;
                    } else {
                        w.ue(run); // mb_skip_run
                        run = 0;
                        self.write_mb(&mut w, mbx, mby, &code, true);
                    }
                }
            }
        }
        if cw.is_some() {
            w.u((8 - w.bits() as u32 % 8) % 8, 0); // rbsp_alignment_zero_bit: the flush wrote the stop bit
        } else {
            if run > 0 {
                w.ue(run);
            }
            w.trailing();
        }
        // A B picture is never a reference: nal_ref_idc 0.
        let slice = nal::nal_unit(if bpic { 0 } else { 3 }, if idr { nal::NAL_IDR } else { nal::NAL_SLICE }, &w.finish());
        let nals = if idr { vec![self.sps.clone(), self.pps.clone(), slice] } else { vec![slice] };
        self.rc_update(nals.iter().map(|n| n.len() as u64 * 8).sum());
        self.last_qp = slice_qp;
        self.deblock();
        self.store_crop();
        if self.cfg.bframes == 0 {
            core::mem::swap(&mut self.rec, &mut self.refp);
        } else if !bpic {
            // The new anchor is the held B pictures' L1 (`group` makes it L0 after them); its motion is
            // their co-located motion.
            core::mem::swap(&mut self.rec, &mut self.fwd);
            if idr {
                self.col.fill(MbMotion::NONE);
            } else {
                self.col.copy_from_slice(&self.motion);
            }
        }
        if idr {
            self.idr_count = self.idr_count.wrapping_add(1);
        }
        let au = Access { keyframe: idr, nals, pts: display + (self.cfg.bframes > 0) as u64, dts: self.coded };
        self.coded += 1;
        au
    }

    /// Copy the input into the macroblock-padded source planes, replicating the last column / row.
    fn load(&mut self, f: &Yuv420) -> Result<()> {
        let (w, h) = (self.cfg.width as usize, self.cfg.height as usize);
        let (cw, ch) = (w / 2, h / 2);
        if f.width != self.cfg.width
            || f.height != self.cfg.height
            || f.y_stride < w
            || f.uv_stride < cw
            || f.y.len() < (h - 1) * f.y_stride + w
            || f.u.len() < (ch - 1) * f.uv_stride + cw
            || f.v.len() < (ch - 1) * f.uv_stride + cw
        {
            return Err(Error::FrameSize);
        }
        let fill = |pl: &mut Plane, data: &[u8], stride: usize, w: usize, h: usize| {
            for y in 0..pl.h {
                let row = &data[y.min(h - 1) * stride..];
                for x in 0..pl.w {
                    pl.data[y * pl.w + x] = row[x.min(w - 1)];
                }
            }
        };
        fill(&mut self.src[0], f.y, f.y_stride, w, h);
        fill(&mut self.src[1], f.u, f.uv_stride, cw, ch);
        fill(&mut self.src[2], f.v, f.uv_stride, cw, ch);
        Ok(())
    }

    fn store_crop(&mut self) {
        let (w, h) = (self.cfg.width as usize, self.cfg.height as usize);
        for c in 0..3 {
            let (pw, ph) = if c == 0 { (w, h) } else { (w / 2, h / 2) };
            let pl = &self.rec[c];
            for y in 0..ph {
                self.crop[c][y * pw..(y + 1) * pw].copy_from_slice(&pl.data[y * pl.w..y * pl.w + pw]);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Shared macroblock helpers
    // -----------------------------------------------------------------------------------------

    /// SAD of the n x n source block at (x, y) of plane `c` against `pred` (row stride n).
    pub(crate) fn sad(&self, c: usize, x: usize, y: usize, n: usize, pred: &[i32]) -> u32 {
        let pl = &self.src[c];
        let mut s = 0u32;
        for i in 0..n {
            for j in 0..n {
                s += (pl.data[(y + i) * pl.w + x + j] as i32 - pred[i * n + j]).unsigned_abs();
            }
        }
        s
    }

    /// source - prediction of the n x n block at (x, y) of plane `c` into `out` (row stride n).
    pub(crate) fn residual(&self, c: usize, x: usize, y: usize, n: usize, pred: &[i32], out: &mut [i32]) {
        let pl = &self.src[c];
        for i in 0..n {
            for j in 0..n {
                out[i * n + j] = pl.data[(y + i) * pl.w + x + j] as i32 - pred[i * n + j];
            }
        }
    }

    /// Write an n x n prediction into the reconstruction of plane `c` at (x, y), clipped to 8 bits
    /// (the decoder's `write_block`).
    pub(crate) fn write_pred(&mut self, c: usize, x: usize, y: usize, n: usize, pred: &[i32]) {
        let pl = &mut self.rec[c];
        for i in 0..n {
            for j in 0..n {
                pl.data[(y + i) * pl.w + x + j] = pred[i * n + j].clamp(0, 255) as u8;
            }
        }
    }

    /// Code and reconstruct the chroma of a macroblock from its two 8x8 predictions: the levels
    /// into `code` (with CodedBlockPatternChroma), the prediction plus residual into the
    /// reconstruction with LevelScale lists 1 / 2 (intra) or 4 / 5 (inter).
    pub(crate) fn chroma_mb(&mut self, mbx: usize, mby: usize, preds: &[[i32; 64]; 2], intra: bool, code: &mut MbCode) {
        let (cx, cy) = (mbx * 8, mby * 8);
        let mut res = [0i32; 64];
        for c in 0..2 {
            self.residual(1 + c, cx, cy, 8, &preds[c], &mut res);
            let (dc, ac) = transform::code_chroma(&res, self.qpc, intra);
            code.cdc[c] = dc;
            code.cac[c] = ac;
        }
        code.cbp = (code.cbp & 15) | (code.chroma_cbp() << 4);
        let cs = self.rec[1].w;
        for c in 0..2 {
            self.write_pred(1 + c, cx, cy, 8, &preds[c]);
            if code.cbp >> 4 != 0 {
                let list = if intra { 1 + c } else { 4 + c };
                transform::recon_chroma(&code.cdc[c], &code.cac[c], &self.ls, list, self.qpc, &mut self.rec[1 + c].data, cy * cs + cx, cs);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // macroblock_layer (7.3.5) and residual (7.3.5.3), CAVLC
    // -----------------------------------------------------------------------------------------

    fn write_mb(&mut self, w: &mut BitWriter, mbx: usize, mby: usize, code: &MbCode, p_slice: bool) {
        let off = if p_slice { 5 } else { 0 }; // intra mb_type follows the 5 P types in a P slice
        match code.kind {
            Kind::I4 => {
                w.ue(off);
                if self.cfg.transform8x8 {
                    w.flag(code.t8x8); // transform_size_8x8_flag
                }
                let w4 = self.mb_w * 4;
                for blk in 0..mode_fields(code.t8x8) {
                    let (bx, by) = mode_at(code.t8x8, blk);
                    let (x4, y4) = (mbx * 4 + bx, mby * 4 + by);
                    let pred = intra::predicted_mode(&self.modes, w4, x4, y4);
                    let m = code.modes[blk];
                    if m == pred {
                        w.flag(true); // prev_intra4x4_pred_mode_flag
                    } else {
                        w.flag(false);
                        w.u(3, if m < pred { m } else { m - 1 } as u32); // rem_intra4x4_pred_mode
                    }
                }
                w.ue(code.chroma_mode as u32);
                w.ue(cbp_code(&CBP_INTRA, code.cbp));
            }
            Kind::I16 => {
                let luma = if code.cbp & 15 != 0 { 12 } else { 0 };
                w.ue(off + 1 + code.i16_mode as u32 + 4 * (code.cbp >> 4) as u32 + luma);
                w.ue(code.chroma_mode as u32);
            }
            Kind::P16 | Kind::BDirect | Kind::BL0 | Kind::BL1 | Kind::BBi => {
                // P_L0_16x16 0 in a P slice; B_Direct_16x16 0, B_L0_16x16 1, B_L1_16x16 2, B_Bi_16x16 3 in a B
                // slice (Table 7-14); ref_idx absent (one active reference per list); every mvd_l0 before mvd_l1.
                w.ue(match code.kind {
                    Kind::BL0 => 1,
                    Kind::BL1 => 2,
                    Kind::BBi => 3,
                    _ => 0,
                });
                if code.kind.l0() {
                    w.se(code.mvd[0]);
                    w.se(code.mvd[1]);
                }
                if code.kind.l1() {
                    w.se(code.mvd1[0]);
                    w.se(code.mvd1[1]);
                }
                w.ue(cbp_code(&CBP_INTER, code.cbp));
                if self.cfg.transform8x8 && code.cbp & 15 != 0 {
                    w.flag(code.t8x8); // transform_size_8x8_flag
                }
            }
            Kind::PSkip | Kind::BSkip => return,
        }
        if code.cbp != 0 || code.kind == Kind::I16 {
            w.se(self.qp - self.qp_pred); // mb_qp_delta (0 at constant QP); QPY,pred moves as h264_core's mb.rs moves it
            self.qp_pred = self.qp;
            self.write_residual(w, mbx, mby, code);
        }
    }

    fn write_residual(&mut self, w: &mut BitWriter, mbx: usize, mby: usize, code: &MbCode) {
        let i16 = code.kind == Kind::I16;
        let (x4m, y4m) = (mbx * 4, mby * 4);
        if i16 {
            let nc = self.nnz.nc_luma(x4m, y4m);
            cavlc::write_block(w, nc, 16, &scan(&code.dc, 0));
        }
        for blk in 0..16 {
            let (x4, y4) = (x4m + BLK_X[blk], y4m + BLK_Y[blk]);
            let mut tc = 0;
            if (code.cbp >> (blk / 4)) & 1 == 1 {
                let nc = self.nnz.nc_luma(x4, y4);
                tc = if code.t8x8 {
                    cavlc::write_block(w, nc, 16, &interleave8(&code.luma8[blk / 4], blk % 4)) // 7.3.5.3.2: 4 interleaved 4x4 lists
                } else if i16 {
                    cavlc::write_block(w, nc, 15, &scan(&code.luma[blk], 1))
                } else {
                    cavlc::write_block(w, nc, 16, &scan(&code.luma[blk], 0))
                };
            }
            self.nnz.set_luma(x4, y4, tc);
        }
        let cc = code.cbp >> 4;
        if cc != 0 {
            for c in 0..2 {
                cavlc::write_block(w, -1, 4, &code.cdc[c]);
            }
        }
        for c in 0..2 {
            for blk in 0..4 {
                let (x2, y2) = (mbx * 2 + blk % 2, mby * 2 + blk / 2);
                let mut tc = 0;
                if cc == 2 {
                    let nc = self.nnz.nc_chroma(c, x2, y2);
                    tc = cavlc::write_block(w, nc, 15, &scan(&code.cac[c][blk], 1));
                }
                self.nnz.set_chroma(c, x2, y2, tc);
            }
        }
    }
}

/// The levels of a raster 4x4 block in zig-zag scan order from scan index `start` (8.5.6 inverted).
fn scan(c: &[i32; 16], start: usize) -> [i32; 16] {
    let mut l = [0i32; 16];
    for k in start..16 {
        l[k - start] = c[ZIGZAG4[k] as usize];
    }
    l
}

/// codeNum of coded_block_pattern `cbp` in a Table 9-4 column.
fn cbp_code(table: &[u8; 48], cbp: u8) -> u32 {
    table.iter().position(|&v| v == cbp).expect("coded_block_pattern outside Table 9-4") as u32
}

/// The number of intra prediction mode fields of an I_NxN macroblock: 16 (Intra_4x4) or 4 (Intra_8x8).
pub(crate) fn mode_fields(t8x8: bool) -> usize {
    if t8x8 { 4 } else { 16 }
}

/// The 4x4 position (x, y) inside the macroblock of mode field `blk`'s top-left 4x4 block: the
/// luma4x4BlkIdx block, or under the 8x8 transform the luma8x8BlkIdx block (whose mode the
/// encoder stores on all four of its 4x4s, as h264_core's decoder does).
pub(crate) fn mode_at(t8x8: bool, blk: usize) -> (usize, usize) {
    if t8x8 { ((blk % 2) * 2, (blk / 2) * 2) } else { (BLK_X[blk], BLK_Y[blk]) }
}

/// The CAVLC list of 4x4 block `sub` of a raster 8x8 block (7.3.5.3.2: lumaLevel8x8[4 * k + sub]
/// over the 8x8 zig-zag of 8.5.7 inverted), as h264_core's `parse_residual` reads it back.
fn interleave8(c: &[i32; 64], sub: usize) -> [i32; 16] {
    let mut l = [0i32; 16];
    for k in 0..16 {
        l[k] = c[ZIGZAG8[4 * k + sub] as usize];
    }
    l
}

/// The 64 levels of a raster 8x8 block in 8x8 zig-zag scan order (CABAC ctxBlockCat 5).
pub(crate) fn scan8(c: &[i32; 64]) -> [i32; 64] {
    let mut l = [0i32; 64];
    for k in 0..64 {
        l[k] = c[ZIGZAG8[k] as usize];
    }
    l
}
