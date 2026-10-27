// SPDX-License-Identifier: LGPL-3.0-or-later
//! HEVCREXT (LEDGER SR334) KATs: 4:2:2, 4:4:4, 12-bit and unequal luma / chroma depth pictures assembled by hand with
//! the test kit's CABAC encoder, each decoded picture compared with a picture built by hand: (a) all-DC pictures at
//! 4:2:2 / 4:4:4, 8 and 12 bits, are 1 << (BitDepth - 1) in every plane of the format's size; (b) pcm coding units at
//! the same four and at luma 10 / chroma 8 decode to their samples (PcmBitDepthC below BitDepthC shifted, 8.4.4.1), each
//! plane narrowed by its own depth; (c) a 4:2:2 residual in the SECOND Cb block only (cbf_cb mask 0b10) at slice QP 37:
//! QpC = Min(qPi, 51) = 37 (8.6.1; 4:2:0's Table 8-10 gives 34); (d) SAO band offset on 4:4:4 12-bit Cb (bandShift 7,
//! cMax 31) over the whole 32x32 chroma CTB; (e) chroma deblocking every 8 CHROMA samples: 4:4:4 at luma x = 8 and
//! y = 8, 4:2:2 at luma y = 8 only (luma x = 8 is chroma x = 4); (f) P-slice chroma motion compensation reads mvCLX =
//! mvLX * 2 / SubWidthC, * 2 / SubHeightC (8.5.3.2.10) at 4:2:2 and 4:4:4; (g) Table 8-3 and the refusals by name
//! (range extension tools, separate colour planes, the 4:2:0-only consumer gate).

mod common;

use common::*;
use h264_core::ChromaFormat;
use hevc_core::tables::*;
use hevc_core::{Decoder, Error, Frame};

fn decode(stream: &[u8]) -> Vec<Frame> {
    let mut d = Decoder::new();
    d.decode_annexb(stream).expect("decode");
    d.flush();
    d.take_frames()
}

fn headers(cfg: &SpsCfg, p: &PpsCfg) -> Vec<u8> {
    let mut s = vps(false);
    s.extend(sps(cfg));
    s.extend(pps(p));
    s
}

/// (SubWidthC, SubHeightC) of chroma_format_idc 1..3.
fn sub(idc: u32) -> (usize, usize) {
    match idc {
        1 => (2, 2),
        2 => (2, 1),
        _ => (1, 1),
    }
}

fn format(idc: u32) -> ChromaFormat {
    match idc {
        1 => ChromaFormat::Yuv420,
        2 => ChromaFormat::Yuv422,
        _ => ChromaFormat::Yuv444,
    }
}

/// Plane `c` of a frame as decoded samples (planes16 above 8 bits, else the 8-bit plane).
fn samples(f: &Frame, c: usize) -> Vec<u16> {
    match &f.planes16 {
        Some(p) => p[c].clone(),
        None => f.planes[c].iter().map(|&v| v as u16).collect(),
    }
}

fn cfg32(idc: u32, bit_depth: u32) -> SpsCfg {
    SpsCfg { width: 32, height: 32, log2_ctb: 5, chroma_format_idc: idc, bit_depth, ..Default::default() }
}

/// One 32x32 CTB holding one INTRA_DC coding unit, no residual.
fn dc32(cfg: &SpsCfg) -> Vec<u8> {
    let mut s = headers(cfg, &PpsCfg { init_qp: 26, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, false), 0, 26);
    e.split_cu(0, false);
    e.dc_cu_fmt(1, cfg.chroma_format_idc);
    s.extend(nal(19, &e.finish().out));
    s
}

#[test]
fn a_all_dc_pictures_are_flat_in_every_format() {
    for (idc, bd) in [(2, 8), (3, 8), (2, 12), (3, 12)] {
        let f = decode(&dc32(&cfg32(idc, bd)));
        assert_eq!(f.len(), 1);
        let (sw, sh) = sub(idc);
        assert_eq!((f[0].chroma, f[0].bit_depth as u32), (format(idc), bd), "idc {idc} bd {bd}");
        for c in 0..3 {
            let p = samples(&f[0], c);
            let n = if c == 0 { 32 * 32 } else { (32 / sw) * (32 / sh) };
            assert_eq!(p.len(), n, "idc {idc} bd {bd} plane {c}");
            assert!(p.iter().all(|&v| v == 1 << (bd - 1)), "idc {idc} bd {bd} plane {c}");
            assert!(f[0].planes[c].iter().all(|&v| v == 128));
        }
        assert_eq!(f[0].planes16.is_some(), bd > 8);
    }
}

fn pat(x: usize, y: usize, seed: usize, bits: u32) -> u16 {
    ((x * 37 + y * 29 + seed) % (1 << bits)) as u16
}

#[test]
fn b_pcm_pictures_keep_their_samples() {
    // (chroma_format_idc, BitDepthY, BitDepthC, PcmBitDepthY, PcmBitDepthC)
    for (idc, bdy, bdc, pl, pc) in [(2, 8, 8, 8, 8), (3, 8, 8, 8, 8), (2, 12, 12, 12, 11), (3, 12, 12, 12, 11), (1, 10, 8, 10, 8)] {
        let cfg = SpsCfg { pcm: Some((pl, pc, 3, 5, true)), chroma_bit_depth: Some(bdc), ..cfg32(idc, bdy) };
        let mut s = headers(&cfg, &PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() });
        let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
        let (sw, sh) = sub(idc);
        let (cw, ch) = (32 / sw, 32 / sh);
        let plane = |w: usize, h: usize, seed: usize, bits: u32| (0..w * h).map(|i| pat(i % w, i / w, seed, bits)).collect::<Vec<u16>>();
        let (y, cb, cr) = (plane(32, 32, 300, pl), plane(cw, ch, 100, pc), plane(cw, ch, 7, pc));
        e.pcm_cu_fmt(0, 5, pl, pc, &y, &cb, &cr, (sw, sh));
        s.extend(nal(19, &e.finish().out));
        let f = decode(&s);
        let what = format!("idc {idc} luma {bdy} chroma {bdc}");
        assert_eq!(f[0].chroma, format(idc), "{what}");
        assert_eq!(samples(&f[0], 0), y.iter().map(|&v| v << (bdy - pl)).collect::<Vec<u16>>(), "{what}");
        for (c, want) in [(1, &cb), (2, &cr)] {
            let want: Vec<u16> = want.iter().map(|&v| v << (bdc - pc)).collect();
            assert_eq!(samples(&f[0], c), want, "{what} plane {c}");
            // `planes` narrows each plane by its own depth.
            assert_eq!(f[0].planes[c], want.iter().map(|&v| (v >> (bdc - 8)) as u8).collect::<Vec<u8>>(), "{what} plane {c}");
        }
        assert_eq!(f[0].planes[0], samples(&f[0], 0).iter().map(|&v| (v >> (bdy - 8)) as u8).collect::<Vec<u8>>(), "{what}");
    }
}

/// 8.6.2 then 8.6.4.2 (8x8 DCT) for one DC level at Qp'C `qp`, flat scaling, 8 bits.
fn dc_residual_8x8(level: i64, qp: i64) -> i32 {
    let (bd, log2) = (8i64, 3i64);
    let bd_shift = bd + log2 - 5;
    let d = ((level * 16 * ((LEVEL_SCALE[(qp % 6) as usize] as i64) << (qp / 6)) + (1 << (bd_shift - 1))) >> bd_shift).clamp(-32768, 32767);
    let g = ((64 * d + 64) >> 7).clamp(-32768, 32767);
    let bd_shift2 = 20 - bd;
    ((64 * g + (1 << (bd_shift2 - 1))) >> bd_shift2) as i32
}

#[test]
fn c_422_second_cb_block_residual_at_qp_37() {
    let cfg = SpsCfg { width: 16, height: 16, log2_ctb: 4, log2_max_tb: 4, chroma_format_idc: 2, ..Default::default() };
    let mut s = headers(&cfg, &PpsCfg { init_qp: 37, deblocking_disabled: true, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 37);
    e.split_cu(0, false);
    e.prev_intra(true);
    e.mpm_idx(1); // DC
    e.chroma_dm();
    // The 16x16 root transform unit: cbf_cb (top 0, second 1), cbf_cr (0, 0), cbf_luma 0.
    for v in [false, true, false, false] {
        e.cbf_chroma(0, v);
    }
    e.cbf_luma(0, false);
    // residual_coding of the second 8x8 Cb block (log2TrafoSizeC 3): one level 16 at (0,0): last prefixes 0 (chroma
    // ctxOffset 15), greater1 1 (chroma ctxSet 0, greater1Ctx 1), greater2 1, sign 0, remaining 16 - 3 = 13.
    e.decision(LAST_X + 15, 0);
    e.decision(LAST_Y + 15, 0);
    e.decision(GT1 + 16 + 1, 1);
    e.decision(GT2 + 4, 1);
    e.bypass(0);
    e.remaining(13, 0);
    s.extend(nal(19, &e.finish().out));
    let f = decode(&s);
    assert_eq!(f[0].chroma, ChromaFormat::Yuv422);
    let r = dc_residual_8x8(16, 37);
    // d = (16 * 16 * (45 << 6) + 32) >> 6 = 11520, g = (64 * 11520 + 64) >> 7 = 5760, r = (64 * 5760 + 2048) >> 12 = 90.
    assert_eq!(r, 90);
    assert_ne!(r, dc_residual_8x8(16, 34), "the 4:2:0 QpC would differ");
    let cb = &f[0].planes[1];
    assert_eq!(cb.len(), 8 * 16);
    for y in 0..16 {
        for x in 0..8 {
            let want = if y < 8 { 128 } else { 128 + r };
            assert_eq!(cb[y * 8 + x] as i32, want, "Cb ({x},{y})");
        }
    }
    assert!(f[0].planes[0].iter().chain(f[0].planes[2].iter()).all(|&v| v == 128));
}

#[test]
fn d_sao_band_offset_on_444_12_bit_cb_covers_the_whole_chroma_ctb() {
    let cfg = SpsCfg { sao: true, ..cfg32(3, 12) };
    let mut s = headers(&cfg, &PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, true, true, true), 0, 26);
    // sao() (7.3.8.3), no merge candidates: Cb band offset (sao_type_idx_chroma 1: bins 1, 0), sao_offset_abs
    // [3, 0, 0, 0] (cMax (1 << (Min(12, 10) - 5)) - 1 = 31), sign 0, band position 16 = 2048 >> 7; Cr shares the
    // type: offsets 0, band position 0.
    e.decision(SAO_TYPE, 1);
    e.bypass(0);
    for a in [3, 0, 0, 0] {
        e.bypass_tu(a, 31);
    }
    e.bypass(0);
    e.bypass_bits(16, 5);
    for _ in 0..4 {
        e.bypass_tu(0, 31);
    }
    e.bypass_bits(0, 5);
    e.split_cu(0, false);
    e.dc_cu_fmt(1, 3);
    s.extend(nal(19, &e.finish().out));
    let f = decode(&s);
    let p16 = f[0].planes16.as_ref().unwrap();
    assert_eq!(p16[1].len(), 32 * 32);
    assert!(p16[1].iter().all(|&v| v == 2051), "Cb");
    assert!(p16[0].iter().chain(p16[2].iter()).all(|&v| v == 2048));
}

/// The chroma filter of 8.7.2.5.5 (bS 2) across every edge `e` (a multiple of 8 chroma samples) of a w x h plane.
fn hand_chroma_deblock(p: &mut [i32], w: usize, h: usize, vertical: bool, tc: i32) {
    let (outer, inner) = if vertical { (w, h) } else { (h, w) };
    for e in (8..outer).step_by(8) {
        for s in 0..inner {
            let at = |i: isize| -> usize {
                let k = e as isize + i;
                if vertical { s * w + k as usize } else { k as usize * w + s }
            };
            let (p1, p0, q0, q1) = (p[at(-2)], p[at(-1)], p[at(0)], p[at(1)]);
            let delta = ((((q0 - p0) << 2) + p1 - q1 + 4) >> 3).clamp(-tc, tc);
            p[at(-1)] = (p0 + delta).clamp(0, 255);
            p[at(0)] = (q0 - delta).clamp(0, 255);
        }
    }
}

#[test]
fn e_chroma_deblocking_follows_the_chroma_sample_grid() {
    // Four 8x8 pcm coding units (pcm_loop_filter_disabled_flag 0, so they are filtered; intra: bS 2), luma flat.
    let cb_of = [60u16, 100, 90, 130];
    let cr_of = [200u16, 160, 190, 150];
    for idc in [3, 2] {
        let (sw, sh) = sub(idc);
        let cfg = SpsCfg { width: 16, height: 16, log2_ctb: 4, log2_max_tb: 4, chroma_format_idc: idc, pcm: Some((8, 8, 3, 3, false)), ..Default::default() };
        let mut s = headers(&cfg, &PpsCfg { init_qp: 26, ..Default::default() });
        let mut e = CabacEnc::new(idr_slice_header(false, false, false, false), 0, 26);
        e.split_cu(0, true);
        let (cw, ch) = (8 / sw, 8 / sh);
        for k in 0..4 {
            e.part_mode_2nx2n();
            e.pcm_samples_fmt(3, 8, 8, &[100; 64], &vec![cb_of[k]; cw * ch], &vec![cr_of[k]; cw * ch], (sw, sh));
        }
        s.extend(nal(19, &e.finish().out));
        let f = decode(&s);
        assert!(f[0].planes[0].iter().all(|&v| v == 100));
        // QpY 26 on both sides, cQpPicOffset 0: QpC = Min(26, 51) = 26; tC' at Q = 26 + 2 * (bS - 1) = 28.
        let tc = TC_TABLE[28] as i32;
        let (w, h) = (16 / sw, 16 / sh);
        for (c, of) in [(1, cb_of), (2, cr_of)] {
            let mut want: Vec<i32> = (0..w * h).map(|i| of[(i / w) / ch * 2 + (i % w) / cw] as i32).collect();
            hand_chroma_deblock(&mut want, w, h, true, tc);
            hand_chroma_deblock(&mut want, w, h, false, tc);
            let got: Vec<i32> = f[0].planes[c].iter().map(|&v| v as i32).collect();
            assert_eq!(got, want, "idc {idc} plane {c}");
        }
        // 4:2:2: the luma x = 8 edge is chroma x = 4, off the 8-sample chroma grid, so the columns 3 / 4 keep their
        // values in the rows the horizontal edge does not reach.
        if idc == 2 {
            assert_eq!((f[0].planes[1][3], f[0].planes[1][4]), (60, 100));
        } else {
            assert_ne!((f[0].planes[1][7], f[0].planes[1][8]), (60, 100));
        }
    }
}

/// 8.5.3.3.3.2 for one chroma sample of a w x h plane (eighth-sample chroma mv).
fn hand_chroma(r: &[u16], w: i32, h: i32, x: i32, y: i32, mv: [i32; 2]) -> i32 {
    let s = |x: i32, y: i32| r[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize] as i32;
    let (xf, yf) = ((mv[0] & 7) as usize, (mv[1] & 7) as usize);
    let (xi, yi) = (x + (mv[0] >> 3), y + (mv[1] >> 3));
    let hz = |yy: i32| (0..4).map(|i| CHROMA_FILTER[xf][i] * s(xi + i as i32 - 1, yy)).sum::<i32>();
    if yf == 0 {
        if xf == 0 { s(xi, yi) << 6 } else { hz(yi) }
    } else if xf == 0 {
        (0..4).map(|i| CHROMA_FILTER[yf][i] * s(xi, yi + i as i32 - 1)).sum()
    } else {
        (0..4).map(|n| CHROMA_FILTER[yf][n] * hz(yi + n as i32 - 1)).sum::<i32>() >> 6
    }
}

#[test]
fn f_inter_chroma_mv_scales_by_the_chroma_format() {
    let mvd = [1, 3];
    for idc in [2, 3] {
        let (sw, sh) = sub(idc);
        let cfg = SpsCfg { width: 32, height: 32, log2_ctb: 4, log2_max_tb: 4, chroma_format_idc: idc, pcm: Some((8, 8, 3, 4, true)), ..Default::default() };
        let mut s = vps(false);
        s.extend(nal(33, &sps_rbsp_ext(&cfg, &InterSps { max_dec_pic_buffering_minus1: 4, ..Default::default() })));
        s.extend(nal(34, &pps_rbsp_ext(&PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() }, &PpsInter::default())));
        // I0: four 16x16 pcm CTBs of gradients.
        let (w, h) = (32 / sw, 32 / sh);
        let gy = |x: usize, y: usize| (x * 5 + y * 3) as u16;
        let gcb = |x: usize, y: usize| (40 + x * 3 + y) as u16;
        let gcr = |x: usize, y: usize| (200 - x - 2 * y) as u16;
        let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
        for k in 0..4 {
            let (bx, by) = (k % 2, k / 2);
            let ctb = |f: &dyn Fn(usize, usize) -> u16, cw: usize, chh: usize| {
                (0..cw * chh).map(|i| f(bx * cw + i % cw, by * chh + i / cw)).collect::<Vec<u16>>()
            };
            let (cw, chh) = (16 / sw, 16 / sh);
            e.pcm_cu_fmt(0, 4, 8, 8, &ctb(&gy, 16, 16), &ctb(&gcb, cw, chh), &ctb(&gcr, cw, chh), (sw, sh));
            if k < 3 {
                e.terminate(0);
            }
        }
        s.extend(nal(19, &e.finish().out));
        // P1: CTB 0 a 2Nx2N AMVP prediction unit (mvp 0: mv = mvd), CTBs 1..3 skip coding units merging it.
        let cfg_p = SliceCfg { nal_type: 1, slice_type: 1, poc_lsb: 1, neg: vec![(-1, true)], five_minus_max_num_merge_cand: 4, deblocking_disabled: true, ..Default::default() };
        let mut e = CabacEnc::new(slice_header(&cfg_p), 1, 26);
        e.split_cu(0, false);
        e.cu_skip(0, false);
        e.pred_inter();
        e.part_mode_2nx2n();
        e.merge_flag(false);
        e.mvd(mvd);
        e.mvp_flag(0);
        e.rqt_root_cbf(false);
        for (inc, last) in [(0usize, false), (0, false), (2, true)] {
            e.terminate(0);
            e.split_cu(0, false);
            e.cu_skip(inc, true);
            if last {
                break;
            }
        }
        s.extend(nal(1, &e.finish().out));
        let f = decode(&s);
        let p1 = f.iter().find(|f| f.poc == 1).expect("P1");
        // mvCLX (8.5.3.2.10): (1 * 2 / SubWidthC, 3 * 2 / SubHeightC) eighth chroma samples — (1, 6) at 4:2:2, (2, 6)
        // at 4:4:4 (4:2:0 reads (1, 3)).
        let mvc = [mvd[0] * 2 / sw as i32, mvd[1] * 2 / sh as i32];
        assert_eq!(mvc, if idc == 2 { [1, 6] } else { [2, 6] });
        for (c, g) in [(1, &gcb as &dyn Fn(usize, usize) -> u16), (2, &gcr)] {
            let r: Vec<u16> = (0..w * h).map(|i| g(i % w, i / w)).collect();
            for y in 0..h {
                for x in 0..w {
                    let want = ((hand_chroma(&r, w as i32, h as i32, x as i32, y as i32, mvc) + 32) >> 6).clamp(0, 255);
                    assert_eq!(p1.planes[c][y * w + x] as i32, want, "idc {idc} plane {c} ({x},{y})");
                }
            }
        }
    }
}

#[test]
fn g_table_8_3_and_the_refusals_by_name() {
    use hevc_core::intra::{chroma_mode, chroma_mode_422};
    // Table 8-3 spot values (mode X -> 4:2:2 mode), and DM of luma 7 then mapped.
    for (x, m) in [(0, 0), (1, 1), (2, 2), (5, 2), (6, 3), (7, 5), (10, 10), (18, 21), (21, 23), (26, 26), (34, 31)] {
        assert_eq!(chroma_mode_422(x), m, "mode {x}");
    }
    assert_eq!(chroma_mode_422(chroma_mode(4, 7)), 5);
    let names = [
        "transform_skip_rotation_enabled_flag",
        "transform_skip_context_enabled_flag",
        "implicit_rdpcm_enabled_flag",
        "explicit_rdpcm_enabled_flag",
        "extended_precision_processing_flag",
        "intra_smoothing_disabled_flag",
        "high_precision_offsets_enabled_flag",
        "persistent_rice_adaptation_enabled_flag",
        "cabac_bypass_alignment_enabled_flag",
    ];
    for (i, name) in names.iter().enumerate() {
        let mut f = [false; 9];
        f[i] = true;
        let err = Decoder::new().decode_annexb(&dc32(&SpsCfg { range_ext: Some(f), ..cfg32(3, 8) })).unwrap_err();
        let Error::Unsupported(msg) = err else { panic!("{name}: {err:?}") };
        assert!(msg.contains(name) && msg.contains("HEVCREXT-TOOLS"), "{name}: {msg}");
    }
    // A range extension present with every flag 0 decodes.
    assert_eq!(decode(&dc32(&SpsCfg { range_ext: Some([false; 9]), ..cfg32(2, 8) })).len(), 1);
    // Separate colour planes: the IDR slice header carries colour_plane_id u(2) after slice_type (7.3.6.1).
    let cfg = SpsCfg { separate_colour_plane: true, ..cfg32(3, 8) };
    let mut s = headers(&cfg, &PpsCfg { init_qp: 26, ..Default::default() });
    let mut b = BitWriter::default();
    b.bit(1); // first_slice_segment_in_pic_flag
    b.bit(0); // no_output_of_prior_pics_flag
    b.ue(0);
    b.ue(2); // slice_type I
    b.u(0, 2); // colour_plane_id
    b.se(0); // slice_qp_delta
    b.bit(1); // slice_loop_filter_across_slices_enabled_flag
    b.trailing();
    let mut e = CabacEnc::new(b, 0, 26);
    e.split_cu(0, false);
    e.dc_cu_fmt(1, 1);
    s.extend(nal(19, &e.finish().out));
    assert_eq!(Decoder::new().decode_annexb(&s).unwrap_err(), Error::Unsupported("separate colour planes (HEVCREXT)"));
    // The 4:2:0-only consumer (gneiss_pal until COLOURONE-422) refuses 4:2:2 / 4:4:4 by name, and still takes 4:2:0.
    for idc in [2, 3] {
        let mut d = Decoder::new();
        d.set_chroma_420_only(true);
        assert_eq!(d.decode_annexb(&dc32(&cfg32(idc, 8))).unwrap_err(), Error::Unsupported("chroma 4:2:2 / 4:4:4 not displayable yet (COLOURONE-422)"));
    }
    let mut d = Decoder::new();
    d.set_chroma_420_only(true);
    d.decode_annexb(&dc32(&cfg32(1, 8))).expect("4:2:0 through the gate");
    d.flush();
    assert_eq!(d.take_frames()[0].chroma, ChromaFormat::Yuv420);
}
