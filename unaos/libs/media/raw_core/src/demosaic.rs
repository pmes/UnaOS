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
//! Means are taken in LINEAR light, then a [`ToneMap`] maps black..white to bytes: [`Tone`] (sRGB, no white
//! balance — Facet's FULL path, the camera's native RGB, flat and greenish) or [`WbTone`] (RAWVIEW, B711: the
//! as-shot white balance per channel, then a pure 2.2 gamma). [`half_rgba`] is the half-size demosaic: the
//! [`Binner`] at `k = 2`, each 2x2 CFA cell one RGB pixel (R and B their own sample, G the mean of the two), so a
//! raw with no embedded preview still shows. RAWCOLOR (B724): [`WbTone::with_matrix`] puts the colour matrix
//! ([`crate::matrix`], camera RGB -> linear sRGB, Q12) between the white balance and the gamma — a whole-pixel map
//! ([`ToneMap::map_px`]), which the [`Binner`] calls once per output pixel.

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

/// A raw sample of colour `c` (0 R, 1 G, 2 B) -> a display byte.
pub trait ToneMap {
    fn map_c(&self, c: usize, v: u32) -> u8;
    /// One pixel's three raw means -> RGB bytes (per channel unless the map mixes channels).
    #[inline]
    fn map_px(&self, v: [u32; 3]) -> [u8; 3] {
        [self.map_c(0, v[0]), self.map_c(1, v[1]), self.map_c(2, v[2])]
    }
}

impl ToneMap for Tone {
    #[inline]
    fn map_c(&self, _c: usize, v: u32) -> u8 {
        self.map(v)
    }
}

/// White-balanced tone (RAWVIEW, B711): channel `c`'s `black..=white` stretched to `0..1` in linear light, times
/// its gain (the [`crate::Wb`] multipliers normalised so the SMALLEST is one — highlights clip, as dcraw's
/// do), then encoded by a pure 2.2 power curve. One table per channel, `white + 1` entries each.
pub struct WbTone {
    lut: [Vec<u8>; 3],
    /// The gains applied, Q10, smallest = 1024 (the wire's and the KAT's).
    pub gain: [u32; 3],
    /// RAWCOLOR (B724): the matrix path — per channel the white-balanced linear value in Q16 (`black..white` x gain),
    /// the Q12 matrix, the 2.2 encoder over Q16.
    mat: Option<Mat>,
}

struct Mat {
    m: [[i32; 3]; 3],
    lin: [Vec<u32>; 3],
    enc: Vec<u8>,
}

impl WbTone {
    /// `white == 0` takes `max_code`; `wb == None` is unity gains.
    pub fn new(black: u16, white: u16, max_code: u16, wb: Option<&crate::Wb>) -> Result<Self, Error> {
        Self::with_matrix(black, white, max_code, wb, None)
    }

    /// [`new`](Self::new) with the colour matrix (`None`: the camera's native RGB, B711's output to the byte).
    pub fn with_matrix(black: u16, white: u16, max_code: u16, wb: Option<&crate::Wb>, matrix: Option<&[[i32; 3]; 3]>) -> Result<Self, Error> {
        let white = if white == 0 { max_code } else { white }.max(1);
        let black = if black >= white { 0 } else { black };
        let mul = wb.map(|w| w.mul).unwrap_or([crate::wb::ONE; 3]);
        let lo = mul.iter().copied().min().unwrap_or(crate::wb::ONE).max(1) as u64;
        let gain = mul.map(|m| ((m as u64 * crate::wb::ONE as u64 + lo / 2) / lo) as u32);
        let t = color::gamma22_thresholds();
        let span = (white - black) as f64;
        let mut lut = [crate::zeroed::<u8>(white as usize + 1)?, crate::zeroed::<u8>(white as usize + 1)?, crate::zeroed::<u8>(white as usize + 1)?];
        for (c, l) in lut.iter_mut().enumerate() {
            let g = mul[c] as f64 / lo as f64;
            let mut code = 0usize;
            for v in black as usize..=white as usize {
                // (v - black) * g first: an exact product when the gain is a short binary fraction (the KAT's
                // grey cell lands every channel on the same code).
                let lin = ((v - black as usize) as f64 * g) / span;
                while code < 255 && t[code] <= lin {
                    code += 1;
                }
                l[v] = code as u8;
            }
        }
        let mat = match matrix {
            None => None,
            Some(m) => {
                let n = white as usize + 1;
                let mut lin = [crate::zeroed::<u32>(n)?, crate::zeroed::<u32>(n)?, crate::zeroed::<u32>(n)?];
                let spanq = (white - black) as u64 * crate::wb::ONE as u64;
                for (c, l) in lin.iter_mut().enumerate() {
                    for v in black as usize..n {
                        l[v] = ((v - black as usize) as u64 * gain[c] as u64 * 65535 / spanq) as u32;
                    }
                }
                let mut enc = crate::zeroed::<u8>(65536)?;
                let mut code = 0usize;
                for (x, e) in enc.iter_mut().enumerate() {
                    let lv = x as f64 / 65535.0;
                    while code < 255 && t[code] <= lv {
                        code += 1;
                    }
                    *e = code as u8;
                }
                Some(Mat { m: *m, lin, enc })
            }
        };
        Ok(WbTone { lut, gain, mat })
    }
}

impl ToneMap for WbTone {
    #[inline]
    fn map_c(&self, c: usize, v: u32) -> u8 {
        let l = &self.lut[c.min(2)];
        l[(v as usize).min(l.len() - 1)]
    }

    #[inline]
    fn map_px(&self, v: [u32; 3]) -> [u8; 3] {
        let Some(m) = &self.mat else { return [self.map_c(0, v[0]), self.map_c(1, v[1]), self.map_c(2, v[2])] };
        let cam = [0, 1, 2].map(|c| m.lin[c][(v[c] as usize).min(m.lin[c].len() - 1)]);
        crate::matrix::apply(&m.m, cam).map(|x| m.enc[x as usize])
    }
}

/// The half-size demosaic of a `w x h` mosaic: `(rgba, w / 2, h / 2)`, each 2x2 CFA cell one pixel (an odd last
/// row or column is dropped). Bounds-checked; the output is reserved fallibly.
pub fn half_rgba<T: ToneMap>(m: &[u16], w: usize, h: usize, cfa: &[u8; 4], tone: &T) -> Result<(Vec<u8>, usize, usize), Error> {
    let (ow, oh) = (w / 2, h / 2);
    if ow == 0 || oh == 0 {
        return Err(Error::Malformed("mosaic below one 2x2 cell"));
    }
    if m.len() < w.checked_mul(h).ok_or(Error::TooLarge)? {
        return Err(Error::Truncated);
    }
    let mut bin = Binner::new(2, ow, oh, *cfa)?;
    for y in 0..oh * 2 {
        bin.row(y, &m[y * w..(y + 1) * w], tone);
    }
    Ok((bin.rgba, ow, oh))
}

/// The colour (0 R, 1 G, 2 B) at `(x, y)` of a 2x2 pattern.
#[inline]
pub fn colour_at(cfa: &[u8; 4], x: usize, y: usize) -> usize {
    cfa[(y & 1) * 2 + (x & 1)] as usize
}

/// The bilinear estimate of all three channels at `(x, y)`.
#[inline]
pub fn bilinear_at(m: &[u16], w: usize, h: usize, cfa: &[u8; 4], x: usize, y: usize) -> [u32; 3] {
    let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
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
    let mut v = [0u32; 3];
    for c in 0..3 {
        v[c] = if c == own { m[y * w + x] as u32 } else if cnt[c] > 0 { (sum[c] + cnt[c] / 2) / cnt[c] } else { 0 };
    }
    v
}

/// Full-size bilinear demosaic of a `w x h` mosaic to RGBA8.
pub fn bilinear_rgba(m: &[u16], w: usize, h: usize, cfa: &[u8; 4], tone: &Tone) -> Result<Vec<u8>, Error> {
    if m.len() < w * h || w == 0 || h == 0 {
        return Err(Error::Truncated);
    }
    let mut out = crate::zeroed::<u8>(w * h * 4)?;
    for y in 0..h {
        for x in 0..w {
            let v = bilinear_at(m, w, h, cfa, x, y);
            let o = (y * w + x) * 4;
            for c in 0..3 {
                out[o + c] = tone.map(v[c]);
            }
            out[o + 3] = 255;
        }
    }
    Ok(out)
}

/// Full-size bilinear demosaic to LINEAR `f32` RGB, `black..white` -> `0.0..1.0` unclamped above white (lux's
/// `RgbBuffer` contract).
pub fn bilinear_linear(m: &[u16], w: usize, h: usize, cfa: &[u8; 4], black: u16, white: u16) -> Result<Vec<f32>, Error> {
    if m.len() < w * h || w == 0 || h == 0 {
        return Err(Error::Truncated);
    }
    let span = (white.saturating_sub(black)).max(1) as f32;
    let mut out = crate::zeroed::<f32>(w * h * 3)?;
    for y in 0..h {
        for x in 0..w {
            let v = bilinear_at(m, w, h, cfa, x, y);
            for c in 0..3 {
                out[(y * w + x) * 3 + c] = v[c].saturating_sub(black as u32) as f32 / span;
            }
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
    pub fn row<T: ToneMap>(&mut self, y: usize, r: &[u16], tone: &T) {
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

    fn flush<T: ToneMap>(&mut self, oy: usize, tone: &T) {
        for ox in 0..self.ow {
            let o = (oy * self.ow + ox) * 4;
            let mut v = [0u32; 3];
            for (c, vc) in v.iter_mut().enumerate() {
                let n = self.cnt[ox][c] as u64;
                *vc = if n > 0 { ((self.acc[ox][c] + n / 2) / n) as u32 } else { 0 };
            }
            self.rgba[o..o + 3].copy_from_slice(&tone.map_px(v));
            self.rgba[o + 3] = 255;
            self.acc[ox] = [0; 3];
            self.cnt[ox] = [0; 3];
        }
    }
}
