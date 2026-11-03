// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Step 2: the grid. Lines by a gradient-oriented Hough transform (Duda & Hart, CACM 1972; O'Gorman & Clowes, IEEE
//! Trans. Computers 1976): each edgel votes only near its own gradient direction, over the full circle so the
//! polarity is kept (θ in 0.5° bins over [0, 360), ρ in 1 px bins); peaks are 5 x 5 local maxima of at least
//! min(w, h) / 8 votes, each refined by total least squares over the edgels within 1.5 px whose direction agrees
//! within 6°. An ink stroke is TWO such edge lines of opposite polarity whose normals point away from each other
//! (the gradient points to the brighter paper on both sides), at most [`MAX_STROKE_PX`] apart: the pair gives one
//! centre line and the stroke width. A part's edge has no partner and drops out.
//!
//! The two families: the centre lines' angles mod 180° histogrammed in 1° bins weighted by support; the top peak
//! and the top peak at least 45° from it; a line joins the nearer peak within 25° (perspective spreads a family).
//! Each family is ordered along the other family's best-supported line; consecutive lines step the lattice index
//! by the spacing over the local median spacing (a line hidden end to end keeps the count honest); a line off the
//! lattice by more than 0.3 of a step is dropped. Intersections are geom_core's `line_line2`, kept when inside the
//! photo and SEEN: both lines have edge support within half a pitch of the crossing.

use alloc::vec;
use alloc::vec::Vec;

use geom_core::{Line2, LineLine2, line_line2};

use crate::edges::Edgel;
use crate::{MIN_LINES, Refusal, Tol, Vec2};

/// θ bins over the full circle.
const NT: usize = 720;
/// Each edgel votes in its own θ bin and this many on either side (±5°: the gradient direction's noise).
const VOTE_SPAN: i64 = 10;
/// Refinement: the band (px) and the direction agreement (degrees).
const BAND_PX: f64 = 1.5;
const AGREE_DEG: f64 = 6.0;
/// The widest ink stroke paired (px).
pub const MAX_STROKE_PX: f64 = 20.0;
/// Two opposite edges pair when their normals are anti-parallel within this (degrees).
const PAIR_DEG: f64 = 2.0;
/// A line joins a family within this of the family's peak (degrees).
const FAMILY_DEG: f64 = 25.0;
/// The second family's peak lies at least this far from the first (degrees).
const FAMILY_SEP_DEG: f64 = 45.0;
/// A line is off the lattice when its spacing is further than this from a whole number of steps.
const STEP_SLACK: f64 = 0.3;

/// One refined edge line: the unit normal `n` (the gradient side), `n · p = rho`, its supporting edgels.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeLine {
    pub n: Vec2,
    pub rho: f64,
    pub c: Vec2,
    pub pts: Vec<Vec2>,
}

/// One ink stroke's centre line: a point, the unit direction, the stroke width (px) and the sorted parameters of
/// its edge support along `dir` from `point`.
#[derive(Clone, Debug, PartialEq)]
pub struct GridLine {
    pub point: Vec2,
    pub dir: Vec2,
    pub width_px: f64,
    pub support: Vec<f64>,
}

impl GridLine {
    pub fn line2(&self) -> Line2 {
        Line2 { point: self.point, dir: self.dir }
    }

    /// Whether the edge support reaches within `r` of parameter `t`.
    pub fn seen_near(&self, t: f64, r: f64) -> bool {
        let k = self.support.partition_point(|&x| x < t - r);
        k < self.support.len() && self.support[k] <= t + r
    }
}

/// A seen lattice intersection: photo px and its lattice index (i in family a, j in family b).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inter {
    pub px: Vec2,
    pub i: i32,
    pub j: i32,
}

/// The grid: both families with their lattice indices, the median spacing and stroke width (px), the seen
/// intersections.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub a: Vec<(i32, GridLine)>,
    pub b: Vec<(i32, GridLine)>,
    pub pitch_px: f64,
    pub width_px: f64,
    pub inter: Vec<Inter>,
}

fn ang_diff(a: f64, b: f64) -> f64 {
    let two_pi = 2.0 * core::f64::consts::PI;
    let mut d = (a - b) % two_pi;
    if d > core::f64::consts::PI {
        d -= two_pi;
    } else if d < -core::f64::consts::PI {
        d += two_pi;
    }
    d.abs()
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// TLS over `pts`: the centroid and the unit direction of the major axis.
pub fn tls(pts: &[Vec2]) -> Option<(Vec2, Vec2)> {
    if pts.len() < 2 {
        return None;
    }
    let n = pts.len() as f64;
    let c = pts.iter().fold(Vec2::ZERO, |a, &p| a + p) / n;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for &p in pts {
        let d = p - c;
        sxx += d.x * d.x;
        sxy += d.x * d.y;
        syy += d.y * d.y;
    }
    let phi = 0.5 * libm::atan2(2.0 * sxy, sxx - syy);
    Some((c, Vec2::new(libm::cos(phi), libm::sin(phi))))
}

fn refine(el: &[Edgel], theta: f64, rho0: f64, min_support: usize) -> Option<EdgeLine> {
    let mut n = Vec2::new(libm::cos(theta), libm::sin(theta));
    let mut rho = rho0;
    let agree = AGREE_DEG * core::f64::consts::PI / 180.0;
    let mut pts = Vec::new();
    let mut c = Vec2::ZERO;
    for pass in 0..3 {
        let band = if pass == 0 { 2.0 } else { BAND_PX };
        let th = libm::atan2(n.y, n.x);
        pts.clear();
        for e in el {
            if (n.dot(e.p) - rho).abs() <= band && ang_diff(e.theta, th) <= agree {
                pts.push(e.p);
            }
        }
        if pts.len() < min_support {
            return None;
        }
        let (cc, d) = tls(&pts)?;
        let mut m = d.perp();
        if m.dot(n) < 0.0 {
            m = -m;
        }
        n = m;
        c = cc;
        rho = n.dot(c);
    }
    Some(EdgeLine { n, rho, c, pts })
}

/// The edge lines of a photo `w` x `h` px.
pub fn hough_lines(el: &[Edgel], w: usize, h: usize) -> Vec<EdgeLine> {
    let diag = libm::ceil(libm::hypot(w as f64, h as f64)) as i64;
    let nr = (2 * diag + 1) as usize;
    let dt = 2.0 * core::f64::consts::PI / NT as f64;
    let cs: Vec<(f64, f64)> = (0..NT).map(|t| (libm::cos(t as f64 * dt), libm::sin(t as f64 * dt))).collect();
    let mut votes = vec![0u32; NT * nr];
    for e in el {
        let mut th = e.theta;
        if th < 0.0 {
            th += 2.0 * core::f64::consts::PI;
        }
        let t0 = libm::round(th / dt) as i64;
        for k in -VOTE_SPAN..=VOTE_SPAN {
            let t = (t0 + k).rem_euclid(NT as i64) as usize;
            let (c, s) = cs[t];
            let r = libm::round(e.p.x * c + e.p.y * s) as i64 + diag;
            if r >= 0 && (r as usize) < nr {
                votes[t * nr + r as usize] += 1;
            }
        }
    }
    let min_votes = (w.min(h) / 8).max(8) as u32;
    let mut lines: Vec<(usize, EdgeLine)> = Vec::new();
    for t in 0..NT {
        for r in 0..nr {
            let v = votes[t * nr + r];
            if v < min_votes {
                continue;
            }
            let here = t * nr + r;
            let mut peak = true;
            'win: for dtb in -2i64..=2 {
                let tt = (t as i64 + dtb).rem_euclid(NT as i64) as usize;
                for drb in -2i64..=2 {
                    let rr = r as i64 + drb;
                    if rr < 0 || rr as usize >= nr || (dtb == 0 && drb == 0) {
                        continue;
                    }
                    let idx = tt * nr + rr as usize;
                    let o = votes[idx];
                    if o > v || (o == v && idx < here) {
                        peak = false;
                        break 'win;
                    }
                }
            }
            if !peak {
                continue;
            }
            let rho = (r as i64 - diag) as f64;
            if let Some(l) = refine(el, t as f64 * dt, rho, (min_votes / 2) as usize) {
                let support = l.pts.len();
                lines.push((support, l));
            }
        }
    }
    // A peak and its side lobes refine to one line: keep the best-supported of each.
    lines.sort_by(|a, b| b.0.cmp(&a.0));
    let same = libm::cos(1.0 * core::f64::consts::PI / 180.0);
    let mut out: Vec<EdgeLine> = Vec::new();
    for (_, l) in lines {
        if out.iter().all(|o| !(o.n.dot(l.n) > same && (o.n.dot(l.c) - o.rho).abs() < 3.0)) {
            out.push(l);
        }
    }
    out
}

/// The signed distance, along `a.n`, of line `b` from line `a`, measured at `a`'s centroid.
fn offset_at(a: &EdgeLine, b: &EdgeLine) -> f64 {
    let foot = a.c - b.n * (b.n.dot(a.c) - b.rho);
    a.n.dot(foot) - a.rho
}

/// Pair opposite-polarity edges into ink strokes (mutual nearest, normals pointing away from each other).
pub fn strokes(lines: &[EdgeLine]) -> Vec<GridLine> {
    let anti = -libm::cos(PAIR_DEG * core::f64::consts::PI / 180.0);
    let best = |i: usize| -> Option<usize> {
        let a = &lines[i];
        let mut pick: Option<(usize, f64)> = None;
        for (j, b) in lines.iter().enumerate() {
            if j == i || a.n.dot(b.n) > anti {
                continue;
            }
            let s = offset_at(a, b);
            let s2 = offset_at(b, a);
            if s < -0.5 && s > -MAX_STROKE_PX && s2 < -0.5 && s2 > -MAX_STROKE_PX {
                if pick.is_none_or(|(_, ps)| -s < ps) {
                    pick = Some((j, -s));
                }
            }
        }
        pick.map(|(j, _)| j)
    };
    let mut out = Vec::new();
    for i in 0..lines.len() {
        let Some(j) = best(i) else { continue };
        if j < i || best(j) != Some(i) {
            continue;
        }
        let (a, b) = (&lines[i], &lines[j]);
        let Ok(n) = (a.n - b.n).normalize() else { continue };
        let foot = a.c - b.n * (b.n.dot(a.c) - b.rho);
        let point = (a.c + foot) * 0.5;
        let dir = n.perp();
        let width_px = (a.n.dot(foot) - a.rho).abs();
        let mut support: Vec<f64> = a.pts.iter().chain(&b.pts).map(|&p| (p - point).dot(dir)).collect();
        support.sort_by(|x, y| x.total_cmp(y));
        out.push(GridLine { point, dir, width_px, support });
    }
    out
}

fn deg_mod180(d: Vec2) -> f64 {
    let mut a = libm::atan2(d.y, d.x) * 180.0 / core::f64::consts::PI;
    while a < 0.0 {
        a += 180.0;
    }
    while a >= 180.0 {
        a -= 180.0;
    }
    a
}

fn circ180(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 180.0;
    d.min(180.0 - d)
}

/// Order a family along `reference` and give each line its lattice index.
fn index_family(fam: Vec<GridLine>, reference: &GridLine) -> Vec<(i32, GridLine)> {
    let tol = Tol::DEFAULT;
    let mut at: Vec<(f64, GridLine)> = fam
        .into_iter()
        .filter_map(|l| match line_line2(&reference.line2(), &l.line2(), tol) {
            LineLine2::Point { t, .. } => Some((t, l)),
            _ => None,
        })
        .collect();
    at.sort_by(|x, y| x.0.total_cmp(&y.0));
    if at.len() < 2 {
        return at.into_iter().map(|(_, l)| (0, l)).collect();
    }
    let sp: Vec<f64> = at.windows(2).map(|w| w[1].0 - w[0].0).collect();
    let local = |k: usize| -> f64 {
        let lo = k.saturating_sub(2);
        let hi = (k + 2).min(sp.len() - 1);
        let mut v: Vec<f64> = sp[lo..=hi].to_vec();
        median(&mut v)
    };
    let mut out: Vec<(i32, GridLine)> = Vec::new();
    let mut prev_t = at[0].0;
    let mut prev_i = 0i32;
    let mut it = at.into_iter().enumerate();
    if let Some((_, (_, l0))) = it.next() {
        out.push((0, l0));
    }
    for (k, (t, l)) in it {
        let e = local(k - 1);
        if !(e > 0.0) {
            continue;
        }
        let ratio = (t - prev_t) / e;
        let step = libm::round(ratio);
        if step < 1.0 || (ratio - step).abs() > STEP_SLACK {
            continue;
        }
        prev_i += step as i32;
        prev_t = t;
        out.push((prev_i, l));
    }
    out
}

/// Find the grid in a photo's edgels.
pub fn find(el: &[Edgel], w: usize, h: usize) -> Result<Grid, Refusal> {
    let lines = hough_lines(el, w, h);
    let centres = strokes(&lines);
    let mut hist = [0.0f64; 180];
    for l in &centres {
        let b = (deg_mod180(l.dir) as usize).min(179);
        hist[b] += l.support.len() as f64;
    }
    let sm: Vec<f64> = (0..180).map(|b| (-2i32..=2).map(|k| hist[(b as i32 + k).rem_euclid(180) as usize]).sum::<f64>()).collect();
    let p1 = (0..180).max_by(|&x, &y| sm[x].total_cmp(&sm[y]).then(y.cmp(&x))).unwrap_or(0);
    let p2 = (0..180)
        .filter(|&b| circ180(b as f64, p1 as f64) >= FAMILY_SEP_DEG)
        .max_by(|&x, &y| sm[x].total_cmp(&sm[y]).then(y.cmp(&x)))
        .unwrap_or((p1 + 90) % 180);
    let (pa, pb) = (p1 as f64 + 0.5, p2 as f64 + 0.5);
    let mut fa = Vec::new();
    let mut fb = Vec::new();
    for l in centres {
        let a = deg_mod180(l.dir);
        let (da, db) = (circ180(a, pa), circ180(a, pb));
        if da <= db && da <= FAMILY_DEG {
            fa.push(l);
        } else if db < da && db <= FAMILY_DEG {
            fb.push(l);
        }
    }
    if fa.len() < MIN_LINES || fb.len() < MIN_LINES {
        return Err(Refusal::NoGrid { a: fa.len(), b: fb.len() });
    }
    let best_of = |f: &[GridLine]| -> GridLine {
        f.iter().max_by_key(|l| l.support.len()).cloned().unwrap_or_else(|| f[0].clone())
    };
    let ref_b = best_of(&fb[..]);
    let ref_a = best_of(&fa[..]);
    let a = index_family(fa, &ref_b);
    let b = index_family(fb, &ref_a);
    if a.len() < MIN_LINES || b.len() < MIN_LINES {
        return Err(Refusal::NoGrid { a: a.len(), b: b.len() });
    }
    let mut steps: Vec<f64> = Vec::new();
    for fam in [&a, &b] {
        for w2 in fam.windows(2) {
            let ((i0, l0), (i1, l1)) = (&w2[0], &w2[1]);
            let d = (l1.point - l0.point).dot(l0.dir.perp()).abs();
            if i1 > i0 {
                steps.push(d / (i1 - i0) as f64);
            }
        }
    }
    let pitch_px = median(&mut steps);
    let mut widths: Vec<f64> = a.iter().chain(&b).map(|(_, l)| l.width_px).collect();
    let width_px = median(&mut widths);
    let tol = Tol::DEFAULT;
    let mut inter = Vec::new();
    for (i, la) in &a {
        for (j, lb) in &b {
            if let LineLine2::Point { p, t, u } = line_line2(&la.line2(), &lb.line2(), tol) {
                let inside = p.x >= 0.0 && p.y >= 0.0 && p.x < w as f64 && p.y < h as f64;
                if inside && la.seen_near(t, 0.5 * pitch_px) && lb.seen_near(u, 0.5 * pitch_px) {
                    inter.push(Inter { px: p, i: *i, j: *j });
                }
            }
        }
    }
    Ok(Grid { a, b, pitch_px, width_px, inter })
}
