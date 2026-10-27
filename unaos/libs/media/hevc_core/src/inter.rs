// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria (A/V) — seam `shared-core` (hevc_core's inter prediction).
//!
//! HEVCINTER (LEDGER SR216): the decoding process for coding units coded in inter prediction mode (8.5.3), written
//! from ITU-T Rec. H.265: the prediction block geometry of each PartMode, the availability of prediction blocks
//! (6.4.2), the merge mode (8.5.3.2.2 .. 8.5.3.2.5: parallel merge level, spatial A1 B1 B0 A0 B2 with their pruning,
//! the temporal candidate, combined bi-predictive and zero candidates, the 8x4 / 4x8 bi-prediction restriction),
//! motion vector prediction (8.5.3.2.6 / 8.5.3.2.7 with POC-distance scaling), temporal motion vector prediction
//! (8.5.3.2.8 / 8.5.3.2.9), the fractional sample interpolation (8.5.3.3.3: luma 8-tap, chroma 4-tap, 14-bit
//! intermediates) and the weighted sample prediction (8.5.3.3.4.2 default, 8.5.3.3.4.3 explicit). The syntax is
//! parsed in [`crate::ctu`]; h264_core's 6-tap / bilinear motion compensation is a different filter and is not used.

use alloc::vec;
use alloc::vec::Vec;

use crate::dpb::{Dpb, RefList};
use crate::picture::{Picture, Plane, PuMotion};
use crate::ps::{Geometry, Pps};
use crate::slice::{SliceHeader, SliceType};
use crate::tables::{CHROMA_FILTER, COMB_IDX, LUMA_FILTER};

/// inter_pred_idc values (Table 7-15).
pub const PRED_L0: u32 = 0;
pub const PRED_L1: u32 = 1;
pub const PRED_BI: u32 = 2;

/// PartMode of an inter coding unit (Table 7-10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    P2Nx2N,
    P2NxN,
    PNx2N,
    PNxN,
    P2NxnU,
    P2NxnD,
    PnLx2N,
    PnRx2N,
}

impl Part {
    /// The prediction blocks (x, y, w, h) of an n x n coding block at (x0, y0), in partIdx order (7.3.8.5).
    pub fn pus(self, x0: i32, y0: i32, n: i32) -> Vec<(i32, i32, i32, i32)> {
        let (h, q, q3) = (n / 2, n / 4, 3 * n / 4);
        match self {
            Part::P2Nx2N => vec![(x0, y0, n, n)],
            Part::P2NxN => vec![(x0, y0, n, h), (x0, y0 + h, n, h)],
            Part::PNx2N => vec![(x0, y0, h, n), (x0 + h, y0, h, n)],
            Part::PNxN => vec![(x0, y0, h, h), (x0 + h, y0, h, h), (x0, y0 + h, h, h), (x0 + h, y0 + h, h, h)],
            Part::P2NxnU => vec![(x0, y0, n, q), (x0, y0 + q, n, q3)],
            Part::P2NxnD => vec![(x0, y0, n, q3), (x0, y0 + q3, n, q)],
            Part::PnLx2N => vec![(x0, y0, q, n), (x0 + q, y0, q3, n)],
            Part::PnRx2N => vec![(x0, y0, q3, n), (x0 + q3, y0, q, n)],
        }
    }
}

/// One prediction block: its coding block (xCb, yCb, nCbS), its own position and size, PartMode and partIdx.
#[derive(Clone, Copy, Debug)]
pub struct Pb {
    pub xc: i32,
    pub yc: i32,
    pub ncb: i32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub part: Part,
    pub idx: usize,
}

/// What an inter slice predicts from: its reference picture lists over the DPB, the current PicOrderCntVal, the
/// collocated picture of the temporal predictor (slice_temporal_mvp_enabled_flag) and NoBackwardPredFlag.
#[derive(Clone, Copy)]
pub struct InterCtx<'a> {
    pub lists: &'a [RefList; 2],
    pub dpb: &'a Dpb,
    pub poc: i32,
    pub col: Option<&'a Picture>,
    pub no_backward_pred: bool,
}

impl<'a> InterCtx<'a> {
    pub fn new(lists: &'a [RefList; 2], dpb: &'a Dpb, poc: i32, sh: &SliceHeader) -> InterCtx<'a> {
        let col = if sh.temporal_mvp_enabled_flag {
            let l = if sh.slice_type == SliceType::B && !sh.collocated_from_l0_flag { 1 } else { 0 };
            lists[l].dpb.get(sh.collocated_ref_idx as usize).and_then(|&i| dpb.pics.get(i)).map(|d| &d.pic)
        } else {
            None
        };
        // NoBackwardPredFlag (8.5.3.2.9): DiffPicOrderCnt(aPic, CurrPic) <= 0 for every picture of both lists.
        let no_backward_pred = lists.iter().all(|l| l.poc.iter().all(|&p| p <= poc));
        InterCtx { lists, dpb, poc, col, no_backward_pred }
    }

    /// The reference picture RefPicListX[refIdx].
    pub fn ref_pic(&self, x: usize, ref_idx: i8) -> Option<&'a Picture> {
        let i = *self.lists[x].dpb.get(usize::try_from(ref_idx).ok()?)?;
        self.dpb.pics.get(i).map(|d| &d.pic)
    }

    /// Normalise a motion (an unused list reset) and fill each used list's reference POC and long-term flag.
    pub fn complete(&self, m: &mut PuMotion) {
        for x in 0..2 {
            let r = if m.uses(x) { usize::try_from(m.ref_idx[x]).ok().filter(|&r| r < self.lists[x].len()) } else { None };
            match r {
                Some(r) => {
                    m.ref_poc[x] = self.lists[x].poc[r];
                    m.ref_lt[x] = self.lists[x].long_term[r];
                }
                None => {
                    m.pred &= !(1 << x);
                    m.ref_idx[x] = -1;
                    m.mv[x] = [0, 0];
                    m.ref_poc[x] = 0;
                    m.ref_lt[x] = false;
                }
            }
        }
    }
}

/// 6.4.2: the motion of the prediction block covering (xn, yn) when it is available to `b` and inter-coded.
fn neighbour(pic: &Picture, geo: &Geometry, b: &Pb, xn: i32, yn: i32) -> Option<PuMotion> {
    let same_cb = b.xc <= xn && xn < b.xc + b.ncb && b.yc <= yn && yn < b.yc + b.ncb;
    let avail = if !same_cb {
        pic.available(geo, b.x, b.y, xn, yn)
    } else {
        !((b.w << 1) == b.ncb && (b.h << 1) == b.ncb && b.idx == 1 && b.yc + b.h <= yn && b.xc + b.w > xn)
    };
    if !avail || xn < 0 || yn < 0 {
        return None;
    }
    let m = pic.motion[pic.idx4(xn as usize, yn as usize)];
    if m.pred == 0 { None } else { Some(m) }
}

/// The 16-bit wrap of mvLX = mvpLX + mvdLX (8-272 .. 8-275).
pub fn wrap_mv(v: i32) -> i32 {
    let u = (v as i64).rem_euclid(1 << 16);
    (if u >= 1 << 15 { u - (1 << 16) } else { u }) as i32
}

/// The POC-distance scaling of a motion vector (8-206 .. 8-211 / 8-183 .. 8-188): td and tb are the unclipped
/// DiffPicOrderCnt values.
fn scale_mv(mv: [i32; 2], td: i32, tb: i32) -> [i32; 2] {
    let td = td.clamp(-128, 127);
    let tb = tb.clamp(-128, 127);
    if td == 0 {
        return mv;
    }
    let tx = (16384 + (td.abs() >> 1)) / td;
    let dsf = ((tb * tx + 32) >> 6).clamp(-4096, 4095);
    mv.map(|v| {
        let p = dsf * v;
        (p.signum() * ((p.abs() + 127) >> 8)).clamp(-32768, 32767)
    })
}

/// 8.5.3.2.9: the collocated motion vector of list `x` for reference index `ref_idx` at the modified location
/// (xc, yc) of ColPic.
fn col_mv(ctx: &InterCtx, col: &Picture, sh: &SliceHeader, xc: i32, yc: i32, x: usize, ref_idx: usize) -> Option<[i32; 2]> {
    if xc < 0 || yc < 0 || xc as usize >= col.width || yc as usize >= col.height {
        return None;
    }
    let m = col.motion[col.idx4(xc as usize, yc as usize)];
    if m.pred == 0 {
        return None;
    }
    let lc = if !m.uses(0) {
        1
    } else if !m.uses(1) {
        0
    } else if ctx.no_backward_pred {
        x
    } else {
        sh.collocated_from_l0_flag as usize
    };
    let cur_lt = *ctx.lists[x].long_term.get(ref_idx)?;
    if cur_lt != m.ref_lt[lc] {
        return None;
    }
    let col_diff = col.poc - m.ref_poc[lc];
    let cur_diff = ctx.poc - ctx.lists[x].poc[ref_idx];
    if cur_lt || col_diff == cur_diff { Some(m.mv[lc]) } else { Some(scale_mv(m.mv[lc], col_diff, cur_diff)) }
}

/// 8.5.3.2.8: the temporal luma motion vector prediction of list `x`, reference index `ref_idx` — the bottom-right
/// collocated block when it lies in the same CTB row and inside the picture, else the centre one, each at the 16x16
/// granularity ((x >> 4) << 4).
fn temporal(ctx: &InterCtx, pic: &Picture, log2_ctb: u32, sh: &SliceHeader, b: &Pb, x: usize, ref_idx: usize) -> Option<[i32; 2]> {
    let col = ctx.col?;
    let (xbr, ybr) = (b.x + b.w, b.y + b.h);
    if (b.y >> log2_ctb) == (ybr >> log2_ctb) && (ybr as usize) < pic.height && (xbr as usize) < pic.width {
        if let Some(mv) = col_mv(ctx, col, sh, (xbr >> 4) << 4, (ybr >> 4) << 4, x, ref_idx) {
            return Some(mv);
        }
    }
    let (xc, yc) = (b.x + (b.w >> 1), b.y + (b.h >> 1));
    col_mv(ctx, col, sh, (xc >> 4) << 4, (yc >> 4) << 4, x, ref_idx)
}

/// 8.5.3.2.2 .. 8.5.3.2.5: the motion of merge candidate `merge_idx` for prediction block `b0`.
pub fn merge_motion(pic: &Picture, geo: &Geometry, ctx: &InterCtx, sh: &SliceHeader, pps: &Pps, b0: &Pb, merge_idx: usize) -> PuMotion {
    let plevel = pps.log2_parallel_merge_level;
    // singleMCLFlag: every prediction unit of an 8x8 coding unit shares the 2Nx2N candidate list.
    let b = if plevel > 2 && b0.ncb == 8 { Pb { x: b0.xc, y: b0.yc, w: b0.ncb, h: b0.ncb, idx: 0, ..*b0 } } else { *b0 };
    let get = |xn: i32, yn: i32| {
        if (b.x >> plevel) == (xn >> plevel) && (b.y >> plevel) == (yn >> plevel) { None } else { neighbour(pic, geo, &b, xn, yn) }
    };
    let second_vert = matches!(b.part, Part::PNx2N | Part::PnLx2N | Part::PnRx2N) && b.idx == 1;
    let second_hor = matches!(b.part, Part::P2NxN | Part::P2NxnU | Part::P2NxnD) && b.idx == 1;
    // 8.5.3.2.3 spatial candidates.
    let a1 = if second_vert { None } else { get(b.x - 1, b.y + b.h - 1) };
    let mut b1 = if second_hor { None } else { get(b.x + b.w - 1, b.y - 1) };
    if b1.is_some() && b1 == a1 {
        b1 = None;
    }
    let mut bb0 = get(b.x + b.w, b.y - 1);
    if bb0.is_some() && bb0 == b1 {
        bb0 = None;
    }
    let mut a0 = get(b.x - 1, b.y + b.h);
    if a0.is_some() && a0 == a1 {
        a0 = None;
    }
    let four = [a0, a1, bb0, b1].iter().filter(|c| c.is_some()).count() == 4;
    let mut b2 = if four { None } else { get(b.x - 1, b.y - 1) };
    if b2.is_some() && (b2 == a1 || b2 == b1) {
        b2 = None;
    }
    let mut list: Vec<PuMotion> = [a1, b1, bb0, a0, b2].iter().flatten().copied().collect();
    let is_b = sh.slice_type == SliceType::B;
    // The temporal candidate (refIdxLXCol 0).
    if sh.temporal_mvp_enabled_flag {
        let mut c = PuMotion::default();
        for x in 0..if is_b { 2 } else { 1 } {
            if let Some(mv) = temporal(ctx, pic, geo.log2_ctb, sh, &b, x, 0) {
                c.pred |= 1 << x;
                c.ref_idx[x] = 0;
                c.mv[x] = mv;
            }
        }
        if c.pred != 0 {
            ctx.complete(&mut c);
            list.push(c);
        }
    }
    let max = (sh.max_num_merge_cand as usize).max(merge_idx + 1);
    // 8.5.3.2.4 combined bi-predictive candidates.
    let orig = list.len();
    if is_b && orig > 1 && orig < max {
        let mut ci = 0;
        loop {
            let (i0, i1) = COMB_IDX[ci];
            let (l0, l1) = (list[i0], list[i1]);
            if l0.uses(0) && l1.uses(1) && (l0.ref_poc[0] != l1.ref_poc[1] || l0.mv[0] != l1.mv[1]) {
                let mut c = PuMotion { pred: 3, ref_idx: [l0.ref_idx[0], l1.ref_idx[1]], mv: [l0.mv[0], l1.mv[1]], ..Default::default() };
                ctx.complete(&mut c);
                list.push(c);
            }
            ci += 1;
            if ci == orig * (orig - 1) || list.len() == max {
                break;
            }
        }
    }
    // 8.5.3.2.5 zero candidates.
    let nref = if is_b { ctx.lists[0].len().min(ctx.lists[1].len()) } else { ctx.lists[0].len() };
    let mut zero = 0usize;
    while list.len() < max {
        let r = if zero < nref { zero as i8 } else { 0 };
        let mut c = if is_b {
            PuMotion { pred: 3, ref_idx: [r, r], ..Default::default() }
        } else {
            PuMotion { pred: 1, ref_idx: [r, -1], ..Default::default() }
        };
        ctx.complete(&mut c);
        list.push(c);
        zero += 1;
    }
    let mut m = list[merge_idx];
    // 8.5.3.2.2: an 8x4 / 4x8 prediction block is not bi-predicted.
    if m.pred == 3 && b0.w + b0.h == 12 {
        m.pred = 1;
        ctx.complete(&mut m);
    }
    m
}

/// 8.5.3.2.6 / 8.5.3.2.7: mvpListLX (two predictors) of list `x` and reference index `ref_idx` for block `b`.
pub fn amvp(pic: &Picture, geo: &Geometry, ctx: &InterCtx, sh: &SliceHeader, b: &Pb, x: usize, ref_idx: usize) -> [[i32; 2]; 2] {
    let y = 1 - x;
    let target_poc = ctx.lists[x].poc.get(ref_idx).copied().unwrap_or(ctx.poc);
    let target_lt = ctx.lists[x].long_term.get(ref_idx).copied().unwrap_or(false);
    let same = |m: &PuMotion| -> Option<[i32; 2]> {
        if m.uses(x) && m.ref_poc[x] == target_poc {
            Some(m.mv[x])
        } else if m.uses(y) && m.ref_poc[y] == target_poc {
            Some(m.mv[y])
        } else {
            None
        }
    };
    let scaled = |m: &PuMotion| -> Option<[i32; 2]> {
        for l in [x, y] {
            if m.uses(l) && m.ref_lt[l] == target_lt {
                if !target_lt {
                    return Some(scale_mv(m.mv[l], ctx.poc - m.ref_poc[l], ctx.poc - target_poc));
                }
                return Some(m.mv[l]);
            }
        }
        None
    };
    let a0 = neighbour(pic, geo, b, b.x - 1, b.y + b.h);
    let a1 = neighbour(pic, geo, b, b.x - 1, b.y + b.h - 1);
    let is_scaled = a0.is_some() || a1.is_some();
    let mut mva = None;
    for m in [a0, a1].iter().flatten() {
        if mva.is_none() {
            mva = same(m);
        }
    }
    for m in [a0, a1].iter().flatten() {
        if mva.is_none() {
            mva = scaled(m);
        }
    }
    let bs = [
        neighbour(pic, geo, b, b.x + b.w, b.y - 1),
        neighbour(pic, geo, b, b.x + b.w - 1, b.y - 1),
        neighbour(pic, geo, b, b.x - 1, b.y - 1),
    ];
    let mut mvb = None;
    for m in bs.iter().flatten() {
        if mvb.is_none() {
            mvb = same(m);
        }
    }
    if !is_scaled && mvb.is_some() {
        mva = mvb;
    }
    if !is_scaled {
        mvb = None;
        for m in bs.iter().flatten() {
            if mvb.is_none() {
                mvb = scaled(m);
            }
        }
    }
    let col = if mva.is_some() && mvb.is_some() && mva != mvb {
        None
    } else if sh.temporal_mvp_enabled_flag {
        temporal(ctx, pic, geo.log2_ctb, sh, b, x, ref_idx)
    } else {
        None
    };
    let mut list: Vec<[i32; 2]> = Vec::with_capacity(3);
    if let Some(a) = mva {
        list.push(a);
    }
    if let Some(v) = mvb {
        if mva != Some(v) {
            list.push(v);
        }
    }
    if list.len() < 2 {
        if let Some(c) = col {
            list.push(c);
        }
    }
    while list.len() < 2 {
        list.push([0, 0]);
    }
    [list[0], list[1]]
}

/// 8.5.3.3.3.1: the w x h luma prediction samples (14-bit intermediates, predSamplesLX) of the block at (xp, yp) with
/// motion vector `mv` (quarter samples) from reference plane `r`; reference samples outside the picture are the
/// nearest edge samples.
pub fn luma_mc(r: &Plane, xp: i32, yp: i32, mv: [i32; 2], w: i32, h: i32, bd: u32, out: &mut [i32]) {
    let (xf, yf) = ((mv[0] & 3) as usize, (mv[1] & 3) as usize);
    let (xi, yi) = (xp + (mv[0] >> 2), yp + (mv[1] >> 2));
    let shift1 = bd.min(12) as i32 - 8;
    let shift3 = (14 - bd as i32).max(2);
    let (mx, my) = (r.w as i32 - 1, r.h as i32 - 1);
    let s = |x: i32, y: i32| r.at(x.clamp(0, mx) as usize, y.clamp(0, my) as usize) as i32;
    let (fx, fy) = (&LUMA_FILTER[xf], &LUMA_FILTER[yf]);
    for y in 0..h {
        for x in 0..w {
            let (x0, y0) = (xi + x, yi + y);
            let v = if xf == 0 && yf == 0 {
                s(x0, y0) << shift3
            } else if yf == 0 {
                (0..8).map(|i| fx[i] * s(x0 + i as i32 - 3, y0)).sum::<i32>() >> shift1
            } else if xf == 0 {
                (0..8).map(|i| fy[i] * s(x0, y0 + i as i32 - 3)).sum::<i32>() >> shift1
            } else {
                let mut acc = 0;
                for n in 0..8 {
                    let t = (0..8).map(|i| fx[i] * s(x0 + i as i32 - 3, y0 + n as i32 - 3)).sum::<i32>() >> shift1;
                    acc += fy[n] * t;
                }
                acc >> 6
            };
            out[(y * w + x) as usize] = v;
        }
    }
}

/// 8.5.3.3.3.2: the w x h chroma prediction samples of the 4:2:0 chroma block at (xc, yc) (chroma samples) with the
/// luma motion vector `mv`, read as eighth chroma samples (mvCLX = mvLX).
pub fn chroma_mc(r: &Plane, xc: i32, yc: i32, mv: [i32; 2], w: i32, h: i32, bd: u32, out: &mut [i32]) {
    let (xf, yf) = ((mv[0] & 7) as usize, (mv[1] & 7) as usize);
    let (xi, yi) = (xc + (mv[0] >> 3), yc + (mv[1] >> 3));
    let shift1 = bd.min(12) as i32 - 8;
    let shift3 = (14 - bd as i32).max(2);
    let (mx, my) = (r.w as i32 - 1, r.h as i32 - 1);
    let s = |x: i32, y: i32| r.at(x.clamp(0, mx) as usize, y.clamp(0, my) as usize) as i32;
    let (fx, fy) = (&CHROMA_FILTER[xf], &CHROMA_FILTER[yf]);
    for y in 0..h {
        for x in 0..w {
            let (x0, y0) = (xi + x, yi + y);
            let v = if xf == 0 && yf == 0 {
                s(x0, y0) << shift3
            } else if yf == 0 {
                (0..4).map(|i| fx[i] * s(x0 + i as i32 - 1, y0)).sum::<i32>() >> shift1
            } else if xf == 0 {
                (0..4).map(|i| fy[i] * s(x0, y0 + i as i32 - 1)).sum::<i32>() >> shift1
            } else {
                let mut acc = 0;
                for n in 0..4 {
                    let t = (0..4).map(|i| fx[i] * s(x0 + i as i32 - 1, y0 + n as i32 - 1)).sum::<i32>() >> shift1;
                    acc += fy[n] * t;
                }
                acc >> 6
            };
            out[(y * w + x) as usize] = v;
        }
    }
}

/// The explicit weight, offset and log2WD of list `l`, reference index `ref_idx`, component `c` (7-56, 8.5.3.3.4.3).
fn explicit_weight(sh: &SliceHeader, c: usize, l: usize, ref_idx: i8, bd: u32) -> (i32, i32, u32) {
    let shift1 = 14 - bd;
    let Some(pw) = sh.pred_weights.as_ref() else { return (1, 0, shift1) };
    let wl = if l == 0 { &pw.l0 } else { &pw.l1 };
    let r = usize::try_from(ref_idx).unwrap_or(0);
    if c == 0 {
        let d = pw.luma_log2_weight_denom;
        let (w, o) = wl.luma.get(r).copied().unwrap_or((1 << d, 0));
        (w, o << (bd - 8), d + shift1)
    } else {
        let d = pw.chroma_log2_weight_denom;
        let (w, delta) = wl.chroma.get(r).map(|e| e[c - 1]).unwrap_or((1 << d, 0));
        let o = (128 + delta - ((128 * w) >> d)).clamp(-128, 127);
        (w, o << (bd - 8), d + shift1)
    }
}

/// 8.5.3.3: predict the samples of prediction block (x, y, w, h) with motion `m` into the current picture's planes
/// (default or explicit weighted sample prediction).
pub fn predict_pu(pic: &mut Picture, ctx: &InterCtx, sh: &SliceHeader, pps: &Pps, bd_y: u32, bd_c: u32, x: i32, y: i32, w: i32, h: i32, m: &PuMotion) {
    let weighted = match sh.slice_type {
        SliceType::P => pps.weighted_pred_flag,
        SliceType::B => pps.weighted_bipred_flag,
        SliceType::I => false,
    } && sh.pred_weights.is_some();
    for c in 0..3 {
        let (cx, cy, cw, ch) = if c == 0 { (x, y, w, h) } else { (x / 2, y / 2, w / 2, h / 2) };
        let bd = if c == 0 { bd_y } else { bd_c };
        let n = (cw * ch) as usize;
        let mut pred: [Option<Vec<i32>>; 2] = [None, None];
        for l in 0..2 {
            if !m.uses(l) {
                continue;
            }
            if let Some(r) = ctx.ref_pic(l, m.ref_idx[l]) {
                let mut v = vec![0i32; n];
                if c == 0 {
                    luma_mc(&r.planes[0], cx, cy, m.mv[l], cw, ch, bd, &mut v);
                } else {
                    chroma_mc(&r.planes[c], cx, cy, m.mv[l], cw, ch, bd, &mut v);
                }
                pred[l] = Some(v);
            }
        }
        let max = (1i32 << bd) - 1;
        let shift1 = 14 - bd as i32;
        let out: Vec<i32> = match (&pred[0], &pred[1]) {
            (Some(a), Some(b)) => {
                if weighted {
                    let (w0, o0, lwd) = explicit_weight(sh, c, 0, m.ref_idx[0], bd);
                    let (w1, o1, _) = explicit_weight(sh, c, 1, m.ref_idx[1], bd);
                    a.iter().zip(b.iter()).map(|(&p0, &p1)| (p0 * w0 + p1 * w1 + ((o0 + o1 + 1) << lwd)) >> (lwd + 1)).collect()
                } else {
                    let shift2 = 15 - bd as i32;
                    a.iter().zip(b.iter()).map(|(&p0, &p1)| (p0 + p1 + (1 << (shift2 - 1))) >> shift2).collect()
                }
            }
            (Some(a), None) | (None, Some(a)) => {
                let l = if pred[0].is_some() { 0 } else { 1 };
                if weighted {
                    let (w0, o0, lwd) = explicit_weight(sh, c, l, m.ref_idx[l], bd);
                    a.iter()
                        .map(|&p| if lwd >= 1 { ((p * w0 + (1 << (lwd - 1))) >> lwd) + o0 } else { p * w0 + o0 })
                        .collect()
                } else {
                    a.iter().map(|&p| (p + (1 << (shift1 - 1))) >> shift1).collect()
                }
            }
            (None, None) => continue,
        };
        let plane = &mut pic.planes[c];
        for j in 0..ch {
            for i in 0..cw {
                let (px, py) = ((cx + i) as usize, (cy + j) as usize);
                if px < plane.w && py < plane.h {
                    plane.set(px, py, out[(j * cw + i) as usize].clamp(0, max) as u8);
                }
            }
        }
    }
}
