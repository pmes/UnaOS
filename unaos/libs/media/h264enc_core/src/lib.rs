// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENC (LEDGER SR197): an H.264 / AVC encoder (ITU-T Rec. H.264), `no_std` + `alloc`, whose
//! one dependency is h264_core — the editor's export leg (the NLE line's first encoder).
//!
//! CHARTER: Stria (A/V) — seam `shared-core` (a `no_std` lib both rings link, beside h264_core).
//!
//! What it writes: Constrained Baseline (profile_idc 66, constraint_set0/1), CAVLC, frames only,
//! one slice per picture, constant QP (16..=40). An IDR every `gop` frames, P frames between them
//! against the previous reconstruction (one reference frame). Macroblocks: Intra_4x4 and
//! Intra_16x16 (the SAD-cheapest mode of each, chroma likewise) in I slices; P_Skip and
//! P_L0_16x16 by an integer-sample full search in a +-8 window in P slices. Output as Annex B
//! byte stream or as avcC samples with the AVCDecoderConfigurationRecord ([`Encoder::avcc`]).
//!
//! | module          | Recommendation                                                          |
//! |-----------------|-------------------------------------------------------------------------|
//! | [`nal`]         | 7.2 RBSP writing, 9.1 ue / se, 7.3.1 NAL units, emulation prevention, avcC |
//! | [`sps`]         | 7.3.2.1 SPS (+ VUI E.1.1), 7.3.2.2 PPS, 7.3.3 slice header                |
//! | [`transform`]   | 8.5 forward: 4x4 core transform, DC Hadamards, quantiser                 |
//! | [`intra`]       | 8.3 mode decision over h264_core's predictors, 8.3.1.1 mode coding       |
//! | [`cavlc`]       | 9.2 residual_block_cavlc written, 9.2.1 nC                               |
//! | [`me`]          | 8.4.1 MV prediction and P_Skip, integer motion search                    |
//!
//! Shared, never copied: h264_core's `transform` (`LevelScale`, `scale4`, `luma_dc`, `chroma_dc`,
//! `idct4_add`) is the encoder's reconstruction; its `intra` (`pred_nxn`, `pred_16x16`,
//! `pred_chroma`, `Edges`) its prediction; its `tables` (coeff_token, total_zeros, run_before,
//! coded_block_pattern, QPc, zig-zag) its code words; its `inter` (`luma_mc`, `chroma_mc`) and
//! `picture` (`Plane`, `Mv`) its motion compensation. The reconstruction loop is the decoder's
//! arithmetic in the decoder's order, so `h264_core::Decoder` decodes the output to exactly
//! [`Encoder::reconstruction`] (the KAT). The neighbour rules (availability, predicted intra mode,
//! nC, mvp) are decoder-private methods there, so they are mirrored here for one slice per
//! picture; making them shared free fns is owed (H264ENC-NBSHARE).
//!
//! Owed (handlers.md §5): rate control (H264ENC-RC), CABAC / Main / High (H264ENC-CABAC), sub-pel
//! motion (H264ENC-SUBPEL), B-frames (H264ENC-BFRAMES), the in-loop deblocking filter
//! (H264ENC-DEBLOCK: slices carry disable_deblocking_filter_idc 1), hardware encode (H264ENC-HW).

#![no_std]
#![forbid(unsafe_code)]
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

extern crate alloc;

pub mod cavlc;
pub mod intra;
pub mod me;
pub mod nal;
pub mod sps;
pub mod transform;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use h264_core::picture::{Plane, BLK_X, BLK_Y};
use h264_core::ps::ScalingMatrices;
use h264_core::tables::{CBP_INTER, CBP_INTRA, QPC, ZIGZAG4};
use h264_core::transform::LevelScale;

use crate::cavlc::Nnz;
use crate::me::MbMotion;
use crate::nal::BitWriter;
use crate::sps::SeqParams;

/// Every way the encoder refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Width or height zero, odd (4:2:0 cropping is in units of 2) or above 16384.
    Geometry,
    /// QP outside 16..=40.
    Qp,
    /// A GOP length of 0.
    Gop,
    /// A frame whose size or planes do not match the configuration.
    FrameSize,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Error::Geometry => "h264enc: width / height not even and in 2..=16384",
            Error::Qp => "h264enc: QP outside 16..=40",
            Error::Gop => "h264enc: GOP length 0",
            Error::FrameSize => "h264enc: frame does not match the configured size",
        })
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// The stream's fixed parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub width: u32,
    pub height: u32,
    /// Constant QP (QPY) of every macroblock, 16..=40.
    pub qp: u8,
    /// An IDR picture every `gop` frames (1 = every frame intra).
    pub gop: u32,
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
}

/// The decided syntax of one macroblock: modes, motion and levels (raster within each block, as
/// h264_core's `mb::Mb` holds them).
pub(crate) struct MbCode {
    pub kind: Kind,
    /// Intra4x4PredMode by luma4x4BlkIdx.
    pub modes: [u8; 16],
    pub i16_mode: u8,
    pub chroma_mode: u8,
    /// mvd_l0 of P_L0_16x16 (quarter samples).
    pub mvd: [i32; 2],
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
}

impl MbCode {
    pub fn new(kind: Kind) -> MbCode {
        MbCode { kind, modes: [2; 16], i16_mode: 0, chroma_mode: 0, mvd: [0; 2], cbp: 0, dc: [0; 16], luma: [[0; 16]; 16], cdc: [[0; 4]; 2], cac: [[[0; 16]; 4]; 2] }
    }

    /// CodedBlockPatternLuma for 4x4-coded luma: bit b8 when a block of that 8x8 has a level.
    pub fn luma_cbp(&self) -> u8 {
        let mut cbp = 0u8;
        for blk in 0..16 {
            if self.luma[blk].iter().any(|&v| v != 0) {
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
    pub(crate) qp: i32,
    /// QPc of the constant QP (chroma_qp_index_offset 0, Table 8-15).
    pub(crate) qpc: i32,
    log2_max_frame_num: u32,
    sps: Vec<u8>,
    pps: Vec<u8>,
    pub(crate) ls: Box<LevelScale>,
    /// The input, padded to whole macroblocks by edge replication.
    pub(crate) src: [Plane; 3],
    /// The picture being reconstructed.
    pub(crate) rec: [Plane; 3],
    /// The previous reconstruction: the reference of a P picture.
    pub(crate) refp: [Plane; 3],
    crop: [Vec<u8>; 3],
    pub(crate) nnz: Nnz,
    /// Intra4x4PredMode per 4x4 block of the picture (2 for any block not coded Intra_4x4).
    pub(crate) modes: Vec<u8>,
    /// Motion per macroblock of the current P picture.
    pub(crate) motion: Vec<MbMotion>,
    frames: u64,
    idr_count: u32,
    frame_num: u32,
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
        let (mb_w, mb_h) = (cfg.width.div_ceil(16) as usize, cfg.height.div_ceil(16) as usize);
        let qp = cfg.qp as i32;
        let mut log2_max_frame_num = 4;
        while log2_max_frame_num < 16 && (1u64 << log2_max_frame_num) < cfg.gop as u64 {
            log2_max_frame_num += 1;
        }
        let p = SeqParams { width: cfg.width, height: cfg.height, mb_w: mb_w as u32, mb_h: mb_h as u32, log2_max_frame_num, qp };
        let sps = nal::nal_unit(3, nal::NAL_SPS, &sps::write_sps(&p));
        let pps = nal::nal_unit(3, nal::NAL_PPS, &sps::write_pps(qp));
        let planes = || [Plane::new(mb_w * 16, mb_h * 16, 0), Plane::new(mb_w * 8, mb_h * 8, 128), Plane::new(mb_w * 8, mb_h * 8, 128)];
        let (w, h) = (cfg.width as usize, cfg.height as usize);
        Ok(Encoder {
            cfg,
            mb_w,
            mb_h,
            qp,
            qpc: QPC[qp as usize] as i32,
            log2_max_frame_num,
            sps,
            pps,
            ls: Box::new(LevelScale::new(&ScalingMatrices::FLAT)),
            src: planes(),
            rec: planes(),
            refp: planes(),
            crop: [vec![0; w * h], vec![128; w * h / 4], vec![128; w * h / 4]],
            nnz: Nnz::new(mb_w, mb_h),
            modes: vec![2; mb_w * mb_h * 16],
            motion: vec![MbMotion::NONE; mb_w * mb_h],
            frames: 0,
            idr_count: 0,
            frame_num: 0,
        })
    }

    pub fn config(&self) -> Config {
        self.cfg
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

    /// Encode the next frame: an IDR when the frame index is a multiple of the GOP, else a P frame.
    pub fn encode(&mut self, f: &Yuv420) -> Result<Access> {
        self.load(f)?;
        let idr = self.frames.is_multiple_of(self.cfg.gop as u64);
        self.frame_num = if idr { 0 } else { (self.frame_num + 1) % (1 << self.log2_max_frame_num) };
        self.nnz.reset();
        self.modes.fill(2);
        self.motion.fill(MbMotion::NONE);
        let mut w = BitWriter::new();
        sps::write_slice_header(&mut w, idr, self.frame_num, self.log2_max_frame_num, self.idr_count & 0xffff);
        let mut run = 0u32;
        for mby in 0..self.mb_h {
            for mbx in 0..self.mb_w {
                if idr {
                    let code = self.intra_mb(mbx, mby);
                    self.write_mb(&mut w, mbx, mby, &code, false);
                } else {
                    let code = self.inter_mb(mbx, mby);
                    if code.kind == Kind::PSkip {
                        run += 1;
                    } else {
                        w.ue(run); // mb_skip_run
                        run = 0;
                        self.write_mb(&mut w, mbx, mby, &code, true);
                    }
                }
            }
        }
        if run > 0 {
            w.ue(run);
        }
        w.trailing();
        let slice = nal::nal_unit(3, if idr { nal::NAL_IDR } else { nal::NAL_SLICE }, &w.finish());
        let nals = if idr { vec![self.sps.clone(), self.pps.clone(), slice] } else { vec![slice] };
        self.store_crop();
        core::mem::swap(&mut self.rec, &mut self.refp);
        self.frames += 1;
        if idr {
            self.idr_count = self.idr_count.wrapping_add(1);
        }
        Ok(Access { keyframe: idr, nals })
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
                let w4 = self.mb_w * 4;
                for blk in 0..16 {
                    let (x4, y4) = (mbx * 4 + BLK_X[blk], mby * 4 + BLK_Y[blk]);
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
            Kind::P16 => {
                w.ue(0); // P_L0_16x16; ref_idx_l0 absent (one active reference)
                w.se(code.mvd[0]);
                w.se(code.mvd[1]);
                w.ue(cbp_code(&CBP_INTER, code.cbp));
            }
            Kind::PSkip => return,
        }
        if code.cbp != 0 || code.kind == Kind::I16 {
            w.se(0); // mb_qp_delta: QP is constant
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
                tc = if i16 { cavlc::write_block(w, nc, 15, &scan(&code.luma[blk], 1)) } else { cavlc::write_block(w, nc, 16, &scan(&code.luma[blk], 0)) };
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
