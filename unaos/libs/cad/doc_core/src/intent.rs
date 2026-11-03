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
//! doc_core takes no libm: the one square root is geom_core's `Vec2::mag` (the RMS as a hypot accumulation).

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

/// What the geometry is checked against. Scan shapes (SR497) are VUG-INTENT-SCAN's (owed).
#[derive(Clone, Debug, PartialEq)]
pub enum Measurement {
    /// A CLOSED polyline in the plane of the document's first solved sketch (a photo outline), mm.
    Outline { pts: Vec<Vec2>, sigma_mm: f64 },
    /// Points on the part's surface (a scan), mm, in the document's world frame.
    Points { pts: Vec<Vec3>, sigma_mm: f64 },
}

impl Measurement {
    pub fn sigma_mm(&self) -> f64 {
        match self {
            Measurement::Outline { sigma_mm, .. } | Measurement::Points { sigma_mm, .. } => *sigma_mm,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Measurement::Outline { pts, .. } => pts.len(),
            Measurement::Points { pts, .. } => pts.len(),
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
