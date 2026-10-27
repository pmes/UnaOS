// SPDX-License-Identifier: LGPL-3.0-or-later
//! HEVCPARSE test kit: a bit writer, a CABAC ENCODER written as the inverse of the decoding engine (the encoder of
//! H.264 9.3.4.2 .. 9.3.4.6, which H.265's engine shares: the flush ends with the 1 that is the stop / alignment bit),
//! and builders for hand-assembled VPS / SPS / PPS / slice NAL units. Used only by the tests (HEVCTESTKIT owes one
//! shared copy with h264_core's tests/structure.rs).
#![allow(dead_code)]

use h264_core::tables::{RANGE_TAB_LPS, TRANS_IDX_LPS, TRANS_IDX_MPS};
use hevc_core::cabac::initial_contexts;
use hevc_core::tables::*;

#[derive(Default, Clone)]
pub struct BitWriter {
    pub out: Vec<u8>,
    pub n: usize,
}

impl BitWriter {
    pub fn bit(&mut self, b: u32) {
        if self.n % 8 == 0 {
            self.out.push(0);
        }
        if b != 0 {
            *self.out.last_mut().unwrap() |= 0x80 >> (self.n % 8);
        }
        self.n += 1;
    }
    pub fn u(&mut self, v: u64, n: u32) {
        for i in (0..n).rev() {
            self.bit(((v >> i) & 1) as u32);
        }
    }
    pub fn flag(&mut self, b: bool) {
        self.bit(b as u32);
    }
    pub fn ue(&mut self, v: u32) {
        let x = v as u64 + 1;
        let len = 64 - x.leading_zeros();
        for _ in 0..len - 1 {
            self.bit(0);
        }
        self.u(x, len);
    }
    pub fn se(&mut self, v: i32) {
        self.ue(if v > 0 { (2 * v - 1) as u32 } else { (-2 * v) as u32 });
    }
    pub fn aligned(&self) -> bool {
        self.n % 8 == 0
    }
    pub fn zero_align(&mut self) {
        while !self.aligned() {
            self.bit(0);
        }
    }
    /// rbsp_trailing_bits / byte_alignment(): a 1, then zeros.
    pub fn trailing(&mut self) {
        self.bit(1);
        self.zero_align();
    }
}

/// The CABAC encoder.
pub struct CabacEnc {
    pub bw: BitWriter,
    low: u32,
    range: u32,
    outstanding: u32,
    first: bool,
    pub state: [u8; 460],
}

impl CabacEnc {
    /// An encoder appending to `bw` (byte aligned), contexts initialised for (initType, SliceQpY).
    pub fn new(bw: BitWriter, init_type: usize, qp: i32) -> CabacEnc {
        assert!(bw.aligned());
        CabacEnc { bw, low: 0, range: 510, outstanding: 0, first: true, state: initial_contexts(init_type, qp) }
    }
    fn put_bit(&mut self, b: u32) {
        if self.first {
            self.first = false;
        } else {
            self.bw.bit(b);
        }
        while self.outstanding > 0 {
            self.bw.bit(1 - b);
            self.outstanding -= 1;
        }
    }
    fn renorm(&mut self) {
        while self.range < 256 {
            if self.low < 256 {
                self.put_bit(0);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put_bit(1);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }
    pub fn decision(&mut self, ctx: usize, bin: u32) {
        let s = self.state[ctx];
        let mut p = (s >> 1) as usize;
        let mut mps = (s & 1) as u32;
        let q = ((self.range >> 6) & 3) as usize;
        let lps = RANGE_TAB_LPS[p][q] as u32;
        self.range -= lps;
        if bin != mps {
            self.low += self.range;
            self.range = lps;
            if p == 0 {
                mps = 1 - mps;
            }
            p = TRANS_IDX_LPS[p] as usize;
        } else {
            p = TRANS_IDX_MPS[p] as usize;
        }
        self.state[ctx] = ((p as u8) << 1) | mps as u8;
        self.renorm();
    }
    pub fn bypass(&mut self, bin: u32) {
        self.low <<= 1;
        if bin != 0 {
            self.low += self.range;
        }
        if self.low >= 1024 {
            self.put_bit(1);
            self.low -= 1024;
        } else if self.low < 512 {
            self.put_bit(0);
        } else {
            self.low -= 512;
            self.outstanding += 1;
        }
    }
    pub fn bypass_bits(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.bypass((v >> i) & 1);
        }
    }
    /// TR with cRiceParam 0 in bypass bins.
    pub fn bypass_tu(&mut self, v: u32, cmax: u32) {
        for _ in 0..v {
            self.bypass(1);
        }
        if v < cmax {
            self.bypass(0);
        }
    }
    pub fn egk(&mut self, mut v: u32, mut k: u32) {
        while v >= (1 << k) {
            self.bypass(1);
            v -= 1 << k;
            k += 1;
        }
        self.bypass(0);
        self.bypass_bits(v, k);
    }
    /// coeff_abs_level_remaining (9.3.3.11) with cRiceParam `rice`.
    pub fn remaining(&mut self, v: u32, rice: u32) {
        if (v >> rice) < 4 {
            let p = v >> rice;
            for _ in 0..p {
                self.bypass(1);
            }
            self.bypass(0);
            self.bypass_bits(v & ((1 << rice) - 1), rice);
        } else {
            let mut p = 4;
            while v >= (((1u32 << (p - 2)) + 2) << rice) {
                p += 1;
            }
            for _ in 0..p {
                self.bypass(1);
            }
            self.bypass(0);
            self.bypass_bits(v - (((1 << (p - 3)) + 2) << rice), p - 3 + rice);
        }
    }
    /// DecodeTerminate's encoder; a 1 flushes (the code word ends with the stop / alignment 1).
    pub fn terminate(&mut self, bin: u32) {
        self.range -= 2;
        if bin != 0 {
            self.low += self.range;
            self.range = 2;
            self.renorm();
            self.put_bit((self.low >> 9) & 1);
            self.bw.u((((self.low >> 7) & 3) | 1) as u64, 2);
        } else {
            self.renorm();
        }
    }
    /// After a terminate of 1: zero bits to the byte boundary (pcm_alignment_zero_bit / byte_alignment zeros).
    pub fn zero_align(&mut self) {
        self.bw.zero_align();
    }
    /// Re-initialise the engine (after pcm samples or at a substream) keeping the context states.
    pub fn restart(&mut self) {
        assert!(self.bw.aligned());
        self.low = 0;
        self.range = 510;
        self.outstanding = 0;
        self.first = true;
    }

    // Syntax elements with hevc_core's slots (the crate doc's layout).
    pub fn split_cu(&mut self, inc: usize, v: bool) {
        self.decision(SPLIT_CU + inc, v as u32);
    }
    pub fn part_mode_2nx2n(&mut self) {
        self.decision(PART_MODE, 1);
    }
    pub fn prev_intra(&mut self, v: bool) {
        self.decision(PREV_INTRA_LUMA, v as u32);
    }
    pub fn mpm_idx(&mut self, i: u32) {
        self.bypass_tu(i, 2);
    }
    pub fn chroma_dm(&mut self) {
        self.decision(INTRA_CHROMA, 0);
    }
    pub fn cbf_chroma(&mut self, depth: usize, v: bool) {
        self.decision(CBF_CHROMA + depth, v as u32);
    }
    pub fn cbf_luma(&mut self, depth: usize, v: bool) {
        self.decision(CBF_LUMA + if depth == 0 { 1 } else { 0 }, v as u32);
    }
    /// A 2Nx2N intra CU above the minimum size coded as INTRA_DC through mpm_idx `mpm`, no residual.
    pub fn dc_cu(&mut self, mpm: u32) {
        self.prev_intra(true);
        self.mpm_idx(mpm);
        self.chroma_dm();
        self.cbf_chroma(0, false);
        self.cbf_chroma(0, false);
        self.cbf_luma(0, false);
    }
    pub fn finish(mut self) -> BitWriter {
        self.terminate(1);
        self.zero_align();
        self.bw
    }
}

/// A NAL unit (two header bytes, emulation prevention), without a start code.
pub fn nal_bytes(ty: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut v = vec![ty << 1, 1];
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            v.push(3);
            zeros = 0;
        }
        v.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    v
}

/// A NAL unit as Annex B bytes.
pub fn nal(ty: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 1];
    v.extend(nal_bytes(ty, rbsp));
    v
}

/// profile_tier_level(1, n): Main profile, level 2 (general_level_idc 60).
pub fn ptl(b: &mut BitWriter, max_sub_layers_minus1: u32) {
    b.u(0, 2);
    b.bit(0);
    b.u(1, 5);
    b.u(0x6000_0000, 32);
    b.bit(1); // progressive
    b.bit(0);
    b.bit(0);
    b.bit(1); // frame only
    b.u(0, 44);
    b.u(60, 8);
    for _ in 0..max_sub_layers_minus1 {
        b.bit(0);
        b.bit(0);
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            b.u(0, 2);
        }
    }
}

/// hrd_parameters(1, 0): NAL HRD only, one CPB, fixed picture rate.
pub fn hrd(b: &mut BitWriter) {
    b.bit(1); // nal_hrd_parameters_present_flag
    b.bit(0); // vcl
    b.bit(0); // sub_pic_hrd_params_present_flag
    b.u(2, 4); // bit_rate_scale
    b.u(3, 4); // cpb_size_scale
    b.u(23, 5);
    b.u(23, 5);
    b.u(23, 5);
    b.bit(1); // fixed_pic_rate_general_flag
    b.ue(0); // elemental_duration_in_tc_minus1
    b.ue(0); // cpb_cnt_minus1
    b.ue(999); // bit_rate_value_minus1
    b.ue(1999); // cpb_size_value_minus1
    b.bit(0); // cbr_flag
}

pub fn vps(timing: bool) -> Vec<u8> {
    let mut b = BitWriter::default();
    b.u(0, 4);
    b.bit(1);
    b.bit(1);
    b.u(0, 6);
    b.u(0, 3);
    b.bit(1);
    b.u(0xffff, 16);
    ptl(&mut b, 0);
    b.bit(1); // vps_sub_layer_ordering_info_present_flag
    b.ue(1);
    b.ue(0);
    b.ue(0);
    b.u(0, 6); // vps_max_layer_id
    b.ue(0); // vps_num_layer_sets_minus1
    b.flag(timing);
    if timing {
        b.u(1001, 32);
        b.u(30000, 32);
        b.bit(0);
        b.ue(1); // vps_num_hrd_parameters
        b.ue(0); // hrd_layer_set_idx
        hrd(&mut b);
    }
    b.bit(0); // vps_extension_flag
    b.trailing();
    nal(32, &b.out)
}

#[derive(Clone)]
pub struct SpsCfg {
    pub width: u32,
    pub height: u32,
    pub chroma_format_idc: u32,
    pub bit_depth: u32,
    pub log2_min_cb: u32,
    pub log2_ctb: u32,
    pub log2_min_tb: u32,
    pub log2_max_tb: u32,
    pub depth_intra: u32,
    pub sao: bool,
    /// (bit depth luma, chroma, log2 min, log2 max, loop filter disabled)
    pub pcm: Option<(u32, u32, u32, u32, bool)>,
    pub strong: bool,
    pub conf_win: Option<[u32; 4]>,
}

impl Default for SpsCfg {
    fn default() -> Self {
        SpsCfg {
            width: 64,
            height: 48,
            chroma_format_idc: 1,
            bit_depth: 8,
            log2_min_cb: 3,
            log2_ctb: 6,
            log2_min_tb: 2,
            log2_max_tb: 5,
            depth_intra: 0,
            sao: false,
            pcm: None,
            strong: false,
            conf_win: None,
        }
    }
}

pub fn sps_rbsp(c: &SpsCfg) -> Vec<u8> {
    let mut b = BitWriter::default();
    b.u(0, 4);
    b.u(0, 3);
    b.bit(1);
    ptl(&mut b, 0);
    b.ue(0);
    b.ue(c.chroma_format_idc);
    if c.chroma_format_idc == 3 {
        b.bit(0);
    }
    b.ue(c.width);
    b.ue(c.height);
    match c.conf_win {
        Some(w) => {
            b.bit(1);
            for v in w {
                b.ue(v);
            }
        }
        None => b.bit(0),
    }
    b.ue(c.bit_depth - 8);
    b.ue(c.bit_depth - 8);
    b.ue(4); // log2_max_pic_order_cnt_lsb_minus4
    b.bit(1);
    b.ue(0);
    b.ue(0);
    b.ue(0);
    b.ue(c.log2_min_cb - 3);
    b.ue(c.log2_ctb - c.log2_min_cb);
    b.ue(c.log2_min_tb - 2);
    b.ue(c.log2_max_tb - c.log2_min_tb);
    b.ue(0);
    b.ue(c.depth_intra);
    b.bit(0); // scaling_list_enabled_flag
    b.bit(0); // amp
    b.flag(c.sao);
    match c.pcm {
        Some((bl, bc, lmin, lmax, lf)) => {
            b.bit(1);
            b.u((bl - 1) as u64, 4);
            b.u((bc - 1) as u64, 4);
            b.ue(lmin - 3);
            b.ue(lmax - lmin);
            b.flag(lf);
        }
        None => b.bit(0),
    }
    b.ue(0); // num_short_term_ref_pic_sets
    b.bit(0); // long_term_ref_pics_present_flag
    b.bit(0); // sps_temporal_mvp_enabled_flag
    b.flag(c.strong);
    b.bit(0); // vui
    b.bit(0); // extension
    b.trailing();
    b.out
}

pub fn sps(c: &SpsCfg) -> Vec<u8> {
    nal(33, &sps_rbsp(c))
}

#[derive(Clone, Default)]
pub struct PpsCfg {
    pub init_qp: i32,
    pub sign_hiding: bool,
    pub deblocking_disabled: bool,
    pub cb_qp_offset: i32,
}

pub fn pps_rbsp(c: &PpsCfg) -> Vec<u8> {
    let mut b = BitWriter::default();
    b.ue(0);
    b.ue(0);
    b.bit(0); // dependent_slice_segments_enabled_flag
    b.bit(0); // output_flag_present_flag
    b.u(0, 3);
    b.flag(c.sign_hiding);
    b.bit(0); // cabac_init_present_flag
    b.ue(0);
    b.ue(0);
    b.se(c.init_qp - 26);
    b.bit(0); // constrained_intra_pred_flag
    b.bit(0); // transform_skip_enabled_flag
    b.bit(0); // cu_qp_delta_enabled_flag
    b.se(c.cb_qp_offset);
    b.se(0);
    b.bit(0);
    b.bit(0);
    b.bit(0);
    b.bit(0); // transquant_bypass_enabled_flag
    b.bit(0); // tiles
    b.bit(0); // entropy_coding_sync
    b.bit(1); // pps_loop_filter_across_slices_enabled_flag
    if c.deblocking_disabled {
        b.bit(1); // deblocking_filter_control_present_flag
        b.bit(0); // override enabled
        b.bit(1); // pps_deblocking_filter_disabled_flag
    } else {
        b.bit(0);
    }
    b.bit(0); // pps_scaling_list_data_present_flag
    b.bit(0); // lists_modification_present_flag
    b.ue(0);
    b.bit(0);
    b.bit(0);
    b.trailing();
    b.out
}

pub fn pps(c: &PpsCfg) -> Vec<u8> {
    nal(34, &pps_rbsp(c))
}

/// The header of an IDR_W_RADL I slice (the whole picture, PPS 0, slice_qp_delta 0) through byte_alignment().
pub fn idr_slice_header(sao_luma: bool, sao_chroma: bool, sao_enabled: bool, deblocking_disabled: bool) -> BitWriter {
    let mut b = BitWriter::default();
    b.bit(1); // first_slice_segment_in_pic_flag
    b.bit(0); // no_output_of_prior_pics_flag
    b.ue(0);
    b.ue(2); // slice_type I
    if sao_enabled {
        b.flag(sao_luma);
        b.flag(sao_chroma);
    }
    b.se(0); // slice_qp_delta
    if sao_luma || sao_chroma || !deblocking_disabled {
        b.bit(1); // slice_loop_filter_across_slices_enabled_flag
    }
    b.trailing();
    b
}
