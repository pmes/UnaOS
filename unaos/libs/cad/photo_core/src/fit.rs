// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Step 6: primitives. A loop is simplified by Douglas-Peucker at 2 σ; a DP vertex turning at least [`CORNER_DEG`]
//! is a corner. Between two corners the run is ONE line when its total-least-squares fit holds every point within
//! 3 σ; otherwise lines grow from the DP segments at least a pitch long while the TLS fit holds, and each stretch left
//! is an arc when Pratt's fit holds (else it is split at its middle DP vertex, a single DP segment being a line).
//! The points within pitch / 8 of a corner (the morphology's rounding) are left out of the fits; two corners closer
//! than that are merged unless they end a thin side (the sides beyond them parallel and over 2 σ apart, SR655). A
//! hole that is one circle within 3 σ is a circle. Pratt's circle (Pratt, "Direct least-squares fitting of algebraic surfaces",
//! SIGGRAPH 1987) is solved by Chernov's Newton iteration on its characteristic polynomial (Chernov, "Circular and
//! Linear Regression", 2010, §5), started at Kåsa's solution (x = 0; Kåsa, IEEE Trans. Instrum. Meas. 1976).
//!
//! Junctions use geom_core's closed forms: line / line `line_line2`; line / arc TANGENT when the centre's distance
//! to the line is the radius within 3 σ (the junction is the centre's foot on the line), else the `line_circle2`
//! crossing nearer the boundary; arc / arc tangent when the centre distance is the sum or difference of the radii
//! within 3 σ, else `circle_circle2`'s nearer crossing.

use alloc::vec;
use alloc::vec::Vec;

use geom_core::{
    Circle2, CircleCircle, Line2, LineCircle, LineLine2, Predicate, circle_circle2, line_circle2, line_line2, orient2d,
};

use crate::contour::{Loops, dp_closed};
use crate::grid::tls;
use crate::{Refusal, Tol, Vec2};

/// A DP vertex turning at least this much is a corner (degrees).
pub const CORNER_DEG: f64 = 35.0;
/// Every acceptance and inference band is this many σ.
pub const BAND_SIGMA: f64 = 3.0;

/// A fitted primitive of an outline loop, in paper mm, in the loop's direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prim {
    Line { a: Vec2, b: Vec2, rms: f64 },
    /// From `from` to `to` about `centre`, counterclockwise when `ccw`; `tangent_in` / `tangent_out`: the junction
    /// with the previous / next primitive is tangent.
    Arc { centre: Vec2, r: f64, from: Vec2, to: Vec2, ccw: bool, tangent_in: bool, tangent_out: bool, rms: f64 },
    Circle { centre: Vec2, r: f64, rms: f64 },
}

/// The fitted outline: the outer loop (CCW), the holes (CW), and the σ it was fitted at.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub outer: Vec<Prim>,
    pub holes: Vec<Vec<Prim>>,
    pub sigma_mm: f64,
}

impl Outline {
    /// (lines, arcs, circles) over every loop.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for p in self.outer.iter().chain(self.holes.iter().flatten()) {
            match p {
                Prim::Line { .. } => c.0 += 1,
                Prim::Arc { .. } => c.1 += 1,
                Prim::Circle { .. } => c.2 += 1,
            }
        }
        c
    }
}

/// A TLS line: centroid, unit direction, rms and max |distance|.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineFit {
    pub c: Vec2,
    pub d: Vec2,
    pub rms: f64,
    pub max: f64,
}

impl LineFit {
    pub fn line2(&self) -> Line2 {
        Line2 { point: self.c, dir: self.d }
    }
}

pub fn fit_line(pts: &[Vec2]) -> Option<LineFit> {
    let (c, d) = tls(pts)?;
    let n = d.perp();
    let (mut s, mut max) = (0.0, 0.0f64);
    for &p in pts {
        let e = (p - c).dot(n).abs();
        s += e * e;
        max = max.max(e);
    }
    Some(LineFit { c, d, rms: libm::sqrt(s / pts.len() as f64), max })
}

/// A fitted circle: centre, radius, rms and max | |p − c| − r |.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CircleFit {
    pub c: Vec2,
    pub r: f64,
    pub rms: f64,
    pub max: f64,
}

/// Pratt's algebraic circle fit (Chernov's Newton, from Kåsa).
pub fn fit_circle(pts: &[Vec2]) -> Option<CircleFit> {
    let n = pts.len();
    if n < 3 {
        return None;
    }
    let m = pts.iter().fold(Vec2::ZERO, |a, &p| a + p) / n as f64;
    let (mut mxx, mut myy, mut mxy, mut mxz, mut myz, mut mzz) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for &p in pts {
        let (x, y) = (p.x - m.x, p.y - m.y);
        let z = x * x + y * y;
        mxx += x * x;
        myy += y * y;
        mxy += x * y;
        mxz += x * z;
        myz += y * z;
        mzz += z * z;
    }
    let k = n as f64;
    let (mxx, myy, mxy, mxz, myz, mzz) = (mxx / k, myy / k, mxy / k, mxz / k, myz / k, mzz / k);
    let mz = mxx + myy;
    let cov_xy = mxx * myy - mxy * mxy;
    let (mxz2, myz2) = (mxz * mxz, myz * myz);
    let a2 = 4.0 * cov_xy - 3.0 * mz * mz - mzz;
    let a1 = mzz * mz + 4.0 * cov_xy * mz - mxz2 - myz2 - mz * mz * mz;
    let a0 = mxz2 * myy + myz2 * mxx - mzz * cov_xy - 2.0 * mxz * myz * mxy + mz * mz * cov_xy;
    let a22 = a2 + a2;
    let (mut x, mut y) = (0.0f64, 1e20f64);
    for _ in 0..20 {
        let yold = y;
        y = a0 + x * (a1 + x * (a2 + 4.0 * x * x));
        if y.abs() > yold.abs() {
            x = 0.0;
            break;
        }
        let dy = a1 + x * (a22 + 16.0 * x * x);
        if dy == 0.0 {
            break;
        }
        let xold = x;
        x = xold - y / dy;
        if x != 0.0 && ((x - xold) / x).abs() < 1e-12 {
            break;
        }
        if x < 0.0 {
            x = 0.0;
        }
    }
    let solve = |x: f64| -> Option<(Vec2, f64)> {
        let det = x * x - x * mz + cov_xy;
        if !(det.abs() > 1e-300) {
            return None;
        }
        let cx = (mxz * (myy - x) - myz * mxy) / det / 2.0;
        let cy = (myz * (mxx - x) - mxz * mxy) / det / 2.0;
        let r = libm::sqrt(cx * cx + cy * cy + mz + 2.0 * x);
        let c = Vec2::new(cx + m.x, cy + m.y);
        if c.is_finite() && r.is_finite() && r > 0.0 { Some((c, r)) } else { None }
    };
    let (c, r) = solve(x).or_else(|| solve(0.0))?;
    let (mut s, mut max) = (0.0, 0.0f64);
    for &p in pts {
        let e = (p.dist(c) - r).abs();
        s += e * e;
        max = max.max(e);
    }
    Some(CircleFit { c, r, rms: libm::sqrt(s / k), max })
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    L(LineFit),
    A(CircleFit),
}

/// A run's piece: its fit and its ring offsets.
#[derive(Clone, Copy, Debug)]
struct Piece {
    kind: Kind,
    from: usize,
    to: usize,
}

fn clip(ring: &[Vec2], lo: usize, hi: usize, a: usize, b: usize) -> &[Vec2] {
    let (x, y) = (a.max(lo), b.min(hi));
    if x + 1 < y { &ring[x..=y] } else { &ring[a..=b] }
}

fn line_ok(pts: &[Vec2], band: f64) -> Option<LineFit> {
    fit_line(pts).filter(|f| f.max <= band)
}

#[allow(clippy::too_many_arguments)]
fn arcs(ring: &[Vec2], dpo: &[usize], sa: usize, sb: usize, lo: usize, hi: usize, band: f64, out: &mut Vec<Piece>) {
    let (a, b) = (dpo[sa], dpo[sb + 1]);
    if sa == sb {
        if let Some(f) = fit_line(clip(ring, lo, hi, a, b)) {
            out.push(Piece { kind: Kind::L(f), from: a, to: b });
        }
        return;
    }
    if let Some(c) = fit_circle(clip(ring, lo, hi, a, b)).filter(|c| c.max <= band) {
        out.push(Piece { kind: Kind::A(c), from: a, to: b });
        return;
    }
    let mid = (sa + sb) / 2;
    arcs(ring, dpo, sa, mid, lo, hi, band, out);
    arcs(ring, dpo, mid + 1, sb, lo, hi, band, out);
}

/// One corner-to-corner run (DP vertex offsets `dpo` into `ring`) into pieces.
fn segment_run(ring: &[Vec2], dpo: &[usize], band: f64, pitch: f64, trim: f64) -> Vec<Piece> {
    let (s0, e0) = (dpo[0], dpo[dpo.len() - 1]);
    let (mut lo, mut acc) = (s0, 0.0);
    while lo < e0 && acc < trim {
        acc += ring[lo + 1].dist(ring[lo]);
        lo += 1;
    }
    let (mut hi, mut acc2) = (e0, 0.0);
    while hi > s0 && acc2 < trim {
        acc2 += ring[hi].dist(ring[hi - 1]);
        hi -= 1;
    }
    if lo + 2 >= hi {
        lo = s0;
        hi = e0;
    }
    if let Some(f) = line_ok(clip(ring, lo, hi, s0, e0), band) {
        return vec![Piece { kind: Kind::L(f), from: s0, to: e0 }];
    }
    let ns = dpo.len() - 1;
    let seg_len = |s: usize| ring[dpo[s]].dist(ring[dpo[s + 1]]);
    let mut owner: Vec<Option<usize>> = vec![None; ns];
    let mut seeds: Vec<usize> = (0..ns).filter(|&s| seg_len(s) >= pitch).collect();
    seeds.sort_by(|&a, &b| seg_len(b).total_cmp(&seg_len(a)));
    let mut nl = 0;
    for s in seeds {
        if owner[s].is_some() || line_ok(clip(ring, lo, hi, dpo[s], dpo[s + 1]), band).is_none() {
            continue;
        }
        let (mut a, mut b) = (s, s);
        loop {
            let mut grew = false;
            if a > 0 && owner[a - 1].is_none() && line_ok(clip(ring, lo, hi, dpo[a - 1], dpo[b + 1]), band).is_some() {
                a -= 1;
                grew = true;
            }
            if b + 1 < ns && owner[b + 1].is_none() && line_ok(clip(ring, lo, hi, dpo[a], dpo[b + 2]), band).is_some() {
                b += 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }
        for o in owner.iter_mut().take(b + 1).skip(a) {
            *o = Some(nl);
        }
        nl += 1;
    }
    let mut out = Vec::new();
    let mut k = 0;
    while k < ns {
        let mut e = k;
        while e + 1 < ns && owner[e + 1] == owner[k] {
            e += 1;
        }
        match owner[k] {
            Some(_) => {
                if let Some(f) = fit_line(clip(ring, lo, hi, dpo[k], dpo[e + 1])) {
                    out.push(Piece { kind: Kind::L(f), from: dpo[k], to: dpo[e + 1] });
                }
            }
            None => arcs(ring, dpo, k, e, lo, hi, band, &mut out),
        }
        k = e + 1;
    }
    out
}

fn nearer(a: Vec2, b: Vec2, hint: Vec2) -> Vec2 {
    if a.dist(hint) <= b.dist(hint) { a } else { b }
}

/// The junction of two consecutive pieces and whether it is tangent.
fn junction(p: Kind, q: Kind, hint: Vec2, band: f64, pitch: f64) -> (Vec2, bool) {
    let tol = Tol::DEFAULT;
    let line_arc = |l: LineFit, c: CircleFit| -> (Vec2, bool) {
        let n = l.d.perp();
        let dd = (c.c - l.c).dot(n);
        let foot = c.c - n * dd;
        if (dd.abs() - c.r).abs() <= band {
            return (foot, true);
        }
        let circle = Circle2 { centre: c.c, radius: c.r };
        let x = match line_circle2(&l.line2(), &circle, tol) {
            LineCircle::Two(a, b) => nearer(a, b, hint),
            LineCircle::Tangent(t) => t,
            LineCircle::Miss => foot,
        };
        (x, false)
    };
    match (p, q) {
        (Kind::L(a), Kind::L(b)) => match line_line2(&a.line2(), &b.line2(), tol) {
            LineLine2::Point { p, .. } if p.dist(hint) <= pitch => (p, false),
            _ => (hint, false),
        },
        (Kind::L(l), Kind::A(c)) | (Kind::A(c), Kind::L(l)) => line_arc(l, c),
        (Kind::A(a), Kind::A(b)) => {
            let w = b.c - a.c;
            let d = w.mag();
            if d > 0.0 && ((d - (a.r + b.r)).abs() <= band || (d - (a.r - b.r).abs()).abs() <= band) {
                let u = w / d;
                let external = (d - (a.r + b.r)).abs() <= (d - (a.r - b.r).abs()).abs();
                let pt = if external || a.r >= b.r { a.c + u * a.r } else { a.c - u * a.r };
                return (pt, true);
            }
            let ca = Circle2 { centre: a.c, radius: a.r };
            let cb = Circle2 { centre: b.c, radius: b.r };
            let x = match circle_circle2(&ca, &cb, tol) {
                CircleCircle::Two(s, t) => nearer(s, t, hint),
                CircleCircle::Tangent(t) => t,
                _ => hint,
            };
            (x, false)
        }
    }
}

/// One closed loop (dense, paper mm) into primitives at σ.
pub fn segment_loop(pts: &[Vec2], sigma: f64, pitch: f64) -> Result<Vec<Prim>, Refusal> {
    let band = BAND_SIGMA * sigma;
    let n = pts.len();
    let idx = dp_closed(pts, 2.0 * sigma);
    let m = idx.len();
    if m < 3 {
        return Err(Refusal::Uncertain);
    }
    let turn: Vec<f64> = (0..m)
        .map(|k| {
            let a = pts[idx[(k + m - 1) % m]];
            let b = pts[idx[k]];
            let c = pts[idx[(k + 1) % m]];
            let (u, v) = (b - a, c - b);
            libm::atan2(u.cross(v), u.dot(v)).abs() * 180.0 / core::f64::consts::PI
        })
        .collect();
    let mut corners: Vec<usize> = (0..m).filter(|&k| turn[k] >= CORNER_DEG).collect();
    // Two corners closer than the morphology's rounding (pitch / 8) are ONE corner seen as two half turns (the crack
    // midpoints cut a square corner's tip): keep the sharper. VUGPHOTOSIDETHINFIT (SR655): unless the pair is a thin
    // side's end (`thin_pair`: its outer sides parallel and further apart than 2 σ), which is a feature, not noise.
    let trim = pitch / 8.0;
    while corners.len() >= 2 {
        let nc = corners.len();
        let Some(q) = (0..nc).find(|&q| pts[idx[corners[q]]].dist(pts[idx[corners[(q + 1) % nc]]]) < trim && !thin_pair(pts, &idx, &corners, q, sigma, band, trim)) else { break };
        let drop = if turn[corners[q]] >= turn[corners[(q + 1) % nc]] { (q + 1) % nc } else { q };
        corners.remove(drop);
    }
    if corners.is_empty() {
        if let Some(c) = fit_circle(pts).filter(|c| c.max <= band) {
            return Ok(vec![Prim::Circle { centre: c.c, r: c.r, rms: c.rms }]);
        }
        let k = (0..m).max_by(|&a, &b| turn[a].total_cmp(&turn[b])).unwrap_or(0);
        corners.push(k);
    }
    let rot = idx[corners[0]];
    let mut ring: Vec<Vec2> = pts[rot..].to_vec();
    ring.extend_from_slice(&pts[..rot]);
    ring.push(pts[rot]);
    let mut dpo: Vec<usize> = (0..m).map(|k| (idx[(corners[0] + k) % m] + n - rot) % n).collect();
    dpo.push(n);
    let mut cpos: Vec<usize> = corners.iter().map(|&c| (c + m - corners[0]) % m).collect();
    cpos.sort_unstable();
    cpos.push(m);
    let mut pieces: Vec<Piece> = Vec::new();
    for w in cpos.windows(2) {
        pieces.extend(segment_run(&ring, &dpo[w[0]..=w[1]], band, pitch, trim));
    }
    let np = pieces.len();
    if np == 0 {
        return Err(Refusal::Uncertain);
    }
    if np == 1 {
        return match pieces[0].kind {
            Kind::A(c) => Ok(vec![Prim::Circle { centre: c.c, r: c.r, rms: c.rms }]),
            Kind::L(_) => Err(Refusal::Uncertain),
        };
    }
    let joins: Vec<(Vec2, bool)> =
        (0..np).map(|k| junction(pieces[k].kind, pieces[(k + 1) % np].kind, ring[pieces[k].to], band, pitch)).collect();
    let mut out = Vec::with_capacity(np);
    for k in 0..np {
        let (from, t_in) = joins[(k + np - 1) % np];
        let (to, t_out) = joins[k];
        let pc = pieces[k];
        out.push(match pc.kind {
            Kind::L(f) => Prim::Line { a: from, b: to, rms: f.rms },
            Kind::A(c) => {
                let mid = ring[(pc.from + pc.to) / 2];
                let ccw = orient2d(ring[pc.from], mid, ring[pc.to]) == Predicate::Positive;
                Prim::Arc { centre: c.c, r: c.r, from, to, ccw, tangent_in: t_in, tangent_out: t_out, rms: c.rms }
            }
        });
    }
    Ok(out)
}

/// The outline of a silhouette's loops at σ.
pub fn outline(loops: &Loops, sigma: f64, pitch: f64) -> Result<Outline, Refusal> {
    let band = BAND_SIGMA * sigma;
    let outer = segment_loop(&loops.outer, sigma, pitch)?;
    let mut holes = Vec::new();
    for h in &loops.holes {
        holes.push(match fit_circle(h).filter(|c| c.max <= band) {
            Some(c) => vec![Prim::Circle { centre: c.c, r: c.r, rms: c.rms }],
            None => segment_loop(h, sigma, pitch)?,
        });
    }
    Ok(Outline { outer, holes, sigma_mm: sigma })
}

// ---- VUGPHOTOSIDETHINFIT (SR655): a close corner pair between parallel sides is a thin side (named tail block) ----
// The corner merge above takes two corners closer than pitch / 8 for one corner seen as two half turns. A thin part
// (a 0.5 mm shim) has its END that short: its two corners are real, and merging them turns the two parallel sides and
// the end into a spike. The ONE rule reads the fit's own numbers: the pair is kept when the side before it and the side
// after it (each from the neighbouring corner, at least `trim` long: real sides, not rounding slivers) run back
// against each other (anti-parallel: a spike or notch, not a step), are parallel within the band over the shorter
// side, and lie further apart than 2 σ (the DP's own tolerance: a thin side has an outline's width of evidence along
// its run). A rounded corner split in two has perpendicular sides and still merges.

/// Whether the close corner pair (`corners[q]`, `corners[q + 1]`) of the DP vertices `idx` into `pts` ends a thin side.
fn thin_pair(pts: &[Vec2], idx: &[usize], corners: &[usize], q: usize, sigma: f64, band: f64, trim: f64) -> bool {
    let nc = corners.len();
    if nc < 4 {
        return false;
    }
    let at = |k: usize| pts[idx[corners[k % nc]]];
    let (p0, p1, q0, q1) = (at(q + nc - 1), at(q), at(q + 1), at(q + 2));
    let (u, v) = (p1 - p0, q1 - q0);
    let (lu, lv) = (u.mag(), v.mag());
    if !(lu >= trim && lv >= trim) || u.dot(v) >= 0.0 {
        return false;
    }
    let (uh, vh) = (u / lu, v / lv);
    uh.cross(vh).abs() * lu.min(lv) <= band && (q0 - p1).cross(uh).abs() > 2.0 * sigma
}
