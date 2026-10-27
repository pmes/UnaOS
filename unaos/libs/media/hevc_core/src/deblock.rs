// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The deblocking filter (8.7.2) over a whole decoded picture: the vertical edges of the picture first, then the
//! horizontal edges on their output (8.7.2.1). Edges are the transform and coding block edges on the 8x8 luma grid
//! (16x16 for 4:2:0 chroma); an edge is skipped at the picture boundary, across a slice boundary whose slice (the one
//! holding q0) disallows it, across a tile boundary when loop_filter_across_tiles_enabled_flag is 0, and in a slice with
//! slice_deblocking_filter_disabled_flag. The boundary strength (8.7.2.4, HEVCINTER SR216): 2 when p0 or q0 is intra
//! (every block of an I picture), 1 on a transform edge with non-zero luma coefficients on either side, 1 when the
//! two prediction blocks use different reference pictures or numbers of motion vectors or motion vectors 4 quarter
//! samples apart, else 0 (no filtering). Prediction block edges are edges too. Luma: the dE / dEp / dEq decisions
//! (8.7.2.5.3) and the strong / normal filters (8.7.2.5.7) with tC from bS; chroma: only bS 2 edges, the filter of
//! 8.7.2.5.5 with QpC from Table 8-10 and cQpPicOffset. pcm (with pcm_loop_filter_disabled_flag) and transquant
//! bypass samples are left as they are.

use crate::picture::{Picture, PuMotion, F_BYPASS, F_CBF, F_EDGE_H, F_EDGE_V, F_PCM, F_PU_EDGE_H, F_PU_EDGE_V};
use crate::ps::{Geometry, Pps, Sps};
use crate::tables::{qpc_420, BETA_TABLE, TC_TABLE};

/// The intra boundary strength (8.7.2.4: p0 or q0 in an intra coding unit).
const BS_INTRA: i32 = 2;

struct EdgeFacts {
    qp_p: i32,
    qp_q: i32,
    beta_offset_div2: i32,
    tc_offset_div2: i32,
    no_p: bool,
    no_q: bool,
    /// bS (8.7.2.4), 1 or 2.
    bs: i32,
}

/// Two motion vectors 4 or more quarter luma samples apart in either component.
fn far(a: [i32; 2], b: [i32; 2]) -> bool {
    (a[0] - b[0]).abs() >= 4 || (a[1] - b[1]).abs() >= 4
}

/// 8.7.2.4 bS 1 from motion: different reference pictures (compared as pictures, by PicOrderCntVal) or numbers of
/// motion vectors, or motion vectors for the same picture 4 quarter samples apart.
pub fn motion_differs(p: &PuMotion, q: &PuMotion) -> bool {
    let (np, nq) = (p.pred.count_ones(), q.pred.count_ones());
    if np != nq {
        return true;
    }
    if np == 1 {
        let lp = if p.uses(0) { 0 } else { 1 };
        let lq = if q.uses(0) { 0 } else { 1 };
        return p.ref_poc[lp] != q.ref_poc[lq] || far(p.mv[lp], q.mv[lq]);
    }
    let (p0, p1, q0, q1) = (p.ref_poc[0], p.ref_poc[1], q.ref_poc[0], q.ref_poc[1]);
    if !((p0 == q0 && p1 == q1) || (p0 == q1 && p1 == q0)) {
        return true;
    }
    if p0 != p1 {
        if p0 == q0 { far(p.mv[0], q.mv[0]) || far(p.mv[1], q.mv[1]) } else { far(p.mv[0], q.mv[1]) || far(p.mv[1], q.mv[0]) }
    } else {
        (far(p.mv[0], q.mv[0]) || far(p.mv[1], q.mv[1])) && (far(p.mv[0], q.mv[1]) || far(p.mv[1], q.mv[0]))
    }
}

/// 8.7.2.4: bS of the edge between the 4x4 blocks `ip` (p0) and `iq` (q0); `transform_edge` when the edge is a
/// transform block edge.
fn boundary_strength(pic: &Picture, ip: usize, iq: usize, transform_edge: bool) -> i32 {
    let (mp, mq) = (&pic.motion[ip], &pic.motion[iq]);
    if mp.pred == 0 || mq.pred == 0 {
        return BS_INTRA;
    }
    if transform_edge && (pic.flags[ip] & F_CBF != 0 || pic.flags[iq] & F_CBF != 0) {
        return 1;
    }
    motion_differs(mp, mq) as i32
}

fn edge_facts(pic: &Picture, geo: &Geometry, pps: &Pps, pcm_lf_off: bool, p: (usize, usize), q: (usize, usize), flag: u8) -> Option<EdgeFacts> {
    let iq = pic.idx4(q.0, q.1);
    let ip = pic.idx4(p.0, p.1);
    let pu_flag = if flag == F_EDGE_V { F_PU_EDGE_V } else { F_PU_EDGE_H };
    if pic.flags[iq] & (flag | pu_flag) == 0 {
        return None;
    }
    let sq = pic.slice_at(geo, q.0, q.1)?;
    let sp = pic.slice_at(geo, p.0, p.1)?;
    if sq.deblocking_disabled {
        return None;
    }
    if sp.addr_rs != sq.addr_rs && !sq.loop_filter_across_slices {
        return None;
    }
    let (cp, cq) = (geo.ctb_rs(p.0 as u32, p.1 as u32), geo.ctb_rs(q.0 as u32, q.1 as u32));
    if !pps.loop_filter_across_tiles_enabled_flag && geo.tile_of_rs(cp) != geo.tile_of_rs(cq) {
        return None;
    }
    let bs = boundary_strength(pic, ip, iq, pic.flags[iq] & flag != 0);
    if bs == 0 {
        return None;
    }
    let keep = |i: usize| (pic.flags[i] & F_PCM != 0 && pcm_lf_off) || pic.flags[i] & F_BYPASS != 0;
    Some(EdgeFacts {
        qp_p: pic.qp[ip] as i32,
        qp_q: pic.qp[iq] as i32,
        beta_offset_div2: sq.beta_offset_div2,
        tc_offset_div2: sq.tc_offset_div2,
        no_p: keep(ip),
        no_q: keep(iq),
        bs,
    })
}

/// The luma filter of one 4-line edge segment. `q0` is the index of q0,0; `across` steps from p0 to q0, `along` from
/// line k to line k + 1.
fn filter_luma(d: &mut [u8], q0: usize, across: isize, along: isize, f: &EdgeFacts, bd: u32) {
    let qpl = (f.qp_q + f.qp_p + 1) >> 1;
    let beta = BETA_TABLE[(qpl + (f.beta_offset_div2 << 1)).clamp(0, 51) as usize] as i32 * (1 << (bd - 8));
    let tc = TC_TABLE[(qpl + 2 * (f.bs - 1) + (f.tc_offset_div2 << 1)).clamp(0, 53) as usize] as i32 * (1 << (bd - 8));
    let at = |k: isize, i: isize| (q0 as isize + k * along + i * across) as usize;
    // p_i at offset -(i + 1), q_i at offset i.
    let px = |d: &[u8], i: isize, k: isize| d[at(k, -(i + 1))] as i32;
    let qx = |d: &[u8], i: isize, k: isize| d[at(k, i)] as i32;
    let dp0 = (px(d, 2, 0) - 2 * px(d, 1, 0) + px(d, 0, 0)).abs();
    let dp3 = (px(d, 2, 3) - 2 * px(d, 1, 3) + px(d, 0, 3)).abs();
    let dq0 = (qx(d, 2, 0) - 2 * qx(d, 1, 0) + qx(d, 0, 0)).abs();
    let dq3 = (qx(d, 2, 3) - 2 * qx(d, 1, 3) + qx(d, 0, 3)).abs();
    let (dpq0, dpq3) = (dp0 + dq0, dp3 + dq3);
    let (dp, dq) = (dp0 + dp3, dq0 + dq3);
    if dpq0 + dpq3 >= beta {
        return;
    }
    let dsam = |d: &[u8], k: isize, dpq: i32| {
        dpq < (beta >> 2)
            && (px(d, 3, k) - px(d, 0, k)).abs() + (qx(d, 0, k) - qx(d, 3, k)).abs() < (beta >> 3)
            && (px(d, 0, k) - qx(d, 0, k)).abs() < ((5 * tc + 1) >> 1)
    };
    let strong = dsam(d, 0, 2 * dpq0) && dsam(d, 3, 2 * dpq3);
    let dep = dp < ((beta + (beta >> 1)) >> 3);
    let deq = dq < ((beta + (beta >> 1)) >> 3);
    let max = (1i32 << bd) - 1;
    for k in 0..4isize {
        let (p0, p1, p2, p3) = (px(d, 0, k), px(d, 1, k), px(d, 2, k), px(d, 3, k));
        let (q0v, q1, q2, q3) = (qx(d, 0, k), qx(d, 1, k), qx(d, 2, k), qx(d, 3, k));
        if strong {
            let c = |v: i32, x: i32| x.clamp(v - 2 * tc, v + 2 * tc);
            if !f.no_p {
                d[at(k, -1)] = c(p0, (p2 + 2 * p1 + 2 * p0 + 2 * q0v + q1 + 4) >> 3) as u8;
                d[at(k, -2)] = c(p1, (p2 + p1 + p0 + q0v + 2) >> 2) as u8;
                d[at(k, -3)] = c(p2, (2 * p3 + 3 * p2 + p1 + p0 + q0v + 4) >> 3) as u8;
            }
            if !f.no_q {
                d[at(k, 0)] = c(q0v, (p1 + 2 * p0 + 2 * q0v + 2 * q1 + q2 + 4) >> 3) as u8;
                d[at(k, 1)] = c(q1, (p0 + q0v + q1 + q2 + 2) >> 2) as u8;
                d[at(k, 2)] = c(q2, (p0 + q0v + q1 + 3 * q2 + 2 * q3 + 4) >> 3) as u8;
            }
        } else {
            let mut delta = (9 * (q0v - p0) - 3 * (q1 - p1) + 8) >> 4;
            if delta.abs() >= tc * 10 {
                continue;
            }
            delta = delta.clamp(-tc, tc);
            if !f.no_p {
                d[at(k, -1)] = (p0 + delta).clamp(0, max) as u8;
                if dep {
                    let dp = ((((p2 + p0 + 1) >> 1) - p1 + delta) >> 1).clamp(-(tc >> 1), tc >> 1);
                    d[at(k, -2)] = (p1 + dp).clamp(0, max) as u8;
                }
            }
            if !f.no_q {
                d[at(k, 0)] = (q0v - delta).clamp(0, max) as u8;
                if deq {
                    let dq = ((((q2 + q0v + 1) >> 1) - q1 - delta) >> 1).clamp(-(tc >> 1), tc >> 1);
                    d[at(k, 1)] = (q1 + dq).clamp(0, max) as u8;
                }
            }
        }
    }
}

/// The chroma filter (bS 2) of one line across an edge.
fn filter_chroma_line(d: &mut [u8], q0: usize, across: isize, tc: i32, no_p: bool, no_q: bool, bd: u32) {
    let at = |i: isize| (q0 as isize + i * across) as usize;
    let (p0, p1, q0v, q1) = (d[at(-1)] as i32, d[at(-2)] as i32, d[at(0)] as i32, d[at(1)] as i32);
    let delta = ((((q0v - p0) << 2) + p1 - q1 + 4) >> 3).clamp(-tc, tc);
    let max = (1i32 << bd) - 1;
    if !no_p {
        d[at(-1)] = (p0 + delta).clamp(0, max) as u8;
    }
    if !no_q {
        d[at(0)] = (q0v - delta).clamp(0, max) as u8;
    }
}

fn chroma_tc(f: &EdgeFacts, offset: i32, bd: u32) -> i32 {
    let qpc = qpc_420(((f.qp_q + f.qp_p + 1) >> 1) + offset);
    TC_TABLE[(qpc + 2 * (BS_INTRA - 1) + (f.tc_offset_div2 << 1)).clamp(0, 53) as usize] as i32 * (1 << (bd - 8))
}

/// 8.7.2 over the whole picture.
pub fn deblock_picture(pic: &mut Picture, geo: &Geometry, sps: &Sps, pps: &Pps) {
    let pcm_lf_off = sps.pcm.is_some_and(|p| p.loop_filter_disabled);
    let (w, h) = (pic.width, pic.height);
    let (bdy, bdc) = (sps.bit_depth_luma, sps.bit_depth_chroma);
    for vertical in [true, false] {
        let flag = if vertical { F_EDGE_V } else { F_EDGE_H };
        // Luma: edges every 8 samples, segments of 4 lines.
        let (stride, lw, lh) = (pic.planes[0].w, w, h);
        let (outer, inner) = if vertical { (lw, lh) } else { (lh, lw) };
        for e in (8..outer).step_by(8) {
            for s in (0..inner).step_by(4) {
                let (q, p) = if vertical { ((e, s), (e - 1, s)) } else { ((s, e), (s, e - 1)) };
                let Some(f) = edge_facts(pic, geo, pps, pcm_lf_off, p, q, flag) else { continue };
                let q0 = q.1 * stride + q.0;
                let (across, along) = if vertical { (1isize, stride as isize) } else { (stride as isize, 1isize) };
                filter_luma(&mut pic.planes[0].data, q0, across, along, &f, bdy);
            }
        }
        // Chroma: edges every 8 chroma samples (16 luma), one line at a time.
        let (cw, ch) = (w / 2, h / 2);
        let cstride = pic.planes[1].w;
        let (outer, inner) = if vertical { (cw, ch) } else { (ch, cw) };
        for e in (8..outer).step_by(8) {
            for s in 0..inner {
                let (qc, pc) = if vertical { ((e, s), (e - 1, s)) } else { ((s, e), (s, e - 1)) };
                let q = (qc.0 * 2, qc.1 * 2);
                let p = (pc.0 * 2 + if vertical { 1 } else { 0 }, pc.1 * 2 + if vertical { 0 } else { 1 });
                let Some(f) = edge_facts(pic, geo, pps, pcm_lf_off, p, q, flag) else { continue };
                if f.bs != BS_INTRA {
                    continue;
                }
                let q0 = qc.1 * cstride + qc.0;
                let across = if vertical { 1isize } else { cstride as isize };
                let tc_cb = chroma_tc(&f, pps.cb_qp_offset, bdc);
                let tc_cr = chroma_tc(&f, pps.cr_qp_offset, bdc);
                filter_chroma_line(&mut pic.planes[1].data, q0, across, tc_cb, f.no_p, f.no_q, bdc);
                filter_chroma_line(&mut pic.planes[2].data, q0, across, tc_cr, f.no_p, f.no_q, bdc);
            }
        }
    }
}
