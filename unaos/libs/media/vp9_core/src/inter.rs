// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Inter prediction (spec §8.5.2): motion vector scaling for references of a different size
//! (§8.5.2.3, 14-bit fixed-point scale factors, x_step_q4 = 16 * scale), block position into the
//! reference, the reference-edge emulation (samples outside the reference's crop are its nearest
//! edge sample), and the 8-tap sub-pixel convolution (§8.5.2.4: horizontal pass into an intermediate
//! buffer with Round2(sum, 7) and a clip, then the vertical pass) for the four kernels; compound
//! prediction averages the second reference into the first with Round2(a + b, 1).
//!
//! Positions follow the reference decoder's dec_build_inter_predictors: the integer start of a
//! scaled block is scale(x) while its sub-pixel phase comes from the scaled luma position, so a
//! chroma block can start one sample off its fractional position; the edge-emulation window is
//! computed the same way and the convolution reads the emulated window row-major, as there.

use crate::frame::{FrameBuf, Mv};
use crate::tables::SUBPEL_FILTERS;
use alloc::vec;
use alloc::vec::Vec;

pub const REF_SCALE_SHIFT: u32 = 14;
pub const REF_NO_SCALE: i32 = 1 << REF_SCALE_SHIFT;

/// Scale factors of one reference against the current frame (§8.5.2.3).
#[derive(Clone, Copy, Debug)]
pub struct ScaleFactors {
    pub x_scale_fp: i32,
    pub y_scale_fp: i32,
    pub x_step_q4: i32,
    pub y_step_q4: i32,
}

impl ScaleFactors {
    /// None when the reference is more than 2x larger or 16x smaller than the frame.
    pub fn new(ref_w: u32, ref_h: u32, w: u32, h: u32) -> Option<ScaleFactors> {
        if 2 * w < ref_w || 2 * h < ref_h || w > 16 * ref_w || h > 16 * ref_h {
            return None;
        }
        let x_scale_fp = ((ref_w as i64) << REF_SCALE_SHIFT) / w as i64;
        let y_scale_fp = ((ref_h as i64) << REF_SCALE_SHIFT) / h as i64;
        let mut s = ScaleFactors { x_scale_fp: x_scale_fp as i32, y_scale_fp: y_scale_fp as i32, x_step_q4: 16, y_step_q4: 16 };
        s.x_step_q4 = s.sx(16);
        s.y_step_q4 = s.sy(16);
        Some(s)
    }
    pub fn is_scaled(&self) -> bool {
        self.x_scale_fp != REF_NO_SCALE || self.y_scale_fp != REF_NO_SCALE
    }
    #[inline]
    fn sx(&self, v: i32) -> i32 {
        ((v as i64 * self.x_scale_fp as i64) >> REF_SCALE_SHIFT) as i32
    }
    #[inline]
    fn sy(&self, v: i32) -> i32 {
        ((v as i64 * self.y_scale_fp as i64) >> REF_SCALE_SHIFT) as i32
    }
}

/// The block being predicted: its place in the mode-info grid and its distance to the frame edges
/// (the reference decoder's mb_to_*_edge, in 1/8 luma samples).
#[derive(Clone, Copy, Debug)]
pub struct BlockPos {
    pub mi_row: usize,
    pub mi_col: usize,
    pub to_left: i32,
    pub to_right: i32,
    pub to_top: i32,
    pub to_bottom: i32,
}

/// One plane's prediction request.
pub struct PredArgs<'a> {
    pub plane: usize,
    pub ss_x: u32,
    pub ss_y: u32,
    /// Block size in plane samples (the bw / bh of the containing prediction block).
    pub bw: i32,
    pub bh: i32,
    /// Offset of this sub-block inside the block, and its size.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub mv: Mv,
    pub filter: u8,
    pub sf: &'a ScaleFactors,
    pub reference: &'a FrameBuf,
    /// Second reference of a compound block: average into what the first wrote.
    pub avg: bool,
    pub bit_depth: u8,
}

/// Predict one sub-block into `dst` (the current frame's plane, at the block's position + x/y).
pub fn predict(dst: &mut [u16], dst_stride: usize, pos: &BlockPos, a: &PredArgs) {
    let (ssx, ssy) = (a.ss_x, a.ss_y);
    let reference = a.reference;
    let (fw, fh) = reference.plane_size(a.plane);
    let (fw, fh) = (fw as i32, fh as i32);
    let rplane = &reference.planes[a.plane];
    let sf = a.sf;
    let scaled = sf.is_scaled();
    let mi_x = (pos.mi_col * 8) as i32;
    let mi_y = (pos.mi_row * 8) as i32;

    let (mut x0, mut y0, mut x0_16, mut y0_16, smv_row, smv_col, xs, ys);
    if scaled {
        // clamp_mv_to_umv_border_sb
        let spel_left = (4 + a.bw) << 4;
        let spel_right = spel_left - 16;
        let spel_top = (4 + a.bh) << 4;
        let spel_bottom = spel_top - 16;
        let mut row = a.mv.row as i32 * (1 << (1 - ssy));
        let mut col = a.mv.col as i32 * (1 << (1 - ssx));
        col = col.clamp(pos.to_left * (1 << (1 - ssx)) - spel_left, pos.to_right * (1 << (1 - ssx)) + spel_right);
        row = row.clamp(pos.to_top * (1 << (1 - ssy)) - spel_top, pos.to_bottom * (1 << (1 - ssy)) + spel_bottom);
        let x_start = (-pos.to_left) >> (3 + ssx);
        let y_start = (-pos.to_top) >> (3 + ssy);
        x0_16 = sf.sx((x_start + a.x) << 4);
        y0_16 = sf.sy((y_start + a.y) << 4);
        x0 = sf.sx(x_start + a.x);
        y0 = sf.sy(y_start + a.y);
        // vp9_scale_mv: the sub-pixel offset comes from the LUMA position mi_x + x.
        let x_off_q4 = sf.sx((mi_x + a.x) << 4) & 15;
        let y_off_q4 = sf.sy((mi_y + a.y) << 4) & 15;
        smv_row = sf.sy(row) + y_off_q4;
        smv_col = sf.sx(col) + x_off_q4;
        xs = sf.x_step_q4;
        ys = sf.y_step_q4;
    } else {
        x0 = ((-pos.to_left) >> (3 + ssx)) + a.x;
        y0 = ((-pos.to_top) >> (3 + ssy)) + a.y;
        x0_16 = x0 << 4;
        y0_16 = y0 << 4;
        smv_row = a.mv.row as i32 * (1 << (1 - ssy));
        smv_col = a.mv.col as i32 * (1 << (1 - ssx));
        xs = 16;
        ys = 16;
    }
    let subpel_x = smv_col & 15;
    let subpel_y = smv_row & 15;
    x0 += smv_col >> 4;
    y0 += smv_row >> 4;
    x0_16 += smv_col;
    y0_16 += smv_row;

    let kernel = &SUBPEL_FILTERS[a.filter.min(3) as usize];
    let dst_off = (((pos.mi_row * 8) >> ssy) as i32 + a.y) as usize * dst_stride + (((pos.mi_col * 8) >> ssx) as i32 + a.x) as usize;

    if scaled || smv_col != 0 || smv_row != 0 || fw & 7 != 0 || fh & 7 != 0 {
        let mut y1 = ((y0_16 + (a.h - 1) * ys) >> 4) + 1;
        let mut x1 = ((x0_16 + (a.w - 1) * xs) >> 4) + 1;
        let (mut x_pad, mut y_pad) = (0, 0);
        let (mut ex0, mut ey0) = (x0, y0);
        if subpel_x != 0 || sf.x_step_q4 != 16 {
            ex0 -= 3;
            x1 += 4;
            x_pad = 1;
        }
        if subpel_y != 0 || sf.y_step_q4 != 16 {
            ey0 -= 3;
            y1 += 4;
            y_pad = 1;
        }
        if ex0 < 0 || ex0 > fw - 1 || x1 < 0 || x1 > fw - 1 || ey0 < 0 || ey0 > fh - 1 || y1 < 0 || y1 > fh - 1 {
            // build_mc_border: the window [ex0, x1] x [ey0, y1] with every coordinate clamped
            // into the reference's crop.
            let b_w = (x1 - ex0 + 1) as usize;
            let b_h = (y1 - ey0 + 1) as usize;
            // Slack past the end: the convolution may read a row's worth beyond the window.
            let mut mc: Vec<u16> = vec![0; b_w * (b_h + 1) + 16];
            for r in 0..b_h {
                let sy = (ey0 + r as i32).clamp(0, fh - 1) as usize;
                let row = &rplane.data[sy * rplane.stride..];
                for c in 0..b_w {
                    let sx = (ex0 + c as i32).clamp(0, fw - 1) as usize;
                    mc[r * b_w + c] = row[sx];
                }
            }
            let origin = (y_pad * 3 * b_w + x_pad * 3) as isize;
            convolve(&mc, b_w, origin, dst, dst_off, dst_stride, kernel, subpel_x, xs, subpel_y, ys, a.w as usize, a.h as usize, a.bit_depth, a.avg);
            return;
        }
    }
    let origin = y0 as isize * rplane.stride as isize + x0 as isize;
    convolve(&rplane.data, rplane.stride, origin, dst, dst_off, dst_stride, kernel, subpel_x, xs, subpel_y, ys, a.w as usize, a.h as usize, a.bit_depth, a.avg);
}

/// The 2-D 8-tap convolution (vpx_convolve8 / vpx_scaled_2d): every selection the reference makes
/// (copy, horizontal-only, vertical-only, 2-D) is this with the phase-0 kernel {0,0,0,128,0,0,0,0}
/// in the unused direction, which is exact.
fn convolve(src: &[u16], stride: usize, origin: isize, dst: &mut [u16], dst_off: usize, dst_stride: usize, kernel: &[[i16; 8]; 16], x0_q4: i32, xs: i32, y0_q4: i32, ys: i32, w: usize, h: usize, bit_depth: u8, avg: bool) {
    let max = (1i32 << bit_depth) - 1;
    let ih = ((((h as i32 - 1) * ys + y0_q4) >> 4) + 8) as usize;
    let mut temp: Vec<i32> = vec![0; ih * w];
    let s = stride as isize;
    let px = |i: isize| -> i32 { if i < 0 { 0 } else { src.get(i as usize).copied().unwrap_or(0) as i32 } };
    for r in 0..ih {
        let row = origin + (r as isize - 3) * s;
        let mut x_q4 = x0_q4;
        for c in 0..w {
            let f = &kernel[(x_q4 & 15) as usize];
            let base = row + (x_q4 >> 4) as isize - 3;
            let mut sum = 0i32;
            for k in 0..8 {
                if f[k] != 0 {
                    sum += px(base + k as isize) * f[k] as i32;
                }
            }
            temp[r * w + c] = ((sum + 64) >> 7).clamp(0, max);
            x_q4 += xs;
        }
    }
    for c in 0..w {
        let mut y_q4 = y0_q4;
        for r in 0..h {
            let f = &kernel[(y_q4 & 15) as usize];
            let base = (y_q4 >> 4) as usize;
            let mut sum = 0i32;
            for k in 0..8 {
                sum += temp[(base + k) * w + c] * f[k] as i32;
            }
            let v = ((sum + 64) >> 7).clamp(0, max);
            let d = &mut dst[dst_off + r * dst_stride + c];
            *d = if avg { ((*d as i32 + v + 1) >> 1) as u16 } else { v as u16 };
            y_q4 += ys;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_factors() {
        let s = ScaleFactors::new(64, 64, 64, 64).unwrap();
        assert!(!s.is_scaled());
        assert_eq!(s.x_step_q4, 16);
        let s = ScaleFactors::new(128, 64, 64, 64).unwrap();
        assert!(s.is_scaled());
        assert_eq!(s.x_step_q4, 32);
        assert!(ScaleFactors::new(129, 64, 64, 64).is_none());
        assert!(ScaleFactors::new(4, 4, 65, 64).is_none());
    }

    /// A whole-pixel MV copies the reference block exactly.
    #[test]
    fn integer_mv_copies() {
        let mut r = FrameBuf::new(64, 64, 1, 1, 8);
        for (i, v) in r.planes[0].data.iter_mut().enumerate() {
            *v = (i % 251) as u16;
        }
        let sf = ScaleFactors::new(64, 64, 64, 64).unwrap();
        let pos = BlockPos { mi_row: 1, mi_col: 1, to_left: -64, to_right: 64 * 8 - 8 * 8 - 64, to_top: -64, to_bottom: 64 * 8 - 8 * 8 - 64 };
        let a = PredArgs { plane: 0, ss_x: 0, ss_y: 0, bw: 8, bh: 8, x: 0, y: 0, w: 8, h: 8, mv: Mv::new(8 * 2, 8 * 3), filter: 0, sf: &sf, reference: &r, avg: false, bit_depth: 8 };
        let mut dst = vec![0u16; 64 * 64];
        predict(&mut dst, 64, &pos, &a);
        let s = r.planes[0].stride;
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(dst[(8 + y) * 64 + 8 + x], r.planes[0].data[(8 + y + 2) * s + 8 + x + 3]);
            }
        }
    }
}
