//! §7.11.3 inter prediction: rounding variables, motion vector scaling (reference frames of a
//! different size included), block inter prediction with the 8-tap / 4-tap sub-pixel filters
//! (regular, smooth, sharp, bilinear; dual filter), block warp with setup shear and the resolve
//! divisor process, local warp estimation (least squares), overlapped block motion compensation,
//! the wedge / difference-weight / inter-intra masks, mask blend and distance weights — and the
//! intra block copy path (§7.11.3.1 with refIdx = -1).

use crate::decode::{Dec, Plane};
use crate::mvpred::round2signed;
use crate::refs::Mv;
use crate::tables::*;
use alloc::vec;
use alloc::vec::Vec;

const INTRA: i32 = INTRA_FRAME as i32;

#[inline]
fn round2(x: i32, n: u32) -> i32 {
    if n == 0 { x } else { (x + (1 << (n - 1))) >> n }
}

/// Wedge_Codebook[ 3 ][ 16 ][ 3 ] (§7.11.3.11): direction, xoff, yoff.
const WEDGE_HORIZONTAL: u8 = 0;
const WEDGE_VERTICAL: u8 = 1;
const WEDGE_OBLIQUE27: u8 = 2;
const WEDGE_OBLIQUE63: u8 = 3;
const WEDGE_OBLIQUE117: u8 = 4;
const WEDGE_OBLIQUE153: u8 = 5;
static WEDGE_CODEBOOK: [[[u8; 3]; 16]; 3] = [
    [
        [WEDGE_OBLIQUE27, 4, 4], [WEDGE_OBLIQUE63, 4, 4], [WEDGE_OBLIQUE117, 4, 4], [WEDGE_OBLIQUE153, 4, 4],
        [WEDGE_HORIZONTAL, 4, 2], [WEDGE_HORIZONTAL, 4, 4], [WEDGE_HORIZONTAL, 4, 6], [WEDGE_VERTICAL, 4, 4],
        [WEDGE_OBLIQUE27, 4, 2], [WEDGE_OBLIQUE27, 4, 6], [WEDGE_OBLIQUE153, 4, 2], [WEDGE_OBLIQUE153, 4, 6],
        [WEDGE_OBLIQUE63, 2, 4], [WEDGE_OBLIQUE63, 6, 4], [WEDGE_OBLIQUE117, 2, 4], [WEDGE_OBLIQUE117, 6, 4],
    ],
    [
        [WEDGE_OBLIQUE27, 4, 4], [WEDGE_OBLIQUE63, 4, 4], [WEDGE_OBLIQUE117, 4, 4], [WEDGE_OBLIQUE153, 4, 4],
        [WEDGE_VERTICAL, 2, 4], [WEDGE_VERTICAL, 4, 4], [WEDGE_VERTICAL, 6, 4], [WEDGE_HORIZONTAL, 4, 4],
        [WEDGE_OBLIQUE27, 4, 2], [WEDGE_OBLIQUE27, 4, 6], [WEDGE_OBLIQUE153, 4, 2], [WEDGE_OBLIQUE153, 4, 6],
        [WEDGE_OBLIQUE63, 2, 4], [WEDGE_OBLIQUE63, 6, 4], [WEDGE_OBLIQUE117, 2, 4], [WEDGE_OBLIQUE117, 6, 4],
    ],
    [
        [WEDGE_OBLIQUE27, 4, 4], [WEDGE_OBLIQUE63, 4, 4], [WEDGE_OBLIQUE117, 4, 4], [WEDGE_OBLIQUE153, 4, 4],
        [WEDGE_HORIZONTAL, 4, 2], [WEDGE_HORIZONTAL, 4, 6], [WEDGE_VERTICAL, 2, 4], [WEDGE_VERTICAL, 6, 4],
        [WEDGE_OBLIQUE27, 4, 2], [WEDGE_OBLIQUE27, 4, 6], [WEDGE_OBLIQUE153, 4, 2], [WEDGE_OBLIQUE153, 4, 6],
        [WEDGE_OBLIQUE63, 2, 4], [WEDGE_OBLIQUE63, 6, 4], [WEDGE_OBLIQUE117, 2, 4], [WEDGE_OBLIQUE117, 6, 4],
    ],
];

/// MasterMask[ dir ][ i ][ j ] of initialise_wedge_mask_table (§7.11.3.11).
fn master_masks() -> Vec<[[u8; 64]; 64]> {
    let w = MASK_MASTER_SIZE;
    let h = MASK_MASTER_SIZE;
    let mut m = vec![[[0u8; 64]; 64]; 6];
    for j in 0..w {
        let mut shift = (MASK_MASTER_SIZE / 4) as isize;
        let mut i = 0;
        while i < h {
            m[WEDGE_OBLIQUE63 as usize][i][j] = WEDGE_MASTER_OBLIQUE_EVEN[(j as isize - shift).clamp(0, MASK_MASTER_SIZE as isize - 1) as usize];
            shift -= 1;
            m[WEDGE_OBLIQUE63 as usize][i + 1][j] = WEDGE_MASTER_OBLIQUE_ODD[(j as isize - shift).clamp(0, MASK_MASTER_SIZE as isize - 1) as usize];
            m[WEDGE_VERTICAL as usize][i][j] = WEDGE_MASTER_VERTICAL[j];
            m[WEDGE_VERTICAL as usize][i + 1][j] = WEDGE_MASTER_VERTICAL[j];
            i += 2;
        }
    }
    for i in 0..h {
        for j in 0..w {
            let msk = m[WEDGE_OBLIQUE63 as usize][i][j];
            m[WEDGE_OBLIQUE27 as usize][j][i] = msk;
            m[WEDGE_OBLIQUE117 as usize][i][w - 1 - j] = 64 - msk;
            m[WEDGE_OBLIQUE153 as usize][w - 1 - j][i] = 64 - msk;
            let v = m[WEDGE_VERTICAL as usize][i][j];
            m[WEDGE_HORIZONTAL as usize][j][i] = v;
        }
    }
    m
}

fn block_shape(bsize: usize) -> usize {
    let w4 = NUM_4X4_BLOCKS_WIDE[bsize];
    let h4 = NUM_4X4_BLOCKS_HIGH[bsize];
    if h4 > w4 {
        0
    } else if h4 < w4 {
        1
    } else {
        2
    }
}

/// WedgeMasks[ bsize ][ sign ][ wedge ] written into `out` (stride 128).
pub fn wedge_mask(bsize: usize, sign: usize, wedge: usize, out: &mut [u8]) {
    let m = master_masks();
    let w = 4 * NUM_4X4_BLOCKS_WIDE[bsize] as usize;
    let h = 4 * NUM_4X4_BLOCKS_HIGH[bsize] as usize;
    let cb = WEDGE_CODEBOOK[block_shape(bsize)][wedge];
    let dir = cb[0] as usize;
    let xoff = MASK_MASTER_SIZE / 2 - ((cb[1] as usize * w) >> 3);
    let yoff = MASK_MASTER_SIZE / 2 - ((cb[2] as usize * h) >> 3);
    let mut sum = 0usize;
    for i in 0..w {
        sum += m[dir][yoff][xoff + i] as usize;
    }
    for i in 1..h {
        sum += m[dir][yoff + i][xoff] as usize;
    }
    let avg = (sum + (w + h - 1) / 2) / (w + h - 1);
    let flip_sign = (avg < 32) as usize;
    for i in 0..h {
        for j in 0..w {
            let v = m[dir][yoff + i][xoff + j];
            out[i * 128 + j] = if sign == flip_sign { v } else { 64 - v };
        }
    }
}

/// The resolve divisor process (§7.11.3.7) -> (divShift, divFactor).
pub fn resolve_divisor(d: i64) -> (i32, i64) {
    let a = d.unsigned_abs();
    let n = 63 - a.leading_zeros() as i32;
    let e = a as i64 - (1i64 << n);
    let f = if n > DIV_LUT_BITS as i32 { (e + (1i64 << (n - DIV_LUT_BITS as i32 - 1))) >> (n - DIV_LUT_BITS as i32) } else { e << (DIV_LUT_BITS as i32 - n) };
    let div_shift = n + DIV_LUT_PREC_BITS as i32;
    let div_factor = if d < 0 { -(DIV_LUT[f as usize] as i64) } else { DIV_LUT[f as usize] as i64 };
    (div_shift, div_factor)
}

/// The setup shear process (§7.11.3.6) -> (warpValid, alpha, beta, gamma, delta).
pub fn setup_shear(wp: &[i32; 6]) -> (bool, i32, i32, i32, i32) {
    let alpha0 = (wp[2] - (1 << WARPEDMODEL_PREC_BITS)).clamp(-32768, 32767);
    let beta0 = wp[3].clamp(-32768, 32767);
    let (div_shift, div_factor) = resolve_divisor(wp[2] as i64);
    let v = (wp[4] as i64) << WARPEDMODEL_PREC_BITS;
    let gamma0 = round2signed_wide(v as i128 * div_factor as i128, div_shift as u32).clamp(-32768, 32767) as i32;
    let w = wp[3] as i64 * wp[4] as i64;
    let delta0 = (wp[5] as i64 - round2signed_wide(w as i128 * div_factor as i128, div_shift as u32) as i64 - (1i64 << WARPEDMODEL_PREC_BITS)).clamp(-32768, 32767) as i32;
    let rb = WARP_PARAM_REDUCE_BITS as u32;
    let alpha = (round2signed(alpha0 as i64, rb) << rb) as i32;
    let beta = (round2signed(beta0 as i64, rb) << rb) as i32;
    let gamma = (round2signed(gamma0 as i64, rb) << rb) as i32;
    let delta = (round2signed(delta0 as i64, rb) << rb) as i32;
    let mut valid = true;
    if 4 * alpha.abs() + 7 * beta.abs() >= (1 << WARPEDMODEL_PREC_BITS) {
        valid = false;
    }
    if 4 * gamma.abs() + 4 * delta.abs() >= (1 << WARPEDMODEL_PREC_BITS) {
        valid = false;
    }
    (valid, alpha, beta, gamma, delta)
}

fn round2signed_wide(x: i128, n: u32) -> i128 {
    if n == 0 {
        return x;
    }
    if x >= 0 {
        (x + (1i128 << (n - 1))) >> n
    } else {
        -((-x + (1i128 << (n - 1))) >> n)
    }
}

/// Reference plane geometry for prediction.
struct RefGeom<'p> {
    plane: &'p Plane,
    last_x: i32,
    last_y: i32,
}

impl<'p> RefGeom<'p> {
    #[inline]
    fn at(&self, x: i32, y: i32) -> i32 {
        self.plane.get(x.clamp(0, self.last_x) as usize, y.clamp(0, self.last_y) as usize) as i32
    }
}

/// The block inter prediction process (§7.11.3.4) into `pred` (w x h, stride w).
#[allow(clippy::too_many_arguments)]
fn block_inter_prediction(
    rg: &RefGeom,
    x: i32,
    y: i32,
    x_step: i32,
    y_step: i32,
    w: usize,
    h: usize,
    filters: [u8; 2],
    round0: u32,
    round1: u32,
    pred: &mut [i32],
) {
    let inter_h = ((((h as i32 - 1) * y_step + (1 << SCALE_SUBPEL_BITS) - 1) >> SCALE_SUBPEL_BITS) + 8) as usize;
    let mut f1 = filters[1] as usize;
    if w <= 4 {
        if f1 == EIGHTTAP || f1 == EIGHTTAP_SHARP {
            f1 = 4;
        } else if f1 == EIGHTTAP_SMOOTH {
            f1 = 5;
        }
    }
    let mut intermediate = vec![0i32; inter_h * w];
    for r in 0..inter_h {
        let yy = (y >> 10) + r as i32 - 3;
        for c in 0..w {
            let p = x + x_step * c as i32;
            let filt = &SUBPEL_FILTERS[f1][((p >> 6) & SUBPEL_MASK as i32) as usize];
            let mut s = 0i32;
            for t in 0..8 {
                s += filt[t] as i32 * rg.at((p >> 10) + t as i32 - 3, yy);
            }
            intermediate[r * w + c] = round2(s, round0);
        }
    }
    let mut f0 = filters[0] as usize;
    if h <= 4 {
        if f0 == EIGHTTAP || f0 == EIGHTTAP_SHARP {
            f0 = 4;
        } else if f0 == EIGHTTAP_SMOOTH {
            f0 = 5;
        }
    }
    for r in 0..h {
        let p = (y & 1023) + y_step * r as i32;
        let filt = &SUBPEL_FILTERS[f0][((p >> 6) & SUBPEL_MASK as i32) as usize];
        let base = (p >> 10) as usize;
        for c in 0..w {
            let mut s = 0i32;
            for t in 0..8 {
                s += filt[t] as i32 * intermediate[(base + t) * w + c];
            }
            pred[r * w + c] = round2(s, round1);
        }
    }
}

impl<'a, 'f> Dec<'a, 'f> {
    fn rounding_variables(&mut self, is_compound: bool) {
        self.inter_round0 = 3;
        self.inter_round1 = if is_compound { 7 } else { 11 };
        if self.fs.bit_depth == 12 {
            self.inter_round0 += 2;
        }
        if self.fs.bit_depth == 12 && !is_compound {
            self.inter_round1 -= 2;
        }
        self.inter_post_round = 2 * FILTER_BITS as u32 - (self.inter_round0 + self.inter_round1);
    }

    /// (RefUpscaledWidth, RefFrameHeight) of refIdx, with -1 meaning the current frame.
    fn ref_dims(&self, ref_idx: isize) -> (u32, u32) {
        if ref_idx < 0 {
            (self.fs.upscaled_width, self.fs.frame_height)
        } else {
            let f = self.refs.frames[ref_idx as usize].as_ref().expect("reference slot");
            (f.upscaled_width, f.frame_height)
        }
    }

    /// The motion vector scaling process (§7.11.3.3) -> (startX, startY, stepX, stepY).
    fn scale_mv(&self, plane: usize, ref_idx: isize, x: usize, y: usize, mv: Mv) -> (i32, i32, i32, i32) {
        let (ref_up_w, ref_h) = self.ref_dims(ref_idx);
        let fw = self.fs.frame_width as i64;
        let fh = self.fs.frame_height as i64;
        let x_scale = (((ref_up_w as i64) << REF_SCALE_SHIFT) + (fw / 2)) / fw;
        let y_scale = (((ref_h as i64) << REF_SCALE_SHIFT) + (fh / 2)) / fh;
        let (sub_x, sub_y) = if plane == 0 { (0, 0) } else { (self.fs.ss_x as u32, self.fs.ss_y as u32) };
        let half_sample = 1i64 << (SUBPEL_BITS - 1);
        let orig_x = ((x as i64) << SUBPEL_BITS) + ((2 * mv[1] as i64) >> sub_x) + half_sample;
        let orig_y = ((y as i64) << SUBPEL_BITS) + ((2 * mv[0] as i64) >> sub_y) + half_sample;
        let base_x = orig_x * x_scale - (half_sample << REF_SCALE_SHIFT);
        let base_y = orig_y * y_scale - (half_sample << REF_SCALE_SHIFT);
        let off = (1i64 << (SCALE_SUBPEL_BITS - SUBPEL_BITS)) / 2;
        let sh = (REF_SCALE_SHIFT + SUBPEL_BITS - SCALE_SUBPEL_BITS) as u32;
        let start_x = round2signed(base_x, sh) + off;
        let start_y = round2signed(base_y, sh) + off;
        let step_x = round2signed(x_scale, (REF_SCALE_SHIFT - SCALE_SUBPEL_BITS) as u32);
        let step_y = round2signed(y_scale, (REF_SCALE_SHIFT - SCALE_SUBPEL_BITS) as u32);
        (start_x as i32, start_y as i32, step_x as i32, step_y as i32)
    }

    fn ref_geom(&self, plane: usize, ref_idx: isize) -> RefGeom<'_> {
        let (sub_x, sub_y) = if plane == 0 { (0, 0) } else { (self.fs.ss_x as u32, self.fs.ss_y as u32) };
        if ref_idx < 0 {
            // intra block copy: (step 11) the whole decoded area, not cropped to the frame size
            let w = (self.fs.mi_cols * MI_SIZE) as u32;
            let h = (self.fs.mi_rows * MI_SIZE) as u32;
            RefGeom { plane: &self.fs.planes[plane], last_x: (((w + sub_x) >> sub_x) - 1) as i32, last_y: (((h + sub_y) >> sub_y) - 1) as i32 }
        } else {
            let f = self.refs.frames[ref_idx as usize].as_ref().expect("reference slot");
            RefGeom {
                plane: &f.planes[plane],
                last_x: (((f.upscaled_width + sub_x) >> sub_x) - 1) as i32,
                last_y: (((f.frame_height + sub_y) >> sub_y) - 1) as i32,
            }
        }
    }

    /// The warp estimation process (§7.11.3.8): LocalWarpParams and LocalValid.
    fn warp_estimation(&mut self) {
        let mut a = [[0i64; 2]; 2];
        let mut bx = [0i64; 2];
        let mut by = [0i64; 2];
        let w4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as i64;
        let h4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as i64;
        let mid_y = self.mi_row as i64 * 4 + h4 * 2 - 1;
        let mid_x = self.mi_col as i64 * 4 + w4 * 2 - 1;
        let suy = mid_y * 8;
        let sux = mid_x * 8;
        let duy = suy + self.mv[0][0] as i64;
        let dux = sux + self.mv[0][1] as i64;
        let ls_product = |a: i64, b: i64| ((a * b) >> 2) + (a + b);
        for i in 0..self.num_samples {
            let c = self.cand_list[i];
            let sy = c[0] as i64 - suy;
            let sx = c[1] as i64 - sux;
            let dy = c[2] as i64 - duy;
            let dx = c[3] as i64 - dux;
            if (sx - dx).abs() < LS_MV_MAX as i64 && (sy - dy).abs() < LS_MV_MAX as i64 {
                a[0][0] += ls_product(sx, sx) + 8;
                a[0][1] += ls_product(sx, sy) + 4;
                a[1][1] += ls_product(sy, sy) + 8;
                bx[0] += ls_product(sx, dx) + 8;
                bx[1] += ls_product(sy, dx) + 4;
                by[0] += ls_product(sx, dy) + 4;
                by[1] += ls_product(sy, dy) + 8;
            }
        }
        let det = a[0][0] * a[1][1] - a[0][1] * a[0][1];
        self.local_valid = det != 0;
        if det == 0 {
            return;
        }
        let (mut div_shift, mut div_factor) = resolve_divisor(det);
        div_shift -= WARPEDMODEL_PREC_BITS as i32;
        if div_shift < 0 {
            div_factor <<= -div_shift;
            div_shift = 0;
        }
        let nondiag = |v: i128| -> i32 {
            round2signed_wide(v * div_factor as i128, div_shift as u32)
                .clamp(-(WARPEDMODEL_NONDIAGAFFINE_CLAMP as i128) + 1, WARPEDMODEL_NONDIAGAFFINE_CLAMP as i128 - 1) as i32
        };
        let diag = |v: i128| -> i32 {
            round2signed_wide(v * div_factor as i128, div_shift as u32).clamp(
                (1i128 << WARPEDMODEL_PREC_BITS) - WARPEDMODEL_NONDIAGAFFINE_CLAMP as i128 + 1,
                (1i128 << WARPEDMODEL_PREC_BITS) + WARPEDMODEL_NONDIAGAFFINE_CLAMP as i128 - 1,
            ) as i32
        };
        let (a00, a01, a11) = (a[0][0] as i128, a[0][1] as i128, a[1][1] as i128);
        let mut p = [0i32; 6];
        p[2] = diag(a11 * bx[0] as i128 - a01 * bx[1] as i128);
        p[3] = nondiag(-a01 * bx[0] as i128 + a00 * bx[1] as i128);
        p[4] = nondiag(a11 * by[0] as i128 - a01 * by[1] as i128);
        p[5] = diag(-a01 * by[0] as i128 + a00 * by[1] as i128);
        let mvx = self.mv[0][1] as i64;
        let mvy = self.mv[0][0] as i64;
        let vx = mvx * (1 << (WARPEDMODEL_PREC_BITS - 3)) - (mid_x * (p[2] as i64 - (1 << WARPEDMODEL_PREC_BITS)) + mid_y * p[3] as i64);
        let vy = mvy * (1 << (WARPEDMODEL_PREC_BITS - 3)) - (mid_x * p[4] as i64 + mid_y * (p[5] as i64 - (1 << WARPEDMODEL_PREC_BITS)));
        let clamp = WARPEDMODEL_TRANS_CLAMP as i64;
        p[0] = vx.clamp(-clamp, clamp - 1) as i32;
        p[1] = vy.clamp(-clamp, clamp - 1) as i32;
        self.local_warp_params = p;
    }

    /// The block warp process (§7.11.3.5) for the 8x8 section (i8, j8) of `pred` (stride w).
    #[allow(clippy::too_many_arguments)]
    fn block_warp(&self, rg: &RefGeom, wp: &[i32; 6], plane: usize, x: usize, y: usize, i8: usize, j8: usize, w: usize, h: usize, pred: &mut [i32]) {
        let (sub_x, sub_y) = if plane == 0 { (0, 0) } else { (self.fs.ss_x as u32, self.fs.ss_y as u32) };
        let src_x = ((x + j8 * 8 + 4) << sub_x) as i64;
        let src_y = ((y + i8 * 8 + 4) << sub_y) as i64;
        let dst_x = wp[2] as i64 * src_x + wp[3] as i64 * src_y + wp[0] as i64;
        let dst_y = wp[4] as i64 * src_x + wp[5] as i64 * src_y + wp[1] as i64;
        let (_, alpha, beta, gamma, delta) = setup_shear(wp);
        let x4 = dst_x >> sub_x;
        let y4 = dst_y >> sub_y;
        let ix4 = (x4 >> WARPEDMODEL_PREC_BITS) as i32;
        let sx4 = (x4 & ((1 << WARPEDMODEL_PREC_BITS) - 1)) as i32;
        let iy4 = (y4 >> WARPEDMODEL_PREC_BITS) as i32;
        let sy4 = (y4 & ((1 << WARPEDMODEL_PREC_BITS) - 1)) as i32;
        let mut intermediate = [[0i32; 8]; 15];
        for i1 in -7i32..8 {
            for i2 in -4i32..4 {
                let sx = sx4 + alpha * i2 + beta * i1;
                let offs = (round2(sx, WARPEDDIFF_PREC_BITS as u32) + WARPEDPIXEL_PREC_SHIFTS as i32) as usize;
                let mut s = 0i32;
                for i3 in 0..8 {
                    s += WARPED_FILTERS[offs][i3] as i32 * rg.at(ix4 + i2 - 3 + i3 as i32, iy4 + i1);
                }
                intermediate[(i1 + 7) as usize][(i2 + 4) as usize] = round2(s, self.inter_round0);
            }
        }
        let lim_y = 4.min(h as i32 - i8 as i32 * 8 - 4);
        let lim_x = 4.min(w as i32 - j8 as i32 * 8 - 4);
        for i1 in -4i32..lim_y {
            for i2 in -4i32..lim_x {
                let sy = sy4 + gamma * i2 + delta * i1;
                let offs = (round2(sy, WARPEDDIFF_PREC_BITS as u32) + WARPEDPIXEL_PREC_SHIFTS as i32) as usize;
                let mut s = 0i32;
                for i3 in 0..8 {
                    s += WARPED_FILTERS[offs][i3] as i32 * intermediate[(i1 + i3 as i32 + 4) as usize][(i2 + 4) as usize];
                }
                let py = (i8 as i32 * 8 + i1 + 4) as usize;
                let px = (j8 as i32 * 8 + i2 + 4) as usize;
                pred[py * w + px] = round2(s, self.inter_round1);
            }
        }
    }

    /// predict_inter( plane, x, y, w, h, candRow, candCol ) (§7.11.3.1)
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn predict_inter(&mut self, plane: usize, x: usize, y: usize, w: usize, h: usize, cand_row: usize, cand_col: usize) {
        let ci = self.fs.mi(cand_row, cand_col);
        let cand_refs = self.fs.ref_frames[ci];
        let is_compound = cand_refs[1] as i32 > INTRA;
        self.rounding_variables(is_compound);
        if plane == 0 && self.motion_mode == LOCALWARP {
            self.warp_estimation();
            if self.local_valid {
                let (v, ..) = setup_shear(&self.local_warp_params);
                self.local_valid = v;
            }
        }
        let mut preds: [Vec<i32>; 2] = [vec![0; w * h], Vec::new()];
        if is_compound {
            preds[1] = vec![0; w * h];
        }
        let mut global_valid = false;
        for ref_list in 0..1 + is_compound as usize {
            let ref_frame = cand_refs[ref_list] as i32;
            let is_global_mode = self.y_mode == GLOBALMV || self.y_mode == GLOBAL_GLOBALMV;
            if is_global_mode && ref_frame > INTRA && self.hdr.gm_type[ref_frame as usize] > TRANSLATION as u8 {
                let (v, ..) = setup_shear(&self.hdr.gm_params[ref_frame as usize]);
                global_valid = v;
            }
            let use_warp = if w < 8 || h < 8 || self.hdr.force_integer_mv {
                0
            } else if self.motion_mode == LOCALWARP && self.local_valid {
                1
            } else if is_global_mode
                && ref_frame > INTRA
                && self.hdr.gm_type[ref_frame as usize] > TRANSLATION as u8
                && !self.is_scaled(ref_frame as usize)
                && global_valid
            {
                2
            } else {
                0
            };
            let mv = self.fs.mvs[ci][ref_list];
            let ref_idx: isize = if !self.use_intrabc { self.hdr.ref_frame_idx[(ref_frame - LAST_FRAME as i32) as usize] as isize } else { -1 };
            let (start_x, start_y, step_x, step_y) = self.scale_mv(plane, ref_idx, x, y, mv);
            if ref_idx >= 0 && (step_x != 1 << SCALE_SUBPEL_BITS || step_y != 1 << SCALE_SUBPEL_BITS) {
                self.fs.stats.scaled_ref_blocks += 1;
            }
            let mut pred = core::mem::take(&mut preds[ref_list]);
            {
                let rg = self.ref_geom(plane, ref_idx);
                if use_warp != 0 {
                    let wp = if use_warp == 1 { self.local_warp_params } else { self.hdr.gm_params[ref_frame as usize] };
                    for i8 in 0..=((h - 1) >> 3) {
                        for j8 in 0..=((w - 1) >> 3) {
                            self.block_warp(&rg, &wp, plane, x, y, i8, j8, w, h, &mut pred);
                        }
                    }
                } else {
                    let filters = self.fs.interp_filters[ci];
                    block_inter_prediction(&rg, start_x, start_y, step_x, step_y, w, h, filters, self.inter_round0, self.inter_round1, &mut pred);
                }
            }
            if use_warp == 2 && plane == 0 {
                self.fs.stats.global_warp_blocks += 1;
            }
            preds[ref_list] = pred;
        }
        // masks
        if self.compound_type == COMPOUND_WEDGE && plane == 0 {
            let mut m = core::mem::take(&mut self.mask);
            wedge_mask(self.mi_size, self.wedge_sign, self.wedge_index, &mut m);
            self.mask = m;
        } else if self.compound_type == COMPOUND_INTRA {
            self.intra_mode_variant_mask(w, h);
        } else if self.compound_type == COMPOUND_DIFFWTD && plane == 0 {
            let bd = self.fs.bit_depth;
            for i in 0..h {
                for j in 0..w {
                    let mut diff = (preds[0][i * w + j] - preds[1][i * w + j]).abs();
                    diff = round2(diff, (bd - 8) + self.inter_post_round);
                    let m = (38 + diff / 16).clamp(0, 64);
                    self.mask[i * 128 + j] = if self.mask_type != 0 { 64 - m } else { m } as u8;
                }
            }
        }
        if self.compound_type == COMPOUND_DISTANCE {
            self.distance_weights(cand_row, cand_col);
        }
        let maxv = (1i32 << self.fs.bit_depth) - 1;
        let clip1 = |v: i32| v.clamp(0, maxv) as u16;
        if !is_compound && !self.is_inter_intra {
            let p = &mut self.fs.planes[plane];
            for i in 0..h {
                for j in 0..w {
                    p.set(x + j, y + i, clip1(preds[0][i * w + j]));
                }
            }
        } else if self.compound_type == COMPOUND_AVERAGE {
            let sh = 1 + self.inter_post_round;
            let p = &mut self.fs.planes[plane];
            for i in 0..h {
                for j in 0..w {
                    p.set(x + j, y + i, clip1(round2(preds[0][i * w + j] + preds[1][i * w + j], sh)));
                }
            }
        } else if self.compound_type == COMPOUND_DISTANCE {
            let sh = 4 + self.inter_post_round;
            let (fw, bw) = (self.fwd_weight, self.bck_weight);
            let p = &mut self.fs.planes[plane];
            for i in 0..h {
                for j in 0..w {
                    p.set(x + j, y + i, clip1(round2(fw * preds[0][i * w + j] + bw * preds[1][i * w + j], sh)));
                }
            }
        } else {
            self.mask_blend(&preds, plane, x, y, w, h);
        }
        if self.motion_mode == OBMC {
            self.overlapped_motion_compensation(plane, w, h);
        }
    }

    /// The intra mode variant mask process (§7.11.3.13).
    fn intra_mode_variant_mask(&mut self, w: usize, h: usize) {
        let size_scale = MAX_SB_SIZE / h.max(w);
        for i in 0..h {
            for j in 0..w {
                self.mask[i * 128 + j] = match self.interintra_mode {
                    II_V_PRED => II_WEIGHTS_1D[i * size_scale],
                    II_H_PRED => II_WEIGHTS_1D[j * size_scale],
                    II_SMOOTH_PRED => II_WEIGHTS_1D[i.min(j) * size_scale],
                    _ => 32,
                };
            }
        }
    }

    /// The mask blend process (§7.11.3.14).
    fn mask_blend(&mut self, preds: &[Vec<i32>; 2], plane: usize, dst_x: usize, dst_y: usize, w: usize, h: usize) {
        let (sub_x, sub_y) = if plane == 0 { (0, 0) } else { (self.fs.ss_x, self.fs.ss_y) };
        let maxv = (1i32 << self.fs.bit_depth) - 1;
        let post = self.inter_post_round;
        for y in 0..h {
            for x in 0..w {
                let mk = |yy: usize, xx: usize| self.mask[yy * 128 + xx] as i32;
                let m = if (sub_x == 0 && sub_y == 0) || (self.interintra && !self.wedge_interintra) {
                    mk(y, x)
                } else if sub_x != 0 && sub_y == 0 {
                    round2(mk(y, 2 * x) + mk(y, 2 * x + 1), 1)
                } else {
                    round2(mk(2 * y, 2 * x) + mk(2 * y, 2 * x + 1) + mk(2 * y + 1, 2 * x) + mk(2 * y + 1, 2 * x + 1), 2)
                };
                if self.interintra {
                    let pred0 = round2(preds[0][y * w + x], post).clamp(0, maxv);
                    let pred1 = self.fs.planes[plane].get(x + dst_x, y + dst_y) as i32;
                    self.fs.planes[plane].set(x + dst_x, y + dst_y, round2(m * pred1 + (64 - m) * pred0, 6) as u16);
                } else {
                    let pred0 = preds[0][y * w + x];
                    let pred1 = preds[1][y * w + x];
                    self.fs.planes[plane].set(x + dst_x, y + dst_y, round2(m * pred0 + (64 - m) * pred1, 6 + post).clamp(0, maxv) as u16);
                }
            }
        }
    }

    /// The distance weights process (§7.11.3.15).
    fn distance_weights(&mut self, cand_row: usize, cand_col: usize) {
        let rf = self.fs.ref_frames[self.fs.mi(cand_row, cand_col)];
        let mut dist = [0i32; 2];
        for ref_list in 0..2 {
            let h = self.hdr.order_hints[rf[ref_list] as usize];
            dist[ref_list] = crate::obu::get_relative_dist(self.seq, h, self.hdr.order_hint).abs().clamp(0, MAX_FRAME_DISTANCE as i32);
        }
        let d0 = dist[1];
        let d1 = dist[0];
        let order = (d0 <= d1) as usize;
        if d0 == 0 || d1 == 0 {
            self.fwd_weight = QUANT_DIST_LOOKUP[3][order] as i32;
            self.bck_weight = QUANT_DIST_LOOKUP[3][1 - order] as i32;
        } else {
            let mut i = 0;
            while i < 3 {
                let c0 = QUANT_DIST_WEIGHT[i][order] as i32;
                let c1 = QUANT_DIST_WEIGHT[i][1 - order] as i32;
                if order != 0 {
                    if d0 * c0 > d1 * c1 {
                        break;
                    }
                } else if d0 * c0 < d1 * c1 {
                    break;
                }
                i += 1;
            }
            self.fwd_weight = QUANT_DIST_LOOKUP[i][order] as i32;
            self.bck_weight = QUANT_DIST_LOOKUP[i][1 - order] as i32;
        }
    }

    /// The overlapped motion compensation process (§7.11.3.9).
    fn overlapped_motion_compensation(&mut self, plane: usize, w: usize, h: usize) {
        let (sub_x, sub_y) = if plane == 0 { (0, 0) } else { (self.fs.ss_x, self.fs.ss_y) };
        if self.avail_u && self.get_plane_residual_size(self.mi_size, plane) >= BLOCK_8X8 {
            let w4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as usize;
            let mut x4 = self.mi_col;
            let y4 = self.mi_row;
            let mut n_count = 0;
            let n_limit = 4.min(MI_WIDTH_LOG2[self.mi_size] as usize);
            while n_count < n_limit && x4 < self.fs.mi_cols.min(self.mi_col + w4) {
                let cand_row = self.mi_row - 1;
                let cand_col = x4 | 1;
                let cand_sz = self.fs.mi_sizes[self.fs.mi(cand_row, cand_col)] as usize;
                let step4 = (NUM_4X4_BLOCKS_WIDE[cand_sz] as usize).clamp(2, 16);
                if self.fs.ref_frames[self.fs.mi(cand_row, cand_col)][0] as i32 > INTRA {
                    n_count += 1;
                    let pred_w = w.min((step4 * MI_SIZE) >> sub_x);
                    let pred_h = (h >> 1).min(32 >> sub_y);
                    self.predict_overlap(plane, cand_row, cand_col, x4, y4, pred_w, pred_h, 0, pred_h);
                }
                x4 += step4;
            }
        }
        if self.avail_l {
            let h4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as usize;
            let x4 = self.mi_col;
            let mut y4 = self.mi_row;
            let mut n_count = 0;
            let n_limit = 4.min(MI_HEIGHT_LOG2[self.mi_size] as usize);
            while n_count < n_limit && y4 < self.fs.mi_rows.min(self.mi_row + h4) {
                let cand_col = self.mi_col - 1;
                let cand_row = y4 | 1;
                let cand_sz = self.fs.mi_sizes[self.fs.mi(cand_row, cand_col)] as usize;
                let step4 = (NUM_4X4_BLOCKS_HIGH[cand_sz] as usize).clamp(2, 16);
                if self.fs.ref_frames[self.fs.mi(cand_row, cand_col)][0] as i32 > INTRA {
                    n_count += 1;
                    let pred_w = (w >> 1).min(32 >> sub_x);
                    let pred_h = h.min((step4 * MI_SIZE) >> sub_y);
                    self.predict_overlap(plane, cand_row, cand_col, x4, y4, pred_w, pred_h, 1, pred_w);
                }
                y4 += step4;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn predict_overlap(&mut self, plane: usize, cand_row: usize, cand_col: usize, x4: usize, y4: usize, pred_w: usize, pred_h: usize, pass: usize, mask_len: usize) {
        let (sub_x, sub_y) = if plane == 0 { (0, 0) } else { (self.fs.ss_x, self.fs.ss_y) };
        let ci = self.fs.mi(cand_row, cand_col);
        let mv = self.fs.mvs[ci][0];
        let ref_idx = self.hdr.ref_frame_idx[self.fs.ref_frames[ci][0] as usize - LAST_FRAME] as isize;
        let pred_x = (x4 * 4) >> sub_x;
        let pred_y = (y4 * 4) >> sub_y;
        let (start_x, start_y, step_x, step_y) = self.scale_mv(plane, ref_idx, pred_x, pred_y, mv);
        let mut obmc_pred = vec![0i32; pred_w * pred_h];
        {
            let rg = self.ref_geom(plane, ref_idx);
            let filters = self.fs.interp_filters[ci];
            block_inter_prediction(&rg, start_x, start_y, step_x, step_y, pred_w, pred_h, filters, self.inter_round0, self.inter_round1, &mut obmc_pred);
        }
        let maxv = (1i32 << self.fs.bit_depth) - 1;
        let mask: &[u8] = match mask_len {
            2 => &OBMC_MASK_2,
            4 => &OBMC_MASK_4,
            8 => &OBMC_MASK_8,
            16 => &OBMC_MASK_16,
            _ => &OBMC_MASK_32,
        };
        let p = &mut self.fs.planes[plane];
        for i in 0..pred_h {
            for j in 0..pred_w {
                let o = obmc_pred[i * pred_w + j].clamp(0, maxv);
                let m = if pass == 0 { mask[i] } else { mask[j] } as i32;
                let cur = p.get(pred_x + j, pred_y + i) as i32;
                p.set(pred_x + j, pred_y + i, round2(m * cur + (64 - m) * o, 6) as u16);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subpel_filters_have_unit_gain() {
        for f in SUBPEL_FILTERS.iter() {
            for taps in f.iter() {
                assert_eq!(taps.iter().map(|&t| t as i32).sum::<i32>(), 128);
            }
        }
        for taps in WARPED_FILTERS.iter() {
            assert_eq!(taps.iter().map(|&t| t as i32).sum::<i32>(), 128);
        }
    }
    #[test]
    fn identity_warp_is_valid_and_unsheared() {
        let wp = [0, 0, 1 << 16, 0, 0, 1 << 16];
        assert_eq!(setup_shear(&wp), (true, 0, 0, 0, 0));
    }
    #[test]
    fn resolve_divisor_approximates_reciprocal() {
        // 1/3 ~ divFactor / 2^divShift
        let (sh, f) = resolve_divisor(3);
        let approx = f as f64 / (1u64 << sh) as f64;
        assert!((approx - 1.0 / 3.0).abs() < 1e-3);
        let (sh, f) = resolve_divisor(-1000);
        let approx = f as f64 / (1u64 << sh) as f64;
        assert!((approx + 0.001).abs() < 1e-5);
    }
    #[test]
    fn wedge_masks_are_complementary() {
        let mut a = vec![0u8; 128 * 128];
        let mut b = vec![0u8; 128 * 128];
        for wedge in 0..16 {
            wedge_mask(BLOCK_16X16, 0, wedge, &mut a);
            wedge_mask(BLOCK_16X16, 1, wedge, &mut b);
            for i in 0..16 {
                for j in 0..16 {
                    assert_eq!(a[i * 128 + j] as u32 + b[i * 128 + j] as u32, 64);
                }
            }
        }
    }
}
