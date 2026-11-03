// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Step 5: the contour. Crack following (the pixel-edge form of Moore tracing; Pavlidis, "Algorithms for Graphics
//! and Image Processing", 1982): every side of a region pixel that faces a non-region pixel is a directed crack
//! with the region on its LEFT (the rectified frame is y-up, so a region's outer boundary runs CCW); cracks chain
//! end to start. Where two cracks leave one vertex (two pixels touching only at a corner) the region's connectivity
//! decides: the part (8-connected) turns right and stays one loop, a hole (4-connected) turns left. The polyline is
//! the crack MIDPOINTS (no staircase corners), in paper mm. The part's outer loop is the one of largest positive
//! area; each hole's loop is reversed to run CW; both orientations are checked by geom_core's exact `is_ccw`.
//! Douglas-Peucker (Douglas & Peucker, Cartographica 1973; the closed form splits at the vertex farthest from the
//! first) simplifies at a stated tolerance.

use alloc::vec;
use alloc::vec::Vec;

use geom_core::{Predicate, area, is_ccw};

use crate::silhouette::Silhouette;
use crate::{Refusal, Vec2};

const NO: u8 = u8::MAX;
const DX: [i64; 4] = [1, 0, -1, 0];
const DY: [i64; 4] = [0, 1, 0, -1];

/// The dense boundary loops of a silhouette (paper mm): the part's outer loop CCW, each hole's loop CW.
#[derive(Clone, Debug, PartialEq)]
pub struct Loops {
    pub outer: Vec<Vec2>,
    pub holes: Vec<Vec<Vec2>>,
}

/// The crack loops of the region `inside` over a `w` x `h` grid, as crack midpoints in pixel-corner units.
pub fn trace_region(inside: &dyn Fn(i64, i64) -> bool, w: usize, h: usize, join_diagonal: bool) -> Vec<Vec<(f64, f64)>> {
    let vw = w + 1;
    let nv = vw * (h + 1);
    let mut outs = vec![[NO; 2]; nv];
    let mut add = |vx: i64, vy: i64, d: u8| {
        let s = &mut outs[vy as usize * vw + vx as usize];
        if s[0] == NO {
            s[0] = d;
        } else {
            s[1] = d;
        }
    };
    for v in 0..h as i64 {
        for u in 0..w as i64 {
            if !inside(u, v) {
                continue;
            }
            if !inside(u, v - 1) {
                add(u, v, 0);
            }
            if !inside(u + 1, v) {
                add(u + 1, v, 1);
            }
            if !inside(u, v + 1) {
                add(u + 1, v + 1, 2);
            }
            if !inside(u - 1, v) {
                add(u, v + 1, 3);
            }
        }
    }
    let mut used = vec![[false; 2]; nv];
    let mut loops = Vec::new();
    for start in 0..nv {
        for s0 in 0..2 {
            if outs[start][s0] == NO || used[start][s0] {
                continue;
            }
            let mut pts = Vec::new();
            let (mut vtx, mut slot) = (start, s0);
            loop {
                used[vtx][slot] = true;
                let d = outs[vtx][slot] as usize;
                let (vx, vy) = ((vtx % vw) as i64, (vtx / vw) as i64);
                pts.push((vx as f64 + 0.5 * DX[d] as f64, vy as f64 + 0.5 * DY[d] as f64));
                let next = ((vy + DY[d]) as usize) * vw + (vx + DX[d]) as usize;
                let o = outs[next];
                let chosen = if o[1] == NO {
                    0
                } else {
                    let want = (if join_diagonal { (d + 3) % 4 } else { (d + 1) % 4 }) as u8;
                    if o[0] == want { 0 } else { 1 }
                };
                if o[chosen] == NO || used[next][chosen] {
                    break;
                }
                vtx = next;
                slot = chosen;
            }
            loops.push(pts);
        }
    }
    loops
}

/// The part's outer loop and the holes' loops of a silhouette.
pub fn trace(sil: &Silhouette) -> Result<Loops, Refusal> {
    let (w, h) = (sil.w, sil.h);
    let to_mm = |l: &[(f64, f64)]| -> Vec<Vec2> { l.iter().map(|&(x, y)| sil.corner_mm(x, y)).collect() };
    let part = |u: i64, v: i64| u >= 0 && v >= 0 && (u as usize) < w && (v as usize) < h && sil.part[v as usize * w + u as usize];
    let outer = trace_region(&part, w, h, true)
        .iter()
        .map(|l| to_mm(l))
        .max_by(|a, b| area(a).total_cmp(&area(b)))
        .ok_or(Refusal::Uncertain)?;
    if is_ccw(&outer) != Predicate::Positive {
        return Err(Refusal::Uncertain);
    }
    let mut holes = Vec::new();
    for k in 1..=sil.holes as u32 {
        let hk = |u: i64, v: i64| u >= 0 && v >= 0 && (u as usize) < w && (v as usize) < h && sil.hole[v as usize * w + u as usize] == k;
        let Some(mut l) = trace_region(&hk, w, h, false).iter().map(|l| to_mm(l)).max_by(|a, b| area(a).abs().total_cmp(&area(b).abs())) else {
            continue;
        };
        l.reverse();
        if is_ccw(&l) != Predicate::Negative {
            return Err(Refusal::Uncertain);
        }
        holes.push(l);
    }
    Ok(Loops { outer, holes })
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.mag_sq();
    if l2 == 0.0 {
        return p.dist(a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.dist(a + ab * t)
}

/// Douglas-Peucker on an open polyline: the kept indices (both ends included), ascending.
pub fn dp_open(pts: &[Vec2], eps: f64) -> Vec<usize> {
    let n = pts.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut km, mut dm) = (a, -1.0);
        for k in a + 1..b {
            let d = seg_dist(pts[k], pts[a], pts[b]);
            if d > dm {
                dm = d;
                km = k;
            }
        }
        if dm > eps {
            keep[km] = true;
            stack.push((a, km));
            stack.push((km, b));
        }
    }
    (0..n).filter(|&k| keep[k]).collect()
}

/// Douglas-Peucker on a closed loop: split at vertex 0 and the vertex farthest from it; the kept indices ascending.
pub fn dp_closed(pts: &[Vec2], eps: f64) -> Vec<usize> {
    let n = pts.len();
    if n <= 3 {
        return (0..n).collect();
    }
    let far = (1..n).max_by(|&a, &b| pts[a].dist(pts[0]).total_cmp(&pts[b].dist(pts[0]))).unwrap_or(n / 2);
    let mut idx = dp_open(&pts[..=far], eps);
    let mut chain: Vec<Vec2> = pts[far..].to_vec();
    chain.push(pts[0]);
    for k in dp_open(&chain, eps) {
        let i = far + k;
        if i < n {
            idx.push(i);
        }
    }
    idx.sort_unstable();
    idx.dedup();
    idx
}

/// The loop's Douglas-Peucker vertices at `eps`.
pub fn simplify(pts: &[Vec2], eps: f64) -> Vec<Vec2> {
    dp_closed(pts, eps).into_iter().map(|k| pts[k]).collect()
}
