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
//! (range extension tools, separate colour planes, the 4:2:0-only consumer gate). HEVCREXTTOOLS (LEDGER SR345): one picture
//! per range extension tool, h..s below, each failing if its tool is missing; (g) now decodes every flag alone. HEVCSCCTAIL
//! (LEDGER SR365): (t) a monochrome picture's one-component hash SEI gives a verdict; (u) the A.4 level limits, refused
//! by name only under `Decoder::set_strict_levels`.

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
    // HEVCREXTTOOLS (SR345): each range extension flag alone now decodes the all-DC picture (no tool changes it).
    for i in 0..9 {
        let mut f = [false; 9];
        f[i] = true;
        let fr = decode(&dc32(&SpsCfg { range_ext: Some(f), ..cfg32(3, 8) }));
        assert!(fr[0].planes.iter().all(|p| p.iter().all(|&v| v == 128)), "flag {i}");
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
    // ... and unequal luma / chroma depths (I420Hi carries one bit_depth), which decode without the gate (b).
    let mut d = Decoder::new();
    d.set_chroma_420_only(true);
    let unequal = SpsCfg { chroma_bit_depth: Some(8), ..cfg32(1, 10) };
    assert_eq!(d.decode_annexb(&dc32(&unequal)).unwrap_err(), Error::Unsupported("luma and chroma bit depths differ (COLOURONE-422)"));
    let mut d = Decoder::new();
    d.set_chroma_420_only(true);
    d.decode_annexb(&dc32(&cfg32(1, 8))).expect("4:2:0 through the gate");
    d.flush();
    assert_eq!(d.take_frames()[0].chroma, ChromaFormat::Yuv420);
    // HEVCREXTTOOLS (SR345): monochrome passes the gate (gneiss_pal's to_frame fills its grey chroma).
    let mut d = Decoder::new();
    d.set_chroma_420_only(true);
    d.decode_annexb(&dc32(&SpsCfg { chroma_bit_depth: Some(8), ..cfg32(0, 10) })).expect("4:0:0 through the gate");
    d.flush();
    assert_eq!(d.take_frames()[0].chroma, ChromaFormat::Mono);
}

// ---------------------------------------------------------------- HEVCREXTTOOLS (LEDGER SR345)

/// An 8x8 picture in one 16x16 CTB (the split to the 8x8 CU inferred), max transform 8x8.
fn cfg8(idc: u32, bit_depth: u32, flag: Option<usize>) -> SpsCfg {
    let mut f = [false; 9];
    if let Some(i) = flag {
        f[i] = true;
    }
    SpsCfg { width: 8, height: 8, log2_ctb: 4, log2_max_tb: 3, chroma_format_idc: idc, bit_depth, range_ext: Some(f), ..Default::default() }
}

/// VPS, an SPS with a two-picture DPB, and the PPS with the range extension fields `r`.
fn hdr(cfg: &SpsCfg, p: &PpsCfg, pi: &PpsInter, r: &PpsRext) -> Vec<u8> {
    let mut s = vps(false);
    s.extend(nal(33, &sps_rbsp_ext(cfg, &InterSps { max_dec_pic_buffering_minus1: 2, ..Default::default() })));
    s.extend(nal(34, &pps_rbsp_rext(p, pi, r)));
    s
}

fn pps_qp(init_qp: i32) -> PpsCfg {
    PpsCfg { init_qp, deblocking_disabled: true, ..Default::default() }
}

/// One DC level of a residual_coding() whose last position is (0, 0): last prefixes 0 at ctxOffset `off`, greater1 1
/// (ctxSet 0, greater1Ctx 1), greater2 1, then (after `align` when cabac_bypass_alignment) sign 0 and the remaining
/// level - 3 by `rem`.
fn dc_level(e: &mut CabacEnc, off: usize, chroma: bool, align: bool, rem: impl FnOnce(&mut CabacEnc)) {
    e.decision(LAST_X + off, 0);
    e.decision(LAST_Y + off, 0);
    e.decision(GT1 + if chroma { 16 } else { 0 } + 1, 1);
    e.decision(GT2 + if chroma { 4 } else { 0 }, 1);
    if align {
        e.align();
    }
    e.bypass(0);
    rem(e);
}

/// The 8x8 intra CU's head: (transquant bypass), part_mode 2Nx2N, the luma mode by mpm_idx, chroma DM.
fn cu_head(e: &mut CabacEnc, bypass: Option<bool>, mpm: u32) {
    if let Some(b) = bypass {
        e.decision(TRANSQUANT_BYPASS, b as u32);
    }
    e.part_mode_2nx2n();
    e.prev_intra(true);
    e.mpm_idx(mpm);
    e.chroma_dm();
}

fn luma_of(f: &Frame) -> Vec<i32> {
    samples(f, 0).iter().map(|&v| v as i32).collect()
}

/// 8.6.2 + 8.6.4.2 (transform skip) for one level at qP `qp`, flat scaling, log2TransformRange 15.
fn ts_residual(level: i64, qp: i64, log2: i64, bd: i64) -> i32 {
    let s1 = bd + log2 - 5;
    let d = ((level * 16 * ((LEVEL_SCALE[(qp % 6) as usize] as i64) << (qp / 6)) + (1 << (s1 - 1))) >> s1).clamp(-32768, 32767);
    let s2 = 20 - bd;
    (((d << (5 + log2)) + (1 << (s2 - 1))) >> s2) as i32
}

#[test]
fn h_implicit_rdpcm_on_an_8x8_transform_skip_block() {
    // INTRA_ANGULAR26 (mpm_idx 2) with implicit_rdpcm_enabled_flag; transform skip at 8x8 (log2_max_transform_skip_block_size
    // _minus2 1): one level 4 at (0, 0) accumulates down column 0 (8.6.8 vertical: r[x][y] += r[x][y - 1]).
    let cfg = cfg8(1, 8, Some(2));
    let r = PpsRext { transform_skip: true, range: Some(PpsRange { log2_max_transform_skip_block_size_minus2: 1, ..Default::default() }), ..Default::default() };
    let mut s = hdr(&cfg, &pps_qp(26), &PpsInter::default(), &r);
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    cu_head(&mut e, None, 2);
    e.cbf_chroma(0, false);
    e.cbf_chroma(0, false);
    e.cbf_luma(0, true);
    e.decision(TRANSFORM_SKIP, 1);
    dc_level(&mut e, 3, false, false, |e| e.remaining(1, 0));
    s.extend(nal(19, &e.finish().out));
    let y = luma_of(&decode(&s)[0]);
    // d = (4 * 16 * (51 << 4) + 32) >> 6 = 816; r = ((816 << 8) + 2048) >> 12 = 51.
    let res = ts_residual(4, 26, 3, 8);
    assert_eq!(res, 51);
    for i in 0..64 {
        assert_eq!(y[i], if i % 8 == 0 { 128 + res } else { 128 }, "luma ({}, {})", i % 8, i / 8);
    }
}

#[test]
fn i_rotation_and_the_transform_skip_context_on_a_4x4_bypass_block() {
    // transform_skip_rotation + transform_skip_context, a transquant-bypass intra DC CU split into four 4x4 TUs; TU 0
    // carries one level 5 at (1, 1): last (1, 1) (diagonal scan position 4), the four sig_coeff_flags before it on the
    // transform-skip context (ctxInc 42), and the 180-degree rotation puts the residual at (2, 2).
    let cfg = SpsCfg { depth_intra: 1, range_ext: Some([true, true, false, false, false, false, false, false, false]), ..cfg8(1, 8, None) };
    let mut s = hdr(&cfg, &pps_qp(26), &PpsInter::default(), &PpsRext { transquant_bypass: true, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    cu_head(&mut e, Some(true), 1);
    e.decision(SPLIT_TRANSFORM + 2, 1);
    e.cbf_chroma(0, false);
    e.cbf_chroma(0, false);
    e.cbf_luma(1, true);
    for (ctx, bin) in [(LAST_X, 1), (LAST_X + 1, 0), (LAST_Y, 1), (LAST_Y + 1, 0)] {
        e.decision(ctx, bin);
    }
    for _ in 0..4 {
        e.decision(SIG + 42, 0);
    }
    e.decision(GT1 + 1, 1);
    e.decision(GT2, 1);
    e.bypass(0);
    e.remaining(2, 0);
    for _ in 0..3 {
        e.cbf_luma(1, false);
    }
    s.extend(nal(19, &e.finish().out));
    let y = luma_of(&decode(&s)[0]);
    for i in 0..64 {
        assert_eq!(y[i], if i == 2 * 8 + 2 { 133 } else { 128 }, "luma ({}, {})", i % 8, i / 8);
    }
}

#[test]
fn j_explicit_rdpcm_in_an_inter_bypass_cu() {
    // A P picture over a flat IDR: a merged 2Nx2N transquant-bypass CU (mv 0), explicit_rdpcm_flag 1 with
    // explicit_rdpcm_dir_flag 1 (vertical): one level 5 at (0, 0) fills column 0.
    let cfg = cfg8(1, 8, Some(3));
    let mut s = hdr(&cfg, &pps_qp(26), &PpsInter::default(), &PpsRext { transquant_bypass: true, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    e.decision(TRANSQUANT_BYPASS, 0);
    e.part_mode_2nx2n();
    e.dc_cu_fmt(1, 1);
    s.extend(nal(19, &e.finish().out));
    let pc = SliceCfg { nal_type: 1, slice_type: 1, poc_lsb: 1, neg: vec![(-1, true)], five_minus_max_num_merge_cand: 4, deblocking_disabled: true, ..Default::default() };
    let mut e = CabacEnc::new(slice_header(&pc), 1, 26);
    e.decision(TRANSQUANT_BYPASS, 1);
    e.cu_skip(0, false);
    e.pred_inter();
    e.part_mode_2nx2n();
    e.merge_flag(true);
    // rqt_root_cbf inferred 1 (2Nx2N merge); one 8x8 TU: cbf_cb 0, cbf_cr 0, cbf_luma inferred 1.
    e.cbf_chroma(0, false);
    e.cbf_chroma(0, false);
    e.decision(EXPLICIT_RDPCM, 1);
    e.decision(EXPLICIT_RDPCM_DIR, 1);
    dc_level(&mut e, 3, false, false, |e| e.remaining(2, 0));
    s.extend(nal(1, &e.finish().out));
    let f = decode(&s);
    let p = f.iter().find(|f| f.poc == 1).expect("POC 1");
    let y = luma_of(p);
    for i in 0..64 {
        assert_eq!(y[i], if i % 8 == 0 { 133 } else { 128 }, "luma ({}, {})", i % 8, i / 8);
    }
}

#[test]
fn k_cu_chroma_qp_offset_picks_list_entry_one() {
    // chroma_qp_offset_list [(0, 0), (6, -3)], cu_chroma_qp_offset_flag 1, cu_chroma_qp_offset_idx 1: Cb's qPi is
    // 37 + 6 = 43 (8.6.1), QpC 37 by Table 8-10 instead of 34; one Cb DC level 16 at slice QP 37.
    let cfg = SpsCfg { width: 16, height: 16, log2_ctb: 4, log2_max_tb: 4, ..Default::default() };
    let r = PpsRext { range: Some(PpsRange { chroma_qp_offset_list: Some((0, vec![(0, 0), (6, -3)])), ..Default::default() }), ..Default::default() };
    let mut s = hdr(&cfg, &pps_qp(37), &PpsInter::default(), &r);
    let mut b = BitWriter::default();
    b.bit(1); // first_slice_segment_in_pic_flag
    b.bit(0); // no_output_of_prior_pics_flag
    b.ue(0);
    b.ue(2); // slice_type I
    b.se(0); // slice_qp_delta
    b.bit(1); // cu_chroma_qp_offset_enabled_flag
    b.trailing();
    let mut e = CabacEnc::new(b, 0, 37);
    e.split_cu(0, false);
    e.prev_intra(true);
    e.mpm_idx(1);
    e.chroma_dm();
    e.cbf_chroma(0, true);
    e.cbf_chroma(0, false);
    e.cbf_luma(0, false);
    e.decision(CU_CHROMA_QP_OFFSET_FLAG, 1);
    e.decision(CU_CHROMA_QP_OFFSET_IDX, 1); // TR cMax 1: idx 1
    dc_level(&mut e, 15, true, false, |e| e.remaining(13, 0));
    s.extend(nal(19, &e.finish().out));
    let f = decode(&s);
    let want = dc_residual_8x8(16, qpc_420(43) as i64);
    assert_ne!(want, dc_residual_8x8(16, qpc_420(37) as i64), "the offset must be visible");
    assert!(f[0].planes[1].iter().all(|&v| v as i32 == 128 + want), "Cb {:?}", &f[0].planes[1][..8]);
    assert!(f[0].planes[0].iter().chain(f[0].planes[2].iter()).all(|&v| v == 128));
}

#[test]
fn l_cross_component_prediction_scales_the_luma_residual_into_cb() {
    // 4:4:4 lossless DC CU, chroma DM: luma level 8 at (0, 0), cbf_cb 0; log2_res_scale_abs_plus1[0] 3, sign 0 ->
    // ResScaleVal 4: rCb = (4 * ((8 << 8) >> 8)) >> 3 = 4 (8.6.6); Cr's log2_res_scale_abs_plus1 0.
    let cfg = cfg8(3, 8, None);
    let r = PpsRext { transquant_bypass: true, range: Some(PpsRange { cross_component_prediction: true, ..Default::default() }), ..Default::default() };
    let mut s = hdr(&cfg, &pps_qp(26), &PpsInter::default(), &r);
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    cu_head(&mut e, Some(true), 1);
    e.cbf_chroma(0, false);
    e.cbf_chroma(0, false);
    e.cbf_luma(0, true);
    dc_level(&mut e, 3, false, false, |e| e.remaining(5, 0));
    for (k, bin) in [1, 1, 1, 0].into_iter().enumerate() {
        e.decision(LOG2_RES_SCALE + k, bin);
    }
    e.decision(RES_SCALE_SIGN, 0);
    e.decision(LOG2_RES_SCALE + 4, 0);
    s.extend(nal(19, &e.finish().out));
    let f = &decode(&s)[0];
    for i in 0..64 {
        assert_eq!(f.planes[0][i], if i == 0 { 136 } else { 128 }, "Y {i}");
        assert_eq!(f.planes[1][i], if i == 0 { 132 } else { 128 }, "Cb {i}");
        assert_eq!(f.planes[2][i], 128, "Cr {i}");
    }
}

#[test]
fn m_persistent_rice_carries_stat_coeff_across_transform_units() {
    // Two lossless 8x8 DC CUs (16x8 picture) each split into four 4x4 TUs, each TU one DC level 23 (remaining 20):
    // StatCoeff[3] (luma, bypass) counts up once per TU (20 >= 3 << (StatCoeff / 4)), so TUs 4..7 start at cRiceParam 1;
    // a decoder that does not carry StatCoeff reads them at 0 and fails.
    let cfg = SpsCfg { width: 16, depth_intra: 1, ..cfg8(1, 8, Some(7)) };
    let mut s = hdr(&cfg, &pps_qp(26), &PpsInter::default(), &PpsRext { transquant_bypass: true, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    let mut stat = 0u32;
    let mut rices = Vec::new();
    for _cu in 0..2 {
        cu_head(&mut e, Some(true), 1);
        e.decision(SPLIT_TRANSFORM + 2, 1);
        e.cbf_chroma(0, false);
        e.cbf_chroma(0, false);
        for _tu in 0..4 {
            e.cbf_luma(1, true);
            let rice = stat / 4;
            rices.push(rice);
            dc_level(&mut e, 0, false, false, |e| e.remaining(20, rice));
            if 20 >= 3 << rice {
                stat += 1;
            } else if 40 < (1 << rice) && stat > 0 {
                stat -= 1;
            }
        }
    }
    assert_eq!(rices, [0, 0, 0, 0, 1, 1, 1, 1]);
    s.extend(nal(19, &e.finish().out));
    let y = luma_of(&decode(&s)[0]);
    for i in 0..128 {
        let (x, yy) = (i % 16, i / 16);
        assert_eq!(y[i], if x % 4 == 0 && yy % 4 == 0 { 151 } else { 128 }, "luma ({x}, {yy})");
    }
}

#[test]
fn n_cabac_bypass_alignment_before_the_sign_and_escape_bins() {
    // One lossless DC level 100: greater2 1 sets escapeDataPresent, so the encoder aligns (ivlCurrRange 256) before
    // the sign and coeff_abs_level_remaining 97; a decoder that does not align reads other bins.
    let cfg = cfg8(1, 8, Some(8));
    let mut s = hdr(&cfg, &pps_qp(26), &PpsInter::default(), &PpsRext { transquant_bypass: true, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    cu_head(&mut e, Some(true), 1);
    e.cbf_chroma(0, false);
    e.cbf_chroma(0, false);
    e.cbf_luma(0, true);
    dc_level(&mut e, 3, false, true, |e| e.remaining(97, 0));
    s.extend(nal(19, &e.finish().out));
    let y = luma_of(&decode(&s)[0]);
    for i in 0..64 {
        assert_eq!(y[i], if i == 0 { 228 } else { 128 }, "luma {i}");
    }
}

#[test]
fn o_intra_smoothing_disabled_predicts_from_unfiltered_references() {
    // A pcm CU whose column 7 is L[y] = 4 y^2 (convex, so the [1 2 1] filter would move it), then an 8x8 INTRA_ANGULAR18
    // CU (rem_intra_luma_pred_mode 16 past MPMs {0, 1, 26}): min distance 8 > 7 would filter; intra_smoothing_disabled
    // leaves the references, so pred(x, y) = L[0] for x >= y (top and corner substituted from L[0]), else L[y - x - 1].
    let cfg = SpsCfg { width: 16, pcm: Some((8, 8, 3, 3, true)), ..cfg8(1, 8, Some(5)) };
    let mut s = hdr(&cfg, &pps_qp(26), &PpsInter::default(), &PpsRext::default());
    let l = |y: usize| (4 * y * y) as u16;
    let ypcm: Vec<u16> = (0..64).map(|i| if i % 8 == 7 { l(i / 8) } else { 128 }).collect();
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    e.part_mode_2nx2n();
    e.pcm_samples_fmt(3, 8, 8, &ypcm, &[128; 16], &[128; 16], (2, 2));
    e.part_mode_2nx2n();
    e.terminate(0); // pcm_flag 0
    e.prev_intra(false);
    e.bypass_bits(16, 5);
    e.chroma_dm();
    e.cbf_chroma(0, false);
    e.cbf_chroma(0, false);
    e.cbf_luma(0, false);
    s.extend(nal(19, &e.finish().out));
    let y = luma_of(&decode(&s)[0]);
    for yy in 0..8 {
        for x in 0..8 {
            let want = if x >= yy { l(0) } else { l(yy - x - 1) } as i32;
            assert_eq!(y[yy * 16 + 8 + x], want, "pred ({x}, {yy})");
        }
    }
}

#[test]
fn p_high_precision_offsets_add_the_offset_at_full_bit_depth() {
    // 10-bit P skip CU over a flat 512 IDR, explicit weights: LumaWeight 1 (denom 0), luma_offset 100: with
    // high_precision_offsets_enabled_flag the offset is not scaled by 1 << (BitDepth - 8): 612, not 912.
    let cfg = cfg8(1, 10, Some(6));
    let mut s = hdr(&cfg, &pps_qp(26), &PpsInter { weighted_pred: true, ..Default::default() }, &PpsRext::default());
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    e.part_mode_2nx2n();
    e.dc_cu_fmt(1, 1);
    s.extend(nal(19, &e.finish().out));
    let pc = SliceCfg {
        nal_type: 1,
        slice_type: 1,
        poc_lsb: 1,
        neg: vec![(-1, true)],
        weights: Some((0, vec![Some((0, 100))])),
        five_minus_max_num_merge_cand: 4,
        deblocking_disabled: true,
        ..Default::default()
    };
    let mut e = CabacEnc::new(slice_header(&pc), 1, 26);
    e.cu_skip(0, true);
    s.extend(nal(1, &e.finish().out));
    let f = decode(&s);
    let p = f.iter().find(|f| f.poc == 1).expect("POC 1");
    assert!(samples(p, 0).iter().all(|&v| v == 612), "{:?}", &samples(p, 0)[..4]);
    assert!(samples(p, 1).iter().chain(samples(p, 2).iter()).all(|&v| v == 512));
}

#[test]
fn q_log2_sao_offset_scale_shifts_the_band_offset() {
    // 10-bit luma band offset 3 at band 16 (512 >> 5) with log2_sao_offset_scale_luma 2: SaoOffsetVal 3 << 2 = 12.
    let cfg = SpsCfg { sao: true, ..cfg32(1, 10) };
    let r = PpsRext { range: Some(PpsRange { log2_sao_offset_scale: (2, 0), ..Default::default() }), ..Default::default() };
    let mut s = hdr(&cfg, &pps_qp(26), &PpsInter::default(), &r);
    let mut e = CabacEnc::new(idr_slice_header(true, false, true, true), 0, 26);
    e.decision(SAO_TYPE, 1);
    e.bypass(0);
    for a in [3, 0, 0, 0] {
        e.bypass_tu(a, 31);
    }
    e.bypass(0);
    e.bypass_bits(16, 5);
    e.split_cu(0, false);
    e.dc_cu_fmt(1, 1);
    s.extend(nal(19, &e.finish().out));
    let f = &decode(&s)[0];
    assert!(samples(f, 0).iter().all(|&v| v == 524), "{:?}", &samples(f, 0)[..4]);
    assert!(samples(f, 1).iter().all(|&v| v == 512));
}

#[test]
fn r_extended_precision_at_sixteen_bits() {
    // (a) 16-bit 4:4:4 all-DC pictures are 1 << 15 (the 16-bit profiles decode).
    let f = &decode(&dc32(&SpsCfg { range_ext: Some([false, false, false, false, true, false, false, false, false]), ..cfg32(3, 16) }))[0];
    assert_eq!(f.bit_depth, 16);
    assert!(f.planes16.as_ref().unwrap().iter().all(|p| p.iter().all(|&v| v == 32768)));
    // (b) extended_precision_processing_flag at 16 bits (log2TransformRange 22): one transform-skip level 40000 (above the
    // 16-bit CoeffMax) through the limited EGk at slice QP -48 (qP 0): d = (40000 * 16 * 40 + 32) >> 6 = 400000 (bdShift
    // 16 + 2 + 10 - 22), r = ((400000 << 7) + 1024) >> 11 = 25000 (bdShift Max(4, 11), tsShift Min(5, 9) + 2).
    let cfg = SpsCfg { depth_intra: 1, ..cfg8(1, 16, Some(4)) };
    let mut s = hdr(&cfg, &pps_qp(-48), &PpsInter::default(), &PpsRext { transform_skip: true, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, -48);
    cu_head(&mut e, None, 1);
    e.decision(SPLIT_TRANSFORM + 2, 1);
    e.cbf_chroma(0, false);
    e.cbf_chroma(0, false);
    e.cbf_luma(1, true);
    e.decision(TRANSFORM_SKIP, 1);
    dc_level(&mut e, 0, false, false, |e| e.remaining_limited(39997, 0, 22));
    for _ in 0..3 {
        e.cbf_luma(1, false);
    }
    s.extend(nal(19, &e.finish().out));
    let y = luma_of(&decode(&s)[0]);
    for i in 0..64 {
        assert_eq!(y[i], if i == 0 { 32768 + 25000 } else { 32768 }, "luma {i}");
    }
}

#[test]
fn s_monochrome_pcm_and_all_dc() {
    // chroma_format_idc 0: no chroma syntax, a Frame with ChromaFormat::Mono and empty Cb / Cr planes.
    for bd in [8, 12] {
        let f = &decode(&dc32(&SpsCfg { chroma_bit_depth: Some(8), ..cfg32(0, bd) }))[0];
        assert_eq!(f.chroma, ChromaFormat::Mono);
        assert!(f.planes[1].is_empty() && f.planes[2].is_empty());
        assert!(samples(f, 0).iter().all(|&v| v == 1 << (bd - 1)), "bd {bd}");
    }
    for bd in [8u32, 12] {
        let cfg = SpsCfg { pcm: Some((bd, 8, 3, 3, true)), chroma_bit_depth: Some(8), range_ext: None, ..cfg8(0, bd, None) };
        let mut s = hdr(&cfg, &pps_qp(26), &PpsInter::default(), &PpsRext::default());
        let y: Vec<u16> = (0..64).map(|i| pat(i % 8, i / 8, 3, bd)).collect();
        let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
        e.part_mode_2nx2n();
        e.pcm_samples_fmt(3, bd, 8, &y, &[], &[], (1, 1));
        s.extend(nal(19, &e.finish().out));
        let f = &decode(&s)[0];
        assert_eq!(f.chroma, ChromaFormat::Mono);
        assert_eq!(samples(f, 0), y, "bd {bd}");
        assert!(f.planes[1].is_empty() && f.planes[2].is_empty());
    }
}

/// HEVCSCCTAIL (SR365): an 8x8 monochrome pcm picture (8 bits) whose SPS holds `max_dec_minus1` + 1 pictures, followed
/// by `sei` (a suffix SEI RBSP) when given, and its luma samples.
fn mono_pcm(max_dec_minus1: u32, sei: Option<&dyn Fn(&[u16]) -> Vec<u8>>) -> (Vec<u8>, Vec<u16>) {
    let cfg = SpsCfg { pcm: Some((8, 8, 3, 3, true)), chroma_bit_depth: Some(8), range_ext: None, ..cfg8(0, 8, None) };
    let mut s = vps(false);
    s.extend(nal(33, &sps_rbsp_ext(&cfg, &InterSps { max_dec_pic_buffering_minus1: max_dec_minus1, ..Default::default() })));
    s.extend(nal(34, &pps_rbsp_rext(&pps_qp(26), &PpsInter::default(), &PpsRext::default())));
    let y: Vec<u16> = (0..64).map(|i| pat(i % 8, i / 8, 5, 8)).collect();
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    e.part_mode_2nx2n();
    e.pcm_samples_fmt(3, 8, 8, &y, &[], &[], (1, 1));
    s.extend(nal(19, &e.finish().out));
    if let Some(f) = sei {
        s.extend(nal(40, &f(&y)));
    }
    (s, y)
}

#[test]
fn t_monochrome_picture_hash_sei_gives_a_verdict() {
    use hevc_core::picture::Plane;
    use hevc_core::sei::{plane_crc, HashCheck};
    // D.2.19 at chroma_format_idc 0: hash_type 1 (CRC) and ONE component, payloadSize 3.
    let crc_sei = |delta: u16| move |y: &[u16]| -> Vec<u8> {
        let crc = plane_crc(&Plane { w: 8, h: 8, data: y.to_vec() }, 8).wrapping_add(delta);
        vec![132, 3, 1, (crc >> 8) as u8, crc as u8, 0x80]
    };
    for (delta, want) in [(0u16, HashCheck::Match), (1, HashCheck::Mismatch(0))] {
        let f = crc_sei(delta);
        let (s, _) = mono_pcm(1, Some(&f as &dyn Fn(&[u16]) -> Vec<u8>));
        let mut d = Decoder::new();
        d.decode_annexb(&s).expect("decode");
        d.flush();
        assert_eq!(d.take_frames().len(), 1);
        assert_eq!(d.take_hash_checks(), vec![(0, want)], "delta {delta}");
    }
}

#[test]
fn u_level_limits_refuse_by_name_only_when_strict() {
    use hevc_core::level::{check_level, level, max_dpb_size};
    use hevc_core::ps::{Ordering, Pps, ProfileTierLevel, Sps};
    let sps = |w: u32, h: u32, idc: u8, high: bool, dpb: u32| Sps {
        width: w,
        height: h,
        ptl: ProfileTierLevel { general_profile_idc: 1, general_level_idc: idc, general_tier_flag: high, ..Default::default() },
        ordering: vec![Ordering { max_dec_pic_buffering_minus1: dpb, ..Default::default() }],
        ..Default::default()
    };
    let pps = |cols: u32, rows: u32| Pps { num_tile_columns: cols, num_tile_rows: rows, ..Default::default() };
    let refused = |r: Result<(), Error>, what: &str| match r {
        Err(Error::Unsupported(m)) => assert!(m.contains(what) && m.contains("HEVC-LEVEL"), "{m}"),
        other => panic!("{what}: {other:?}"),
    };
    // Level 1 (MaxLumaPs 36864): 64x48 fits; 256x192 (49152) does not; 552x8 fits the area but not Sqrt(8 x 36864) = 543.
    assert_eq!(check_level(&sps(64, 48, 30, false, 0), &pps(1, 1)), Ok(()));
    refused(check_level(&sps(256, 192, 30, false, 0), &pps(1, 1)), "picture above level 1 (MaxLumaPs");
    refused(check_level(&sps(552, 8, 30, false, 0), &pps(1, 1)), "width or height");
    refused(check_level(&sps(64, 48, 30, false, 0), &pps(2, 1)), "tiles");
    refused(check_level(&sps(64, 48, 30, true, 0), &pps(1, 1)), "high tier");
    refused(check_level(&sps(64, 48, 31, false, 0), &pps(1, 1)), "unknown general_level_idc");
    // Level 8.5 (255) has no limits; level 4.1 high tier is a level.
    assert_eq!(check_level(&sps(16384, 16384, 255, false, 15), &pps(30, 30)), Ok(()));
    assert_eq!(check_level(&sps(1920, 1080, 123, true, 5), &pps(1, 1)), Ok(()));
    // A.4.2: 1920x1080 at level 4.1 (2073600 > 3/4 of 2228224) keeps maxDpbPicBuf: 6, 7 for an SCC profile.
    assert_eq!(max_dpb_size(&sps(1920, 1080, 123, false, 0), level(123).unwrap()), 6);
    let mut scc = sps(1920, 1080, 123, false, 0);
    scc.ptl.general_profile_idc = 9;
    assert_eq!(max_dpb_size(&scc, level(123).unwrap()), 7);
    refused(check_level(&sps(1920, 1080, 123, false, 6), &pps(1, 1)), "MaxDpbSize");
    assert_eq!(check_level(&scc_dpb(scc, 6), &pps(1, 1)), Ok(()));
    // By decode: the 8x8 picture at level 2 (MaxDpbSize min(4 x 6, 16) = 16) with a 17-picture DPB is refused only when
    // strict; 16 pictures pass strict.
    let (s17, y) = mono_pcm(16, None);
    let mut d = Decoder::new();
    d.set_strict_levels(true);
    refused(d.decode_annexb(&s17), "MaxDpbSize");
    let f = &decode(&s17)[0];
    assert_eq!(samples(f, 0), y);
    let (s16, _) = mono_pcm(15, None);
    let mut d = Decoder::new();
    d.set_strict_levels(true);
    d.decode_annexb(&s16).expect("within level 2");
    d.flush();
    assert_eq!(d.take_frames().len(), 1);
}

fn scc_dpb(mut s: hevc_core::ps::Sps, dpb: u32) -> hevc_core::ps::Sps {
    s.ordering[0].max_dec_pic_buffering_minus1 = dpb;
    s
}
