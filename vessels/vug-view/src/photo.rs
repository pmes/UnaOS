// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D), a host face — the photo underlay and the proposal glyphs (VUGFACEPHOTO, LEDGER SR506);
//! holds no geometry (the outline is photo_core's, the verdicts doc_core::intent's, the sketch sketch_core's).
//!
//! "AI proposes, geometry disposes" (docs/dev/USERLAND/VUG.md §7, principle 11; BP-vug-7). A photo of a flat part on
//! graph paper opens through [`open`]: pixel_core decodes it, `photo_core::measure` gives the measured outline, its
//! sigma, the paper-mm -> pixel homography and the measured sketch; the sketch opens whole as the SR485 sketch face
//! (`SketchFace::over`, so dimension, erase and extrude work on it unchanged) on the XY plane, the paper is the drawn
//! model, and [`underlay`] draws the photo under the sketch (each pixel's ray hits the paper plane, the homography maps
//! the hit to the photo, bilinear, dimmed toward the background) with the outline's uncertainty as a faint band at
//! ±sigma.
//!
//! Proposals are doc_core statement lines. [`offer`] verifies each one with `doc_core::verify` against the face's
//! sketch written as a document ([`doc_of`]) and the measured outline, and the face draws each as a glyph at the entity
//! it names (the statement, the residual, the confidence; a refused one by its refusal's name). `Tab` cycles the focus
//! (the glyph under the cursor takes it too), `Enter` re-verifies the focused one against the sketch as it stands
//! now and, when accepted, enters it through the sketch face's one road (`SketchFace::constrain`, i.e. `apply_as`: a
//! refusal removes exactly what was added): `doc_core::apply` -> `doc_core::replay` (the ONE text -> sketch_core
//! translation) -> the proposal's replayed constraint renumbered onto the face's ids ([`remap`]). A refused proposal
//! is never applied. `Backspace` rejects the focused one.
//!
//! `i` asks Vein — over the BUS, never by linking vein_core here: the provider choice, the key custody and the
//! verified-TLS road are handlers/vein's. No bus verb carries a propose yet (VUG-VEIN-BUS, owed), so `i` says so in
//! the corner; offline, Echo gives no proposals and the road by hand still works.

use crate::model::Model;
use crate::raster::{self, Frame as Raster, rgba};
use crate::sketch::{ACTIVE, INK, REFUSED, SketchFace, world_planes};
use crate::view::View;
use doc_core::{Cons, Doc, Measurement, PRef, Proposal, Stmt, Verdict};
use sketch_core::{Constraint, ConstraintId, Entity, EntityId, PointRef, Tol, Vec2};
use std::fmt;

/// The graph paper's pitch a photo is measured on when nothing says otherwise (photo_core's default; an argument /
/// preference is VUG-FACE-PHOTO-PITCH, owed).
pub const PITCH_MM: f64 = 5.0;
/// The underlay's weight against the background: dimmed so the sketch reads over it.
pub const DIM: f32 = 0.55;
/// A proposal glyph within this many surface pixels of the cursor takes the focus.
pub const FOCUS_PX: f64 = 12.0;
/// The proposal colour (violet); the focused one in the sketch's accent, a refused one red.
pub const PROPOSAL: u32 = palette_core::ACCENTS[6];
/// The sigma band: `text.tertiary`, dark, faint.
pub const BAND: u32 = palette_core::TEXT_TERTIARY.1;
/// The corner note for `i` while no bus verb reaches Vein.
pub const OFFLINE: &str = "intent: no provider - offline (echo) gives no proposals";
/// The id [`doc_of`] gives the photo's sketch statement.
pub const SKETCH_ID: &str = "s";

/// Where an offered proposal stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Shown, waiting on the person (accepted or refused by its verdict).
    Open,
    /// Entered into the sketch.
    Applied,
    /// Rejected by the person (Backspace).
    Rejected,
}

/// One offered proposal and its verdict against the measurement.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub proposal: Proposal,
    pub verdict: Verdict,
    pub state: State,
}

/// A proposal glyph as drawn: where, what, in which colour, and which offer.
#[derive(Clone, Debug, PartialEq)]
pub struct ProposalGlyph {
    pub offer: usize,
    pub at: (f32, f32),
    pub text: String,
    pub colour: u32,
}

/// The opened photo: the image, the paper-mm -> pixel homography, the measured outline and its sigma, the offers.
#[derive(Clone)]
pub struct Photo {
    pub image: pixel_core::Image,
    /// Row-major: `[u, v, w] = h · [x_mm, y_mm, 1]`, the photo pixel at (u / w, v / w).
    pub h: [[f64; 3]; 3],
    /// The measured outline (closed), paper millimetres = the sketch plane's coordinates.
    pub outline: Vec<Vec2>,
    pub sigma_mm: f64,
    pub offers: Vec<Offer>,
    pub focus: Option<usize>,
    /// The corner note: the last thing the proposal road said (offline, applied, refused by name).
    pub note: Option<String>,
}

impl fmt::Debug for Photo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Photo")
            .field("image", &(self.image.width, self.image.height))
            .field("sigma_mm", &self.sigma_mm)
            .field("outline", &self.outline.len())
            .field("offers", &self.offers)
            .field("focus", &self.focus)
            .field("note", &self.note)
            .finish()
    }
}

impl Photo {
    /// The measurement every proposal is verified against.
    pub fn measurement(&self) -> Measurement {
        Measurement::Outline { pts: self.outline.clone(), sigma_mm: self.sigma_mm }
    }

    /// The photo's colour at paper point `q` (mm): the homography, then bilinear between pixel centres; `None` off
    /// the photo.
    pub fn sample(&self, q: [f64; 2]) -> Option<[f32; 3]> {
        let h = &self.h;
        let w = h[2][0] * q[0] + h[2][1] * q[1] + h[2][2];
        if !(w > 0.0) {
            return None;
        }
        let u = (h[0][0] * q[0] + h[0][1] * q[1] + h[0][2]) / w - 0.5;
        let v = (h[1][0] * q[0] + h[1][1] * q[1] + h[1][2]) / w - 0.5;
        let (iw, ih) = (self.image.width as f64, self.image.height as f64);
        if !(u >= -0.5 && v >= -0.5 && u <= iw - 0.5 && v <= ih - 0.5) {
            return None;
        }
        let (x0, y0) = (u.floor().max(0.0), v.floor().max(0.0));
        let (fx, fy) = ((u - x0).clamp(0.0, 1.0) as f32, (v - y0).clamp(0.0, 1.0) as f32);
        let (x0, y0) = (x0 as u32, y0 as u32);
        let (x1, y1) = ((x0 + 1).min(self.image.width - 1), (y0 + 1).min(self.image.height - 1));
        let px = |x: u32, y: u32| {
            let i = (y as usize * self.image.width as usize + x as usize) * 4;
            let p = &self.image.rgba[i..i + 3];
            [p[0] as f32, p[1] as f32, p[2] as f32]
        };
        let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
        Some([0, 1, 2].map(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bot = c[k] + (d[k] - c[k]) * fx;
            top + (bot - top) * fy
        }))
    }
}

/// Open a photo on `pitch_mm` graph paper: decode, measure, the paper as the drawn model, the measured sketch as a
/// face on the XY plane (paper millimetres are the plane's coordinates).
pub fn open(bytes: &[u8], pitch_mm: f64) -> Result<(Model, SketchFace, Photo), String> {
    let image = pixel_core::decode(bytes).map_err(|e| format!("image {e:?}"))?;
    let m = photo_core::measure(&image, photo_core::PhotoParams { pitch_mm }, Tol::DEFAULT).map_err(|e| format!("photo {e:?}"))?;
    let model = paper(&m.outline, pitch_mm)?;
    let photo = Photo { image, h: m.h, outline: m.outline, sigma_mm: m.sigma_mm, offers: Vec::new(), focus: None, note: None };
    let face = SketchFace::over(world_planes()[0].1, m.sketch);
    Ok((model, face, photo))
}

/// The paper as a quad at z = 0 facing +Z: the outline's bounds widened by max(4 pitches, a quarter of the larger side).
fn paper(outline: &[Vec2], pitch_mm: f64) -> Result<Model, String> {
    let first = outline.first().ok_or("photo: the measured outline is empty")?;
    let (mut lo, mut hi) = ((first.x, first.y), (first.x, first.y));
    for p in outline {
        lo = (lo.0.min(p.x), lo.1.min(p.y));
        hi = (hi.0.max(p.x), hi.1.max(p.y));
    }
    let m = (4.0 * pitch_mm).max(0.25 * (hi.0 - lo.0).max(hi.1 - lo.1));
    let (x0, y0, x1, y1) = (lo.0 - m, lo.1 - m, hi.0 + m, hi.1 + m);
    Model::from_triangles(&[[x0, y0, 0.0], [x1, y0, 0.0], [x1, y1, 0.0], [x0, y1, 0.0]], &[[0, 1, 2], [0, 2, 3]])
}

/// The underlay, drawn by `raster::render` after the grid and before the triad and the sketch: every pixel whose ray
/// meets the paper plane (z = 0) with nothing nearer takes the photo's colour there, dimmed toward the background;
/// then the sigma band. Resampled every frame (a frame is one settled burst of input: once per view change).
pub fn underlay(view: &View, fr: &mut Raster) {
    let Some(ph) = view.photo.as_ref() else { return };
    let (w, h) = (fr.width, fr.height);
    let bg = rgba(raster::BACKGROUND);
    // The ray direction is affine in the pixel (dir . forward = 1): three rays give every one exactly.
    let (o, d00) = view.ray_f64(0.5, 0.5);
    let (_, d10) = view.ray_f64(1.5, 0.5);
    let (_, d01) = view.ray_f64(0.5, 1.5);
    let dx = [0, 1, 2].map(|k| d10[k] - d00[k]);
    let dy = [0, 1, 2].map(|k| d01[k] - d00[k]);
    for y in 0..h {
        for x in 0..w {
            let d = [0, 1, 2].map(|k| d00[k] + dx[k] * x as f64 + dy[k] * y as f64);
            if d[2].abs() < 1e-12 {
                continue;
            }
            let t = -o[2] / d[2];
            if !(t > 0.0) {
                continue;
            }
            let i = (y * w + x) as usize;
            let iz = fr.inv_z[i] as f64;
            if iz > (1.0 / t) * (1.0 + 1e-4) {
                continue; // a body nearer than the paper
            }
            let Some(s) = ph.sample([o[0] + d[0] * t, o[1] + d[1] * t]) else { continue };
            let px = &mut fr.rgba[i * 4..i * 4 + 4];
            for k in 0..3 {
                px[k] = (DIM * s[k] + (1.0 - DIM) * bg[k] as f32).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    band(view, ph, fr);
}

/// The outline's uncertainty: the region between the outline offset by +sigma and by -sigma (vertex normals of the
/// closed polyline), tinted faintly through `raster::tint`'s even-odd rule.
fn band(view: &View, ph: &Photo, fr: &mut Raster) {
    let n = ph.outline.len();
    if n < 3 || !(ph.sigma_mm > 0.0) {
        return;
    }
    let ring = |s: f64| -> Option<Vec<(f32, f32)>> {
        (0..n)
            .map(|i| {
                let (a, p, b) = (ph.outline[(i + n - 1) % n], ph.outline[i], ph.outline[(i + 1) % n]);
                let (ux, uy) = (b.x - a.x, b.y - a.y);
                let l = ux.hypot(uy).max(1e-300);
                let q = [p.x + uy / l * s, p.y - ux / l * s, 0.0];
                view.project_f64(q).map(|(x, y)| (x as f32, y as f32))
            })
            .collect()
    };
    if let (Some(out), Some(inn)) = (ring(ph.sigma_mm), ring(-ph.sigma_mm)) {
        raster::tint(fr, &[out, inn], rgba(BAND), 0.25);
    }
}

// ---- The document road: the face's sketch as doc_core statements, and back ----

fn ename(e: EntityId) -> String {
    format!("e{}", e.0)
}

fn pref(p: PointRef) -> PRef {
    match p {
        PointRef::Point(e) => PRef::Point(ename(e)),
        PointRef::Centre(e) => PRef::Centre(ename(e)),
        PointRef::Start(e) => PRef::Start(ename(e)),
        PointRef::End(e) => PRef::End(ename(e)),
    }
}

/// A sketch_core constraint as doc_core's text form (ids `e<n>` by the face's entity id).
fn cons(c: &Constraint) -> Cons {
    match *c {
        Constraint::Coincident(a, b) => Cons::Coincident(pref(a), pref(b)),
        Constraint::PointOnLine(p, l) => Cons::PointOnLine(pref(p), ename(l)),
        Constraint::PointOnCircle(p, r) => Cons::PointOnCircle(pref(p), ename(r)),
        Constraint::Horizontal(l) => Cons::Horizontal(ename(l)),
        Constraint::Vertical(l) => Cons::Vertical(ename(l)),
        Constraint::Parallel(a, b) => Cons::Parallel(ename(a), ename(b)),
        Constraint::Perpendicular(a, b) => Cons::Perpendicular(ename(a), ename(b)),
        Constraint::TangentLineCircle(l, r) => Cons::TangentLineCircle(ename(l), ename(r)),
        Constraint::TangentCircles(a, b) => Cons::TangentCircles(ename(a), ename(b)),
        Constraint::EqualLength(a, b) => Cons::EqualLength(ename(a), ename(b)),
        Constraint::EqualRadius(a, b) => Cons::EqualRadius(ename(a), ename(b)),
        Constraint::Distance(a, b, d) => Cons::Distance(pref(a), pref(b), d),
        Constraint::DistancePointLine(p, l, d) => Cons::DistancePointLine(pref(p), ename(l), d),
        Constraint::Angle(a, b, t) => Cons::Angle(ename(a), ename(b), t),
        Constraint::Radius(r, v) => Cons::Radius(ename(r), v),
        Constraint::Diameter(r, v) => Cons::Diameter(ename(r), v),
        Constraint::Fix(p, at) => Cons::Fix(pref(p), [at.x, at.y]),
        Constraint::Midpoint(p, l) => Cons::Midpoint(pref(p), ename(l)),
        Constraint::Symmetric(a, b, l) => Cons::Symmetric(pref(a), pref(b), ename(l)),
    }
}

/// The face's sketch as a doc_core document — the inverse of `doc_core::replay`: the sketch statement `s` on the
/// face's plane, every live entity `e<n>` (n its face id) and every live constraint `k<n>`, in id order (so replay's
/// dense EntityIds are the live entities in order). The text a proposal names its references in. (Its home is
/// doc_core, beside tools/photo's writer: VUG-DOC-FROMSKETCH, owed.)
pub fn doc_of(face: &SketchFace) -> Doc {
    let sk = &face.sketch;
    let v3 = |p: geom_core::Vec3| [p.x, p.y, p.z];
    let s = SKETCH_ID.to_string();
    let mut doc = Doc::new(Tol::DEFAULT);
    doc.stmts.push(Stmt::Sketch { id: s.clone(), o: v3(face.plane.o), z: v3(face.plane.z), x: v3(face.plane.x) });
    let pr = sk.params();
    for (i, e) in sk.entities().iter().enumerate() {
        let id = EntityId(i as u32);
        let st = match *e {
            Entity::Point { x } => Stmt::Point { id: ename(id), sketch: s.clone(), at: [pr[x], pr[x + 1]] },
            Entity::Line { a, b } => Stmt::Line { id: ename(id), sketch: s.clone(), a: ename(a), b: ename(b) },
            Entity::Circle { centre, r } => Stmt::Circle { id: ename(id), sketch: s.clone(), centre: ename(centre), r: pr[r] },
            Entity::Arc { centre, r } => {
                Stmt::Arc { id: ename(id), sketch: s.clone(), centre: ename(centre), r: pr[r], start: pr[r + 1], end: pr[r + 2] }
            }
            Entity::Removed => continue,
        };
        doc.stmts.push(st);
    }
    for i in 0..face.issued() {
        if let Some(c) = sk.constraint(ConstraintId(i)) {
            doc.stmts.push(Stmt::Constraint { id: format!("k{i}"), sketch: s.clone(), c: cons(c) });
        }
    }
    doc
}

/// A constraint's entity ids renumbered by `f` (replay's dense ids onto the face's); `None` when one has no image.
pub fn remap(c: &Constraint, f: &dyn Fn(EntityId) -> Option<EntityId>) -> Option<Constraint> {
    let p = |r: PointRef| -> Option<PointRef> {
        Some(match r {
            PointRef::Point(e) => PointRef::Point(f(e)?),
            PointRef::Centre(e) => PointRef::Centre(f(e)?),
            PointRef::Start(e) => PointRef::Start(f(e)?),
            PointRef::End(e) => PointRef::End(f(e)?),
        })
    };
    Some(match *c {
        Constraint::Coincident(a, b) => Constraint::Coincident(p(a)?, p(b)?),
        Constraint::PointOnLine(q, l) => Constraint::PointOnLine(p(q)?, f(l)?),
        Constraint::PointOnCircle(q, r) => Constraint::PointOnCircle(p(q)?, f(r)?),
        Constraint::Horizontal(l) => Constraint::Horizontal(f(l)?),
        Constraint::Vertical(l) => Constraint::Vertical(f(l)?),
        Constraint::Parallel(a, b) => Constraint::Parallel(f(a)?, f(b)?),
        Constraint::Perpendicular(a, b) => Constraint::Perpendicular(f(a)?, f(b)?),
        Constraint::TangentLineCircle(l, r) => Constraint::TangentLineCircle(f(l)?, f(r)?),
        Constraint::TangentCircles(a, b) => Constraint::TangentCircles(f(a)?, f(b)?),
        Constraint::EqualLength(a, b) => Constraint::EqualLength(f(a)?, f(b)?),
        Constraint::EqualRadius(a, b) => Constraint::EqualRadius(f(a)?, f(b)?),
        Constraint::Distance(a, b, d) => Constraint::Distance(p(a)?, p(b)?, d),
        Constraint::DistancePointLine(q, l, d) => Constraint::DistancePointLine(p(q)?, f(l)?, d),
        Constraint::Angle(a, b, t) => Constraint::Angle(f(a)?, f(b)?, t),
        Constraint::Radius(r, v) => Constraint::Radius(f(r)?, v),
        Constraint::Diameter(r, v) => Constraint::Diameter(f(r)?, v),
        Constraint::Fix(q, at) => Constraint::Fix(p(q)?, at),
        Constraint::Midpoint(q, l) => Constraint::Midpoint(p(q)?, f(l)?),
        Constraint::Symmetric(a, b, l) => Constraint::Symmetric(p(a)?, p(b)?, f(l)?),
    })
}

/// Enter an accepted proposal into the face: `doc_core::apply` -> `doc_core::replay` -> the proposal's constraint
/// (by its ordinal among the constraint statements) renumbered onto the face -> `SketchFace::constrain`. A statement
/// that replaces one of the face's `k<n>` removes constraint n first and re-adds it when the new one is refused.
fn enter(face: &mut SketchFace, doc: &Doc, p: &Proposal) -> Result<(), String> {
    let cand = doc_core::apply(doc, p).map_err(|r| r.text())?;
    let idx = if cand.stmts.len() > doc.stmts.len() {
        cand.stmts.len() - 1
    } else {
        (0..doc.stmts.len()).find(|&j| cand.stmts[j] != doc.stmts[j]).ok_or("no change")?
    };
    let Stmt::Constraint { id, .. } = &cand.stmts[idx] else {
        return Err("not a constraint: only a constraint enters the sketch face".into());
    };
    let rep = doc_core::replay(&cand, doc.tol);
    if let Some(f) = rep.failures.first() {
        return Err(format!("replay {:?} {}", f.fault, f.id));
    }
    let sk = &rep.sketches.iter().find(|s| s.id == SKETCH_ID).ok_or("no sketch")?.sketch;
    let ordinal = cand.stmts[..idx].iter().filter(|s| matches!(s, Stmt::Constraint { .. })).count();
    let fresh = sk.constraint(ConstraintId(ordinal as u32)).copied().ok_or("replay renumbered the constraints")?;
    let live: Vec<EntityId> = face.sketch.entities().iter().enumerate().filter(|(_, e)| !matches!(e, Entity::Removed)).map(|(i, _)| EntityId(i as u32)).collect();
    let c = remap(&fresh, &|e: EntityId| live.get(e.0 as usize).copied()).ok_or("a reference outside the sketch")?;
    // A replacing statement (`k<n>`, the face's own constraint n): out first, back on refusal.
    let old = if idx < doc.stmts.len() { id.strip_prefix('k').and_then(|n| n.parse::<u32>().ok()).map(ConstraintId) } else { None };
    let old_c = old.and_then(|o| face.sketch.constraint(o).copied());
    if let Some(o) = old {
        face.sketch.remove(o);
    }
    match face.constrain(vec![c]) {
        Ok(_) => Ok(()),
        Err(e) => {
            if let Some(oc) = old_c {
                let _ = face.constrain(vec![oc]);
            }
            Err(e)
        }
    }
}

// ---- The proposals: offered, focused, applied, rejected, drawn ----

/// Offer proposals (the bus reply's door; a test's by hand): each verified against the face's sketch and the
/// measured outline; the first open one focused. True when anything was offered.
pub fn offer(view: &mut View, ps: Vec<Proposal>) -> bool {
    let (Some(face), Some(ph)) = (view.design.face.as_ref(), view.photo.as_mut()) else { return false };
    let doc = doc_of(face);
    let m = ph.measurement();
    let n = ps.len();
    for p in ps {
        let verdict = doc_core::verify(&doc, &p, &m, Tol::DEFAULT);
        ph.offers.push(Offer { proposal: p, verdict, state: State::Open });
    }
    let refused = ph.offers.iter().filter(|o| o.state == State::Open && matches!(o.verdict, Verdict::Refuse { .. })).count();
    let open = ph.offers.iter().filter(|o| o.state == State::Open).count();
    ph.note = Some(format!("intent: proposed={n} acceptable={} refused={refused}", open - refused));
    if ph.focus.is_none_or(|i| ph.offers.get(i).is_none_or(|o| o.state != State::Open)) {
        ph.focus = ph.offers.iter().position(|o| o.state == State::Open);
    }
    n > 0
}

fn is_enter(k: &str) -> bool {
    matches!(k, "Return" | "KP_Enter" | "\r" | "\n")
}

fn is_back(k: &str) -> bool {
    matches!(k, "BackSpace" | "Backspace" | "\u{8}" | "\u{7f}")
}

/// The view's key, first: `i`, `Tab`, and Enter / Backspace on the focused proposal. `None` (the sketch face's keys)
/// with no photo, while a pull or a dimension edit is open, and for Enter / Backspace with nothing focused.
pub fn key(view: &mut View, k: &str) -> Option<bool> {
    let ph = view.photo.as_ref()?;
    if view.design.pull.is_some() || view.design.face.as_ref().is_some_and(|f| f.edit.is_some()) {
        return None;
    }
    let focused = ph.focus.filter(|&i| ph.offers.get(i).is_some_and(|o| o.state == State::Open));
    match k {
        "i" | "I" => {
            // VUG-VEIN-BUS (owed): the propose verb to handlers/vein goes here; nothing reaches a provider today.
            view.photo.as_mut()?.note = Some(OFFLINE.to_string());
            Some(true)
        }
        "Tab" | "\t" => {
            let n = ph.offers.len();
            let start = ph.focus.map(|i| i + 1).unwrap_or(0);
            let next = (0..n).map(|j| (start + j) % n.max(1)).find(|&j| ph.offers[j].state == State::Open);
            let ph = view.photo.as_mut()?;
            let changed = ph.focus != next;
            ph.focus = next;
            Some(changed)
        }
        _ if is_enter(k) => focused.map(|i| accept(view, i)),
        _ if is_back(k) => {
            let i = focused?;
            let ph = view.photo.as_mut()?;
            ph.offers[i].state = State::Rejected;
            ph.note = Some(format!("rejected {}", ph.offers[i].proposal.stmt.trim()));
            ph.focus = ph.offers.iter().position(|o| o.state == State::Open);
            Some(true)
        }
        _ => None,
    }
}

/// Enter on offer `i`: verified again against the sketch as it stands (an earlier accept may have changed it); a
/// refusal is named in the corner and nothing is applied; an accepted one enters through `enter`.
fn accept(view: &mut View, i: usize) -> bool {
    let (Some(face), Some(ph)) = (view.design.face.as_mut(), view.photo.as_mut()) else { return false };
    let p = ph.offers[i].proposal.clone();
    let doc = doc_of(face);
    let verdict = doc_core::verify(&doc, &p, &ph.measurement(), Tol::DEFAULT);
    ph.offers[i].verdict = verdict.clone();
    if let Verdict::Refuse { why } = &verdict {
        ph.note = Some(format!("refused {}", why.text()));
        return true;
    }
    match enter(face, &doc, &p) {
        Ok(()) => {
            ph.offers[i].state = State::Applied;
            ph.note = Some(format!("applied {}", p.stmt.trim()));
            ph.focus = ph.offers.iter().position(|o| o.state == State::Open);
        }
        Err(e) => ph.note = Some(format!("refused by the sketch {e}")),
    }
    true
}

/// The sketch point a statement is anchored at: the first `e<n>` it names (a point itself, a line's midpoint, a
/// circle's / arc's centre).
fn anchor(face: &SketchFace, stmt: &str) -> Option<Vec2> {
    for tok in stmt.split_whitespace().skip(2) {
        let Some((k, v)) = tok.split_once('=') else { continue };
        if k == "sketch" {
            continue;
        }
        let name = v.split('.').next().unwrap_or("");
        let Some(n) = name.strip_prefix('e').and_then(|n| n.parse::<u32>().ok()) else { continue };
        let id = EntityId(n);
        let sk = &face.sketch;
        let q = match sk.entity(id)? {
            Entity::Point { .. } => sk.point(PointRef::Point(id)),
            Entity::Line { a, b } => match (sk.point(PointRef::Point(*a)), sk.point(PointRef::Point(*b))) {
                (Some(p), Some(q)) => Some(Vec2::new(0.5 * (p.x + q.x), 0.5 * (p.y + q.y))),
                _ => None,
            },
            Entity::Circle { .. } | Entity::Arc { .. } => sk.point(PointRef::Centre(id)),
            Entity::Removed => None,
        };
        if q.is_some() {
            return q;
        }
    }
    None
}

fn num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0');
    s.trim_end_matches('.').to_string()
}

/// The glyph of every open offer: at its anchor (else listed down the left under the corner note), the statement and
/// the residual and confidence (accepted) or the refusal's name (refused), upper-cased for the bitmap font.
pub fn glyphs(view: &View) -> Vec<ProposalGlyph> {
    let (Some(face), Some(ph)) = (view.design.face.as_ref(), view.photo.as_ref()) else { return Vec::new() };
    let mut out = Vec::new();
    let mut row = 0;
    for (i, o) in ph.offers.iter().enumerate() {
        if o.state != State::Open {
            continue;
        }
        let stmt = o.proposal.stmt.trim();
        let (text, refused) = match &o.verdict {
            Verdict::Accept { residual_mm, .. } => (format!("{stmt} R={} C={}", num(*residual_mm), num(o.proposal.confidence)), false),
            Verdict::Refuse { why } => (format!("{stmt} REFUSED {}", why.text()), true),
        };
        let at = match anchor(face, stmt).and_then(|q| face.to_screen(view, q)) {
            Some((x, y)) => (x as f32 + 6.0, y as f32 - 14.0),
            None => {
                row += 1;
                (6.0, 30.0 + 12.0 * row as f32)
            }
        };
        let colour = if ph.focus == Some(i) {
            ACTIVE
        } else if refused {
            REFUSED
        } else {
            PROPOSAL
        };
        out.push(ProposalGlyph { offer: i, at, text: text.to_uppercase(), colour });
    }
    out
}

/// The focus follows the glyph under the cursor (within `FOCUS_PX` of its first character); true when it moved.
pub fn hover(view: &mut View, x: f64, y: f64) -> bool {
    let near = glyphs(view)
        .into_iter()
        .map(|g| (g.offer, (g.at.0 as f64 + 3.0 - x).hypot(g.at.1 as f64 + 5.0 - y)))
        .filter(|(_, d)| *d <= FOCUS_PX)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(core::cmp::Ordering::Equal));
    let Some((i, _)) = near else { return false };
    let Some(ph) = view.photo.as_mut() else { return false };
    let moved = ph.focus != Some(i);
    ph.focus = Some(i);
    moved
}

/// The proposal overlay, drawn after the sketch's (`View::overlay`): each glyph (a cross at its anchor's corner and
/// its text) and the corner note under the dof.
pub fn overlay(view: &View, fr: &mut Raster) {
    let Some(ph) = view.photo.as_ref() else { return };
    for g in glyphs(view) {
        let col = rgba(g.colour);
        let (x, y) = (g.at.0 - 6.0, g.at.1 + 14.0);
        raster::line(fr, (x - 3.0, y - 3.0), (x + 3.0, y + 3.0), col);
        raster::line(fr, (x - 3.0, y + 3.0), (x + 3.0, y - 3.0), col);
        raster::text(fr, &g.text, g.at.0, g.at.1, col);
    }
    if let Some(note) = &ph.note {
        let col = if note.starts_with("refused") { REFUSED } else { INK };
        raster::text(fr, &note.to_uppercase(), 6.0, 18.0, rgba(col));
    }
}
