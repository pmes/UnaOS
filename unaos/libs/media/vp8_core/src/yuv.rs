// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! I420 → RGBA as WebP decoders present it: libwebp's BT.601 limited-range converter (its 14-bit `MultHi` per-product
//! fixed point) and its "fancy" (9-3-3-1, rounded to the sample first) chroma upsampling, so a lossy WebP lands on the
//! same RGB a browser shows. VP8COLOURONE (LEDGER SR356): both run in yuv_core's ONE loop, `Matrix::MultHi(WEBP)` with
//! `Chroma::Fancy`; this module keeps only libwebp's constants and the picture shape.

use alloc::vec;
use alloc::vec::Vec;

use yuv_core::{Chroma, Matrix, Picture};

use crate::Yuv420;

/// libwebp's `VP8YUVToR/G/B` (yuv.h): coefficients at 8 fractional bits, constants at the 6-bit (`YUV_FIX2`) sum.
pub const WEBP: yuv_core::MultHi = ([19077, 26149, 6419, 13320, 33050], [-14234, 8708, -17685]);

/// One pixel, Y'CbCr (limited range) → R'G'B'.
#[inline]
pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let (yp, up, vp) = ([y], [u], [v]);
    let p = Picture { w: 1, h: 1, y: &yp[..], u: &up[..], v: &vp[..], y_stride: 1, uv_stride: 1, ss: (0, 0), mono: false, bit_depth: 8, full: false };
    let mut out = [0u8; 4];
    yuv_core::to_rgba(&p, Matrix::MultHi(WEBP), Chroma::Nearest, false, &mut out);
    [out[0], out[1], out[2]]
}

/// The whole picture to straight RGBA (alpha 255), fancy-upsampled.
pub fn to_rgba(p: &Yuv420) -> Vec<u8> {
    let (w, h) = (p.width as usize, p.height as usize);
    let pic = Picture { w, h, y: &p.y[..], u: &p.u[..], v: &p.v[..], y_stride: w, uv_stride: p.chroma_width() as usize, ss: (1, 1), mono: false, bit_depth: 8, full: false };
    let mut out = vec![0u8; w * h * 4];
    yuv_core::to_rgba(&pic, Matrix::MultHi(WEBP), Chroma::Fancy, false, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_and_primaries() {
        assert_eq!(yuv_to_rgb(16, 128, 128), [0, 0, 0]);
        assert_eq!(yuv_to_rgb(235, 128, 128), [255, 255, 255]);
        let r = yuv_to_rgb(81, 90, 240);
        assert!(r[0] >= 253 && r[1] <= 2 && r[2] <= 2, "{r:?}");
    }

    #[test]
    fn flat_picture_stays_flat() {
        for (w, h) in [(1, 1), (2, 2), (3, 5), (16, 9)] {
            let p = Yuv420 {
                width: w,
                height: h,
                y: vec![100; (w * h) as usize],
                u: vec![60; (w.div_ceil(2) * h.div_ceil(2)) as usize],
                v: vec![200; (w.div_ceil(2) * h.div_ceil(2)) as usize],
            };
            let px = to_rgba(&p);
            let want = yuv_to_rgb(100, 60, 200);
            for c in px.chunks_exact(4) {
                assert_eq!(&c[..3], &want);
            }
        }
    }

    /// libwebp's yuv.h, verbatim: `MultHi` and `VP8Clip8` (the reference the retired converter was).
    fn libwebp(y: i32, u: i32, v: i32) -> [u8; 3] {
        let mh = |v: i32, c: i32| (v * c) >> 8;
        let clip8 = |v: i32| if v & !((256 << 6) - 1) == 0 { (v >> 6) as u8 } else if v < 0 { 0 } else { 255 };
        [
            clip8(mh(y, 19077) + mh(v, 26149) - 14234),
            clip8(mh(y, 19077) - mh(u, 6419) - mh(v, 13320) + 8708),
            clip8(mh(y, 19077) + mh(u, 33050) - 17685),
        ]
    }

    #[test]
    fn vp8colourone_libwebp_exhaustive() {
        // Every one of the 2^24 triples through yuv_core's MultHi arm (one 256x256 Cb x Cr picture per Y) is libwebp's.
        let (u, v): (Vec<u8>, Vec<u8>) = (0..65536u32).map(|i| ((i >> 8) as u8, i as u8)).unzip();
        let mut out = vec![0u8; 65536 * 4];
        for y in 0..=255u8 {
            let yp = vec![y; 65536];
            let p = Picture { w: 256, h: 256, y: &yp[..], u: &u[..], v: &v[..], y_stride: 256, uv_stride: 256, ss: (0, 0), mono: false, bit_depth: 8, full: false };
            yuv_core::to_rgba(&p, Matrix::MultHi(WEBP), Chroma::Nearest, false, &mut out);
            for i in 0..65536 {
                assert_eq!(out[i * 4..i * 4 + 3], libwebp(y as i32, u[i] as i32, v[i] as i32), "y {y} u {} v {}", u[i], v[i]);
            }
        }
    }

    #[test]
    fn vp8colourone_gradient_pins() {
        // Pinned from the PRE-VP8COLOURONE converter (libwebp's UpsampleRgbaLinePair, vp8_core's own line_pair): odd and
        // even sizes, so both edge rules are covered.
        let fnv = |b: &[u8]| b.iter().fold(0xcbf2_9ce4_8422_2325u64, |a, &x| (a ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3));
        for (w, h, want) in [(17usize, 13usize, 0xca00_ae15_f52f_3f26u64), (16, 12, 0x3c11_29f8_3a3a_ff83)] {
            let (cw, ch) = ((w + 1) / 2, (h + 1) / 2);
            let p = Yuv420 {
                width: w as u32,
                height: h as u32,
                y: (0..w * h).map(|i| (((i % w) * 15 + (i / w) * 7) % 256) as u8).collect(),
                u: (0..cw * ch).map(|i| (((i % cw) * 29 + (i / cw) * 11 + 3) % 256) as u8).collect(),
                v: (0..cw * ch).map(|i| ((255 + 256 * 4 - (i % cw) * 23 - (i / cw) * 17) % 256) as u8).collect(),
            };
            let o = to_rgba(&p);
            assert_eq!(fnv(&o), want, "{w}x{h}");
            assert_eq!((&o[12..15], &o[(5 * w + 8) * 4..(5 * w + 8) * 4 + 3]), (&[190u8, 0, 0][..], &[167u8, 156, 180][..]), "{w}x{h}");
        }
    }
}
