// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Inter macroblocks: motion vector prediction (8.4.1.3) and the P_Skip motion (8.4.1.1) over
//! whole-macroblock partitions, an integer-sample full search in a +-[`SEARCH`] window over the
//! encoder's own reconstruction of the previous frame, its half / quarter-sample refinement
//! ([`refine`], `Config::subpel`; SR211), and the P_Skip / P_L0_16x16 decision. A refinement
//! candidate is predicted by `luma_mc` itself, so the search scores the decoder's own samples.
//! Prediction samples come from h264_core's `inter::luma_mc` / `chroma_mc` (the chroma vector is
//! the luma vector in eighth chroma samples, as the decoder's `predict` passes it).
//!
//! The neighbour rule mirrors h264_core's `inter.rs` `nb_mv` / `nb_abc` / `mvp` / `p_skip_motion`
//! for one slice per picture with only 16x16 partitions: A is the macroblock to the left, B the
//! one above, C the one above-right or, when that is outside the picture, D the one above-left;
//! a neighbour inside the picture is available (every one is earlier in decoding order).
//!
//! H264ENCBFRAMES (SR333): B macroblocks ([`Encoder::b_mb`]) over two references (L0 the past anchor
//! `refp`, L1 the future one `fwd`) and two motion grids (`motion` list 0, `motion1` list 1, so
//! [`mvp`] serves either list unchanged): spatial direct ([`direct_spatial`], h264_core `inter.rs`
//! `spatial_direct_base` / `direct_motion` for one 16x16 partition and the anchor's whole-macroblock
//! co-located motion), B_Skip when its prediction leaves no residual, else the cheapest of
//! B_Direct_16x16 / B_L0_16x16 / B_L1_16x16 / B_Bi_16x16 by SAD + lambda x bits; the bi-prediction is
//! the decoder's default average `(a + b + 1) >> 1`.

use h264_core::inter::{chroma_mc, luma_mc};
use h264_core::picture::{Mv, Plane, BLK_X, BLK_Y};

use crate::nal::{se_bits, ue_bits};
use crate::transform::{code_chroma, fdct16, quant4, recon4};
use crate::{lambda, Encoder, Kind, MbCode};

/// The integer search range in samples, each direction (the refinement adds up to 3/4 sample).
pub const SEARCH: i32 = 8;

/// The motion of a coded macroblock: refIdxL0 (-1 = not inter / not coded yet) and mvL0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MbMotion {
    pub r: i32,
    pub mv: Mv,
}

impl MbMotion {
    pub const NONE: MbMotion = MbMotion { r: -1, mv: Mv::ZERO };
}

/// A neighbour as 8.4.1.3.2 sees it: availability, refIdxLXN, mvLXN (zero unless r >= 0).
#[derive(Clone, Copy)]
struct Nb {
    avail: bool,
    r: i32,
    mv: Mv,
}

fn nb(motion: &[MbMotion], mb_w: usize, x: isize, y: isize) -> Nb {
    if x < 0 || y < 0 || x >= mb_w as isize {
        return Nb { avail: false, r: -1, mv: Mv::ZERO };
    }
    let m = motion[y as usize * mb_w + x as usize];
    Nb { avail: true, r: m.r, mv: if m.r >= 0 { m.mv } else { Mv::ZERO } }
}

fn median(a: i32, b: i32, c: i32) -> i32 {
    a.max(b).min(a.min(b).max(c))
}

/// A, B and C (D when C is unavailable) of the 16x16 partition at macroblock (mbx, mby) (8.4.1.3.2).
fn abc(motion: &[MbMotion], mb_w: usize, mbx: usize, mby: usize) -> [Nb; 3] {
    let (x, y) = (mbx as isize, mby as isize);
    let a = nb(motion, mb_w, x - 1, y);
    let b = nb(motion, mb_w, x, y - 1);
    let mut c = nb(motion, mb_w, x + 1, y - 1);
    if !c.avail {
        c = nb(motion, mb_w, x - 1, y - 1);
    }
    [a, b, c]
}

/// mvpLX (8.4.1.3) of a 16x16 partition with refIdxLX 0 at macroblock (mbx, mby), over the list's
/// motion grid.
pub fn mvp(motion: &[MbMotion], mb_w: usize, mbx: usize, mby: usize) -> Mv {
    let [a, b, c] = abc(motion, mb_w, mbx, mby);
    // 8.4.1.3.1: only A available -> B and C take A.
    let (b, c) = if !b.avail && !c.avail && a.avail { (a, a) } else { (b, c) };
    let hits = (a.r == 0) as u32 + (b.r == 0) as u32 + (c.r == 0) as u32;
    if hits == 1 {
        return if a.r == 0 {
            a.mv
        } else if b.r == 0 {
            b.mv
        } else {
            c.mv
        };
    }
    Mv::new(median(a.mv.x as i32, b.mv.x as i32, c.mv.x as i32), median(a.mv.y as i32, b.mv.y as i32, c.mv.y as i32))
}

/// The P_Skip motion vector (8.4.1.1): zero when A or B is unavailable or is a zero vector on
/// reference 0, else mvpL0.
pub fn skip_mv(motion: &[MbMotion], mb_w: usize, mbx: usize, mby: usize) -> Mv {
    let (x, y) = (mbx as isize, mby as isize);
    let a = nb(motion, mb_w, x - 1, y);
    let b = nb(motion, mb_w, x, y - 1);
    if !a.avail || !b.avail || (a.r == 0 && a.mv == Mv::ZERO) || (b.r == 0 && b.mv == Mv::ZERO) {
        return Mv::ZERO;
    }
    mvp(motion, mb_w, mbx, mby)
}

fn min_positive(x: i32, y: i32) -> i32 {
    if x >= 0 && y >= 0 { x.min(y) } else { x.max(y) }
}

/// Spatial direct motion (8.4.1.2.2) of macroblock (mbx, mby) over the list-0 / list-1 grids `m0` /
/// `m1`, the co-located block being zero-motion on refIdx 0 (`col_zero`): per list refIdx = MinPositive
/// of A, B, C (both -1 -> both 0 with zero vectors), mv = mvpLX unless refIdx is 0 and `col_zero`.
/// h264_core `spatial_direct_base` + `direct_motion`; refIdx is -1 or 0 (one active reference per list).
pub fn direct_spatial(m0: &[MbMotion], m1: &[MbMotion], mb_w: usize, mbx: usize, mby: usize, col_zero: bool) -> [MbMotion; 2] {
    let mut r = [-1i32; 2];
    for (l, m) in [m0, m1].into_iter().enumerate() {
        let [a, b, c] = abc(m, mb_w, mbx, mby);
        r[l] = min_positive(a.r, min_positive(b.r, c.r));
    }
    let zero = r[0] < 0 && r[1] < 0;
    if zero {
        r = [0, 0];
    }
    let mut out = [MbMotion::NONE; 2];
    for (l, m) in [m0, m1].into_iter().enumerate() {
        out[l].r = r[l];
        if r[l] >= 0 && !zero && !(r[l] == 0 && col_zero) {
            out[l].mv = mvp(m, mb_w, mbx, mby);
        }
    }
    out
}

/// Integer-sample full search of the 16x16 luma block at (px, py) of `src` over `refp` (samples
/// outside it clamped, as `Plane::at` gives the decoder's reference padding) in +-[`SEARCH`]
/// around (0, 0); cost = SAD + `lam` x the bits of mvd against `mvp`. Returns the vector in
/// quarter samples.
pub fn search(src: &Plane, refp: &Plane, px: usize, py: usize, mvp: Mv, lam: u32) -> Mv {
    let mut best = (u32::MAX, Mv::ZERO);
    for dy in -SEARCH..=SEARCH {
        for dx in -SEARCH..=SEARCH {
            let mv = Mv::new(dx * 4, dy * 4);
            let mut cost = lam * (se_bits(mv.x as i32 - mvp.x as i32) + se_bits(mv.y as i32 - mvp.y as i32));
            if cost >= best.0 {
                continue;
            }
            'rows: for y in 0..16 {
                let row = (py + y) * src.w + px;
                for x in 0..16 {
                    let r = refp.at((px + x) as i32 + dx, (py + y) as i32 + dy) as i32;
                    cost += (src.data[row + x] as i32 - r).unsigned_abs();
                }
                if cost >= best.0 {
                    break 'rows;
                }
            }
            if cost < best.0 {
                best = (cost, mv);
            }
        }
    }
    best.1
}

/// Sub-sample refinement of the 16x16 luma block at (px, py) around the integer vector `mv`:
/// `depth` 0 returns `mv`; 1 tries its 8 half-sample neighbours; 2 then the 8 quarter-sample
/// neighbours of the best. Cost = SAD against `luma_mc`'s prediction + `lam` x the bits of mvd
/// against `mvp`; a candidate wins only with a strictly lower cost.
pub fn refine(src: &Plane, refp: &Plane, px: usize, py: usize, mv: Mv, mvp: Mv, lam: u32, depth: u8) -> Mv {
    if depth == 0 {
        return mv;
    }
    let mut buf = [0u8; 256];
    let mut cost = |m: Mv| -> u32 {
        let mut c = lam * (se_bits(m.x as i32 - mvp.x as i32) + se_bits(m.y as i32 - mvp.y as i32));
        luma_mc(refp, px as i32, py as i32, m, 16, 16, &mut buf);
        for y in 0..16 {
            let row = (py + y) * src.w + px;
            for x in 0..16 {
                c += (src.data[row + x] as i32 - buf[y * 16 + x] as i32).unsigned_abs();
            }
        }
        c
    };
    let mut best = (cost(mv), mv);
    for step in [2i32, 1].into_iter().take(depth.min(2) as usize) {
        let centre = best.1;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let m = Mv::new(centre.x as i32 + dx * step, centre.y as i32 + dy * step);
                let c = cost(m);
                if c < best.0 {
                    best = (c, m);
                }
            }
        }
    }
    best.1
}

impl Encoder {
    /// The motion-compensated prediction of macroblock (mbx, mby) for `mv` from the reference set
    /// `refs`: luma (16x16) and the two chroma blocks (8x8).
    fn inter_pred(&self, refs: &[Plane; 3], mbx: usize, mby: usize, mv: Mv) -> ([i32; 256], [[i32; 64]; 2]) {
        let (px, py) = ((mbx * 16) as i32, (mby * 16) as i32);
        let mut buf = [0u8; 256];
        luma_mc(&refs[0], px, py, mv, 16, 16, &mut buf);
        let mut luma = [0i32; 256];
        for k in 0..256 {
            luma[k] = buf[k] as i32;
        }
        let mut chroma = [[0i32; 64]; 2];
        for c in 0..2 {
            chroma_mc(&refs[1 + c], px / 2, py / 2, mv, 8, 8, &mut buf);
            for y in 0..8 {
                for x in 0..8 {
                    chroma[c][y * 8 + x] = buf[y * 16 + x] as i32;
                }
            }
        }
        (luma, chroma)
    }

    /// The inter levels of macroblock (mbx, mby) against a luma prediction (the chroma is coded
    /// and reconstructed by [`Encoder::chroma_mb`] once the vector is final).
    fn inter_luma(&self, mbx: usize, mby: usize, luma: &[i32; 256], kind: Kind) -> MbCode {
        let mut res = [0i32; 256];
        self.residual(0, mbx * 16, mby * 16, 16, luma, &mut res);
        let mut code = MbCode::new(kind);
        code.luma = fdct16(&res);
        for b in code.luma.iter_mut() {
            quant4(b, self.qp, false, false);
        }
        code.cbp = code.luma_cbp();
        code
    }

    /// Would the chroma residual against these predictions quantise to nothing?
    fn chroma_is_empty(&self, mbx: usize, mby: usize, chroma: &[[i32; 64]; 2]) -> bool {
        let mut res = [0i32; 64];
        for c in 0..2 {
            self.residual(1 + c, mbx * 8, mby * 8, 8, &chroma[c], &mut res);
            let (dc, ac) = code_chroma(&res, self.qpc, false);
            if dc.iter().any(|&v| v != 0) || ac.iter().flatten().any(|&v| v != 0) {
                return false;
            }
        }
        true
    }

    /// Decide, code and reconstruct the P macroblock (mbx, mby): P_Skip when the skip vector's
    /// residual quantises to nothing, else P_L0_16x16 with the searched vector.
    pub(crate) fn inter_mb(&mut self, mbx: usize, mby: usize) -> MbCode {
        let i = mby * self.mb_w + mbx;
        let skip = skip_mv(&self.motion, self.mb_w, mbx, mby);
        let (mut luma, mut chroma) = self.inter_pred(&self.refp, mbx, mby, skip);
        let mut code = self.inter_luma(mbx, mby, &luma, Kind::P16);
        if code.cbp == 0 && self.chroma_is_empty(mbx, mby, &chroma) {
            code.kind = Kind::PSkip;
            self.write_pred(0, mbx * 16, mby * 16, 16, &luma);
            for c in 0..2 {
                self.write_pred(1 + c, mbx * 8, mby * 8, 8, &chroma[c]);
            }
            self.motion[i] = MbMotion { r: 0, mv: skip };
            return code;
        }
        let pred_mv = mvp(&self.motion, self.mb_w, mbx, mby);
        let mv = refine(&self.src[0], &self.refp[0], mbx * 16, mby * 16, search(&self.src[0], &self.refp[0], mbx * 16, mby * 16, pred_mv, lambda(self.qp)), pred_mv, lambda(self.qp), self.cfg.subpel.min(2));
        if mv != skip {
            (luma, chroma) = self.inter_pred(&self.refp, mbx, mby, mv);
            code = self.inter_luma(mbx, mby, &luma, Kind::P16);
        }
        code.mvd = [mv.x as i32 - pred_mv.x as i32, mv.y as i32 - pred_mv.y as i32];
        self.inter_recon(mbx, mby, &luma, &chroma, &mut code);
        self.motion[i] = MbMotion { r: 0, mv };
        code
    }

    /// Reconstruct a coded inter macroblock as the decoder does: the prediction, then each coded 4x4
    /// block (LevelScale list 3, inter Y), then the chroma (lists 4 / 5).
    fn inter_recon(&mut self, mbx: usize, mby: usize, luma: &[i32; 256], chroma: &[[i32; 64]; 2], code: &mut MbCode) {
        let (px, py, stride) = (mbx * 16, mby * 16, self.rec[0].w);
        self.write_pred(0, px, py, 16, luma);
        let q6 = (self.qp % 6) as usize;
        for blk in 0..16 {
            let off = (py + BLK_Y[blk] * 4) * stride + px + BLK_X[blk] * 4;
            recon4(&code.luma[blk], &self.ls.l4[3][q6], self.qp, &mut self.rec[0].data, off, stride);
        }
        self.chroma_mb(mbx, mby, chroma, false, code);
    }

    /// The prediction of a B macroblock's motion `m` (SR333): one list's samples, or with both lists the
    /// decoder's default weighted average `(a + b + 1) >> 1` (h264_core `predict`, weighted_bipred_idc 0).
    fn bi_pred(&self, mbx: usize, mby: usize, m: [MbMotion; 2]) -> ([i32; 256], [[i32; 64]; 2]) {
        match (m[0].r >= 0, m[1].r >= 0) {
            (true, false) => self.inter_pred(&self.refp, mbx, mby, m[0].mv),
            (false, true) => self.inter_pred(&self.fwd, mbx, mby, m[1].mv),
            _ => {
                let (mut l, mut c) = self.inter_pred(&self.refp, mbx, mby, m[0].mv);
                let (l1, c1) = self.inter_pred(&self.fwd, mbx, mby, m[1].mv);
                for k in 0..256 {
                    l[k] = (l[k] + l1[k] + 1) >> 1;
                }
                for p in 0..2 {
                    for k in 0..64 {
                        c[p][k] = (c[p][k] + c1[p][k] + 1) >> 1;
                    }
                }
                (l, c)
            }
        }
    }

    /// Decide, code and reconstruct the B macroblock (mbx, mby) (SR333): B_Skip when the spatial-direct
    /// prediction quantises to nothing, else the cheapest of B_Direct_16x16 / B_L0_16x16 / B_L1_16x16 /
    /// B_Bi_16x16 by luma SAD + lambda x (mb_type bits + mvd bits), ties in that order.
    pub(crate) fn b_mb(&mut self, mbx: usize, mby: usize) -> MbCode {
        let i = mby * self.mb_w + mbx;
        let col = self.col[i];
        let col_zero = col.r == 0 && (-1..=1).contains(&col.mv.x) && (-1..=1).contains(&col.mv.y);
        let direct = direct_spatial(&self.motion, &self.motion1, self.mb_w, mbx, mby, col_zero);
        let (luma, chroma) = self.bi_pred(mbx, mby, direct);
        let code = self.inter_luma(mbx, mby, &luma, Kind::BSkip);
        if code.cbp == 0 && self.chroma_is_empty(mbx, mby, &chroma) {
            self.write_pred(0, mbx * 16, mby * 16, 16, &luma);
            for c in 0..2 {
                self.write_pred(1 + c, mbx * 8, mby * 8, 8, &chroma[c]);
            }
            [self.motion[i], self.motion1[i]] = direct;
            return code;
        }
        let lam = lambda(self.qp);
        let (px, py, depth) = (mbx * 16, mby * 16, self.cfg.subpel.min(2));
        let p0 = mvp(&self.motion, self.mb_w, mbx, mby);
        let mv0 = refine(&self.src[0], &self.refp[0], px, py, search(&self.src[0], &self.refp[0], px, py, p0, lam), p0, lam, depth);
        let p1 = mvp(&self.motion1, self.mb_w, mbx, mby);
        let mv1 = refine(&self.src[0], &self.fwd[0], px, py, search(&self.src[0], &self.fwd[0], px, py, p1, lam), p1, lam, depth);
        let mvd0 = [mv0.x as i32 - p0.x as i32, mv0.y as i32 - p0.y as i32];
        let mvd1 = [mv1.x as i32 - p1.x as i32, mv1.y as i32 - p1.y as i32];
        let bits = |d: [i32; 2]| se_bits(d[0]) + se_bits(d[1]);
        let m0 = MbMotion { r: 0, mv: mv0 };
        let m1 = MbMotion { r: 0, mv: mv1 };
        let cands = [
            (Kind::BDirect, direct, ue_bits(0)),
            (Kind::BL0, [m0, MbMotion::NONE], ue_bits(1) + bits(mvd0)),
            (Kind::BL1, [MbMotion::NONE, m1], ue_bits(2) + bits(mvd1)),
            (Kind::BBi, [m0, m1], ue_bits(3) + bits(mvd0) + bits(mvd1)),
        ];
        let mut best: Option<(u32, Kind, [MbMotion; 2], [i32; 256], [[i32; 64]; 2])> = None;
        for (kind, m, b) in cands {
            let (l, c) = if kind == Kind::BDirect { (luma, chroma) } else { self.bi_pred(mbx, mby, m) };
            let cost = self.sad(0, px, py, 16, &l) + lam * b;
            if best.as_ref().is_none_or(|x| cost < x.0) {
                best = Some((cost, kind, m, l, c));
            }
        }
        let (_, kind, m, l, c) = best.expect("four candidates");
        let mut code = self.inter_luma(mbx, mby, &l, kind);
        if kind.l0() {
            code.mvd = mvd0;
        }
        if kind.l1() {
            code.mvd1 = mvd1;
        }
        self.inter_recon(mbx, mby, &l, &c, &mut code);
        [self.motion[i], self.motion1[i]] = m;
        code
    }
}
