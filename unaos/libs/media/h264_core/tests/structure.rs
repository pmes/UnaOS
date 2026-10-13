// SPDX-License-Identifier: LGPL-3.0-or-later
//! H264CORE decode-structure KATs over hand-built streams (no reference encoder exists on the build
//! host, so each stream is written here bit by bit, and the CABAC one through an arithmetic
//! ENCODER written from 9.3.4.2 .. 9.3.4.6): an all-Intra_16x16-DC frame decodes flat (128, the
//! "no neighbours" DC of 8.3.3.3); a P frame of P_Skip macroblocks copies its reference; an I / P /
//! non-reference B sequence comes out in POC order; a CABAC I_PCM macroblock round-trips its
//! samples; the avcC + length-prefixed path decodes what the Annex B path does; 4:2:2 and field
//! pictures are refused by name; the Exp-Golomb, emulation-prevention and framing helpers keep
//! their contracts.

use h264_core::bits::BitReader;
use h264_core::tables::{CABAC_INIT_MN, RANGE_TAB_LPS, TRANS_IDX_LPS, TRANS_IDX_MPS};
use h264_core::{nal, Decoder, Error, Frame};

#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    n: usize,
}

impl BitWriter {
    fn bit(&mut self, b: u32) {
        if self.n % 8 == 0 {
            self.out.push(0);
        }
        if b != 0 {
            *self.out.last_mut().unwrap() |= 0x80 >> (self.n % 8);
        }
        self.n += 1;
    }
    fn u(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1);
        }
    }
    fn ue(&mut self, v: u32) {
        let x = v as u64 + 1;
        let len = 64 - x.leading_zeros();
        for _ in 0..len - 1 {
            self.bit(0);
        }
        for i in (0..len).rev() {
            self.bit(((x >> i) & 1) as u32);
        }
    }
    fn se(&mut self, v: i32) {
        self.ue(if v > 0 { (2 * v - 1) as u32 } else { (-2 * v) as u32 });
    }
    fn aligned(&self) -> bool {
        self.n % 8 == 0
    }
    /// rbsp_trailing_bits (7.3.2.11).
    fn trailing(&mut self) {
        self.bit(1);
        while !self.aligned() {
            self.bit(0);
        }
    }
}

/// A NAL unit as Annex B bytes: start code, header, the RBSP with emulation prevention (7.4.1).
fn nal_unit(ref_idc: u8, ty: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 1, (ref_idc << 5) | ty];
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

/// seq_parameter_set_rbsp: profile `profile`, POC type 0 with 4-bit lsb, 4-bit frame_num, w x h
/// macroblocks, frame_mbs_only_flag 1 unless `fields`.
fn sps(profile: u32, w: u32, h: u32, refs: u32, chroma_422: bool, fields: bool) -> Vec<u8> {
    let mut b = BitWriter::default();
    b.u(profile, 8);
    b.u(0, 8);
    b.u(30, 8);
    b.ue(0);
    if profile == 100 || profile == 122 {
        b.ue(if chroma_422 { 2 } else { 1 });
        b.ue(0);
        b.ue(0);
        b.bit(0);
        b.bit(0);
    }
    b.ue(0); // log2_max_frame_num_minus4
    b.ue(0); // pic_order_cnt_type
    b.ue(0); // log2_max_pic_order_cnt_lsb_minus4
    b.ue(refs);
    b.bit(0);
    b.ue(w - 1);
    b.ue(h - 1);
    if fields {
        b.bit(0); // frame_mbs_only_flag
        b.bit(0); // mb_adaptive_frame_field_flag
    } else {
        b.bit(1);
    }
    b.bit(1); // direct_8x8_inference_flag
    b.bit(0);
    b.bit(0);
    b.trailing();
    nal_unit(3, 7, &b.out)
}

fn pps(cabac: bool) -> Vec<u8> {
    let mut b = BitWriter::default();
    b.ue(0);
    b.ue(0);
    b.bit(cabac as u32);
    b.bit(0);
    b.ue(0);
    b.ue(0);
    b.ue(0);
    b.bit(0);
    b.u(0, 2);
    b.se(0);
    b.se(0);
    b.se(0);
    b.bit(0); // deblocking_filter_control_present_flag
    b.bit(0);
    b.bit(0);
    b.trailing();
    nal_unit(3, 8, &b.out)
}

/// The start of an IDR I slice header (first_mb 0, frame_num 0, POC lsb 0, QP 26).
fn idr_header(b: &mut BitWriter) {
    b.ue(0);
    b.ue(7);
    b.ue(0);
    b.u(0, 4);
    b.ue(0); // idr_pic_id
    b.u(0, 4); // pic_order_cnt_lsb
    b.bit(0); // no_output_of_prior_pics_flag
    b.bit(0); // long_term_reference_flag
    b.se(0); // slice_qp_delta
}

/// A w x h-macroblock IDR frame of Intra_16x16 DC macroblocks with no residual (CAVLC).
fn flat_idr(w: u32, h: u32) -> Vec<u8> {
    let mut b = BitWriter::default();
    idr_header(&mut b);
    for _ in 0..w * h {
        b.ue(3); // mb_type I_16x16_2_0_0: Intra16x16PredMode 2 (DC), no coded blocks
        b.ue(0); // intra_chroma_pred_mode DC
        b.se(0); // mb_qp_delta
        b.bit(1); // Intra16x16DCLevel coeff_token: TotalCoeff 0 (nC 0)
    }
    b.trailing();
    nal_unit(3, 5, &b.out)
}

/// A pattern for I_PCM macroblock `m` of a w-macroblock-wide frame: (luma, cb, cr) at (x, y).
fn pattern(x: usize, y: usize) -> [u8; 3] {
    [((x * 7 + y * 3) & 255) as u8, (64 + x / 2) as u8, (192 - y / 2) as u8]
}

/// An IDR frame of I_PCM macroblocks carrying [`pattern`] (CAVLC).
fn pcm_idr(w: u32, h: u32) -> Vec<u8> {
    let mut b = BitWriter::default();
    idr_header(&mut b);
    for m in 0..(w * h) as usize {
        let (mx, my) = ((m % w as usize) * 16, (m / w as usize) * 16);
        b.ue(25); // I_PCM
        while !b.aligned() {
            b.bit(0);
        }
        for y in 0..16 {
            for x in 0..16 {
                b.u(pattern(mx + x, my + y)[0] as u32, 8);
            }
        }
        for c in 1..3 {
            for y in 0..8 {
                for x in 0..8 {
                    b.u(pattern(mx + 2 * x, my + 2 * y)[c] as u32, 8);
                }
            }
        }
    }
    b.trailing();
    nal_unit(3, 5, &b.out)
}

/// A P frame (frame_num 1, POC lsb `poc`) whose macroblocks are all P_Skip.
fn skip_p(total: u32, poc: u32) -> Vec<u8> {
    let mut b = BitWriter::default();
    b.ue(0);
    b.ue(5);
    b.ue(0);
    b.u(1, 4);
    b.u(poc, 4);
    b.bit(0); // num_ref_idx_active_override_flag
    b.bit(0); // ref_pic_list_modification_flag_l0
    b.bit(0); // adaptive_ref_pic_marking_mode_flag
    b.se(0);
    b.ue(total); // mb_skip_run
    b.trailing();
    nal_unit(2, 1, &b.out)
}

/// A non-reference B frame (frame_num 2, POC lsb `poc`) whose macroblocks are all B_Skip
/// (spatial direct).
fn skip_b(total: u32, poc: u32) -> Vec<u8> {
    let mut b = BitWriter::default();
    b.ue(0);
    b.ue(6);
    b.ue(0);
    b.u(2, 4);
    b.u(poc, 4);
    b.bit(1); // direct_spatial_mv_pred_flag
    b.bit(0); // num_ref_idx_active_override_flag
    b.bit(0); // ref_pic_list_modification_flag_l0
    b.bit(0); // ref_pic_list_modification_flag_l1
    b.se(0);
    b.ue(total);
    b.trailing();
    nal_unit(0, 1, &b.out)
}

fn decode(stream: &[u8]) -> Result<Vec<Frame>, Error> {
    let mut d = Decoder::new();
    d.decode_annexb(stream)?;
    d.flush();
    Ok(d.take_frames())
}

fn pattern_frame(w: usize, h: usize) -> [Vec<u8>; 3] {
    let mut p = [Vec::new(), Vec::new(), Vec::new()];
    for y in 0..h {
        for x in 0..w {
            p[0].push(pattern(x, y)[0]);
        }
    }
    for c in 1..3 {
        for y in 0..h / 2 {
            for x in 0..w / 2 {
                p[c].push(pattern(2 * x, 2 * y)[c]);
            }
        }
    }
    p
}

#[test]
fn intra16x16_dc_frame_decodes_flat() {
    let s = [sps(66, 3, 2, 1, false, false), pps(false), flat_idr(3, 2)].concat();
    let f = decode(&s).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].width, f[0].height), (48, 32));
    assert!(f[0].planes[0].iter().all(|&v| v == 128), "luma not flat 128");
    assert!(f[0].planes[1].iter().chain(&f[0].planes[2]).all(|&v| v == 128), "chroma not flat 128");
    assert_eq!(f[0].poc, 0);
}

#[test]
fn pcm_frame_round_trips_and_p_skip_copies_it() {
    let s = [sps(66, 2, 2, 1, false, false), pps(false), pcm_idr(2, 2), skip_p(4, 2)].concat();
    let f = decode(&s).unwrap();
    assert_eq!(f.len(), 2);
    let want = pattern_frame(32, 32);
    for (i, fr) in f.iter().enumerate() {
        assert_eq!(fr.planes, want, "frame {i} (I_PCM, then P_Skip) is not the pattern");
    }
    assert_eq!((f[0].poc, f[1].poc), (0, 2));
}

#[test]
fn frames_leave_in_poc_order() {
    // Decoding order I (POC 0), P (POC 4), non-reference B (POC 2); output order I, B, P.
    let s = [sps(77, 2, 2, 2, false, false), pps(false), pcm_idr(2, 2), skip_p(4, 4), skip_b(4, 2)].concat();
    let f = decode(&s).unwrap();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4]);
    let want = pattern_frame(32, 32);
    for fr in &f {
        assert_eq!(fr.planes, want, "POC {}: skipped P / B frames copy the I_PCM pattern", fr.poc);
    }
}

/// The CABAC arithmetic encoder (9.3.4.2 .. 9.3.4.6), the inverse of the decoder under test.
struct CabacEncoder<'a> {
    w: &'a mut BitWriter,
    low: u32,
    range: u32,
    outstanding: u32,
    first: bool,
    state: Vec<(u8, u8)>,
}

impl<'a> CabacEncoder<'a> {
    fn new(w: &'a mut BitWriter, qp: i32) -> Self {
        let state = CABAC_INIT_MN[0]
            .iter()
            .map(|&(m, n)| {
                let pre = (((m as i32 * qp.clamp(0, 51)) >> 4) + n as i32).clamp(1, 126);
                if pre <= 63 { ((63 - pre) as u8, 0) } else { ((pre - 64) as u8, 1) }
            })
            .collect();
        CabacEncoder { w, low: 0, range: 510, outstanding: 0, first: true, state }
    }
    fn put_bit(&mut self, b: u32) {
        if self.first {
            self.first = false;
        } else {
            self.w.bit(b);
        }
        while self.outstanding > 0 {
            self.w.bit(1 - b);
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
    fn decision(&mut self, ctx: usize, bin: u32) {
        let (p, mps) = self.state[ctx];
        let lps = RANGE_TAB_LPS[p as usize][((self.range >> 6) & 3) as usize] as u32;
        self.range -= lps;
        if bin != mps as u32 {
            self.low += self.range;
            self.range = lps;
            let nmps = if p == 0 { 1 - mps } else { mps };
            self.state[ctx] = (TRANS_IDX_LPS[p as usize], nmps);
        } else {
            self.state[ctx] = (TRANS_IDX_MPS[p as usize], mps);
        }
        self.renorm();
    }
    /// EncodeTerminate(1) followed by EncodeFlush (9.3.4.5).
    fn terminate_and_flush(&mut self) {
        self.range -= 2;
        self.low += self.range;
        self.range = 2;
        self.renorm();
        self.put_bit((self.low >> 9) & 1);
        let v = ((self.low >> 7) & 3) | 1;
        self.w.u(v, 2);
    }
}

#[test]
fn cabac_i_pcm_macroblock_round_trips() {
    let mut b = BitWriter::default();
    idr_header(&mut b);
    while !b.aligned() {
        b.bit(1); // cabac_alignment_one_bit
    }
    {
        let mut e = CabacEncoder::new(&mut b, 26);
        e.decision(3, 1); // mb_type bin 0 (ctxIdx 3 + 0: no neighbours): not I_NxN
        e.terminate_and_flush(); // bin 1 on ctxIdx 276: I_PCM
    }
    while !b.aligned() {
        b.bit(0); // pcm_alignment_zero_bit
    }
    for y in 0..16 {
        for x in 0..16 {
            b.u(pattern(x, y)[0] as u32, 8);
        }
    }
    for c in 1..3 {
        for y in 0..8 {
            for x in 0..8 {
                b.u(pattern(2 * x, 2 * y)[c] as u32, 8);
            }
        }
    }
    {
        // The engine restarts after the samples; end_of_slice_flag = 1, whose flush ends with
        // the rbsp_stop_one_bit.
        let mut e = CabacEncoder::new(&mut b, 26);
        e.terminate_and_flush();
    }
    while !b.aligned() {
        b.bit(0);
    }
    let s = [sps(77, 1, 1, 1, false, false), pps(true), nal_unit(3, 5, &b.out)].concat();
    let f = decode(&s).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].planes, pattern_frame(16, 16));
}

#[test]
fn avcc_samples_decode_like_annexb() {
    let (sps_b, pps_b) = (sps(66, 2, 2, 1, false, false), pps(false));
    let strip = |v: &[u8]| v[4..].to_vec();
    let mut avcc = vec![1, 66, 0, 30, 0xff, 0xe1];
    avcc.extend_from_slice(&(strip(&sps_b).len() as u16).to_be_bytes());
    avcc.extend_from_slice(&strip(&sps_b));
    avcc.push(1);
    avcc.extend_from_slice(&(strip(&pps_b).len() as u16).to_be_bytes());
    avcc.extend_from_slice(&strip(&pps_b));
    let mut d = Decoder::new();
    d.configure_avcc(&avcc).unwrap();
    let mut got = Vec::new();
    for (tag, au) in [(0i64, pcm_idr(2, 2)), (40, skip_p(4, 2))] {
        let n = strip(&au);
        let mut sample = (n.len() as u32).to_be_bytes().to_vec();
        sample.extend_from_slice(&n);
        d.set_tag(tag);
        d.decode_sample(&sample).unwrap();
        got.extend(d.take_frames());
    }
    d.flush();
    got.extend(d.take_frames());
    let annexb = decode(&[sps_b, pps_b, pcm_idr(2, 2), skip_p(4, 2)].concat()).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got.iter().map(|f| &f.planes).collect::<Vec<_>>(), annexb.iter().map(|f| &f.planes).collect::<Vec<_>>());
    assert_eq!(got.iter().map(|f| f.tag).collect::<Vec<_>>(), vec![0, 40]);
}

#[test]
fn chroma_422_and_field_pictures_are_refused_by_name() {
    let s = [sps(122, 1, 1, 1, true, false), pps(false), flat_idr(1, 1)].concat();
    match decode(&s) {
        Err(Error::Unsupported(m)) => assert!(m.contains("4:2:2"), "{m}"),
        other => panic!("4:2:2 not refused: {other:?}"),
    }
    // frame_mbs_only_flag 0 and a slice with field_pic_flag 1.
    let mut b = BitWriter::default();
    b.ue(0);
    b.ue(7);
    b.ue(0);
    b.u(0, 4);
    b.bit(1); // field_pic_flag
    b.bit(0); // bottom_field_flag
    b.ue(0);
    b.u(0, 4);
    b.bit(0);
    b.bit(0);
    b.se(0);
    b.trailing();
    let s = [sps(77, 1, 1, 1, false, true), pps(false), nal_unit(3, 5, &b.out)].concat();
    match decode(&s) {
        Err(Error::Unsupported(m)) => assert!(m.contains("interlace"), "{m}"),
        other => panic!("field picture not refused: {other:?}"),
    }
}

#[test]
fn exp_golomb_and_framing_helpers() {
    let mut b = BitWriter::default();
    for v in [0u32, 1, 2, 3, 7, 254, 65535] {
        b.ue(v);
    }
    for v in [0i32, 1, -1, 2, -2, 1000, -1000] {
        b.se(v);
    }
    b.trailing();
    let mut r = BitReader::new(&b.out);
    for v in [0u32, 1, 2, 3, 7, 254, 65535] {
        assert_eq!(r.ue(), v);
    }
    for v in [0i32, 1, -1, 2, -2, 1000, -1000] {
        assert_eq!(r.se(), v);
    }
    assert!(!r.more_rbsp_data());
    let mut out = Vec::new();
    nal::unescape(&[0, 0, 3, 1, 0, 0, 3, 0, 0, 3], &mut out);
    assert_eq!(out, vec![0, 0, 1, 0, 0, 0, 0]);
    let units = nal::annexb_units(&[9, 0, 0, 1, 0x67, 5, 0, 0, 0, 1, 0x68, 7, 0, 0, 1, 0x65]);
    assert_eq!(units, vec![&[0x67u8, 5][..], &[0x68, 7][..], &[0x65][..]]);
    let units = nal::length_prefixed_units(&[0, 2, 0x67, 1, 0, 1, 0x68], 2).unwrap();
    assert_eq!(units, vec![&[0x67u8, 1][..], &[0x68][..]]);
    assert_eq!(nal::length_prefixed_units(&[0, 9, 1], 2), Err(Error::Truncated));
}
