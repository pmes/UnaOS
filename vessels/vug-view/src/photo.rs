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
//! verified-TLS road are handlers/vein's. VUGFACEINTENTSEND (SR514): `i` queues one `IntentEvent::Propose` (the face's
//! document, the measurement table, the photo as `image/png`, a fresh request id) on [`Photo::outbox`], which the
//! controller drains onto the bus (`Viewer::outbox`); [`answer`] takes Vein's `Proposals` for the live id through
//! [`offer`] and names a `Refused` in the corner; an answer to any other id is ignored and counted. Echo gives no
//! proposals and the road by hand still works.
//!
//! VUGFACESTREAM (SR535): Vein streams each proposal as it parses (`IntentEvent::Proposal`, SR531) before its one
//! terminal answer. A streamed proposal for the live id enters as a PREVIEW (`Offer::streaming`): verified through the
//! same road as [`offer`] and drawn with its residual and a `STREAMING` mark; Tab focuses it, Enter on it is refused by
//! name (`wait: the answer is not complete`). The terminal `Proposals` settles the previews by index (an equal item keeps
//! its verdict, a differing one is verified again, an extra preview is withdrawn); `Refused` withdraws every preview and
//! names why with the count that arrived; a new ask withdraws the stale request's previews.

use crate::extrude::Pull; // VUGFACEPHOTOSIDE (SR547): the committed pull the journal records
use crate::model::Model;
use crate::raster::{self, Frame as Raster, rgba};
use crate::sketch::{ACTIVE, INK, REFUSED, SketchFace, world_planes};
use crate::view::View;
use bandy::{IntentEvent, IntentImage, IntentProposal, Origin};
use doc_core::{Doc, Measurement, PlainNames, Proposal, SketchNames, Stmt, Term, Verdict}; // VUGFACEPHOTOSIDE (SR547): PlainNames, SketchNames; VUGINTENTTERMTYPE (SR577): Term
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
    /// VUGFACESTREAM (SR535): the stream index of a preview of the live request (Enter waits for the terminal answer);
    /// `None` for a settled or hand-offered proposal.
    pub streaming: Option<u32>,
    /// VUGINTENTSETFACE (SR570): the statement re-mapped into the journal's names when it was offered ([`mark`]): the
    /// stable identity a panel delete does not move; `proposal.stmt` byte for byte when no name moved.
    pub named: String,
    /// VUGINTENTSETFACE (SR570): a member of one doc_core statement set, painted as ONE card ([`Member`]).
    pub set: Option<Member>,
    /// VUGINTENTSETFACESCROLL (SR583): the first member line shown of the card this offer heads (0 = the top; PageDown /
    /// PageUp move it, [`page_card`]); only a card's head offer's is read.
    pub page: usize,
}

/// A proposal glyph as drawn: where, what, in which colour, and which offer.
#[derive(Clone, Debug, PartialEq)]
pub struct ProposalGlyph {
    pub offer: usize,
    pub at: (f32, f32),
    pub text: String,
    pub colour: u32,
    /// VUGINTENTSETFACE (SR570): a statement set's one card (its member lines drawn under `text`); None for a single.
    pub card: Option<Card>,
}

/// The opened photo: the image, the paper-mm -> pixel homography, the measured outline and its sigma, the offers.
#[derive(Clone)]
pub struct Photo {
    pub image: pixel_core::Image,
    /// Row-major: `[u, v, w] = h · [x_mm, y_mm, 1]`, the photo pixel at (u / w, v / w).
    pub h: [[f64; 3]; 3],
    /// The measured outline (closed), paper millimetres = the sketch plane's coordinates.
    pub outline: Vec<Vec2>,
    /// The measured loops (the outer first, then every hole), the SR507 measurement the proposals are checked against.
    pub loops: Vec<Vec<Vec2>>,
    pub sigma_mm: f64,
    /// The band the measurement is CHECKED at (BP-vug-7): the photo's outline acceptance (`PhotoParams.band.lin`, the band
    /// photo_core accepted its own fits at) or `sigma_mm`, whichever is larger. The pixel sigma alone is below the band
    /// the loops were simplified and fitted at, so a proposal checked against it would refuse the measured part itself.
    pub band_mm: f64,
    pub offers: Vec<Offer>,
    pub focus: Option<usize>,
    /// The corner note: the last thing the proposal road said (asked, applied, refused by name).
    pub note: Option<String>,
    /// VUGFACEINTENTSEND (SR514): the asks not yet fired (the controller drains them onto the bus, `Viewer::outbox`).
    pub outbox: Vec<IntentEvent>,
    /// VUGFACEINTENTSEND (SR514): the request counter (0 = never asked; each `i` takes the next).
    pub request: u64,
    /// VUGFACEINTENTSEND (SR514): the LIVE request id, cleared when its answer lands.
    pub asked: Option<u64>,
    /// VUGFACEINTENTSEND (SR514): answers to any other id (stale, unknown, already answered), ignored and counted.
    pub ignored: u32,
    /// VUGFACEPHOTOSIDE (SR547): the flat photo's measurement `photo_core::measure_side` pairs a side photo against;
    /// None on a hand-built face.
    pub first: Option<photo_core::Measured>,
    /// VUGINTENTTHICKNESS (SR557): the side photo's thickness as a doc_core `Measurement::Thickness`, set by [`side`];
    /// an extrude of the sketch is judged by it beside the outline ([`judged`]). None until a side photo is measured.
    pub thickness: Option<Measurement>,
    /// VUGFACEPHOTOCAMERAPREF (SR576): the camera's datum from Principia's store (`vug.photo.*`, read once at start by
    /// main.rs); the default (none set) is the lattice's own camera solve about the photo's centre.
    pub prefs: PhotoPrefs,
    /// VUGFACEPHOTOCROPSAVE (SR640): the face's crop road (`:crop`, the drag, Return saves beside the photo's file).
    pub crop: Crop,
}

impl fmt::Debug for Photo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Photo")
            .field("image", &(self.image.width, self.image.height))
            .field("sigma_mm", &self.sigma_mm)
            .field("band_mm", &self.band_mm)
            .field("outline", &self.outline.len())
            .field("offers", &self.offers)
            .field("focus", &self.focus)
            .field("note", &self.note)
            .field("request", &self.request)
            .field("asked", &self.asked)
            .field("ignored", &self.ignored)
            .finish()
    }
}

impl Photo {
    /// The measurement every proposal is verified against.
    pub fn measurement(&self) -> Measurement {
        Measurement::Outline { loops: self.loops.clone(), sigma_mm: self.band_mm }
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
/// face on the XY plane (paper millimetres are the plane's coordinates). No camera prefs ([`open_with`]'s default).
pub fn open(bytes: &[u8], pitch_mm: f64) -> Result<(Model, SketchFace, Photo), String> {
    open_with(bytes, pitch_mm, PhotoPrefs::default())
}

/// VUGFACEPHOTOCAMERAPREF (SR576): [`open`] under the camera's prefs: the lens term about `prefs.principal_px`, and the
/// prefs held on the [`Photo`] for the side photo ([`side`]).
pub fn open_with(bytes: &[u8], pitch_mm: f64, prefs: PhotoPrefs) -> Result<(Model, SketchFace, Photo), String> {
    let image = pixel_core::decode(bytes).map_err(|e| format!("image {e:?}"))?;
    let params = prefs.params(pitch_mm); // VUGFACEPHOTOCAMERAPREF (SR576)
    let m = photo_core::measure(&image, params, Tol::DEFAULT).map_err(|e| format!("photo {e:?}"))?;
    let model = paper(&m.outline, pitch_mm)?;
    let first = Some(m.clone()); // VUGFACEPHOTOSIDE (SR547): held for the side photo
    let photo = Photo { image, h: m.h, outline: m.outline, loops: m.loops, sigma_mm: m.sigma_mm, band_mm: params.band.lin.max(m.sigma_mm), offers: Vec::new(), focus: None, note: None, outbox: Vec::new(), request: 0, asked: None, ignored: 0, first, thickness: None, prefs, crop: Crop::default() }; // VUGINTENTTHICKNESS (SR557): thickness; VUGFACEPHOTOCAMERAPREF (SR576): prefs; VUGFACEPHOTOCROPSAVE (SR640): crop
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
    band(view, ph, fr); crop_mask(view, ph, fr); // VUGFACEPHOTOCROPSAVE (SR640): the sheet outside an armed crop dims
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

/// The face's sketch as a doc_core document (sketch `s`, `e<n>` / `k<n>` by face id): the text a proposal names
/// its references in. VUGFACETREE (SR513) composed: the one writer is `journal::sketch_doc` (the journal's own `Open`
/// naming), this is its name on the photo road.
pub fn doc_of(face: &SketchFace) -> Doc {
    crate::journal::sketch_doc(face)
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
    let t = ph.thickness.clone(); // VUGINTENTTHICKNESS (SR557)
    let n = ps.len();
    for p in ps {
        ph.offers.push(verified(&doc, &m, t.as_ref(), p, None)); // VUGFACESTREAM (SR535): the one verify road
    }
    mark(ph, &view.design.journal, face); tally(ph, n); // VUGFACESTREAM (SR535): the one corner and focus rule; VUGINTENTSETFACE (SR570): the sets marked first
    n > 0
}

fn is_enter(k: &str) -> bool {
    crate::sketch::FieldKey::of(k) == Some(crate::sketch::FieldKey::Enter) // VUGFACEFIELDKEYSREST (SR605): the one key-name table
}

fn is_back(k: &str) -> bool {
    crate::sketch::FieldKey::of(k) == Some(crate::sketch::FieldKey::Backspace) // VUGFACEFIELDKEYSREST (SR605)
}

/// The view's key, first: `i`, `Tab`, and Enter / Backspace on the focused proposal. `None` (the sketch face's keys)
/// with no photo, while a pull or a dimension edit is open, and for Enter / Backspace with nothing focused.
pub fn key(view: &mut View, k: &str) -> Option<bool> {
    if let Some(r) = crop_key(view, k) { return Some(r); } let ph = view.photo.as_ref()?; // VUGFACEPHOTOCROPSAVE (SR640): an armed crop takes Return / Escape first
    if view.design.pull.is_some() || view.design.face.as_ref().is_some_and(|f| f.edit.is_some()) {
        return None;
    }
    let focused = ph.focus.filter(|&i| ph.offers.get(i).is_some_and(|o| o.state == State::Open));
    match k {
        "i" | "I" => Some(ask(view)), // VUGFACEINTENTSEND (SR514): the Propose onto the bus
        "Tab" | "\t" if !crate::journal::choosing(&view.design) => { // VUGFACEJOURNALFAULTFIXMANY (SR584): an offered fix card takes Tab
            let n = ph.offers.len();
            let start = ph.focus.map(|i| i + 1).unwrap_or(0);
            let next = (0..n).map(|j| (start + j) % n.max(1)).find(|&j| ph.offers[j].state == State::Open && !tail_member(ph, j)); // VUGINTENTSETFACE (SR570): a card is one stop
            let ph = view.photo.as_mut()?;
            let changed = ph.focus != next;
            ph.focus = next;
            Some(changed)
        }
        "r" | "R" if line_free(view) => Some(crop_arm(view)), // VUGFACEPHOTOCROPKEYPREF (SR645): the crop key, the arm `:crop` takes (line_free: the `:` line's gate)
        _ if is_enter(k) => focused.map(|i| accept(view, i)),
        _ if is_back(k) => {
            let i = focused?;
            if let Some(r) = reject_card(view, i) { return Some(r); } // VUGINTENTSETFACE (SR570): a card is rejected whole
            let ph = view.photo.as_mut()?;
            ph.offers[i].state = State::Rejected;
            ph.note = Some(format!("rejected {}", ph.offers[i].proposal.stmt.trim()));
            ph.focus = ph.offers.iter().position(|o| o.state == State::Open);
            Some(true)
        }
        _ if crate::sketch::FieldKey::of(k) == Some(crate::sketch::FieldKey::Escape) => focused.and_then(|i| reject_card(view, i)), "Page_Down" | "PageDown" | "Next" | "Page_Up" | "PageUp" | "Prior" => focused.and_then(|i| page_card(view, i, k)), // VUGINTENTSETFACESCROLL (SR583): PageDown / PageUp page a card taller than the sheet; VUGINTENTSETFACE (SR570): Escape refuses the whole card; off a card the sketch's Escape
        _ => None,
    }
}

/// Enter on offer `i`: verified again against the sketch as it stands (an earlier accept may have changed it); a
/// refusal is named in the corner and nothing is applied; an accepted one enters through `enter`.
fn accept(view: &mut View, i: usize) -> bool {
    if let Some(done) = set_road(view, i).or_else(|| single_road(view, i)) { return done; } // VUGINTENTUNDOSET (SR563): a statement set enters whole, one journal step; VUGINTENTSINGLERENAMED (SR577): a lone proposal in the journal's names
    let (Some(face), Some(ph)) = (view.design.face.as_mut(), view.photo.as_mut()) else { return false };
    if ph.offers[i].streaming.is_some() {
        // VUGFACESTREAM (SR535): a preview enters only once the terminal answer has settled it.
        ph.note = Some("wait: the answer is not complete".to_string());
        return true;
    }
    let p = ph.offers[i].proposal.clone();
    let doc = doc_of(face);
    let verdict = judged(&doc, &p, &ph.measurement(), ph.thickness.as_ref()); // VUGINTENTTHICKNESS (SR557): outline and thickness
    ph.offers[i].verdict = verdict.clone();
    if let Verdict::Refuse { why } = &verdict {
        ph.note = Some(format!("refused {}", why.text()));
        return true;
    }
    if let Some(x) = extruded(&doc, &p) { return enter_extrude(view, i, x); } // VUGFACEPHOTOSIDE (SR547): an extrude enters by the journal's pull road
    let applied = match enter(face, &doc, &p) {
        Ok(()) => {
            ph.offers[i].state = State::Applied;
            ph.note = Some(format!("applied {}", p.stmt.trim()));
            ph.focus = ph.offers.iter().position(|o| o.state == State::Open);
            true
        }
        Err(e) => {
            ph.note = Some(format!("refused by the sketch {e}"));
            false
        }
    };
    if applied {
        // VUGFACETREE (SR513) composed: an accepted proposal is a committed face action (one undo step).
        view.design.journal.record(view.design.face.as_ref(), None);
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
        if o.state != State::Open || tail_member(ph, i) {
            continue; // VUGINTENTSETFACE (SR570): a set's later members are lines of its one card
        }
        let stmt = o.proposal.stmt.trim();
        let (mut text, refused) = match &o.verdict {
            Verdict::Accept { residual_mm, .. } => (format!("{stmt} R={} C={}", num(*residual_mm), num(o.proposal.confidence)), false),
            Verdict::Refuse { why } => (format!("{stmt} REFUSED {}", why.text()), true),
        };
        if o.streaming.is_some() {
            text.push_str(" STREAMING"); // VUGFACESTREAM (SR535): the preview's mark
        }
        let card = card_of(ph, i); if let Some(c) = &card { text = card_head(ph, i, c); } // VUGINTENTSETFACE (SR570): the set's one card
        let at = match anchor(face, stmt).and_then(|q| face.to_screen(view, q)) {
            Some((x, y)) => (x as f32 + 6.0, y as f32 - 14.0),
            None => {
                row += 1;
                let y = 30.0 + 12.0 * row as f32;
                row += paged(card.clone(), y, view.size.1).map_or(0, |c| c.rows().len()); // VUGINTENTSETFACELISTROWS (SR589): the PAGED card's rows (fit + the more line); VUGINTENTSETFACE (SR570): the card's lines take their rows
                (6.0, y)
            }
        };
        let colour = if ph.focus == Some(i) {
            ACTIVE
        } else if refused {
            REFUSED
        } else {
            PROPOSAL
        };
        let card = paged(card, at.1, view.size.1); out.push(ProposalGlyph { offer: i, at, text: text.to_uppercase(), colour, card }); // VUGINTENTSETFACESCROLL (SR583): the card's page on the sheet; VUGINTENTSETFACE (SR570): card
    }
    unstack(view, ph, face, out) // VUGINTENTSETFACEANCHOREDOVERLAP (SR596): an anchored glyph overlapping an earlier anchored one moves down to its foot
}

/// The focus follows the glyph under the cursor (within `FOCUS_PX` of its first character); true when it moved, or
/// (VUGINTENTSETFACEANCHOREDOVERLAP SR596) when the more line under the cursor changed from `was` (the cursor before
/// this motion): entering or leaving one asks for the frame its cue paints in.
pub fn hover(view: &mut View, was: (f64, f64), x: f64, y: f64) -> bool {
    if crop_motion(view, x, y) { return true; } let cue = more_at(view, was) != more_at(view, (x, y)); // VUGFACEPHOTOCROPSAVE (SR640): a crop drag follows the cursor; VUGINTENTSETFACEANCHOREDOVERLAP (SR596): the more line's cue
    let near = glyphs(view)
        .into_iter()
        .map(|g| (g.offer, (g.at.0 as f64 + 3.0 - x).hypot(g.at.1 as f64 + 5.0 - y)))
        .filter(|(_, d)| *d <= FOCUS_PX)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(core::cmp::Ordering::Equal));
    let Some((i, _)) = near else { return cue };
    let Some(ph) = view.photo.as_mut() else { return cue };
    let moved = ph.focus != Some(i);
    ph.focus = Some(i);
    moved || cue
}

/// The proposal overlay, drawn after the sketch's (`View::overlay`): each glyph (a cross at its anchor's corner and
/// its text) and the corner note under the dof.
pub fn overlay(view: &View, fr: &mut Raster) {
    let Some(ph) = view.photo.as_ref() else { return };
    for g in glyphs(view) {
        let col = rgba(g.colour); paint_leader(view, fr, &g); // VUGINTENTSETFACELISTEDOVERLAP (SR601): a pushed glyph's leader, INK, under its cross and text
        let (x, y) = (g.at.0 - 6.0, g.at.1 + 14.0);
        raster::line(fr, (x - 3.0, y - 3.0), (x + 3.0, y + 3.0), col);
        raster::line(fr, (x - 3.0, y + 3.0), (x + 3.0, y - 3.0), col);
        raster::text(fr, &g.text, g.at.0, g.at.1, col); card_lines(fr, &g, col); more_cue(view, fr, &g); // VUGINTENTSETFACEANCHOREDOVERLAP (SR596): the more line INK, ACTIVE under the cursor; VUGINTENTSETFACE (SR570): the card's member lines under its head
    }
    if let Some(note) = &ph.note {
        let col = if note.starts_with("refused") || note.starts_with("Vein refused") { REFUSED } else { INK };
        raster::text(fr, &note.to_uppercase(), 6.0, 18.0, rgba(col));
    }
    if let Some(w) = wide_note(view) { raster::text(fr, &w.to_uppercase(), 6.0, 30.0, rgba(INK)); } // VUGINTENTSETFACEPUSHEDGE (SR606): the corner's second row names a glyph wider than the sheet
}

// ---- VUGFACEINTENTSEND (SR514): the ask over the bus and its answer (named tail block) ----

/// Who asks: the person at the face (the facet-view / una vessels' shape).
fn principal() -> Origin {
    Origin::LocalUser(std::env::var("USER").unwrap_or_else(|_| "una".into()))
}

/// `i`: one `IntentEvent::Propose` queued on the photo's outbox — the face's sketch as its document (`doc_of`), the
/// measurement table (`doc_core::measurements_text`), the decoded photo re-encoded as `image/png` (pixel_core's one
/// encoder) and the next request id, which becomes the live one; the corner says so. A PNG encode error is named and
/// nothing is sent. True when the corner changed.
fn ask(view: &mut View) -> bool {
    let (Some(face), Some(ph)) = (view.design.face.as_ref(), view.photo.as_mut()) else { return false };
    let png = match pixel_core::png::encode::encode_rgba(ph.image.width, ph.image.height, &ph.image.rgba) {
        Ok(b) => b,
        Err(e) => {
            ph.note = Some(format!("intent: png {e:?}"));
            return true;
        }
    };
    withdraw(ph); // VUGFACESTREAM (SR535): the stale request's previews go with it
    ph.request += 1;
    let request = ph.request;
    ph.outbox.push(IntentEvent::Propose {
        principal: principal(),
        context: doc_core::write(&doc_of(face)),
        measurements: doc_core::measurements_text(&ph.measurement()),
        images: vec![IntentImage { media_type: "image/png".to_string(), bytes: png }],
        request,
    });
    ph.asked = Some(request);
    ph.note = Some(format!("asked Vein (request {request})"));
    true
}

/// Vein's answer off the bus (`Viewer::input`): `Proposals` for the live id enter through [`offer`] (verified, drawn);
/// `Refused` for the live id is named in the corner; an answer to any other id is ignored and counted; our own `Propose`
/// (the bus is shared) is not an answer. True when the frame changed.
pub fn answer(view: &mut View, ev: &IntentEvent) -> bool {
    let Some(ph) = view.photo.as_mut() else { return false };
    let request = match ev {
        IntentEvent::Propose { .. } => return false,
        IntentEvent::Proposals { request, .. } | IntentEvent::Refused { request, .. } | IntentEvent::Proposal { request, .. } => *request,
    };
    if ph.asked != Some(request) {
        ph.ignored += 1;
        return false;
    }
    if let IntentEvent::Proposal { index, proposal, .. } = ev {
        return preview(view, request, *index, from_bus(proposal)); // VUGFACESTREAM (SR535): the terminal still follows
    }
    ph.asked = None;
    match ev {
        IntentEvent::Proposals { proposals, .. } => settle(view, proposals.iter().map(from_bus).collect()), // VUGFACESTREAM (SR535)
        IntentEvent::Refused { why, .. } => {
            let k = withdraw(ph); // VUGFACESTREAM (SR535): every preview withdrawn, the count named
            ph.note = Some(refused_note(why, k));
            true
        }
        IntentEvent::Propose { .. } | IntentEvent::Proposal { .. } => false,
    }
}

// ---- VUGFACESTREAM (SR535): the streamed preview (named tail block) ----

/// The one verify road: `p` checked by `doc_core::verify` against the face's document and the measured outline, Open.
fn verified(doc: &Doc, m: &Measurement, t: Option<&Measurement>, p: Proposal, streaming: Option<u32>) -> Offer {
    let verdict = judged(doc, &p, m, t); // VUGINTENTTHICKNESS (SR557): the held thickness beside the outline
    Offer { proposal: p, verdict, state: State::Open, streaming, named: String::new(), set: None, page: 0 } // VUGINTENTSETFACE (SR570): named / set by `mark`
}

/// The one corner and focus rule after `n` proposals were offered: `intent: proposed=n acceptable=a refused=r` over the
/// open offers, and the first open one focused when the focus is not on an open one.
fn tally(ph: &mut Photo, n: usize) {
    let refused = ph.offers.iter().filter(|o| o.state == State::Open && matches!(o.verdict, Verdict::Refuse { .. })).count();
    let open = ph.offers.iter().filter(|o| o.state == State::Open).count();
    ph.note = Some(format!("intent: proposed={n} acceptable={} refused={refused}", open - refused));
    refocus(ph);
}

/// The first open offer takes the focus when the focus is not on an open one.
fn refocus(ph: &mut Photo) {
    if ph.focus.is_none_or(|i| ph.offers.get(i).is_none_or(|o| o.state != State::Open)) {
        ph.focus = ph.offers.iter().position(|o| o.state == State::Open);
    }
}

/// Vein's proposal as doc_core's.
fn from_bus(q: &IntentProposal) -> Proposal {
    Proposal { stmt: q.stmt.clone(), confidence: q.confidence, why: q.why.clone() }
}

/// A streamed proposal for the live `request` at `index`: a preview at the same index is replaced and verified again,
/// else a new preview is verified and offered; the corner counts the previews. True (the frame changed).
fn preview(view: &mut View, request: u64, index: u32, p: Proposal) -> bool {
    let (Some(face), Some(ph)) = (view.design.face.as_ref(), view.photo.as_mut()) else { return false };
    let o = verified(&doc_of(face), &ph.measurement(), ph.thickness.as_ref(), p, Some(index)); // VUGINTENTTHICKNESS (SR557)
    match ph.offers.iter().position(|q| q.streaming == Some(index)) {
        Some(j) => {
            let state = ph.offers[j].state;
            ph.offers[j] = Offer { state, ..o };
        }
        None => ph.offers.push(o),
    }
    let k = ph.offers.iter().filter(|q| q.streaming.is_some()).count();
    ph.note = Some(format!("Vein streaming: {k} proposals (request {request})"));
    refocus(ph);
    true
}

/// The terminal `Proposals`: item i settles the preview at index i (equal: the mark cleared, the verdict kept; differing:
/// replaced and verified again, its state kept), an item with no preview is verified and offered, a preview past the
/// list is withdrawn; then the one corner. True (the frame changed).
fn settle(view: &mut View, ps: Vec<Proposal>) -> bool {
    let (Some(face), Some(ph)) = (view.design.face.as_ref(), view.photo.as_mut()) else { return false };
    let doc = doc_of(face);
    let m = ph.measurement();
    let t = ph.thickness.clone(); // VUGINTENTTHICKNESS (SR557)
    let n = ps.len();
    for (i, p) in ps.into_iter().enumerate() {
        match ph.offers.iter().position(|q| q.streaming == Some(i as u32)) {
            Some(j) if ph.offers[j].proposal == p => ph.offers[j].streaming = None,
            Some(j) => {
                let state = ph.offers[j].state;
                ph.offers[j] = Offer { state, ..verified(&doc, &m, t.as_ref(), p, None) };
            }
            None => ph.offers.push(verified(&doc, &m, t.as_ref(), p, None)),
        }
    }
    withdraw(ph);
    mark(ph, &view.design.journal, face); tally(ph, n); // VUGINTENTSETFACE (SR570): the sets marked before the corner counts
    true
}

/// Every preview withdrawn (removed, the focus re-ruled); the count that had arrived.
fn withdraw(ph: &mut Photo) -> usize {
    let before = ph.offers.len();
    // A settled focused offer keeps the focus: its index less the previews before it.
    let kept = ph.focus.filter(|&i| ph.offers.get(i).is_some_and(|o| o.streaming.is_none())).map(|i| ph.offers[..i].iter().filter(|o| o.streaming.is_none()).count());
    ph.offers.retain(|o| o.streaming.is_none());
    let k = before - ph.offers.len();
    if k > 0 {
        ph.focus = kept;
        refocus(ph);
    }
    k
}

/// The corner for a refusal after `k` previews: `Vein refused: <why>`, plus ` after <k> proposals` unless Vein's own text
/// already ends with it (handlers/vein `intent`), so the count is never written twice.
fn refused_note(why: &str, k: usize) -> String {
    let tail = format!(" after {k} proposals");
    if k == 0 || why.ends_with(&tail) { format!("Vein refused: {why}") } else { format!("Vein refused: {why}{tail}") }
}

// ---- VUGFACEPHOTOSIDE (SR547): the second photo's thickness (named tail block) ----

/// `photo side <path>` (main.rs `--side`): [`side`] over the file's bytes, named by its file name; a read error is
/// refused by name the same way.
pub fn side_path(view: &mut View, path: &str) -> Result<(), String> {
    let name = std::path::Path::new(path).file_name().and_then(|s| s.to_str()).unwrap_or(path).to_string();
    match std::fs::read(path) {
        Ok(bytes) => side(view, &bytes, &name),
        Err(e) => side_refused(view, format!("{path}: {e}")),
    }
}

/// A photo of the part on its side: `photo_core::measure_side` against the held flat measurement ([`Photo::first`]),
/// and its thickness as ONE proposal through [`offer`] (the one verify road): the extrude of the face's sketch (the
/// largest profile, along the plane's normal) by `thickness_mm`, written by doc_core's writer; why names the side
/// photo and its sigma, the confidence is the pairing margin inside measure_side's band. The corner reads
/// `side <name>: thickness=<mm> sigma=<mm> paired=<mm>`. Refused by name (`NoFirstPhoto`, `image ..`, measure_side's
/// refusal) in the corner and in the `Err`; nothing is offered then. VUGFACEPHOTOSIDEPARALLAX (SR569): the camera
/// height self-calibrated from the side photo's lattice ([`side_camera`] with `None`). VUGFACEPHOTOCAMERAPREF (SR576):
/// the camera height is the photo's `prefs.camera_mm` (Principia's `vug.photo.camera_mm`) when set.
pub fn side(view: &mut View, bytes: &[u8], name: &str) -> Result<(), String> {
    let camera_mm = view.photo.as_ref().and_then(|ph| ph.prefs.camera_mm);
    side_camera(view, bytes, name, camera_mm)
}

/// VUGFACEPHOTOSIDEPARALLAX (SR569): [`side`] through `photo_core::measure_side_raised` (the part stands on its side,
/// its top raised): `camera_mm` the person's camera height (Principia's `vug.photo.camera_mm` through [`side`],
/// VUGFACEPHOTOCAMERAPREF SR576), `None` the lattice's own solve. The corner and why add `camera=<mm> mm lattice|set`.
pub fn side_camera(view: &mut View, bytes: &[u8], name: &str, camera_mm: Option<f64>) -> Result<(), String> {
    let (p, s) = match side_proposal(view, bytes, name, camera_mm) {
        Ok(v) => v,
        Err(e) => return side_refused(view, e),
    };
    hold_thickness(view, &s); // VUGINTENTTHICKNESS (SR557): held before the offer, so the extrude is judged by it
    offer(view, vec![p]);
    if let Some(ph) = view.photo.as_mut() {
        ph.note = Some(format!("side {name}: thickness={} sigma={} paired={} {}", num(s.thickness_mm), num(s.sigma_mm), num(s.paired_mm), camera_text(&s, camera_mm.is_some())));
    }
    Ok(())
}

/// A side refusal: `refused side <why>` in the corner (when a photo is open) and returned.
fn side_refused(view: &mut View, e: String) -> Result<(), String> {
    let text = format!("refused side {e}");
    if let Some(ph) = view.photo.as_mut() {
        ph.note = Some(text.clone());
    }
    Err(text)
}

/// The side photo measured and its extrude proposal built (nothing offered yet).
fn side_proposal(view: &View, bytes: &[u8], name: &str, camera_mm: Option<f64>) -> Result<(Proposal, photo_core::Side), String> {
    let (Some(face), Some(first)) = (view.design.face.as_ref(), view.photo.as_ref().and_then(|ph| ph.first.as_ref())) else {
        return Err("NoFirstPhoto".to_string());
    };
    let image = pixel_core::decode(bytes).map_err(|e| format!("image {e:?}"))?;
    let principal_px = view.photo.as_ref().and_then(|ph| ph.prefs.principal_px); // VUGFACEPHOTOCAMERAPREF (SR576)
    let params = photo_core::PhotoParams { camera_mm, principal_px, ..photo_core::PhotoParams::new(first.pitch_mm) }; // VUGFACEPHOTOSIDEPARALLAX (SR569)
    let s = photo_core::measure_side_raised(first, &image, params, Tol::DEFAULT).map_err(|e| match e {
        photo_core::Refusal::NoSideMatch { length_mm, nearest_mm, band_mm } => {
            format!("NoSideMatch length={} nearest={} band={}", num(length_mm), num(nearest_mm), num(band_mm))
        }
        photo_core::Refusal::NoCamera { tilt_deg } => format!("NoCamera tilt={}", num(tilt_deg)), // VUGFACEPHOTOSIDEPARALLAX (SR569)
        e => e.name().to_string(),
    })?;
    let outer = face
        .profiles
        .iter()
        .max_by(|a, b| a.outer.area.abs().total_cmp(&b.outer.area.abs()))
        .and_then(|p| p.outer.edges.first())
        .map(|e| PlainNames.entity(e.src()))
        .ok_or("NoProfile")?;
    let mut doc = doc_of(face);
    let id = (0u32..).map(|n| format!("x{n}")).find(|id| !doc.stmts.iter().any(|st| st.id() == Some(id.as_str()))).unwrap_or_default();
    let z = face.plane.z;
    doc.stmts.push(Stmt::Extrude { id, sketch: SKETCH_ID.to_string(), outer, dir: [z.x, z.y, z.z], length: s.thickness_mm });
    let stmt = doc_core::write(&doc).lines().last().unwrap_or("").to_string();
    let band = photo_core::fit::BAND_SIGMA * first.sigma_mm.hypot(s.sigma_mm);
    let confidence = if band > 0.0 { (1.0 - (s.length_mm - s.paired_mm).abs() / band).clamp(0.0, 1.0) } else { 0.0 };
    let why = format!("side photo {name}: thickness {} sigma {} {}", num(s.thickness_mm), num(s.sigma_mm), camera_text(&s, camera_mm.is_some()));
    Ok((Proposal { stmt, confidence, why }, s))
}

/// The statement a proposal appends, when it is an extrude of the photo's sketch: (outer, dir, length).
fn extruded(doc: &Doc, p: &Proposal) -> Option<(String, [f64; 3], f64)> {
    let cand = doc_core::apply(doc, p).ok()?;
    if cand.stmts.len() != doc.stmts.len() + 1 {
        return None;
    }
    match cand.stmts.last()? {
        Stmt::Extrude { sketch, outer, dir, length, .. } if sketch == SKETCH_ID => Some((outer.clone(), *dir, *length)),
        _ => None,
    }
}

/// Enter on an accepted extrude (offer `i`): [`extrude_road`]; Ok -> the journal, the face (closed: None) and the body
/// it regenerated, the offer Applied; Err -> nothing changes and the corner names it.
fn enter_extrude(view: &mut View, i: usize, x: (String, [f64; 3], f64)) -> bool {
    let r = extrude_road(&view.design, x);
    let Some(ph) = view.photo.as_mut() else { return false };
    match r {
        Ok((journal, regen)) => {
            let stmt = ph.offers[i].proposal.stmt.trim().to_string();
            ph.offers[i].state = State::Applied;
            ph.note = Some(format!("applied {stmt}"));
            ph.focus = ph.offers.iter().position(|o| o.state == State::Open);
            view.design.journal = journal;
            view.design.face = regen.face;
            view.design.body = regen.body;
            view.design.pull = None;
            view.design.refusal = None;
        }
        Err(e) => ph.note = Some(format!("refused by the sketch {e}")),
    }
    true
}

/// The pull's own journal road: the profile `outer` names (PlainNames, the names the proposal was verified in) as a
/// committed `Pull` along `dir` (the sketch normal, within `Tol::DEFAULT.ang`) by `length`, recorded on a COPY of the
/// journal (`Journal::record`, which reads the pull's profile, plane and distance), then `journal::regenerate` (the
/// history edit's road): the state `commit_pull` leaves, or a named refusal with the journal untouched.
fn extrude_road(d: &crate::sketch::Design, x: (String, [f64; 3], f64)) -> Result<(crate::journal::Journal, crate::journal::Regen), String> {
    let (outer, dir, length) = x;
    let face = d.face.as_ref().ok_or("no open sketch")?;
    let z = face.plane.z;
    let dn = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    let cross = [dir[1] * z.z - dir[2] * z.y, dir[2] * z.x - dir[0] * z.z, dir[0] * z.y - dir[1] * z.x];
    let sin = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt() / dn;
    if !(dn > 0.0 && sin <= Tol::DEFAULT.ang) {
        return Err(format!("dir {},{},{} is not the sketch normal", num(dir[0]), num(dir[1]), num(dir[2])));
    }
    let along = dir[0] * z.x + dir[1] * z.y + dir[2] * z.z;
    let profile = face
        .profiles
        .iter()
        .find(|p| p.outer.edges.first().map(|e| PlainNames.entity(e.src())).as_deref() == Some(outer.as_str()))
        .cloned()
        .ok_or_else(|| format!("outer {outer} names no profile"))?;
    let o = face.plane.o;
    let distance = if along < 0.0 { -length } else { length };
    let pull = Pull { profile, plane: face.plane, centroid: [o.x, o.y, o.z], distance, start: (0.0, 0.0), preview: None };
    let mut j = d.journal.clone();
    j.record(Some(face), Some(&pull));
    let r = crate::journal::regenerate(&j.doc)?;
    Ok((j, r))
}

// ---- VUGINTENTTHICKNESS (SR557): the extrude judged by the side photo's thickness (named tail block) ----

/// The side photo's thickness held on the photo as `Measurement::Thickness`: `thickness_mm` along the open sketch's
/// normal, checked at the band the outline is (`PhotoParams.band.lin` or the side photo's sigma, whichever is larger:
/// [`Photo::band_mm`]'s rule). Nothing without an open sketch and a photo.
fn hold_thickness(view: &mut View, s: &photo_core::Side) {
    let Some(z) = view.design.face.as_ref().map(|f| f.plane.z) else { return };
    let Some(ph) = view.photo.as_mut() else { return };
    let pitch = ph.first.as_ref().map_or(PITCH_MM, |m| m.pitch_mm);
    let sigma_mm = photo_core::PhotoParams::new(pitch).band.lin.max(s.sigma_mm);
    ph.thickness = Some(Measurement::Thickness { mm: s.thickness_mm, sigma_mm, along: z });
}

/// The face's verdict on `p`: `doc_core::verify` against the outline `m`; when that accepts, a thickness `t` is held
/// and `p` is an extrude of the photo's sketch ([`extruded`]), `doc_core::verify` against `t` too. The first refusal
/// is the verdict (the outline's, then the thickness's), else the accept with the larger `residual_sigma`. A
/// constraint proposal is never judged by the thickness.
fn judged(doc: &Doc, p: &Proposal, m: &Measurement, t: Option<&Measurement>) -> Verdict {
    let v = doc_core::verify(doc, p, m, Tol::DEFAULT);
    let Verdict::Accept { residual_sigma: outline_sigma, .. } = v else { return v };
    let Some(t) = t else { return v };
    if extruded(doc, p).is_none() {
        return v;
    }
    match doc_core::verify(doc, p, t, Tol::DEFAULT) {
        Verdict::Accept { residual_sigma, .. } if residual_sigma <= outline_sigma => v,
        w => w,
    }
}

// ---- VUGINTENTUNDOSET (SR563): a statement set enters whole as one journal step (named tail block) ----

/// Enter on offer `i` when it is a member of a doc_core statement set: the run is the consecutive Open, settled offers
/// around `i` sharing its non-empty `why`; doc_core's `verify_all` decides whether the run is ONE set (`Batch.sets`
/// from 0 over the whole run) and gives its one verdict, then [`judged`]'s thickness rule for a set that extrudes the
/// photo's sketch. None (the single road) when the run is not one set. A refusal names itself and applies nothing; an
/// accept enters through [`enter_set`].
fn set_road(view: &mut View, i: usize) -> Option<bool> {
    let (face, ph) = (view.design.face.as_ref()?, view.photo.as_ref()?);
    let (lo, hi) = set_run(ph, i)?; // VUGINTENTSETFACE (SR570): the one run rule
    let why = ph.offers[i].proposal.why.clone();
    // VUGINTENTSETFACE (SR570): the set re-verified in the journal's CURRENT document, in the names captured at offer time.
    let (doc, names) = journal_doc(&view.design.journal, face);
    let set = named_set(ph, lo, hi, &names);
    let (verdict, _, cand) = judge_set(&doc, &set, &ph.measurement(), ph.thickness.as_ref())?;
    let f = face.clone();
    let entered = match &verdict {
        Verdict::Refuse { why } => Err(format!("refused {}", why.text())),
        Verdict::Accept { .. } => enter_set(&mut view.design, &f, &doc, cand).map_err(|e| format!("refused by the sketch {e}")),
    };
    let ph = view.photo.as_mut()?;
    for o in &mut ph.offers[lo..=hi] {
        o.verdict = verdict.clone();
    }
    match entered {
        Ok(()) => {
            for o in &mut ph.offers[lo..=hi] {
                o.state = State::Applied;
            }
            ph.note = Some(format!("applied set n={} {}", set.len(), why));
            ph.focus = ph.offers.iter().position(|o| o.state == State::Open);
        }
        Err(e) => ph.note = Some(e),
    }
    Some(true)
}

/// The accepted set's document `cand` into the journal as ONE snapshot, on a copy (the journal untouched on a refusal,
/// [`extrude_road`]'s shape): `Journal::record` makes the face's sketch the document before the set, which must be
/// `doc` (the names the set was verified in), then `Journal::adopt(cand)`; the journal, face and body swapped in.
fn enter_set(d: &mut crate::sketch::Design, f: &SketchFace, doc: &Doc, cand: Doc) -> Result<(), String> {
    let mut j = d.journal.clone();
    j.record(Some(f), None);
    if j.doc != *doc {
        return Err("the document is not the photo's sketch".to_string());
    }
    let r = j.adopt(cand)?;
    d.journal = j;
    d.face = r.face;
    d.body = r.body;
    d.pull = None;
    d.refusal = None;
    Ok(())
}

// ---- VUGFACEPHOTOSIDEPARALLAX (SR569): the camera the side photo's parallax was corrected by (named tail block) ----

/// `camera=<mm> mm lattice` (the side lattice's own solve) or `camera=<mm> mm set` (the person's `camera_mm`); empty
/// on a side measured without the raised correction.
fn camera_text(s: &photo_core::Side, set: bool) -> String {
    match s.parallax {
        Some(p) => format!("camera={} mm {}", num(p.camera.height_mm), if set { "set" } else { "lattice" }),
        None => String::new(),
    }
}

// ---- VUGINTENTSETFACE (SR570): the set as one card, re-verified in the journal's names (named tail block) ----

/// An offer's place in one doc_core statement set: consecutive offers with one `key` (the first member's index when it
/// was marked) are ONE card; `term` is the measurement its verdict was judged by ([`judge_set`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Member {
    pub key: usize,
    pub term: Term, // VUGINTENTTERMTYPE (SR577)
}

/// A statement set as drawn: its members' statements (upper-cased, the bitmap font's case), the term and the set's one
/// verdict (every member carries it).
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub lines: Vec<String>,
    pub term: Term, // VUGINTENTTERMTYPE (SR577)
    pub verdict: Verdict,
    /// VUGINTENTSETFACESCROLL (SR583): the first member line shown (the head offer's `page`, clamped by [`paged`]).
    pub first: usize,
    /// VUGINTENTSETFACESCROLL (SR583): the member lines the sheet shows under the head ([`paged`]); every line until measured.
    pub fit: usize,
}

/// The face's document in the JOURNAL's names, and the face-slot (PlainNames, [`doc_of`]) -> journal id pairs: a journal
/// clone takes `Journal::record` of the face (enter_set's own move). When the journal document holds exactly doc_of's
/// statements kind for kind (the photo's sketch alone), that document and the pairs; else doc_of and no pairs, so
/// enter_set's SR563 refusal ("the document is not the photo's sketch") still names it. A panel delete renames the
/// face's slots (regenerate re-densifies them) while the journal keeps its ids: the journal's name is the stable one.
fn journal_doc(j: &crate::journal::Journal, face: &SketchFace) -> (Doc, Vec<(String, String)>) {
    let doc = doc_of(face);
    let mut jc = j.clone();
    jc.record(Some(face), None);
    let same = jc.doc.stmts.len() == doc.stmts.len()
        && doc.stmts.iter().zip(&jc.doc.stmts).all(|(a, b)| std::mem::discriminant(a) == std::mem::discriminant(b));
    if !same {
        return (doc, Vec::new());
    }
    let names = doc.stmts.iter().zip(&jc.doc.stmts).filter_map(|(a, b)| Some((a.id()?.to_string(), b.id()?.to_string()))).collect();
    (jc.doc, names)
}

/// `stmt` with its own id (the second word) and every `key=value` head (before a `.`) that `names` maps re-written to
/// the mapped name; the input untouched when nothing maps.
pub(crate) fn renamed(stmt: &str, names: &[(String, String)]) -> String { // VUGFACEJOURNALFAULTFIXREF (SR590): the fix card re-names through it
    let look = |n: &str| names.iter().find(|(a, b)| a == n && a != b).map(|(_, b)| b.clone());
    let mut changed = false;
    let words: Vec<String> = stmt
        .split_whitespace()
        .enumerate()
        .map(|(k, w)| {
            let new = if k == 1 {
                look(w)
            } else if let Some((key, v)) = w.split_once('=') {
                let (head, tail) = v.split_once('.').map_or((v, None), |(h, t)| (h, Some(t)));
                look(head).map(|n| match tail {
                    Some(t) => format!("{key}={n}.{t}"),
                    None => format!("{key}={n}"),
                })
            } else {
                None
            };
            changed |= new.is_some();
            new.unwrap_or_else(|| w.to_string())
        })
        .collect();
    if changed { words.join(" ") } else { stmt.to_string() }
}

/// After offers are verified ([`offer`], [`settle`]): every offer not yet named takes its statement re-mapped into the
/// journal's current names ([`journal_doc`], [`renamed`]) — a FRESH own id (no face slot named it) that the journal
/// document already holds moves to the first free `<id>-<n>` (its references in the batch follow), so it never replaces
/// a journal statement; then each run [`set_run`] finds with an unmarked member is judged once by [`judge_set`] in the
/// journal document: one set -> every member marked ([`Member`]) and given the set's verdict, else its marks cleared.
fn mark(ph: &mut Photo, j: &crate::journal::Journal, face: &SketchFace) {
    let (doc, mut names) = journal_doc(j, face);
    let held = |id: &str, names: &[(String, String)]| doc.stmts.iter().any(|s| s.id() == Some(id)) || names.iter().any(|(_, b)| b == id);
    for o in ph.offers.iter().filter(|o| o.named.is_empty()) {
        let Some(own) = o.proposal.stmt.split_whitespace().nth(1) else { continue };
        if names.iter().any(|(a, _)| a == own) || !doc.stmts.iter().any(|s| s.id() == Some(own)) {
            continue;
        }
        let free = (1u32..).map(|n| format!("{own}-{n}")).find(|f| !held(f.as_str(), &names)).unwrap_or_default();
        names.push((own.to_string(), free));
    }
    for o in ph.offers.iter_mut().filter(|o| o.named.is_empty()) {
        o.named = renamed(&o.proposal.stmt, &names);
    }
    let (m, t) = (ph.measurement(), ph.thickness.clone());
    let mut i = 0;
    while i < ph.offers.len() {
        let Some((lo, hi)) = set_run(ph, i) else {
            i += 1;
            continue;
        };
        if ph.offers[lo..=hi].iter().any(|o| o.set.is_none()) {
            match judge_set(&doc, &named_set(ph, lo, hi, &names), &m, t.as_ref()) {
                Some((v, term, _)) => {
                    for o in &mut ph.offers[lo..=hi] {
                        o.verdict = v.clone();
                        o.set = Some(Member { key: lo, term });
                    }
                }
                None => {
                    for o in &mut ph.offers[lo..=hi] {
                        o.set = None;
                    }
                }
            }
        }
        i = hi + 1;
    }
}

/// SR563's run rule: the consecutive Open, settled offers around `i` sharing its non-empty `why`, as (first, last);
/// None when `i` is not in a run of two or more.
fn set_run(ph: &Photo, i: usize) -> Option<(usize, usize)> {
    let why = &ph.offers.get(i)?.proposal.why;
    let member = |o: &Offer| o.state == State::Open && o.streaming.is_none() && !why.is_empty() && o.proposal.why == *why;
    if !member(&ph.offers[i]) {
        return None;
    }
    let lo = (0..i).rev().take_while(|&j| member(&ph.offers[j])).last().unwrap_or(i);
    let hi = (i + 1..ph.offers.len()).take_while(|&j| member(&ph.offers[j])).last().unwrap_or(i);
    if hi == lo { None } else { Some((lo, hi)) }
}

/// The run's proposals in the journal's names: each offer's `named` (captured when it was offered), else its statement
/// re-mapped now.
fn named_set(ph: &Photo, lo: usize, hi: usize, names: &[(String, String)]) -> Vec<Proposal> {
    ph.offers[lo..=hi]
        .iter()
        .map(|o| {
            let stmt = if o.named.is_empty() { renamed(&o.proposal.stmt, names) } else { o.named.clone() };
            Proposal { stmt, ..o.proposal.clone() }
        })
        .collect()
}

/// SR563's judging: doc_core's `verify_all` decides whether `set` is ONE set (`Batch.sets` from 0 over all of it) and
/// gives its verdict, then [`judged`]'s thickness rule for a set that extrudes the photo's sketch. The verdict, its term
/// (a Residual's own; else the thickness's when it decided; else `m`'s) and the document with the set applied.
fn judge_set(doc: &Doc, set: &[Proposal], m: &Measurement, t: Option<&Measurement>) -> Option<(Verdict, Term, Doc)> {
    let b = doc_core::verify_all(doc, set, m, Tol::DEFAULT);
    if !b.sets.first().is_some_and(|(start, steps)| *start == 0 && steps.len() == set.len()) {
        return None;
    }
    let mut verdict = b.entries.first()?.2.clone();
    let mut term = m.term();
    if let (Verdict::Accept { residual_sigma: outline_sigma, .. }, Some(t)) = (verdict.clone(), t) {
        if b.doc.stmts.iter().any(|s| matches!(s, Stmt::Extrude { sketch, .. } if sketch == SKETCH_ID)) {
            match &doc_core::verify_all(doc, set, t, Tol::DEFAULT).entries.first()?.2 {
                Verdict::Accept { residual_sigma, .. } if *residual_sigma <= outline_sigma => {}
                w => {
                    verdict = w.clone();
                    term = t.term();
                }
            }
        }
    }
    if let Verdict::Refuse { why: doc_core::Reason::Residual { term: r, .. } } = &verdict {
        term = *r;
    }
    Some((verdict, term, b.doc))
}

/// Offer `i` is a card's later member: the offer before it is an Open member of the same set.
fn tail_member(ph: &Photo, i: usize) -> bool {
    let Some(s) = ph.offers.get(i).and_then(|o| o.set) else { return false };
    i > 0 && ph.offers[i - 1].state == State::Open && ph.offers[i - 1].set == Some(s)
}

/// The Open members of the set offer `i` belongs to, as (first, last); None when `i` is not a member.
fn card_run(ph: &Photo, i: usize) -> Option<(usize, usize)> {
    let s = ph.offers.get(i)?.set?;
    let member = |j: usize| ph.offers[j].state == State::Open && ph.offers[j].set == Some(s);
    if !member(i) {
        return None;
    }
    let lo = (0..i).rev().take_while(|&j| member(j)).last().unwrap_or(i);
    let hi = (i + 1..ph.offers.len()).take_while(|&j| member(j)).last().unwrap_or(i);
    Some((lo, hi))
}

/// The card offer `i` heads: its members' statements, the term and the set's verdict.
fn card_of(ph: &Photo, i: usize) -> Option<Card> {
    let (lo, hi) = card_run(ph, i)?;
    let term = ph.offers[i].set?.term;
    let lines: Vec<String> = ph.offers[lo..=hi].iter().map(|o| o.proposal.stmt.trim().to_uppercase()).collect();
    Some(Card { first: ph.offers[i].page, fit: lines.len(), lines, term, verdict: ph.offers[i].verdict.clone() }) // VUGINTENTSETFACESCROLL (SR583): first / fit
}

/// The card's head line: `SET N=<n> <why> R=<rms> C=<least confidence> TERM=<term>`, or `... REFUSED <reason> TERM=`.
fn card_head(ph: &Photo, i: usize, c: &Card) -> String {
    let (lo, hi) = card_run(ph, i).unwrap_or((i, i));
    let why = ph.offers[i].proposal.why.trim();
    let n = c.lines.len();
    match &c.verdict {
        Verdict::Accept { residual_mm, .. } => {
            let conf = ph.offers[lo..=hi].iter().map(|o| o.proposal.confidence).fold(f64::INFINITY, f64::min);
            format!("SET N={n} {why} R={} C={} TERM={}", num(*residual_mm), num(conf), c.term)
        }
        Verdict::Refuse { why: r } => format!("SET N={n} {why} REFUSED {} TERM={}", r.text(), c.term),
    }
}

/// The card's member statements, one row each under its head (the overlay's one loop).
fn card_lines(fr: &mut Raster, g: &ProposalGlyph, col: [u8; 4]) {
    let Some(c) = &g.card else { return };
    for (k, l) in c.rows().iter().enumerate() { // VUGINTENTSETFACESCROLL (SR583): the page's rows and its more line
        raster::text(fr, l, g.at.0 + 8.0, g.at.1 + 12.0 * (k + 1) as f32, col);
    }
}

/// Escape (or Backspace) on a card: every Open member Rejected at once, the corner `rejected set n=<n> <why>`, the focus
/// to the first open offer. None when offer `i` is not a card's member (the single road's keys).
fn reject_card(view: &mut View, i: usize) -> Option<bool> {
    let ph = view.photo.as_mut()?;
    let (lo, hi) = card_run(ph, i)?;
    for o in &mut ph.offers[lo..=hi] {
        o.state = State::Rejected;
    }
    ph.note = Some(format!("rejected set n={} {}", hi - lo + 1, ph.offers[lo].proposal.why.trim()));
    ph.focus = ph.offers.iter().position(|o| o.state == State::Open);
    Some(true)
}


// ---- VUGINTENTSINGLERENAMED (SR577): the single proposal in the journal's names (named tail block) ----

/// Enter on a lone offer `i` once the journal's names are not the face's: the single is a set of ONE on SR570's road:
/// [`journal_doc`], [`named_set`] (the statement captured in the journal's names at offer time), [`judged`] in the
/// journal's current document, then [`enter_set`] (one snapshot). None (the single road as it was) with no journal map,
/// when the map renames nothing and the captured statement is the offer's own, for a streaming preview, and for an
/// extrude (SR547's pull road). A refusal names itself and applies nothing; the corner says `applied <statement>`.
fn single_road(view: &mut View, i: usize) -> Option<bool> {
    let (face, ph) = (view.design.face.as_ref()?, view.photo.as_ref()?);
    let o = ph.offers.get(i)?;
    if o.streaming.is_some() {
        return None;
    }
    let (doc, names) = journal_doc(&view.design.journal, face);
    let p = named_set(ph, i, i, &names).pop()?;
    if names.is_empty() || (names.iter().all(|(a, b)| a == b) && p.stmt == o.proposal.stmt) {
        return None;
    }
    if let Some(x) = extruded(&doc, &p) {
        return extrude_named(view, i, &doc, &p, x, &names); // VUGINTENTSETFACESCROLL (SR583): the extrude in the journal's names, SR547's apply
    }
    let stmt = o.proposal.stmt.trim().to_string();
    let verdict = judged(&doc, &p, &ph.measurement(), ph.thickness.as_ref());
    let f = face.clone();
    let entered = match &verdict {
        Verdict::Refuse { why } => Err(format!("refused {}", why.text())),
        Verdict::Accept { .. } => doc_core::apply(&doc, &p)
            .map_err(|r| r.text())
            .and_then(|cand| enter_set(&mut view.design, &f, &doc, cand))
            .map_err(|e| format!("refused by the sketch {e}")),
    };
    let ph = view.photo.as_mut()?;
    ph.offers[i].verdict = verdict;
    match entered {
        Ok(()) => {
            ph.offers[i].state = State::Applied;
            ph.note = Some(format!("applied {stmt}"));
            ph.focus = ph.offers.iter().position(|o| o.state == State::Open);
        }
        Err(e) => ph.note = Some(e),
    }
    Some(true)
}

// ---- VUGFACEPHOTOCAMERAPREF (SR576): the camera's datum from Principia's store (named tail block) ----

/// The photo face's namespace in Principia's store (`vug.photo.camera_mm`, `vug.photo.principal_px`; declared in
/// prefs_core's schema with consumer defaults, operator-written).
pub const PREF_NS: &str = "vug";

/// The camera's once-per-setup datum: its height above the paper and its principal point. `None` = unset: the
/// lattice's own camera solve, the photo's centre.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhotoPrefs {
    /// `vug.photo.camera_mm`: the camera height (mm) the side photo's parallax correction takes.
    pub camera_mm: Option<f64>,
    /// `vug.photo.principal_px` (`x,y`): the principal point in photo pixels (the camera solve and the lens term).
    pub principal_px: Option<Vec2>,
}

impl PhotoPrefs {
    /// The prefs from a lookup of `vug` keys (the store's `get` in [`PhotoPrefs::load_from`]): the camera height a
    /// finite positive float (or a hand-written positive integer), the principal point `x,y` with both finite; anything
    /// else is unset.
    pub fn from_prefs(get: impl Fn(&str) -> Option<bandy::PrefValue>) -> PhotoPrefs {
        let camera_mm = match get("photo.camera_mm") {
            Some(bandy::PrefValue::Float(f)) if f.is_finite() && f > 0.0 => Some(f),
            Some(bandy::PrefValue::Int(i)) if i > 0 => Some(i as f64),
            _ => None,
        };
        let principal_px = match get("photo.principal_px") {
            Some(bandy::PrefValue::Str(s)) => xy(&s),
            _ => None,
        };
        PhotoPrefs { camera_mm, principal_px }
    }

    /// The prefs in the store folder `dir`, read-only (`principia::PrefStore::load`; Principia stays the writer of
    /// record). An unreadable store is the defaults.
    pub fn load_from(dir: &std::path::Path) -> PhotoPrefs {
        match principia::PrefStore::load(dir) {
            Ok(store) => PhotoPrefs::from_prefs(|k| store.get(PREF_NS, k)),
            Err(_) => PhotoPrefs::default(),
        }
    }

    /// The prefs in the standard store (`principia::default_prefs_path()`): main.rs calls it once at start.
    pub fn load() -> PhotoPrefs {
        PhotoPrefs::load_from(&principia::default_prefs_path())
    }

    /// photo_core's params on `pitch_mm` paper under these prefs.
    pub fn params(&self, pitch_mm: f64) -> photo_core::PhotoParams {
        photo_core::PhotoParams { camera_mm: self.camera_mm, principal_px: self.principal_px, ..photo_core::PhotoParams::new(pitch_mm) }
    }
}

/// `x,y` as a point (both finite), else None.
fn xy(s: &str) -> Option<Vec2> {
    let (x, y) = s.split_once(',')?;
    let (x, y) = (x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?);
    (x.is_finite() && y.is_finite()).then(|| Vec2::new(x, y))
}


// ---- VUGPHOTOCROPMETA (SR582): the photo's command road, writing the camera's datum to Principia's store (named tail block) ----

/// The usage the photo's command road refuses with.
pub const PREF_USAGE: &str = "usage: camera <mm>|clear | pp <x> <y>|clear"; // VUGPHOTOPREFFACEWIRE (SR588): clear

/// One line on the photo's command road in the standard store ([`command_in`] on `principia::default_prefs_path()`).
pub fn command(view: &mut View, line: &str) -> Result<bool, String> {
    command_in(view, &principia::default_prefs_path(), line)
}

/// One line on the photo's command road in the store folder `dir`: `camera <mm>` writes `vug.photo.camera_mm`, `pp <x>
/// <y>` writes `vug.photo.principal_px` (`x,y`), through `principia::PrefStore::set` (the schema's check and clamp; the
/// rows are operator-written and this is the host operator road, quartzite `chrome_rows::store_str`'s). The stored value
/// is folded into the open photo's prefs by [`PhotoPrefs::from_prefs`]'s rules, so the next side read and open take it;
/// the corner reads `pref camera=<mm> mm stored` / `pref pp=<x>,<y> stored` (` clamped` when the schema clamped it).
/// Refused by name, nothing stored: [`PREF_USAGE`] (anything else, or a number that is not finite), `pref: <why>` (the
/// store did not load or refused the value). True when stored. VUGPHOTOPREFFACEWIRE (SR588): `camera clear` / `pp clear`
/// unset the key ([`clear_in`]).
pub fn command_in(view: &mut View, dir: &std::path::Path, line: &str) -> Result<bool, String> {
    let words: Vec<&str> = line.split_whitespace().collect(); if let Some(r) = clear_in(view, dir, &words) { return r; } // VUGPHOTOPREFFACEWIRE (SR588): `camera clear` / `pp clear` unset the key
    let finite = |t: &str| t.parse::<f64>().ok().filter(|v| v.is_finite());
    let (key, value) = match words.as_slice() {
        ["camera", mm] => ("photo.camera_mm", bandy::PrefValue::Float(finite(*mm).ok_or(PREF_USAGE)?)),
        ["pp", x, y] => {
            let (x, y) = (finite(*x).ok_or(PREF_USAGE)?, finite(*y).ok_or(PREF_USAGE)?);
            ("photo.principal_px", bandy::PrefValue::Str(format!("{},{}", num(x), num(y))))
        }
        _ => return Err(PREF_USAGE.to_string()),
    };
    let out = match principia::PrefStore::load(dir).and_then(|mut store| store.set(PREF_NS, key, value)) {
        Ok(out) => out,
        Err(e) => {
            let text = format!("pref: {e}");
            if let Some(ph) = view.photo.as_mut() {
                ph.note = Some(format!("refused {text}"));
            }
            return Err(text);
        }
    };
    let applied = PhotoPrefs::from_prefs(|k| (k == key).then(|| out.value.clone()));
    let shown = match &out.value {
        bandy::PrefValue::Float(f) => num(*f),
        bandy::PrefValue::Str(s) => s.clone(),
        v => format!("{v:?}"),
    };
    let clamped = if out.clamped { " clamped" } else { "" };
    let text = if key == "photo.camera_mm" { format!("pref camera={shown} mm stored{clamped}") } else { format!("pref pp={shown} stored{clamped}") };
    if let Some(ph) = view.photo.as_mut() {
        if key == "photo.camera_mm" {
            ph.prefs.camera_mm = applied.camera_mm;
        } else {
            ph.prefs.principal_px = applied.principal_px;
        }
        ph.note = Some(text);
    }
    Ok(true)
}

// ---- VUGINTENTSETFACESCROLL (SR583): a card taller than the sheet pages; a single extrude in the journal's names (named tail block) ----

/// A card's row pitch and the bitmap glyph's height (5 rows of 2 px), the painter's own geometry ([`card_lines`]).
const CARD_ROW_PX: f32 = 12.0;
const GLYPH_PX: f32 = 10.0;

/// The card's page on a sheet `sheet_h` px tall with its head at `y` (B1119's rule, Quarry's menu): every member line
/// when they fit under the head; else the rows that fit less one (at least one line), the last row the more line. The
/// first shown line is clamped so the last page is full. No scroll model: the sheet never moves, the page turns.
fn paged(card: Option<Card>, y: f32, sheet_h: u32) -> Option<Card> {
    let mut c = card?;
    let n = c.lines.len();
    let rows = ((sheet_h as f32 - y - GLYPH_PX) / CARD_ROW_PX).floor().max(0.0) as usize;
    c.fit = if n <= rows { n } else { rows.saturating_sub(1).max(1) };
    c.first = c.first.min(n.saturating_sub(c.fit));
    Some(c)
}

impl Card {
    /// The rows painted under the head: the member lines `[first, first + fit)` and, when the card is paged, ONE more
    /// line: `... <n> MORE` while lines remain below, else (the last page) `... <first> BACK` (B1119's `<first> back`).
    pub fn rows(&self) -> Vec<String> {
        let n = self.lines.len();
        let first = self.first.min(n);
        let end = first.saturating_add(self.fit).min(n);
        let mut v = self.lines[first..end].to_vec();
        if self.fit < n {
            let below = n - end;
            v.push(if below > 0 { format!("... {below} MORE") } else { format!("... {first} BACK") });
        }
        v
    }
}

/// PageDown / PageUp on the focused card (offer `i`): its head's page moves by the fit the painter measured (the ONE
/// geometry: the head's glyph from [`glyphs`]), PageDown to at most `n - fit`, PageUp to at least 0. None (the key falls
/// through) off a card and on a card the sheet shows whole; true when the page moved.
fn page_card(view: &mut View, i: usize, k: &str) -> Option<bool> {
    let lo = card_run(view.photo.as_ref()?, i)?.0;
    let c = glyphs(view).into_iter().find(|g| g.offer == lo)?.card?;
    let n = c.lines.len();
    if c.fit >= n {
        return None;
    }
    let first = if k == MORE_CLICK { if c.first + c.fit >= n { 0 } else { (c.first + c.fit).min(n - c.fit) } } else if matches!(k, "Page_Down" | "PageDown" | "Next") { (c.first + c.fit).min(n - c.fit) } else { c.first.saturating_sub(c.fit) }; // VUGINTENTSETFACELISTROWS (SR589): the more-line click is PageDown, 0 from the last page (B1119)
    let ph = view.photo.as_mut()?;
    let moved = ph.offers[lo].page != first;
    ph.offers[lo].page = first;
    Some(moved)
}

/// Enter on a lone extrude once the journal's names are not the face's ([`single_road`]'s take-over rule): `p` (in the
/// journal's names, `doc` the journal's current document) judged by [`judged`] (the outline and a held thickness); a
/// refusal names itself and applies nothing; else the journal-named `outer` mapped back to the face's slot by the
/// inverse of [`journal_doc`]'s pairs, and [`enter_extrude`] (SR547's pull road: `Journal::record` writes the slot's
/// journal name back, the same `outer`).
fn extrude_named(view: &mut View, i: usize, doc: &Doc, p: &Proposal, x: (String, [f64; 3], f64), names: &[(String, String)]) -> Option<bool> {
    let ph = view.photo.as_ref()?;
    let verdict = judged(doc, p, &ph.measurement(), ph.thickness.as_ref());
    let (outer, dir, length) = x;
    let slot = names.iter().find(|(_, b)| *b == outer).map(|(a, _)| a.clone());
    let ph = view.photo.as_mut()?;
    ph.offers[i].verdict = verdict.clone();
    if let Verdict::Refuse { why } = &verdict {
        ph.note = Some(format!("refused {}", why.text()));
        return Some(true);
    }
    let Some(slot) = slot else {
        ph.note = Some(format!("refused by the sketch outer {outer} names no face slot"));
        return Some(true);
    };
    Some(enter_extrude(view, i, (slot, dir, length)))
}

// ---- VUGINTENTSETFACELISTROWS (SR589): the card's more line and the wheel over it (named tail block) ----

/// The pseudo-key a click on a card's more line hands [`page_card`]: PageDown's step, 0 from the last page (B1119).
const MORE_CLICK: &str = "vug.more-line";

/// A paged card's more line as painted by [`card_lines`] (its last row): (x0, y0, w, h), the bitmap's 8 px advance and
/// one row pitch tall. None for a single or a card the sheet shows whole.
fn more_line(g: &ProposalGlyph) -> Option<(f32, f32, f32, f32)> {
    let c = g.card.as_ref()?;
    if c.fit >= c.lines.len() {
        return None;
    }
    let rows = c.rows();
    let last = rows.last()?;
    Some((g.at.0 + 8.0, g.at.1 + CARD_ROW_PX * rows.len() as f32, 8.0 * last.chars().count() as f32, CARD_ROW_PX))
}

/// A paged card's whole rect (head to more line, the widest row): (x0, y0, w, h). None as [`more_line`].
fn card_rect(g: &ProposalGlyph) -> Option<(f32, f32, f32, f32)> {
    more_line(g)?;
    Some(glyph_rect(g)) // VUGINTENTSETFACEANCHOREDOVERLAP (SR596): the one glyph geometry
}

fn inside(r: (f32, f32, f32, f32), x: f64, y: f64) -> bool {
    let (x, y) = (x as f32, y as f32);
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

/// The view's press, first: a Left press on a paged card's `... N MORE` / `... F BACK` line focuses that card and turns
/// its page ([`page_card`] with [`MORE_CLICK`]). None (the press goes on) under `key`'s gate (no photo, a pull or a
/// dimension edit open), for another button, and off every more line.
pub fn press(view: &mut View, button: crate::view::Button, x: f64, y: f64) -> Option<bool> {
    view.photo.as_ref()?;
    if button != crate::view::Button::Left || view.design.pull.is_some() || view.design.face.as_ref().is_some_and(|f| f.edit.is_some()) {
        return None;
    }
    if let Some(r) = crop_press(view, x, y) { return Some(r); } let g = glyphs(view).into_iter().find(|g| more_line(g).is_some_and(|r| inside(r, x, y)))?; // VUGFACEPHOTOCROPSAVE (SR640): an armed crop takes the press first
    let moved = page_card(view, g.offer, MORE_CLICK).unwrap_or(false);
    let ph = view.photo.as_mut()?;
    let focus = ph.focus != Some(g.offer);
    ph.focus = Some(g.offer);
    Some(moved || focus)
}

/// The view's wheel, first: with the cursor over a paged card, one page per event (dy > 0 PageDown, dy < 0 PageUp)
/// through [`page_card`]; the event is consumed there (no zoom). None (the view zooms) elsewhere and under `key`'s gate.
pub fn wheel(view: &mut View, dy: f64) -> Option<bool> {
    view.photo.as_ref()?;
    if dy == 0.0 || view.design.pull.is_some() || view.design.face.as_ref().is_some_and(|f| f.edit.is_some()) {
        return None;
    }
    let (x, y) = view.cursor;
    let g = glyphs(view).into_iter().find(|g| card_rect(g).is_some_and(|r| inside(r, x, y)))?;
    Some(page_card(view, g.offer, if dy > 0.0 { "Page_Down" } else { "Page_Up" }).unwrap_or(false))
}

// ---- VUGINTENTSETFACEANCHOREDOVERLAP (SR596): anchored glyphs stack; the more line's hover cue (named tail block) ----

/// Any glyph's painted rect: its head row and every row [`card_lines`] paints under it (a single is its head alone):
/// (x0, y0, w, h), the bitmap's 8 px advance, the widest row, one [`CARD_ROW_PX`] pitch per row.
pub fn glyph_rect(g: &ProposalGlyph) -> (f32, f32, f32, f32) {
    let rows = g.card.as_ref().map(|c| c.rows()).unwrap_or_default();
    let wide = rows.iter().map(|r| 8.0 + 8.0 * r.chars().count() as f32).fold(8.0 * g.text.chars().count() as f32, f32::max);
    (g.at.0, g.at.1, wide, CARD_ROW_PX * (rows.len() + 1) as f32)
}

fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// SR596's rule, ONE pass over [`glyphs`]' output in offer (= anchor) order: an anchored glyph (its statement's
/// [`anchor`] on screen, the predicate `glyphs` placed it by) whose [`glyph_rect`] overlaps an earlier anchored glyph's
/// moves down to that glyph's foot (by the overlap), again while it overlaps one, its card re-paged at the new height
/// from the head offer's page ([`paged`]). A listed glyph keeps SR589's column. Nothing is stored.
/// VUGINTENTSETFACELISTEDOVERLAP (SR601): first, an anchored glyph overlapping the listed column ([`listed_column`])
/// moves RIGHT to the column's edge + 6 px (the only case x moves, at most once: x never comes back); both checks re-run
/// after either push, so a right-pushed glyph still goes down under an earlier anchored one.
/// VUGINTENTSETFACEPUSHEDGE (SR606): when the right push would carry the rect's right edge past the sheet
/// (`view.size.0`), the glyph goes DOWN to the column's foot instead (x unmoved, [`drop_to`]); at most once per glyph
/// (x and y only grow), then at most one SR596 push per earlier placed rect. A glyph wider than the sheet keeps its
/// anchor x, is clipped, and the corner names it ([`wide_note`]).
/// VUGINTENTSETFACEPUSHFOOT (SR617): when SR596's down push leaves the glyph past the sheet's bottom ([`past_bottom`]),
/// it turns UP for good: over each placed rect it overlaps it rises to that rect's top less its own height (the nearest
/// free y above the obstacle; over a stack, above the stack), through the same [`drop_to`], floored at the running row's
/// foot (the listed column's bottom, else the corner's second row's foot 30 + [`CARD_ROW_PX`]); no rise left ends it.
/// SR606's foot drop lands on that foot, so it passes the bottom only for a glyph taller than the free band below it:
/// clipped at the foot, the corner says so ([`wide_note`]). Bound: the down phase as SR606's; after the turn y strictly
/// falls per lift, never below the foot, never down again (the column cannot overlap at y >= foot); each lift clears one
/// placed rect, which overlaps again only when the re-page grew the card (its rows only grow as it rises, at most its
/// line count): at most 1 + |placed| + |placed| x (lines + 1) steps per glyph.
fn unstack(view: &View, ph: &Photo, face: &SketchFace, mut out: Vec<ProposalGlyph>) -> Vec<ProposalGlyph> {
    let col = listed_column(view, ph, face, &out); // VUGINTENTSETFACELISTEDOVERLAP (SR601): the listed column's strip
    let foot = band_top(col); // VUGINTENTSETFACEPUSHFOOT (SR617): the free band's top, the lift's floor
    let mut placed: Vec<(f32, f32, f32, f32)> = Vec::new();
    for g in &mut out {
        if anchor(face, ph.offers[g.offer].proposal.stmt.trim()).and_then(|q| face.to_screen(view, q)).is_none() {
            continue;
        }
        let (mut up, mut turn) = (false, 0.0f32); // VUGINTENTSETFACEPUSHFOOT (SR617): once a down push passes the bottom, the glyph only rises; VUGINTENTSETFACEPUSHFULL (SR629): turn, the y the down push turned at
        loop {
            let me = glyph_rect(g);
            if let Some(c) = col.filter(|c| overlaps(*c, me)) {
                if c.0 + c.2 + 6.0 + me.2 > view.size.0 as f32 { drop_to(view, ph, g, c.1 + c.3); continue; } // VUGINTENTSETFACEPUSHEDGE (SR606): a right push past the sheet's edge goes down to the column's foot, x unmoved
                g.at.0 = c.0 + c.2 + 6.0; // VUGINTENTSETFACELISTEDOVERLAP (SR601): right to the column's edge + 6, y kept
                continue;
            }
            let Some(r) = placed.iter().copied().find(|r| overlaps(*r, me)) else { break };
            if !up {
                drop_to(view, ph, g, r.1 + r.3); // VUGINTENTSETFACEPUSHEDGE (SR606): SR596's down push through the one re-page
                up = past_bottom(view, glyph_rect(g)); turn = g.at.1; // VUGINTENTSETFACEPUSHFOOT (SR617): past the sheet's bottom: turn up
                if !up {
                    continue;
                }
            }
            let y = (r.1 - glyph_rect(g).3).max(foot); // VUGINTENTSETFACEPUSHFOOT (SR617): the nearest free y above the obstacle, floored at the foot
            if y >= g.at.1 {
                no_free_band(view, ph, g, turn, &placed); break; // VUGINTENTSETFACEPUSHFULL (SR629): on the foot and still overlapping: no free band, back to the walk's last free y
            }
            drop_to(view, ph, g, y); // VUGINTENTSETFACEPUSHFOOT (SR617): the lift through the one re-page
        }
        placed.push(glyph_rect(g));
    }
    out
}

/// The offer whose paged card's more line ([`more_line`]) is under `p`; [`hover`] compares it before and after a
/// motion, so entering, leaving or crossing to another card's more line asks for a frame.
fn more_at(view: &View, p: (f64, f64)) -> Option<usize> {
    glyphs(view).into_iter().find(|g| more_line(g).is_some_and(|r| inside(r, p.0, p.1))).map(|g| g.offer)
}

/// A paged card's more line repainted (the same string at [`card_lines`]' last row, so exactly its pixels): ACTIVE
/// under the cursor, else INK (a control line, not a statement: the cue reads on a focused card too).
fn more_cue(view: &View, fr: &mut Raster, g: &ProposalGlyph) {
    let Some(r) = more_line(g) else { return };
    let Some(last) = g.card.as_ref().and_then(|c| c.rows().pop()) else { return };
    let (x, y) = view.cursor;
    raster::text(fr, &last, r.0, r.1, rgba(if inside(r, x, y) { ACTIVE } else { INK }));
}

// ---- VUGPHOTOPREFFACEWIRE (SR588): clearing the camera's datum (named tail block) ----

/// `camera clear` / `pp clear` on the photo's command road ([`command_in`]'s first look; None for every other line):
/// `vug.photo.camera_mm` / `vug.photo.principal_px` leave Principia's store in `dir` (`principia::PrefStore::unset`,
/// the domain file rewritten by the store's own swap) and the open photo's prefs, so the next side read takes the
/// lattice's own camera solve and the next open the photo's centre (the consumer defaults). The corner reads
/// `pref camera cleared` / `pref pp cleared` (` (was unset)` when nothing was stored); a refusal is
/// [`command_in`]'s (`refused pref: <why>` on the corner, `Err("pref: <why>")`). Ok(true) when a value left the store.
fn clear_in(view: &mut View, dir: &std::path::Path, words: &[&str]) -> Option<Result<bool, String>> {
    let (key, name) = match words {
        ["camera", "clear"] => ("photo.camera_mm", "camera"),
        ["pp", "clear"] => ("photo.principal_px", "pp"),
        _ => return None,
    };
    let gone = match principia::PrefStore::load(dir).and_then(|mut store| store.unset(PREF_NS, key)) {
        Ok(gone) => gone,
        Err(e) => {
            let text = format!("pref: {e}");
            if let Some(ph) = view.photo.as_mut() {
                ph.note = Some(format!("refused {text}"));
            }
            return Some(Err(text));
        }
    };
    if let Some(ph) = view.photo.as_mut() {
        if key == "photo.camera_mm" {
            ph.prefs.camera_mm = None;
        } else {
            ph.prefs.principal_px = None;
        }
        ph.note = Some(format!("pref {name} cleared{}", if gone { "" } else { " (was unset)" }));
    }
    Some(Ok(gone))
}

// ---- VUGFACEPHOTOPREFLINE (SR595): the typed `:` line on the photo's command road (named tail block) ----
// VUGPHOTOPREFFACEWIRE (SR588) reached `command` from the flags only: the `:` line (deviation.rs `Viewer::line_key`) ran
// `map_command` and its gate shut over a photo (a photo opens as its sketch face). The line's Enter now asks
// [`line_command`] first and its gate takes [`line_free`]; the line's text is handed to [`command_in`] verbatim (no
// second parser). With a photo and a map both open (`against_journal` over a photo) a photo verb is the photo's.

/// The photo's verbs on the `:` line: [`command_in`]'s first words.
const LINE_VERBS: [&str; 2] = ["camera", "pp"];

/// `:` opens over a photo (deviation.rs `Viewer::line_key`'s gate, beside the map's): a photo is open and no pull or
/// dimension edit ([`key`]'s gate), journal gesture or edit, or edge tool (the map's gate) holds the keys.
pub fn line_free(view: &View) -> bool {
    view.photo.is_some() && view.design.pull.is_none() && !view.design.face.as_ref().is_some_and(|f| f.edit.is_some()) && !crate::journal::busy(&view.design) && view.fillet.is_none()
}

/// The `:` line's Enter on the photo's road: None (the map's road, `Viewer::map_command`) with no photo open, or with a
/// map up and a line whose first word is not a photo verb ([`LINE_VERBS`]); otherwise [`command_in`] in
/// `view.entry.store`'s folder, or [`command`] (Principia's standard store) when it is None. With a photo and no map
/// every line is the photo's, so a stray line refuses [`PREF_USAGE`] by name.
pub fn line_command(view: &mut View, line: &str) -> Option<Result<bool, String>> {
    view.photo.as_ref()?; if line.trim() == "crop" { return Some(Ok(crop_arm(view))); } // VUGFACEPHOTOCROPSAVE (SR640): `:crop` arms the drag
    let verb = line.split_whitespace().next().is_some_and(|w| LINE_VERBS.contains(&w));
    if view.deviation.is_some() && !verb {
        return None;
    }
    Some(match view.entry.store.clone() {
        Some(dir) => command_in(view, &dir, line),
        None => command(view, line),
    })
}

// ---- VUGINTENTSETFACELISTEDOVERLAP (SR601): the listed column pushes an anchored glyph right; its leader (named tail block) ----

/// The listed column as one strip: the bounding rect of every LISTED glyph's [`glyph_rect`] in [`glyphs`]' output (x 6
/// to the widest listed row's right edge, y from the first listed head to the running row's foot SR589 reserves; the
/// listed rects tile it). None with nothing listed.
fn listed_column(view: &View, ph: &Photo, face: &SketchFace, out: &[ProposalGlyph]) -> Option<(f32, f32, f32, f32)> {
    let listed = out.iter().filter(|g| anchor(face, ph.offers[g.offer].proposal.stmt.trim()).and_then(|q| face.to_screen(view, q)).is_none());
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for g in listed {
        let r = glyph_rect(g);
        (x0, y0, x1, y1) = (x0.min(r.0), y0.min(r.1), x1.max(r.0 + r.2), y1.max(r.1 + r.3));
    }
    (x0 <= x1).then_some((x0, y0, x1 - x0, y1 - y0))
}

/// A pushed anchored glyph's leader: from its cross (where [`overlay`] paints it, (at.0 - 6, at.1 + 14)) to its
/// statement's [`anchor`] on screen. None for a listed glyph and for one the push did not move (the cross within half a
/// pixel of its anchor). The one geometry the painter ([`paint_leader`]) and the tests read.
pub fn leader(view: &View, g: &ProposalGlyph) -> Option<((f32, f32), (f32, f32))> {
    let (face, ph) = (view.design.face.as_ref()?, view.photo.as_ref()?);
    let (ax, ay) = anchor(face, ph.offers.get(g.offer)?.proposal.stmt.trim()).and_then(|q| face.to_screen(view, q))?;
    let (cross, at) = ((g.at.0 - 6.0, g.at.1 + 14.0), (ax as f32, ay as f32));
    ((cross.0 - at.0).abs() > 0.5 || (cross.1 - at.1).abs() > 0.5).then_some((cross, at))
}

/// The leader painted through the raster's line painter in INK, before the glyph's cross and text (so under its own
/// glyph, over the earlier glyphs it crosses).
fn paint_leader(view: &View, fr: &mut Raster, g: &ProposalGlyph) {
    if let Some((a, b)) = leader(view, g) {
        raster::line(fr, a, b, rgba(INK));
    }
}

// ---- VUGINTENTSETFACEPUSHEDGE (SR606): a right push past the sheet's edge goes down to the column's foot; the corner names a glyph wider than the sheet (named tail block) ----

/// SR596's down push and SR606's foot push, ONE re-page: the glyph's head moves to `y` (x unmoved) and its card is
/// re-paged at the new height from its head offer's page ([`paged`]). VUGINTENTSETFACEPUSHFOOT (SR617): either
/// direction: [`unstack`]'s lift moves the head UP through the same statements (no second re-page).
fn drop_to(view: &View, ph: &Photo, g: &mut ProposalGlyph, y: f32) {
    g.at.1 = y;
    if let Some(c) = g.card.as_mut() {
        c.first = ph.offers[g.offer].page;
    }
    g.card = paged(g.card.take(), g.at.1, view.size.1);
}

/// The corner's second row ((6, 30), the row SR589's listed column skips under the note): `glyph wider than the sheet:
/// <id>` for the first glyph in [`glyphs`]' output whose [`glyph_rect`] is wider than the sheet (`view.size.0`), which no
/// push can hold and the raster clips; `<id>` is its statement's doc_core id (`<verb> <id>`), else `#<offer>`. Derived
/// each frame (the pass stores nothing, so [`Photo::note`], the event roads' line, is never overwritten); said once.
/// VUGINTENTSETFACEPUSHFOOT (SR617): ONE line for both words, the first glyph wins: else `glyph taller than the free
/// band: <id>` for an anchored glyph [`past_bottom`] whose painted height passes the free band (the sheet's bottom less
/// the running row's foot, [`band_top`]): the lift stopped at the foot and the raster clips it.
pub fn wide_note(view: &View) -> Option<String> {
    let (face, ph) = (view.design.face.as_ref()?, view.photo.as_ref()?);
    let gs = glyphs(view);
    let band = view.size.1 as f32 - band_top(listed_column(view, ph, face, &gs)); // VUGINTENTSETFACEPUSHFOOT (SR617): the free band
    gs.iter().find_map(|g| {
        let r = glyph_rect(g);
        let anchored = || anchor(face, ph.offers[g.offer].proposal.stmt.trim()).and_then(|q| face.to_screen(view, q)).is_some();
        let word = if r.2 > view.size.0 as f32 {
            "wider than the sheet"
        } else if past_bottom(view, r) && r.3 - (CARD_ROW_PX - GLYPH_PX) > band && anchored() {
            "taller than the free band" // VUGINTENTSETFACEPUSHFOOT (SR617)
        } else if past_bottom(view, r) && pushed_down(view, g) {
            "has no free band" // VUGINTENTSETFACEPUSHFULL (SR629)
        } else {
            return None;
        };
        let stmt = ph.offers.get(g.offer).map(|o| o.proposal.stmt.as_str()).unwrap_or("");
        let id = stmt.split_whitespace().nth(1).map_or_else(|| format!("#{}", g.offer), str::to_string);
        Some(format!("glyph {word}: {id}"))
    })
}

// ---- VUGINTENTSETFACEPUSHFOOT (SR617): a down push past the sheet's bottom lifts above its obstacle; the free band (named tail block) ----

/// A rect past the sheet's bottom by [`paged`]'s own fit: its last row's glyph ([`GLYPH_PX`] of its [`CARD_ROW_PX`]
/// pitch) ends below `view.size.1`.
fn past_bottom(view: &View, r: (f32, f32, f32, f32)) -> bool {
    r.1 + r.3 - (CARD_ROW_PX - GLYPH_PX) > view.size.1 as f32
}

/// The free band's top, the running row's foot: the listed column's bottom ([`listed_column`]), else the corner's
/// second row's foot (30 + [`CARD_ROW_PX`], where SR589's first listed head sits).
fn band_top(col: Option<(f32, f32, f32, f32)>) -> f32 {
    col.map_or(30.0 + CARD_ROW_PX, |c| c.1 + c.3)
}

// ---- VUGINTENTSETFACEPUSHFULL (SR629): a band full of placed rects: the glyph goes back to the walk's last free y; the corner names it (named tail block) ----

/// [`unstack`]'s lift ended on the foot ([`band_top`]) still overlapping a placed rect: no y between the foot and the
/// obstacle is free, and the sheet has no overflow road (it is the window, `view.size`: it never pages or grows; only a
/// card pages, [`paged`]). A glyph the free band holds goes back to the walk's last non-overlapping y, the turn
/// (`turn`, where SR596's down push left it past the bottom), then down under each placed rect it still overlaps there
/// (SR596's push through the one [`drop_to`]): never on a placed rect, x unmoved, clipped, and the corner says
/// `glyph has no free band: <id>` ([`wide_note`]). A glyph taller than the free band (past the bottom on the foot)
/// keeps SR617's foot and SR617's word. Bound: one more walk per glyph; y only grows, each push clears one placed rect
/// for good (the card only shrinks as it falls, [`paged`]'s rows fall with y): at most 1 + |placed| more steps.
fn no_free_band(view: &View, ph: &Photo, g: &mut ProposalGlyph, turn: f32, placed: &[(f32, f32, f32, f32)]) {
    if past_bottom(view, glyph_rect(g)) {
        return;
    }
    drop_to(view, ph, g, turn);
    while let Some(r) = placed.iter().copied().find(|r| overlaps(*r, glyph_rect(g))) {
        drop_to(view, ph, g, r.1 + r.3);
    }
}

/// An anchored glyph whose cross sits BELOW its anchor ([`leader`]): a down push moved it. Past the bottom, only
/// [`no_free_band`] leaves a glyph the free band holds there (a right push keeps y; an untouched glyph has no leader).
fn pushed_down(view: &View, g: &ProposalGlyph) -> bool {
    leader(view, g).is_some_and(|(c, a)| c.1 > a.1 + 0.5)
}

// ---- VUGFACEPHOTOCROPSAVE (SR640): the face's crop, its mask, its save beside the photo, the re-open (named tail block) ----
// VUG-PHOTO-CROP-META-WRITE stayed owed since SR588 measured that the face neither cropped nor saved the photo. `:crop`
// (the photo's `:` line, [`line_command`]) arms; a Left drag on the photo spans the rectangle in photo pixels ([`press`],
// [`hover`], [`release`]: the ray's paper-plane hit through `Photo.h`), [`crop_rect`] the ONE rule the mask and the save
// read; the sheet outside the crop dims through `raster::tint` ([`crop_mask`]); Return keeps it ([`crop_keep`]): the
// cropped pixels (photo_core `fixture::crop`) with photo_core's `crop_pair` as their tEXt pair through pixel_core's
// `encode_rgba_text`, written by lib.rs `write_atomic` beside the photo as `<stem>.crop.png` (never over an existing file,
// never over the original); the controller re-opens it through `Viewer::open_with` ([`follow`]). Escape cancels.

/// The face's crop road on the open photo.
#[derive(Clone, Debug, Default)]
pub struct Crop {
    /// The photo's file (lib.rs `Viewer::open_with` sets it); None for a photo opened from bytes (the save refuses by name).
    pub source: Option<std::path::PathBuf>,
    /// `:crop` armed the drag; Return keeps the rectangle, Escape cancels.
    pub armed: bool,
    /// The press corner, photo pixels (continuous: pixel i spans [i, i + 1)).
    pub from: Option<(f64, f64)>,
    /// The current corner, photo pixels.
    pub to: Option<(f64, f64)>,
    /// A Left press on the photo is held (view.rs `release` ends it through [`release`]).
    pub dragging: bool,
    /// The saved crop the controller re-opens ([`follow`], drained once).
    pub reopen: Option<std::path::PathBuf>,
}

/// The corner while the crop is armed with no rectangle.
const CROP_HINT: &str = "crop: drag a rectangle on the photo, Return keeps it, Esc cancels";

/// `:crop` on the photo's line: arms the drag (a rectangle already dragged is dropped).
/// VUGFACEPHOTOCROPKEYPREF (SR645): `:crop` on the photo's line, or the `r` key ([`key`], under [`line_free`]).
fn crop_arm(view: &mut View) -> bool {
    let Some(ph) = view.photo.as_mut() else { return false };
    let source = ph.crop.source.take();
    ph.crop = Crop { source, armed: true, ..Crop::default() };
    ph.note = Some(CROP_HINT.to_string());
    true
}

/// The photo pixel under surface point (x, y): the ray's hit on the paper plane (z = 0, as [`underlay`] samples it),
/// through `Photo.h`; None when the ray misses the plane.
fn px_at(view: &View, ph: &Photo, x: f64, y: f64) -> Option<(f64, f64)> {
    let (o, d) = view.ray_f64(x, y);
    if d[2].abs() < 1e-12 {
        return None;
    }
    let t = -o[2] / d[2];
    if !(t > 0.0) {
        return None;
    }
    let p = photo_core::homography::apply(&ph.h, Vec2::new(o[0] + d[0] * t, o[1] + d[1] * t))?;
    Some((p.x, p.y))
}

/// THE crop rectangle `[x, y, w, h]` in photo pixels: each corner clamped to the image and rounded to the nearest pixel
/// edge; None with no rectangle or an empty one. [`crop_mask`] paints it, [`crop_keep`] cuts it.
pub fn crop_rect(ph: &Photo) -> Option<[u32; 4]> {
    let (a, b) = (ph.crop.from?, ph.crop.to?);
    let (iw, ih) = (ph.image.width as f64, ph.image.height as f64);
    let edge = |v: f64, hi: f64| v.clamp(0.0, hi).round() as u32;
    let (x0, x1) = (edge(a.0.min(b.0), iw), edge(a.0.max(b.0), iw));
    let (y0, y1) = (edge(a.1.min(b.1), ih), edge(a.1.max(b.1), ih));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some([x0, y0, x1 - x0, y1 - y0])
}

/// [`press`] first while armed: a Left press on the photo starts the rectangle (both corners at the press); every press is
/// the crop's while it is armed. None when not armed.
fn crop_press(view: &mut View, x: f64, y: f64) -> Option<bool> {
    let ph = view.photo.as_ref()?;
    if !ph.crop.armed {
        return None;
    }
    let p = px_at(view, ph, x, y);
    let ph = view.photo.as_mut()?;
    let Some(p) = p else { return Some(false) };
    ph.crop.from = Some(p);
    ph.crop.to = Some(p);
    ph.crop.dragging = true;
    Some(true)
}

/// [`hover`] first: while a crop drag is held its corner follows the cursor (true: a frame); false otherwise.
fn crop_motion(view: &mut View, x: f64, y: f64) -> bool {
    let Some(ph) = view.photo.as_ref() else { return false };
    if !ph.crop.dragging {
        return false;
    }
    let p = px_at(view, ph, x, y);
    let Some(ph) = view.photo.as_mut() else { return false };
    if let Some(p) = p {
        ph.crop.to = Some(p);
    }
    true
}

/// The view's release, first (view.rs `release`): a held crop drag ends at (x, y) and the corner names the rectangle.
/// None (the release goes on) when no crop drag is held.
pub fn release(view: &mut View, x: f64, y: f64) -> Option<bool> {
    let ph = view.photo.as_ref()?;
    if !ph.crop.dragging {
        return None;
    }
    let p = px_at(view, ph, x, y);
    let ph = view.photo.as_mut()?;
    if let Some(p) = p {
        ph.crop.to = Some(p);
    }
    ph.crop.dragging = false;
    ph.note = Some(match crop_rect(ph) {
        Some([x0, y0, w, h]) => format!("crop {x0},{y0} {w}x{h} px: Return keeps it, Esc cancels"),
        None => CROP_HINT.to_string(),
    });
    Some(true)
}

/// [`key`] first while armed (under its gate: no pull or dimension edit): Return keeps the crop ([`crop_keep`]), Escape
/// cancels it; any other key goes on. None when not armed.
fn crop_key(view: &mut View, k: &str) -> Option<bool> {
    let ph = view.photo.as_ref()?;
    if !ph.crop.armed || view.design.pull.is_some() || view.design.face.as_ref().is_some_and(|f| f.edit.is_some()) {
        return None;
    }
    match crate::sketch::FieldKey::of(k) {
        Some(crate::sketch::FieldKey::Enter) => Some(crop_keep(view)),
        Some(crate::sketch::FieldKey::Escape) => {
            let ph = view.photo.as_mut()?;
            let source = ph.crop.source.take();
            ph.crop = Crop { source, ..Crop::default() };
            ph.note = Some("crop cancelled".to_string());
            Some(true)
        }
        _ => None,
    }
}

/// Return on an armed crop: refused by name (the corner, nothing written, still armed) with no rectangle, with the
/// document unsaved (the re-open replaces the face), with no file, or when `<stem>.crop.png` exists; else the crop is
/// written ([`crop_write`]) and queued for the controller's re-open ([`follow`]), the corner `saved <name>`.
fn crop_keep(view: &mut View) -> bool {
    let dirty = view.design.journal.dirty();
    let Some(ph) = view.photo.as_mut() else { return false };
    let out = match (crop_rect(ph), ph.crop.source.clone()) {
        (None, _) => Err("refused crop: drag a rectangle on the photo".to_string()),
        _ if dirty => Err("refused crop: the document is unsaved (s saves it)".to_string()),
        (Some(_), None) => Err("refused crop: the photo has no file (open it by its path)".to_string()),
        (Some(r), Some(src)) => crop_write(&ph.image, r, &src, ph.prefs.principal_px), // VUGFACEPHOTOCROPPREFFRAC (SR648): the pref the photo was measured about
    };
    match out {
        Ok(out) => {
            let source = ph.crop.source.take();
            ph.note = Some(format!("saved {}", file_name(&out)));
            ph.crop = Crop { source, reopen: Some(out), ..Crop::default() };
        }
        Err(e) => ph.note = Some(e),
    }
    true
}

/// The crop `[x, y, w, h]` of `img` as PNG beside `src`: `<stem>.crop.png` (`Path::with_extension`: a .jpg / .webp / .png
/// photo alike), refused when it exists; the cropped pixels carry photo_core's `crop_pair_with` as their one tEXt pair
/// (VUGFACEPHOTOCROPPREFFRAC SR648: under `principal_px`, the pref a meta-free photo was measured about, carried as the
/// pair's point so the re-open measures about it less the offset).
fn crop_write(img: &pixel_core::Image, [x, y, w, h]: [u32; 4], src: &std::path::Path, principal_px: Option<Vec2>) -> Result<std::path::PathBuf, String> {
    let out = src.with_extension("crop.png");
    if out.exists() {
        return Err(format!("refused crop: {} exists", file_name(&out)));
    }
    let c = photo_core::fixture::crop(img, x, y, w, h);
    let (key, value) = photo_core::crop_pair_with(img, x, y, principal_px); // VUGFACEPHOTOCROPPREFFRAC (SR648): the one writer, the pref's point carried
    let png = pixel_core::png::encode::encode_rgba_text(c.width, c.height, &c.rgba, &[(key.as_str(), value.as_str())]).map_err(|e| format!("refused crop: png {e:?}"))?;
    crate::write_atomic(&out, &png).map_err(|e| format!("refused crop: {e}"))?;
    Ok(out)
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string()
}

/// The sheet outside an armed crop's rectangle dims toward the background once more (the underlay's own [`DIM`]):
/// `raster::tint` even-odd over the sheet and the rectangle's quad (its photo corners through H⁻¹ onto the paper, then
/// the view), its edge in `ACTIVE`. Nothing with no rectangle or a corner off the view.
fn crop_mask(view: &View, ph: &Photo, fr: &mut Raster) {
    if !ph.crop.armed {
        return;
    }
    let Some([x, y, w, h]) = crop_rect(ph) else { return };
    let Ok(inv) = photo_core::homography::invert(&ph.h) else { return };
    let (x0, y0, x1, y1) = (x as f64, y as f64, (x + w) as f64, (y + h) as f64);
    let quad: Option<Vec<(f32, f32)>> = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        .iter()
        .map(|&(u, v)| {
            let q = photo_core::homography::apply(&inv, Vec2::new(u, v))?;
            view.project_f64([q.x, q.y, 0.0]).map(|(sx, sy)| (sx as f32, sy as f32))
        })
        .collect();
    let Some(quad) = quad else { return };
    let (fw, fh) = (fr.width as f32, fr.height as f32);
    let sheet = vec![(0.0, 0.0), (fw, 0.0), (fw, fh), (0.0, fh)];
    raster::tint(fr, &[sheet, quad.clone()], rgba(raster::BACKGROUND), 1.0 - DIM);
    for i in 0..4 {
        raster::line(fr, quad[i], quad[(i + 1) % 4], rgba(ACTIVE));
    }
}

/// The controller's re-open of a saved crop (lib.rs `Viewer::key`'s last line, as `deviation::follow`): the saved file
/// through THE open road `Viewer::open_with` (read back from disk, measured again under the photo's prefs), the `:` line
/// carried over, the corner `saved <name>`; a saved crop that does not measure keeps the face and names why.
pub fn follow(v: &mut crate::Viewer) {
    let Some(path) = v.view.photo.as_mut().and_then(|ph| ph.crop.reopen.take()) else { return };
    let prefs = v.view.photo.as_ref().map(|ph| ph.prefs).unwrap_or_default();
    let name = file_name(&path);
    match crate::Viewer::open_with(&path.to_string_lossy(), v.view.size, v.view.preset, prefs) {
        Ok(mut next) => {
            next.view.entry = std::mem::take(&mut v.view.entry);
            if let Some(ph) = next.view.photo.as_mut() {
                ph.note = Some(format!("saved {name}"));
            }
            *v = next;
        }
        Err(e) => {
            if let Some(ph) = v.view.photo.as_mut() {
                ph.note = Some(format!("refused the saved crop {e} (saved {name})"));
            }
        }
    }
}
