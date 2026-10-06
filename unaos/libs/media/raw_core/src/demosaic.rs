// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! From the mosaic to pixel_core's surface (straight RGBA8, sRGB). Two developers, one tone:
//!
//! * [`bilinear_rgba`] — full size: each pixel keeps its own channel and takes the other two as the mean of the
//!   same-colour samples in its 3x3 neighbourhood (for RGGB this is the textbook bilinear; it holds for any
//!   2x2 pattern). Row-strided, no threads (lux's rayon loop, single-core).
//! * [`Binner`] — for a window smaller than the sensor (`k >= 2`): each output pixel is the mean of every
//!   sample of each colour in its `k x k` block — any 2x2 window holds all three colours, so no interpolation
//!   is needed — fed one row at a time, holding one accumulator row (the kernel's streaming path).
//!
//! Means are taken in LINEAR light, then [`Tone`] maps black..white to sRGB. No white balance and no colour
//! matrix yet (owed): the image is the camera's native RGB, flat and greenish.

use alloc::vec::Vec;

use crate::color;
use crate::Error;

/// Raw sample -> sRGB byte, `black..=white` stretched to `0..=255` in linear light.
pub struct Tone {
    lut: Vec<u8>,
}

impl Tone {
    /// `white == 0` takes `max_code`; a black at or above the white is ignored.
    pub fn new(black: u16, white: u16, max_code: u16) -> Result<Self, Error> {
        let white = if white == 0 { max_code } else { white }.max(1);
        let black = if black >= white { 0 } else { black };
        let mut lut = crate::zeroed::<u8>(white as usize + 1)?;
        let t = color::thresholds();
        let span = (white - black) as f64;
        let mut code = 0usize;
        for v in black as usize..=white as usize {
            let lin = (v - black as usize) as f64 / span;
            while code < 255 && t[code] <= lin {
                code += 1;
            }
            lut[v] = code as u8;
        }
        Ok(Tone { lut })
    }
    #[inline]
    pub fn map(&self, v: u32) -> u8 {
        let i = (v as usize).min(self.lut.len() - 1);
        self.lut[i]
    }
}

/// The colour (0 R, 1 G, 2 B) at `(x, y)` of a 2x2 pattern.
#[inline]
pub fn colour_at(cfa: &[u8; 4], x: usize, y: usize) -> usize {
    cfa[(y & 1) * 2 + (x & 1)] as usize
}

/// Full-size bilinear demosaic of a `w x h` mosaic to RGBA8.
pub fn bilinear_rgba(m: &[u16], w: usize, h: usize, cfa: &[u8; 4], tone: &Tone) -> Result<Vec<u8>, Error> {
    if m.len() < w * h || w == 0 || h == 0 {
        return Err(Error::Truncated);
    }
    let mut out = crate::zeroed::<u8>(w * h * 4)?;
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let own = colour_at(cfa, x, y);
            let mut sum = [0u32; 3];
            let mut cnt = [0u32; 3];
            for yy in y0..=y1 {
                for xx in x0..=x1 {
                    let c = colour_at(cfa, xx, yy);
                    if c == own && (xx != x || yy != y) {
                        continue;
                    }
                    sum[c] += m[yy * w + xx] as u32;
                    cnt[c] += 1;
                }
            }
            let o = (y * w + x) * 4;
            for c in 0..3 {
                let v = if c == own { m[y * w + x] as u32 } else if cnt[c] > 0 { (sum[c] + cnt[c] / 2) / cnt[c] } else { 0 };
                out[o + c] = tone.map(v);
            }
            out[o + 3] = 255;
        }
    }
    Ok(out)
}

/// Streaming `k x k` binning developer: feed rows 0.. in order, read [`Binner::rgba`] at the end.
pub struct Binner {
    k: usize,
    ow: usize,
    oh: usize,
    cfa: [u8; 4],
    acc: Vec<[u64; 3]>,
    cnt: Vec<[u32; 3]>,
    /// `ow * oh * 4` straight RGBA.
    pub rgba: Vec<u8>,
}

impl Binner {
    /// Output `ow x oh`, each pixel a `k x k` block (`k >= 1`; `k == 1` takes each sample's own colour only —
    /// use [`bilinear_rgba`] for a full-size image).
    pub fn new(k: usize, ow: usize, oh: usize, cfa: [u8; 4]) -> Result<Self, Error> {
        if k == 0 || ow == 0 || oh == 0 {
            return Err(Error::Malformed("binner geometry"));
        }
        Ok(Binner { k, ow, oh, cfa, acc: crate::zeroed(ow)?, cnt: crate::zeroed(ow)?, rgba: crate::zeroed(ow.checked_mul(oh).ok_or(Error::TooLarge)?.checked_mul(4).ok_or(Error::TooLarge)?)? })
    }

    /// Accumulate mosaic row `y`.
    pub fn row(&mut self, y: usize, r: &[u16], tone: &Tone) {
        let oy = y / self.k;
        if oy >= self.oh {
            return;
        }
        let xs = r.len().min(self.ow * self.k);
        for (x, &v) in r[..xs].iter().enumerate() {
            let c = colour_at(&self.cfa, x, y);
            let ox = x / self.k;
            self.acc[ox][c] += v as u64;
            self.cnt[ox][c] += 1;
        }
        if y % self.k == self.k - 1 {
            self.flush(oy, tone);
        }
    }

    fn flush(&mut self, oy: usize, tone: &Tone) {
        for ox in 0..self.ow {
            let o = (oy * self.ow + ox) * 4;
            for c in 0..3 {
                let n = self.cnt[ox][c] as u64;
                let v = if n > 0 { (self.acc[ox][c] + n / 2) / n } else { 0 };
                self.rgba[o + c] = tone.map(v as u32);
            }
            self.rgba[o + 3] = 255;
            self.acc[ox] = [0; 3];
            self.cnt[ox] = [0; 3];
        }
    }
}
