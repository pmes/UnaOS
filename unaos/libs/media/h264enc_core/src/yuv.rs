// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RGBA -> I420 (NLE5B, LEDGER SR206): the encoder's input side, over the tree's ONE forward colour converter.
//!
//! CHARTER: Stria (A/V) — seam `shared-core`: the export's RGBA frames become the [`Yuv420`] the encoder codes; the
//! inverse (I420 -> RGBA) is gneiss_pal's `dsp::video::Frame::to_rgba`.
//!
//! H264ENCPLANES (rmbp-ledger B973): the conversion is `yuv_core::from_rgba` (beside `yuv_core::to_rgba`, both directions in
//! one crate); this file's own 16-bit BT.601 table and loop were retired onto it byte for byte (`yuv_core::rgb_coeffs` of
//! `KR_KB_601` limited is the retired table, pinned there). [`I420::from_rgba`] keeps its meaning: ITU-R BT.601, limited range
//! (Y' 16..235, Cb / Cr 16..240), chroma the 2x2 block average (an edge block of an odd-sized frame averages the pixels it
//! has), any grey Cb = Cr = 128, alpha ignored; [`I420::from_rgba_in`] takes the matrix and range.

use alloc::vec;
use alloc::vec::Vec;

use crate::{Error, Result, Yuv420};

/// One owned 8-bit 4:2:0 frame: Y `width x height`, Cb and Cr `ceil(width/2) x ceil(height/2)`, row-major, no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I420 {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl I420 {
    /// Convert straight RGBA (`width * height * 4` bytes, top row first) by BT.601 limited range.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<I420> {
        I420::from_rgba_in(width, height, rgba, yuv_core::KR_KB_601, false)
    }

    /// Convert straight RGBA by one (Kr, Kb) x 10000 and range (H264ENCPLANES B973: `yuv_core::from_rgba`, the ONE converter).
    pub fn from_rgba_in(width: u32, height: u32, rgba: &[u8], kr_kb: (i64, i64), full: bool) -> Result<I420> {
        let (w, h) = (width as usize, height as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut y = vec![0u8; w * h];
        let mut u = vec![128u8; cw * ch];
        let mut v = vec![128u8; cw * ch];
        if !yuv_core::from_rgba(rgba, w, h, kr_kb, full, &mut y, &mut u, &mut v) {
            return Err(Error::FrameSize);
        }
        Ok(I420 { width, height, y, u, v })
    }

    /// The borrowed form [`crate::Encoder::encode`] takes.
    pub fn as_yuv420(&self) -> Yuv420<'_> {
        Yuv420 {
            width: self.width,
            height: self.height,
            y: &self.y,
            u: &self.u,
            v: &self.v,
            y_stride: self.width as usize,
            uv_stride: (self.width as usize).div_ceil(2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        let f = I420::from_rgba(2, 2, &[r, g, b, 255].repeat(4)).unwrap();
        assert!(f.y.iter().all(|&x| x == f.y[0]));
        (f.y[0], f.u[0], f.v[0])
    }

    #[test]
    fn bt601_limited_kat() {
        // Hand-computed: white 16 + 219 = 235; black 16; red 16 + floor(4324163 / 65536) = 81,
        // Cb 128 + floor(-2444302 / 65536) = 90, Cr 128 + floor(7372688 / 65536) = 240.
        assert_eq!(one(255, 255, 255), (235, 128, 128));
        assert_eq!(one(0, 0, 0), (16, 128, 128));
        assert_eq!(one(255, 0, 0), (81, 90, 240));
        assert_eq!(one(128, 128, 128), (126, 128, 128));
    }

    #[test]
    fn chroma_is_the_block_average() {
        // Top row red, bottom row black: Y 81 81 / 16 16; Cb 128 + floor(-4823068 / 262144) = 109,
        // Cr 128 + floor(14810912 / 262144) = 184.
        let mut px = [255, 0, 0, 255].repeat(2);
        px.extend([0, 0, 0, 255].repeat(2));
        let f = I420::from_rgba(2, 2, &px).unwrap();
        assert_eq!(f.y, vec![81, 81, 16, 16]);
        assert_eq!((f.u[0], f.v[0]), (109, 184));
    }

    #[test]
    fn odd_size_edges_and_short_buffer() {
        let f = I420::from_rgba(3, 3, &[255, 0, 0, 255].repeat(9)).unwrap();
        assert_eq!((f.y.len(), f.u.len(), f.v.len()), (9, 4, 4));
        assert!(f.u.iter().all(|&c| c == 90) && f.v.iter().all(|&c| c == 240), "edge blocks average the pixels they have");
        let yuv = f.as_yuv420();
        assert_eq!((yuv.y_stride, yuv.uv_stride), (3, 2));
        assert_eq!(I420::from_rgba(2, 2, &[0; 15]), Err(Error::FrameSize));
    }
}
