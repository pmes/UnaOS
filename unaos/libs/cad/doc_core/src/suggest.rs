// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D) — shared-core: doc_core::suggest, the offline design-intent proposer (BP-vug-7 by hand).
//!
//! VUGINTENTSCAN (LEDGER SR508): [`suggest`] reads the document's drawn values (the measured state: points where they
//! were measured, circles at their measured radii) and the measurement's sigma, and proposes what a person reverse-
//! engineering the part would: a line within sigma of the sketch axes is horizontal / vertical, lines within sigma of
//! one direction are parallel (or perpendicular to the first), a length within sigma of a whole (else a half) mm is
//! that dimension, a hole within sigma of a standard size is that size ([`crate::nearest_standard`]), holes of equal
//! radius within sigma are equal, centres within sigma are concentric, and three or more hole centres within sigma of
//! one circle are a bolt circle (ONE statement set: the centre point, then a distance to each hole, sharing one why).
//! Each is a [`Proposal`] with `why` the rule and `confidence = 1 - residual / sigma`. It never verifies: the caller
//! runs [`crate::verify_all`], and geometry disposes. No model, no network.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use brep_core::geom_core::Vec2;

use crate::intent::{Measurement, Proposal};
use crate::standards::{Class, nearest_standard};
use crate::write::f64_text;
use crate::{Cons, Doc, PRef, Stmt};

/// The offline proposals for `doc` against `m`'s sigma, in rule order per sketch.
pub fn suggest(doc: &Doc, m: &Measurement) -> Vec<Proposal> {
    let sigma = m.sigma_mm();
    let mut out = Vec::new();
    if !(sigma.is_finite() && sigma > 0.0) {
        return out;
    }
    let mut ids = Ids { taken: doc.stmts.iter().filter_map(|s| s.id().map(String::from)).collect(), n: 0 };
    for st in &doc.stmts {
        if let Stmt::Sketch { id, .. } = st {
            sketch_rules(doc, id, sigma, &mut ids, &mut out);
        }
    }
    out
}

/// Fresh statement ids `i<n>` the document does not hold.
struct Ids {
    taken: BTreeSet<String>,
    n: u32,
}

impl Ids {
    fn next(&mut self) -> String {
        loop {
            self.n += 1;
            let id = format!("i{}", self.n);
            if self.taken.insert(id.clone()) {
                return id;
            }
        }
    }
}

fn confidence(res: f64, sigma: f64) -> f64 {
    (1.0 - res / sigma).clamp(0.0, 1.0)
}

fn mm(v: f64) -> String {
    format!("{:.3}", v)
}

/// `v` rounded to a whole mm within sigma, else to a half mm within sigma: (value, |v - value|).
fn round_within(v: f64, sigma: f64) -> Option<(f64, f64)> {
    for step in [1.0, 0.5] {
        let r = libm::round(v / step) * step;
        if r > 0.0 && (v - r).abs() <= sigma {
            return Some((r, (v - r).abs()));
        }
    }
    None
}

struct Line<'a> {
    id: &'a str,
    a: &'a str,
    b: &'a str,
    pa: Vec2,
    pb: Vec2,
}

impl Line<'_> {
    fn dir(&self) -> Vec2 {
        self.pb - self.pa
    }
    fn len(&self) -> f64 {
        self.dir().mag()
    }
    /// How far this line's far end sits off the direction `u` (unit), in mm: parallel within sigma when small.
    fn off_parallel(&self, u: Vec2) -> f64 {
        u.cross(self.dir()).abs()
    }
    fn off_perpendicular(&self, u: Vec2) -> f64 {
        u.dot(self.dir()).abs()
    }
    fn unit(&self) -> Option<Vec2> {
        self.dir().normalize().ok()
    }
}

struct Circle<'a> {
    id: &'a str,
    centre: &'a str,
    c: Vec2,
    r: f64,
}

fn point_id(p: &PRef) -> Option<&str> {
    match p {
        PRef::Point(id) => Some(id),
        _ => None,
    }
}

fn sketch_rules(doc: &Doc, sk: &str, sigma: f64, ids: &mut Ids, out: &mut Vec<Proposal>) {
    // The sketch's drawn entities.
    let mut pts: BTreeMap<&str, Vec2> = BTreeMap::new();
    for st in &doc.stmts {
        if let Stmt::Point { id, sketch, at } = st {
            if sketch == sk {
                pts.insert(id, Vec2::new(at[0], at[1]));
            }
        }
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut circles: Vec<Circle> = Vec::new();
    for st in &doc.stmts {
        match st {
            Stmt::Line { id, sketch, a, b } if sketch == sk => {
                if let (Some(&pa), Some(&pb)) = (pts.get(a.as_str()), pts.get(b.as_str())) {
                    lines.push(Line { id, a, b, pa, pb });
                }
            }
            Stmt::Circle { id, sketch, centre, r } if sketch == sk => {
                if let Some(&c) = pts.get(centre.as_str()) {
                    circles.push(Circle { id, centre, c, r: *r });
                }
            }
            _ => {}
        }
    }
    // What the document already says (an entity it constrains is not proposed again).
    let mut oriented: BTreeSet<&str> = BTreeSet::new();
    let mut dims: Vec<(Vec2, f64)> = Vec::new();
    let mut dimensioned: BTreeSet<&str> = BTreeSet::new();
    let mut sized: BTreeSet<&str> = BTreeSet::new();
    let mut joined: BTreeSet<(&str, &str)> = BTreeSet::new();
    for st in &doc.stmts {
        let Stmt::Constraint { sketch, c, .. } = st else { continue };
        if sketch != sk {
            continue;
        }
        match c {
            Cons::Horizontal(l) | Cons::Vertical(l) => {
                oriented.insert(l);
            }
            Cons::Parallel(a, b) | Cons::Perpendicular(a, b) | Cons::Angle(a, b, _) => {
                oriented.insert(a);
                oriented.insert(b);
            }
            Cons::EqualLength(a, b) => {
                dimensioned.insert(a);
                dimensioned.insert(b);
            }
            Cons::Distance(a, b, d) => {
                if let (Some(a), Some(b)) = (point_id(a), point_id(b)) {
                    for l in &lines {
                        if (l.a == a && l.b == b) || (l.a == b && l.b == a) {
                            dimensioned.insert(l.id);
                            if let Some(u) = l.unit() {
                                dims.push((u, *d));
                            }
                        }
                    }
                }
            }
            Cons::Radius(k, _) | Cons::Diameter(k, _) => {
                sized.insert(k);
            }
            Cons::EqualRadius(a, b) => {
                sized.insert(a);
                sized.insert(b);
            }
            Cons::Coincident(a, b) => {
                if let (Some(a), Some(b)) = (point_id(a), point_id(b)) {
                    joined.insert((a, b));
                    joined.insert((b, a));
                }
            }
            _ => {}
        }
    }
    let mut push = |stmt: String, res: f64, why: String| {
        out.push(Proposal { stmt, confidence: confidence(res, sigma), why });
    };

    // 1. Horizontal / vertical: the end-to-end offset across the axis within sigma.
    let mut axis_aligned: BTreeSet<&str> = BTreeSet::new();
    for l in lines.iter().filter(|l| !oriented.contains(l.id)) {
        let d = l.dir();
        let (dx, dy) = (d.x.abs(), d.y.abs());
        if dy <= sigma && dx > sigma {
            push(format!("horizontal {} sketch={} line={}", ids.next(), sk, l.id), dy, format!("horizontal: end offset {} mm within sigma {}", mm(dy), mm(sigma)));
            axis_aligned.insert(l.id);
        } else if dx <= sigma && dy > sigma {
            push(format!("vertical {} sketch={} line={}", ids.next(), sk, l.id), dx, format!("vertical: end offset {} mm within sigma {}", mm(dx), mm(sigma)));
            axis_aligned.insert(l.id);
        }
    }

    // 2. Parallel to the first line of a direction class; a new class head perpendicular to the first head.
    let mut heads: Vec<(&str, Vec2)> = Vec::new();
    for l in lines.iter().filter(|l| !oriented.contains(l.id) && !axis_aligned.contains(l.id)) {
        let Some(u) = l.unit() else { continue };
        if let Some(&(h, hu)) = heads.iter().find(|(_, hu)| l.off_parallel(*hu) <= sigma) {
            let res = l.off_parallel(hu);
            push(format!("parallel {} sketch={} a={} b={}", ids.next(), sk, h, l.id), res, format!("parallel: end offset {} mm within sigma {}", mm(res), mm(sigma)));
            continue;
        }
        if let Some(&(h, hu)) = heads.first() {
            let res = l.off_perpendicular(hu);
            if res <= sigma {
                push(format!("perpendicular {} sketch={} a={} b={}", ids.next(), sk, h, l.id), res, format!("perpendicular: end offset {} mm within sigma {}", mm(res), mm(sigma)));
            }
        }
        heads.push((l.id, u));
    }

    // 3. Round lengths; a line parallel to an already-dimensioned one at the same value is fixed by the closure.
    for l in lines.iter().filter(|l| !dimensioned.contains(l.id)) {
        let Some((v, res)) = round_within(l.len(), sigma) else { continue };
        if dims.iter().any(|&(u, d)| d == v && l.off_parallel(u) <= sigma) {
            continue;
        }
        push(
            format!("distance {} sketch={} a={} b={} d={}", ids.next(), sk, l.a, l.b, f64_text(v)),
            res,
            format!("round dimension: {} mm measured, {} within sigma {}", mm(l.len()), f64_text(v), mm(sigma)),
        );
        if let Some(u) = l.unit() {
            dims.push((u, v));
        }
    }

    // 4. Holes: equal radii within sigma grouped; the group's first at the standard (else round) diameter.
    let free: Vec<&Circle> = circles.iter().filter(|c| !sized.contains(c.id)).collect();
    let mut groups: Vec<Vec<&Circle>> = Vec::new();
    for &c in &free {
        match groups.iter_mut().find(|g| (g[0].r - c.r).abs() <= sigma) {
            Some(g) => g.push(c),
            None => groups.push(alloc::vec![c]),
        }
    }
    for g in &groups {
        let head = g[0];
        let d = 2.0 * head.r;
        if let Some(st) = nearest_standard(d, sigma) {
            let why = match st.class {
                Class::TapDrill => format!("ISO 2306 tap drill for {} (pitch {}): {} mm measured", st.thread, f64_text(st.pitch_mm), mm(d)),
                c => format!("ISO 273 {} clearance for {}: {} mm measured", c.name(), st.thread, mm(d)),
            };
            push(format!("diameter {} sketch={} circle={} d={}", ids.next(), sk, head.id, f64_text(st.hole_mm)), (d - st.hole_mm).abs(), why);
        } else if let Some((v, res)) = round_within(d, sigma) {
            push(
                format!("diameter {} sketch={} circle={} d={}", ids.next(), sk, head.id, f64_text(v)),
                res,
                format!("round diameter: {} mm measured, {} within sigma {}", mm(d), f64_text(v), mm(sigma)),
            );
        }
        for c in &g[1..] {
            let res = (c.r - head.r).abs();
            push(format!("equal_radius {} sketch={} a={} b={}", ids.next(), sk, head.id, c.id), res, format!("equal radii: {} mm apart within sigma {}", mm(res), mm(sigma)));
        }
    }

    // 5. Concentric: distinct centres within sigma.
    for i in 0..circles.len() {
        for j in i + 1..circles.len() {
            let (a, b) = (&circles[i], &circles[j]);
            let res = a.c.dist(b.c);
            if a.centre != b.centre && res <= sigma && !joined.contains(&(a.centre, b.centre)) {
                push(format!("coincident {} sketch={} a={} b={}", ids.next(), sk, a.centre, b.centre), res, format!("concentric: centres {} mm apart within sigma {}", mm(res), mm(sigma)));
                joined.insert((a.centre, b.centre));
                joined.insert((b.centre, a.centre));
            }
        }
    }

    // 6. Bolt circle: >= 3 distinct hole centres within sigma of one fitted circle, as ONE statement set.
    let mut centres: Vec<(&str, Vec2)> = Vec::new();
    for c in &circles {
        if !centres.iter().any(|(id, _)| *id == c.centre) {
            centres.push((c.centre, c.c));
        }
    }
    if centres.len() >= 3 {
        let ps: Vec<Vec2> = centres.iter().map(|x| x.1).collect();
        if let Some((o, r)) = kasa(&ps) {
            let dev = ps.iter().map(|p| (p.dist(o) - r).abs()).fold(0.0, f64::max);
            if dev <= sigma {
                if let Some((rr, rres)) = round_within(r, sigma) {
                    let why = format!("bolt-circle n={} r={}: centres within {} mm of r {} mm, sigma {}", ps.len(), f64_text(rr), mm(dev), mm(r), mm(sigma));
                    let res = dev.max(rres);
                    let pc = ids.next();
                    push(format!("point {} sketch={} x={} y={}", pc, sk, f64_text(o.x), f64_text(o.y)), res, why.clone());
                    for (cid, _) in &centres {
                        push(format!("distance {} sketch={} a={} b={} d={}", ids.next(), sk, pc, cid, f64_text(rr)), res, why.clone());
                    }
                }
            }
        }
    }
}

/// Kasa's algebraic circle fit (least squares of `x² + y² + D x + E y + F` over points centred on their mean; the 3x3
/// normal equations by Cramer's rule): (centre, radius), None for collinear or degenerate input.
fn kasa(ps: &[Vec2]) -> Option<(Vec2, f64)> {
    let n = ps.len() as f64;
    let mean = ps.iter().fold(Vec2::new(0.0, 0.0), |a, &p| a + p) * (1.0 / n);
    let (mut sxx, mut sxy, mut syy, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut sxz, mut syz, mut sz) = (0.0, 0.0, 0.0);
    for &p in ps {
        let q = p - mean;
        let z = q.x * q.x + q.y * q.y;
        sxx += q.x * q.x;
        sxy += q.x * q.y;
        syy += q.y * q.y;
        sx += q.x;
        sy += q.y;
        sxz += q.x * z;
        syz += q.y * z;
        sz += z;
    }
    let m = [[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, n]];
    let rhs = [-sxz, -syz, -sz];
    let det = det3(m);
    let scale = sxx.max(syy).max(1.0);
    if !(det.abs() > 1e-12 * scale * scale * n) {
        return None;
    }
    let col = |k: usize| {
        let mut a = m;
        for (row, v) in a.iter_mut().zip(rhs) {
            row[k] = v;
        }
        det3(a) / det
    };
    let (dd, ee, ff) = (col(0), col(1), col(2));
    let c = Vec2::new(-dd / 2.0, -ee / 2.0);
    let r2 = c.x * c.x + c.y * c.y - ff;
    if !(r2.is_finite() && r2 > 0.0) {
        return None;
    }
    Some((c + mean, libm::sqrt(r2)))
}

fn det3(m: [[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}
