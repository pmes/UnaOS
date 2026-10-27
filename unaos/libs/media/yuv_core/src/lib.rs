// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CHARTER: Stria (CODEX §2 A/V), seam shared-core — the ONE YCbCr -> RGB kernel gneiss_pal, av1_core and the kernel link.
//! `yuv_core` — Stria's ONE integer Y'CbCr -> R'G'B' kernel (AV1COLOURONE, LEDGER SR340).
//!
//! COLOURONE (SR328) wrote it in gneiss_pal's `dsp/video.rs` as `yuv_coeffs` + `yuv_to_rgba`; AV1COLOURONE moved it here,
//! byte for byte, and retired av1_core's floating-point `planes_to_rgba_with` onto it, so a still (AVIF / HEIC), a
//! played frame and the kernel Player's AV1 picture are converted by the same integer arithmetic. [`coeffs`] derives the
//! fixed-point table of one (Kr, Kb), range and sample depth; [`to_rgba`] runs the one loop over 8-bit or deeper planes:
//! any chroma subsampling, nearest or bilinear (9-3-3-1, centre-sited) chroma, monochrome, the identity (GBR) matrix and
//! YCgCo, and an optional 2x2 ordered dither on the narrowing of samples deeper than 8 bits. No tone mapping.
//!
//! `#![no_std]`, no `alloc`, no dependencies: the caller owns the RGBA buffer.
#![no_std]
#![forbid(unsafe_code)]

/// (Kr, Kb) x 10000 of BT.601 and BT.709 (H.273 Table 4).
pub const KR_KB_601: (i64, i64) = (2990, 1140);
pub const KR_KB_709: (i64, i64) = (2126, 722);

/// (Kr, Kb) x 10000 of an H.273 MatrixCoefficients value that states one: 1 BT.709, 4 FCC, 5 / 6 BT.601, 7 SMPTE 240M,
/// 9 BT.2020 non-constant-luminance. Every other value (0 identity, 2 unspecified, 8 YCgCo, 10 BT.2020 constant-luminance,
/// reserved) is `None`: the caller applies its own default.
pub fn kr_kb(matrix: u8) -> Option<(i64, i64)> {
    match matrix {
        1 => Some(KR_KB_709),
        4 => Some((3000, 1100)),
        5 | 6 => Some(KR_KB_601),
        7 => Some((2120, 870)),
        9 => Some((2627, 593)),
        _ => None,
    }
}

/// `(shift, [yc, rv, gu, gv, bu], y_offset, c_offset)`, the fixed-point table [`coeffs`] derives.
pub type Coeffs = (u32, [i32; 5], i32, i32);

/// The integer YCbCr -> RGB coefficients of one matrix, range and sample depth `n` (COLOURONE SR328):
/// `(shift, [yc, rv, gu, gv, bu], y_offset, c_offset)`. Shift 8 at 8 bits (the historical precision), 16 above; each
/// coefficient rounded half up from the exact rational: yc = 255 / ys, rv = 2(1-Kr) / cs, bu = 2(1-Kb) / cs,
/// gu = 2Kb(1-Kb) / (Kg cs), gv = 2Kr(1-Kr) / (Kg cs), with ys = 219 << (n-8), cs = 224 << (n-8) limited and
/// ys = cs = 2^n - 1 full. The 255 maps every depth straight to 8-bit output.
pub fn coeffs((kr, kb): (i64, i64), full: bool, n: u32) -> Coeffs {
    let s = if n <= 8 { 8 } else { 16 };
    let (ys, cs) = if full { (((1i64 << n) - 1), ((1i64 << n) - 1)) } else { (219i64 << (n - 8), 224i64 << (n - 8)) };
    let rnd = |a: i64, b: i64| ((2 * a + b) / (2 * b)) as i32;
    let kg = 10000 - kr - kb;
    let c = [
        rnd(255 << s, ys),
        rnd(2 * (10000 - kr) * (255 << s), 10000 * cs),
        rnd(2 * kb * (10000 - kb) * (255 << s), 10000 * kg * cs),
        rnd(2 * kr * (10000 - kr) * (255 << s), 10000 * kg * cs),
        rnd(2 * (10000 - kb) * (255 << s), 10000 * cs),
    ];
    let (yoff, coff) = if full { (0, 1i32 << (n - 1)) } else { (16i32 << (n - 8), 128i32 << (n - 8)) };
    (s, c, yoff, coff)
}

/// The matrix [`to_rgba`] applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matrix {
    /// A Kr / Kb matrix; the table is [`coeffs`] of the picture's range and depth.
    Kr((i64, i64)),
    /// A caller's own table at the picture's depth (av1_core's oracle-only libyuv model).
    Table(Coeffs),
    /// H.273 MatrixCoefficients 0: G = Y, B = Cb, R = Cr, each scaled with the luma range.
    Identity,
    /// H.273 MatrixCoefficients 8: R = Y - Cg + Co, G = Y + Cg, B = Y - Cg - Co, exact (one rounded division per channel).
    YCgCo,
}

/// How chroma reaches luma resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chroma {
    /// Each chroma sample covers its 2x2 (or 2x1) luma block.
    Nearest,
    /// 9-3-3-1 (3-1 per subsampled axis) interpolation of centre-sited chroma, edges clamped, in integer at 2 extra
    /// fractional bits per subsampled axis.
    Bilinear,
}

/// One picture's planes. `y` is `y_stride` samples per row; `u` / `v` are `uv_stride` per row at
/// `((w + ss_x) >> ss_x) x ((h + ss_y) >> ss_y)` (ignored when `mono`). `bit_depth` 8..=16; `full` is full range.
#[derive(Debug, Clone, Copy)]
pub struct Picture<'a, T> {
    pub w: usize,
    pub h: usize,
    pub y: &'a [T],
    pub u: &'a [T],
    pub v: &'a [T],
    pub y_stride: usize,
    pub uv_stride: usize,
    pub ss: (u32, u32),
    pub mono: bool,
    pub bit_depth: u32,
    pub full: bool,
}

/// 2x2 ordered (Bayer) dither thresholds, in quarters of an output LSB.
const BAYER: [[i32; 2]; 2] = [[0, 2], [3, 1]];

/// THE ONE Y'CbCr -> RGBA loop: packed 8-bit RGBA, row-major, into `out[..w * h * 4]` (alpha 255). A sample deeper than
/// 8 bits is narrowed inside the one rounded shift, after the conversion; with `dither` (and only above 8 bits) the
/// rounding half is replaced by the 2x2 Bayer bias (1/8, 5/8, 7/8, 3/8 of an LSB, mean 1/2), so a flat field keeps its
/// mean within half an LSB. YCgCo rounds half up (no dither). At 8-bit 4:2:0, `Matrix::Kr`, `Chroma::Nearest` and no
/// dither this is COLOURONE's `yuv_to_rgba` byte for byte.
pub fn to_rgba<T: Copy + Into<i32>>(p: &Picture<'_, T>, m: Matrix, up: Chroma, dither: bool, out: &mut [u8]) {
    let n = p.bit_depth;
    let k = match m {
        Matrix::Table(k) => k,
        Matrix::Kr(kk) => coeffs(kk, p.full, n),
        Matrix::Identity | Matrix::YCgCo => coeffs(KR_KB_601, p.full, n),
    };
    let (s, [yc, rv, gu, gv, bu], yoff, coff) = k;
    let (sx, sy) = p.ss;
    let bil = up == Chroma::Bilinear;
    let fx = if bil && sx != 0 { 2 } else { 0 };
    let fy = if bil && sy != 0 { 2 } else { 0 };
    let f = fx + fy;
    let sh = s + f;
    let dith = dither && n > 8;
    let cw = (p.w + sx as usize) >> sx;
    let ch = (p.h + sy as usize) >> sy;
    // A chroma sample at luma (row, col), scaled by 2^f.
    let chroma = |plane: &[T], row: usize, col: usize| -> i32 {
        if !bil || (sx == 0 && sy == 0) {
            return plane[(row >> sy) * p.uv_stride + (col >> sx)].into();
        }
        let near_far = |pos: usize, ss: u32, len: usize| -> (usize, usize) {
            let near = pos >> ss;
            let far = if ss == 0 { near } else if pos & 1 == 0 { near.saturating_sub(1) } else { (near + 1).min(len - 1) };
            (near, far)
        };
        let (xn, xf) = near_far(col, sx, cw);
        let (yn, yf) = near_far(row, sy, ch);
        let hrow = |r: usize| -> i32 {
            let a: i32 = plane[r * p.uv_stride + xn].into();
            if sx == 0 { a } else { 3 * a + Into::<i32>::into(plane[r * p.uv_stride + xf]) }
        };
        if sy == 0 { hrow(yn) } else { 3 * hrow(yn) + hrow(yf) }
    };
    let (ys, cs) = if p.full { (((1i64 << n) - 1), ((1i64 << n) - 1)) } else { (219i64 << (n - 8), 224i64 << (n - 8)) };
    for row in 0..p.h {
        for col in 0..p.w {
            let yraw: i32 = p.y[row * p.y_stride + col].into();
            let yy = yraw - yoff;
            let bias = if dith { (2 * BAYER[row & 1][col & 1] + 1) << (sh - 3) } else { 1i32 << (sh - 1) };
            let c = ((yc * yy) << f) + bias;
            let (r, g, b) = if p.mono {
                (c >> sh, c >> sh, c >> sh)
            } else {
                let (u, v) = (chroma(p.u, row, col), chroma(p.v, row, col));
                match m {
                    Matrix::Kr(_) | Matrix::Table(_) => {
                        let (cb, cr) = (u - (coff << f), v - (coff << f));
                        ((c + rv * cr) >> sh, (c - gu * cb - gv * cr) >> sh, (c + bu * cb) >> sh)
                    }
                    Matrix::Identity => (((yc * (v - (yoff << f))) + bias) >> sh, c >> sh, ((yc * (u - (yoff << f))) + bias) >> sh),
                    Matrix::YCgCo => {
                        let yn = (yy as i64) * cs << f;
                        let cg = (u - (coff << f)) as i64 * ys;
                        let co = (v - (coff << f)) as i64 * ys;
                        let d = ys * cs << f;
                        let q = |num: i64| (2 * 255 * num + d).div_euclid(2 * d) as i32;
                        (q(yn - cg + co), q(yn + cg), q(yn - cg - co))
                    }
                }
            };
            let o = (row * p.w + col) * 4;
            out[o] = r.clamp(0, 255) as u8;
            out[o + 1] = g.clamp(0, 255) as u8;
            out[o + 2] = b.clamp(0, 255) as u8;
            out[o + 3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One flat 2x2 4:2:0 picture's first pixel.
    fn px<T: Copy + Into<i32>>(y: T, u: T, v: T, n: u32, full: bool, m: Matrix) -> [u8; 3] {
        let (yp, up, vp) = ([y; 4], [u; 1], [v; 1]);
        let p = Picture { w: 2, h: 2, y: &yp, u: &up, v: &vp, y_stride: 2, uv_stride: 1, ss: (1, 1), mono: false, bit_depth: n, full };
        let mut out = [0u8; 16];
        to_rgba(&p, m, Chroma::Nearest, false, &mut out);
        assert_eq!(out[3], 255);
        [out[0], out[1], out[2]]
    }

    #[test]
    fn colourone_tables_moved_byte_for_byte() {
        // COLOURONE's (SR328) KATs, re-run from the kernel's new home.
        assert_eq!(coeffs(KR_KB_601, false, 8), (8, [298, 409, 100, 208, 516], 16, 128));
        assert_eq!(coeffs(KR_KB_709, false, 8), (8, [298, 459, 55, 136, 541], 16, 128));
        assert_eq!(coeffs(KR_KB_601, true, 8), (8, [256, 359, 88, 183, 454], 0, 128));
        assert_eq!(coeffs(kr_kb(9).unwrap(), false, 10), (16, [19077, 27503, 3069, 10657, 35091], 64, 512));
        assert_eq!((kr_kb(0), kr_kb(2), kr_kb(8), kr_kb(10)), (None, None, None, None));
    }

    #[test]
    fn colourone_hand_computed_samples() {
        assert_eq!(px(120u8, 140, 100, 8, false, Matrix::Kr(KR_KB_709)), [71, 133, 146]);
        assert_eq!(px(120u8, 140, 100, 8, false, Matrix::Kr(KR_KB_601)), [76, 139, 145]);
        assert_eq!(px(100u8, 150, 60, 8, true, Matrix::Kr(KR_KB_601)), [5, 141, 139]);
        assert_eq!(px(400u16, 600, 300, 10, false, Matrix::Kr((2627, 593))), [9, 128, 145]);
        // A Main10 sample narrowed after the conversion: (87, 145, 176), not the >> 2 planes' (86, 145, 175).
        assert_eq!(px(515u16, 600, 400, 10, false, Matrix::Kr(KR_KB_601)), [87, 145, 176]);
        assert_eq!(px(128u8, 150, 100, 8, false, Matrix::Kr(KR_KB_601)), [86, 145, 175]);
        // A table passed in is used as given.
        assert_eq!(px(120u8, 140, 100, 8, false, Matrix::Table(coeffs(KR_KB_709, false, 8))), [71, 133, 146]);
    }

    #[test]
    fn av1colourone_bilinear_is_9_3_3_1() {
        // 4x2 luma Y 120, chroma 2x1 Cb (100, 180), Cr 128, BT.709 limited 8-bit. Interpolated Cb per column, x16:
        // 1600, 1920 (3*100 + 180, x4), 2560, 2880 (edges clamped) = 100, 120, 160, 180.
        let (y, u, v) = ([120u8; 8], [100u8, 180], [128u8; 2]);
        let p = Picture { w: 4, h: 2, y: &y, u: &u, v: &v, y_stride: 4, uv_stride: 2, ss: (1, 1), mono: false, bit_depth: 8, full: false };
        let mut out = [0u8; 32];
        to_rgba(&p, Matrix::Kr(KR_KB_709), Chroma::Bilinear, false, &mut out);
        let want = [[121, 127, 62], [121, 123, 104], [121, 114, 189], [121, 110, 231]];
        for row in 0..2 {
            for (col, w) in want.iter().enumerate() {
                assert_eq!(out[(row * 4 + col) * 4..][..3], w[..], "row {row} col {col}");
            }
        }
        // Nearest: each chroma sample covers its 2x2 block.
        to_rgba(&p, Matrix::Kr(KR_KB_709), Chroma::Nearest, false, &mut out);
        assert_eq!((&out[4..7], &out[8..11]), (&[121u8, 127, 62][..], &[121u8, 110, 231][..]));
    }

    #[test]
    fn av1colourone_mono_identity_and_ycgco() {
        // Monochrome 10-bit limited Y 502: grey (yc * 438 + half) >> 16 = 127; no chroma planes read.
        let y = [502u16; 4];
        let p = Picture { w: 2, h: 2, y: &y, u: &[], v: &[], y_stride: 2, uv_stride: 0, ss: (1, 1), mono: true, bit_depth: 10, full: false };
        let mut out = [0u8; 16];
        to_rgba(&p, Matrix::Kr(KR_KB_601), Chroma::Bilinear, false, &mut out);
        assert_eq!(out[..4], [127, 127, 127, 255]);
        // MATRIX-MORE: identity (GBR) is a pass-through at full range (G = Y, B = Cb, R = Cr) and the luma scale limited.
        assert_eq!(px(50u8, 200, 10, 8, true, Matrix::Identity), [10, 50, 200]);
        assert_eq!(px(120u8, 200, 60, 8, false, Matrix::Identity), [51, 121, 214]);
        // YCgCo, exact: full 8-bit Y 100 Cg 140 Co 110 -> 255 * (Y - Cg + Co, Y + Cg, Y - Cg - Co) / 255 = (70, 112, 106);
        // limited (Y / 219, chroma / 224): (64, 111, 105).
        assert_eq!(px(100u8, 140, 110, 8, true, Matrix::YCgCo), [70, 112, 106]);
        assert_eq!(px(100u8, 140, 110, 8, false, Matrix::YCgCo), [64, 111, 105]);
    }

    #[test]
    fn colourone_dither_keeps_the_flat_field_mean() {
        // COLOURONE-DITHER: a 10-bit BT.601 limited grey Y 303 is 69.571 in 8 bits: undithered 70 everywhere; the 2x2 Bayer
        // bias gives (69, 70 / 70, 69), mean 69.5, within half an LSB of 70.
        let (y, u, v) = ([303u16; 16], [512u16; 4], [512u16; 4]);
        let p = Picture { w: 4, h: 4, y: &y, u: &u, v: &v, y_stride: 4, uv_stride: 2, ss: (1, 1), mono: false, bit_depth: 10, full: false };
        let (mut flat, mut dith) = ([0u8; 64], [0u8; 64]);
        to_rgba(&p, Matrix::Kr(KR_KB_601), Chroma::Nearest, false, &mut flat);
        to_rgba(&p, Matrix::Kr(KR_KB_601), Chroma::Nearest, true, &mut dith);
        assert!(flat.chunks(4).all(|c| c == [70, 70, 70, 255]));
        assert_eq!([dith[0], dith[4], dith[16], dith[20]], [69, 70, 70, 69]);
        let (sf, sd) = (flat.chunks(4).map(|c| c[0] as u32).sum::<u32>(), dith.chunks(4).map(|c| c[0] as u32).sum::<u32>());
        assert!((sf as i32 - sd as i32).unsigned_abs() * 2 <= 16, "means {sf}/16 vs {sd}/16");
        // At 8 bits there is no narrowing: dither is a no-op.
        let (y8, c8) = ([120u8; 16], [140u8; 4]);
        let p8 = Picture { w: 4, h: 4, y: &y8, u: &c8, v: &c8, y_stride: 4, uv_stride: 2, ss: (1, 1), mono: false, bit_depth: 8, full: false };
        to_rgba(&p8, Matrix::Kr(KR_KB_709), Chroma::Nearest, false, &mut flat);
        to_rgba(&p8, Matrix::Kr(KR_KB_709), Chroma::Nearest, true, &mut dith);
        assert_eq!(flat, dith);
    }
}
