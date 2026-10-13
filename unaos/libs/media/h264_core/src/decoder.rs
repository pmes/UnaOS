// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The decoder: NAL units in decoding order, frames out in output order.
//!
//! - Access units and primary coded pictures (7.4.1.2.3 / 7.4.1.2.4): a picture ends when the
//!   first slice of the next one arrives (the frame_num / PPS / nal_ref_idc / POC / IDR tests of
//!   7.4.1.2.4), when a non-VCL NAL unit that can only start an access unit arrives (SEI, SPS,
//!   PPS, access unit delimiter, end of sequence / stream, types 15..18), at the end of a
//!   length-prefixed sample, or on [`Decoder::flush`].
//! - Picture start: parameter set activation, the frame_num gap process (8.2.5.2), POC (8.2.1).
//! - Slices: reference lists (8.2.4), slice_data (7.3.4) for CAVLC (mb_skip_run) and CABAC
//!   (cabac_alignment_one_bit, context initialisation, mb_skip_flag, end_of_slice_flag), and the
//!   reconstruction of each macroblock (8.3 intra, 8.4 inter, 8.5 residual).
//! - Picture end: deblocking (8.7), marking (8.2.5), DPB storage and output (C.4).

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use crate::bits::BitReader;
use crate::cabac::Cabac;
use crate::deblock::{deblock_picture, SliceDeblock};
use crate::dpb::{Dpb, PocResult, PocState};
use crate::intra::{filter_8x8_edges, pred_16x16, pred_chroma, pred_nxn, Edges};
use crate::mb::{Entropy, Mb, SliceCtx};
use crate::nal::{self, NAL_IDR, NAL_SLICE};
use crate::picture::{blk_at, MbKind, PicState, Picture, BLK_X, BLK_Y};
use crate::ps::{self, Pps, SliceHeader, SliceType, Sps};
use crate::tables::QPC;
use crate::transform::{chroma_dc, idct4_add, idct8_add, luma_dc, scale4, scale8, LevelScale};
use crate::{Error, Frame, Result};

/// The picture under decoding.
struct CurPic {
    pic: Picture,
    st: PicState,
    /// The first slice header (the picture's identity for 7.4.1.2.4 and its marking).
    sh: SliceHeader,
    sps: Sps,
    pps: Pps,
    poc: PocResult,
    slices: Vec<SliceDeblock>,
    scale: LevelScale,
}

/// An H.264 decoder for one stream.
pub struct Decoder {
    sps: Vec<Option<Sps>>,
    pps: Vec<Option<Pps>>,
    dpb: Dpb,
    poc: PocState,
    cur: Option<CurPic>,
    out: VecDeque<Frame>,
    tag: i64,
    next_id: i32,
    nal_length_size: usize,
    prev_ref_frame_num: u32,
    /// (width_mbs, height_mbs) of the frames in the DPB.
    dims: Option<(u32, u32)>,
    rbsp: Vec<u8>,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder::new()
    }
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder {
            sps: vec![None; 32],
            pps: vec![None; 256],
            dpb: Dpb::new(),
            poc: PocState::default(),
            cur: None,
            out: VecDeque::new(),
            tag: 0,
            next_id: 0,
            nal_length_size: 4,
            prev_ref_frame_num: 0,
            dims: None,
            rbsp: Vec::new(),
        }
    }

    /// Forget every picture (after a seek); parameter sets and the avcC configuration stay.
    pub fn reset(&mut self) {
        self.dpb = Dpb::new();
        self.poc.reset();
        self.cur = None;
        self.out.clear();
        self.prev_ref_frame_num = 0;
        self.dims = None;
    }

    /// Read an AVCDecoderConfigurationRecord (MP4 `avcC`, Matroska CodecPrivate): its parameter
    /// sets and the NAL length size of the samples that follow.
    pub fn configure_avcc(&mut self, avcc: &[u8]) -> Result<()> {
        let c = nal::parse_avcc(avcc)?;
        self.nal_length_size = c.nal_length_size;
        for s in c.sps.iter().chain(c.pps.iter()) {
            self.decode_nal(s)?;
        }
        Ok(())
    }

    /// The tag frames decoded from the next access unit carry (a presentation time).
    pub fn set_tag(&mut self, tag: i64) {
        self.tag = tag;
    }

    /// Decode an Annex B byte stream chunk (whole NAL units with start codes). A picture whose
    /// last slice is in this chunk finishes when the next access unit begins or on [`flush`].
    ///
    /// [`flush`]: Decoder::flush
    pub fn decode_annexb(&mut self, data: &[u8]) -> Result<()> {
        for u in nal::annexb_units(data) {
            self.decode_nal(u)?;
        }
        Ok(())
    }

    /// Decode one length-prefixed sample (one access unit, ISO/IEC 14496-15) and finish its
    /// picture.
    pub fn decode_sample(&mut self, data: &[u8]) -> Result<()> {
        let units = nal::length_prefixed_units(data, self.nal_length_size)?;
        let mut first_err = None;
        for u in units {
            if let Err(e) = self.decode_nal(u) {
                first_err.get_or_insert(e);
            }
        }
        self.end_access_unit();
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// The current access unit is complete: finish its picture.
    pub fn end_access_unit(&mut self) {
        self.finish_picture();
    }

    /// End of stream: finish the current picture and output every frame still held.
    pub fn flush(&mut self) {
        self.finish_picture();
        self.dpb.flush(&mut self.out);
    }

    /// The next frame in output order, if one is ready.
    pub fn next_frame(&mut self) -> Option<Frame> {
        self.out.pop_front()
    }

    /// Every frame ready for output, in output order.
    pub fn take_frames(&mut self) -> Vec<Frame> {
        self.out.drain(..).collect()
    }

    /// Decode one NAL unit (header byte first, no start code or length prefix).
    pub fn decode_nal(&mut self, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let h = nal::parse_header(data[0])?;
        match h.nal_type {
            NAL_SLICE | NAL_IDR => self.decode_slice_nal(h.nal_type, h.ref_idc, &data[1..]),
            nal::NAL_SLICE_DPA..=nal::NAL_SLICE_DPC => Err(Error::Unsupported("data partitioning (Extended profile)")),
            nal::NAL_SPS => {
                self.finish_picture();
                nal::unescape(&data[1..], &mut self.rbsp);
                let s = ps::parse_sps(&self.rbsp)?;
                let id = s.id as usize;
                self.sps[id] = Some(s);
                Ok(())
            }
            nal::NAL_PPS => {
                self.finish_picture();
                nal::unescape(&data[1..], &mut self.rbsp);
                let sps = &self.sps;
                let p = ps::parse_pps(&self.rbsp, |id| sps.get(id as usize).and_then(|s| s.as_ref()).map_or(1, |s| s.chroma_format_idc))?;
                let id = p.id as usize;
                self.pps[id] = Some(p);
                Ok(())
            }
            nal::NAL_SEI | nal::NAL_AUD | nal::NAL_END_SEQ | nal::NAL_END_STREAM | 15..=18 => {
                self.finish_picture();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn decode_slice_nal(&mut self, nal_type: u8, ref_idc: u8, payload: &[u8]) -> Result<()> {
        let mut rbsp = core::mem::take(&mut self.rbsp);
        nal::unescape(payload, &mut rbsp);
        let r = self.decode_slice(nal_type, ref_idc, &rbsp);
        self.rbsp = rbsp;
        r
    }

    fn decode_slice(&mut self, nal_type: u8, ref_idc: u8, rbsp: &[u8]) -> Result<()> {
        let mut r = BitReader::new(rbsp);
        let sh = {
            let (pps, sps) = (&self.pps, &self.sps);
            ps::parse_slice_header(
                &mut r,
                nal_type,
                ref_idc,
                |id| pps.get(id as usize).and_then(|p| p.as_ref()),
                |id| sps.get(id as usize).and_then(|s| s.as_ref()),
            )?
        };
        if sh.redundant_pic_cnt > 0 {
            // A redundant coded picture: the primary one is decoded (it is never needed here).
            return Ok(());
        }
        if let Some(cur) = &self.cur {
            if new_picture(&cur.sh, &sh, &cur.sps) || cur.st.info.get(sh.first_mb as usize).is_none_or(|i| i.slice != u32::MAX) {
                self.finish_picture();
            }
        }
        if self.cur.is_none() {
            self.start_picture(&sh)?;
        }
        self.slice_data(&sh, rbsp)
    }

    /// 7.4.1.2.4 is not repeated for the slices of the picture; the parameter sets are those the
    /// first slice activated.
    fn start_picture(&mut self, sh: &SliceHeader) -> Result<()> {
        let pps = self.pps[sh.pps_id as usize].clone().ok_or(Error::Invalid("missing PPS"))?;
        let sps = self.sps[pps.sps_id as usize].clone().ok_or(Error::Invalid("missing SPS"))?;
        let dims = (sps.width_mbs, sps.height_mbs());
        if self.dims != Some(dims) {
            // A new frame size: everything held is output first (C.4.4, inferred
            // no_output_of_prior_pics_flag taken as 0).
            self.dpb.flush(&mut self.out);
            self.dpb.pics.clear();
            self.dims = Some(dims);
        }
        if !sh.idr && self.dpb.pics.iter().all(|p| !p.is_ref()) && sh.slice_type != SliceType::I && self.prev_ref_frame_num == 0 && self.next_id == 0 {
            return Err(Error::Invalid("stream starts with a predicted picture"));
        }
        self.dpb.size = sps.dpb_frames();
        self.dpb.max_reorder = sps.max_num_reorder.map(|v| v as usize);
        let (mb_w, mb_h) = (dims.0 as usize, dims.1 as usize);
        let max_frame_num = 1u32 << sps.log2_max_frame_num;
        if sh.idr {
            self.prev_ref_frame_num = 0;
        } else if sh.frame_num != self.prev_ref_frame_num && sh.frame_num != (self.prev_ref_frame_num + 1) % max_frame_num {
            self.fill_frame_num_gap(&sps, sh, mb_w, mb_h);
        }
        let poc = self.poc.compute(&sps, sh);
        let mut pic = Picture::new(mb_w, mb_h, self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        pic.top_poc = poc.top;
        pic.bottom_poc = poc.bottom;
        pic.poc = poc.top.min(poc.bottom);
        pic.frame_num = sh.frame_num;
        pic.idr = sh.idr;
        pic.tag = self.tag;
        pic.crop = sps.crop_rect();
        pic.full_range = sps.full_range;
        let scale = LevelScale::new(&pps.scaling(&sps));
        self.cur = Some(CurPic { pic, st: PicState::new(mb_w, mb_h), sh: sh.clone(), sps, pps, poc, slices: Vec::new(), scale });
        Ok(())
    }

    /// 8.2.5.2: frames for the skipped frame_num values, short-term references by the sliding
    /// window, never output. Only the last max_num_ref_frames of a long gap can survive the
    /// window, so only those are allocated.
    fn fill_frame_num_gap(&mut self, sps: &Sps, sh: &SliceHeader, mb_w: usize, mb_h: usize) {
        let max = 1u32 << sps.log2_max_frame_num;
        let gap = (sh.frame_num + max - self.prev_ref_frame_num - 1) % max;
        let keep = (sps.max_num_ref_frames.max(1)).min(gap);
        for k in 0..gap {
            let n = (self.prev_ref_frame_num + 1) % max;
            let mut fake = sh.clone();
            fake.frame_num = n;
            fake.nal_ref_idc = 1;
            fake.idr = false;
            fake.delta_poc = [0, 0];
            fake.delta_poc_bottom = 0;
            let mut poc = 0;
            if sps.poc_type != 0 {
                let r = self.poc.compute(sps, &fake);
                poc = r.top.min(r.bottom);
                self.poc.finish(sps, &fake, &r, false, 0);
            }
            if k >= gap - keep {
                let mut p = Picture::new(mb_w, mb_h, self.next_id);
                self.next_id = self.next_id.wrapping_add(1);
                p.frame_num = n;
                p.poc = poc;
                p.top_poc = poc;
                p.bottom_poc = poc;
                p.non_existing = true;
                self.dpb.sliding_window(sps, n);
                p.reference = crate::picture::RefState::Short;
                self.dpb.store(p, &mut self.out);
            }
            self.prev_ref_frame_num = n;
        }
    }

    /// Deblock, mark and store the picture under decoding.
    fn finish_picture(&mut self) {
        let Some(mut cur) = self.cur.take() else { return };
        let (mb_w, mb_h) = (cur.pic.mb_w, cur.pic.mb_h);
        deblock_picture(&mut cur.pic.planes, &cur.pic.motion, &cur.st.info, &cur.slices, mb_w, mb_h, cur.pps.chroma_qp_offset);
        self.dpb.mark(&mut cur.pic, &cur.sh, &cur.sps);
        let mmco5 = cur.pic.mmco5;
        let mut top_after = cur.pic.top_poc;
        if mmco5 {
            // 8.2.1: tempPicOrderCnt = PicOrderCnt(CurrPic) is subtracted after decoding.
            let t = cur.pic.poc;
            cur.pic.top_poc -= t;
            cur.pic.bottom_poc -= t;
            cur.pic.poc = 0;
            top_after = cur.pic.top_poc;
        }
        self.poc.finish(&cur.sps, &cur.sh, &cur.poc, mmco5, top_after);
        if cur.sh.nal_ref_idc != 0 {
            self.prev_ref_frame_num = if mmco5 { 0 } else { cur.sh.frame_num };
        }
        if cur.sh.idr || mmco5 {
            if cur.sh.idr && cur.sh.no_output_of_prior_pics {
                self.dpb.pics.clear();
            } else {
                self.dpb.flush(&mut self.out);
            }
        }
        cur.pic.output_needed = true;
        self.dpb.store(cur.pic, &mut self.out);
    }

    /// slice_data() (7.3.4) of one slice.
    fn slice_data(&mut self, sh: &SliceHeader, rbsp: &[u8]) -> Result<()> {
        let max_frame_num = 1u32 << self.cur.as_ref().map_or(4, |c| c.sps.log2_max_frame_num);
        let cur_poc = self.cur.as_ref().map_or(0, |c| c.pic.poc);
        let lists = if sh.slice_type == SliceType::I { [Vec::new(), Vec::new()] } else { self.dpb.ref_lists(sh, cur_poc, max_frame_num)? };
        let cur = self.cur.as_mut().ok_or(Error::Invalid("slice without a picture"))?;
        let slice = cur.slices.len() as u32;
        cur.slices.push(SliceDeblock { disable: sh.disable_deblocking, alpha_offset: sh.alpha_offset, beta_offset: sh.beta_offset });
        let (mb_w, mb_h) = (cur.pic.mb_w, cur.pic.mb_h);
        let total = mb_w * mb_h;
        let scale = &cur.scale;
        let mut ctx = SliceCtx {
            sps: &cur.sps,
            pps: &cur.pps,
            sh,
            slice,
            mb_w,
            mb_h,
            st: &mut cur.st,
            pic: &mut cur.pic,
            dpb: &self.dpb.pics,
            lists: [&lists[0], &lists[1]],
            qp: sh.qp,
            prev_qp_nz: false,
        };
        let mut addr = sh.first_mb as usize;
        let is_i = sh.slice_type == SliceType::I;
        if ctx.pps.cabac {
            let mut c = Cabac::new(rbsp, sh.data_bit.div_ceil(8) * 8);
            c.init_contexts(if is_i { 0 } else { 1 + sh.cabac_init_idc as usize }, sh.qp);
            let mut e = Entropy::Cabac(c);
            loop {
                if addr >= total {
                    return Err(Error::Invalid("slice data runs past the picture"));
                }
                let skip = match &mut e {
                    Entropy::Cabac(c) => !is_i && ctx.cabac_skip(c, addr),
                    _ => false,
                };
                let mb = if skip { ctx.skip_mb(addr) } else { ctx.parse_mb(&mut e, addr)? };
                reconstruct(&mut ctx, scale, addr, &mb)?;
                addr += 1;
                let Entropy::Cabac(c) = &mut e else { unreachable!() };
                if c.terminate() == 1 {
                    break;
                }
                if c.overran() {
                    return Err(Error::Truncated);
                }
            }
        } else {
            let mut br = BitReader::new(rbsp);
            br.set_pos(sh.data_bit);
            let mut e = Entropy::Cavlc(br);
            loop {
                let Entropy::Cavlc(r) = &mut e else { unreachable!() };
                if !is_i {
                    let run = r.ue() as usize;
                    if r.overran() || addr + run > total {
                        return Err(Error::Invalid("mb_skip_run past the picture"));
                    }
                    for _ in 0..run {
                        let mb = ctx.skip_mb(addr);
                        reconstruct(&mut ctx, scale, addr, &mb)?;
                        addr += 1;
                    }
                    let Entropy::Cavlc(r) = &mut e else { unreachable!() };
                    if run > 0 && !r.more_rbsp_data() {
                        break;
                    }
                }
                if addr >= total {
                    return Err(Error::Invalid("slice data runs past the picture"));
                }
                let mb = ctx.parse_mb(&mut e, addr)?;
                reconstruct(&mut ctx, scale, addr, &mb)?;
                addr += 1;
                let Entropy::Cavlc(r) = &mut e else { unreachable!() };
                if !r.more_rbsp_data() {
                    break;
                }
            }
        }
        Ok(())
    }
}

/// 7.4.1.2.4: does slice header `b` begin a new primary coded picture after `a`?
fn new_picture(a: &SliceHeader, b: &SliceHeader, sps: &Sps) -> bool {
    a.frame_num != b.frame_num
        || a.pps_id != b.pps_id
        || (a.nal_ref_idc == 0) != (b.nal_ref_idc == 0)
        || (sps.poc_type == 0 && (a.poc_lsb != b.poc_lsb || a.delta_poc_bottom != b.delta_poc_bottom))
        || (sps.poc_type == 1 && a.delta_poc != b.delta_poc)
        || a.idr != b.idr
        || (a.idr && b.idr && a.idr_pic_id != b.idr_pic_id)
}

/// Is the luma sample at (x, y) relative to macroblock `addr` available for intra prediction
/// of a block whose top-left luma4x4BlkIdx is `limit` (8.3.1.2 and its 8x8 / 16x16 / chroma
/// counterparts)?
fn sample_available(ctx: &SliceCtx, addr: usize, x: isize, y: isize, limit: usize) -> bool {
    match ctx.nb(addr, x.div_euclid(4), y.div_euclid(4), 4) {
        None => false,
        Some((m, nx, ny)) => {
            if m == addr {
                return blk_at(nx, ny) < limit;
            }
            !(ctx.pps.constrained_intra_pred && !ctx.st.info[m].kind.is_intra())
        }
    }
}

/// Gather the edges of a luma block at (bx, by) (samples, relative to the macroblock) of size n,
/// with `tr` top-right samples (n for 4x4 / 8x8, 0 for 16x16).
fn luma_edges(ctx: &SliceCtx, addr: usize, bx: usize, by: usize, n: usize, tr: usize, limit: usize) -> Edges {
    let pl = &ctx.pic.planes[0];
    let (px, py) = ((addr % ctx.mb_w) * 16 + bx, (addr / ctx.mb_w) * 16 + by);
    let (bxi, byi) = (bx as isize, by as isize);
    let mut e = Edges::new();
    e.has_top = sample_available(ctx, addr, bxi, byi - 1, limit);
    e.has_left = sample_available(ctx, addr, bxi - 1, byi, limit);
    e.has_tl = sample_available(ctx, addr, bxi - 1, byi - 1, limit);
    if e.has_top {
        for x in 0..n {
            e.top[x] = pl.data[(py - 1) * pl.w + px + x] as i32;
        }
        if tr > 0 {
            if sample_available(ctx, addr, bxi + n as isize, byi - 1, limit) {
                for x in 0..tr {
                    e.top[n + x] = pl.data[(py - 1) * pl.w + px + n + x] as i32;
                }
            } else {
                for x in 0..tr {
                    e.top[n + x] = e.top[n - 1];
                }
            }
        }
    }
    if e.has_left {
        for y in 0..n {
            e.left[y] = pl.data[(py + y) * pl.w + px - 1] as i32;
        }
    }
    if e.has_tl {
        e.tl = pl.data[(py - 1) * pl.w + px - 1] as i32;
    }
    e
}

fn chroma_edges(ctx: &SliceCtx, addr: usize, c: usize) -> Edges {
    let pl = &ctx.pic.planes[c];
    let (px, py) = ((addr % ctx.mb_w) * 8, (addr / ctx.mb_w) * 8);
    let mut e = Edges::new();
    e.has_top = sample_available(ctx, addr, 0, -1, 16);
    e.has_left = sample_available(ctx, addr, -1, 0, 16);
    e.has_tl = sample_available(ctx, addr, -1, -1, 16);
    if e.has_top {
        for x in 0..8 {
            e.top[x] = pl.data[(py - 1) * pl.w + px + x] as i32;
        }
    }
    if e.has_left {
        for y in 0..8 {
            e.left[y] = pl.data[(py + y) * pl.w + px - 1] as i32;
        }
    }
    if e.has_tl {
        e.tl = pl.data[(py - 1) * pl.w + px - 1] as i32;
    }
    e
}

fn write_block(ctx: &mut SliceCtx, c: usize, x0: usize, y0: usize, n: usize, src: &[i32]) {
    let pl = &mut ctx.pic.planes[c];
    for y in 0..n {
        for x in 0..n {
            pl.data[(y0 + y) * pl.w + x0 + x] = src[y * n + x].clamp(0, 255) as u8;
        }
    }
}

/// Reconstruct macroblock `addr` from its parsed syntax: prediction (8.3 / 8.4) plus residual
/// (8.5), into the current picture.
fn reconstruct(ctx: &mut SliceCtx, ls: &LevelScale, addr: usize, mb: &Mb) -> Result<()> {
    let (mx, my) = ((addr % ctx.mb_w) * 16, (addr / ctx.mb_w) * 16);
    let intra = mb.kind.is_intra();
    ctx.pic.motion.mb_intra[addr] = intra;
    if mb.kind == MbKind::IPcm {
        for c in 0..3 {
            let (n, x0, y0, off) = if c == 0 { (16, mx, my, 0) } else { (8, mx / 2, my / 2, 256 + (c - 1) * 64) };
            let src: Vec<i32> = mb.pcm[off..off + n * n].iter().map(|&v| v as i32).collect();
            write_block(ctx, c, x0, y0, n, &src);
        }
        return Ok(());
    }
    let qp = mb.qp;
    let qpc = [QPC[(qp + ctx.pps.chroma_qp_offset[0]).clamp(0, 51) as usize] as i32, QPC[(qp + ctx.pps.chroma_qp_offset[1]).clamp(0, 51) as usize] as i32];
    let (l4y, l8) = if intra { (0, 0) } else { (3, 1) };
    let stride = ctx.pic.planes[0].w;

    // Luma.
    match mb.kind {
        MbKind::I4x4 => {
            let mut pred = [0i32; 64];
            for blk in 0..16 {
                let (bx, by) = (BLK_X[blk] * 4, BLK_Y[blk] * 4);
                let e = luma_edges(ctx, addr, bx, by, 4, 4, blk);
                pred_nxn(4, mb.modes[blk], &e, &mut pred);
                write_block(ctx, 0, mx + bx, my + by, 4, &pred[..16]);
                if (ctx.st.info[addr].nz >> blk) & 1 == 1 {
                    let mut c = mb.luma[blk];
                    scale4(&mut c, &ls.l4[l4y][(qp % 6) as usize], qp, false);
                    idct4_add(&c, &mut ctx.pic.planes[0].data, (my + by) * stride + mx + bx, stride);
                }
            }
        }
        MbKind::I8x8 => {
            let mut pred = [0i32; 64];
            for b8 in 0..4 {
                let (bx, by) = ((b8 % 2) * 8, (b8 / 2) * 8);
                let e = luma_edges(ctx, addr, bx, by, 8, 8, b8 * 4);
                let f = filter_8x8_edges(&e);
                pred_nxn(8, mb.modes[b8], &f, &mut pred);
                write_block(ctx, 0, mx + bx, my + by, 8, &pred);
                if (ctx.st.info[addr].nz >> (b8 * 4)) & 1 == 1 {
                    let mut c = mb.luma8[b8];
                    scale8(&mut c, &ls.l8[l8][(qp % 6) as usize], qp);
                    idct8_add(&c, &mut ctx.pic.planes[0].data, (my + by) * stride + mx + bx, stride);
                }
            }
        }
        MbKind::I16x16 => {
            let e = luma_edges(ctx, addr, 0, 0, 16, 0, 16);
            let mut pred = [0i32; 256];
            pred_16x16(mb.i16_mode, &e, &mut pred);
            write_block(ctx, 0, mx, my, 16, &pred);
            let q6 = (qp % 6) as usize;
            let dc = if mb.dc.iter().any(|&v| v != 0) { luma_dc(&mb.dc, ls.l4[0][q6][0], qp) } else { [0; 16] };
            for blk in 0..16 {
                let (bx, by) = (BLK_X[blk], BLK_Y[blk]);
                let mut c = mb.luma[blk];
                c[0] = dc[by * 4 + bx];
                if c.iter().all(|&v| v == 0) {
                    continue;
                }
                scale4(&mut c, &ls.l4[0][q6], qp, true);
                idct4_add(&c, &mut ctx.pic.planes[0].data, (my + by * 4) * stride + mx + bx * 4, stride);
            }
        }
        _ => {
            ctx.inter_mb(addr, mb)?;
            let nz = ctx.st.info[addr].nz;
            if mb.t8x8 {
                for b8 in 0..4 {
                    if (nz >> (b8 * 4)) & 1 == 1 {
                        let (bx, by) = ((b8 % 2) * 8, (b8 / 2) * 8);
                        let mut c = mb.luma8[b8];
                        scale8(&mut c, &ls.l8[l8][(qp % 6) as usize], qp);
                        idct8_add(&c, &mut ctx.pic.planes[0].data, (my + by) * stride + mx + bx, stride);
                    }
                }
            } else {
                for blk in 0..16 {
                    if (nz >> blk) & 1 == 1 {
                        let (bx, by) = (BLK_X[blk] * 4, BLK_Y[blk] * 4);
                        let mut c = mb.luma[blk];
                        scale4(&mut c, &ls.l4[l4y][(qp % 6) as usize], qp, false);
                        idct4_add(&c, &mut ctx.pic.planes[0].data, (my + by) * stride + mx + bx, stride);
                    }
                }
            }
        }
    }

    // Chroma.
    let cstride = ctx.pic.planes[1].w;
    for c in 0..2 {
        if intra {
            let e = chroma_edges(ctx, addr, 1 + c);
            let mut pred = [0i32; 64];
            pred_chroma(mb.chroma_mode, &e, &mut pred);
            write_block(ctx, 1 + c, mx / 2, my / 2, 8, &pred);
        }
        if mb.cbp >> 4 == 0 {
            continue;
        }
        let list = if intra { 1 + c } else { 4 + c };
        let q = qpc[c];
        let q6 = (q % 6) as usize;
        let dc = chroma_dc(&mb.cdc[c], ls.l4[list][q6][0], q);
        for blk in 0..4 {
            let mut co = mb.cac[c][blk];
            co[0] = dc[blk];
            if co.iter().all(|&v| v == 0) {
                continue;
            }
            scale4(&mut co, &ls.l4[list][q6], q, true);
            let (bx, by) = ((blk % 2) * 4, (blk / 2) * 4);
            idct4_add(&co, &mut ctx.pic.planes[1 + c].data, (my / 2 + by) * cstride + mx / 2 + bx, cstride);
        }
    }
    Ok(())
}
