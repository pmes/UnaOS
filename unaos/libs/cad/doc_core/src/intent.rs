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
//! VUGINTENTHOLES (LEDGER SR507): the Outline is LOOPS (the outer, then every hole, as photo_core's `Measured.loops`
//! gives them). Each replayed profile loop (outer and each hole) is paired one-to-one with the measured loop whose
//! centroid is nearest within the larger loop's extent (`pair_loops`); a loop left without a partner on either side
//! is refused by name ([`Reason::UnmatchedLoop`]); the two-sided residual runs per pair, max and RMS over all pairs.
//!
//! VUGINTENTPOINTS (LEDGER SR523): the Points measurement is TWO-SIDED. Forward, every point to the replayed surface
//! through mesh_core's `Bvh::closest` (SR510) over the solids' tessellation (the SR505 chord, one mesh), warm-started
//! from the previous point's answer; the same geom_core closest point and distance as the SR505 brute force (a value
//! can differ only in the last ulp where two triangles tie within the box test's rounding). Back, the surface to the
//! points: one sample per triangle (its centroid) of the proposal's replay, measured to the nearest scan point through
//! a private median-split point tree, minus the scan's spacing `h` (the median nearest-other-point distance), floored
//! at 0. Only CHANGED surface counts: a sample within sigma of the document's own replayed surface is the document's,
//! never held against the proposal (a partial scan with the bottom on the table is not refused). Shrinking was already
//! caught forward; the back side catches growth where no point is. Both sides go into the one max / RMS and limit.
//!
//! The RMS's square root is geom_core's `Vec2::mag` (a hypot accumulation); libm (VUGINTENTSCAN, SR508, as ruled)
//! gives the shape residual's angles (`atan2`, `atan`) and suggest's rounding (`round`).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::f64::consts::{PI, TAU};

use brep_core::geom_core::{Aabb3, Segment, Vec2, Vec3, closest_point_segment};
use brep_core::mesh_core::{Bvh, Mesh}; // VUGINTENTPOINTS (SR523)
use sketch_core::{Arc2, Edge, Loop, profiles};

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
    /// CLOSED polylines in the plane of the document's first solved sketch (a photo outline), mm: the outer, then
    /// every hole (photo_core `Measured.loops`). One loop: [`Measurement::outline`].
    Outline { loops: Vec<Vec<Vec2>>, sigma_mm: f64 },
    /// Points on the part's surface (a scan), mm, in the document's world frame.
    Points { pts: Vec<Vec3>, sigma_mm: f64 },
    /// VUGINTENTSCAN (SR508): scan_core's detected shapes (SR497) copied as [`ShapeRef`]s, in the document's world
    /// frame; an angle counts `sigma_mm` per `sigma_deg` in the residual.
    Shapes { shapes: Vec<ShapeRef>, sigma_mm: f64, sigma_deg: f64 },
}

impl Measurement {
    /// VUGINTENTHOLES (SR507): ONE closed polyline (a part without holes) as an Outline of one loop.
    pub fn outline(pts: Vec<Vec2>, sigma_mm: f64) -> Measurement {
        Measurement::Outline { loops: alloc::vec![pts], sigma_mm }
    }

    pub fn sigma_mm(&self) -> f64 {
        match self {
            Measurement::Outline { sigma_mm, .. } | Measurement::Points { sigma_mm, .. } => *sigma_mm,
            Measurement::Shapes { sigma_mm, .. } => *sigma_mm, // VUGINTENTSCAN (SR508)
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Measurement::Outline { loops, .. } => loops.iter().map(|l| l.len()).sum(),
            Measurement::Points { pts, .. } => pts.len(),
            Measurement::Shapes { shapes, .. } => shapes.len(), // VUGINTENTSCAN (SR508)
        }
    }

    /// No points, or an Outline loop with no points (VUGINTENTHOLES SR507).
    pub fn is_empty(&self) -> bool {
        self.len() == 0 || matches!(self, Measurement::Outline { loops, .. } if loops.iter().any(|l| l.is_empty()))
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
    /// VUGINTENTHOLES (SR507): a replayed profile loop or a measured loop has no partner (no loop of the other side
    /// whose centroid is within the larger extent, one-to-one); the counts are the replayed and measured loops.
    UnmatchedLoop { replayed: usize, measured: usize },
    /// VUGINTENTSCAN (SR508): measured shape `index` has no replayed face of its kind. VUG-INTENT-SCAN-UNMATCHED (SR515):
    /// only when NO measured shape has one (the proposal cannot be judged; `index` / `kind` are the first shape's); a
    /// shape alone without a partner is left out of the residual and reported in [`Batch::unmatched`].
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
            Reason::UnmatchedLoop { replayed, measured } => {
                format!("unmatched-loop replayed={} measured={}", replayed, measured)
            }
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
        // VUGFACEFILLET (SR511): an edge name that no longer resolves, or resolves twice.
        Fault::NoSuchEdge(n) => format!("no-such-edge({})", n),
        Fault::AmbiguousEdge(n) => format!("ambiguous-edge({})", n),
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
    let fit = residual(&after, m, tol, Some(&before))?;
    let limit = K_SIGMA * sigma;
    if fit.max > limit {
        return Err(Reason::Residual { max_mm: fit.max, limit_mm: limit });
    }
    if let Ok(b) = residual(&before, m, tol, None) {
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
    /// VUG-INTENT-SCAN-UNMATCHED (SR515): each measured shape (index, shape) with no face of its kind in the document's
    /// replay, left out of every residual and reported; empty for any other measurement.
    pub unmatched: Vec<(usize, ShapeRef)>,
}

impl Batch {
    pub fn accepted(&self) -> usize {
        self.entries.iter().filter(|(_, _, v)| matches!(v, Verdict::Accept { .. })).count()
    }

    /// `intent: proposed=<n> accepted=<a> refused=<r>`, one `unmatched <kind> #<i> support=<n> rms=<r>` line per
    /// unmatched shape (VUG-INTENT-SCAN-UNMATCHED, SR515), then one line per proposal.
    pub fn text(&self) -> String {
        let n = self.entries.len();
        let a = self.accepted();
        let mut out = format!("intent: proposed={} accepted={} refused={}\n", n, a, n - a);
        for (i, s) in &self.unmatched {
            out.push_str(&format!("unmatched {} #{} support={} rms={}\n", s.kind(), i, s.support, f64_text(s.rms_mm)));
        }
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
    // VUG-INTENT-SCAN-UNMATCHED (SR515): the shapes the document's own replay has no face of their kind for.
    let unmatched = match m {
        Measurement::Shapes { shapes, .. } => {
            let r = replay(doc, tol);
            shapes.iter().enumerate().filter(|(_, s)| !has_partner(&r, s)).map(|(i, s)| (i, *s)).collect()
        }
        _ => Vec::new(),
    };
    Batch { doc: cur, entries, unmatched }
}

struct Fit {
    rms: f64,
    max: f64,
}

/// The distances from the measurement to the replayed geometry; with a `base` (the document's own replay) the Points
/// measurement also runs its surface side (VUGINTENTPOINTS SR523).
fn residual(r: &Replay, m: &Measurement, tol: Tol, base: Option<&Replay>) -> Result<Fit, Reason> {
    let mut d: Vec<f64> = Vec::new();
    match m {
        Measurement::Outline { loops, .. } => {
            let Some(s) = r.sketches.first() else { return Err(Reason::NoGeometry) };
            // VUGINTENTHOLES (SR507): every replayed profile loop (outer, then each hole), paired with a measured loop.
            let ps = profiles(&s.sketch, tol);
            let mut mine: Vec<&Loop> = Vec::new();
            for p in &ps {
                mine.push(&p.outer);
                mine.extend(p.holes.iter());
            }
            mine.retain(|l| !l.edges.is_empty());
            if mine.is_empty() {
                return Err(Reason::NoGeometry);
            }
            let Some(pairs) = pair_loops(&mine, loops) else {
                return Err(Reason::UnmatchedLoop { replayed: mine.len(), measured: loops.len() });
            };
            for (i, j) in pairs {
                let (edges, pts) = (&mine[i].edges, &loops[j]);
                for &q in pts {
                    d.push(edges.iter().map(|e| to_edge(q, e, tol)).fold(f64::INFINITY, f64::min));
                }
                for e in edges {
                    d.push(to_polyline(e.start(), pts));
                    if let Edge::Arc { arc, .. } = e {
                        d.push(to_polyline(arc.point_at(0.5), pts));
                    }
                }
            }
        }
        // VUGINTENTPOINTS (SR523): forward through Bvh::closest, then the changed surface back to the points.
        Measurement::Points { pts, sigma_mm } => {
            if r.solids.is_empty() {
                return Err(Reason::NoGeometry);
            }
            let chord = (sigma_mm * 0.25).max(4.0 * tol.lin);
            let mesh = solid_mesh(r, chord, tol)?;
            let bvh = Bvh::build(&mesh);
            forward(&mesh, &bvh, pts, tol, &mut d)?;
            if let Some(b) = base {
                surface_side(&mesh, b, pts, *sigma_mm, chord, tol, &mut d);
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

// VUGINTENTHOLES (SR507): loop pairing for the Outline residual.

/// Arc samples per edge for the pairing centroid only (never a distance).
const PAIR_SAMPLES: usize = 8;

/// A loop's key: its shoelace area centroid (the vertex mean when the area vanishes) and its extent (the largest
/// distance from that centroid to a vertex).
fn loop_key(pts: &[Vec2]) -> (Vec2, f64) {
    let n = pts.len();
    let (mut a2, mut cx, mut cy) = (0.0, 0.0, 0.0);
    let (mut mx, mut my) = (0.0, 0.0);
    for i in 0..n {
        let (p, q) = (pts[i], pts[(i + 1) % n]);
        let c = p.cross(q);
        a2 += c;
        cx += (p.x + q.x) * c;
        cy += (p.y + q.y) * c;
        mx += p.x;
        my += p.y;
    }
    let c = if a2.abs() > 2e-12 {
        Vec2::new(cx / (3.0 * a2), cy / (3.0 * a2))
    } else if n > 0 {
        Vec2::new(mx / n as f64, my / n as f64)
    } else {
        Vec2::new(0.0, 0.0)
    };
    let e = pts.iter().map(|&p| p.dist(c)).fold(0.0, f64::max);
    (c, e)
}

/// The points of a replayed loop for its key: each edge's start, and on an arc `PAIR_SAMPLES` points along it.
fn replayed_pts(l: &Loop) -> Vec<Vec2> {
    let mut out = Vec::new();
    for e in &l.edges {
        out.push(e.start());
        if let Edge::Arc { arc, .. } = e {
            for k in 1..PAIR_SAMPLES {
                out.push(arc.point_at(k as f64 / PAIR_SAMPLES as f64));
            }
        }
    }
    out
}

/// One-to-one (replayed, measured) pairs: a candidate's centroids are within the larger loop's extent; candidates
/// are taken greedily by centroid distance plus extent difference (the extent term only orders). `None` when any
/// loop on either side is left without a partner.
fn pair_loops(mine: &[&Loop], measured: &[Vec<Vec2>]) -> Option<Vec<(usize, usize)>> {
    let a: Vec<(Vec2, f64)> = mine.iter().map(|l| loop_key(&replayed_pts(l))).collect();
    let b: Vec<(Vec2, f64)> = measured.iter().map(|m| loop_key(m)).collect();
    let mut cand: Vec<(f64, usize, usize)> = Vec::new();
    for (i, &(ci, ei)) in a.iter().enumerate() {
        for (j, &(cj, ej)) in b.iter().enumerate() {
            let dc = ci.dist(cj);
            if dc <= ei.max(ej) {
                cand.push((dc + (ei - ej).abs(), i, j));
            }
        }
    }
    cand.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(core::cmp::Ordering::Equal));
    let (mut used_a, mut used_b) = (alloc::vec![false; a.len()], alloc::vec![false; b.len()]);
    let mut pairs = Vec::new();
    for (_, i, j) in cand {
        if !used_a[i] && !used_b[j] {
            used_a[i] = true;
            used_b[j] = true;
            pairs.push((i, j));
        }
    }
    if pairs.len() == a.len() && pairs.len() == b.len() { Some(pairs) } else { None }
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

/// Whether any replayed face is of `s`'s kind (VUG-INTENT-SCAN-UNMATCHED, SR515).
fn has_partner(r: &Replay, s: &ShapeRef) -> bool {
    r.solids.iter().any(|solid| solid.brep.faces.iter().any(|f| pair(&s.params, &f.surface).is_some()))
}

/// Every shape matched to the replayed face of its kind with the smallest `max(dist_mm, ang_deg * sigma_mm /
/// sigma_deg)`; that value is the shape's residual (so judge's one `K_SIGMA * sigma_mm` limit holds both).
/// VUG-INTENT-SCAN-UNMATCHED (SR515): a shape with no face of its kind (a TSDF-rounded corner read as a cone, an open
/// rim as a sphere) is left out, and [`verify_all`] reports it; only when no shape matches is the proposal refused.
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
    let mut matched = 0usize;
    for s in shapes {
        let mut best: Option<f64> = None;
        for solid in &r.solids {
            for face in &solid.brep.faces {
                if let Some((mm, deg)) = pair(&s.params, &face.surface) {
                    let v = mm.max(deg * per_deg);
                    best = Some(best.map_or(v, |b: f64| b.min(v)));
                }
            }
        }
        if let Some(v) = best {
            d.push(v);
            matched += 1;
        }
    }
    match shapes.first() {
        Some(s) if matched == 0 => Err(Reason::UnmatchedShape { index: 0, kind: s.kind() }),
        _ => Ok(()),
    }
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
        Measurement::Outline { loops, sigma_mm } => {
            // VUGINTENTHOLES (SR507) composed: the outer loop, then every hole, each with its edges.
            let total: usize = loops.iter().map(|l| l.len()).sum();
            out.push_str(&format!("outline: loops={} n={} sigma={}\n", loops.len(), total, num(*sigma_mm, 3)));
            for (k, pts) in loops.iter().enumerate() {
                out.push_str(&format!("loop {} n={}\n", k, pts.len()));
                let n = pts.len();
                for i in 0..n {
                    let (a, b) = (pts[i], pts[(i + 1) % n]);
                    out.push_str(&format!("edge a={} b={} len={}\n", v2(a), v2(b), num(a.dist(b), 3)));
                }
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

// ---- VUGINTENTPOINTS (SR523): the two-sided Points residual ----

/// Every solid of the replay tessellated at `chord` into ONE mesh (indices offset per solid), in solid order.
fn solid_mesh(r: &Replay, chord: f64, tol: Tol) -> Result<Mesh, Reason> {
    let mut mesh = Mesh::default();
    for s in &r.solids {
        let t = brep_core::tessellate(&s.brep, chord, tol).map_err(Reason::Tess)?;
        let off = mesh.positions.len() as u32;
        mesh.positions.extend_from_slice(&t.mesh.positions);
        mesh.triangles.extend(t.mesh.triangles.iter().map(|tr| [tr[0] + off, tr[1] + off, tr[2] + off]));
    }
    Ok(mesh)
}

/// Each point's distance to the mesh through `Bvh::closest`; the reach starts at the previous answer plus the step
/// between the points (the true nearest is within it), a miss at that bound re-queries unbounded, and a point with
/// no non-degenerate triangle at all is NoGeometry (SR505's rule).
fn forward(mesh: &Mesh, bvh: &Bvh, pts: &[Vec3], tol: Tol, d: &mut Vec<f64>) -> Result<(), Reason> {
    let mut prev: Option<(Vec3, f64)> = None;
    for &q in pts {
        let reach = prev.map_or(f64::INFINITY, |(p, dist)| dist + q.dist(p));
        let near = bvh.closest(mesh, q, reach, tol).or_else(|| bvh.closest(mesh, q, f64::INFINITY, tol));
        let Some(n) = near else { return Err(Reason::NoGeometry) };
        d.push(n.dist);
        prev = Some((q, n.dist));
    }
    Ok(())
}

/// The surface side: one sample per triangle of `mesh` (its centroid; the density is the forward side's chord), a
/// sample within `sigma` of the base replay's surface skipped (the document's own), every other sample's distance to
/// the nearest scan point minus the scan's spacing, floored at 0. A base with no solid, or one that does not
/// tessellate, skips nothing.
fn surface_side(mesh: &Mesh, base: &Replay, pts: &[Vec3], sigma: f64, chord: f64, tol: Tol, d: &mut Vec<f64>) {
    let own = if base.solids.is_empty() { None } else { solid_mesh(base, chord, tol).ok() };
    let own_bvh = own.as_ref().map(Bvh::build);
    let tree = PointTree::build(pts);
    let h = spacing(&tree, pts);
    for t in 0..mesh.triangles.len() {
        let Some([a, b, c]) = mesh.corners(t) else { continue };
        let g = (a + b + c) * (1.0 / 3.0);
        if let (Some(m), Some(bv)) = (own.as_ref(), own_bvh.as_ref()) {
            if bv.closest(m, g, sigma, tol).is_some() {
                continue;
            }
        }
        if let Some((_, dist)) = tree.nearest(pts, g, None) {
            d.push((dist - h).max(0.0));
        }
    }
}

/// At most this many evenly strided scan points estimate the spacing.
const SPACING_SAMPLES: usize = 1024;

/// The scan's spacing `h`: the median nearest-OTHER-point distance over at most [`SPACING_SAMPLES`] points (0 for
/// fewer than two points; duplicates pull it toward 0, so no allowance).
fn spacing(tree: &PointTree, pts: &[Vec3]) -> f64 {
    let step = (pts.len() / SPACING_SAMPLES).max(1);
    let mut nn: Vec<f64> =
        (0..pts.len()).step_by(step).filter_map(|i| tree.nearest(pts, pts[i], Some(i as u32)).map(|(_, x)| x)).collect();
    if nn.is_empty() {
        return 0.0;
    }
    nn.sort_by(|a, b| a.total_cmp(b));
    nn[nn.len() / 2]
}

/// The most points a point-tree leaf holds.
const POINT_LEAF: usize = 8;

#[derive(Clone, Copy, Debug)]
struct PNode {
    bb: Aabb3,
    /// Leaf: first index into `order`; inner: the left child (the right child is `first + 1`).
    first: u32,
    /// Leaf: point count (> 0); inner: 0.
    count: u32,
}

/// A median-split tree over scan point ids (Bvh::build's shape for points: the longest axis of the node's box,
/// `select_nth_unstable_by`, an explicit stack). It holds ids, so a query takes the slice it was built from.
struct PointTree {
    nodes: Vec<PNode>,
    order: Vec<u32>,
}

impl PointTree {
    fn build(pts: &[Vec3]) -> PointTree {
        let mut t = PointTree { nodes: Vec::new(), order: (0..pts.len() as u32).collect() };
        let Some(&p0) = pts.first() else { return t };
        let at = |i: u32| pts[i as usize];
        t.nodes.push(PNode { bb: Aabb3 { min: p0, max: p0 }, first: 0, count: 0 });
        let mut stack: Vec<(usize, usize, usize)> = alloc::vec![(0, 0, pts.len())];
        while let Some((ni, start, end)) = stack.pop() {
            let ids = &mut t.order[start..end];
            let f = at(ids[0]);
            let bb = ids.iter().fold(Aabb3 { min: f, max: f }, |b, &i| b.union(Aabb3 { min: at(i), max: at(i) }));
            let count = end - start;
            if count <= POINT_LEAF {
                t.nodes[ni] = PNode { bb, first: start as u32, count: count as u32 };
                continue;
            }
            let ext = bb.max - bb.min;
            let axis = if ext.x >= ext.y && ext.x >= ext.z {
                0
            } else if ext.y >= ext.z {
                1
            } else {
                2
            };
            let key = |i: u32| {
                let p = at(i);
                match axis {
                    0 => p.x,
                    1 => p.y,
                    _ => p.z,
                }
            };
            let half = count / 2;
            ids.select_nth_unstable_by(half, |a, b| key(*a).total_cmp(&key(*b)));
            let left = t.nodes.len();
            t.nodes.push(PNode { bb, first: 0, count: 0 });
            t.nodes.push(PNode { bb, first: 0, count: 0 });
            t.nodes[ni] = PNode { bb, first: left as u32, count: 0 };
            stack.push((left, start, start + half));
            stack.push((left + 1, start + half, end));
        }
        t
    }

    /// The scan point nearest `q` other than id `skip`: (id, distance). Branch and bound on the squared point-box
    /// distance, the nearer child first; a non-finite distance is never taken.
    fn nearest(&self, pts: &[Vec3], q: Vec3, skip: Option<u32>) -> Option<(u32, f64)> {
        if self.nodes.is_empty() || !q.is_finite() {
            return None;
        }
        let mut best_sq = f64::INFINITY;
        let mut best: Option<u32> = None;
        let mut stack: Vec<usize> = alloc::vec![0];
        while let Some(ni) = stack.pop() {
            let Some(n) = self.nodes.get(ni) else { continue };
            if box_dist_sq(q, &n.bb) > best_sq {
                continue;
            }
            if n.count == 0 {
                let (l, r) = (n.first as usize, n.first as usize + 1);
                let dl = self.nodes.get(l).map_or(f64::INFINITY, |c| box_dist_sq(q, &c.bb));
                let dr = self.nodes.get(r).map_or(f64::INFINITY, |c| box_dist_sq(q, &c.bb));
                if dl <= dr {
                    stack.push(r);
                    stack.push(l);
                } else {
                    stack.push(l);
                    stack.push(r);
                }
                continue;
            }
            let s = n.first as usize;
            for &i in self.order.get(s..s + n.count as usize).unwrap_or(&[]) {
                if Some(i) == skip {
                    continue;
                }
                let Some(&p) = pts.get(i as usize) else { continue };
                let d2 = (p - q).mag_sq();
                if d2 < best_sq {
                    best_sq = d2;
                    best = Some(i);
                }
            }
        }
        best.map(|i| (i, libm::sqrt(best_sq)))
    }
}

/// The squared distance from `p` to the box (0 inside it); mesh_core's private `aabb_dist_sq`, which doc_core cannot
/// reach (the lift is owed: VUG-INTENT-POINTS-DENSE).
fn box_dist_sq(p: Vec3, b: &Aabb3) -> f64 {
    let d = |x: f64, lo: f64, hi: f64| if x < lo { lo - x } else if x > hi { x - hi } else { 0.0 };
    let (dx, dy, dz) = (d(p.x, b.min.x, b.max.x), d(p.y, b.min.y, b.max.y), d(p.z, b.min.z, b.max.z));
    dx * dx + dy * dy + dz * dz
}
