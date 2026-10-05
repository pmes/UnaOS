// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The loop filters (RFC 6386 §15): the normal filter (macroblock-edge and sub-block-edge
//! variants, §15.3) and the simple filter (§15.2), applied per macroblock in raster order after the
//! whole frame is reconstructed.

/// The per-level filter parameters (§15.2 / §15.3 `interior_limit`, edge limits, `hev_threshold`).
#[derive(Clone, Copy)]
pub struct Params {
    pub mb_limit: i32,
    pub sub_limit: i32,
    pub interior: i32,
    pub hev: i32,
}

pub fn params(level: i32, sharpness: i32, key_frame: bool) -> Params {
    let mut interior = level;
    if sharpness > 0 {
        interior >>= if sharpness > 4 { 2 } else { 1 };
        interior = interior.min(9 - sharpness);
    }
    interior = interior.max(1);
    let hev = if key_frame {
        if level >= 40 { 2 } else if level >= 15 { 1 } else { 0 }
    } else if level >= 40 {
        3
    } else if level >= 20 {
        2
    } else if level >= 15 {
        1
    } else {
        0
    };
    Params { mb_limit: (level + 2) * 2 + interior, sub_limit: level * 2 + interior, interior, hev }
}

#[inline]
fn c8(v: i32) -> i32 {
    v.clamp(-128, 127)
}
#[inline]
fn s(v: u8) -> i32 {
    v as i32 - 128
}
#[inline]
fn u(v: i32) -> u8 {
    (v + 128) as u8
}

/// The common adjustment (§15.2 `common_adjust`): returns the filter value `a` applied to p0/q0.
#[inline]
fn common_adjust(use_outer_taps: bool, p1: i32, p0: &mut i32, q0: &mut i32, q1: i32) -> i32 {
    let mut a = if use_outer_taps { c8(p1 - q1) } else { 0 };
    a = c8(a + 3 * (*q0 - *p0));
    let f1 = c8(a + 4) >> 3;
    let f2 = c8(a + 3) >> 3;
    *q0 = c8(*q0 - f1);
    *p0 = c8(*p0 + f2);
    f1
}

#[inline]
fn simple_ok(limit: i32, p1: u8, p0: u8, q0: u8, q1: u8) -> bool {
    (p0 as i32 - q0 as i32).abs() * 2 + (p1 as i32 - q1 as i32).abs() / 2 <= limit
}

/// Simple-filter one edge of `n` samples: `step` crosses the edge, `pitch` runs along it.
pub fn simple_edge(b: &mut [u8], o: usize, step: usize, pitch: usize, n: usize, limit: i32) {
    for k in 0..n {
        let i = o + k * pitch;
        let (p1, p0, q0, q1) = (b[i - 2 * step], b[i - step], b[i], b[i + step]);
        if simple_ok(limit, p1, p0, q0, q1) {
            let (mut sp0, mut sq0) = (s(p0), s(q0));
            common_adjust(true, s(p1), &mut sp0, &mut sq0, s(q1));
            b[i - step] = u(sp0);
            b[i] = u(sq0);
        }
    }
}

#[inline]
fn normal_ok(e: i32, i: i32, px: &[i32; 8]) -> bool {
    let [p3, p2, p1, p0, q0, q1, q2, q3] = *px;
    (p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 <= e
        && (p3 - p2).abs() <= i
        && (p2 - p1).abs() <= i
        && (p1 - p0).abs() <= i
        && (q3 - q2).abs() <= i
        && (q2 - q1).abs() <= i
        && (q1 - q0).abs() <= i
}

/// Normal filter across one edge: `mb` selects the macroblock-edge variant (§15.3).
#[allow(clippy::too_many_arguments)]
pub fn normal_edge(b: &mut [u8], o: usize, step: usize, pitch: usize, n: usize, edge_limit: i32, p: &Params, mb: bool) {
    for k in 0..n {
        let i = o + k * pitch;
        let px = [
            b[i - 4 * step] as i32,
            b[i - 3 * step] as i32,
            b[i - 2 * step] as i32,
            b[i - step] as i32,
            b[i] as i32,
            b[i + step] as i32,
            b[i + 2 * step] as i32,
            b[i + 3 * step] as i32,
        ];
        if !normal_ok(edge_limit, p.interior, &px) {
            continue;
        }
        let hev = (px[2] - px[3]).abs() > p.hev || (px[5] - px[4]).abs() > p.hev;
        let (p2, p1, mut p0, mut q0, q1, q2) = (px[1] - 128, px[2] - 128, px[3] - 128, px[4] - 128, px[5] - 128, px[6] - 128);
        if mb {
            if hev {
                common_adjust(true, p1, &mut p0, &mut q0, q1);
                b[i - step] = u(p0);
                b[i] = u(q0);
            } else {
                let w = c8(c8(p1 - q1) + 3 * (q0 - p0));
                let a = c8((27 * w + 63) >> 7);
                b[i] = u(c8(q0 - a));
                b[i - step] = u(c8(p0 + a));
                let a = c8((18 * w + 63) >> 7);
                b[i + step] = u(c8(q1 - a));
                b[i - 2 * step] = u(c8(p1 + a));
                let a = c8((9 * w + 63) >> 7);
                b[i + 2 * step] = u(c8(q2 - a));
                b[i - 3 * step] = u(c8(p2 + a));
            }
        } else {
            let a = common_adjust(hev, p1, &mut p0, &mut q0, q1);
            b[i - step] = u(p0);
            b[i] = u(q0);
            if !hev {
                let a = (a + 1) >> 1;
                b[i + step] = u(c8(q1 - a));
                b[i - 2 * step] = u(c8(p1 + a));
            }
        }
    }
}
