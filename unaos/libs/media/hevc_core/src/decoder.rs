// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The decoder: NAL units in decoding order, frames out in output order (the API of h264_core's `Decoder`).
//!
//! - A picture begins at a slice segment with first_slice_segment_in_pic_flag and ends at the next one, at an access
//!   unit delimiter, a VPS / SPS, end of sequence / bitstream, the end of a length-prefixed sample, or
//!   [`Decoder::flush`].
//! - Picture start: parameter set activation (the refusals by name), PicOrderCntVal (8.3.1, prevTid0Pic),
//!   NoRaslOutputFlag (IDR, BLA, the first CRA and the CRA after an end of sequence; the RASL pictures of such an IRAP
//!   are dropped, as C.5.2.2 never outputs them) and the output of prior pictures (C.5.2.2).
//! - Picture end: deblocking (8.7.2), SAO (8.7.3), the conformance-window crop, and the "bumping" of C.5.2.3 on
//!   sps_max_num_reorder_pics / SpsMaxLatencyPictures / sps_max_dec_pic_buffering.

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use crate::ctu::{SliceDecoder, Storage};
use crate::deblock::deblock_picture;
use crate::nal::{self, NalHeader, AUD_NUT, CRA_NUT, EOB_NUT, EOS_NUT, PPS_NUT, SPS_NUT, VPS_NUT};
use crate::picture::{Picture, SliceInfo};
use crate::ps::{parse_pps, parse_sps, parse_vps, Geometry, Pps, Sps, Vps};
use crate::sao::sao_picture;
use crate::slice::{parse_slice_header, peek_pps_id, SliceHeader, SliceType};
use crate::{Error, Frame, Result};

struct CurPic {
    pic: Picture,
    sps: Sps,
    pps: Pps,
    geo: Geometry,
    store: Storage,
    /// The header of the current slice's independent segment (for dependent segments).
    indep: Option<SliceHeader>,
}

/// A decoded picture waiting for output (C.5.2: "needed for output").
struct Pending {
    frame: Frame,
    latency: u32,
}

/// An H.265 decoder for one stream.
pub struct Decoder {
    vps: Vec<Option<Vps>>,
    sps: Vec<Option<Sps>>,
    pps: Vec<Option<Pps>>,
    cur: Option<CurPic>,
    pending: Vec<Pending>,
    out: VecDeque<Frame>,
    tag: i64,
    nal_length_size: usize,
    rbsp: Vec<u8>,
    /// No picture since the start, a reset or an end of sequence: a CRA gets NoRaslOutputFlag 1.
    first_picture: bool,
    /// The associated IRAP had NoRaslOutputFlag 1: its RASL pictures are dropped.
    skip_rasl: bool,
    /// PicOrderCntVal of prevTid0Pic.
    prev_tid0_poc: i32,
    /// (sps_max_num_reorder_pics, SpsMaxLatencyPictures or 0, sps_max_dec_pic_buffering) of the active SPS.
    bump: (u32, u32, u32),
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder::new()
    }
}

/// The refusals by name (the crate doc): what an activated SPS / PPS may not use.
fn check_supported(sps: &Sps, pps: &Pps) -> Result<()> {
    if sps.chroma_format_idc != 1 || sps.separate_colour_plane_flag {
        return Err(Error::Unsupported("chroma other than 4:2:0"));
    }
    if sps.bit_depth_luma != 8 || sps.bit_depth_chroma != 8 {
        return Err(Error::Unsupported("bit depth above 8 (HEVCMAIN10)"));
    }
    if sps.range_ext.any() || pps.range_ext.any() {
        return Err(Error::Unsupported("range extension tools"));
    }
    if sps.scc_extension_flag || pps.scc_extension_flag {
        return Err(Error::Unsupported("screen content coding extension"));
    }
    Ok(())
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder {
            vps: vec![None; 16],
            sps: vec![None; 16],
            pps: vec![None; 64],
            cur: None,
            pending: Vec::new(),
            out: VecDeque::new(),
            tag: 0,
            nal_length_size: 4,
            rbsp: Vec::new(),
            first_picture: true,
            skip_rasl: false,
            prev_tid0_poc: 0,
            bump: (0, 0, 1),
        }
    }

    /// Forget every picture (after a seek); parameter sets and the hvcC configuration stay.
    pub fn reset(&mut self) {
        self.cur = None;
        self.pending.clear();
        self.out.clear();
        self.first_picture = true;
        self.skip_rasl = false;
        self.prev_tid0_poc = 0;
    }

    /// Read an HEVCDecoderConfigurationRecord (MP4 `hvcC`, Matroska CodecPrivate): its parameter sets (and SEI) and
    /// the NAL length size of the samples that follow.
    pub fn configure_hvcc(&mut self, hvcc: &[u8]) -> Result<()> {
        let c = nal::parse_hvcc(hvcc)?;
        if c.nal_length_size == 3 {
            return Err(Error::Invalid("hvcC lengthSizeMinusOne is 2"));
        }
        self.nal_length_size = c.nal_length_size;
        for u in c.units() {
            self.decode_nal(u)?;
        }
        Ok(())
    }

    /// The tag frames decoded from the next access unit carry (a presentation time).
    pub fn set_tag(&mut self, tag: i64) {
        self.tag = tag;
    }

    /// Decode an Annex B byte stream chunk (whole NAL units with start codes).
    pub fn decode_annexb(&mut self, data: &[u8]) -> Result<()> {
        for u in nal::annexb_units(data) {
            self.decode_nal(u)?;
        }
        Ok(())
    }

    /// Decode one length-prefixed sample (one access unit, ISO/IEC 14496-15) and finish its picture.
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
        self.bump_all();
    }

    /// The next frame in output order, if one is ready.
    pub fn next_frame(&mut self) -> Option<Frame> {
        self.out.pop_front()
    }

    /// Every frame ready for output, in output order.
    pub fn take_frames(&mut self) -> Vec<Frame> {
        self.out.drain(..).collect()
    }

    /// The active SPS of the picture under decoding, or the stored SPS `id`.
    pub fn sps(&self, id: usize) -> Option<&Sps> {
        self.sps.get(id).and_then(|s| s.as_ref())
    }

    pub fn pps(&self, id: usize) -> Option<&Pps> {
        self.pps.get(id).and_then(|s| s.as_ref())
    }

    pub fn vps(&self, id: usize) -> Option<&Vps> {
        self.vps.get(id).and_then(|s| s.as_ref())
    }

    /// Decode one NAL unit (two header bytes first, no start code or length prefix).
    pub fn decode_nal(&mut self, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let h = nal::parse_header(data)?;
        if h.layer_id > 0 {
            return Ok(());
        }
        let mut rbsp = core::mem::take(&mut self.rbsp);
        nal::unescape(&data[2..], &mut rbsp);
        let r = self.decode_rbsp(&h, &rbsp);
        self.rbsp = rbsp;
        r
    }

    fn decode_rbsp(&mut self, h: &NalHeader, rbsp: &[u8]) -> Result<()> {
        match h.nal_type {
            0..=9 | 16..=21 => self.decode_slice(h, rbsp),
            VPS_NUT => {
                self.finish_picture();
                let v = parse_vps(rbsp)?;
                let id = v.id as usize;
                self.vps[id] = Some(v);
                Ok(())
            }
            SPS_NUT => {
                self.finish_picture();
                let s = parse_sps(rbsp)?;
                let id = s.id as usize;
                self.sps[id] = Some(s);
                Ok(())
            }
            PPS_NUT => {
                let p = parse_pps(rbsp)?;
                let id = p.id as usize;
                self.pps[id] = Some(p);
                Ok(())
            }
            AUD_NUT => {
                self.finish_picture();
                Ok(())
            }
            EOS_NUT | EOB_NUT => {
                self.finish_picture();
                self.bump_all();
                self.first_picture = true;
                Ok(())
            }
            // SEI, filler data, reserved and unspecified types.
            _ => Ok(()),
        }
    }

    fn decode_slice(&mut self, nal: &NalHeader, rbsp: &[u8]) -> Result<()> {
        if rbsp.is_empty() {
            return Err(Error::Truncated);
        }
        let first = rbsp[0] & 0x80 != 0;
        if first {
            self.finish_picture();
        }
        if nal.is_rasl() && self.skip_rasl {
            return Ok(());
        }
        let pps_id = peek_pps_id(rbsp, nal)? as usize;
        if first {
            let pps = self.pps.get(pps_id).cloned().flatten().ok_or(Error::Invalid("slice references a missing PPS"))?;
            let sps = self.sps.get(pps.sps_id as usize).cloned().flatten().ok_or(Error::Invalid("PPS references a missing SPS"))?;
            let sh = parse_slice_header(rbsp, nal, &sps, &pps, None)?;
            let poc = self.picture_order(nal, &sh, &sps);
            if sh.slice_type != SliceType::I {
                return Err(Error::Unsupported("inter slices (HEVCINTER)"));
            }
            check_supported(&sps, &pps)?;
            self.start_picture(nal, &sh, sps, pps, poc)?;
            return self.decode_segment(sh, rbsp);
        }
        let Some(cur) = self.cur.as_ref() else {
            return Err(Error::Invalid("slice segment without its picture's first segment"));
        };
        if pps_id != cur.pps.id as usize {
            self.cur = None;
            return Err(Error::Invalid("slice segments of one picture name different PPSs"));
        }
        let sh = match parse_slice_header(rbsp, nal, &cur.sps, &cur.pps, cur.indep.as_ref()) {
            Ok(s) => s,
            Err(e) => {
                self.cur = None;
                return Err(e);
            }
        };
        if sh.slice_type != SliceType::I {
            self.cur = None;
            return Err(Error::Unsupported("inter slices (HEVCINTER)"));
        }
        self.decode_segment(sh, rbsp)
    }

    /// 8.3.1 PicOrderCntVal, NoRaslOutputFlag and the prevTid0Pic bookkeeping of the picture starting with `sh`.
    fn picture_order(&mut self, nal: &NalHeader, sh: &SliceHeader, sps: &Sps) -> i32 {
        let irap = nal.is_irap();
        let no_rasl = irap && (nal.is_idr() || nal.is_bla() || self.first_picture || nal.nal_type != CRA_NUT);
        if irap {
            self.skip_rasl = no_rasl;
        }
        let max = 1i32 << sps.log2_max_poc_lsb;
        let lsb = sh.pic_order_cnt_lsb as i32;
        let msb = if irap && no_rasl {
            0
        } else {
            let prev_lsb = self.prev_tid0_poc & (max - 1);
            let prev_msb = self.prev_tid0_poc - prev_lsb;
            if lsb < prev_lsb && prev_lsb - lsb >= max / 2 {
                prev_msb + max
            } else if lsb > prev_lsb && lsb - prev_lsb > max / 2 {
                prev_msb - max
            } else {
                prev_msb
            }
        };
        let poc = msb + lsb;
        if nal.temporal_id == 0 && !nal.is_rasl() && !nal.is_radl() && !nal.is_slnr() {
            self.prev_tid0_poc = poc;
        }
        if irap && no_rasl && !self.first_picture {
            // C.5.2.2: an IRAP with NoRaslOutputFlag 1 that is not the first picture empties the DPB — by output when
            // NoOutputOfPriorPicsFlag is 0, by discarding otherwise (a CRA infers 1).
            if nal.nal_type == CRA_NUT || sh.no_output_of_prior_pics_flag {
                self.pending.clear();
            } else {
                self.bump_all();
            }
        }
        self.first_picture = false;
        poc
    }

    fn start_picture(&mut self, nal: &NalHeader, sh: &SliceHeader, sps: Sps, pps: Pps, poc: i32) -> Result<()> {
        let geo = Geometry::new(&sps, &pps)?;
        let mut pic = Picture::new(sps.width as usize, sps.height as usize, &geo);
        pic.poc = poc;
        pic.tag = self.tag;
        pic.full_range = sps.vui.as_ref().is_some_and(|v| v.video_full_range_flag);
        pic.output = sh.pic_output_flag && !(nal.is_rasl() && self.skip_rasl);
        let [l, r, t, b] = sps.conf_win;
        let (x0, y0) = (2 * l as usize, 2 * t as usize);
        let cw = 2 * (l as usize + r as usize);
        let chh = 2 * (t as usize + b as usize);
        if cw >= pic.width || chh >= pic.height {
            return Err(Error::Invalid("conformance window empty"));
        }
        pic.crop = (x0, y0, pic.width - cw, pic.height - chh);
        let o = sps.highest_ordering();
        let latency = if o.max_latency_increase_plus1 != 0 { o.max_num_reorder_pics + o.max_latency_increase_plus1 - 1 } else { 0 };
        self.bump = (o.max_num_reorder_pics, latency, o.max_dec_pic_buffering_minus1 + 1);
        // C.5.2.2: before a non-IRAP-with-NoRaslOutputFlag picture, make room in the DPB.
        while !self.pending.is_empty() && self.pending.len() as u32 >= self.bump.2 {
            self.bump_one();
        }
        self.cur = Some(CurPic { pic, sps, pps, geo, store: Storage::default(), indep: None });
        Ok(())
    }

    fn decode_segment(&mut self, sh: SliceHeader, rbsp: &[u8]) -> Result<()> {
        let Some(cur) = self.cur.as_mut() else {
            return Err(Error::Invalid("no picture"));
        };
        let (slice_idx, addr_rs) = if sh.dependent_slice_segment_flag {
            if cur.pic.slices.is_empty() {
                self.cur = None;
                return Err(Error::Invalid("dependent slice segment first"));
            }
            let i = cur.pic.slices.len() - 1;
            (i as u32, cur.pic.slices[i].addr_rs)
        } else {
            cur.pic.slices.push(SliceInfo {
                addr_rs: sh.segment_address,
                deblocking_disabled: sh.deblocking_filter_disabled_flag,
                beta_offset_div2: sh.beta_offset_div2,
                tc_offset_div2: sh.tc_offset_div2,
                loop_filter_across_slices: sh.loop_filter_across_slices_enabled_flag,
                sao_luma: sh.sao_luma_flag,
                sao_chroma: sh.sao_chroma_flag,
            });
            ((cur.pic.slices.len() - 1) as u32, sh.segment_address)
        };
        let r = {
            let mut d = SliceDecoder::new(&cur.sps, &cur.pps, &cur.geo, &sh, &mut cur.pic, &mut cur.store, rbsp, slice_idx, addr_rs);
            d.decode()
        };
        if let Err(e) = r {
            self.cur = None;
            return Err(e);
        }
        if !sh.dependent_slice_segment_flag {
            cur.indep = Some(sh);
        }
        Ok(())
    }

    fn finish_picture(&mut self) {
        let Some(mut cur) = self.cur.take() else { return };
        deblock_picture(&mut cur.pic, &cur.geo, &cur.sps, &cur.pps);
        sao_picture(&mut cur.pic, &cur.geo, &cur.sps, &cur.pps);
        if !cur.pic.output {
            return;
        }
        for p in self.pending.iter_mut() {
            p.latency += 1;
        }
        self.pending.push(Pending { frame: cur.pic.to_frame(), latency: 0 });
        let (reorder, latency, _) = self.bump;
        while self.pending.len() as u32 > reorder || (latency != 0 && self.pending.iter().any(|p| p.latency >= latency)) {
            self.bump_one();
        }
    }

    /// C.5.2.4 "bumping": output the pending picture with the smallest PicOrderCntVal.
    fn bump_one(&mut self) {
        if let Some((i, _)) = self.pending.iter().enumerate().min_by_key(|(_, p)| p.frame.poc) {
            let p = self.pending.remove(i);
            self.out.push_back(p.frame);
        }
    }

    fn bump_all(&mut self) {
        while !self.pending.is_empty() {
            self.bump_one();
        }
    }
}
