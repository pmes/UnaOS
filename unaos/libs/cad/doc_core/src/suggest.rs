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
//!
//! VUGINTENTPOSITIONS (LEDGER SR527): where a hole sits. The DATUM is the sketch's first `fix` on a Point (at its `at`),
//! else the sketch origin; the POSITION LADDER is the nearest multiple of 0.5 mm, a whole mm on a tie, within sigma
//! (lengths keep `round_within`). Targets are the distinct hole centres and the isolated points the document does not
//! already place, minus a centre joined to an earlier one and every bolt-circle centre (rule 6 owns those and is still
//! emitted last). Rule 7: two targets mirrored within sigma about a drawn line, or about the midline of two parallel
//! lines (built ONCE as a statement set: a point at each rung's midpoint, `midpoint`, the line), are `symmetric`; the
//! second is the mirror member and gets no position. Rule 8: per remaining target, to the nearest vertical-class and
//! the nearest horizontal-class line, at most one `distance_point_line` each: the datum grid (the target's offset from
//! the datum a round pair and the line's offset round: their difference) else the edge distance (the measured distance
//! round). At most one per axis because `verify_all` is cumulative and refuses a redundant statement.
//!
//! VUGINTENTFEATURES (LEDGER SR536): a FEATURE from `Measurement::Shapes` alone ([`features`], after every sketch rule).
//! A measured cylinder the replayed document does not already model, whose axis is within sigma_deg of a measured plane
//! that is a face of the document (paired within `K_SIGMA` x sigma through intent's one `pair`), stands on the plane with
//! the least support-centroid offset `t` (scan_core's cylinder point is its inliers' centroid along the axis). THE EXTENT
//! RULE: h = 2|t| (the centroid is mid-extent, the near end on the plane), or |s| to a measured parallel plane on the
//! support side whose near end e = |t - s/2| is within 3 sigma (the far end: a boss top, a pocket floor, a plate's far
//! face); a measured parallel plane crossing the span (|s| < 2|t| - 6 sigma) means no end on the plane within 3 sigma and
//! nothing is proposed; one beyond it is ignored. The support centroid inside the replayed solid is a POCKET (scan
//! normals are canonical, not outward), outside a BOSS; the set is `sketch` on the plane (z toward the support), the
//! centre `point`, `circle` (a pocket's diameter through the standards table, else round; a boss's round), `extrude`
//! along z by the height (round within sigma), ONE statement set sharing one why, confidence from the worst residual.
//! VUGINTENTFEATURESEXTENT (LEDGER SR653): a cylinder that carries its support's axial extent (`ShapeRef::extent`) is
//! measured, not inferred: h0 is its far end's offset from the plane (a far plane within 3 sigma of that end), not 2|t|.
//! VUGINTENTFEATURESSLOT (LEDGER SR666): a SLOT (an obround pocket or boss) scans as two cylinders of one radius and
//! two parallel plane walls (scan_core's detect keeps the walls two planes of one normal sign and the half-cylinders two
//! cylinders). [`slots`] runs before the cylinder loop: two cylinders no face models, radii within sigma, axes within
//! sigma_deg, axis distance d > 2 sigma, with a measured wall at +r and at -r from the joining line, are ONE slot; both
//! are taken (never also two bosses or pockets) and the midpoint is measured by the cylinder road's own [`stand`] (the
//! plane, the extent rule, pocket or boss); the set is the obround in the replay's own form (six points, two lines, two
//! arcs, no constraint) on the plane, its length d and width 2r round within sigma, extruded by the height.
//!
//! VUGINTENTSETVERIFY (LEDGER SR541): a statement set is emitted as [`crate::verify_all`] disposes it WHOLE: contiguous,
//! one `why` that no adjacent proposal shares (a feature's names its cylinder, a bolt circle its n and r, a midline its
//! pair), every member linked to the others by id (`sketch=` / `centre=` / `outer=`, `a=` / `p=` / `line=`). The set is
//! applied, replayed and judged once by the residual after its last member, so a boss top or a pocket floor only the
//! extrude models no longer refuses the set's leading lines.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use brep_core::geom_core::{Vec2, Vec3};
use brep_core::{Where, point_in_solid}; // VUGINTENTFEATURES (SR536)

use crate::intent::{K_SIGMA, Measurement, Proposal, ShapeParams, axis_deg, pair};
use crate::replay::replay; // VUGINTENTFEATURES (SR536)
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
    features(doc, m, sigma, &mut ids, &mut out); // VUGINTENTFEATURES (SR536): bosses / pockets from shapes (file tail)
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
        out.push(Proposal { stmt, confidence: confidence(res, sigma), why, shapes: Vec::new() }); // VUGINTENTSETSIBLINGS (SR674): a sketch rule names no shape
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

    // 6. Bolt circle: >= 3 distinct hole centres within sigma of one fitted circle, as ONE statement set (detected
    // here, emitted LAST: VUGINTENTPOSITIONS leaves its centres to it).
    let bolt = bolt_circle(&circles, sigma);

    // 7./8. VUGINTENTPOSITIONS (SR527): symmetric pairs, then datum-grid / edge distances (bodies at the file tail).
    positions(doc, sk, sigma, &lines, &circles, &joined, bolt.as_ref(), ids, &mut push);

    if let Some(b) = &bolt {
        let why = format!(
            "bolt-circle n={} r={}: centres within {} mm of r {} mm, sigma {}",
            b.centres.len(),
            f64_text(b.rr),
            mm(b.dev),
            mm(b.r),
            mm(sigma)
        );
        let res = b.dev.max(b.rres);
        let pc = ids.next();
        push(format!("point {} sketch={} x={} y={}", pc, sk, f64_text(b.o.x), f64_text(b.o.y)), res, why.clone());
        for (cid, _) in &b.centres {
            push(format!("distance {} sketch={} a={} b={} d={}", ids.next(), sk, pc, cid, f64_text(b.rr)), res, why.clone());
        }
    }
}

/// A bolt circle: its distinct centres (doc order), the fitted centre and radius, the worst deviation, the round radius.
struct Bolt<'a> {
    centres: Vec<(&'a str, Vec2)>,
    o: Vec2,
    r: f64,
    dev: f64,
    rr: f64,
    rres: f64,
}

/// Rule 6's detection: >= 3 distinct centres within sigma of one Kasa circle whose radius is round within sigma.
fn bolt_circle<'a>(circles: &[Circle<'a>], sigma: f64) -> Option<Bolt<'a>> {
    let mut centres: Vec<(&'a str, Vec2)> = Vec::new();
    for c in circles {
        if !centres.iter().any(|(id, _)| *id == c.centre) {
            centres.push((c.centre, c.c));
        }
    }
    if centres.len() < 3 {
        return None;
    }
    let ps: Vec<Vec2> = centres.iter().map(|x| x.1).collect();
    let (o, r) = kasa(&ps)?;
    let dev = ps.iter().map(|p| (p.dist(o) - r).abs()).fold(0.0, f64::max);
    if !(dev <= sigma) {
        return None;
    }
    let (rr, rres) = round_within(r, sigma)?;
    Some(Bolt { centres, o, r, dev, rr, rres })
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

// ---- VUGINTENTPOSITIONS (SR527): where a hole sits — symmetric pairs, datum-grid and edge distances ----

/// The position ladder: `v` to the nearest multiple of 0.5 mm (a whole mm on a tie), within sigma: (value, |v - value|).
fn round_position(v: f64, sigma: f64) -> Option<(f64, f64)> {
    let w = libm::round(v);
    let h = libm::round(v * 2.0) / 2.0;
    let r = (if (v - w).abs() <= (v - h).abs() { w } else { h }) + 0.0;
    let res = (v - r).abs();
    if res <= sigma { Some((r, res)) } else { None }
}

/// `q` mirrored about the line through `a` with unit direction `u`.
fn reflect(q: Vec2, a: Vec2, u: Vec2) -> Vec2 {
    let v = q - a;
    a + u * (2.0 * v.dot(u)) - v
}

/// A point whose position is proposed: a hole centre or an isolated point.
struct Target<'a> {
    id: &'a str,
    c: Vec2,
}

/// The midline of the parallel lines `a` / `b`: the line through the midpoints `m1` / `m2` of the rungs `r1` / `r2`
/// (the first two lines, doc order, with one end on each).
struct Midline<'a> {
    a: &'a str,
    b: &'a str,
    r1: &'a str,
    r2: &'a str,
    m1: Vec2,
    m2: Vec2,
}

#[allow(clippy::too_many_arguments)]
fn positions<'a>(
    doc: &'a Doc,
    sk: &str,
    sigma: f64,
    lines: &[Line<'a>],
    circles: &[Circle<'a>],
    joined: &BTreeSet<(&'a str, &'a str)>,
    bolt: Option<&Bolt<'a>>,
    ids: &mut Ids,
    push: &mut dyn FnMut(String, f64, String),
) {
    let ts = targets(doc, sk, circles, joined, bolt);
    let mirrored = symmetric_rule(sk, sigma, lines, &ts, ids, &mut *push);
    position_rule(doc, sk, sigma, lines, &ts, &mirrored, ids, &mut *push);
}

/// The distinct hole centres (doc order) then the isolated points, minus what the document already places, a centre
/// joined to an earlier target, and the bolt circle's centres.
fn targets<'a>(
    doc: &'a Doc,
    sk: &str,
    circles: &[Circle<'a>],
    joined: &BTreeSet<(&'a str, &'a str)>,
    bolt: Option<&Bolt<'a>>,
) -> Vec<Target<'a>> {
    let mut placed: BTreeSet<&'a str> = BTreeSet::new();
    let mut used: BTreeSet<&'a str> = BTreeSet::new();
    for st in &doc.stmts {
        match st {
            Stmt::Constraint { sketch, c, .. } if sketch == sk => {
                let refs: [Option<&'a PRef>; 2] = match c {
                    Cons::Distance(a, b, _) | Cons::Symmetric(a, b, _) => [Some(a), Some(b)],
                    Cons::DistancePointLine(p, ..) | Cons::Fix(p, _) | Cons::Midpoint(p, _) | Cons::PointOnLine(p, _) => {
                        [Some(p), None]
                    }
                    _ => [None, None],
                };
                for r in refs.into_iter().flatten() {
                    if let Some(id) = point_id(r) {
                        placed.insert(id);
                    }
                }
            }
            Stmt::Line { sketch, a, b, .. } if sketch == sk => {
                used.insert(a);
                used.insert(b);
            }
            Stmt::Circle { sketch, centre, .. } | Stmt::Arc { sketch, centre, .. } if sketch == sk => {
                used.insert(centre);
            }
            _ => {}
        }
    }
    let mut out: Vec<Target<'a>> = Vec::new();
    for c in circles {
        let in_bolt = bolt.is_some_and(|b| b.centres.iter().any(|(id, _)| *id == c.centre));
        if in_bolt
            || placed.contains(c.centre)
            || out.iter().any(|t| t.id == c.centre || joined.contains(&(t.id, c.centre)))
        {
            continue;
        }
        out.push(Target { id: c.centre, c: c.c });
    }
    for st in &doc.stmts {
        if let Stmt::Point { id, sketch, at } = st {
            if sketch == sk && !used.contains(id.as_str()) && !placed.contains(id.as_str()) {
                out.push(Target { id, c: Vec2::new(at[0], at[1]) });
            }
        }
    }
    out
}

/// The datum rule: the sketch's first `fix` on a Point, at its `at`; else the sketch origin. (point, its name)
fn datum(doc: &Doc, sk: &str) -> (Vec2, String) {
    for st in &doc.stmts {
        if let Stmt::Constraint { sketch, c: Cons::Fix(PRef::Point(p), at), .. } = st {
            if sketch == sk {
                return (Vec2::new(at[0], at[1]), format!("fix {}", p));
            }
        }
    }
    (Vec2::new(0.0, 0.0), String::from("origin"))
}

/// Every pair of lines parallel within sigma and more than sigma apart with two rungs, as its [`Midline`].
fn midlines<'a>(lines: &[Line<'a>], sigma: f64) -> Vec<Midline<'a>> {
    let mut out = Vec::new();
    for i in 0..lines.len() {
        let Some(u) = lines[i].unit() else { continue };
        for j in i + 1..lines.len() {
            let (la, lb) = (&lines[i], &lines[j]);
            if !(lb.off_parallel(u) <= sigma && u.cross(lb.pa - la.pa).abs() > sigma) {
                continue;
            }
            let (ea, eb) = ([la.a, la.b], [lb.a, lb.b]);
            let mut rungs = lines.iter().filter(|r| {
                r.id != la.id
                    && r.id != lb.id
                    && ((ea.contains(&r.a) && eb.contains(&r.b)) || (ea.contains(&r.b) && eb.contains(&r.a)))
            });
            let (Some(r1), Some(r2)) = (rungs.next(), rungs.next()) else { continue };
            out.push(Midline {
                a: la.id,
                b: lb.id,
                r1: r1.id,
                r2: r2.id,
                m1: (r1.pa + r1.pb) * 0.5,
                m2: (r2.pa + r2.pb) * 0.5,
            });
        }
    }
    out
}

/// Rule 7: each unpaired target pair (i < j) mirrored within sigma about a drawn line or a midline (the least residual,
/// the earlier axis on a tie; the first target more than sigma off the axis) as ONE statement set. Returns the mirror
/// members.
fn symmetric_rule<'a>(
    sk: &str,
    sigma: f64,
    lines: &[Line<'a>],
    ts: &[Target<'a>],
    ids: &mut Ids,
    push: &mut dyn FnMut(String, f64, String),
) -> BTreeSet<&'a str> {
    let mids = midlines(lines, sigma);
    let mut built: Vec<Option<String>> = alloc::vec![None; mids.len()];
    let mut paired: BTreeSet<&'a str> = BTreeSet::new();
    let mut mirrored: BTreeSet<&'a str> = BTreeSet::new();
    for i in 0..ts.len() {
        if paired.contains(ts[i].id) {
            continue;
        }
        for j in i + 1..ts.len() {
            if paired.contains(ts[j].id) {
                continue;
            }
            let (p, q) = (ts[i].c, ts[j].c);
            // (point on the axis, unit, Ok(drawn line) | Err(midline)).
            let drawn = lines.iter().enumerate().filter_map(|(k, l)| l.unit().map(|u| (l.pa, u, Ok::<usize, usize>(k))));
            let mid = mids
                .iter()
                .enumerate()
                .filter_map(|(k, m)| (m.m2 - m.m1).normalize().ok().map(|u| (m.m1, u, Err::<usize, usize>(k))));
            let mut best: Option<(f64, Result<usize, usize>)> = None;
            for (a, u, which) in drawn.chain(mid) {
                if !(u.cross(p - a).abs() > sigma) {
                    continue;
                }
                let res = reflect(p, a, u).dist(q);
                if res <= sigma && best.is_none_or(|(b, _)| res < b) {
                    best = Some((res, which));
                }
            }
            let Some((res, which)) = best else { continue };
            let about = match which {
                Ok(k) => format!("line {}", lines[k].id),
                Err(k) => format!("the midline of {} {}", mids[k].a, mids[k].b),
            };
            let why = format!("symmetric: {} {} mirror about {} within {} mm, sigma {}", ts[i].id, ts[j].id, about, mm(res), mm(sigma));
            let axis = match which {
                Ok(k) => String::from(lines[k].id),
                Err(k) => {
                    if let Some(id) = built[k].clone() {
                        id
                    } else {
                        let m = &mids[k];
                        let m1 = ids.next();
                        push(format!("point {} sketch={} x={} y={}", m1, sk, f64_text(m.m1.x), f64_text(m.m1.y)), res, why.clone());
                        push(format!("midpoint {} sketch={} p={} line={}", ids.next(), sk, m1, m.r1), res, why.clone());
                        let m2 = ids.next();
                        push(format!("point {} sketch={} x={} y={}", m2, sk, f64_text(m.m2.x), f64_text(m.m2.y)), res, why.clone());
                        push(format!("midpoint {} sketch={} p={} line={}", ids.next(), sk, m2, m.r2), res, why.clone());
                        let ml = ids.next();
                        push(format!("line {} sketch={} a={} b={}", ml, sk, m1, m2), res, why.clone());
                        built[k] = Some(ml.clone());
                        ml
                    }
                }
            };
            push(format!("symmetric {} sketch={} a={} b={} line={}", ids.next(), sk, ts[i].id, ts[j].id, axis), res, why);
            paired.insert(ts[i].id);
            paired.insert(ts[j].id);
            mirrored.insert(ts[j].id);
            break;
        }
    }
    mirrored
}

/// Rule 8: per target that is not a mirror member, x (the nearest vertical-class line) then y (the nearest
/// horizontal-class line, ties within Tol.lin to the earlier line), at most one `distance_point_line`: the datum grid,
/// else the edge distance.
#[allow(clippy::too_many_arguments)]
fn position_rule<'a>(
    doc: &Doc,
    sk: &str,
    sigma: f64,
    lines: &[Line<'a>],
    ts: &[Target<'a>],
    mirrored: &BTreeSet<&'a str>,
    ids: &mut Ids,
    push: &mut dyn FnMut(String, f64, String),
) {
    let (dat, dname) = datum(doc, sk);
    let lin = doc.tol.lin;
    for t in ts.iter().filter(|t| !mirrored.contains(t.id)) {
        let off = t.c - dat;
        let pair = round_position(off.x, sigma).zip(round_position(off.y, sigma));
        for vertical in [true, false] {
            let mut near: Option<(&Line<'a>, f64)> = None;
            for l in lines {
                let d = l.dir();
                let class = if vertical {
                    d.x.abs() <= sigma && d.y.abs() > sigma
                } else {
                    d.y.abs() <= sigma && d.x.abs() > sigma
                };
                let Some(u) = l.unit() else { continue };
                if !class {
                    continue;
                }
                let dist = u.cross(t.c - l.pa).abs();
                if near.is_none_or(|(_, b)| dist + lin < b) {
                    near = Some((l, dist));
                }
            }
            let Some((l, dist)) = near else { continue };
            // (a) The datum grid: the round offset pair against the line's round offset from the datum.
            let grid = pair.and_then(|((rx, ex), (ry, ey))| {
                let (rc, ec) = if vertical { (rx, ex) } else { (ry, ey) };
                let lo = if vertical { (l.pa.x + l.pb.x) * 0.5 - dat.x } else { (l.pa.y + l.pb.y) * 0.5 - dat.y };
                let (rl, _) = round_position(lo, sigma)?;
                let v = (rc - rl).abs();
                let res = (dist - v).abs();
                (v > 0.0 && res <= sigma).then(|| {
                    let why = format!(
                        "datum grid: {} ({}, {}) from {} -> ({}, {}); to {} {} mm, {} within sigma {}",
                        t.id,
                        mm(off.x),
                        mm(off.y),
                        dname,
                        f64_text(rx),
                        f64_text(ry),
                        l.id,
                        mm(dist),
                        f64_text(v),
                        mm(sigma)
                    );
                    (v, res.max(ec), why)
                })
            });
            // (b) The edge distance: the measured distance round on the ladder.
            let pick = grid.or_else(|| {
                let (v, res) = round_position(dist, sigma)?;
                (v > 0.0).then(|| {
                    (v, res, format!("edge distance: {} to {} {} mm, {} within sigma {}", t.id, l.id, mm(dist), f64_text(v), mm(sigma)))
                })
            });
            let Some((v, res, why)) = pick else { continue };
            push(format!("distance_point_line {} sketch={} p={} line={} d={}", ids.next(), sk, t.id, l.id, f64_text(v)), res, why);
        }
    }
}

// ---- VUGINTENTFEATURES (SR536): a boss or a pocket from a cylinder standing on a document face ----

/// A measured plane: its shape index, unit normal, offset along it (`n . p = d`) and support.
struct PlaneM {
    i: usize,
    n: Vec3,
    d: f64,
    support: u32,
}

/// The extent rule for the cylinder whose support centroid sits `t` off `planes[pk]`: (h, the far-end plane's (shape
/// index, e) when one is measured); None when a measured parallel plane crosses the span (no end on the plane within
/// 3 sigma). VUGINTENTFEATURESEXTENT (SR653): `reach` is the support's measured far end (ke = 1); None is the centroid
/// road h0 = 2|t| (ke = 2: a far end off by e moves the centroid by e / 2), bit-identical to SR536.
fn extent(planes: &[PlaneM], pk: usize, t: f64, reach: Option<f64>, sigma: f64, sigma_deg: f64) -> Option<(f64, Option<(usize, f64)>)> {
    let p = &planes[pk];
    let (h0, ke) = reach.map_or((2.0 * t.abs(), 2.0), |f| (f, 1.0));
    let lim = K_SIGMA * sigma;
    let mut far: Option<(usize, f64, f64)> = None;
    for (k, q) in planes.iter().enumerate() {
        if k == pk || axis_deg(q.n, p.n) > sigma_deg {
            continue;
        }
        // Q's offset along P's normal (a flipped normal flips the offset), relative to P.
        let s = (if q.n.dot(p.n) < 0.0 { -q.d } else { q.d }) - p.d;
        if !(s * t > 0.0 && s.abs() > lim) {
            continue;
        }
        let e = (h0 - s.abs()).abs() / ke;
        if e <= lim {
            if far.is_none_or(|(_, b, _)| e < b) {
                far = Some((q.i, e, s.abs()));
            }
        } else if s.abs() < h0 - ke * lim {
            return None;
        }
    }
    Some(match far {
        Some((i, e, s)) => (s, Some((i, e))),
        None => (h0, None),
    })
}

/// The feature rule (module doc): per measured cylinder in order, at most ONE statement set (judged whole by
/// `verify_all`, VUGINTENTSETVERIFY SR541).
fn features(doc: &Doc, m: &Measurement, sigma: f64, ids: &mut Ids, out: &mut Vec<Proposal>) {
    let Measurement::Shapes { shapes, sigma_deg, .. } = m else { return };
    let sigma_deg = *sigma_deg;
    if !(sigma_deg.is_finite() && sigma_deg > 0.0) {
        return;
    }
    let r = replay(doc, doc.tol);
    if r.solids.is_empty() {
        return;
    }
    let band = K_SIGMA * sigma;
    let per_deg = sigma / sigma_deg;
    // Some replayed face pairs the shape within the band (intent's one value: max(mm, deg x sigma_mm / sigma_deg)).
    let fits = |s: &ShapeParams| {
        r.solids
            .iter()
            .flat_map(|so| so.brep.faces.iter())
            .filter_map(|f| pair(s, &f.surface))
            .any(|(d, deg)| d.max(deg * per_deg) <= band)
    };
    let mut planes: Vec<PlaneM> = Vec::new();
    for (i, s) in shapes.iter().enumerate() {
        if let ShapeParams::Plane { normal, d } = s.params {
            let k = normal.mag();
            if let (true, Ok(n)) = (k > 0.0, normal.normalize()) {
                planes.push(PlaneM { i, n, d: d / k, support: s.support });
            }
        }
    }
    let on_doc: Vec<bool> = planes.iter().map(|p| fits(&shapes[p.i].params)).collect();
    let taken = slots(doc, &r, shapes, &planes, &on_doc, &fits, sigma, sigma_deg, ids, out); // VUGINTENTFEATURESSLOT (SR666): slots first, their cylinders taken
    for (ci, s) in shapes.iter().enumerate() {
        let ShapeParams::Cylinder { point, axis, radius } = s.params else { continue };
        if fits(&s.params) || taken.contains(&ci) {
            continue;
        }
        let Some(Stand { pk, t, ends, h, far, inside }) = stand(doc, &r, &planes, &on_doc, point, axis, s.extent, sigma, sigma_deg) else { continue }; // VUGINTENTFEATURESSLOT (SR666): the measurement lifted verbatim (file tail)
        let p = &planes[pk];
        let z = p.n * (if t > 0.0 { 1.0 } else { -1.0 });
        let z = Vec3::new(z.x + 0.0, z.y + 0.0, z.z + 0.0);
        let foot = point - p.n * t;
        let hint = if z.x.abs() < 0.9 { "1 0 0" } else { "0 1 0" };
        let (hv, hres) = round_within(h, sigma).unwrap_or((h, 0.0));
        let dm = 2.0 * radius;
        let standard = if inside { nearest_standard(dm, sigma) } else { None };
        let (rv, rres, rwhy) = if let Some(st) = standard {
            let name = match st.class {
                Class::TapDrill => format!("ISO 2306 tap drill for {}", st.thread),
                c => format!("ISO 273 {} clearance for {}", c.name(), st.thread),
            };
            (st.hole_mm / 2.0, (dm - st.hole_mm).abs(), format!("{} {} mm measured", name, mm(dm)))
        } else if let Some((v, res)) = round_within(dm, sigma) {
            (v / 2.0, res, format!("round diameter {} from {} mm", f64_text(v), mm(dm)))
        } else {
            (radius, 0.0, format!("diameter {} mm as measured", mm(dm)))
        };
        let e = far.map_or(0.0, |(_, e)| e);
        let src = match (far, ends) {
            (Some((qi, e)), _) => format!("far-end plane #{} (near end {} mm off)", qi, mm(e)),
            (None, Some((near, _))) => format!("the support's axial extent (near end {} mm off)", mm(near.abs())), // VUGINTENTFEATURESEXTENT (SR653)
            (None, None) => format!("the support centroid ({} mm off)", mm(t.abs())),
        };
        let why = format!(
            "{}: cylinder #{} on plane #{} (a document face), extent {} mm from {}, height {}, {}, sigma {}",
            if inside { "pocket" } else { "boss" },
            ci,
            p.i,
            mm(h),
            src,
            f64_text(hv),
            rwhy,
            mm(sigma)
        );
        let res = e.max(hres).max(rres);
        let conf = confidence(res, sigma);
        let (a, b, c, x) = (ids.next(), ids.next(), ids.next(), ids.next());
        let v3 = |v: Vec3| format!("{} {} {}", f64_text(v.x), f64_text(v.y), f64_text(v.z));
        for stmt in [
            format!("sketch {} o=[{}] z=[{}] x=[{}]", a, v3(foot), v3(z), hint),
            format!("point {} sketch={} x=0 y=0", b, a),
            format!("circle {} sketch={} centre={} r={}", c, a, b, f64_text(rv)),
            format!("extrude {} sketch={} outer={} dir=[{}] length={}", x, a, c, v3(z), f64_text(hv)),
        ] {
            out.push(Proposal { stmt, confidence: conf, why: why.clone(), shapes: modelled(&[ci], far) }); // VUGINTENTSETSIBLINGS (SR674)
        }
    }
}

// ---- VUGINTENTFEATURESSLOT (SR666): a slot (an obround pocket or boss) from two cylinders and their two walls ----

/// What a support standing on a document face measures: the plane (`planes` index) and the support centroid's offset
/// `t` from it, SR653's (near, far) ends, the extent rule's h with its far-end plane, and the centroid inside the
/// replayed solids (a pocket) or not (a boss).
struct Stand {
    pk: usize,
    t: f64,
    ends: Option<(f64, f64)>,
    h: f64,
    far: Option<(usize, f64)>,
    inside: bool,
}

/// `features`' per-cylinder measurement, lifted verbatim (SR536 + SR653): the support at `point` along `axis`, with
/// `span` its carried axial extent; None where the feature rule proposes nothing. The cylinder road and the slot road
/// both read it.
#[allow(clippy::too_many_arguments)]
fn stand(
    doc: &Doc,
    r: &crate::replay::Replay,
    planes: &[PlaneM],
    on_doc: &[bool],
    point: Vec3,
    axis: Vec3,
    span: Option<[f64; 2]>,
    sigma: f64,
    sigma_deg: f64,
) -> Option<Stand> {
    let lin = doc.tol.lin;
    // The plane it stands on: a document face, parallel, the least |t| (ties to the larger support, then earlier).
    let mut best: Option<(usize, f64)> = None;
    for (k, p) in planes.iter().enumerate() {
        if !on_doc[k] || axis_deg(axis, p.n) > sigma_deg {
            continue;
        }
        let t = p.n.dot(point) - p.d;
        if !(t.abs() > sigma) {
            continue;
        }
        let better = best.is_none_or(|(b, bt)| {
            t.abs() + lin < bt.abs() || ((t.abs() - bt.abs()).abs() <= lin && p.support > planes[b].support)
        });
        if better {
            best = Some((k, t));
        }
    }
    let Some((pk, t)) = best else { return None };
    // VUGINTENTFEATURESEXTENT (SR653): the carried extent's ends as offsets from the plane on the support side.
    let sg = if t > 0.0 { 1.0 } else { -1.0 };
    let ends = span.and_then(|[a0, a1]| {
        let c = axis.normalize().ok()?.dot(planes[pk].n);
        let (o0, o1) = (sg * (t + a0 * c), sg * (t + a1 * c));
        Some((o0.min(o1), o0.max(o1)))
    });
    if ends.is_some_and(|(_, f)| !(f.is_finite() && f > sigma)) {
        return None;
    }
    let Some((h, far)) = extent(&planes, pk, t, ends.map(|e| e.1), sigma, sigma_deg) else { return None };
    // Boss or pocket: the support centroid against the replayed solids.
    let (mut inside, mut unsure) = (false, false);
    for so in &r.solids {
        match point_in_solid(&so.brep, point, doc.tol) {
            Ok(Where::Inside) => inside = true,
            Ok(Where::Outside) => {}
            _ => unsure = true,
        }
    }
    if unsure {
        return None;
    }
    Some(Stand { pk, t, ends, h, far, inside })
}

/// One recognised slot: the cylinders' shape indices (i < j), the walls' shape indices (+w side, -w side), the midpoint
/// of the two axis points, the unit direction joining the axes (normal to them), the axis distance and the mean radius.
struct Slot {
    i: usize,
    j: usize,
    walls: [usize; 2],
    m: Vec3,
    u: Vec3,
    d: f64,
    r: f64,
}

/// The slot recogniser (module doc): for cylinders i < j in shape order that no document face models and no earlier
/// slot took, radii within sigma, axes within sigma_deg, axis distance d > 2 sigma, and two measured planes parallel to
/// the axes and to the joining line u (normal within sigma_deg of w = axis x u, compared unsigned: scan_core's
/// `canonical` gives both walls one normal sign) at +r and -r along w from the axes' midpoint within K_SIGMA sigma (the
/// nearest per side). The first j that pairs takes both (greedy, deterministic).
fn slot_of(shapes: &[crate::intent::ShapeRef], planes: &[PlaneM], fits: &dyn Fn(&ShapeParams) -> bool, sigma: f64, sigma_deg: f64) -> Vec<Slot> {
    let band = K_SIGMA * sigma;
    let cyl = |k: usize| match shapes[k].params {
        ShapeParams::Cylinder { point, axis, radius } if !fits(&shapes[k].params) => axis.normalize().ok().map(|a| (point, a, radius)),
        _ => None,
    };
    let mut taken = alloc::vec![false; shapes.len()];
    let mut out = Vec::new();
    for i in 0..shapes.len() {
        if taken[i] {
            continue;
        }
        let Some((pi, ai, ri)) = cyl(i) else { continue };
        for j in i + 1..shapes.len() {
            if taken[j] {
                continue;
            }
            let Some((pj, aj, rj)) = cyl(j) else { continue };
            if (ri - rj).abs() > sigma || axis_deg(ai, aj) > sigma_deg {
                continue;
            }
            let c = pj - pi;
            let u = c - ai * ai.dot(c);
            let d = u.mag();
            if !(d > 2.0 * sigma) {
                continue;
            }
            let u = u * (1.0 / d);
            let w = ai.cross(u);
            let m = (pi + pj) * 0.5;
            let r = 0.5 * (ri + rj);
            // The walls: the plane's offset from m along w (a flipped normal flips it), nearest to +r and to -r.
            let mut side: [Option<(usize, f64)>; 2] = [None, None];
            for q in planes {
                if axis_deg(q.n, w) > sigma_deg {
                    continue;
                }
                let o = (q.d - q.n.dot(m)) * (if q.n.dot(w) < 0.0 { -1.0 } else { 1.0 });
                let k = if o > 0.0 { 0 } else { 1 };
                let e = (o.abs() - r).abs();
                if e <= band && side[k].is_none_or(|(_, b)| e < b) {
                    side[k] = Some((q.i, e));
                }
            }
            let [Some((wa, _)), Some((wb, _))] = side else { continue };
            taken[i] = true;
            taken[j] = true;
            out.push(Slot { i, j, walls: [wa, wb], m, u, d, r });
            break;
        }
    }
    out
}

/// The slot rule (module doc): per recognised slot, its two cylinders are TAKEN (the cylinder loop skips them, so a
/// slot is never also two bosses or pockets) and, where `stand` measures the midpoint on a document face, ONE statement
/// set: the obround as the replay's own form (six points, two lines, two arcs, no constraint) on the plane, extruded by
/// the extent rule's height. No slot: nothing drawn from `ids`, nothing pushed.
#[allow(clippy::too_many_arguments)]
fn slots(
    doc: &Doc,
    r: &crate::replay::Replay,
    shapes: &[crate::intent::ShapeRef],
    planes: &[PlaneM],
    on_doc: &[bool],
    fits: &dyn Fn(&ShapeParams) -> bool,
    sigma: f64,
    sigma_deg: f64,
    ids: &mut Ids,
    out: &mut Vec<Proposal>,
) -> BTreeSet<usize> {
    let mut taken = BTreeSet::new();
    for sl in slot_of(shapes, planes, fits, sigma, sigma_deg) {
        taken.insert(sl.i);
        taken.insert(sl.j);
        let ShapeParams::Cylinder { axis, .. } = shapes[sl.i].params else { continue };
        let Ok(a) = axis.normalize() else { continue };
        // Each carried extent about m along i's unit axis; the union, or None (the centroid road at m) when either is.
        let along = |k: usize| -> Option<[f64; 2]> {
            let ShapeParams::Cylinder { point, axis, .. } = shapes[k].params else { return None };
            let [e0, e1] = shapes[k].extent?;
            let sg = if axis.dot(a) < 0.0 { -1.0 } else { 1.0 };
            let o = a.dot(point - sl.m);
            let (x0, x1) = (o + sg * e0, o + sg * e1);
            Some([x0.min(x1), x0.max(x1)])
        };
        let span = match (along(sl.i), along(sl.j)) {
            (Some(x), Some(y)) => Some([x[0].min(y[0]), x[1].max(y[1])]),
            _ => None,
        };
        let Some(Stand { pk, t, ends, h, far, inside }) = stand(doc, r, planes, on_doc, sl.m, axis, span, sigma, sigma_deg) else {
            continue;
        };
        let p = &planes[pk];
        let z = p.n * (if t > 0.0 { 1.0 } else { -1.0 });
        let z = Vec3::new(z.x + 0.0, z.y + 0.0, z.z + 0.0);
        let foot = sl.m - p.n * t;
        let Ok(x) = (sl.u - p.n * p.n.dot(sl.u)).normalize() else { continue };
        let x = Vec3::new(x.x + 0.0, x.y + 0.0, x.z + 0.0);
        let (hv, hres) = round_within(h, sigma).unwrap_or((h, 0.0));
        let (lv, lres) = round_within(sl.d, sigma).unwrap_or((sl.d, 0.0));
        let (wv, wres) = round_within(2.0 * sl.r, sigma).unwrap_or((2.0 * sl.r, 0.0));
        let e = far.map_or(0.0, |(_, e)| e);
        let src = match (far, ends) {
            (Some((qi, e)), _) => format!("far-end plane #{} (near end {} mm off)", qi, mm(e)),
            (None, Some((near, _))) => format!("the support's axial extent (near end {} mm off)", mm(near.abs())),
            (None, None) => format!("the support centroid ({} mm off)", mm(t.abs())),
        };
        let why = format!(
            "slot {}: cylinders #{} and #{} on plane #{} (a document face), walls #{} and #{}, length {} from {} mm, width {} from {} mm, extent {} mm from {}, height {}, sigma {}",
            if inside { "pocket" } else { "boss" },
            sl.i,
            sl.j,
            p.i,
            sl.walls[0],
            sl.walls[1],
            f64_text(lv),
            mm(sl.d),
            f64_text(wv),
            mm(2.0 * sl.r),
            mm(h),
            src,
            f64_text(hv),
            mm(sigma)
        );
        let conf = confidence(e.max(hres).max(lres).max(wres), sigma);
        let v3 = |v: Vec3| format!("{} {} {}", f64_text(v.x), f64_text(v.y), f64_text(v.z));
        let (hl, hw) = (f64_text(lv / 2.0), f64_text(wv / 2.0));
        let (nl, nw) = (f64_text(-lv / 2.0), f64_text(-wv / 2.0));
        let q = core::f64::consts::FRAC_PI_2;
        let sk = ids.next();
        let (c1, c2, q1, q2, q3, q4) = (ids.next(), ids.next(), ids.next(), ids.next(), ids.next(), ids.next());
        let (l1, k1, l2, k2, ex) = (ids.next(), ids.next(), ids.next(), ids.next(), ids.next());
        for stmt in [
            format!("sketch {} o=[{}] z=[{}] x=[{}]", sk, v3(foot), v3(z), v3(x)),
            format!("point {} sketch={} x={} y=0", c1, sk, nl),
            format!("point {} sketch={} x={} y=0", c2, sk, hl),
            format!("point {} sketch={} x={} y={}", q1, sk, nl, nw),
            format!("point {} sketch={} x={} y={}", q2, sk, hl, nw),
            format!("point {} sketch={} x={} y={}", q3, sk, hl, hw),
            format!("point {} sketch={} x={} y={}", q4, sk, nl, hw),
            format!("line {} sketch={} a={} b={}", l1, sk, q1, q2),
            format!("arc {} sketch={} centre={} r={} start={} end={}", k1, sk, c2, hw, f64_text(-q), f64_text(q)),
            format!("line {} sketch={} a={} b={}", l2, sk, q3, q4),
            format!("arc {} sketch={} centre={} r={} start={} end={}", k2, sk, c1, hw, f64_text(q), f64_text(3.0 * q)),
            format!("extrude {} sketch={} outer={} dir=[{}] length={}", ex, sk, l1, v3(z), f64_text(hv)),
        ] {
            out.push(Proposal { stmt, confidence: conf, why: why.clone(), shapes: modelled(&[sl.i, sl.j, sl.walls[0], sl.walls[1]], far) }); // VUGINTENTSETSIBLINGS (SR674)
        }
    }
    taken
}

// ---- VUGINTENTSETSIBLINGS (SR674): the shapes a feature set models (named tail block) ----

/// The measured shapes a feature set's statements create a face for: `own` (the cylinder, or the slot's two cylinders
/// and two walls) then the far-end plane when the extent rule measured one; the plane the feature stands on is not
/// named (the document already models it).
fn modelled(own: &[usize], far: Option<(usize, f64)>) -> Vec<usize> {
    own.iter().copied().chain(far.map(|(q, _)| q)).collect()
}
