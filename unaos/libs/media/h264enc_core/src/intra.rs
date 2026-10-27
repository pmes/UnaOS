// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Intra macroblocks (8.3): the neighbouring samples gathered from the encoder's reconstruction
//! with the decoder's availability rule, prediction through h264_core's `intra::pred_nxn` /
//! `pred_16x16` / `pred_chroma`, the SAD-cheapest mode of each block, Intra_4x4 against
//! Intra_16x16 by SAD with the [`crate::I4_BIAS`] penalty, and Intra4x4PredMode prediction
//! (8.3.1.1) for the mode syntax.
//!
//! Availability mirrors h264_core's decoder (`decoder.rs` `sample_available` over `mb.rs` `nb`)
//! for one slice per picture without constrained intra prediction: a sample in the current
//! macroblock is available when its 4x4 block precedes the predicted block in luma4x4BlkIdx order;
//! one to the right of the current macroblock or below it never is; one in the macroblock left,
//! above, above-left or above-right is when that macroblock is inside the picture.

use h264_core::intra::{filter_8x8_edges, pred_16x16, pred_chroma, pred_nxn, Edges};
use h264_core::picture::{blk_at, Plane, BLK_X, BLK_Y};

use crate::transform::{fdct16, fdct4, fdct8, hadamard4, quant4, quant8, quant_dc, recon4, recon8, recon_i16, satd4, satd8};
use crate::{lambda, Encoder, Kind, MbCode, I4_BIAS, I8_BIAS};

/// Is the luma sample at (x, y), relative to macroblock (mbx, mby), available for predicting the
/// block whose luma4x4BlkIdx is `limit` (16 for a whole-macroblock prediction)?
pub fn available(mbx: usize, mby: usize, mb_w: usize, x: isize, y: isize, limit: usize) -> bool {
    let (bx, by) = (x.div_euclid(4), y.div_euclid(4));
    let dx = if bx < 0 { -1 } else if bx >= 4 { 1 } else { 0 };
    let dy = if by < 0 { -1 } else if by >= 4 { 1 } else { 0 };
    if dy > 0 || (dy == 0 && dx > 0) {
        return false;
    }
    if dx == 0 && dy == 0 {
        return blk_at(bx as usize, by as usize) < limit;
    }
    let (nx, ny) = (mbx as isize + dx, mby as isize + dy);
    nx >= 0 && ny >= 0 && nx < mb_w as isize
}

/// The edges of the n x n luma block at (bx, by) in macroblock (mbx, mby), with `tr` top-right
/// samples (n for Intra_4x4, 0 for Intra_16x16) substituted from p[n-1, -1] when not available.
pub fn luma_edges(pl: &Plane, mbx: usize, mby: usize, mb_w: usize, bx: usize, by: usize, n: usize, tr: usize, limit: usize) -> Edges {
    let (px, py) = (mbx * 16 + bx, mby * 16 + by);
    let (bxi, byi) = (bx as isize, by as isize);
    let mut e = Edges::new();
    e.has_top = available(mbx, mby, mb_w, bxi, byi - 1, limit);
    e.has_left = available(mbx, mby, mb_w, bxi - 1, byi, limit);
    e.has_tl = available(mbx, mby, mb_w, bxi - 1, byi - 1, limit);
    if e.has_top {
        for x in 0..n {
            e.top[x] = pl.data[(py - 1) * pl.w + px + x] as i32;
        }
        if tr > 0 {
            if available(mbx, mby, mb_w, bxi + n as isize, byi - 1, limit) {
                for x in 0..tr {
                    e.top[n + x] = pl.data[(py - 1) * pl.w + px + n + x] as i32;
                }
            } else {
                for x in 0..tr {
                    e.top[n + x] = e.top[n - 1];
                }
            }
        }
    }
    if e.has_left {
        for y in 0..n {
            e.left[y] = pl.data[(py + y) * pl.w + px - 1] as i32;
        }
    }
    if e.has_tl {
        e.tl = pl.data[(py - 1) * pl.w + px - 1] as i32;
    }
    e
}

/// The edges of the 8x8 chroma block of macroblock (mbx, mby) in chroma plane `pl` (availability
/// is the luma macroblock's).
pub fn chroma_edges(pl: &Plane, mbx: usize, mby: usize, mb_w: usize) -> Edges {
    let (px, py) = (mbx * 8, mby * 8);
    let mut e = Edges::new();
    e.has_top = available(mbx, mby, mb_w, 0, -1, 16);
    e.has_left = available(mbx, mby, mb_w, -1, 0, 16);
    e.has_tl = available(mbx, mby, mb_w, -1, -1, 16);
    if e.has_top {
        for x in 0..8 {
            e.top[x] = pl.data[(py - 1) * pl.w + px + x] as i32;
        }
    }
    if e.has_left {
        for y in 0..8 {
            e.left[y] = pl.data[(py + y) * pl.w + px - 1] as i32;
        }
    }
    if e.has_tl {
        e.tl = pl.data[(py - 1) * pl.w + px - 1] as i32;
    }
    e
}

/// predIntra4x4PredMode (8.3.1.1) of the 4x4 block at picture 4x4 position (x4, y4): DC (2) when
/// the block left or above is outside the picture, else the smaller of their modes (`modes` holds
/// 2 for every block not coded Intra_4x4).
pub fn predicted_mode(modes: &[u8], w4: usize, x4: usize, y4: usize) -> u8 {
    if x4 == 0 || y4 == 0 {
        return 2;
    }
    modes[y4 * w4 + x4 - 1].min(modes[(y4 - 1) * w4 + x4])
}

/// The Intra_4x4 modes (Table 8-2) whose samples are all available.
fn allowed4(mode: u8, e: &Edges) -> bool {
    match mode {
        0 | 3 | 7 => e.has_top,
        1 | 8 => e.has_left,
        2 => true,
        _ => e.has_top && e.has_left && e.has_tl,
    }
}

/// The Intra_16x16 modes (Table 8-4) whose samples are all available.
fn allowed16(mode: u8, e: &Edges) -> bool {
    match mode {
        0 => e.has_top,
        1 => e.has_left,
        2 => true,
        _ => e.has_top && e.has_left && e.has_tl,
    }
}

/// The chroma modes (Table 8-5: 0 DC, 1 Horizontal, 2 Vertical, 3 Plane) whose samples are all
/// available.
fn allowed_chroma(mode: u8, e: &Edges) -> bool {
    match mode {
        0 => true,
        1 => e.has_left,
        2 => e.has_top,
        _ => e.has_top && e.has_left && e.has_tl,
    }
}

impl Encoder {
    /// Decide, code and reconstruct the intra macroblock (mbx, mby).
    pub(crate) fn intra_mb(&mut self, mbx: usize, mby: usize) -> MbCode {
        let qp = self.qp;
        let q6 = (qp % 6) as usize;
        let (mb_w, w4) = (self.mb_w, self.mb_w * 4);
        let stride = self.rec[0].w;
        let (px, py) = (mbx * 16, mby * 16);

        // Intra_16x16: its edges lie outside the macroblock, so they are read before the 4x4
        // candidate writes its reconstruction inside it.
        let e16 = luma_edges(&self.rec[0], mbx, mby, mb_w, 0, 0, 16, 0, 16);
        let mut p16 = [0i32; 256];
        let mut best16 = (u32::MAX, 2u8);
        for mode in 0..4u8 {
            if allowed16(mode, &e16) {
                pred_16x16(mode, &e16, &mut p16);
                let s = self.sad(0, px, py, 16, &p16);
                if s < best16.0 {
                    best16 = (s, mode);
                }
            }
        }

        // Intra_4x4, block by block: each block is predicted from the reconstruction of the ones
        // before it, as the decoder does.
        let mut code = MbCode::new(Kind::I4);
        let mut sad4 = 0u32;
        let t8 = self.cfg.transform8x8;
        let mut satd_4 = 0u32; // the I4 residual's SATD, for the 8x8 choice (SR322)
        let mut pred = [0i32; 64];
        let mut bestp = [0i32; 16];
        let mut res = [0i32; 16];
        for blk in 0..16 {
            let (bx, by) = (BLK_X[blk] * 4, BLK_Y[blk] * 4);
            let e = luma_edges(&self.rec[0], mbx, mby, mb_w, bx, by, 4, 4, blk);
            let mut best = (u32::MAX, 2u8);
            for mode in 0..9u8 {
                if allowed4(mode, &e) {
                    pred_nxn(4, mode, &e, &mut pred);
                    let s = self.sad(0, px + bx, py + by, 4, &pred[..16]);
                    if s < best.0 {
                        best = (s, mode);
                        bestp.copy_from_slice(&pred[..16]);
                    }
                }
            }
            sad4 += best.0;
            code.modes[blk] = best.1;
            self.modes[(mby * 4 + BLK_Y[blk]) * w4 + mbx * 4 + BLK_X[blk]] = best.1;
            self.residual(0, px + bx, py + by, 4, &bestp, &mut res);
            if t8 {
                satd_4 += satd4(&res);
            }
            let mut c = fdct4(&res);
            quant4(&mut c, qp, true, false);
            self.write_pred(0, px + bx, py + by, 4, &bestp);
            recon4(&c, &self.ls.l4[0][q6], qp, &mut self.rec[0].data, (py + by) * stride + px + bx, stride);
            code.luma[blk] = c;
        }

        // SR322: Intra_8x8 replaces the Intra_4x4 candidate when its SATD + bias is lower; the winner
        // then meets Intra_16x16 by the SAD rule (with the knob off its operands are SR197's).
        let mut nxn = sad4.saturating_add(I4_BIAS * lambda(qp));
        if t8 {
            if let Some((c8, sad8)) = self.intra8_candidate(mbx, mby, &code, satd_4) {
                code = c8;
                nxn = sad8.saturating_add(I8_BIAS * lambda(qp));
            }
        }
        if best16.0 <= nxn {
            for blk in 0..16 {
                self.modes[(mby * 4 + BLK_Y[blk]) * w4 + mbx * 4 + BLK_X[blk]] = 2;
            }
            code = MbCode::new(Kind::I16);
            code.i16_mode = best16.1;
            pred_16x16(best16.1, &e16, &mut p16);
            let mut r = [0i32; 256];
            self.residual(0, px, py, 16, &p16, &mut r);
            let mut blocks = fdct16(&r);
            let mut w = [0i32; 16];
            for blk in 0..16 {
                w[BLK_Y[blk] * 4 + BLK_X[blk]] = blocks[blk][0];
            }
            let mut dc = hadamard4(&w);
            quant_dc(&mut dc, qp, true);
            let mut any_ac = false;
            for b in blocks.iter_mut() {
                quant4(b, qp, true, true);
                any_ac |= b.iter().any(|&v| v != 0);
            }
            code.dc = dc;
            code.luma = blocks;
            code.cbp = if any_ac { 15 } else { 0 };
            self.write_pred(0, px, py, 16, &p16);
            recon_i16(&code.dc, &code.luma, &self.ls, qp, &mut self.rec[0].data, py * stride + px, stride);
        } else {
            code.cbp = code.luma_cbp();
        }

        // Chroma: one mode for both components, the cheapest by SAD over Cb + Cr.
        let e = [chroma_edges(&self.rec[1], mbx, mby, mb_w), chroma_edges(&self.rec[2], mbx, mby, mb_w)];
        let mut preds = [[0i32; 64]; 2];
        let mut best = (u32::MAX, 0u8);
        for mode in 0..4u8 {
            if allowed_chroma(mode, &e[0]) {
                let mut s = 0;
                for c in 0..2 {
                    pred_chroma(mode, &e[c], &mut preds[c]);
                    s += self.sad(1 + c, mbx * 8, mby * 8, 8, &preds[c]);
                }
                if s < best.0 {
                    best = (s, mode);
                }
            }
        }
        code.chroma_mode = best.1;
        for c in 0..2 {
            pred_chroma(best.1, &e[c], &mut preds[c]);
        }
        self.chroma_mb(mbx, mby, &preds, true, &mut code);
        code
    }

    /// H264ENC8X8 (SR322): the Intra_8x8 candidate of macroblock (mbx, mby), coded over the Intra_4x4
    /// candidate already reconstructed there (`code4`, its residual's SATD `satd_4`). Each 8x8 block:
    /// the encoder's edges (n 8, its 8 top-right samples substituted as the decoder substitutes them),
    /// h264_core's `filter_8x8_edges` (8.3.2.2.1), the SAD-cheapest of the 9 modes through
    /// `pred_nxn(8, ..)`, the 8x8 transform and quantiser, `recon8` with LevelScale8x8 list 0. Kept
    /// when SATD + [`I8_BIAS`] x lambda is strictly below the 4x4's SATD + [`I4_BIAS`] x lambda:
    /// returns the code and its SAD; else the 4x4 reconstruction and modes are restored.
    fn intra8_candidate(&mut self, mbx: usize, mby: usize, code4: &MbCode, satd_4: u32) -> Option<(MbCode, u32)> {
        let qp = self.qp;
        let q6 = (qp % 6) as usize;
        let (mb_w, w4, stride) = (self.mb_w, self.mb_w * 4, self.rec[0].w);
        let (px, py) = (mbx * 16, mby * 16);
        let mut saved = [0u8; 256];
        for y in 0..16 {
            saved[y * 16..y * 16 + 16].copy_from_slice(&self.rec[0].data[(py + y) * stride + px..(py + y) * stride + px + 16]);
        }
        let mut code = MbCode::new(Kind::I4);
        code.t8x8 = true;
        let (mut sad8, mut satd_8) = (0u32, 0u32);
        let mut pred = [0i32; 64];
        let mut bestp = [0i32; 64];
        let mut res = [0i32; 64];
        for b8 in 0..4 {
            let (bx, by) = ((b8 % 2) * 8, (b8 / 2) * 8);
            let e = luma_edges(&self.rec[0], mbx, mby, mb_w, bx, by, 8, 8, b8 * 4);
            let f = filter_8x8_edges(&e);
            let mut best = (u32::MAX, 2u8);
            for mode in 0..9u8 {
                if allowed4(mode, &e) {
                    pred_nxn(8, mode, &f, &mut pred);
                    let s = self.sad(0, px + bx, py + by, 8, &pred);
                    if s < best.0 {
                        best = (s, mode);
                        bestp = pred;
                    }
                }
            }
            sad8 += best.0;
            code.modes[b8] = best.1;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                self.modes[(mby * 4 + by / 4 + dy) * w4 + mbx * 4 + bx / 4 + dx] = best.1;
            }
            self.residual(0, px + bx, py + by, 8, &bestp, &mut res);
            satd_8 += satd8(&res);
            let mut c = fdct8(&res);
            quant8(&mut c, qp, true);
            self.write_pred(0, px + bx, py + by, 8, &bestp);
            recon8(&c, &self.ls.l8[0][q6], qp, &mut self.rec[0].data, (py + by) * stride + px + bx, stride);
            code.luma8[b8] = c;
        }
        let lam = lambda(qp);
        if satd_8.saturating_add(I8_BIAS * lam) < satd_4.saturating_add(I4_BIAS * lam) {
            return Some((code, sad8));
        }
        for y in 0..16 {
            self.rec[0].data[(py + y) * stride + px..(py + y) * stride + px + 16].copy_from_slice(&saved[y * 16..y * 16 + 16]);
        }
        for blk in 0..16 {
            self.modes[(mby * 4 + BLK_Y[blk]) * w4 + mbx * 4 + BLK_X[blk]] = code4.modes[blk];
        }
        None
    }
}
