// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The pixel operations Facet owns: orientation, quarter turns, flips, crop, the documented resize
//! filter, and CSS-semantics brightness/contrast. All on straight 8-bit RGBA, row-major, top-down.

/// A straight (non-premultiplied) 8-bit RGBA picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Raster {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        debug_assert_eq!(rgba.len(), width as usize * height as usize * 4);
        Raster { width, height, rgba }
    }

    pub fn filled(width: u32, height: u32, px: [u8; 4]) -> Self {
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..width as usize * height as usize {
            rgba.extend_from_slice(&px);
        }
        Raster { width, height, rgba }
    }

    pub fn px(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    /// Remap every pixel: stored `(x, y)` lands at `f(x, y, w, h)` in an `ow x oh` output.
    fn remap(&self, ow: u32, oh: u32, f: impl Fn(usize, usize, usize, usize) -> (usize, usize)) -> Raster {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![0u8; self.rgba.len()];
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = f(x, y, w, h);
                let s = (y * w + x) * 4;
                let d = (dy * ow as usize + dx) * 4;
                out[d..d + 4].copy_from_slice(&self.rgba[s..s + 4]);
            }
        }
        Raster { width: ow, height: oh, rgba: out }
    }

    /// Display the picture upright per its EXIF orientation (EXIF 2.3 §4.6.4 A, tag 0x0112) —
    /// the same mapping as PIXELCORE's `Image::apply_orientation` and CSS `image-orientation:
    /// from-image`. 5..=8 swap width and height; anything outside 2..=8 is the identity.
    pub fn oriented(&self, o: u8) -> Raster {
        let (w, h) = (self.width, self.height);
        match o {
            2 => self.remap(w, h, |x, y, w, _| (w - 1 - x, y)),
            3 => self.remap(w, h, |x, y, w, h| (w - 1 - x, h - 1 - y)),
            4 => self.remap(w, h, |x, y, _, h| (x, h - 1 - y)),
            5 => self.remap(h, w, |x, y, _, _| (y, x)),
            6 => self.remap(h, w, |x, y, _, h| (h - 1 - y, x)),
            7 => self.remap(h, w, |x, y, w, h| (h - 1 - y, w - 1 - x)),
            8 => self.remap(h, w, |x, y, w, _| (y, w - 1 - x)),
            _ => self.clone(),
        }
    }

    /// Rotate clockwise by `quarter_turns` x 90 degrees.
    pub fn rotated(&self, quarter_turns: u8) -> Raster {
        match quarter_turns % 4 {
            1 => self.oriented(6),
            2 => self.oriented(3),
            3 => self.oriented(8),
            _ => self.clone(),
        }
    }

    /// Mirror left/right (`horizontal`) or top/bottom.
    pub fn flipped(&self, horizontal: bool) -> Raster {
        self.oriented(if horizontal { 2 } else { 4 })
    }

    /// The `width x height` window at `(x, y)`. The caller has bounds-checked it.
    pub fn cropped(&self, x: u32, y: u32, width: u32, height: u32) -> Raster {
        let mut out = Vec::with_capacity(width as usize * height as usize * 4);
        for row in y..y + height {
            let s = (row as usize * self.width as usize + x as usize) * 4;
            out.extend_from_slice(&self.rgba[s..s + width as usize * 4]);
        }
        Raster { width, height, rgba: out }
    }

    /// Resample to exactly `dw x dh` with Facet's TRIANGLE filter.
    ///
    /// The filter, so any implementation can reproduce it: separable (horizontal pass, then
    /// vertical), a tent kernel `k(t) = max(0, 1 - |t|)` stretched by `f = max(1, s)` where `s =
    /// src/dst` on that axis (so minification averages every source pixel under the footprint and
    /// magnification is plain bilinear); destination sample `i` is centred on source coordinate
    /// `c = (i + 0.5) * s` and source pixel `j` on `j + 0.5`, weight `k((j + 0.5 - c) / f)`; weights
    /// that fall outside the image are dropped and the rest renormalised to sum 1 (edge clamp by
    /// renormalisation). Samples are filtered as alpha-PREMULTIPLIED sRGB-encoded values in f32
    /// (no fringe from transparent pixels), un-premultiplied at the end, and rounded half away from
    /// zero to 8 bits. The intermediate after the horizontal pass stays f32 (no double rounding).
    /// This is Pillow's `BILINEAR` and the `image` crate's `FilterType::Triangle` definition.
    pub fn resized(&self, dw: u32, dh: u32) -> Raster {
        let (sw, sh) = (self.width as usize, self.height as usize);
        let (dw_, dh_) = (dw as usize, dh as usize);
        // Premultiply into f32.
        let mut src = vec![0f32; sw * sh * 4];
        for (i, p) in self.rgba.chunks_exact(4).enumerate() {
            let a = p[3] as f32 / 255.0;
            src[i * 4] = p[0] as f32 * a;
            src[i * 4 + 1] = p[1] as f32 * a;
            src[i * 4 + 2] = p[2] as f32 * a;
            src[i * 4 + 3] = p[3] as f32;
        }
        let hw = weights(sw, dw_);
        let mut mid = vec![0f32; dw_ * sh * 4];
        for y in 0..sh {
            for (x, (lo, ws)) in hw.iter().enumerate() {
                let mut acc = [0f32; 4];
                for (k, w) in ws.iter().enumerate() {
                    let s = (y * sw + lo + k) * 4;
                    for c in 0..4 {
                        acc[c] += src[s + c] * w;
                    }
                }
                mid[(y * dw_ + x) * 4..(y * dw_ + x) * 4 + 4].copy_from_slice(&acc);
            }
        }
        let vw = weights(sh, dh_);
        let mut out = vec![0u8; dw_ * dh_ * 4];
        for (y, (lo, ws)) in vw.iter().enumerate() {
            for x in 0..dw_ {
                let mut acc = [0f32; 4];
                for (k, w) in ws.iter().enumerate() {
                    let s = ((lo + k) * dw_ + x) * 4;
                    for c in 0..4 {
                        acc[c] += mid[s + c] * w;
                    }
                }
                let a = acc[3].clamp(0.0, 255.0);
                let d = (y * dw_ + x) * 4;
                if a > 0.0 {
                    let inv = 255.0 / a;
                    for c in 0..3 {
                        out[d + c] = (acc[c] * inv).round().clamp(0.0, 255.0) as u8;
                    }
                }
                out[d + 3] = a.round() as u8;
            }
        }
        Raster { width: dw, height: dh, rgba: out }
    }

    /// CSS Filter Effects 1 `brightness(b) contrast(c)`, in that order, per channel on the straight
    /// sRGB-encoded value `v` in [0, 1], alpha untouched: brightness `v' = clamp(b * v)` (§13.2
    /// brightness: feComponentTransfer linear, slope b, intercept 0); contrast `v'' = clamp(c * v' +
    /// 0.5 - 0.5 * c)` (§13.2 contrast: slope c, intercept -(0.5 * c) + 0.5). Each step clamps, as
    /// feComponentTransfer does. Rounded half away from zero to 8 bits. 1.0 / 1.0 is the identity.
    pub fn adjusted(&self, brightness: f32, contrast: f32) -> Raster {
        let mut lut = [0u8; 256];
        for (i, slot) in lut.iter_mut().enumerate() {
            let (b, c) = (brightness as f64, contrast as f64);
            let v = i as f64 / 255.0;
            let v = (v * b).clamp(0.0, 1.0);
            let v = (c * v + 0.5 - 0.5 * c).clamp(0.0, 1.0);
            *slot = (v * 255.0).round() as u8;
        }
        let mut out = self.clone();
        for p in out.rgba.chunks_exact_mut(4) {
            p[0] = lut[p[0] as usize];
            p[1] = lut[p[1] as usize];
            p[2] = lut[p[2] as usize];
        }
        out
    }
}

/// Per destination sample: the first contributing source index and the normalised weights.
fn weights(src: usize, dst: usize) -> Vec<(usize, Vec<f32>)> {
    let s = src as f64 / dst as f64;
    let f = s.max(1.0);
    (0..dst)
        .map(|i| {
            let c = (i as f64 + 0.5) * s;
            let lo = ((c - f).floor().max(0.0)) as usize;
            let hi = ((c + f).ceil() as usize).min(src);
            let mut ws: Vec<f32> = (lo..hi)
                .map(|j| (1.0 - ((j as f64 + 0.5 - c) / f).abs()).max(0.0) as f32)
                .collect();
            // Trim zero-weight ends so `lo` is the first real contributor.
            let first = ws.iter().position(|&w| w > 0.0).unwrap_or(0);
            let last = ws.iter().rposition(|&w| w > 0.0).map(|p| p + 1).unwrap_or(ws.len().max(1));
            ws = ws[first..last.max(first + 1).min(ws.len())].to_vec();
            if ws.is_empty() {
                ws.push(1.0);
            }
            let sum: f32 = ws.iter().sum();
            if sum > 0.0 {
                for w in ws.iter_mut() {
                    *w /= sum;
                }
            }
            ((lo + first).min(src - 1), ws)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3x2, pixel value = its index, so every mapping is legible.
    fn r32() -> Raster {
        let mut v = Vec::new();
        for i in 0..6u8 {
            v.extend([i, i, i, 255]);
        }
        Raster::new(3, 2, v)
    }
    fn ids(r: &Raster) -> Vec<u8> {
        r.rgba.chunks_exact(4).map(|p| p[0]).collect()
    }

    // Stored 3x2:   0 1 2
    //               3 4 5
    #[test]
    fn kat_orientations() {
        let r = r32();
        assert_eq!(ids(&r.oriented(1)), [0, 1, 2, 3, 4, 5]);
        assert_eq!(ids(&r.oriented(2)), [2, 1, 0, 5, 4, 3]);
        assert_eq!(ids(&r.oriented(3)), [5, 4, 3, 2, 1, 0]);
        assert_eq!(ids(&r.oriented(4)), [3, 4, 5, 0, 1, 2]);
        let o5 = r.oriented(5);
        assert_eq!((o5.width, o5.height), (2, 3));
        assert_eq!(ids(&o5), [0, 3, 1, 4, 2, 5]);
        assert_eq!(ids(&r.oriented(6)), [3, 0, 4, 1, 5, 2]);
        assert_eq!(ids(&r.oriented(7)), [5, 2, 4, 1, 3, 0]);
        assert_eq!(ids(&r.oriented(8)), [2, 5, 1, 4, 0, 3]);
    }

    #[test]
    fn kat_rotate_flip_crop() {
        let r = r32();
        assert_eq!(ids(&r.rotated(1)), [3, 0, 4, 1, 5, 2]);
        assert_eq!(r.rotated(4), r);
        assert_eq!(r.rotated(1).rotated(3), r);
        assert_eq!(r.rotated(2), r.flipped(true).flipped(false));
        assert_eq!(ids(&r.cropped(1, 0, 2, 2)), [1, 2, 4, 5]);
        assert_eq!(ids(&r.cropped(0, 1, 3, 1)), [3, 4, 5]);
    }

    #[test]
    fn kat_resize_triangle() {
        // 2x1 [0, 200] -> 1x1: footprint covers both equally -> 100.
        let r = Raster::new(2, 1, vec![0, 0, 0, 255, 200, 200, 200, 255]);
        assert_eq!(r.resized(1, 1).px(0, 0), [100, 100, 100, 255]);
        // 2x1 [0, 200] -> 4x1 bilinear: centres at 0.25, 0.75, 1.25, 1.75 source px:
        // first and last clamp (renormalised) to 0 and 200, inner ones 50 and 150.
        let up = r.resized(4, 1);
        assert_eq!(ids(&up), [0, 50, 150, 200]);
        // Identity size is the identity.
        let id = r32();
        assert_eq!(id.resized(3, 2), id);
        // Premultiplied: a transparent red pixel does not tint its opaque neighbour's average.
        let t = Raster::new(2, 1, vec![255, 0, 0, 0, 0, 0, 255, 255]);
        assert_eq!(t.resized(1, 1).px(0, 0), [0, 0, 255, 128]);
    }

    #[test]
    fn kat_adjust_css() {
        let r = Raster::new(3, 1, vec![0, 0, 0, 10, 128, 128, 128, 20, 255, 255, 255, 30]);
        assert_eq!(r.adjusted(1.0, 1.0), r);
        // brightness(0.5): 128 -> 64, 255 -> 128 (127.5 rounds away from zero); alpha untouched.
        assert_eq!(ids(&r.adjusted(0.5, 1.0)), [0, 64, 128]);
        assert_eq!(r.adjusted(0.5, 1.0).px(2, 0)[3], 30);
        // contrast(0): everything is mid-grey 0.5 -> 128.
        assert_eq!(ids(&r.adjusted(1.0, 0.0)), [128, 128, 128]);
        // contrast(1.5): 0 -> clamp(-0.25) = 0; 128 -> 1.5*128 - 63.75 = 128.25 -> 128; 255 clamps.
        assert_eq!(ids(&r.adjusted(1.0, 1.5)), [0, 128, 255]);
        // ... and 200 -> 300 - 63.75 = 236.25 -> 236.
        assert_eq!(Raster::new(1, 1, vec![200, 0, 0, 255]).adjusted(1.0, 1.5).px(0, 0), [236, 0, 0, 255]);
        // brightness(2) then contrast(0.5): 128 -> 1.0 (clamped) -> 0.75 -> 191.25 -> 191.
        assert_eq!(ids(&r.adjusted(2.0, 0.5))[1], 191);
    }
}
