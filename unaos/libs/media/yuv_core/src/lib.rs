// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CHARTER: Stria (CODEX §2 A/V), seam shared-core — the ONE YCbCr -> RGB kernel gneiss_pal, av1_core, pixel_core, vp8_core, probe_core and the kernel link.
//! `yuv_core` — Stria's ONE integer Y'CbCr -> R'G'B' kernel (AV1COLOURONE, LEDGER SR340).
//!
//! COLOURONE (SR328) wrote it in gneiss_pal's `dsp/video.rs` as `yuv_coeffs` + `yuv_to_rgba`; AV1COLOURONE moved it here,
//! byte for byte, and retired av1_core's floating-point `planes_to_rgba_with` onto it, so a still (AVIF / HEIC), a
//! played frame and the kernel Player's AV1 picture are converted by the same integer arithmetic. [`coeffs`] derives the
//! fixed-point table of one (Kr, Kb), range and sample depth; [`to_rgba`] runs the one loop over 8-bit or deeper planes:
//! any chroma subsampling, nearest or bilinear (9-3-3-1, centre-sited) chroma, monochrome, the identity (GBR) matrix and
//! YCgCo, and an optional 2x2 ordered dither on the narrowing of samples deeper than 8 bits. No tone mapping. VP8COLOURONE (SR356):
//! libwebp's per-product arithmetic ([`Matrix::MultHi`]) and its fancy upsampling ([`Chroma::Fancy`]), so WebP converts here too.
//!
//! `#![no_std]`, no `alloc`, no dependencies: the caller owns the RGBA buffer.
#![no_std]
#![forbid(unsafe_code)]
pub mod scale; // KERNELSCALEONE (rmbp-ledger B965): the ONE nearest resampler (`fit`, `nearest_rows`), user-stria's and the kernel Player's

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

/// The video pick (COLOURONE SR328's `to_rgba_colour` match, moved here by AV1COLOURONE SR340 so the host Player and
/// the kernel Player share it): matrix 0 identity, 8 YCgCo, a stated Kr / Kb by [`kr_kb`], and any other matrix
/// (2 unspecified, 10 BT.2020 constant-luminance, reserved) or `None` the size default — BT.601 below HD, BT.709 when the
/// picture is at least 1280 wide or 720 high. Full range only when `full_range == Some(true)`.
pub fn pick(matrix: Option<u8>, full_range: Option<bool>, w: u32, h: u32) -> (Matrix, bool) {
    let hd = w >= 1280 || h >= 720;
    let m = match matrix {
        Some(0) => Matrix::Identity,
        Some(8) => Matrix::YCgCo,
        mc => Matrix::Kr(mc.and_then(kr_kb).unwrap_or(if hd { KR_KB_709 } else { KR_KB_601 })),
    };
    (m, full_range == Some(true))
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

/// libwebp's fixed-point form (VP8COLOURONE SR356): `([yc, rv, gu, gv, bu], [kr, kg, kb])`, coefficients at 8 fractional bits,
/// each product of a RAW 8-bit sample truncated `>> 8` on its own (libwebp's `MultHi`; `>> 8 + f` with Bilinear's carried bits),
/// plus the channel constant, the sum at 6 fractional bits clamped (libwebp's `VP8Clip8`). No affine table is this function:
/// the per-product truncation of the chroma term is not linear (measured over all 2^24 triples).
pub type MultHi = ([i32; 5], [i32; 3]);

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
    /// libwebp's per-product arithmetic ([`MultHi`]): vp8_core's WebP converter. Monochrome reads the BT.601 grey.
    MultHi(MultHi),
}

/// How chroma reaches luma resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chroma {
    /// Each chroma sample covers its 2x2 (or 2x1) luma block.
    Nearest,
    /// 9-3-3-1 (3-1 per subsampled axis) interpolation of centre-sited chroma, edges clamped, in integer at 2 extra
    /// fractional bits per subsampled axis.
    Bilinear,
    /// Bilinear's 9-3-3-1 sum rounded half up to the sample BEFORE the matrix: libwebp's `UpsampleRgbaLinePair` (VP8COLOURONE
    /// SR356). libjpeg's fancy filters are the same kernel with their own parity bias: [`fancy`] (YUVFANCY SR369).
    Fancy,
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

/// The whole picture through [`to_rgba_rows`]: packed 8-bit RGBA, row-major, into `out[..w * h * 4]`.
pub fn to_rgba<T: Copy + Into<i32>>(p: &Picture<'_, T>, m: Matrix, up: Chroma, dither: bool, out: &mut [u8]) {
    to_rgba_rows(p, 0, p.h, m, up, dither, out)
}

/// THE ONE Y'CbCr -> RGBA loop over rows `y0..y1` and the columns `cols` (absolute, any order, repeats allowed, each < w) of the
/// picture: packed 8-bit RGBA, row-major, into `out[..(y1 - y0) * ncols * 4]` (alpha 255; KERNELSCALECOLS rmbp-ledger B978: the
/// column list, [`to_rgba_rows`] its `0..w` call); every neighbour read and the dither keyed on the absolute row and column, so its
/// pixels are the whole picture's pixels byte for
/// byte (KERNELSCALECONVERT rmbp-ledger B971: the kernel Player's shown rows). A sample deeper than
/// 8 bits is narrowed inside the one rounded shift, after the conversion; with `dither` (and only above 8 bits) the
/// rounding half is replaced by the 2x2 Bayer bias (1/8, 5/8, 7/8, 3/8 of an LSB, mean 1/2), so a flat field keeps its
/// mean within half an LSB. YCgCo rounds half up (no dither). At 8-bit 4:2:0, `Matrix::Kr`, `Chroma::Nearest` and no
/// dither this is COLOURONE's `yuv_to_rgba` byte for byte.
pub fn to_rgba_cols<T: Copy + Into<i32>>(p: &Picture<'_, T>, y0: usize, y1: usize, cols: impl Iterator<Item = usize> + Clone, m: Matrix, up: Chroma, dither: bool, out: &mut [u8]) {
    let n = p.bit_depth;
    let k = match m {
        Matrix::Table(k) => k,
        Matrix::Kr(kk) => coeffs(kk, p.full, n),
        Matrix::Identity | Matrix::YCgCo | Matrix::MultHi(_) => coeffs(KR_KB_601, p.full, n),
    };
    let (s, [yc, rv, gu, gv, bu], yoff, coff) = k;
    let (sx, sy) = p.ss;
    let bil = up != Chroma::Nearest;
    let rf = up == Chroma::Fancy;
    let fx = if bil && sx != 0 { 2 } else { 0 };
    let fy = if bil && sy != 0 { 2 } else { 0 };
    let fs = fx + fy;
    let f = if rf { 0 } else { fs };
    let sh = s + f;
    let dith = dither && n > 8;
    let cw = (p.w + sx as usize) >> sx;
    let ch = (p.h + sy as usize) >> sy;
    // A chroma sample at luma (row, col), scaled by 2^f.
    let chroma = |plane: &[T], row: usize, col: usize| -> i32 {
        if !bil || (sx == 0 && sy == 0) {
            return plane[(row >> sy) * p.uv_stride + (col >> sx)].into();
        }
        // YUVFANCY (SR369): the ONE 9-3-3-1 kernel is [`fancy_sum`] / [`fancy`] at the file tail; Fancy is libwebp's, half at both parities.
        let h = (1i32 << fs) >> 1;
        if rf { fancy(plane, p.uv_stride, (cw, ch), p.ss, (row, col), (h, h)) } else { fancy_sum(plane, p.uv_stride, (cw, ch), p.ss, (row, col)) }
    };
    let (ys, cs) = if p.full { (((1i64 << n) - 1), ((1i64 << n) - 1)) } else { (219i64 << (n - 8), 224i64 << (n - 8)) };
    let mut cur = 0usize;
    for row in y0..y1.min(p.h) {
        for col in cols.clone() {
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
                    Matrix::MultHi(([wy, wrv, wgu, wgv, wbu], [kr, kg, kb])) => {
                        let hi = |s: i32, w: i32| (s * w) >> (8 + f);
                        let yh = (yraw * wy) >> 8;
                        ((yh + hi(v, wrv) + kr) >> 6, (yh - hi(u, wgu) - hi(v, wgv) + kg) >> 6, (yh + hi(u, wbu) + kb) >> 6)
                    }
                }
            };
            let o = cur; cur += 4;
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
        // The video pick: the size default at 1280 wide or 720 high, a stated matrix wins, full only on Some(true).
        assert_eq!(pick(None, None, 2, 2), (Matrix::Kr(KR_KB_601), false));
        assert_eq!(pick(Some(2), Some(false), 1280, 2), (Matrix::Kr(KR_KB_709), false));
        assert_eq!(pick(Some(10), None, 2, 720), (Matrix::Kr(KR_KB_709), false));
        assert_eq!(pick(Some(6), Some(true), 1920, 1080), (Matrix::Kr(KR_KB_601), true));
        assert_eq!((pick(Some(0), None, 2, 2).0, pick(Some(8), None, 2, 2).0), (Matrix::Identity, Matrix::YCgCo));
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

    /// libwebp's constants (vp8_core's `yuv::WEBP`).
    const WEBP: MultHi = ([19077, 26149, 6419, 13320, 33050], [-14234, 8708, -17685]);

    #[test]
    fn vp8colourone_multhi_is_libwebps_arithmetic() {
        // Hand-computed from libwebp's MultHi / VP8Clip8: Y 120 Cb 140 Cr 100 -> (76, 139, 145); limited black and white.
        assert_eq!(px(120u8, 140, 100, 8, false, Matrix::MultHi(WEBP)), [76, 139, 145]);
        assert_eq!(px(16u8, 128, 128, 8, false, Matrix::MultHi(WEBP)), [0, 0, 0]);
        assert_eq!(px(235u8, 128, 128, 8, false, Matrix::MultHi(WEBP)), [255, 255, 255]);
    }

    #[test]
    fn vp8colourone_fancy_rounds_first() {
        // 4x2 luma Y 120, chroma 2x1 Cb (100, 181), Cr 128. The 9-3-3-1 Cb per column, x16: 1600, 1924, 2572, 2896.
        // Fancy rounds each half up to the sample first (100, 120, 161, 181), so it equals Nearest over those planes at
        // ss (0, 0); Bilinear carries 120.25 / 160.75 into the matrix instead.
        let (y, u, v) = ([120u8; 8], [100u8, 181], [128u8; 2]);
        let p = Picture { w: 4, h: 2, y: &y, u: &u, v: &v, y_stride: 4, uv_stride: 2, ss: (1, 1), mono: false, bit_depth: 8, full: false };
        let (mut fancy, mut bil, mut full) = ([0u8; 32], [0u8; 32], [0u8; 32]);
        to_rgba(&p, Matrix::MultHi(WEBP), Chroma::Fancy, false, &mut fancy);
        to_rgba(&p, Matrix::MultHi(WEBP), Chroma::Bilinear, false, &mut bil);
        let (fu, fv) = ([100u8, 120, 161, 181, 100, 120, 161, 181], [128u8; 8]);
        let q = Picture { u: &fu, v: &fv, uv_stride: 4, ss: (0, 0), ..p };
        to_rgba(&q, Matrix::MultHi(WEBP), Chroma::Nearest, false, &mut full);
        assert_eq!(fancy, full);
        assert_ne!(fancy, bil);
        // Kr / Table pictures are untouched by the new mode: Fancy over Kr equals Nearest over the rounded planes too.
        to_rgba(&p, Matrix::Kr(KR_KB_709), Chroma::Fancy, false, &mut fancy);
        to_rgba(&q, Matrix::Kr(KR_KB_709), Chroma::Nearest, false, &mut full);
        assert_eq!(fancy, full);
    }

    #[test]
    fn to_rgba_rows_are_to_rgbas_rows() {
        // KERNELSCALECONVERT (rmbp-ledger B971): every row window of an LCG 10-bit 7x5 picture, 4:2:0 and 4:2:2, Nearest /
        // Bilinear / Fancy, dither on and off, equals the whole conversion's slice.
        const W: usize = 7;
        const H: usize = 5;
        let mut s = 0x9e37_79b9u32;
        let mut next = || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            64 + ((s >> 16) % 877) as u16
        };
        let mut y = [0u16; W * H];
        let (mut u, mut v) = ([0u16; 4 * H], [0u16; 4 * H]);
        y.iter_mut().for_each(|b| *b = next());
        u.iter_mut().for_each(|b| *b = next());
        v.iter_mut().for_each(|b| *b = next());
        for ss in [(1u32, 1u32), (1, 0)] {
            let p = Picture { w: W, h: H, y: &y, u: &u, v: &v, y_stride: W, uv_stride: 4, ss, mono: false, bit_depth: 10, full: false };
            for up in [Chroma::Nearest, Chroma::Bilinear, Chroma::Fancy] {
                for dither in [false, true] {
                    let mut full = [0u8; W * H * 4];
                    to_rgba(&p, Matrix::Kr(KR_KB_709), up, dither, &mut full);
                    for y0 in 0..H {
                        for y1 in y0 + 1..=H {
                            let mut rows = [0u8; W * H * 4];
                            to_rgba_rows(&p, y0, y1, Matrix::Kr(KR_KB_709), up, dither, &mut rows);
                            assert_eq!(rows[..(y1 - y0) * W * 4], full[y0 * W * 4..y1 * W * 4], "{ss:?} {up:?} {dither} {y0}..{y1}");
                        }
                    }
                }
            }
            // The kernel Player's AV1 shown rows: each sampled source row through to_rgba_rows (Bilinear), its columns sampled,
            // equals the whole Bilinear conversion then nearest-sampled.
            let mut full = [0u8; W * H * 4];
            to_rgba(&p, Matrix::Kr(KR_KB_709), Chroma::Bilinear, false, &mut full);
            for (dw, dh) in [(3usize, 2usize), (11, 9)] {
                let mut idx = [0u32; H];
                idx.iter_mut().enumerate().for_each(|(i, r)| *r = i as u32);
                let col = Picture { w: 1, h: H, y: &idx[..], u: &[], v: &[], y_stride: 1, uv_stride: 0, ss: (0, 0), mono: true, bit_depth: 8, full: true };
                let mut one = [0u32; 1];
                let mut seen = 0;
                crate::scale::nearest_rows(&col, 1, dh, &mut one, &mut [], &mut [], |r, sr| {
                    let sy = sr.y[0] as usize;
                    let mut row = [0u8; W * 4];
                    to_rgba_rows(&p, sy, sy + 1, Matrix::Kr(KR_KB_709), Chroma::Bilinear, false, &mut row);
                    for c in 0..dw {
                        let sx = (c * W / dw).min(W - 1);
                        assert_eq!(row[sx * 4..sx * 4 + 4], full[(sy * W + sx) * 4..(sy * W + sx) * 4 + 4], "{dw}x{dh} row {r} col {c}");
                    }
                    assert_eq!(sy, (r * H / dh).min(H - 1));
                    seen += 1;
                });
                assert_eq!(seen, dh);
            }
        }
    }
}

// ── H264ENCPLANES (rmbp-ledger B973): the ONE R'G'B' -> Y'CbCr, beside `to_rgba` (handlers.md §5 NLEPROBE-HDR's H264ENC-PLANES) ──
// h264enc_core's private BT.601 table (`yuv.rs` `I420::from_rgba`, the export's input side) moved here and became a function of
// (Kr, Kb) and range, so both directions live in one crate and derive from the same `kr_kb` values: the BT.601-limited table is
// the retired one bit for bit (measured, pinned below), so every RGBA export keeps its bytes. The kernel export converts a mixed
// or non-matching frame at its FILE's matrix through it (`unaos/crates/kernel/src/video/export.rs`).

/// `([y_r, y_g, y_b], [cb_r, cb_g, cb_b], [cr_r, cr_g, cr_b], y_offset)`, the coefficients x 65536 [`rgb_coeffs`] derives.
pub type RgbCoeffs = ([i64; 3], [i64; 3], [i64; 3], i64);

/// The integer R'G'B' -> Y'CbCr coefficients of one (Kr, Kb) and range at 8 bits, 16 fractional bits: each Y' coefficient
/// rounded half up from Kx x ys / 255 (ys 219 limited, 255 full); Cb's B and Cr's R rounded from cs / 510 (cs 224 limited, 255
/// full), Cb's R from Kr cs / (510 (1-Kb)) and Cr's B from Kb cs / (510 (1-Kr)), each chroma G = -(R + B) so the row sums to
/// zero and any grey has Cb = Cr = 128 exactly. The Y' offset is 16 limited, 0 full.
pub fn rgb_coeffs((kr, kb): (i64, i64), full: bool) -> RgbCoeffs {
    let (ys, cs) = if full { (255i64, 255i64) } else { (219i64, 224i64) };
    let rnd = |a: i64, b: i64| (2 * a + b) / (2 * b);
    let kg = 10000 - kr - kb;
    let ky = [rnd((kr * ys) << 16, 10000 * 255), rnd((kg * ys) << 16, 10000 * 255), rnd((kb * ys) << 16, 10000 * 255)];
    let half = rnd(cs << 16, 2 * 255);
    let cb_r = rnd((kr * cs) << 16, 2 * (10000 - kb) * 255);
    let cr_b = rnd((kb * cs) << 16, 2 * (10000 - kr) * 255);
    (ky, [-cb_r, cb_r - half, half], [half, cr_b - half, -cr_b], if full { 0 } else { 16 })
}

/// THE ONE straight-RGBA -> 8-bit 4:2:0 Y'CbCr: `w * h * 4` bytes, top row first, into `y` (`w x h`) and `u` / `v`
/// (`ceil(w/2) x ceil(h/2)`), row-major, no padding, by [`rgb_coeffs`] of `kr_kb` and `full`. Y' per pixel; chroma sited at
/// the 2x2 block and computed from the block's RGB sums (the block's average; an edge block of an odd size averages the pixels it
/// has). Alpha is ignored. False (nothing written) when a buffer is short. No alloc: the caller owns the planes.
#[allow(clippy::too_many_arguments)]
pub fn from_rgba(rgba: &[u8], w: usize, h: usize, kr_kb: (i64, i64), full: bool, y: &mut [u8], u: &mut [u8], v: &mut [u8]) -> bool {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    if rgba.len() < w * h * 4 || y.len() < w * h || u.len() < cw * ch || v.len() < cw * ch {
        return false;
    }
    let (ky, ku, kv, yoff) = rgb_coeffs(kr_kb, full);
    for row in 0..h {
        for col in 0..w {
            let p = &rgba[(row * w + col) * 4..];
            let s = ky[0] * p[0] as i64 + ky[1] * p[1] as i64 + ky[2] * p[2] as i64;
            y[row * w + col] = (yoff + ((s + 32768) >> 16)).clamp(0, 255) as u8;
        }
    }
    for by in 0..ch {
        for bx in 0..cw {
            let (mut sum, mut n) = ([0i64; 3], 0i64);
            for row in by * 2..(by * 2 + 2).min(h) {
                for col in bx * 2..(bx * 2 + 2).min(w) {
                    let p = &rgba[(row * w + col) * 4..];
                    for c in 0..3 {
                        sum[c] += p[c] as i64;
                    }
                    n += 1;
                }
            }
            let chroma = |k: &[i64; 3]| (128 + (k[0] * sum[0] + k[1] * sum[1] + k[2] * sum[2] + n * 32768).div_euclid(n * 65536)).clamp(0, 255) as u8;
            u[by * cw + bx] = chroma(&ku);
            v[by * cw + bx] = chroma(&kv);
        }
    }
    true
}

#[cfg(test)]
mod from_rgba_kats {
    use super::*;

    /// H264ENCPLANES (B973): the BT.601-limited table is h264enc_core's retired `KY` / `KU` / `KV`, bit for bit.
    #[test]
    fn h264encplanes_bt601_limited_is_the_retired_table() {
        assert_eq!(rgb_coeffs(KR_KB_601, false), ([16829, 33039, 6416], [-9714, -19070, 28784], [28784, -24103, -4681], 16));
        assert_eq!(rgb_coeffs(KR_KB_601, true).3, 0);
        for (kk, full) in [(KR_KB_601, false), (KR_KB_709, false), (KR_KB_601, true), (KR_KB_709, true)] {
            let (_, u, v, _) = rgb_coeffs(kk, full);
            assert_eq!((u.iter().sum::<i64>(), v.iter().sum::<i64>()), (0, 0), "a grey is Cb = Cr = 128");
        }
    }

    const ALL: [((i64, i64), bool); 4] = [(KR_KB_601, false), (KR_KB_709, false), (KR_KB_601, true), (KR_KB_709, true)];

    /// Planes first: a flat in-gamut 2x2 picture -> `to_rgba` -> `from_rgba` is within 1 per sample (measured 0 / 1 / 1 / 1 for
    /// 601 / 709 limited, 601 / 709 full over this grid); a triple whose RGB clamps (out of gamut) is skipped.
    #[test]
    fn h264encplanes_planes_round_trip_within_one() {
        for (kk, full) in ALL {
            let (lo, hi, clo, chi) = if full { (0u32, 255u32, 0u32, 255u32) } else { (16, 235, 16, 240) };
            for yy in (lo..=hi).step_by(5) {
                for uu in (clo..=chi).step_by(5) {
                    for vv in (clo..=chi).step_by(5) {
                        let (py, pu, pv) = ([yy as u8; 4], [uu as u8], [vv as u8]);
                        let p = Picture { w: 2, h: 2, y: &py[..], u: &pu[..], v: &pv[..], y_stride: 2, uv_stride: 1, ss: (1, 1), mono: false, bit_depth: 8, full };
                        let mut rgba = [0u8; 16];
                        to_rgba(&p, Matrix::Kr(kk), Chroma::Nearest, false, &mut rgba);
                        if rgba.chunks(4).any(|c| c[..3].iter().any(|&x| x == 0 || x == 255)) {
                            continue;
                        }
                        let (mut y2, mut u2, mut v2) = ([0u8; 4], [0u8; 1], [0u8; 1]);
                        assert!(from_rgba(&rgba, 2, 2, kk, full, &mut y2, &mut u2, &mut v2));
                        let ok = y2.iter().all(|&s| s.abs_diff(yy as u8) <= 1) && u2[0].abs_diff(uu as u8) <= 1 && v2[0].abs_diff(vv as u8) <= 1;
                        assert!(ok, "{:?} full={} ({}, {}, {}) -> {:?} {:?} {:?}", kk, full, yy, uu, vv, y2, u2, v2);
                    }
                }
            }
        }
    }

    /// RGB first: a flat 2x2 RGBA -> `from_rgba` -> `to_rgba` is within 2 per channel (measured 2: the 8-bit Y'CbCr grid is
    /// coarser than R'G'B', so the arc's 1 holds only planes first).
    #[test]
    fn h264encplanes_rgba_round_trip_within_two() {
        for (kk, full) in ALL {
            for r in (0u32..=255).step_by(5) {
                for g in (0u32..=255).step_by(5) {
                    for b in (0u32..=255).step_by(5) {
                        let rgba = [r as u8, g as u8, b as u8, 255].repeat(4);
                        let (mut y, mut u, mut v) = ([0u8; 4], [0u8; 1], [0u8; 1]);
                        assert!(from_rgba(&rgba, 2, 2, kk, full, &mut y, &mut u, &mut v));
                        let p = Picture { w: 2, h: 2, y: &y[..], u: &u[..], v: &v[..], y_stride: 2, uv_stride: 1, ss: (1, 1), mono: false, bit_depth: 8, full };
                        let mut back = [0u8; 16];
                        to_rgba(&p, Matrix::Kr(kk), Chroma::Nearest, false, &mut back);
                        for (a, z) in rgba.iter().zip(back.iter()) {
                            assert!(a.abs_diff(*z) <= 2, "{:?} full={} ({}, {}, {}) -> {:?}", kk, full, r, g, b, &back[..4]);
                        }
                    }
                }
            }
        }
    }

    /// Short buffers write nothing.
    #[test]
    fn h264encplanes_short_buffers_refused() {
        let (mut y, mut u, mut v) = ([7u8; 4], [7u8; 1], [7u8; 1]);
        assert!(!from_rgba(&[0; 15], 2, 2, KR_KB_601, false, &mut y, &mut u, &mut v));
        assert!(!from_rgba(&[0; 16], 2, 2, KR_KB_601, false, &mut y[..3], &mut u, &mut v));
        assert_eq!((y, u, v), ([7; 4], [7], [7]));
    }
}

// ── KERNELSCALECOLS (rmbp-ledger B978): the row range's column list (handlers.md §5 NLEPROBE-HDR's KERNEL-SCALE-BILINEAR-COLS) ──
// `to_rgba_rows`' loop became [`to_rgba_cols`] in place: every read (luma, the Bilinear / Fancy near and far chroma from the whole
// planes, the Bayer dither) was already keyed on the absolute (row, column), so a column LIST needs no window and no neighbour
// bookkeeping. A downscale's sampled columns span the full width (a window saves nothing); the list converts only them: the
// kernel Player's AV1 present of a 1080p picture in a 640x360 fit converts 640 x 360 = 230,400 pixels where it converted 691,200.

/// Rows `y0..y1`, every column: [`to_rgba_cols`] over `0..w`, into `out[..(y1 - y0) * w * 4]` (KERNELSCALECONVERT B971's call).
pub fn to_rgba_rows<T: Copy + Into<i32>>(p: &Picture<'_, T>, y0: usize, y1: usize, m: Matrix, up: Chroma, dither: bool, out: &mut [u8]) {
    to_rgba_cols(p, y0, y1, 0..p.w, m, up, dither, out)
}

#[cfg(test)]
mod cols_kats {
    use super::*;

    const W: usize = 7;
    const H: usize = 5;

    /// The B971 KAT's LCG 10-bit 7x5 planes (chroma 7 wide so 4:4:4 fits).
    fn planes() -> ([u16; W * H], [u16; W * H], [u16; W * H]) {
        let mut s = 0x9e37_79b9u32;
        let mut next = || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            64 + ((s >> 16) % 877) as u16
        };
        let (mut y, mut u, mut v) = ([0u16; W * H], [0u16; W * H], [0u16; W * H]);
        y.iter_mut().for_each(|b| *b = next());
        u.iter_mut().for_each(|b| *b = next());
        v.iter_mut().for_each(|b| *b = next());
        (y, u, v)
    }

    #[test]
    fn to_rgba_cols_are_the_rows_columns() {
        // KERNELSCALECOLS (B978): every ascending column subset of 0..7 and some unordered / repeated lists, every row window,
        // 4:2:0 / 4:2:2 / 4:4:4 / mono, three matrices, Nearest / Bilinear / Fancy, dither off and on: the whole conversion's picks.
        let (y, u, v) = planes();
        let mut lists: [[usize; W]; 130] = [[0; W]; 130];
        let mut lens = [0usize; 130];
        for mask in 1..128usize {
            for c in 0..W {
                if mask >> c & 1 == 1 {
                    lists[mask][lens[mask]] = c;
                    lens[mask] += 1;
                }
            }
        }
        lists[128][..5].copy_from_slice(&[6, 0, 3, 3, 1]);
        lens[128] = 5;
        lists[129][..3].copy_from_slice(&[4, 4, 4]);
        lens[129] = 3;
        for (ss, mono) in [((1u32, 1u32), false), ((1, 0), false), ((0, 0), false), ((1, 1), true)] {
            let p = Picture { w: W, h: H, y: &y, u: &u, v: &v, y_stride: W, uv_stride: W, ss, mono, bit_depth: 10, full: false };
            for m in [Matrix::Kr(KR_KB_709), Matrix::Identity, Matrix::YCgCo] {
                for up in [Chroma::Nearest, Chroma::Bilinear, Chroma::Fancy] {
                    for dither in [false, true] {
                        let mut full = [0u8; W * H * 4];
                        to_rgba(&p, m, up, dither, &mut full);
                        for i in 1..130 {
                            let cols = &lists[i][..lens[i]];
                            for y0 in 0..H {
                                for y1 in y0 + 1..=H {
                                    let mut got = [0u8; W * H * 4];
                                    to_rgba_cols(&p, y0, y1, cols.iter().copied(), m, up, dither, &mut got);
                                    let mut o = 0;
                                    for row in y0..y1 {
                                        for &c in cols {
                                            let f = (row * W + c) * 4;
                                            assert_eq!(got[o..o + 4], full[f..f + 4], "{ss:?} {mono} {m:?} {up:?} {dither} {cols:?} {y0}..{y1}");
                                            o += 4;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn kernel_present_cols_are_full_bilinear_then_nearest() {
        // The kernel Player's AV1 arm (vplay.rs `Frame::rows`): the scaler's column index, its consecutive dedup, each sampled
        // source row through to_rgba_cols over the distinct columns, the output gathered: == full Bilinear then nearest, and the
        // converted pixels are distinct rows x distinct columns.
        let (y, u, v) = planes();
        let p = Picture { w: W, h: H, y: &y, u: &u, v: &v, y_stride: W, uv_stride: W, ss: (1, 1), mono: false, bit_depth: 10, full: false };
        let mut full = [0u8; W * H * 4];
        to_rgba(&p, Matrix::Kr(KR_KB_709), Chroma::Bilinear, false, &mut full);
        let mut idx = [0u32; W];
        idx.iter_mut().enumerate().for_each(|(i, r)| *r = i as u32);
        for (dw, dh, distinct) in [(3usize, 2usize, 3usize), (5, 4, 5), (11, 9, 7), (14, 10, 7)] {
            let row_pic = Picture { w: W, h: 1, y: &idx[..], u: &[], v: &[], y_stride: W, uv_stride: 0, ss: (0, 0), mono: true, bit_depth: 8, full: true };
            let mut sxs = [0u32; 14];
            crate::scale::nearest_rows(&row_pic, dw, 1, &mut sxs, &mut [], &mut [], |_, _| {});
            let (mut cols, mut at, mut n) = ([0usize; 14], [0usize; 14], 0usize);
            for c in 0..dw {
                if n == 0 || cols[n - 1] != sxs[c] as usize {
                    cols[n] = sxs[c] as usize;
                    n += 1;
                }
                at[c] = n - 1;
            }
            assert_eq!(n, distinct, "{dw}x{dh}");
            let col_pic = Picture { w: 1, h: H, y: &idx[..H], u: &[], v: &[], y_stride: 1, uv_stride: 0, ss: (0, 0), mono: true, bit_depth: 8, full: true };
            let (mut one, mut converted, mut last) = ([0u32; 1], 0usize, usize::MAX);
            let mut line = [0u8; W * 4];
            crate::scale::nearest_rows(&col_pic, 1, dh, &mut one, &mut [], &mut [], |r, s| {
                let sy = s.y[0] as usize;
                if last != sy {
                    to_rgba_cols(&p, sy, sy + 1, cols[..n].iter().copied(), Matrix::Kr(KR_KB_709), Chroma::Bilinear, false, &mut line);
                    converted += n;
                    last = sy;
                }
                for c in 0..dw {
                    let (sx, o) = (sxs[c] as usize, at[c] * 4);
                    assert_eq!(sx, (c * W / dw).min(W - 1));
                    assert_eq!(line[o..o + 4], full[(sy * W + sx) * 4..(sy * W + sx) * 4 + 4], "{dw}x{dh} row {r} col {c}");
                }
            });
            assert_eq!(converted, dh.min(H) * n, "{dw}x{dh}");
        }
    }
}

/// YUVFANCY (LEDGER SR369): THE ONE chroma upsampling kernel, moved out of [`to_rgba`]'s loop byte for byte. The chroma
/// sample at luma `(row, col)` of a plane `stride` samples per row, `cw x ch` valid, subsampled by `(sx, sy)`: the 9-3-3-1
/// sum of centre-sited chroma (3-1 per subsampled axis, the sample itself on an axis that is not), edges clamped, at 2^fs
/// with fs = 2 per subsampled axis. [`Chroma::Bilinear`] carries it into the matrix unrounded; [`fancy`] rounds it.
#[inline]
pub fn fancy_sum<T: Copy + Into<i32>>(plane: &[T], stride: usize, (cw, ch): (usize, usize), (sx, sy): (u32, u32), (row, col): (usize, usize)) -> i32 {
    let near_far = |pos: usize, ss: u32, len: usize| -> (usize, usize) {
        let near = pos >> ss;
        let far = if ss == 0 { near } else if pos & 1 == 0 { near.saturating_sub(1) } else { (near + 1).min(len - 1) };
        (near, far)
    };
    let (xn, xf) = near_far(col, sx, cw);
    let (yn, yf) = near_far(row, sy, ch);
    let hrow = |r: usize| -> i32 {
        let a: i32 = plane[r * stride + xn].into();
        if sx == 0 { a } else { 3 * a + Into::<i32>::into(plane[r * stride + xf]) }
    };
    if sy == 0 { hrow(yn) } else { 3 * hrow(yn) + hrow(yf) }
}

/// YUVFANCY (SR369): [`fancy_sum`] rounded ONCE to the sample, `(sum + bias) >> fs`, the bias the caller's `(even, odd)` by
/// the parity of `col` when the plane is subsampled across (`sx != 0`), else of `row`; the sample itself when nothing is
/// subsampled. The caller chooses the pair for its subsampling: libwebp's `UpsampleRgbaLinePair` is (8, 8) at 4:2:0 (half
/// at both parities: [`Chroma::Fancy`]); libjpeg's `h2v2_fancy_upsample` is (8, 7), its `h2v1_fancy_upsample` (1, 2) by
/// column and `h1v2_fancy_upsample` (1, 2) by row (pixel_core's JPEG upsampler).
#[inline]
pub fn fancy<T: Copy + Into<i32>>(plane: &[T], stride: usize, dims: (usize, usize), ss: (u32, u32), pos: (usize, usize), (even, odd): (i32, i32)) -> i32 {
    let fs: u32 = (if ss.0 != 0 { 2 } else { 0 }) + (if ss.1 != 0 { 2 } else { 0 });
    let sum = fancy_sum(plane, stride, dims, ss, pos);
    if fs == 0 {
        return sum;
    }
    let par = if ss.0 != 0 { pos.1 } else { pos.0 };
    (sum + if par & 1 == 0 { even } else { odd }) >> fs
}

#[cfg(test)]
mod yuvfancy_tests {
    //! YUVFANCY (SR369) KATs: the one kernel under libwebp's and libjpeg's bias pairs, by hand from their formulas.
    use super::*;

    #[test]
    fn yuvfancy_libwebp_and_libjpeg_bias_by_hand() {
        // A 4x2 4:2:0 picture: chroma 2x1 = [10, 12] (one row, so the vertical 3-1 sees it twice). Sums at 2^4:
        // col 0 16*10 = 160, col 1 4*(3*10 + 12) = 168, col 2 4*(3*12 + 10) = 184, col 3 16*12 = 192.
        // libwebp (8, 8): 170>>4, 176>>4, 192>>4, 200>>4 = 10 11 12 12. libjpeg h2v2 (8, 7): col 1 is odd, 175>>4 = 10.
        let c = [10u8, 12];
        let row = |r: usize, b: (i32, i32)| -> [i32; 4] { core::array::from_fn(|x| fancy(&c, 2, (2, 1), (1, 1), (r, x), b)) };
        for r in 0..2 {
            assert_eq!(row(r, (8, 8)), [10, 11, 12, 12], "libwebp row {r}");
            assert_eq!(row(r, (8, 7)), [10, 10, 12, 12], "libjpeg h2v2 row {r}");
        }
        assert_eq!(fancy_sum(&c, 2, (2, 1), (1, 1), (0, 1)), 168);
    }

    #[test]
    fn yuvfancy_1d_bias_by_row_and_column() {
        // [10, 12] 2:1 on one axis. Sums at 2^2: 40, 42 (3*10 + 12), 46 (3*12 + 10), 48.
        // libjpeg (1, 2): 41>>2, 44>>2, 47>>2, 50>>2 = 10 11 11 12; half (2, 2): 10 11 12 12.
        let c = [10u8, 12];
        let h2v1: [i32; 4] = core::array::from_fn(|x| fancy(&c, 2, (2, 1), (1, 0), (0, x), (1, 2)));
        assert_eq!(h2v1, [10, 11, 11, 12], "h2v1 by column");
        let half: [i32; 4] = core::array::from_fn(|x| fancy(&c, 2, (2, 1), (1, 0), (0, x), (2, 2)));
        assert_eq!(half, [10, 11, 12, 12]);
        // h1v2: the same column of two samples (stride 1), the parity is the ROW's (col 0 is always even).
        let h1v2: [i32; 4] = core::array::from_fn(|y| fancy(&c, 1, (1, 2), (0, 1), (y, 0), (1, 2)));
        assert_eq!(h1v2, [10, 11, 11, 12], "h1v2 by row");
        // Nothing subsampled: the sample, whatever the bias.
        assert_eq!(fancy(&c, 2, (2, 1), (0, 0), (0, 1), (8, 7)), 12);
    }

    #[test]
    fn yuvfancy_fancy_is_to_rgbas_fancy() {
        // to_rgba's Chroma::Fancy over a 7x5 4:2:0 gradient == Chroma::Nearest over full planes built by fancy(.., (8, 8)).
        let (w, h, cw, ch) = (7usize, 5usize, 4usize, 3usize);
        let y: [u8; 35] = core::array::from_fn(|i| ((i % w) * 31 + (i / w) * 17 + 16) as u8);
        let u: [u8; 12] = core::array::from_fn(|i| ((i % cw) * 53 + (i / cw) * 29 + 7) as u8);
        let v: [u8; 12] = core::array::from_fn(|i| (250 - (i % cw) * 41 - (i / cw) * 23) as u8);
        let p = Picture { w, h, y: &y, u: &u, v: &v, y_stride: w, uv_stride: cw, ss: (1, 1), mono: false, bit_depth: 8, full: false };
        let fu: [u8; 35] = core::array::from_fn(|i| fancy(&u, cw, (cw, ch), (1, 1), (i / w, i % w), (8, 8)) as u8);
        let fv: [u8; 35] = core::array::from_fn(|i| fancy(&v, cw, (cw, ch), (1, 1), (i / w, i % w), (8, 8)) as u8);
        let q = Picture { u: &fu, v: &fv, uv_stride: w, ss: (0, 0), ..p };
        for m in [Matrix::Kr(KR_KB_601), Matrix::Kr(KR_KB_709)] {
            let (mut a, mut b) = ([0u8; 35 * 4], [0u8; 35 * 4]);
            to_rgba(&p, m, Chroma::Fancy, false, &mut a);
            to_rgba(&q, m, Chroma::Nearest, false, &mut b);
            assert_eq!(a, b, "{m:?}");
        }
    }
}
