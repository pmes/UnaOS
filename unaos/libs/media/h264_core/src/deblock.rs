// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The deblocking filter process (8.7) for frame macroblocks, run once the whole picture is
//! decoded (so slices arriving in any order are filtered against their final neighbours).
//!
//! Per macroblock in address order: the luma vertical edges left to right, then the horizontal
//! edges top to bottom (internal 4x4 edges only where transform_size_8x8_flag is 0), and the
//! chroma edges (chroma x / y = 0 and 4 for 4:2:0) with the bS of the corresponding luma edge.
//! Macroblock edges are skipped at the picture border, under disable_deblocking_filter_idc 1, and
//! against another slice under disable_deblocking_filter_idc 2 (8.7: filterLeftMbEdgeFlag /
//! filterTopMbEdgeFlag). The filter parameters (FilterOffsetA / B) are those of the slice holding
//! q0, the current macroblock.

use crate::picture::{blk_at, MbInfo, MbKind, Motion, Plane};
use crate::tables::{ALPHA, BETA, QPC, TC0};

/// A slice's deblocking controls.
#[derive(Clone, Copy, Debug, Default)]
pub struct SliceDeblock {
    pub disable: u32,
    pub alpha_offset: i32,
    pub beta_offset: i32,
}

/// QPY as the deblocking filter sees it (8.7.2.2: 0 for I_PCM).
fn qp_y(i: &MbInfo) -> i32 {
    if i.kind == MbKind::IPcm { 0 } else { i.qp as i32 }
}

/// QPc of a macroblock for chroma component `c` (8.5.8 with the PPS offsets).
fn qp_c(i: &MbInfo, off: i32) -> i32 {
    QPC[(qp_y(i) + off).clamp(0, 51) as usize] as i32
}

/// Boundary strength (8.7.2.1) between 4x4 luma block p (picture 4x4 index `gp` in macroblock
/// `mp`, luma4x4BlkIdx `bp`) and block q.
fn boundary_strength(info: &[MbInfo], mo: &Motion, (mp, bp, gp): (usize, usize, usize), (mq, bq, gq): (usize, usize, usize), mb_edge: bool) -> u8 {
    let (ip, iq) = (&info[mp], &info[mq]);
    if ip.kind.is_intra() || iq.kind.is_intra() {
        return if mb_edge { 4 } else { 3 };
    }
    if (ip.nz >> bp) & 1 == 1 || (iq.nz >> bq) & 1 == 1 {
        return 2;
    }
    let (rp, rq) = (mo.ref_idx[gp], mo.ref_idx[gq]);
    let (idp, idq) = (mo.ref_id[gp], mo.ref_id[gq]);
    let (mvp, mvq) = (mo.mv[gp], mo.mv[gq]);
    let far = |a: crate::picture::Mv, b: crate::picture::Mv| (a.x as i32 - b.x as i32).abs() >= 4 || (a.y as i32 - b.y as i32).abs() >= 4;
    let np = (rp[0] >= 0) as u32 + (rp[1] >= 0) as u32;
    let nq = (rq[0] >= 0) as u32 + (rq[1] >= 0) as u32;
    if np != nq {
        return 1;
    }
    if np == 1 {
        let (lp, lq) = (if rp[0] >= 0 { 0 } else { 1 }, if rq[0] >= 0 { 0 } else { 1 });
        if idp[lp] != idq[lq] {
            return 1;
        }
        return far(mvp[lp], mvq[lq]) as u8;
    }
    if np == 0 {
        return 0;
    }
    // Two motion vectors each.
    let same_direct = idp[0] == idq[0] && idp[1] == idq[1];
    let same_cross = idp[0] == idq[1] && idp[1] == idq[0];
    if !same_direct && !same_cross {
        return 1;
    }
    if idp[0] != idp[1] {
        // Two different reference pictures: compare the vectors for the same picture.
        if same_direct {
            (far(mvp[0], mvq[0]) || far(mvp[1], mvq[1])) as u8
        } else {
            (far(mvp[0], mvq[1]) || far(mvp[1], mvq[0])) as u8
        }
    } else {
        // Both vectors reference the same picture: both pairings must be far.
        ((far(mvp[0], mvq[0]) || far(mvp[1], mvq[1])) && (far(mvp[0], mvq[1]) || far(mvp[1], mvq[0]))) as u8
    }
}

/// Filter one line of samples across an edge (8.7.2.3 / 8.7.2.4). `q0` indexes q0, `step` is
/// the distance from p0 to q0 (1 across a vertical edge, the stride across a horizontal one).
#[inline]
fn filter_line(d: &mut [u8], q0: usize, step: usize, bs: u8, alpha: i32, beta: i32, tc0: i32, chroma: bool) {
    let p = |k: usize| d[q0 - (k + 1) * step] as i32;
    let q = |k: usize| d[q0 + k * step] as i32;
    let (p0, p1, q0v, q1) = (p(0), p(1), q(0), q(1));
    if !((p0 - q0v).abs() < alpha && (p1 - p0).abs() < beta && (q1 - q0v).abs() < beta) {
        return;
    }
    if chroma {
        if bs < 4 {
            let tc = tc0 + 1;
            let delta = ((((q0v - p0) << 2) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
            d[q0 - step] = (p0 + delta).clamp(0, 255) as u8;
            d[q0] = (q0v - delta).clamp(0, 255) as u8;
        } else {
            d[q0 - step] = ((2 * p1 + p0 + q1 + 2) >> 2) as u8;
            d[q0] = ((2 * q1 + q0v + p1 + 2) >> 2) as u8;
        }
        return;
    }
    let (p2, q2) = (p(2), q(2));
    let ap = (p2 - p0).abs();
    let aq = (q2 - q0v).abs();
    if bs < 4 {
        let tc = tc0 + (ap < beta) as i32 + (aq < beta) as i32;
        let delta = ((((q0v - p0) << 2) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
        d[q0 - step] = (p0 + delta).clamp(0, 255) as u8;
        d[q0] = (q0v - delta).clamp(0, 255) as u8;
        if ap < beta {
            d[q0 - 2 * step] = (p1 + ((p2 + ((p0 + q0v + 1) >> 1) - (p1 << 1)) >> 1).clamp(-tc0, tc0)) as u8;
        }
        if aq < beta {
            d[q0 + step] = (q1 + ((q2 + ((p0 + q0v + 1) >> 1) - (q1 << 1)) >> 1).clamp(-tc0, tc0)) as u8;
        }
    } else {
        let strong = (p0 - q0v).abs() < ((alpha >> 2) + 2);
        let p3 = p(3);
        let q3 = q(3);
        if ap < beta && strong {
            d[q0 - step] = ((p2 + 2 * p1 + 2 * p0 + 2 * q0v + q1 + 4) >> 3) as u8;
            d[q0 - 2 * step] = ((p2 + p1 + p0 + q0v + 2) >> 2) as u8;
            d[q0 - 3 * step] = ((2 * p3 + 3 * p2 + p1 + p0 + q0v + 4) >> 3) as u8;
        } else {
            d[q0 - step] = ((2 * p1 + p0 + q1 + 2) >> 2) as u8;
        }
        if aq < beta && strong {
            d[q0] = ((p1 + 2 * p0 + 2 * q0v + 2 * q1 + q2 + 4) >> 3) as u8;
            d[q0 + step] = ((p0 + q0v + q1 + q2 + 2) >> 2) as u8;
            d[q0 + 2 * step] = ((2 * q3 + 3 * q2 + q1 + q0v + p0 + 4) >> 3) as u8;
        } else {
            d[q0] = ((2 * q1 + q0v + p1 + 2) >> 2) as u8;
        }
    }
}

/// Deblock a decoded frame of `mb_w` x `mb_h` macroblocks.
pub fn deblock_picture(planes: &mut [Plane; 3], mo: &Motion, info: &[MbInfo], slices: &[SliceDeblock], mb_w: usize, mb_h: usize, chroma_qp_offset: [i32; 2]) {
    let w4 = mb_w * 4;
    for addr in 0..mb_w * mb_h {
        let cur = &info[addr];
        if cur.slice == u32::MAX {
            continue;
        }
        let Some(sp) = slices.get(cur.slice as usize) else { continue };
        if sp.disable == 1 {
            continue;
        }
        let (mbx, mby) = (addr % mb_w, addr / mb_w);
        let usable = |n: usize| info[n].slice != u32::MAX && (sp.disable != 2 || info[n].slice == cur.slice);
        let left = mbx > 0 && usable(addr - 1);
        let top = mby > 0 && usable(addr - mb_w);
        for dir in 0..2 {
            // dir 0: vertical edges (filtering horizontally); dir 1: horizontal edges.
            for e in 0..4usize {
                if e == 0 && !(if dir == 0 { left } else { top }) {
                    continue;
                }
                if e % 2 == 1 && cur.t8x8 {
                    continue;
                }
                let nbr = if e == 0 { if dir == 0 { addr - 1 } else { addr - mb_w } } else { addr };
                // bS per 4-sample segment k along the edge.
                let mut bs = [0u8; 4];
                for k in 0..4 {
                    let (qx, qy) = if dir == 0 { (e, k) } else { (k, e) };
                    let (px, py) = if dir == 0 { ((e + 3) % 4, k) } else { (k, (e + 3) % 4) };
                    let gq = (mby * 4 + qy) * w4 + mbx * 4 + qx;
                    let (nbx, nby) = (nbr % mb_w, nbr / mb_w);
                    let gp = (nby * 4 + py) * w4 + nbx * 4 + px;
                    bs[k] = boundary_strength(info, mo, (nbr, blk_at(px, py), gp), (addr, blk_at(qx, qy), gq), e == 0);
                }
                if bs == [0; 4] {
                    continue;
                }
                let pinf = &info[nbr];
                // Luma.
                {
                    let qpav = (qp_y(pinf) + qp_y(cur) + 1) >> 1;
                    let ia = (qpav + sp.alpha_offset).clamp(0, 51) as usize;
                    let ib = (qpav + sp.beta_offset).clamp(0, 51) as usize;
                    let (alpha, beta) = (ALPHA[ia] as i32, BETA[ib] as i32);
                    let pl = &mut planes[0];
                    let stride = pl.w;
                    for k in 0..16 {
                        let b = bs[k / 4];
                        if b == 0 {
                            continue;
                        }
                        let tc0 = if b < 4 { TC0[ia][b as usize - 1] as i32 } else { 0 };
                        let (x, y, step) = if dir == 0 { (mbx * 16 + e * 4, mby * 16 + k, 1) } else { (mbx * 16 + k, mby * 16 + e * 4, stride) };
                        filter_line(&mut pl.data, y * stride + x, step, b, alpha, beta, tc0, false);
                    }
                }
                // Chroma: the edges at chroma 0 and 4 (luma edges 0 and 2).
                if e % 2 == 0 {
                    for c in 0..2 {
                        let qpav = (qp_c(pinf, chroma_qp_offset[c]) + qp_c(cur, chroma_qp_offset[c]) + 1) >> 1;
                        let ia = (qpav + sp.alpha_offset).clamp(0, 51) as usize;
                        let ib = (qpav + sp.beta_offset).clamp(0, 51) as usize;
                        let (alpha, beta) = (ALPHA[ia] as i32, BETA[ib] as i32);
                        let pl = &mut planes[1 + c];
                        let stride = pl.w;
                        for k in 0..8 {
                            let b = bs[k / 2];
                            if b == 0 {
                                continue;
                            }
                            let tc0 = if b < 4 { TC0[ia][b as usize - 1] as i32 } else { 0 };
                            let (x, y, step) = if dir == 0 { (mbx * 8 + e * 2, mby * 8 + k, 1) } else { (mbx * 8 + k, mby * 8 + e * 2, stride) };
                            filter_line(&mut pl.data, y * stride + x, step, b, alpha, beta, tc0, true);
                        }
                    }
                }
            }
        }
    }
}
