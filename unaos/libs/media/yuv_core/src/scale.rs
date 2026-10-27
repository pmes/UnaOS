// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CHARTER: Stria (CODEX §2 A/V), seam shared-core — the ONE nearest resampler (letterbox geometry + per-row 4:4:4 gather) user-stria and the kernel Player sample with.
//! `yuv_core::scale` — the ONE nearest resampler (KERNELSCALEONE, rmbp-ledger B965).
//!
//! USERSTRIACOLOURONE (SR363) wrote it in user-stria's `media.rs` as `fit` / `fits` / `nearest_row`; the kernel Player's
//! `video/vplay.rs` `blit_fit` carried the same letterbox and the same indices inline. Both moved here, the arithmetic
//! unchanged: [`fit`] is the letterbox (aspect kept, centred), [`nearest_rows`] hands each output row as a 1-high 4:4:4
//! [`Picture`] (each output pixel's sample and the chroma sample covering it: what `Chroma::Nearest` reads at that luma
//! position), so scale-then-[`crate::to_rgba`] is bit for bit full-size `to_rgba` (Nearest) then nearest-sampled. A packed
//! RGBA frame samples through it as a mono `Picture<u32>`. No alloc: the caller owns the three `dw`-sample scratch rows.

use crate::Picture;

/// A `w x h` rect at `(x, y)` inside its box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub w: usize,
    pub h: usize,
    pub x: usize,
    pub y: usize,
}

/// The letterbox: the rect of a `sw x sh` picture inside a `dw x dh` box, aspect kept, centred. All four are > 0.
pub fn fit(sw: usize, sh: usize, dw: usize, dh: usize) -> Rect {
    let (w, h) = if dw * sh <= dh * sw { (dw, (sh * dw / sw).clamp(1, dh)) } else { ((sw * dh / sh).clamp(1, dw), dh) };
    Rect { w, h, x: (dw - w) / 2, y: (dh - h) / 2 }
}

/// Every sample [`nearest_rows`] can read lies inside the planes (`p.w`, `p.h` > 0).
pub fn fits<T>(p: &Picture<'_, T>) -> bool {
    let (cw, ch) = ((p.w + p.ss.0 as usize) >> p.ss.0, (p.h + p.ss.1 as usize) >> p.ss.1);
    p.y.len() >= (p.h - 1) * p.y_stride + p.w && (p.mono || p.u.len().min(p.v.len()) >= (ch - 1) * p.uv_stride + cw)
}

/// THE SCALER (nearest): `p` sampled to `dw x dh`, output row `r` from source row `(r * h / dh)`, column `c` from
/// `(c * w / dw)`, each handed to `row(r, &picture)` as a 1-high 4:4:4 picture of `dw` samples (no chroma when `mono`),
/// `p`'s depth, range and mono. `ry` (and, unless `mono`, `ru` / `rv`) hold at least `dw` samples; `fits(p)`.
pub fn nearest_rows<T: Copy>(p: &Picture<'_, T>, dw: usize, dh: usize, ry: &mut [T], ru: &mut [T], rv: &mut [T], mut row: impl FnMut(usize, &Picture<'_, T>)) {
    for r in 0..dh {
        let sy = (r * p.h / dh).min(p.h - 1);
        let (yrow, crow) = (sy * p.y_stride, (sy >> p.ss.1) * p.uv_stride);
        for c in 0..dw {
            let sx = (c * p.w / dw).min(p.w - 1);
            ry[c] = p.y[yrow + sx];
            if !p.mono {
                let ci = crow + (sx >> p.ss.0);
                ru[c] = p.u[ci];
                rv[c] = p.v[ci];
            }
        }
        let (u, v): (&[T], &[T]) = if p.mono { (&[], &[]) } else { (&ru[..dw], &rv[..dw]) };
        let pic = Picture { w: dw, h: 1, y: &ry[..dw], u, v, y_stride: dw, uv_stride: dw, ss: (0, 0), mono: p.mono, bit_depth: p.bit_depth, full: p.full };
        row(r, &pic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{to_rgba, Chroma, Matrix, KR_KB_601};

    #[test]
    fn scale_fit_geometry() {
        assert_eq!(fit(320, 240, 288, 260), Rect { w: 288, h: 216, x: 0, y: 22 });
        assert_eq!(fit(1920, 1080, 640, 480), Rect { w: 640, h: 360, x: 0, y: 60 });
        assert_eq!(fit(64, 48, 288, 260), Rect { w: 288, h: 216, x: 0, y: 22 });
        assert_eq!(fit(100, 400, 288, 260), Rect { w: 65, h: 260, x: 111, y: 0 });
        // a sliver keeps one pixel (the clamp)
        assert_eq!(fit(1000, 1, 10, 10), Rect { w: 10, h: 1, x: 0, y: 4 });
        assert_eq!(fit(1, 1000, 10, 10), Rect { w: 1, h: 10, x: 4, y: 0 });
    }

    #[test]
    fn scale_nearest_rows_hand() {
        // 4x2 4:2:0: y = 0..8, one chroma row of two samples.
        let (y, u, v) = ([0u8, 1, 2, 3, 4, 5, 6, 7], [100u8, 101], [200u8, 201]);
        let p = Picture { w: 4, h: 2, y: &y, u: &u, v: &v, y_stride: 4, uv_stride: 2, ss: (1, 1), mono: false, bit_depth: 8, full: false };
        assert!(fits(&p));
        let (mut ry, mut ru, mut rv) = ([0u8; 6], [0u8; 6], [0u8; 6]);
        let mut got = [[0u8; 18]; 3];
        let mut n = 0;
        nearest_rows(&p, 2, 1, &mut ry, &mut ru, &mut rv, |r, s| {
            assert_eq!((r, s.w, s.h, s.ss, s.y_stride), (0, 2, 1, (0, 0), 2));
            assert_eq!((s.y, s.u, s.v), (&[0u8, 2][..], &[100u8, 101][..], &[200u8, 201][..]));
            n += 1;
        });
        assert_eq!(n, 1);
        nearest_rows(&p, 6, 3, &mut ry, &mut ru, &mut rv, |r, s| {
            got[r][..6].copy_from_slice(s.y);
            got[r][6..12].copy_from_slice(s.u);
            got[r][12..].copy_from_slice(s.v);
        });
        // columns c * 4 / 6 = 0,0,1,2,2,3; rows r * 2 / 3 = 0,0,1
        assert_eq!(got[0], [0, 0, 1, 2, 2, 3, 100, 100, 100, 101, 101, 101, 200, 200, 200, 201, 201, 201]);
        assert_eq!(got[1], got[0]);
        assert_eq!(got[2], [4, 4, 5, 6, 6, 7, 100, 100, 100, 101, 101, 101, 200, 200, 200, 201, 201, 201]);
        // mono: no chroma read, none handed
        let m = Picture { mono: true, u: &[], v: &[], ..p };
        assert!(fits(&m));
        nearest_rows(&m, 3, 2, &mut ry, &mut [], &mut [], |r, s| {
            assert!(s.mono && s.u.is_empty() && s.v.is_empty());
            assert_eq!(s.y, if r == 0 { &[0u8, 1, 2][..] } else { &[4u8, 5, 6][..] });
        });
    }

    #[test]
    fn scale_nearest_then_convert_matches_convert_then_nearest() {
        // 16x12 4:2:0 LCG picture, BT.601 limited, Chroma::Nearest.
        const W: usize = 16;
        const H: usize = 12;
        let mut s = 0x1234_5678u32;
        let mut next = || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 24) as u8
        };
        let mut y = [0u8; W * H];
        let (mut u, mut v) = ([0u8; (W / 2) * (H / 2)], [0u8; (W / 2) * (H / 2)]);
        y.iter_mut().for_each(|b| *b = next());
        u.iter_mut().for_each(|b| *b = next());
        v.iter_mut().for_each(|b| *b = next());
        let p = Picture { w: W, h: H, y: &y, u: &u, v: &v, y_stride: W, uv_stride: W / 2, ss: (1, 1), mono: false, bit_depth: 8, full: false };
        let mut full = [0u8; W * H * 4];
        to_rgba(&p, Matrix::Kr(KR_KB_601), Chroma::Nearest, false, &mut full);
        for (dw, dh) in [(7usize, 5usize), (24, 18)] {
            let (mut ry, mut ru, mut rv) = ([0u8; 24], [0u8; 24], [0u8; 24]);
            let mut rgba = [0u8; 24 * 4];
            let mut rows = 0;
            nearest_rows(&p, dw, dh, &mut ry, &mut ru, &mut rv, |r, s| {
                to_rgba(s, Matrix::Kr(KR_KB_601), Chroma::Nearest, false, &mut rgba);
                let sy = (r * H / dh).min(H - 1);
                for c in 0..dw {
                    let sx = (c * W / dw).min(W - 1);
                    let o = (sy * W + sx) * 4;
                    assert_eq!(rgba[c * 4..c * 4 + 4], full[o..o + 4], "{}x{} row {} col {}", dw, dh, r, c);
                }
                rows += 1;
            });
            assert_eq!(rows, dh);
        }
    }
}
