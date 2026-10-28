// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CHARTER: Stria (CODEX §2 A/V), seam shared-core — the ONE nearest resampler (letterbox geometry + per-row 4:4:4 gather) user-stria and the kernel Player sample with.
//! `yuv_core::scale` — the ONE nearest resampler (KERNELSCALEONE, rmbp-ledger B965) and its area twin [`area_rows`] (KERNELSCALEONEARM B980).
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

// ── KERNELSCALEONEARM (rmbp-ledger B980): the area resampler (handlers.md §5 NLEPROBE-HDR's SCALE-AREA) ──────────────────────
// [`area_rows`] sits beside [`nearest_rows`] with its contract (caller scratch, each output row a 1-high 4:4:4 `Picture` of `dw`
// samples, `p`'s depth, range and mono) and its index: output column `c` covers source columns `c * w / dw` (nearest's own pick)
// up to `(c + 1) * w / dw` (at least one), rows likewise. Each sample is the rounded integer mean of its plane over that box,
// chroma over the chroma samples covering it, so the caller converts the averaged row through the ONE converter
// ([`crate::to_rgba`]; at 4:4:4 every chroma mode reads the sample as is). Not the same pixels as converting then averaging RGB
// (the matrix and the range clamp are not linear over a box): the KATs pin hand values. At ratio 1 it is `nearest_rows`.

/// THE AREA SCALER: `p` box-averaged to `dw x dh`, each output row handed to `row(r, &picture)` as [`nearest_rows`] hands it.
/// `ry` (and, unless `mono`, `ru` / `rv`) hold at least `dw` samples; `fits(p)`.
pub fn area_rows<T: Copy + Into<u64> + TryFrom<u64>>(p: &Picture<'_, T>, dw: usize, dh: usize, ry: &mut [T], ru: &mut [T], rv: &mut [T], mut row: impl FnMut(usize, &Picture<'_, T>)) {
    // The box of output index `i` of `d` over `n` source samples: nearest's pick, then up to the next one's (at least one sample).
    let span = |i: usize, n: usize, d: usize| -> (usize, usize) {
        let a = (i * n / d).min(n - 1);
        (a, ((i + 1) * n / d).clamp(a + 1, n))
    };
    // The rounded mean; it never exceeds the largest summed sample, so it fits `T` (the fallback is never taken).
    let mean = |sum: u64, n: usize, or: T| -> T { T::try_from((sum + n as u64 / 2) / n as u64).unwrap_or(or) };
    for r in 0..dh {
        let (y0, y1) = span(r, p.h, dh);
        let (cy0, cy1) = (y0 >> p.ss.1, ((y1 - 1) >> p.ss.1) + 1);
        for c in 0..dw {
            let (x0, x1) = span(c, p.w, dw);
            let mut s = 0u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    s += Into::<u64>::into(p.y[y * p.y_stride + x]);
                }
            }
            ry[c] = mean(s, (y1 - y0) * (x1 - x0), p.y[y0 * p.y_stride + x0]);
            if !p.mono {
                let (cx0, cx1) = (x0 >> p.ss.0, ((x1 - 1) >> p.ss.0) + 1);
                let (mut su, mut sv) = (0u64, 0u64);
                for y in cy0..cy1 {
                    for x in cx0..cx1 {
                        su += Into::<u64>::into(p.u[y * p.uv_stride + x]);
                        sv += Into::<u64>::into(p.v[y * p.uv_stride + x]);
                    }
                }
                let (n, first) = ((cy1 - cy0) * (cx1 - cx0), cy0 * p.uv_stride + cx0);
                ru[c] = mean(su, n, p.u[first]);
                rv[c] = mean(sv, n, p.v[first]);
            }
        }
        let (u, v): (&[T], &[T]) = if p.mono { (&[], &[]) } else { (&ru[..dw], &rv[..dw]) };
        let pic = Picture { w: dw, h: 1, y: &ry[..dw], u, v, y_stride: dw, uv_stride: dw, ss: (0, 0), mono: p.mono, bit_depth: p.bit_depth, full: p.full };
        row(r, &pic);
    }
}

#[cfg(test)]
mod area_kats {
    extern crate std;
    use super::*;
    use crate::{to_rgba, to_rgba_cols, Chroma, Matrix, KR_KB_601, KR_KB_709};

    const W: usize = 7;
    const H: usize = 5;
    const CW: usize = (W + 1) / 2;
    const CH: usize = (H + 1) / 2;

    #[test]
    fn one_arm_cols_are_the_gathered_rows() {
        // KERNELSCALEONEARM (B980): the kernel Player's VP8 arm (8-bit 4:2:0, Nearest) now converts each distinct sampled source
        // row over the distinct sampled columns (to_rgba_cols on the whole picture) where it gathered a 4:4:4 row and converted it
        // (nearest_rows + to_rgba): the same bytes, and the converted count is distinct rows x distinct columns.
        let mut s = 0x2545_f491u32;
        let mut next = || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 24) as u8
        };
        let (mut y, mut u, mut v) = ([0u8; W * H], [0u8; CW * CH], [0u8; CW * CH]);
        y.iter_mut().for_each(|b| *b = next());
        u.iter_mut().for_each(|b| *b = next());
        v.iter_mut().for_each(|b| *b = next());
        for full in [false, true] {
            let p = Picture { w: W, h: H, y: &y, u: &u, v: &v, y_stride: W, uv_stride: CW, ss: (1, 1), mono: false, bit_depth: 8, full };
            assert!(fits(&p));
            for m in [Matrix::Kr(KR_KB_601), Matrix::Kr(KR_KB_709)] {
                for (dw, dh) in [(3usize, 2usize), (5, 4), (11, 9), (14, 10)] {
                    let (mut cols, mut at, mut n) = ([0usize; 14], [0usize; 14], 0usize);
                    for c in 0..dw {
                        let sx = (c * W / dw).min(W - 1);
                        if n == 0 || cols[n - 1] != sx {
                            cols[n] = sx;
                            n += 1;
                        }
                        at[c] = n - 1;
                    }
                    let (mut ry, mut ru, mut rv) = ([0u8; 14], [0u8; 14], [0u8; 14]);
                    let (mut gathered, mut line) = ([0u8; 14 * 4], [0u8; W * 4]);
                    let (mut converted, mut last) = (0usize, usize::MAX);
                    nearest_rows(&p, dw, dh, &mut ry, &mut ru, &mut rv, |r, g| {
                        to_rgba(g, m, Chroma::Nearest, false, &mut gathered);
                        let sy = (r * H / dh).min(H - 1);
                        if last != sy {
                            to_rgba_cols(&p, sy, sy + 1, cols[..n].iter().copied(), m, Chroma::Nearest, false, &mut line);
                            converted += n;
                            last = sy;
                        }
                        for c in 0..dw {
                            assert_eq!(line[at[c] * 4..at[c] * 4 + 4], gathered[c * 4..c * 4 + 4], "{full} {m:?} {dw}x{dh} row {r} col {c}");
                        }
                    });
                    assert_eq!(n, dw.min(W), "{dw}x{dh}");
                    assert_eq!(converted, dh.min(H) * n, "{dw}x{dh}");
                }
            }
        }
    }

    #[test]
    fn area_rows_hand() {
        // 4x2 4:2:0 (the nearest KAT's picture) to 2x1: luma boxes {0,1,4,5} and {2,3,6,7}, means (10 + 2) / 4 = 3 and
        // (18 + 2) / 4 = 5; each box's chroma is one sample.
        let (y, u, v) = ([0u8, 1, 2, 3, 4, 5, 6, 7], [100u8, 101], [200u8, 201]);
        let p = Picture { w: 4, h: 2, y: &y, u: &u, v: &v, y_stride: 4, uv_stride: 2, ss: (1, 1), mono: false, bit_depth: 8, full: false };
        let (mut ry, mut ru, mut rv) = ([0u8; 6], [0u8; 6], [0u8; 6]);
        let mut n = 0;
        area_rows(&p, 2, 1, &mut ry, &mut ru, &mut rv, |r, s| {
            assert_eq!((r, s.w, s.h, s.ss, s.y_stride, s.mono), (0, 2, 1, (0, 0), 2, false));
            assert_eq!((s.y, s.u, s.v), (&[3u8, 5][..], &[100u8, 101][..], &[200u8, 201][..]));
            n += 1;
        });
        assert_eq!(n, 1);
        // ratio 1: the source itself, nearest_rows' rows
        let (mut ny, mut nu, mut nv) = ([0u8; 6], [0u8; 6], [0u8; 6]);
        let mut want = [[0u8; 12]; 2];
        nearest_rows(&p, 4, 2, &mut ny, &mut nu, &mut nv, |r, s| {
            want[r][..4].copy_from_slice(s.y);
            want[r][4..8].copy_from_slice(s.u);
            want[r][8..].copy_from_slice(s.v);
        });
        area_rows(&p, 4, 2, &mut ry, &mut ru, &mut rv, |r, s| {
            assert_eq!((s.y, s.u, s.v), (&want[r][..4], &want[r][4..8], &want[r][8..]), "row {r}");
        });
        // 6x4 4:2:0 (y = 0..24, chroma 3x2 = 10..60) to 3x2: 2x2 luma boxes, one chroma sample each.
        let mut y6 = [0u16; 24];
        y6.iter_mut().enumerate().for_each(|(i, b)| *b = i as u16);
        let (u6, v6) = ([10u16, 20, 30, 40, 50, 60], [61u16, 62, 63, 64, 65, 66]);
        let p6 = Picture { w: 6, h: 4, y: &y6, u: &u6, v: &v6, y_stride: 6, uv_stride: 3, ss: (1, 1), mono: false, bit_depth: 10, full: false };
        let (mut qy, mut qu, mut qv) = ([0u16; 3], [0u16; 3], [0u16; 3]);
        let mut got = [[0u16; 9]; 2];
        area_rows(&p6, 3, 2, &mut qy, &mut qu, &mut qv, |r, s| {
            got[r][..3].copy_from_slice(s.y);
            got[r][3..6].copy_from_slice(s.u);
            got[r][6..].copy_from_slice(s.v);
        });
        // (0+1+6+7+2)/4 = 4, (2+3+8+9+2)/4 = 6, (4+5+10+11+2)/4 = 8; the second row's boxes are 48 more: 16, 18, 20
        assert_eq!(got[0], [4, 6, 8, 10, 20, 30, 61, 62, 63]);
        assert_eq!(got[1], [16, 18, 20, 40, 50, 60, 64, 65, 66]);
        // a non-integer ratio, mono: 5 to 2 columns, boxes [0, 2) and [2, 5): (30 + 1) / 2 = 15, (120 + 1) / 3 = 40; no chroma
        let m5 = [10u8, 20, 30, 40, 50];
        let pm = Picture { w: 5, h: 1, y: &m5, u: &[], v: &[], y_stride: 5, uv_stride: 0, ss: (0, 0), mono: true, bit_depth: 8, full: true };
        assert!(fits(&pm));
        area_rows(&pm, 2, 1, &mut ry, &mut [], &mut [], |_, s| {
            assert!(s.mono && s.u.is_empty() && s.v.is_empty());
            assert_eq!(s.y, &[15u8, 40][..]);
        });
    }

    #[test]
    fn area_cost_line() {
        // The host cost of the two present paths on a 1080p 8-bit 4:2:0 picture in a 640x360 fit (scale + the ONE converter):
        // printed for the record, never asserted (KERNELSCALEONEARM B980; SCALE-AREA-DEFAULT waits for the metal number).
        const SW: usize = 1920;
        const SH: usize = 1080;
        let y: std::vec::Vec<u8> = (0..SW * SH).map(|i| (i * 7 % 251) as u8).collect();
        let (u, v) = (std::vec![128u8; SW / 2 * SH / 2], std::vec![120u8; SW / 2 * SH / 2]);
        let p = Picture { w: SW, h: SH, y: &y, u: &u, v: &v, y_stride: SW, uv_stride: SW / 2, ss: (1, 1), mono: false, bit_depth: 8, full: false };
        let (dw, dh) = (640usize, 360usize);
        let (mut ry, mut ru, mut rv, mut rgba) = (std::vec![0u8; dw], std::vec![0u8; dw], std::vec![0u8; dw], std::vec![0u8; dw * 4]);
        let mut sink = 0u64;
        let t = std::time::Instant::now();
        nearest_rows(&p, dw, dh, &mut ry, &mut ru, &mut rv, |_, s| {
            to_rgba(s, Matrix::Kr(KR_KB_709), Chroma::Nearest, false, &mut rgba);
            sink += rgba[0] as u64;
        });
        let nearest_us = t.elapsed().as_micros();
        let t = std::time::Instant::now();
        area_rows(&p, dw, dh, &mut ry, &mut ru, &mut rv, |_, s| {
            to_rgba(s, Matrix::Kr(KR_KB_709), Chroma::Nearest, false, &mut rgba);
            sink += rgba[0] as u64;
        });
        let area_us = t.elapsed().as_micros();
        std::println!("[yuv] scale cost {}x{} -> {}x{} nearest_us={} area_us={} (sink {})", SW, SH, dw, dh, nearest_us, area_us, sink);
    }
}
