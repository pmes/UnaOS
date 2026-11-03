// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2), seam shared-core — the polygon offset of a pocket's loops (VUGMAKEPOCKET, LEDGER SR479).
//!
//! A level `d` is the set of points at distance `d` from the WHOLE boundary (the outline and every island), built
//! from the original loops each time (no error accumulates ring to ring) without a polygon clipper:
//! 1. the raw offset of every loop by Contour's corner rule (`op::offset_path`: the offset lines' intersection at a
//!    corner that turns toward the offset, an arc about the vertex at one that turns away), the arc flattened as a
//!    CIRCUMSCRIBED tangent polyline, so every point of it is at least `d` from the vertex (it never gouges and leaves
//!    at most `chord` of material in a reflex corner);
//! 2. every raw segment split at every crossing with every other one (the one crossing point handed to both);
//! 3. a piece kept iff its midpoint is no nearer than `d` to the boundary edges — the distance form of the winding
//!    rule on a validated domain (islands strictly inside the outline, no loop crossing another), so swallowtails, the
//!    pieces past a collapse and the pieces outside the domain all go, and outline and island rings merge with no
//!    special case;
//! 4. the kept pieces chained into closed rings by their shared end points; a chain that does not close refuses
//!    `NoRings` (never an open path).
//!
//! Every ring keeps the domain on its left. Only geom names valid on both sides of the geom_core swap are used.

use alloc::vec::Vec;
use libm::{acos, atan2, ceil, cos, fabs, sin, sqrt};

use crate::geom::{Tol, Vec2, line_line2, seg_seg2, signed_area2};
use crate::validate::Refusal;

fn len(v: Vec2) -> f64 {
    sqrt(v.dot(v))
}

fn near(a: Vec2, b: Vec2, tol: Tol) -> bool {
    len(a - b) <= 2.0 * tol.lin
}

/// The foot of `p` on the segment `ab`.
fn foot(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let e = b - a;
    let ee = e.dot(e);
    let t = if ee > 0.0 { ((p - a).dot(e) / ee).clamp(0.0, 1.0) } else { 0.0 };
    a + e * t
}

fn pt_seg(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    len(p - foot(p, a, b))
}

fn seg_seg_dist(a: Vec2, b: Vec2, c: Vec2, d: Vec2, tol: Tol) -> f64 {
    if seg_seg2(a, b, c, d, tol) {
        return 0.0;
    }
    pt_seg(a, c, d).min(pt_seg(b, c, d)).min(pt_seg(c, a, b)).min(pt_seg(d, a, b))
}

fn edge(lp: &[Vec2], i: usize) -> (Vec2, Vec2) {
    (lp[i], lp[(i + 1) % lp.len()])
}

/// The distance from `p` to the nearest boundary edge.
fn dist(p: Vec2, lps: &[Vec<Vec2>]) -> f64 {
    let mut m = f64::INFINITY;
    for lp in lps {
        for i in 0..lp.len() {
            let (a, b) = edge(lp, i);
            m = m.min(pt_seg(p, a, b));
        }
    }
    m
}

/// The loop without repeats or a closing duplicate, oriented counter-clockwise when `ccw`, else clockwise.
fn normalise(pts: &[Vec2], ccw: bool, tol: Tol) -> Result<Vec<Vec2>, Refusal> {
    let mut p: Vec<Vec2> = Vec::with_capacity(pts.len());
    for &q in pts {
        if p.last().map_or(true, |l| len(q - *l) > tol.lin) {
            p.push(q);
        }
    }
    while p.len() >= 2 && len(p[0] - p[p.len() - 1]) <= tol.lin {
        p.pop();
    }
    if p.len() < 3 {
        return Err(Refusal::ContourDegenerate { what: "fewer than 3 points" });
    }
    let a = signed_area2(&p);
    if fabs(a) <= tol.lin * tol.lin {
        return Err(Refusal::ContourDegenerate { what: "zero area" });
    }
    let n = p.len();
    for j in 0..n {
        let (u, v) = (p[j] - p[(j + n - 1) % n], p[(j + 1) % n] - p[j]);
        if fabs(u.cross(v)) <= tol.ang * len(u) * len(v) && u.dot(v) < 0.0 {
            return Err(Refusal::ContourDegenerate { what: "edge reverses" });
        }
    }
    if (a > 0.0) != ccw {
        p.reverse();
    }
    Ok(p)
}

fn self_crosses(lp: &[Vec2], tol: Tol) -> bool {
    let n = lp.len();
    for a in 0..n {
        for b in (a + 2)..n {
            if a == 0 && b == n - 1 {
                continue; // the closing pair is adjacent
            }
            let ((p1, p2), (p3, p4)) = (edge(lp, a), edge(lp, b));
            if seg_seg2(p1, p2, p3, p4, tol) {
                return true;
            }
        }
    }
    false
}

fn loops_cross(x: &[Vec2], y: &[Vec2], tol: Tol) -> bool {
    for a in 0..x.len() {
        for b in 0..y.len() {
            let ((p1, p2), (p3, p4)) = (edge(x, a), edge(y, b));
            if seg_seg2(p1, p2, p3, p4, tol) {
                return true;
            }
        }
    }
    false
}

/// Whether `p` is inside the closed `ring` (the crossing number).
pub(crate) fn inside(p: Vec2, ring: &[Vec2]) -> bool {
    let mut c = false;
    for i in 0..ring.len() {
        let (a, b) = edge(ring, i);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x {
                c = !c;
            }
        }
    }
    c
}

/// The pocket's boundary: the outline counter-clockwise first, then each island clockwise (the domain on the left of
/// every edge). Refuses a loop that crosses itself or an island that crosses another (`OutlineSelfIntersects`) and an
/// island that is not strictly inside the outline and outside every other island (`IslandOutsideOutline`).
pub(crate) fn loops(outline: &[Vec2], islands: &[Vec<Vec2>], tol: Tol) -> Result<Vec<Vec<Vec2>>, Refusal> {
    let o = normalise(outline, true, tol)?;
    if self_crosses(&o, tol) {
        return Err(Refusal::OutlineSelfIntersects);
    }
    let mut out = alloc::vec![o];
    for (k, isl) in islands.iter().enumerate() {
        let l = normalise(isl, false, tol)?;
        if self_crosses(&l, tol) {
            return Err(Refusal::OutlineSelfIntersects);
        }
        if loops_cross(&out[0], &l, tol) || !l.iter().all(|&q| inside(q, &out[0])) {
            return Err(Refusal::IslandOutsideOutline { island: k });
        }
        for prev in &out[1..] {
            if loops_cross(prev, &l, tol) {
                return Err(Refusal::OutlineSelfIntersects);
            }
            if inside(l[0], prev) || inside(prev[0], &l) {
                return Err(Refusal::IslandOutsideOutline { island: k });
            }
        }
        out.push(l);
    }
    Ok(out)
}

/// The raw offset of one loop to its left by `d`, as a closed polyline.
fn raw(lp: &[Vec2], d: f64, chord: f64, tol: Tol) -> Vec<Vec2> {
    let n = lp.len();
    let dir: Vec<Vec2> = (0..n)
        .map(|i| {
            let e = lp[(i + 1) % n] - lp[i];
            e * (1.0 / len(e))
        })
        .collect();
    let nor = |i: usize| Vec2::new(-dir[i].y, dir[i].x) * d;
    // The half angle of one circumscribed chord whose bulge past the arc is `chord`.
    let h = acos(d / (d + chord));
    let mut out = Vec::new();
    for j in 0..n {
        let i = (j + n - 1) % n; // the edge into vertex j
        let v = lp[j];
        let turn = dir[i].cross(dir[j]);
        if fabs(turn) <= tol.ang {
            out.push(v + nor(j));
        } else if turn > 0.0 {
            // Toward the offset: the offset lines' intersection (past a collapse the edge reverses; step 3 drops it).
            match line_line2(lp[i] + nor(i), dir[i], v + nor(j), dir[j], tol) {
                Some(q) => out.push(q),
                None => {
                    out.push(v + nor(i));
                    out.push(v + nor(j));
                }
            }
        } else {
            // Away from the offset: the clockwise arc about v from nor(i) to nor(j), circumscribed.
            let phi = atan2(-turn, dir[i].dot(dir[j]));
            let m = ceil(phi / (2.0 * h)).max(1.0);
            let step = phi / m;
            let k_out = 1.0 / cos(0.5 * step);
            let w = nor(i);
            out.push(v + w);
            let mut k = 1.0;
            while k <= m {
                let th = -(k - 0.5) * step;
                let r = Vec2::new(w.x * cos(th) - w.y * sin(th), w.x * sin(th) + w.y * cos(th));
                out.push(v + r * k_out);
                k += 1.0;
            }
            out.push(v + nor(j));
        }
    }
    out
}

/// The rings at distance `d` from the boundary `lps` (from [`loops`]), each closed and with the domain on its left;
/// empty when nothing of the pocket is that far from its boundary.
pub(crate) fn rings(lps: &[Vec<Vec2>], d: f64, chord: f64, tol: Tol) -> Result<Vec<Vec<Vec2>>, Refusal> {
    let mut segs: Vec<(Vec2, Vec2)> = Vec::new();
    for lp in lps {
        let r = raw(lp, d, chord, tol);
        for i in 0..r.len() {
            let (a, b) = edge(&r, i);
            if len(b - a) > tol.lin {
                segs.push((a, b));
            }
        }
    }
    let k = segs.len();
    let mut cuts: Vec<Vec<(f64, Vec2)>> = alloc::vec![Vec::new(); k];
    for a in 0..k {
        for b in (a + 1)..k {
            let ((p1, p2), (p3, p4)) = (segs[a], segs[b]);
            if !seg_seg2(p1, p2, p3, p4, tol) {
                continue;
            }
            let Some(q) = line_line2(p1, p2 - p1, p3, p4 - p3, tol) else { continue };
            for s in [a, b] {
                let (x, y) = segs[s];
                let e = y - x;
                let t = (q - x).dot(e) / e.dot(e);
                if t > 0.0 && t < 1.0 && len(q - x) > tol.lin && len(q - y) > tol.lin {
                    cuts[s].push((t, q));
                }
            }
        }
    }
    let mut pieces: Vec<(Vec2, Vec2)> = Vec::new();
    for (s, c) in cuts.iter_mut().enumerate() {
        let (x, y) = segs[s];
        c.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap_or(core::cmp::Ordering::Equal));
        c.push((1.0, y));
        let mut from = x;
        for &(_, q) in c.iter() {
            if len(q - from) > tol.lin {
                if dist((from + q) * 0.5, lps) >= d - tol.lin {
                    pieces.push((from, q));
                }
                from = q;
            }
        }
    }
    let mut used = alloc::vec![false; pieces.len()];
    let mut out = Vec::new();
    for s in 0..pieces.len() {
        if used[s] {
            continue;
        }
        used[s] = true;
        let start = pieces[s].0;
        let mut ring = alloc::vec![start];
        let mut at = pieces[s].1;
        loop {
            if near(at, start, tol) {
                break;
            }
            ring.push(at);
            let Some(nx) = (0..pieces.len()).find(|&c| !used[c] && near(pieces[c].0, at, tol)) else {
                return Err(Refusal::NoRings);
            };
            used[nx] = true;
            at = pieces[nx].1;
        }
        let mut per = 0.0;
        for i in 0..ring.len() {
            let (a, b) = edge(&ring, i);
            per += len(b - a);
        }
        if ring.len() >= 3 && fabs(signed_area2(&ring)) > tol.lin * per {
            out.push(ring);
        }
    }
    Ok(out)
}

/// The nearest point of the closed `ring` to `p`: (its distance, the edge index, the point).
pub(crate) fn nearest(p: Vec2, ring: &[Vec2]) -> (f64, usize, Vec2) {
    let mut best = (f64::INFINITY, 0, ring[0]);
    for i in 0..ring.len() {
        let (a, b) = edge(ring, i);
        let q = foot(p, a, b);
        let dd = len(p - q);
        if dd < best.0 {
            best = (dd, i, q);
        }
    }
    best
}

/// The same closed ring, in the same direction, starting at `q` on its edge `e`.
pub(crate) fn start_at(ring: &[Vec2], e: usize, q: Vec2, tol: Tol) -> Vec<Vec2> {
    let n = ring.len();
    let mut out = Vec::with_capacity(n + 1);
    out.push(q);
    for k in 1..=n {
        let v = ring[(e + k) % n];
        if len(v - out[out.len() - 1]) > tol.lin {
            out.push(v);
        }
    }
    while out.len() > 1 && len(out[out.len() - 1] - q) <= tol.lin {
        out.pop();
    }
    out
}

/// The least distance from the segment `ab` to the boundary `lps`.
pub(crate) fn clearance(a: Vec2, b: Vec2, lps: &[Vec<Vec2>], tol: Tol) -> f64 {
    let mut m = f64::INFINITY;
    for lp in lps {
        for i in 0..lp.len() {
            let (c, d) = edge(lp, i);
            m = m.min(seg_seg_dist(a, b, c, d, tol));
        }
    }
    m
}
