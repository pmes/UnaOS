// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RGBA -> I420 (NLE5B, LEDGER SR206): the tree's ONE forward colour converter, the encoder's input side.
//!
//! CHARTER: Stria (A/V) — seam `shared-core`: the export's RGBA frames become the [`Yuv420`] the encoder codes; the
//! inverse (I420 -> RGBA) is gneiss_pal's `dsp::video::Frame::to_rgba`.
//!
//! ITU-R BT.601, limited range (Y' 16..235, Cb / Cr 16..240), in 16-bit fixed point (the 8-bit 66 / 129 / 25 table
//! puts pure red at Y' 82, not 81). Chroma is sited at the 2x2 block and computed from the block's RGB sums, so it is
//! the average of the block; an edge block of an odd-sized frame averages the pixels it has. Each chroma row of
//! coefficients sums to zero, so any grey has Cb = Cr = 128 exactly. Alpha is ignored.

use alloc::vec;
use alloc::vec::Vec;

use crate::{Error, Result, Yuv420};

/// Y' coefficients (x 65536) of R, G, B.
const KY: [i64; 3] = [16829, 33039, 6416];
/// Cb coefficients (x 65536); they sum to 0.
const KU: [i64; 3] = [-9714, -19070, 28784];
/// Cr coefficients (x 65536); they sum to 0.
const KV: [i64; 3] = [28784, -24103, -4681];

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
        let (w, h) = (width as usize, height as usize);
        if rgba.len() < w * h * 4 {
            return Err(Error::FrameSize);
        }
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut y = vec![0u8; w * h];
        let mut u = vec![128u8; cw * ch];
        let mut v = vec![128u8; cw * ch];
        for row in 0..h {
            for col in 0..w {
                let p = &rgba[(row * w + col) * 4..];
                let s = KY[0] * p[0] as i64 + KY[1] * p[1] as i64 + KY[2] * p[2] as i64;
                y[row * w + col] = (16 + ((s + 32768) >> 16)) as u8;
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
                let chroma = |k: &[i64; 3]| {
                    let s = k[0] * sum[0] + k[1] * sum[1] + k[2] * sum[2];
                    (128 + (s + n * 32768).div_euclid(n * 65536)) as u8
                };
                u[by * cw + bx] = chroma(&KU);
                v[by * cw + bx] = chroma(&KV);
            }
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
