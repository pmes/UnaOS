// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Inter macroblocks: motion vector prediction (8.4.1.3) and the P_Skip motion (8.4.1.1) over
//! whole-macroblock partitions, an integer-sample full search in a +-[`SEARCH`] window over the
//! encoder's own reconstruction of the previous frame, and the P_Skip / P_L0_16x16 decision.
//! Prediction samples come from h264_core's `inter::luma_mc` / `chroma_mc` (the chroma vector is
//! the luma vector in eighth chroma samples, as the decoder's `predict` passes it).
//!
//! The neighbour rule mirrors h264_core's `inter.rs` `nb_mv` / `nb_abc` / `mvp` / `p_skip_motion`
//! for one slice per picture with only 16x16 partitions: A is the macroblock to the left, B the
//! one above, C the one above-right or, when that is outside the picture, D the one above-left;
//! a neighbour inside the picture is available (every one is earlier in decoding order).

use h264_core::inter::{chroma_mc, luma_mc};
use h264_core::picture::{Mv, Plane, BLK_X, BLK_Y};

use crate::nal::se_bits;
use crate::transform::{code_chroma, fdct16, quant4, recon4};
use crate::{lambda, Encoder, Kind, MbCode};

/// The integer search range in samples, each direction.
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

/// mvpL0 (8.4.1.3) of a 16x16 partition with refIdxL0 0 at macroblock (mbx, mby).
pub fn mvp(motion: &[MbMotion], mb_w: usize, mbx: usize, mby: usize) -> Mv {
    let (x, y) = (mbx as isize, mby as isize);
    let a = nb(motion, mb_w, x - 1, y);
    let b = nb(motion, mb_w, x, y - 1);
    let mut c = nb(motion, mb_w, x + 1, y - 1);
    if !c.avail {
        c = nb(motion, mb_w, x - 1, y - 1);
    }
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

impl Encoder {
    /// The motion-compensated prediction of macroblock (mbx, mby) for `mv` from the reference:
    /// luma (16x16) and the two chroma blocks (8x8).
    fn inter_pred(&self, mbx: usize, mby: usize, mv: Mv) -> ([i32; 256], [[i32; 64]; 2]) {
        let (px, py) = ((mbx * 16) as i32, (mby * 16) as i32);
        let mut buf = [0u8; 256];
        luma_mc(&self.refp[0], px, py, mv, 16, 16, &mut buf);
        let mut luma = [0i32; 256];
        for k in 0..256 {
            luma[k] = buf[k] as i32;
        }
        let mut chroma = [[0i32; 64]; 2];
        for c in 0..2 {
            chroma_mc(&self.refp[1 + c], px / 2, py / 2, mv, 8, 8, &mut buf);
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
    fn inter_luma(&self, mbx: usize, mby: usize, luma: &[i32; 256]) -> MbCode {
        let mut res = [0i32; 256];
        self.residual(0, mbx * 16, mby * 16, 16, luma, &mut res);
        let mut code = MbCode::new(Kind::P16);
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
        let (mut luma, mut chroma) = self.inter_pred(mbx, mby, skip);
        let mut code = self.inter_luma(mbx, mby, &luma);
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
        let mv = search(&self.src[0], &self.refp[0], mbx * 16, mby * 16, pred_mv, lambda(self.qp));
        if mv != skip {
            (luma, chroma) = self.inter_pred(mbx, mby, mv);
            code = self.inter_luma(mbx, mby, &luma);
        }
        code.mvd = [mv.x as i32 - pred_mv.x as i32, mv.y as i32 - pred_mv.y as i32];
        // Reconstruct as the decoder does: the prediction, then each coded 4x4 block (LevelScale
        // list 3, inter Y), then the chroma (lists 4 / 5).
        let (px, py, stride) = (mbx * 16, mby * 16, self.rec[0].w);
        self.write_pred(0, px, py, 16, &luma);
        let q6 = (self.qp % 6) as usize;
        for blk in 0..16 {
            let off = (py + BLK_Y[blk] * 4) * stride + px + BLK_X[blk] * 4;
            recon4(&code.luma[blk], &self.ls.l4[3][q6], self.qp, &mut self.rec[0].data, off, stride);
        }
        self.chroma_mb(mbx, mby, &chroma, false, &mut code);
        self.motion[i] = MbMotion { r: 0, mv };
        code
    }
}
