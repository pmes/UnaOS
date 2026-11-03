// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D) — shared-core: doc_core::intent, a proposal verified against a measurement (BP-vug-7).
//!
//! VUGINTENT (LEDGER SR505): "AI proposes, geometry disposes" (docs/dev/USERLAND/VUG.md §7, principle 11). A
//! [`Proposal`] is ONE doc_core statement line (the vein_core shape: the two crates agree on the text, not a type). It
//! is lexed by the one reader ([`crate::read`] over [`crate::write`] of the document plus the line; a statement whose
//! id the document already holds REPLACES that statement in place: a changed dimension), replayed by [`crate::replay`],
//! and its residual against a [`Measurement`] is computed through geom_core's closest points: an outline TWO-SIDED
//! (every measured point to the first solved sketch's profile edges, and every profile vertex / arc midpoint to the
//! measured polyline, so a corner that slides along its own edge still counts), points one-sided against brep_core's
//! tessellation of every solid. It is accepted when the max distance is within [`K_SIGMA`] x sigma AND the RMS did not
//! grow past the document's own by more than sigma. [`verify`] never changes the document; [`apply`] is the one road
//! that does, called by the person's accept (and by [`verify_all`], which shows a batch in order, each later proposal
//! seeing the earlier accepted ones). The proposer's confidence is reported, never a gate.
//!
//! The RMS's square root is geom_core's `Vec2::mag` (a hypot accumulation); libm (VUGINTENTSCAN, SR508, as ruled)
//! gives the shape residual's angles (`atan2`, `atan`) and suggest's rounding (`round`).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::f64::consts::{PI, TAU};

use brep_core::geom_core::{Segment, Vec2, Vec3, closest_point_segment, closest_point_triangle};
use sketch_core::{Arc2, Edge, profiles};

use crate::lex::read;
use crate::replay::{Failure, Fault, Replay, replay};
use crate::write::{f64_text, write};
use crate::{Doc, Refusal, Tol, What};

/// The acceptance band: a proposal's max residual must be within `K_SIGMA` x the measurement's sigma.
pub const K_SIGMA: f64 = 3.0;

/// One proposed statement (a constraint, a round dimension, a feature), why, and the proposer's confidence.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    /// ONE doc_core statement line.
    pub stmt: String,
    pub confidence: f64,
    pub why: String,
}

/// What the geometry is checked against.
#[derive(Clone, Debug, PartialEq)]
pub enum Measurement {
    /// A CLOSED polyline in the plane of the document's first solved sketch (a photo outline), mm.
    Outline { pts: Vec<Vec2>, sigma_mm: f64 },
    /// Points on the part's surface (a scan), mm, in the document's world frame.
    Points { pts: Vec<Vec3>, sigma_mm: f64 },
    /// VUGINTENTSCAN (SR508): scan_core's detected shapes (SR497) copied as [`ShapeRef`]s, in the document's world
    /// frame; an angle counts `sigma_mm` per `sigma_deg` in the residual.
    Shapes { shapes: Vec<ShapeRef>, sigma_mm: f64, sigma_deg: f64 },
}

impl Measurement {
    pub fn sigma_mm(&self) -> f64 {
        match self {
            Measurement::Outline { sigma_mm, .. } | Measurement::Points { sigma_mm, .. } => *sigma_mm,
            Measurement::Shapes { sigma_mm, .. } => *sigma_mm, // VUGINTENTSCAN (SR508)
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Measurement::Outline { pts, .. } => pts.len(),
            Measurement::Points { pts, .. } => pts.len(),
            Measurement::Shapes { shapes, .. } => shapes.len(), // VUGINTENTSCAN (SR508)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Why a proposal is refused, by name.
#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    /// Empty, a comment only, or more than one line.
    NotOneLine,
    /// The reader refused it (the line / column are the written candidate's).
    Lex(Refusal),
    /// Replay failures the document did not have (Conflicting / BadRef / Upstream / ...), by statement id.
    Replay(Vec<Failure>),
    /// The solve names more redundant constraints than the document's did: the proposal says nothing new.
    Redundant,
    /// A measurement with no points, or a sigma that is not a finite positive number.
    BadMeasurement,
    /// Nothing to measure against: no solved sketch with a profile (outline), no solid (points).
    NoGeometry,
    Tess(brep_core::Refusal),
    /// The max distance is past `K_SIGMA` x sigma.
    Residual { max_mm: f64, limit_mm: f64 },
    /// The RMS grew past the document's own by more than sigma.
    Worse { rms_mm: f64, before_mm: f64, sigma_mm: f64 },
    /// VUGINTENTSCAN (SR508): measured shape `index` has no replayed face of its kind.
    UnmatchedShape { index: usize, kind: &'static str },
}

impl Reason {
    /// The one-line text of the refusal.
    pub fn text(&self) -> String {
        match self {
            Reason::NotOneLine => String::from("not-one-line"),
            Reason::Lex(r) => format!("lex {:?} at {}:{}", r.what, r.line, r.col),
            Reason::Replay(fs) => {
                let v: Vec<String> = fs.iter().map(|f| format!("{} {}", fault_name(&f.fault), f.id)).collect();
                v.join(", ")
            }
            Reason::Redundant => String::from("redundant"),
            Reason::BadMeasurement => String::from("bad-measurement"),
            Reason::NoGeometry => String::from("no-geometry"),
            Reason::Tess(r) => format!("tess {:?}", r),
            Reason::Residual { max_mm, limit_mm } => {
                format!("residual max={} limit={}", f64_text(*max_mm), f64_text(*limit_mm))
            }
            Reason::Worse { rms_mm, before_mm, sigma_mm } => format!(
                "worse rms={} before={} sigma={}",
                f64_text(*rms_mm),
                f64_text(*before_mm),
                f64_text(*sigma_mm)
            ),
            Reason::UnmatchedShape { index, kind } => format!("unmatched-shape {} #{}", kind, index), // VUGINTENTSCAN
        }
    }
}

fn fault_name(f: &Fault) -> String {
    match f {
        Fault::BadRef(n) => format!("bad-ref({})", n),
        Fault::Conflicting => String::from("conflicting"),
        Fault::Diverged => String::from("diverged"),
        Fault::NoProfile => String::from("no-profile"),
        Fault::Upstream(u) => format!("upstream({})", u),
        Fault::Brep(e) => format!("brep {:?}", e),
        Fault::Make(e) => format!("make {:?}", e),
        Fault::NoMachine => String::from("no-machine"),
    }
}

/// The verdict on one proposal.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// `residual_mm` is the RMS, `max_mm` the largest distance, `residual_sigma` = max / sigma.
    Accept { residual_mm: f64, max_mm: f64, residual_sigma: f64 },
    Refuse { why: Reason },
}

/// The document with a proposal in it: appended, or replacing the statement of the same id in place. The one road
/// that changes a document; the person's accept calls it.
pub fn apply(doc: &Doc, p: &Proposal) -> Result<Doc, Reason> {
    let line = p.stmt.trim();
    if line.is_empty() || line.contains('\n') || line.contains('\r') {
        return Err(Reason::NotOneLine);
    }
    let base = write(doc);
    let at = doc.stmts.len() as u32 + 2;
    let mut appended = base.clone();
    appended.push_str(line);
    appended.push('\n');
    match read(&appended) {
        Ok(d) if d.stmts.len() == doc.stmts.len() + 1 => Ok(d),
        Ok(_) => Err(Reason::NotOneLine),
        Err(r) => {
            let dup = match &r.what {
                What::DuplicateId(id) if r.line == at => Some(id.clone()),
                _ => None,
            };
            let Some(i) = dup.and_then(|id| doc.stmts.iter().position(|s| s.id() == Some(id.as_str()))) else {
                return Err(Reason::Lex(r));
            };
            // Line 0 of `write` is the header; statement i is on line i + 1 (0-based).
            let mut out = String::new();
            for (n, l) in base.lines().enumerate() {
                out.push_str(if n == i + 1 { line } else { l });
                out.push('\n');
            }
            match read(&out) {
                Ok(d) if d.stmts.len() == doc.stmts.len() => Ok(d),
                Ok(_) => Err(Reason::NotOneLine),
                Err(r) => Err(Reason::Lex(r)),
            }
        }
    }
}

/// Verify one proposal against the measurement under `tol`. The document is not changed.
pub fn verify(doc: &Doc, p: &Proposal, m: &Measurement, tol: Tol) -> Verdict {
    match judge(doc, p, m, tol) {
        Ok(v) => v,
        Err(why) => Verdict::Refuse { why },
    }
}

fn judge(doc: &Doc, p: &Proposal, m: &Measurement, tol: Tol) -> Result<Verdict, Reason> {
    let sigma = m.sigma_mm();
    if m.is_empty() || !(sigma.is_finite() && sigma > 0.0) {
        return Err(Reason::BadMeasurement);
    }
    let cand = apply(doc, p)?;
    let before = replay(doc, tol);
    let after = replay(&cand, tol);
    let fresh: Vec<Failure> = after
        .failures
        .iter()
        .filter(|f| !before.failures.iter().any(|b| b.id == f.id && b.fault == f.fault))
        .cloned()
        .collect();
    if !fresh.is_empty() {
        return Err(Reason::Replay(fresh));
    }
    if redundant(&after) > redundant(&before) {
        return Err(Reason::Redundant);
    }
    let fit = residual(&after, m, tol)?;
    let limit = K_SIGMA * sigma;
    if fit.max > limit {
        return Err(Reason::Residual { max_mm: fit.max, limit_mm: limit });
    }
    if let Ok(b) = residual(&before, m, tol) {
        if fit.rms > b.rms + sigma {
            return Err(Reason::Worse { rms_mm: fit.rms, before_mm: b.rms, sigma_mm: sigma });
        }
    }
    Ok(Verdict::Accept { residual_mm: fit.rms, max_mm: fit.max, residual_sigma: fit.max / sigma })
}

fn redundant(r: &Replay) -> usize {
    r.sketches.iter().map(|s| s.solved.redundant.len()).sum()
}

/// A batch shown in order: the document with every accepted proposal applied, and each (statement, confidence,
/// verdict).
#[derive(Clone, Debug)]
pub struct Batch {
    pub doc: Doc,
    pub entries: Vec<(String, f64, Verdict)>,
}

impl Batch {
    pub fn accepted(&self) -> usize {
        self.entries.iter().filter(|(_, _, v)| matches!(v, Verdict::Accept { .. })).count()
    }

    /// `intent: proposed=<n> accepted=<a> refused=<r>`, then one line per proposal.
    pub fn text(&self) -> String {
        let n = self.entries.len();
        let a = self.accepted();
        let mut out = format!("intent: proposed={} accepted={} refused={}\n", n, a, n - a);
        for (s, c, v) in &self.entries {
            match v {
                Verdict::Accept { residual_mm, max_mm, residual_sigma } => out.push_str(&format!(
                    "accept {} rms={} max={} sigma={} confidence={}\n",
                    s.trim(),
                    f64_text(*residual_mm),
                    f64_text(*max_mm),
                    f64_text(*residual_sigma),
                    f64_text(*c)
                )),
                Verdict::Refuse { why } => out.push_str(&format!("refuse {} {}\n", s.trim(), why.text())),
            }
        }
        out
    }
}

/// Verify each proposal in order; an accepted one is applied before the next is verified.
pub fn verify_all(doc: &Doc, ps: &[Proposal], m: &Measurement, tol: Tol) -> Batch {
    let mut cur = doc.clone();
    let mut entries = Vec::new();
    for p in ps {
        let v = verify(&cur, p, m, tol);
        if let Verdict::Accept { .. } = v {
            if let Ok(d) = apply(&cur, p) {
                cur = d;
            }
        }
        entries.push((p.stmt.clone(), p.confidence, v));
    }
    Batch { doc: cur, entries }
}

struct Fit {
    rms: f64,
    max: f64,
}

/// The distances from the measurement to the replayed geometry.
fn residual(r: &Replay, m: &Measurement, tol: Tol) -> Result<Fit, Reason> {
    let mut d: Vec<f64> = Vec::new();
    match m {
        Measurement::Outline { pts, .. } => {
            let Some(s) = r.sketches.first() else { return Err(Reason::NoGeometry) };
            let mut edges: Vec<Edge> = Vec::new();
            for p in profiles(&s.sketch, tol) {
                edges.extend(p.outer.edges.iter().copied());
                for h in &p.holes {
                    edges.extend(h.edges.iter().copied());
                }
            }
            if edges.is_empty() {
                return Err(Reason::NoGeometry);
            }
            for &q in pts {
                d.push(edges.iter().map(|e| to_edge(q, e, tol)).fold(f64::INFINITY, f64::min));
            }
            for e in &edges {
                d.push(to_polyline(e.start(), pts));
                if let Edge::Arc { arc, .. } = e {
                    d.push(to_polyline(arc.point_at(0.5), pts));
                }
            }
        }
        Measurement::Points { pts, sigma_mm } => {
            if r.solids.is_empty() {
                return Err(Reason::NoGeometry);
            }
            let chord = (sigma_mm * 0.25).max(4.0 * tol.lin);
            let mut tris: Vec<[Vec3; 3]> = Vec::new();
            for s in &r.solids {
                let t = brep_core::tessellate(&s.brep, chord, tol).map_err(Reason::Tess)?;
                let at = |k: u32| t.mesh.positions[k as usize];
                for tr in &t.mesh.triangles {
                    tris.push([at(tr[0]), at(tr[1]), at(tr[2])]);
                }
            }
            for &q in pts {
                let mut best = f64::INFINITY;
                for t in &tris {
                    if let Ok((c, _)) = closest_point_triangle(q, t[0], t[1], t[2], tol) {
                        best = best.min(q.dist(c));
                    }
                }
                if !best.is_finite() {
                    return Err(Reason::NoGeometry);
                }
                d.push(best);
            }
        }
        // VUGINTENTSCAN (SR508): each shape against its nearest replayed face of the same kind.
        Measurement::Shapes { shapes, sigma_mm, sigma_deg } => {
            shape_residuals(r, shapes, *sigma_mm, *sigma_deg, &mut d)?
        }
    }
    Ok(fit(&d))
}

/// RMS and max; the RMS as |d| / sqrt(n) with both norms accumulated through geom_core's `Vec2::mag`.
fn fit(d: &[f64]) -> Fit {
    let max = d.iter().copied().fold(0.0, f64::max);
    let norm = d.iter().fold(0.0, |acc, &x| Vec2::new(acc, x).mag());
    let root_n = d.iter().fold(0.0, |acc, _| Vec2::new(acc, 1.0).mag());
    Fit { rms: if root_n > 0.0 { norm / root_n } else { 0.0 }, max }
}

fn lift(v: Vec2) -> Vec3 {
    Vec3::new(v.x, v.y, 0.0)
}

fn to_segment(q: Vec2, a: Vec2, b: Vec2) -> f64 {
    let (c, _) = closest_point_segment(lift(q), &Segment::new(lift(a), lift(b)));
    lift(q).dist(c)
}

/// The distance from `q` to a closed polyline (one point: the distance to it).
fn to_polyline(q: Vec2, pts: &[Vec2]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| to_segment(q, pts[i], pts[(i + 1) % n])).fold(f64::INFINITY, f64::min)
}

fn to_edge(q: Vec2, e: &Edge, tol: Tol) -> f64 {
    match e {
        Edge::Line { from, to, .. } => to_segment(q, *from, *to),
        Edge::Arc { arc, .. } => to_arc(q, arc, e.start(), e.end(), tol),
    }
}

/// The exact distance from `q` to an arc: inside its sweep (two cross-product sign tests, no angle) the distance to
/// the circle, else the nearer end.
fn to_arc(q: Vec2, arc: &Arc2, s: Vec2, e: Vec2, tol: Tol) -> f64 {
    let c = arc.centre;
    let v = q - c;
    // CCW from `a` to `b`.
    let (a, b) = if arc.sweep >= 0.0 { (s - c, e - c) } else { (e - c, s - c) };
    let sweep = arc.sweep.abs();
    let inside = if sweep >= TAU - tol.ang {
        true
    } else if sweep <= PI {
        a.cross(v) >= 0.0 && v.cross(b) >= 0.0
    } else {
        !(b.cross(v) > 0.0 && v.cross(a) > 0.0)
    };
    if inside { (v.mag() - arc.radius).abs() } else { q.dist(s).min(q.dist(e)) }
}

// ---- VUGINTENTSCAN (SR508): the residual against scan shapes and the measurement table ----

/// A detected primitive's parameters: scan_core's `Primitive` fields and conventions, copied (doc_core takes no
/// scan_core: the caller copies a `Shape` into a [`ShapeRef`]). The plane is `normal . p = d`; a cylinder is a point on
/// its axis, the axis and the radius; a cone its apex, its axis into the opening and its half-angle (radians); mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShapeParams {
    Plane { normal: Vec3, d: f64 },
    Cylinder { point: Vec3, axis: Vec3, radius: f64 },
    Sphere { centre: Vec3, radius: f64 },
    Cone { apex: Vec3, axis: Vec3, half_angle: f64 },
}

impl ShapeParams {
    /// scan_core's `Primitive::name` spelling.
    pub fn kind(&self) -> &'static str {
        match self {
            ShapeParams::Plane { .. } => "plane",
            ShapeParams::Cylinder { .. } => "cylinder",
            ShapeParams::Sphere { .. } => "sphere",
            ShapeParams::Cone { .. } => "cone",
        }
    }
}

/// One measured shape: its parameters, the points that support it, and its fit rms (mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeRef {
    pub params: ShapeParams,
    pub support: u32,
    pub rms_mm: f64,
}

impl ShapeRef {
    pub fn kind(&self) -> &'static str {
        self.params.kind()
    }
}

const DEG: f64 = 180.0 / PI;

/// The unsigned angle between two axes (degrees, 0..=90); a zero axis is 90.
fn axis_deg(a: Vec3, b: Vec3) -> f64 {
    match (a.normalize(), b.normalize()) {
        (Ok(a), Ok(b)) => libm::atan2(a.cross(b).mag(), a.dot(b).abs()) * DEG,
        _ => 90.0,
    }
}

/// (distance in mm, angle in degrees) between a measured shape and a face surface of the same kind; None for another
/// kind (a cone face with no slope is no cone partner).
fn pair(s: &ShapeParams, f: &brep_core::Surface) -> Option<(f64, f64)> {
    use brep_core::Surface;
    match (s, f) {
        (ShapeParams::Plane { normal, d }, Surface::Plane { f }) => {
            let m = normal.mag();
            if !(m > 0.0) {
                return None;
            }
            let n = *normal * (1.0 / m);
            Some(((n.dot(f.o) - d / m).abs(), axis_deg(n, f.z)))
        }
        (ShapeParams::Cylinder { point, axis, radius }, Surface::Cylinder { f, r }) => {
            let v = *point - f.o;
            let off = (v - f.z * v.dot(f.z)).mag();
            Some((off.max((radius - r).abs()), axis_deg(*axis, f.z)))
        }
        (ShapeParams::Sphere { centre, radius }, Surface::Sphere { f, r }) => {
            Some((centre.dist(f.o).max((radius - r).abs()), 0.0))
        }
        (ShapeParams::Cone { apex, axis, half_angle }, Surface::Cone { f, r0, k }) => {
            if !(k.is_finite() && *k != 0.0) {
                return None;
            }
            let fa = f.o - f.z * (r0 / k);
            let ha = (half_angle - libm::atan(k.abs())).abs() * DEG;
            Some((apex.dist(fa), axis_deg(*axis, f.z).max(ha)))
        }
        _ => None,
    }
}

/// Every shape matched to the replayed face of its kind with the smallest `max(dist_mm, ang_deg * sigma_mm /
/// sigma_deg)`; that value is the shape's residual (so judge's one `K_SIGMA * sigma_mm` limit holds both).
fn shape_residuals(
    r: &Replay,
    shapes: &[ShapeRef],
    sigma_mm: f64,
    sigma_deg: f64,
    d: &mut Vec<f64>,
) -> Result<(), Reason> {
    if !(sigma_deg.is_finite() && sigma_deg > 0.0) {
        return Err(Reason::BadMeasurement);
    }
    if r.solids.is_empty() {
        return Err(Reason::NoGeometry);
    }
    let per_deg = sigma_mm / sigma_deg;
    for (i, s) in shapes.iter().enumerate() {
        let mut best: Option<f64> = None;
        for solid in &r.solids {
            for face in &solid.brep.faces {
                if let Some((mm, deg)) = pair(&s.params, &face.surface) {
                    let v = mm.max(deg * per_deg);
                    best = Some(best.map_or(v, |b: f64| b.min(v)));
                }
            }
        }
        match best {
            Some(v) => d.push(v),
            None => return Err(Reason::UnmatchedShape { index: i, kind: s.kind() }),
        }
    }
    Ok(())
}

/// `v` to `places` decimals, trailing zeros, a bare point and a negative zero dropped.
fn num(v: f64, places: usize) -> String {
    let mut t = format!("{:.*}", places, v);
    if t.contains('.') {
        while t.ends_with('0') {
            t.pop();
        }
        if t.ends_with('.') {
            t.pop();
        }
    }
    if t == "-0" { String::from("0") } else { t }
}

fn v3(v: Vec3, places: usize) -> String {
    format!("({},{},{})", num(v.x, places), num(v.y, places), num(v.z, places))
}

fn v2(v: Vec2) -> String {
    format!("({},{})", num(v.x, 3), num(v.y, 3))
}

/// The measurement table vein_core's `Propose.measurements` carries: a header, then one line per shape (or per outline
/// segment; a point cloud as its extent), mm to 3 places, unit vectors to 4, degrees to 2. Deterministic.
pub fn measurements_text(m: &Measurement) -> String {
    let mut out = String::new();
    match m {
        Measurement::Outline { pts, sigma_mm } => {
            out.push_str(&format!("outline: n={} sigma={}\n", pts.len(), num(*sigma_mm, 3)));
            let n = pts.len();
            for i in 0..n {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                out.push_str(&format!("edge a={} b={} len={}\n", v2(a), v2(b), num(a.dist(b), 3)));
            }
        }
        Measurement::Points { pts, sigma_mm } => {
            out.push_str(&format!("points: n={} sigma={}\n", pts.len(), num(*sigma_mm, 3)));
            if let Some(&f) = pts.first() {
                let (mut lo, mut hi) = (f, f);
                for p in pts {
                    lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
                    hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
                }
                out.push_str(&format!("extent min={} max={}\n", v3(lo, 3), v3(hi, 3)));
            }
        }
        Measurement::Shapes { shapes, sigma_mm, sigma_deg } => {
            out.push_str(&format!(
                "shapes: n={} sigma={} sigma_deg={}\n",
                shapes.len(),
                num(*sigma_mm, 3),
                num(*sigma_deg, 2)
            ));
            for s in shapes {
                let head = match s.params {
                    ShapeParams::Plane { normal, d } => format!("plane n={} d={}", v3(normal, 4), num(d, 3)),
                    ShapeParams::Cylinder { point, axis, radius } => {
                        format!("cylinder axis={} dir={} r={}", v3(point, 3), v3(axis, 4), num(radius, 3))
                    }
                    ShapeParams::Sphere { centre, radius } => {
                        format!("sphere centre={} r={}", v3(centre, 3), num(radius, 3))
                    }
                    ShapeParams::Cone { apex, axis, half_angle } => format!(
                        "cone apex={} dir={} half_angle_deg={}",
                        v3(apex, 3),
                        v3(axis, 4),
                        num(half_angle * DEG, 2)
                    ),
                };
                out.push_str(&format!("{} support={} rms={}\n", head, s.support, num(s.rms_mm, 3)));
            }
        }
    }
    out
}
