// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2), seam shared-core — the polygon offset of a pocket's loops (VUGMAKEPOCKET, LEDGER SR479), the ONE offset in make_core (VUGMAKEARCFIT, LEDGER SR490: Contour and slice's perimeters ride it too).
//!
//! A level `d` is the set of points at distance `d` from the WHOLE boundary (the outline and every island), built
//! from the original loops each time (no error accumulates ring to ring) without a polygon clipper:
//! 1. the raw offset of every loop by the corner rule (the offset lines' intersection at a corner that turns toward
//!    the offset, an arc about the vertex at one that turns away), the arc kept EXACT as an [`El::Arc`] (centre the
//!    vertex, radius `d`, from one tangent point to the other); [`flatten`] gives the circumscribed polyline (every
//!    point at least `d` from the vertex, at most `chord` of bulge) to a caller that cannot say an arc;
//! 2. every raw piece split at every crossing with every other one — segment / segment, segment / circle and circle /
//!    circle through geom_core's closed forms — the one crossing point handed to both;
//! 3. a piece kept iff its midpoint is no nearer than `d` to the boundary edges (an arc's midpoint is exactly `d` from
//!    its vertex) — the distance form of the winding rule on a validated domain (islands strictly inside the outline,
//!    no loop crossing another), so swallowtails, the pieces past a collapse and the pieces outside the domain all go,
//!    and outline and island rings merge with no special case;
//! 4. the kept pieces chained into closed rings of segments and arcs by their shared end points; a chain that does not
//!    close refuses `NoRings` (never an open path).
//!
//! Every ring keeps the domain on its left. Only geom names valid on both sides of the geom_core swap are used.

use alloc::vec::Vec;
use libm::{acos, ceil, cos, fabs, sin, sqrt};

use crate::geom::{Tol, Vec2, circle_circle2, line_circle2, line_line2, seg_seg2, signed_area2};
use crate::plan::arc_sweep; // VUGMAKEARCFIT (SR490): the one angle fold for every arc parameter
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

// ---- VUGMAKEARCFIT (LEDGER SR490): the offset's pieces are segments and exact arcs. ----

/// One piece of an offset ring: a segment, or an arc about `centre` (clockwise when `cw`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum El {
    Line { from: Vec2, to: Vec2 },
    Arc { from: Vec2, to: Vec2, centre: Vec2, cw: bool },
}

fn rot(w: Vec2, th: f64) -> Vec2 {
    Vec2::new(w.x * cos(th) - w.y * sin(th), w.x * sin(th) + w.y * cos(th))
}

impl El {
    pub(crate) fn start(&self) -> Vec2 {
        match *self {
            El::Line { from, .. } | El::Arc { from, .. } => from,
        }
    }

    pub(crate) fn end(&self) -> Vec2 {
        match *self {
            El::Line { to, .. } | El::Arc { to, .. } => to,
        }
    }

    /// The same carrier from `q` (a point on it).
    pub(crate) fn with_from(self, q: Vec2) -> El {
        match self {
            El::Line { to, .. } => El::Line { from: q, to },
            El::Arc { to, centre, cw, .. } => El::Arc { from: q, to, centre, cw },
        }
    }

    /// The same carrier to `q` (a point on it).
    pub(crate) fn with_to(self, q: Vec2) -> El {
        match self {
            El::Line { from, .. } => El::Line { from, to: q },
            El::Arc { from, centre, cw, .. } => El::Arc { from, to: q, centre, cw },
        }
    }

    pub(crate) fn reversed(self) -> El {
        match self {
            El::Line { from, to } => El::Line { from: to, to: from },
            El::Arc { from, to, centre, cw } => El::Arc { from: to, to: from, centre, cw: !cw },
        }
    }

    /// An arc's swept angle (0 for a segment).
    fn sweep(&self, tol: Tol) -> f64 {
        match *self {
            El::Line { .. } => 0.0,
            El::Arc { from, to, centre, cw } => arc_sweep(from, to, centre, cw, tol),
        }
    }

    fn length(&self, tol: Tol) -> f64 {
        match *self {
            El::Line { from, to } => len(to - from),
            El::Arc { from, centre, .. } => len(from - centre) * self.sweep(tol),
        }
    }

    pub(crate) fn mid(&self, tol: Tol) -> Vec2 {
        match *self {
            El::Line { from, to } => (from + to) * 0.5,
            El::Arc { from, centre, cw, .. } => {
                let h = 0.5 * self.sweep(tol);
                centre + rot(from - centre, if cw { -h } else { h })
            }
        }
    }

    /// The parameter of `q` (a point on this piece's carrier) when it lies on the closed piece: `t` along a segment,
    /// the angle swept from `from` along an arc.
    fn param(&self, q: Vec2, tol: Tol) -> Option<f64> {
        match *self {
            El::Line { from, to } => {
                let e = to - from;
                if pt_seg(q, from, to) <= tol.lin { Some((q - from).dot(e) / e.dot(e)) } else { None }
            }
            El::Arc { from, to, centre, cw } => {
                if len(q - from) <= tol.lin {
                    return Some(0.0);
                }
                let phi = arc_sweep(from, to, centre, cw, tol);
                if len(q - to) <= tol.lin {
                    return Some(phi);
                }
                let s = arc_sweep(from, q, centre, cw, tol);
                if s <= phi { Some(s) } else { None }
            }
        }
    }

    /// Whether `q` is more than `tol.lin` from both ends.
    fn interior(&self, q: Vec2, tol: Tol) -> bool {
        len(q - self.start()) > tol.lin && len(q - self.end()) > tol.lin
    }

    /// The point of the closed piece nearest `p`.
    pub(crate) fn closest(&self, p: Vec2, tol: Tol) -> Vec2 {
        match *self {
            El::Line { from, to } => foot(p, from, to),
            El::Arc { from, to, centre, .. } => {
                let w = p - centre;
                let l = len(w);
                if l > tol.lin {
                    let q = centre + w * (len(from - centre) / l);
                    if self.param(q, tol).is_some() {
                        return q;
                    }
                }
                if len(p - from) <= len(p - to) { from } else { to }
            }
        }
    }
}

/// The raw offset of one loop to its left by `d`, as a closed chain of pieces: per edge `i` the segment from vertex
/// i's exit to vertex i+1's entry, then vertex i+1's corner arc when it turns away from the offset (edge 0 first).
fn raw(lp: &[Vec2], d: f64, tol: Tol) -> Vec<El> {
    let n = lp.len();
    let dir: Vec<Vec2> = (0..n)
        .map(|i| {
            let e = lp[(i + 1) % n] - lp[i];
            e * (1.0 / len(e))
        })
        .collect();
    let nor = |i: usize| Vec2::new(-dir[i].y, dir[i].x) * d;
    let mut entry = alloc::vec![Vec2::default(); n];
    let mut exit = alloc::vec![Vec2::default(); n];
    let mut arc = alloc::vec![false; n];
    for j in 0..n {
        let i = (j + n - 1) % n; // the edge into vertex j
        let v = lp[j];
        let turn = dir[i].cross(dir[j]);
        if fabs(turn) <= tol.ang {
            entry[j] = v + nor(j);
            exit[j] = entry[j];
        } else if turn > 0.0 {
            // Toward the offset: the offset lines' intersection (past a collapse the edge reverses; step 3 drops it).
            match line_line2(lp[i] + nor(i), dir[i], v + nor(j), dir[j], tol) {
                Some(q) => {
                    entry[j] = q;
                    exit[j] = q;
                }
                None => {
                    entry[j] = v + nor(i);
                    exit[j] = v + nor(j);
                }
            }
        } else {
            // Away from the offset: the clockwise arc about v from nor(i) to nor(j), exact.
            entry[j] = v + nor(i);
            exit[j] = v + nor(j);
            arc[j] = true;
        }
    }
    let mut out = Vec::with_capacity(2 * n);
    for i in 0..n {
        let j = (i + 1) % n;
        out.push(El::Line { from: exit[i], to: entry[j] });
        if arc[j] {
            out.push(El::Arc { from: entry[j], to: exit[j], centre: lp[j], cw: true });
        } else if len(exit[j] - entry[j]) > 0.0 {
            out.push(El::Line { from: entry[j], to: exit[j] });
        }
    }
    out
}

/// Where two pieces' carriers meet (segment / segment only when the closed segments touch).
fn crossings(x: El, y: El, tol: Tol) -> ([Vec2; 2], usize) {
    match (x, y) {
        (El::Line { from: p1, to: p2 }, El::Line { from: p3, to: p4 }) => {
            if !seg_seg2(p1, p2, p3, p4, tol) {
                return ([p1; 2], 0);
            }
            match line_line2(p1, p2 - p1, p3, p4 - p3, tol) {
                Some(q) => ([q, q], 1),
                None => ([p1; 2], 0),
            }
        }
        (El::Line { from, to }, El::Arc { from: s, centre, .. })
        | (El::Arc { from: s, centre, .. }, El::Line { from, to }) => line_circle2(from, to, centre, len(s - centre), tol),
        (El::Arc { from: s1, centre: c1, .. }, El::Arc { from: s2, centre: c2, .. }) => {
            circle_circle2(c1, len(s1 - c1), c2, len(s2 - c2), tol)
        }
    }
}

/// A closed ring's exact signed area (the chord polygon plus each arc's circular segment) and its length.
pub(crate) fn measure(ring: &[El], tol: Tol) -> (f64, f64) {
    let (mut a, mut per) = (0.0, 0.0);
    for e in ring {
        a += 0.5 * e.start().cross(e.end());
        per += e.length(tol);
        if let El::Arc { from, centre, cw, .. } = *e {
            let w = from - centre;
            let th = e.sweep(tol);
            let seg = 0.5 * w.dot(w) * (th - sin(th));
            a += if cw { -seg } else { seg };
        }
    }
    (a, per)
}

/// The rings at distance `d` from the boundary `lps` (from [`loops`] or [`side_loop`]), each closed, of segments and
/// exact arcs, with the domain on its left; empty when nothing of the domain is that far from its boundary.
pub(crate) fn rings(lps: &[Vec<Vec2>], d: f64, tol: Tol) -> Result<Vec<Vec<El>>, Refusal> {
    let mut segs: Vec<El> = Vec::new();
    for lp in lps {
        for e in raw(lp, d, tol) {
            if len(e.end() - e.start()) > tol.lin {
                segs.push(e);
            }
        }
    }
    let pieces: Vec<El> =
        split(&segs, tol).into_iter().map(|(_, e)| e).filter(|e| dist(e.mid(tol), lps) >= d - tol.lin).collect();
    chain(&pieces, tol)
}

/// Every piece of `segs` split at every crossing with every other one (segment / segment, segment / circle and circle /
/// circle; the one crossing point handed to both), as (source index, piece) in source order, along each source; a
/// piece no longer than `tol.lin` merges into the next.
pub(crate) fn split(segs: &[El], tol: Tol) -> Vec<(usize, El)> {
    let k = segs.len();
    let mut cuts: Vec<Vec<(f64, Vec2)>> = alloc::vec![Vec::new(); k];
    for a in 0..k {
        for b in (a + 1)..k {
            let (pts, m) = crossings(segs[a], segs[b], tol);
            for &q in &pts[..m] {
                let (Some(ta), Some(tb)) = (segs[a].param(q, tol), segs[b].param(q, tol)) else { continue };
                if segs[a].interior(q, tol) {
                    cuts[a].push((ta, q));
                }
                if segs[b].interior(q, tol) {
                    cuts[b].push((tb, q));
                }
            }
        }
    }
    let mut pieces: Vec<(usize, El)> = Vec::new();
    for (s, c) in cuts.iter_mut().enumerate() {
        let e = segs[s];
        c.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap_or(core::cmp::Ordering::Equal));
        let mut from = e.start();
        for q in c.iter().map(|&(_, q)| q).chain(core::iter::once(e.end())) {
            if len(q - from) > tol.lin {
                pieces.push((s, e.with_from(from).with_to(q)));
                from = q;
            }
        }
    }
    pieces
}

/// The pieces chained into closed rings by their shared end points (first free start wins); a chain that does not
/// close refuses `NoRings` (never an open path); a ring of under two pieces or no more area than `tol.lin` times its
/// length is dropped.
pub(crate) fn chain(pieces: &[El], tol: Tol) -> Result<Vec<Vec<El>>, Refusal> {
    let mut used = alloc::vec![false; pieces.len()];
    let mut out = Vec::new();
    for s in 0..pieces.len() {
        if used[s] {
            continue;
        }
        used[s] = true;
        let start = pieces[s].start();
        let mut ring = alloc::vec![pieces[s]];
        let mut at = pieces[s].end();
        while !near(at, start, tol) {
            let Some(nx) = (0..pieces.len()).find(|&c| !used[c] && near(pieces[c].start(), at, tol)) else {
                return Err(Refusal::NoRings);
            };
            used[nx] = true;
            ring.push(pieces[nx]);
            at = pieces[nx].end();
        }
        let (area, per) = measure(&ring, tol);
        if ring.len() >= 2 && fabs(area) > tol.lin * per {
            out.push(ring);
        }
    }
    Ok(out)
}

/// Contour's one loop: the profile oriented so the side to cut is on its left (Inside counter-clockwise, Outside
/// clockwise); a profile that crosses itself refuses `ContourSelfIntersects`.
pub(crate) fn side_loop(profile: &[Vec2], outside: bool, tol: Tol) -> Result<Vec<Vec<Vec2>>, Refusal> {
    let l = normalise(profile, !outside, tol)?;
    if self_crosses(&l, tol) {
        return Err(Refusal::ContourSelfIntersects);
    }
    Ok(alloc::vec![l])
}

/// The ring as a closed polyline, each arc CIRCUMSCRIBED (every point at least its radius from the centre, at most
/// `chord` of bulge past the arc): for a consumer that cannot say an arc (slice's perimeters).
pub(crate) fn flatten(ring: &[El], chord: f64, tol: Tol) -> Vec<Vec2> {
    let mut out = Vec::new();
    for e in ring {
        out.push(e.start());
        if let El::Arc { from, centre, cw, .. } = *e {
            let w = from - centre;
            let r = len(w);
            let phi = e.sweep(tol);
            // The half angle of one circumscribed chord whose bulge past the arc is `chord`.
            let h = acos(r / (r + chord));
            let m = ceil(phi / (2.0 * h)).max(1.0);
            let step = phi / m;
            let k_out = 1.0 / cos(0.5 * step);
            let sg = if cw { -1.0 } else { 1.0 };
            let mut k = 1.0;
            while k <= m {
                out.push(centre + rot(w, sg * (k - 0.5) * step) * k_out);
                k += 1.0;
            }
        }
    }
    out
}

/// The same ring run the other way.
pub(crate) fn reversed_ring(ring: &[El]) -> Vec<El> {
    ring.iter().rev().map(|e| e.reversed()).collect()
}

/// The nearest point of the closed `ring` to `p`: (its distance, the piece index, the point).
pub(crate) fn nearest(p: Vec2, ring: &[El], tol: Tol) -> (f64, usize, Vec2) {
    let mut best = (f64::INFINITY, 0, ring[0].start());
    for (i, e) in ring.iter().enumerate() {
        let q = e.closest(p, tol);
        let dd = len(p - q);
        if dd < best.0 {
            best = (dd, i, q);
        }
    }
    best
}

/// The same closed ring, in the same direction, starting at `q` on its piece `e` and ending exactly at `q`.
pub(crate) fn start_at(ring: &[El], e: usize, q: Vec2, tol: Tol) -> Vec<El> {
    let n = ring.len();
    let mut out = Vec::with_capacity(n + 1);
    let head = ring[e].with_from(q);
    if len(head.end() - q) > tol.lin {
        out.push(head);
    }
    for k in 1..n {
        out.push(ring[(e + k) % n]);
    }
    let tail = ring[e].with_to(q);
    if len(q - tail.start()) > tol.lin {
        out.push(tail);
    } else if let Some(l) = out.last_mut() {
        *l = l.with_to(q);
    }
    if let Some(f) = out.first_mut() {
        *f = f.with_from(q);
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

// ---- VUGMAKEREST (LEDGER SR517): the offset of a ring of pieces, for the rest a pocket leaves. ----

/// The unit tangent of `e` at `q` (a point on it), in its running direction.
fn tangent(e: &El, q: Vec2) -> Vec2 {
    match *e {
        El::Line { from, to } => (to - from) * (1.0 / len(to - from)),
        El::Arc { centre, cw, .. } => {
            let w = q - centre;
            let t = Vec2::new(-w.y, w.x) * (1.0 / len(w));
            if cw { t * -1.0 } else { t }
        }
    }
}

/// The unit normal to the left of `e` at `q` (a point on it).
pub(crate) fn left_normal(e: &El, q: Vec2) -> Vec2 {
    let t = tangent(e, q);
    Vec2::new(-t.y, t.x)
}

/// One piece moved `d` to its right: (the new start, the new end, the piece). A segment shifts by its right normal; an
/// arc keeps its centre, a clockwise one (centre on the right) at radius rho - d, a counter-clockwise one at rho + d;
/// a clockwise arc whose new radius is no more than `tol.lin` collapses to its centre (no piece).
fn shifted(e: El, d: f64, tol: Tol) -> (Vec2, Vec2, Option<El>) {
    match e {
        El::Line { from, to } => {
            let u = (to - from) * (1.0 / len(to - from));
            let n = Vec2::new(u.y, -u.x) * d;
            (from + n, to + n, Some(El::Line { from: from + n, to: to + n }))
        }
        El::Arc { from, to, centre, cw } => {
            let rho = len(from - centre);
            let r = if cw { rho - d } else { rho + d };
            if r <= tol.lin {
                return (centre, centre, None);
            }
            let k = r / rho;
            let (a, b) = (centre + (from - centre) * k, centre + (to - centre) * (r / len(to - centre)));
            (a, b, Some(El::Arc { from: a, to: b, centre, cw }))
        }
    }
}

/// The raw offset of the closed `ring` by `d` to its right, as a closed chain of pieces (unsplit, untrimmed: the caller
/// splits with [`split`] and keeps by distance). Each piece is [`shifted`]; where two shifted neighbours do not meet,
/// the gap at the ring's junction `J` is filled by the arc about `J` of radius `d` (counter-clockwise) when the ring
/// turns left there, a reversal (a cusp tip) counting as left, else by the PIVOT (two segments through `J`, every point
/// of them nearer than `d` to the ring, so a distance keep drops them). Every clockwise arc fed in has a radius of at
/// least `d - tol.lin` (the rest's rings and regions), so the collapse is the one degenerate case.
pub(crate) fn offset_right(ring: &[El], d: f64, tol: Tol) -> Vec<El> {
    let n = ring.len();
    let sh: Vec<(Vec2, Vec2, Option<El>)> = ring.iter().map(|e| shifted(*e, d, tol)).collect();
    let mut out = Vec::with_capacity(3 * n);
    for i in 0..n {
        if let Some(e) = sh[i].2 {
            out.push(e);
        }
        let j = (i + 1) % n;
        let (a, b) = (sh[i].1, sh[j].0);
        if len(b - a) <= 2.0 * tol.lin {
            continue;
        }
        let jn = ring[i].end();
        let (u, v) = (tangent(&ring[i], jn), tangent(&ring[j], ring[j].start()));
        let c = u.cross(v);
        if c > tol.ang || (fabs(c) <= tol.ang && u.dot(v) < 0.0) {
            out.push(El::Arc { from: a, to: b, centre: jn, cw: false });
        } else {
            out.push(El::Line { from: a, to: jn });
            out.push(El::Line { from: jn, to: b });
        }
    }
    out.retain(|e| len(e.end() - e.start()) > tol.lin);
    out
}

/// The distance from `p` to the nearest piece of `rings` (infinite when there is none).
pub(crate) fn dist_els(p: Vec2, rings: &[Vec<El>], tol: Tol) -> f64 {
    let mut m = f64::INFINITY;
    for ring in rings {
        for e in ring {
            m = m.min(len(p - e.closest(p, tol)));
        }
    }
    m
}
