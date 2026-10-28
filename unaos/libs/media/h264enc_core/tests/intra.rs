// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENC M1 (LEDGER SR197): I frames. Each access unit is decoded by the tree's own h264_core
//! decoder; the decode must equal the encoder's reconstruction byte for byte, sit within the
//! quantiser's bound of the source, and its SPS / PPS / slice header must read back as written.

use h264_core::bits::BitReader;
use h264_core::nal;
use h264_core::ps::{self, SliceType};
use h264_core::{Decoder, Frame};
use h264enc_core::{Access, Config, Encoder, Error, Yuv420};

struct Img {
    w: usize,
    h: usize,
    p: [Vec<u8>; 3],
}

impl Img {
    fn yuv(&self) -> Yuv420<'_> {
        Yuv420 { width: self.w as u32, height: self.h as u32, y: &self.p[0], u: &self.p[1], v: &self.p[2], y_stride: self.w, uv_stride: self.w / 2 }
    }
}

/// A smooth test pattern: a diagonal luma gradient with a soft bump, chroma ramps.
fn smooth(w: usize, h: usize) -> Img {
    let mut p = [vec![0u8; w * h], vec![0u8; w * h / 4], vec![0u8; w * h / 4]];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as i32 - w as i32 / 2, y as i32 - h as i32 / 3);
            let bump = 40 - ((dx * dx + dy * dy) / 24).min(40);
            p[0][y * w + x] = (30 + (x * 140 / w) as i32 + (y * 50 / h) as i32 + bump).clamp(0, 255) as u8;
        }
    }
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            p[1][y * (w / 2) + x] = (96 + x * 64 / (w / 2)) as u8;
            p[2][y * (w / 2) + x] = (160 - y * 48 / (h / 2)) as u8;
        }
    }
    Img { w, h, p }
}

fn flat(w: usize, h: usize, v: [u8; 3]) -> Img {
    Img { w, h, p: [vec![v[0]; w * h], vec![v[1]; w * h / 4], vec![v[2]; w * h / 4]] }
}

fn decode_one(dec: &mut Decoder, au: &Access) -> Frame {
    dec.decode_annexb(&au.annexb()).unwrap();
    dec.end_access_unit();
    let mut f = dec.take_frames();
    assert_eq!(f.len(), 1, "one frame per access unit");
    f.pop().unwrap()
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let se: f64 = a.iter().zip(b).map(|(&x, &y)| ((x as f64) - (y as f64)).powi(2)).sum();
    if se == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / (se / a.len() as f64)).log10()
}

fn assert_decode_is_recon(f: &Frame, enc: &Encoder, w: usize, h: usize) {
    assert_eq!((f.width as usize, f.height as usize), (w, h));
    let rec = enc.reconstruction();
    for c in 0..3 {
        assert_eq!(f.planes[c].as_slice(), rec[c], "plane {c}: h264_core's decode differs from the encoder's reconstruction");
    }
}

fn encode_intra(img: &Img, qp: u8) -> (Encoder, Access) {
    let mut enc = Encoder::new(Config { width: img.w as u32, height: img.h as u32, qp, gop: 1, ..Default::default() }).unwrap();
    let au = enc.encode(&img.yuv()).unwrap();
    (enc, au)
}

#[test]
fn m1_smooth_pattern_qp26_decodes_to_the_reconstruction_within_35db() {
    let img = smooth(64, 48);
    let (enc, au) = encode_intra(&img, 26);
    assert!(au.keyframe);
    let mut dec = Decoder::new();
    let f = decode_one(&mut dec, &au);
    assert_decode_is_recon(&f, &enc, 64, 48);
    for c in 0..3 {
        let db = psnr(&f.planes[c], &img.p[c]);
        println!("plane {c}: PSNR {db:.2} dB, {} bytes", au.annexb().len());
        assert!(db >= 35.0, "plane {c}: {db:.2} dB < 35");
    }
}

#[test]
fn m1_flat_frame_decodes_bit_exact_to_the_source() {
    // Y 144 / Cb 144 / Cr 112: residual +-16 from the 128 of the first macroblock's DC
    // prediction, where both the Intra_4x4 and the Intra_16x16 / chroma DC quantisers at QP 26
    // reconstruct exactly (level 5 / 20 / 10 -> +-16); every later block predicts the value itself.
    let img = flat(64, 48, [144, 144, 112]);
    let (enc, au) = encode_intra(&img, 26);
    let mut dec = Decoder::new();
    let f = decode_one(&mut dec, &au);
    assert_decode_is_recon(&f, &enc, 64, 48);
    for c in 0..3 {
        assert_eq!(f.planes[c], img.p[c], "plane {c} not bit-exact");
    }
}

#[test]
fn m1_qp_16_and_40_decode_to_the_reconstruction_and_bytes_fall_with_qp() {
    let img = smooth(64, 48);
    let mut sizes = Vec::new();
    for qp in [16u8, 26, 40] {
        let (enc, au) = encode_intra(&img, qp);
        let mut dec = Decoder::new();
        let f = decode_one(&mut dec, &au);
        assert_decode_is_recon(&f, &enc, 64, 48);
        let db = psnr(&f.planes[0], &img.p[0]);
        println!("qp {qp}: {} bytes, Y PSNR {db:.2} dB", au.annexb().len());
        sizes.push(au.annexb().len());
    }
    assert!(sizes[0] > sizes[1] && sizes[1] > sizes[2], "bytes by QP 16/26/40: {sizes:?}");
}

#[test]
fn m1_cropped_frame_50x34_decodes_to_50x34() {
    let img = smooth(50, 34);
    let (enc, au) = encode_intra(&img, 26);
    let mut dec = Decoder::new();
    let f = decode_one(&mut dec, &au);
    assert_decode_is_recon(&f, &enc, 50, 34);
    assert_eq!((f.plane_size(1)), (25, 17));
    assert!(psnr(&f.planes[0], &img.p[0]) >= 35.0);
}

#[test]
fn m1_conformance_parameter_sets_and_slice_header_read_back() {
    let img = smooth(50, 34);
    let (enc, au) = encode_intra(&img, 30);
    assert_eq!(au.nals.len(), 3);
    let mut rbsp = Vec::new();
    // SPS
    let h = nal::parse_header(au.nals[0][0]).unwrap();
    assert_eq!((h.nal_type, h.ref_idc), (nal::NAL_SPS, 3));
    nal::unescape(&au.nals[0][1..], &mut rbsp);
    assert_eq!(rbsp[1] & 0xC0, 0xC0, "constraint_set0_flag and constraint_set1_flag (Constrained Baseline)");
    let sps = ps::parse_sps(&rbsp).unwrap();
    assert_eq!(sps.profile_idc, 66);
    assert!(!sps.constraint_set3);
    assert_eq!(sps.level_idc, 10, "4 x 3 macroblocks fit level 1 (MaxFS 99)");
    assert_eq!((sps.width_mbs, sps.height_mbs()), (4, 3));
    assert_eq!(sps.crop, [0, 7, 0, 7]);
    assert_eq!(sps.crop_rect(), (0, 0, 50, 34));
    assert_eq!((sps.poc_type, sps.max_num_ref_frames, sps.chroma_format_idc), (2, 1, 1));
    assert!(sps.frame_mbs_only && sps.direct_8x8_inference && !sps.gaps_allowed);
    assert_eq!((sps.max_num_reorder, sps.max_dec_frame_buffering), (Some(0), Some(1)));
    // PPS
    assert_eq!(nal::parse_header(au.nals[1][0]).unwrap().nal_type, nal::NAL_PPS);
    nal::unescape(&au.nals[1][1..], &mut rbsp);
    let pps = ps::parse_pps(&rbsp, |_| 1).unwrap();
    assert_eq!((pps.id, pps.sps_id, pps.cabac, pps.num_slice_groups), (0, 0, false, 1));
    assert_eq!((pps.pic_init_qp, pps.chroma_qp_offset), (30, [0, 0]));
    assert!(pps.deblocking_filter_control_present && !pps.constrained_intra_pred && !pps.transform_8x8_mode);
    // Slice header
    let h = nal::parse_header(au.nals[2][0]).unwrap();
    assert_eq!((h.nal_type, h.ref_idc), (nal::NAL_IDR, 3));
    nal::unescape(&au.nals[2][1..], &mut rbsp);
    let mut r = BitReader::new(&rbsp);
    let sh = ps::parse_slice_header(&mut r, h.nal_type, h.ref_idc, |_| Some(&pps), |_| Some(&sps)).unwrap();
    assert_eq!((sh.slice_type, sh.idr, sh.first_mb, sh.frame_num, sh.idr_pic_id), (SliceType::I, true, 0, 0, 0));
    assert_eq!((sh.qp, sh.disable_deblocking), (30, 1));
    assert!(!sh.no_output_of_prior_pics && !sh.long_term_reference);
    // avcC round trip
    let c = nal::parse_avcc(&enc.avcc()).unwrap();
    assert_eq!((c.profile_idc, c.level_idc, c.nal_length_size), (66, 10, 4));
    assert_eq!(c.sps, vec![enc.sps_nal().to_vec()]);
    assert_eq!(c.pps, vec![enc.pps_nal().to_vec()]);
    assert_eq!((c.sps[0].as_slice(), c.pps[0].as_slice()), (au.nals[0].as_slice(), au.nals[1].as_slice()));
}

/// A busy frame (hard edges and noise): Intra_4x4 wins, the levels are large and every nC class
/// is reached. The first macroblock's mb_type is read from the slice data to show the 4x4 path ran.
fn textured(w: usize, h: usize) -> Img {
    let mut seed = 0x9e37_79b9u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut p = [vec![0u8; w * h], vec![0u8; w * h / 4], vec![0u8; w * h / 4]];
    for y in 0..h {
        for x in 0..w {
            let edge = if (x / 5 + y / 3) % 2 == 0 { 200 } else { 40 };
            p[0][y * w + x] = (edge + (rnd() % 31) as i32 - 15) as u8;
        }
    }
    for k in 0..w * h / 4 {
        p[1][k] = (64 + rnd() % 128) as u8;
        p[2][k] = if (k / 3) % 2 == 0 { 220 } else { 30 };
    }
    Img { w, h, p }
}

#[test]
fn m1_textured_frame_takes_intra4x4_and_decodes_to_the_reconstruction() {
    let img = textured(64, 48);
    for qp in [16u8, 26, 40] {
        let (enc, au) = encode_intra(&img, qp);
        let mut dec = Decoder::new();
        let f = decode_one(&mut dec, &au);
        assert_decode_is_recon(&f, &enc, 64, 48);
        let mut rbsp = Vec::new();
        nal::unescape(&au.nals[2][1..], &mut rbsp);
        let mut sps_rbsp = Vec::new();
        nal::unescape(&au.nals[0][1..], &mut sps_rbsp);
        let sps = ps::parse_sps(&sps_rbsp).unwrap();
        let mut pps_rbsp = Vec::new();
        nal::unescape(&au.nals[1][1..], &mut pps_rbsp);
        let pps = ps::parse_pps(&pps_rbsp, |_| 1).unwrap();
        let mut r = BitReader::new(&rbsp);
        let sh = ps::parse_slice_header(&mut r, nal::NAL_IDR, 3, |_| Some(&pps), |_| Some(&sps)).unwrap();
        r.set_pos(sh.data_bit);
        assert_eq!(r.ue(), 0, "qp {qp}: macroblock 0 is I_NxN (Intra_4x4)");
        println!("textured qp {qp}: {} bytes, Y PSNR {:.2} dB", au.annexb().len(), psnr(&f.planes[0], &img.p[0]));
    }
}

#[test]
fn m1_configuration_refusals() {
    let cfg = Config { width: 64, height: 48, qp: 26, gop: 10, ..Default::default() };
    assert_eq!(Encoder::new(Config { width: 63, ..cfg }).err(), Some(Error::Geometry));
    assert_eq!(Encoder::new(Config { height: 0, ..cfg }).err(), Some(Error::Geometry));
    assert_eq!(Encoder::new(Config { qp: 15, ..cfg }).err(), Some(Error::Qp));
    assert_eq!(Encoder::new(Config { qp: 41, ..cfg }).err(), Some(Error::Qp));
    assert_eq!(Encoder::new(Config { gop: 0, ..cfg }).err(), Some(Error::Gop));
    let mut enc = Encoder::new(cfg).unwrap();
    let img = smooth(32, 48);
    assert_eq!(enc.encode(&img.yuv()).err(), Some(Error::FrameSize));
}

/// H264ENC2 M1 (LEDGER SR211): with the deblocking filter on, the slice header reads back as
/// disable_deblocking_filter_idc 0 with zero offsets and the filtered I frame decodes to the
/// reconstruction byte for byte.
#[test]
fn m1_deblock_slice_header_reads_back_idc0() {
    let img = textured(64, 48);
    let mut enc = Encoder::new(Config { width: 64, height: 48, qp: 32, gop: 1, deblock: true, ..Default::default() }).unwrap();
    let au = enc.encode(&img.yuv()).unwrap();
    let mut dec = Decoder::new();
    let f = decode_one(&mut dec, &au);
    assert_decode_is_recon(&f, &enc, 64, 48);
    let mut rbsp = Vec::new();
    nal::unescape(&au.nals[0][1..], &mut rbsp);
    let sps = ps::parse_sps(&rbsp).unwrap();
    nal::unescape(&au.nals[1][1..], &mut rbsp);
    let pps = ps::parse_pps(&rbsp, |_| 1).unwrap();
    let h = nal::parse_header(au.nals[2][0]).unwrap();
    nal::unescape(&au.nals[2][1..], &mut rbsp);
    let mut r = BitReader::new(&rbsp);
    let sh = ps::parse_slice_header(&mut r, h.nal_type, h.ref_idc, |_| Some(&pps), |_| Some(&sps)).unwrap();
    assert_eq!((sh.disable_deblocking, sh.alpha_offset, sh.beta_offset), (0, 0, 0));
}

/// H264ENCCABAC (LEDGER SR310): the I-frame scenes above under CABAC (Main profile) decode to the
/// encoder's reconstruction byte for byte: smooth, flat (bit-exact to the source), textured at QP
/// 16 / 26 / 40, the cropped 50x34 frame and the deblocked textured frame.
#[test]
fn cabac_every_intra_scene_decodes_to_the_reconstruction() {
    let scenes = [("smooth", smooth(64, 48)), ("flat", flat(64, 48, [144, 144, 112])), ("textured", textured(64, 48)), ("crop", smooth(50, 34))];
    for (name, img) in &scenes {
        for qp in [16u8, 26, 40] {
            for deblock in [false, true] {
                let base = Config { width: img.w as u32, height: img.h as u32, qp, gop: 1, deblock, ..Default::default() };
                let mut cv = Encoder::new(base).unwrap();
                let cv_au = cv.encode(&img.yuv()).unwrap();
                let mut enc = Encoder::new(Config { entropy: h264enc_core::Entropy::Cabac, ..base }).unwrap();
                let au = enc.encode(&img.yuv()).unwrap();
                let mut dec = Decoder::new();
                let f = decode_one(&mut dec, &au);
                assert_decode_is_recon(&f, &enc, img.w, img.h);
                assert_eq!(enc.reconstruction(), cv.reconstruction(), "{name} qp {qp}: the entropy coder moved the reconstruction");
                if *name == "flat" && qp == 26 && !deblock {
                    for c in 0..3 {
                        assert_eq!(f.planes[c], img.p[c], "cabac flat plane {c} not bit-exact");
                    }
                }
                println!("cabac intra {name} qp {qp} deblock {deblock}: cavlc={} cabac={} bytes", cv_au.annexb().len(), au.annexb().len());
            }
        }
    }
}

/// H264ENC8X8 (LEDGER SR322): with `transform8x8` (High) every intra scene above, under both entropy
/// coders, at QP 16 / 26 / 40 with the filter off and on, decodes to the encoder's reconstruction
/// byte for byte; the flat frame stays bit-exact to the source; the textured scene takes Intra_8x8
/// somewhere (its reconstruction leaves the knob-off one: with no 8x8 chosen the decisions and the
/// reconstruction are SR197's). Bytes printed against the knob off.
#[test]
fn t8x8_every_intra_scene_decodes_to_the_reconstruction() {
    let scenes = [("smooth", smooth(64, 48)), ("flat", flat(64, 48, [144, 144, 112])), ("textured", textured(64, 48)), ("crop", smooth(50, 34))];
    for (name, img) in &scenes {
        let mut took = 0;
        for entropy in [h264enc_core::Entropy::Cavlc, h264enc_core::Entropy::Cabac] {
            for qp in [16u8, 26, 40] {
                for deblock in [false, true] {
                    let base = Config { width: img.w as u32, height: img.h as u32, qp, gop: 1, deblock, entropy, ..Default::default() };
                    let mut off = Encoder::new(base).unwrap();
                    let off_au = off.encode(&img.yuv()).unwrap();
                    let mut enc = Encoder::new(Config { transform8x8: true, ..base }).unwrap();
                    let au = enc.encode(&img.yuv()).unwrap();
                    let mut dec = Decoder::new();
                    let f = decode_one(&mut dec, &au);
                    assert_decode_is_recon(&f, &enc, img.w, img.h);
                    if enc.reconstruction() != off.reconstruction() {
                        took += 1;
                    }
                    if *name == "flat" && qp == 26 && !deblock {
                        for c in 0..3 {
                            assert_eq!(f.planes[c], img.p[c], "8x8 {entropy:?} flat plane {c} not bit-exact");
                        }
                    }
                    println!("8x8 intra {name} {entropy:?} qp {qp} deblock {deblock}: off={} on={} bytes", off_au.annexb().len(), au.annexb().len());
                }
            }
        }
        println!("8x8 intra {name}: Intra_8x8 moved the reconstruction in {took} of 12 encodes");
        if *name == "textured" {
            assert!(took > 0, "the textured scene never took Intra_8x8");
        }
    }
}

/// H264ENC8X8 (LEDGER SR322): the High parameter sets read back through h264_core under both entropy
/// coders: profile_idc 100 with no constraint flags, chroma_format_idc 1, 8-bit, no scaling lists
/// (SPS or PPS), transform_8x8_mode_flag 1; the avcC carries profile 100 and the High extension.
#[test]
fn t8x8_high_parameter_sets_read_back() {
    let img = smooth(64, 48);
    for entropy in [h264enc_core::Entropy::Cavlc, h264enc_core::Entropy::Cabac] {
        let mut enc = Encoder::new(Config { width: 64, height: 48, qp: 26, gop: 1, entropy, transform8x8: true, ..Default::default() }).unwrap();
        let au = enc.encode(&img.yuv()).unwrap();
        let mut rbsp = Vec::new();
        nal::unescape(&au.nals[0][1..], &mut rbsp);
        assert_eq!(rbsp[1], 0, "High: no constraint flags");
        let sps = ps::parse_sps(&rbsp).unwrap();
        assert_eq!((sps.profile_idc, sps.chroma_format_idc, sps.bit_depth_luma, sps.bit_depth_chroma, sps.seq_scaling_present), (100, 1, 8, 8, false));
        nal::unescape(&au.nals[1][1..], &mut rbsp);
        let pps = ps::parse_pps(&rbsp, |_| 1).unwrap();
        assert!(pps.transform_8x8_mode && !pps.pic_scaling_present);
        assert_eq!(pps.cabac, entropy == h264enc_core::Entropy::Cabac);
        assert_eq!(pps.chroma_qp_offset, [0, 0]);
        let avcc = enc.avcc();
        let c = nal::parse_avcc(&avcc).unwrap();
        assert_eq!(c.profile_idc, 100);
        assert_eq!(&avcc[avcc.len() - 4..], &[0xFD, 0xF8, 0xF8, 0], "the avcC High extension");
    }
}
