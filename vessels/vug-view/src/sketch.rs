// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D), a host face — the sketch face (VUGFACE2, LEDGER SR485): input to sketch_core
//! entities and constraints on a plane, the inference, the glyphs; holds no geometry (the Sketch is sketch_core's).
//!
//! "A sketch should feel like drawing" (docs/dev/USERLAND/VUG.md §3.2, §4.1 item 7). A sketch opens on the planar
//! body face under the cursor, else on the world plane (XY / XZ / YZ) most facing the camera, with a fixed origin
//! point to snap to. One key per tool — `l` line, `c` circle, `a` arc, `p` point (inside a sketch; outside, `p` stays
//! the next preset), Esc back to select — and the cursor is the tool: a bare left press places, a drag moves the
//! entity live through `sketch_core::drag`, a release applies what was inferred on hover (a point within 8 px:
//! Coincident; a line's midpoint: Midpoint; a line within 3° of the plane's x / y: Horizontal / Vertical; a line
//! touching a circle within 8 px: TangentLineCircle; a circle's rim on a point: PointOnCircle) as sketch_core
//! Constraints, then solves. A refused solve removes exactly the constraints just inferred and names the refusal; a
//! redundant one is removed (inference never silently over-defines). Every solve re-reads `dof` and `profiles`; the
//! profiles are tinted, the constraints drawn as glyphs, the dof in the corner. `e` pulls the profile under the cursor
//! along the plane normal (extrude.rs), a click commits it.
//!
//! VUGFACEDIMENSION (LEDGER SR494): `d` is the dimension tool — a line gives its length, two points their distance, a
//! point and a line their distance, two lines their angle, a circle its diameter and an arc its radius (`d` again
//! toggles, the SolidWorks / Onshape defaults); the dimension is placed at the cursor and editable at once: the typed
//! value becomes the sketch_core Constraint on Enter, a refusal rolls back exactly that one and shows its name at the
//! dimension. A pull takes a typed distance the same way (type-to-override, Enter commits).
//!
//! The plane hit: the view's camera widened to f64 (`View::ray_f64`), geom_core's `line_plane`, the frame's local
//! coordinates; `to_screen` is the exact inverse (`View::project_f64`).

use crate::extrude::{self, Body, Pull};
use crate::model::Model;
use crate::raster::{self, Frame as Raster, rgba};
use crate::view::{Action, Button, CLICK_PX, View};
use brep_core::Frame;
use sketch_core::{Constraint, ConstraintId, Entity, EntityId, Loop, PointRef, Profile, Sketch, Tol, Vec2};
use std::f64::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

/// The snap radius, surface pixels.
pub const SNAP_PX: f64 = 8.0;
/// A line within this many degrees of the plane's x (y) axis is inferred Horizontal (Vertical).
pub const HV_DEG: f64 = 3.0;

/// Sketch ink: `text.primary`, dark; the selection and the live gesture in the accent; the profile tint moss; glyphs
/// in the warning ochre; the dof label red while a refusal stands.
pub const INK: u32 = palette_core::TEXT_PRIMARY.1;
pub const ACTIVE: u32 = palette_core::ACCENTS[0];
pub const TINT: u32 = palette_core::ACCENTS[2];
pub const GLYPH: u32 = palette_core::ACCENTS[3];
pub const REFUSED: u32 = palette_core::STATUS_CRITICAL;

/// The tools; one key each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Line,
    Circle,
    Arc,
    Point,
    // VUGFACEDIMENSION (SR494): the dimension tool (`d`).
    Dimension,
}

/// What a cursor snapped to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Snap {
    Point(EntityId),
    Midpoint(EntityId),
}

/// A constraint mark drawn in the raster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Horizontal,
    Vertical,
    Equal,
    Perpendicular,
    Tangent,
    Midpoint,
    Coincident,
}

impl Mark {
    /// The glyph raster's bitmap font draws for the mark.
    pub fn ch(self) -> char {
        match self {
            Mark::Horizontal => 'H',
            Mark::Vertical => 'V',
            Mark::Equal => '=',
            Mark::Perpendicular => '⟂',
            Mark::Tangent => 'T',
            Mark::Midpoint => 'M',
            Mark::Coincident => 'o',
        }
    }
}

/// A mark at a surface position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub at: (f32, f32),
    pub mark: Mark,
}

/// The cursor on the plane and what would be inferred there (shown before the release applies it).
#[derive(Clone, Debug, PartialEq)]
pub struct Hover {
    pub at: Vec2,
    pub screen: (f64, f64),
    pub snap: Option<Snap>,
    pub marks: Vec<Mark>,
}

/// A press in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    /// A placing tool's press before it moved `CLICK_PX`: nothing is created yet (lazy: a bare click creates nothing).
    Armed { tool: Tool, start: Vec2, snap: Option<Snap>, screen: (f64, f64) },
    /// A line from `a` (placed) to `b` (dragged).
    Line { a: EntityId, b: EntityId, line: EntityId, snap_a: Option<Snap> },
    /// VUGSKETCHERASE (SR495): a circle's rim being dragged; the circle exists and grows live through
    /// `Sketch::set_radius` (its constraints are inferred on release).
    Circle { centre: EntityId, circle: EntityId, snap_c: Option<Snap> },
    /// An arc's start being dragged (radius and start angle).
    ArcStart { arc: EntityId, centre: EntityId, snap_c: Option<Snap> },
    /// An arc's end following the cursor on its circle until a click.
    ArcEnd { arc: EntityId },
    /// A placed point being dragged.
    Point { p: EntityId },
    /// Select: a sketch point dragged.
    Move { p: EntityId },
}

/// One sketch on one plane.
#[derive(Clone, Debug)]
pub struct SketchFace {
    pub sketch: Sketch,
    pub plane: Frame,
    pub tool: Tool,
    pub hover: Option<Hover>,
    pub selection: Option<EntityId>,
    pub gesture: Option<Gesture>,
    /// sketch_core's `profiles` after the last solve.
    pub profiles: Vec<Profile>,
    /// sketch_core's `dof` count after the last solve.
    pub dof: usize,
    /// The fixed origin point.
    pub origin: EntityId,
    /// One past the highest constraint id this face has added (the glyph walk's bound).
    pub(crate) issued: u32, // VUGFACETREE (SR513): the journal walks ConstraintId 0..issued
    // VUGFACEDIMENSION (SR494): the placed dimensions, the one being edited, a first point waiting for its second.
    pub dims: Vec<Dimension>,
    pub edit: Option<DimEdit>,
    pub pick: Option<PointRef>,
}

/// The design the view carries: the open sketch, the body, the pull in progress, the last refusal (named).
#[derive(Clone, Debug, Default)]
pub struct Design {
    pub face: Option<SketchFace>,
    pub body: Option<Body>,
    pub pull: Option<Pull>,
    pub refusal: Option<String>,
    // VUGFACEDIMENSION (SR494): the pull's typed distance (empty: the cursor drives it).
    pub pull_text: String,
    // VUGFACETREE (SR513): the document, its snapshots and the tree panel (journal.rs).
    pub journal: crate::journal::Journal,
}

/// The three world planes as right-handed sketch frames: XY (normal +Z), XZ (the front plane, normal -Y), YZ (the
/// right plane, normal +X).
pub fn world_planes() -> [(&'static str, Frame); 3] {
    let v = |x: f64, y: f64, z: f64| geom_core::Vec3::new(x, y, z);
    let o = v(0.0, 0.0, 0.0);
    [
        ("XY", Frame { o, x: v(1.0, 0.0, 0.0), y: v(0.0, 1.0, 0.0), z: v(0.0, 0.0, 1.0) }),
        ("XZ", Frame { o, x: v(1.0, 0.0, 0.0), y: v(0.0, 0.0, 1.0), z: v(0.0, -1.0, 0.0) }),
        ("YZ", Frame { o, x: v(0.0, 1.0, 0.0), y: v(0.0, 0.0, 1.0), z: v(1.0, 0.0, 0.0) }),
    ]
}

fn sub(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new(a.x - b.x, a.y - b.y)
}

fn len(a: Vec2) -> f64 {
    a.x.hypot(a.y)
}

fn snap_constraint(p: EntityId, s: Snap) -> Constraint {
    match s {
        Snap::Point(q) => Constraint::Coincident(PointRef::Point(p), PointRef::Point(q)),
        Snap::Midpoint(l) => Constraint::Midpoint(PointRef::Point(p), l),
    }
}

fn snap_mark(s: Snap) -> Mark {
    match s {
        Snap::Point(_) => Mark::Coincident,
        Snap::Midpoint(_) => Mark::Midpoint,
    }
}

/// A loop as a polygon in sketch coordinates (an arc sampled at 64 per turn).
fn polygon(l: &Loop) -> Vec<Vec2> {
    let mut out = Vec::new();
    for e in &l.edges {
        match e {
            sketch_core::Edge::Line { from, .. } => out.push(*from),
            sketch_core::Edge::Arc { arc, .. } => {
                let k = ((arc.sweep.abs() / TAU) * 64.0).ceil().max(1.0) as usize;
                for i in 0..k {
                    let t = arc.start + arc.sweep * i as f64 / k as f64;
                    out.push(Vec2::new(arc.centre.x + arc.radius * t.cos(), arc.centre.y + arc.radius * t.sin()));
                }
            }
        }
    }
    out
}

fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let n = poly.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            c = !c;
        }
    }
    c
}

/// The area centroid of a polygon (its vertex mean when the area vanishes).
fn centroid(poly: &[Vec2]) -> Vec2 {
    let n = poly.len();
    let (mut a, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let w = p.x * q.y - q.x * p.y;
        a += w;
        cx += (p.x + q.x) * w;
        cy += (p.y + q.y) * w;
    }
    if a.abs() > 1e-300 {
        Vec2::new(cx / (3.0 * a), cy / (3.0 * a))
    } else {
        let k = n.max(1) as f64;
        Vec2::new(poly.iter().map(|p| p.x).sum::<f64>() / k, poly.iter().map(|p| p.y).sum::<f64>() / k)
    }
}

impl SketchFace {
    /// A sketch on `plane` with its origin point fixed at (0, 0).
    pub fn new(plane: Frame) -> SketchFace {
        let mut sketch = Sketch::new();
        let origin = sketch.add_point(0.0, 0.0);
        let fix = sketch.add(Constraint::Fix(PointRef::Point(origin), Vec2::new(0.0, 0.0)));
        let issued = fix.map(|id| id.0 + 1).unwrap_or(0);
        let mut f = SketchFace {
            sketch,
            plane,
            tool: Tool::Select,
            hover: None,
            selection: None,
            gesture: None,
            profiles: Vec::new(),
            dof: 0,
            origin,
            issued,
            // VUGFACEDIMENSION (SR494)
            dims: Vec::new(),
            edit: None,
            pick: None,
        };
        f.settle();
        f
    }

    /// Re-read sketch_core's dof and profiles (after every solve or drag).
    pub fn settle(&mut self) {
        self.dof = sketch_core::dof(&self.sketch).count;
        self.profiles = sketch_core::profiles(&self.sketch, Tol::DEFAULT);
    }

    /// sketch_core's `solve`, then `settle` (a script that adds a constraint by hand calls this).
    pub fn solve(&mut self) -> Result<sketch_core::Solved, sketch_core::Refusal> {
        let r = sketch_core::solve(&mut self.sketch, Tol::DEFAULT);
        self.settle();
        r
    }

    /// Add the inferred constraints and solve: a refusal removes exactly these again (the geometry stays as drawn)
    /// and is returned by name; a redundant one among them is removed.
    fn apply(&mut self, cs: Vec<Constraint>) -> Result<(), String> {
        self.apply_as(cs, false).map(|_| ())
    }

    /// VUGFACEDIMENSION (SR494): `apply`'s one road, returning the ids kept. With `refuse_redundant` (a typed
    /// dimension) a redundant one among the added is a refusal like a conflict: exactly the added ids are removed
    /// again and `Redundant([..])` is returned by name; without it (inference) the redundant ones are dropped.
    fn apply_as(&mut self, cs: Vec<Constraint>, refuse_redundant: bool) -> Result<Vec<ConstraintId>, String> {
        let mut ids: Vec<ConstraintId> = Vec::new();
        for c in cs {
            if let Ok(id) = self.sketch.add(c) {
                self.issued = self.issued.max(id.0 + 1);
                ids.push(id);
            }
        }
        let solved = match sketch_core::solve(&mut self.sketch, Tol::DEFAULT) {
            Ok(s) => {
                let red: Vec<ConstraintId> = s.redundant.iter().copied().filter(|id| ids.contains(id)).collect();
                if refuse_redundant && !red.is_empty() { Err(sketch_core::Refusal::Redundant(red)) } else { Ok(red) }
            }
            Err(e) => Err(e),
        };
        let out = match solved {
            Ok(red) => {
                for id in &red {
                    self.sketch.remove(*id);
                }
                Ok(ids.into_iter().filter(|id| !red.contains(id)).collect())
            }
            Err(e) => {
                for id in ids.iter().rev() {
                    self.sketch.remove(*id);
                }
                let _ = sketch_core::solve(&mut self.sketch, Tol::DEFAULT);
                Err(format!("{e:?}"))
            }
        };
        self.settle();
        out
    }

    /// sketch_core's `drag`; false (and nothing moved) when it refuses.
    fn drag_to(&mut self, p: PointRef, to: Vec2) -> bool {
        let ok = sketch_core::drag(&mut self.sketch, p, to, Tol::DEFAULT).is_ok();
        if ok {
            self.settle();
        }
        ok
    }

    fn world(&self, q: Vec2) -> [f64; 3] {
        let w = self.plane.at(q.x, q.y, 0.0);
        [w.x, w.y, w.z]
    }

    /// The sketch point under surface point (x, y): the f64 cursor ray on the plane (`None` parallel, in-plane or
    /// behind the eye).
    pub fn unproject(&self, view: &View, x: f64, y: f64) -> Option<Vec2> {
        let (o, d) = view.ray_f64(x, y);
        let g = |a: [f64; 3]| geom_core::Vec3::new(a[0], a[1], a[2]);
        let line = geom_core::Line::new(g(o), g(d)).ok()?;
        let plane = geom_core::Plane::from_point_normal(self.plane.o, self.plane.z).ok()?;
        match geom_core::line_plane(&line, &plane, geom_core::Tol::DEFAULT) {
            geom_core::LinePlane::Point { p, t } if t > 0.0 => {
                let (a, b, _) = self.plane.local(p);
                Some(Vec2::new(a, b))
            }
            _ => None,
        }
    }

    /// Where a sketch point lands on the surface (the exact inverse of `unproject`).
    pub fn to_screen(&self, view: &View, q: Vec2) -> Option<(f64, f64)> {
        view.project_f64(self.world(q))
    }

    fn px_dist(&self, view: &View, q: Vec2, x: f64, y: f64) -> Option<f64> {
        self.to_screen(view, q).map(|(sx, sy)| (sx - x).hypot(sy - y))
    }

    /// Millimetres per surface pixel on the plane at (x, y) (1 when the plane is not hit there).
    fn mm_per_px(&self, view: &View, x: f64, y: f64) -> f64 {
        match (self.unproject(view, x, y), self.unproject(view, x + 1.0, y)) {
            (Some(a), Some(b)) if len(sub(b, a)) > 0.0 => len(sub(b, a)),
            _ => 1.0,
        }
    }

    /// The nearest sketch point within `SNAP_PX` of (x, y) (ties: the lower id), else (with `midpoints`) the nearest
    /// line midpoint; entities in `skip` (and lines with an end in it) are not candidates.
    pub fn snap_at(&self, view: &View, x: f64, y: f64, skip: &[EntityId], midpoints: bool) -> Option<(Snap, Vec2)> {
        let mut best: Option<(f64, Snap, Vec2)> = None;
        for pass in 0..2 {
            if pass == 1 && (best.is_some() || !midpoints) {
                break;
            }
            for (i, e) in self.sketch.entities().iter().enumerate() {
                let id = EntityId(i as u32);
                if skip.contains(&id) {
                    continue;
                }
                let cand = match (pass, *e) {
                    (0, Entity::Point { .. }) => self.sketch.point(PointRef::Point(id)).map(|p| (Snap::Point(id), p)),
                    (1, Entity::Line { a, b }) if !skip.contains(&a) && !skip.contains(&b) => {
                        match (self.sketch.point(PointRef::Point(a)), self.sketch.point(PointRef::Point(b))) {
                            (Some(p), Some(q)) => Some((Snap::Midpoint(id), Vec2::new(0.5 * (p.x + q.x), 0.5 * (p.y + q.y)))),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let Some((s, p)) = cand else { continue };
                let Some(d) = self.px_dist(view, p, x, y) else { continue };
                if d <= SNAP_PX && best.map_or(true, |(d0, _, _)| d < d0) {
                    best = Some((d, s, p));
                }
            }
        }
        best.map(|(_, s, p)| (s, p))
    }

    /// What a line from `a` to `b` infers by its direction and its contact with the circles: Horizontal / Vertical
    /// within `HV_DEG`, and the first circle or arc it touches within `SNAP_PX` (TangentLineCircle).
    fn line_marks(&self, view: &View, x: f64, y: f64, line: EntityId, a: EntityId, b: EntityId) -> Vec<(Mark, Constraint)> {
        let mut out = Vec::new();
        let (Some(p), Some(q)) = (self.sketch.point(PointRef::Point(a)), self.sketch.point(PointRef::Point(b))) else { return out };
        let d = sub(q, p);
        let l = len(d);
        if !(l > Tol::DEFAULT.lin) {
            return out;
        }
        let ang = d.y.abs().atan2(d.x.abs()).to_degrees();
        if ang <= HV_DEG {
            out.push((Mark::Horizontal, Constraint::Horizontal(line)));
        } else if ang >= 90.0 - HV_DEG {
            out.push((Mark::Vertical, Constraint::Vertical(line)));
        }
        let reach = SNAP_PX * self.mm_per_px(view, x, y);
        for (i, e) in self.sketch.entities().iter().enumerate() {
            let id = EntityId(i as u32);
            if !matches!(e, Entity::Circle { .. } | Entity::Arc { .. }) {
                continue;
            }
            let (Some(c), Some(r)) = (self.sketch.point(PointRef::Centre(id)), self.sketch.radius(id)) else { continue };
            let dist = (d.x * (c.y - p.y) - d.y * (c.x - p.x)).abs() / l;
            if (dist - r).abs() <= reach {
                out.push((Mark::Tangent, Constraint::TangentLineCircle(line, id)));
                break;
            }
        }
        out
    }

    /// The glyphs: each constraint's mark at its entity, then the hover's candidates beside the cursor.
    pub fn glyphs(&self, view: &View) -> Vec<Glyph> {
        let mut out = Vec::new();
        let scr = |q: Option<Vec2>| q.and_then(|q| self.to_screen(view, q)).map(|(x, y)| (x as f32, y as f32));
        let mid = |l: EntityId| match self.sketch.entity(l) {
            Some(Entity::Line { a, b }) => match (self.sketch.point(PointRef::Point(*a)), self.sketch.point(PointRef::Point(*b))) {
                (Some(p), Some(q)) => Some(Vec2::new(0.5 * (p.x + q.x), 0.5 * (p.y + q.y))),
                _ => None,
            },
            _ => None,
        };
        for i in 0..self.issued {
            let Some(c) = self.sketch.constraint(ConstraintId(i)) else { continue };
            let (at, mark) = match *c {
                Constraint::Horizontal(l) => (scr(mid(l)), Mark::Horizontal),
                Constraint::Vertical(l) => (scr(mid(l)), Mark::Vertical),
                Constraint::EqualLength(l, _) => (scr(mid(l)), Mark::Equal),
                Constraint::EqualRadius(c, _) => (scr(self.sketch.point(PointRef::Centre(c))), Mark::Equal),
                Constraint::Perpendicular(l, _) => (scr(mid(l)), Mark::Perpendicular),
                Constraint::TangentLineCircle(l, _) => (scr(mid(l)).map(|(x, y)| (x, y + 10.0)), Mark::Tangent),
                Constraint::Midpoint(p, _) => (scr(self.sketch.point(p)), Mark::Midpoint),
                Constraint::Coincident(p, _) | Constraint::PointOnCircle(p, _) => (scr(self.sketch.point(p)), Mark::Coincident),
                _ => continue,
            };
            if let Some((x, y)) = at {
                out.push(Glyph { at: (x + 5.0, y - 14.0), mark });
            }
        }
        if let Some(h) = &self.hover {
            for (k, m) in h.marks.iter().enumerate() {
                out.push(Glyph { at: (h.screen.0 as f32 + 12.0 + 9.0 * k as f32, h.screen.1 as f32 + 8.0), mark: *m });
            }
        }
        out
    }

    /// The hover at (x, y) for a gesture-free placing tool: the snap and its mark.
    fn hover_at(&self, view: &View, x: f64, y: f64, q: Vec2) -> Hover {
        let snap = self.snap_at(view, x, y, &[], true);
        Hover { at: snap.map_or(q, |(_, p)| p), screen: (x, y), snap: snap.map(|(s, _)| s), marks: snap.map(|(s, _)| snap_mark(s)).into_iter().collect() }
    }
}

impl Design {
    /// The model the view draws and picks in place of the opened one: the pull's preview, else the body.
    pub fn shown(&self) -> Option<Arc<Model>> {
        self.pull.as_ref().and_then(|p| p.preview.as_ref()).or(self.body.as_ref()).map(|b| b.model.clone())
    }

    fn note(&mut self, r: Result<(), String>) {
        match r {
            Ok(()) => self.refusal = None,
            Err(e) => {
                log::warn!("vug-view: sketch solve refused: {e}");
                self.refusal = Some(e);
            }
        }
    }

    /// The plane a new sketch opens on: the planar body face under the cursor, else the world plane most facing the
    /// camera. `model` is the shown model (the body's when there is one), so a pick's triangle indexes `face_of`.
    fn plane_at(&self, view: &View, model: &Model) -> Frame {
        if let Some(body) = &self.body {
            let (x, y) = view.cursor;
            if let Some(f) = view.hit(model, x, y).and_then(|(_, tri)| body.face_frame(tri)) {
                return f;
            }
        }
        let f = view.camera.basis().2;
        let fw = [f.x as f64, f.y as f64, f.z as f64];
        let face = |fr: &Frame| (fw[0] * fr.z.x + fw[1] * fr.z.y + fw[2] * fr.z.z).abs();
        let planes = world_planes();
        let mut best = planes[0].1;
        for (_, p) in planes.iter().skip(1) {
            if face(p) > face(&best) {
                best = *p;
            }
        }
        best
    }

    fn key(&mut self, view: &View, model: &Model, k: &str) -> Option<bool> {
        if let Some(r) = self.typed_key(k) { return Some(r); } // VUGFACEDIMENSION (SR494): an open edit / pull takes its keys first
        let tool = match k {
            "Escape" | "\u{1b}" => {
                if self.pull.take().is_some() {
                    return Some(true);
                }
                let face = self.face.as_mut()?;
                if face.tool != Tool::Select || face.gesture.is_some() {
                    face.tool = Tool::Select;
                    face.gesture = None;
                    face.hover = None;
                } else {
                    face.selection = None;
                }
                return Some(true);
            }
            "e" | "E" => return self.start_pull(view),
            // VUGSKETCHERASE (SR495): Delete / Backspace removes the entity under the cursor.
            "BackSpace" | "Backspace" | "Delete" => return self.erase_at(view),
            "l" | "L" => Tool::Line,
            "c" | "C" => Tool::Circle,
            "a" | "A" => Tool::Arc,
            // Outside a sketch `p` stays the next preset (the view's key).
            "p" | "P" if self.face.is_some() => Tool::Point,
            "d" | "D" if self.face.is_some() => Tool::Dimension, // VUGFACEDIMENSION (SR494)
            _ => return None,
        };
        if self.face.is_none() {
            self.face = Some(SketchFace::new(self.plane_at(view, model)));
        }
        let face = self.face.as_mut()?;
        face.tool = tool;
        face.gesture = None;
        face.hover = None;
        face.pick = None; // VUGFACEDIMENSION (SR494)
        Some(true)
    }

    /// `e`: pull the profile under the cursor (else the largest) along the plane's normal.
    fn start_pull(&mut self, view: &View) -> Option<bool> {
        let face = self.face.as_ref()?;
        if face.profiles.is_empty() {
            return Some(false);
        }
        let (x, y) = view.cursor;
        let q = face.unproject(view, x, y);
        let holds = |p: &Profile| q.is_some_and(|q| inside(&polygon(&p.outer), q) && !p.holes.iter().any(|h| inside(&polygon(h), q)));
        let pick = face.profiles.iter().position(holds).unwrap_or_else(|| {
            let mut best = 0;
            for (i, p) in face.profiles.iter().enumerate() {
                if p.outer.area.abs() > face.profiles[best].outer.area.abs() {
                    best = i;
                }
            }
            best
        });
        let profile = face.profiles[pick].clone();
        let c = centroid(&polygon(&profile.outer));
        let centroid = face.world(c);
        self.pull = Some(Pull { profile, plane: face.plane, centroid, distance: 0.0, start: (x, y), preview: None });
        self.pull_text.clear(); // VUGFACEDIMENSION (SR494)
        Some(true)
    }

    /// VUGSKETCHERASE (SR495): remove the entity under the cursor through `Sketch::remove_entity` (the cascade takes
    /// what is built on it and every constraint naming it), then solve. A point is picked first, else a line by its
    /// midpoint (a circle or arc goes with its centre); the origin is the sketch's anchor and stays; nothing happens
    /// while a gesture is in progress.
    fn erase_at(&mut self, view: &View) -> Option<bool> {
        let face = self.face.as_mut()?;
        if face.gesture.is_some() {
            return Some(false);
        }
        let (x, y) = view.cursor;
        let id = match face.snap_at(view, x, y, &[], true) {
            Some((Snap::Point(p), _)) => p,
            Some((Snap::Midpoint(l), _)) => l,
            None => return Some(false),
        };
        if id == face.origin {
            return Some(false);
        }
        let Ok(gone) = face.sketch.remove_entity(id) else { return Some(false) };
        if face.selection.is_some_and(|s| gone.entities.contains(&s)) {
            face.selection = None;
        }
        face.hover = None;
        let r = face.solve().map(|_| ()).map_err(|e| format!("{e:?}"));
        self.note(r);
        self.journal.record(self.face.as_ref(), None); // VUGFACETREE (SR513): an erase commits
        Some(true)
    }

    /// The pull follows the cursor: the distance along the normal line through the centroid at the cursor ray's
    /// closest approach (screen dy times mm/px when the view looks along the normal), the preview rebuilt.
    fn pull_motion(&mut self, view: &View, x: f64, y: f64) -> bool {
        // VUGFACEDIMENSION (SR494): a typed distance overrides the cursor.
        if !self.pull_text.is_empty() {
            return false;
        }
        let face_mm = self.face.as_ref().map(|f| f.mm_per_px(view, x, y));
        let Some(pull) = self.pull.as_mut() else { return false };
        let (o, d) = view.ray_f64(x, y);
        let dl = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let d = d.map(|v| v / dl);
        let n = [pull.plane.z.x, pull.plane.z.y, pull.plane.z.z];
        let w = [o[0] - pull.centroid[0], o[1] - pull.centroid[1], o[2] - pull.centroid[2]];
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let (b, dd, e) = (dot(d, n), dot(d, w), dot(n, w));
        let denom = 1.0 - b * b;
        pull.distance = if denom > 1e-3 { (e - b * dd) / denom } else { -(y - pull.start.1) * face_mm.unwrap_or(1.0) };
        self.pull_preview();
        true
    }

    fn press(&mut self, view: &View, button: Button, mods: u8, x: f64, y: f64) -> Option<bool> {
        if view.preset.resolve(button, mods) != Action::Pick {
            return None;
        }
        if self.pull.is_some() {
            // A click commits the pull (a pull that never left the plane is dropped).
            self.commit_pull();
            return Some(true);
        }
        let face = self.face.as_mut()?;
        let q = face.unproject(view, x, y)?;
        // VUGFACEDIMENSION (SR494): a click on a dimension's value re-opens it (the dimension and select tools).
        if matches!(face.tool, Tool::Select | Tool::Dimension) && face.reopen_at(view, x, y) {
            return Some(true);
        }
        match face.tool {
            Tool::Dimension => Some(face.dim_press(view, x, y, q)), // VUGFACEDIMENSION (SR494)
            Tool::Select => match face.snap_at(view, x, y, &[], false) {
                Some((Snap::Point(id), _)) => {
                    face.selection = Some(id);
                    face.gesture = Some(Gesture::Move { p: id });
                    Some(true)
                }
                _ => {
                    face.selection = None;
                    None
                }
            },
            Tool::Arc if matches!(face.gesture, Some(Gesture::ArcEnd { .. })) => {
                face.gesture = None;
                face.hover = None;
                self.journal.record(self.face.as_ref(), None); // VUGFACETREE (SR513): the arc's end click commits
                Some(true)
            }
            Tool::Point => {
                let at = face.snap_at(view, x, y, &[], true).map_or(q, |(_, p)| p);
                let p = face.sketch.add_point(at.x, at.y);
                face.gesture = Some(Gesture::Point { p });
                face.settle();
                Some(true)
            }
            tool => {
                let snap = face.snap_at(view, x, y, &[], true);
                let start = snap.map_or(q, |(_, p)| p);
                face.gesture = Some(Gesture::Armed { tool, start, snap: snap.map(|(s, _)| s), screen: (x, y) });
                Some(true)
            }
        }
    }

    fn motion(&mut self, view: &View, x: f64, y: f64) -> Option<bool> {
        if self.pull.is_some() {
            return Some(self.pull_motion(view, x, y));
        }
        let face = self.face.as_mut()?;
        let q = face.unproject(view, x, y);
        let Some(g) = face.gesture else {
            if face.tool == Tool::Select {
                return None;
            }
            let h = q.map(|q| face.hover_at(view, x, y, q));
            let changed = h != face.hover;
            face.hover = h;
            return Some(changed);
        };
        let Some(q) = q else { return Some(false) };
        let g = match g {
            Gesture::Armed { tool, start, snap, screen } => {
                if (x - screen.0).hypot(y - screen.1) < CLICK_PX {
                    return Some(false);
                }
                // The first real motion creates the entity (lazy: a bare click created nothing).
                let created = match tool {
                    Tool::Line => {
                        let a = face.sketch.add_point(start.x, start.y);
                        let b = face.sketch.add_point(start.x, start.y);
                        face.sketch.add_line(a, b).ok().map(|line| Gesture::Line { a, b, line, snap_a: snap })
                    }
                    Tool::Circle => {
                        let c = face.sketch.add_point(start.x, start.y);
                        face.sketch.add_circle(c, len(sub(q, start)).max(Tol::DEFAULT.lin * 10.0)).ok().map(|circle| Gesture::Circle { centre: c, circle, snap_c: snap })
                    }
                    Tool::Arc => {
                        let c = face.sketch.add_point(start.x, start.y);
                        let d = sub(q, start);
                        let ang = d.y.atan2(d.x);
                        face.sketch.add_arc(c, len(d).max(Tol::DEFAULT.lin * 10.0), ang, ang + FRAC_PI_2).ok().map(|arc| Gesture::ArcStart { arc, centre: c, snap_c: snap })
                    }
                    _ => None,
                };
                face.gesture = created;
                match created {
                    Some(g) => g,
                    None => return Some(false),
                }
            }
            g => g,
        };
        match g {
            Gesture::Line { a, b, line, snap_a } => {
                let snap = face.snap_at(view, x, y, &[a, b], true);
                face.drag_to(PointRef::Point(b), snap.map_or(q, |(_, p)| p));
                let mut marks: Vec<Mark> = snap_a.into_iter().map(snap_mark).collect();
                marks.extend(snap.map(|(s, _)| snap_mark(s)));
                marks.extend(face.line_marks(view, x, y, line, a, b).into_iter().map(|(m, _)| m));
                face.hover = Some(Hover { at: q, screen: (x, y), snap: snap.map(|(s, _)| s), marks });
            }
            Gesture::Circle { centre, circle, .. } => {
                // VUGSKETCHERASE (SR495): the radius follows the rim live; no solve (nothing constrains it yet).
                let rim = face.snap_at(view, x, y, &[centre], false).map_or(q, |(_, p)| p);
                if let Some(c) = face.sketch.point(PointRef::Point(centre)) {
                    let _ = face.sketch.set_radius(circle, len(sub(rim, c)).max(Tol::DEFAULT.lin * 10.0));
                    face.settle();
                }
            }
            Gesture::ArcStart { arc, .. } => {
                face.drag_to(PointRef::Start(arc), q);
            }
            Gesture::ArcEnd { arc } => {
                // The end follows the cursor on the arc's circle (projected onto r, so r holds).
                if let (Some(c), Some(r)) = (face.sketch.point(PointRef::Centre(arc)), face.sketch.radius(arc)) {
                    let d = sub(q, c);
                    let l = len(d);
                    if l > 0.0 {
                        face.drag_to(PointRef::End(arc), Vec2::new(c.x + d.x / l * r, c.y + d.y / l * r));
                    }
                }
            }
            Gesture::Point { p } => {
                let at = face.snap_at(view, x, y, &[p], true).map_or(q, |(_, s)| s);
                face.drag_to(PointRef::Point(p), at);
            }
            Gesture::Move { p } => {
                face.drag_to(PointRef::Point(p), q);
            }
            Gesture::Armed { .. } => {}
        }
        Some(true)
    }

    fn release(&mut self, view: &View, x: f64, y: f64) -> Option<bool> {
        if self.pull.is_some() {
            return Some(false);
        }
        let face = self.face.as_mut()?;
        let g = face.gesture?;
        let r = match g {
            Gesture::Armed { .. } => {
                face.gesture = None;
                return Some(false);
            }
            Gesture::Line { a, b, line, snap_a } => {
                let mut cs: Vec<Constraint> = snap_a.map(|s| snap_constraint(a, s)).into_iter().collect();
                if let Some((s, p)) = face.snap_at(view, x, y, &[a, b], true) {
                    face.drag_to(PointRef::Point(b), p);
                    cs.push(snap_constraint(b, s));
                }
                cs.extend(face.line_marks(view, x, y, line, a, b).into_iter().map(|(_, c)| c));
                face.gesture = None;
                face.apply(cs)
            }
            Gesture::Circle { centre, circle, snap_c } => {
                // VUGSKETCHERASE (SR495): the circle already exists; the release infers its constraints.
                face.gesture = None;
                let mut cs: Vec<Constraint> = snap_c.map(|s| snap_constraint(centre, s)).into_iter().collect();
                if let Some((Snap::Point(p), _)) = face.snap_at(view, x, y, &[centre], false) {
                    cs.push(Constraint::PointOnCircle(PointRef::Point(p), circle));
                }
                face.apply(cs)
            }
            Gesture::ArcStart { arc, centre, snap_c } => {
                face.gesture = Some(Gesture::ArcEnd { arc });
                face.apply(snap_c.map(|s| snap_constraint(centre, s)).into_iter().collect())
            }
            Gesture::ArcEnd { .. } => return Some(false),
            Gesture::Point { p } => {
                let cs: Vec<Constraint> = face.snap_at(view, x, y, &[p], true).map(|(s, _)| snap_constraint(p, s)).into_iter().collect();
                face.gesture = None;
                face.apply(cs)
            }
            Gesture::Move { .. } => {
                face.gesture = None;
                face.settle();
                Ok(())
            }
        };
        face.hover = None;
        self.note(r);
        self.journal.record(self.face.as_ref(), None); // VUGFACETREE (SR513): a released gesture commits
        Some(true)
    }
}

/// Run `f` on the view's design with the view readable (the design is taken out for the call and put back).
fn with_design<R>(view: &mut View, f: impl FnOnce(&mut Design, &View) -> R) -> R {
    let mut d = std::mem::take(&mut view.design);
    let r = f(&mut d, view);
    view.design = d;
    r
}

/// The view's press, first: `Some` when the sketch face took it.
pub fn press(view: &mut View, button: Button, mods: u8, x: f64, y: f64) -> Option<bool> {
    with_design(view, |d, v| d.press(v, button, mods, x, y))
}

/// The view's pointer motion (no navigation drag in progress), first.
pub fn motion(view: &mut View, x: f64, y: f64) -> Option<bool> {
    with_design(view, |d, v| d.motion(v, x, y))
}

/// The view's release (no navigation drag in progress), first.
pub fn release(view: &mut View, x: f64, y: f64) -> Option<bool> {
    with_design(view, |d, v| d.release(v, x, y))
}

/// The view's key, first: the tool keys, `e`, Esc.
pub fn key(view: &mut View, model: &Model, k: &str) -> Option<bool> {
    with_design(view, |d, v| d.key(v, model, k))
}

/// The sketch overlay, drawn over the frame: the profile tint, the entities, the points, the live circle, the glyphs
/// and the dof label.
pub fn overlay(view: &View, fr: &mut Raster) {
    let Some(face) = view.design.face.as_ref() else { return };
    let scr = |q: Vec2| face.to_screen(view, q).map(|(x, y)| (x as f32, y as f32));
    let ring = |poly: Vec<Vec2>| -> Option<Vec<(f32, f32)>> { poly.into_iter().map(scr).collect() };
    for p in &face.profiles {
        let rings: Option<Vec<Vec<(f32, f32)>>> = core::iter::once(&p.outer).chain(p.holes.iter()).map(|l| ring(polygon(l))).collect();
        if let Some(rings) = rings {
            raster::tint(fr, &rings, rgba(TINT), 0.3);
        }
    }
    let poly = |fr: &mut Raster, pts: &[Vec2], col: [u8; 4]| {
        for w in pts.windows(2) {
            if let (Some(a), Some(b)) = (scr(w[0]), scr(w[1])) {
                raster::line(fr, a, b, col);
            }
        }
    };
    let circle = |c: Vec2, r: f64, a0: f64, sweep: f64| -> Vec<Vec2> {
        let k = ((sweep.abs() / TAU) * 64.0).ceil().max(2.0) as usize;
        (0..=k).map(|i| a0 + sweep * i as f64 / k as f64).map(|t| Vec2::new(c.x + r * t.cos(), c.y + r * t.sin())).collect()
    };
    let ink = rgba(INK);
    for (i, e) in face.sketch.entities().iter().enumerate() {
        let id = EntityId(i as u32);
        match *e {
            Entity::Line { a, b } => {
                if let (Some(p), Some(q)) = (face.sketch.point(PointRef::Point(a)), face.sketch.point(PointRef::Point(b))) {
                    poly(fr, &[p, q], ink);
                }
            }
            Entity::Circle { .. } => {
                if let (Some(c), Some(r)) = (face.sketch.point(PointRef::Centre(id)), face.sketch.radius(id)) {
                    poly(fr, &circle(c, r, 0.0, TAU), ink);
                }
            }
            Entity::Arc { r, .. } => {
                let pr = face.sketch.params();
                if let Some(c) = face.sketch.point(PointRef::Centre(id)) {
                    let (s, e) = (pr[r + 1], pr[r + 2]);
                    let sweep = (e - s).rem_euclid(TAU);
                    let sweep = if sweep == 0.0 { TAU } else { sweep };
                    poly(fr, &circle(c, pr[r], s, sweep), ink);
                }
            }
            Entity::Point { .. } => {
                if let Some((x, y)) = face.sketch.point(PointRef::Point(id)).and_then(scr) {
                    let col = if face.selection == Some(id) { rgba(ACTIVE) } else { ink };
                    raster::line(fr, (x - 2.0, y), (x + 2.0, y), col);
                    raster::line(fr, (x, y - 2.0), (x, y + 2.0), col);
                }
            }
            // VUGSKETCHERASE (SR495): a tombstone draws nothing.
            Entity::Removed => {}
        }
    }
    // VUGSKETCHERASE (SR495): the live circle in the accent.
    if let Some(Gesture::Circle { circle: live, .. }) = face.gesture {
        if let (Some(c), Some(r)) = (face.sketch.point(PointRef::Centre(live)), face.sketch.radius(live)) {
            poly(fr, &circle(c, r, 0.0, TAU), rgba(ACTIVE));
        }
    }
    if let Some(Hover { at, snap: Some(_), .. }) = &face.hover {
        if let Some((x, y)) = scr(*at) {
            raster::line(fr, (x - 4.0, y - 4.0), (x + 4.0, y + 4.0), rgba(ACTIVE));
            raster::line(fr, (x - 4.0, y + 4.0), (x + 4.0, y - 4.0), rgba(ACTIVE));
        }
    }
    for g in face.glyphs(view) {
        raster::glyph(fr, g.mark.ch(), g.at.0, g.at.1, rgba(GLYPH));
    }
    dimension_overlay(view, face, fr); // VUGFACEDIMENSION (SR494): the dimensions and the typed pull
    let label = if view.design.refusal.is_some() { rgba(REFUSED) } else { ink };
    raster::text(fr, &format!("dof {}", face.dof), 6.0, 6.0, label);
}

// ---- VUGFACEDIMENSION (SR494): the dimension tool, the typed value, the typed pull ----

/// A click within this many surface pixels of a placed dimension's value re-opens it.
pub const REOPEN_PX: f64 = 12.0;

/// What a dimension measures: the sketch_core constraint it becomes on Enter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dim {
    /// A line's length: `Distance` between its two points.
    Length(EntityId),
    /// Two points' distance.
    Points(PointRef, PointRef),
    /// A point's distance to a line (on the side it is drawn).
    PointLine(PointRef, EntityId),
    /// The angle between two lines (typed in degrees, kept with the sign of the drawn CCW angle).
    Angle(EntityId, EntityId),
    Radius(EntityId),
    Diameter(EntityId),
}

/// A placed dimension: its constraint, what it measures, where its value sits on the plane (mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dimension {
    pub constraint: ConstraintId,
    pub dim: Dim,
    pub anchor: Vec2,
}

/// The dimension being edited: the typed text, the dimension it re-opened (an index in `dims`), the refusal's name
/// after a refused Enter (cleared by the next key).
#[derive(Clone, Debug, PartialEq)]
pub struct DimEdit {
    pub dim: Dim,
    pub anchor: Vec2,
    pub buffer: String,
    pub reopened: Option<usize>,
    pub refusal: Option<String>,
}

/// A dimension as drawn, beside the inference glyphs: the value (or the typed text, or the refusal's name) at the
/// anchor and a leader to the foot on the measured geometry (surface pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct DimensionGlyph {
    pub constraint: Option<ConstraintId>,
    pub anchor: (f32, f32),
    pub foot: (f32, f32),
    pub text: String,
    pub editing: bool,
    pub refused: bool,
}

fn line_ends(sk: &Sketch, l: EntityId) -> Option<(Vec2, Vec2)> {
    match sk.entity(l)? {
        Entity::Line { a, b } => Some((sk.point(PointRef::Point(*a))?, sk.point(PointRef::Point(*b))?)),
        _ => None,
    }
}

fn midway(p: Vec2, q: Vec2) -> Vec2 {
    Vec2::new(0.5 * (p.x + q.x), 0.5 * (p.y + q.y))
}

/// The signed CCW angle from line `l1` to line `l2` (radians), as sketch_core's `Angle` measures it.
fn ccw(sk: &Sketch, l1: EntityId, l2: EntityId) -> Option<f64> {
    let (a1, b1) = line_ends(sk, l1)?;
    let (a2, b2) = line_ends(sk, l2)?;
    let (u, v) = (sub(b1, a1), sub(b2, a2));
    Some((u.x * v.y - u.y * v.x).atan2(u.x * v.x + u.y * v.y))
}

/// A value as drawn: three decimals, trailing zeros dropped ("16", "10.5").
pub(crate) fn fmt_value(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" { "0".to_string() } else { s }
}

/// A refusal's name: the text before its `(` ("Conflicting([..])" -> "Conflicting").
pub(crate) fn refusal_word(e: &str) -> String {
    e.split('(').next().unwrap_or(e).to_string()
}

/// A typed key into a number's text: digits, one `.`, a leading `-`, Backspace. False when the key is none of them.
pub(crate) fn type_into(buf: &mut String, ch: Option<char>, back: bool) -> bool { // VUGFACETREE (SR513): the row edit's buffer road
    if back {
        buf.pop();
        return true;
    }
    match ch {
        Some(c) if c.is_ascii_digit() => buf.push(c),
        Some('.') => {
            if !buf.contains('.') {
                buf.push('.');
            }
        }
        Some('-') => {
            if buf.is_empty() {
                buf.push('-');
            }
        }
        _ => return false,
    }
    true
}

/// VUGFACESAVEAS (SR539): a typed key into a free text (the Save As path): any non-control character, Backspace pops.
/// False when the key is none of them (a key name such as `Shift_L`); the caller owns Enter and Esc, as with [`type_into`].
pub(crate) fn type_text(buf: &mut String, ch: Option<char>, back: bool) -> bool {
    if back {
        buf.pop();
        return true;
    }
    match ch {
        Some(c) if !c.is_control() => buf.push(c),
        _ => return false,
    }
    true
}

impl Dim {
    /// The value now: mm, or degrees (the magnitude) for an angle; None when an entity it names is gone.
    pub fn measure(self, sk: &Sketch) -> Option<f64> {
        match self {
            Dim::Length(l) => line_ends(sk, l).map(|(p, q)| len(sub(q, p))),
            Dim::Points(a, b) => Some(len(sub(sk.point(b)?, sk.point(a)?))),
            Dim::PointLine(p, l) => {
                let pp = sk.point(p)?;
                let (a, b) = line_ends(sk, l)?;
                let d = sub(b, a);
                let n = len(d);
                if !(n > 0.0) {
                    return None;
                }
                Some((d.x * (pp.y - a.y) - d.y * (pp.x - a.x)).abs() / n)
            }
            Dim::Angle(l1, l2) => ccw(sk, l1, l2).map(|t| t.abs().to_degrees()),
            Dim::Radius(c) => sk.radius(c),
            Dim::Diameter(c) => sk.radius(c).map(|r| 2.0 * r),
        }
    }

    /// The constraint for the typed value `v` (mm, or degrees): a length not above `Tol.lin` or a non-finite value is
    /// refused `Invalid`.
    pub fn constraint(self, v: f64, sk: &Sketch) -> Result<Constraint, String> {
        let invalid = || String::from("Invalid");
        if !v.is_finite() {
            return Err(invalid());
        }
        let pos = |v: f64| if v > Tol::DEFAULT.lin { Ok(v) } else { Err(invalid()) };
        match self {
            Dim::Angle(l1, l2) => {
                let th = v.to_radians();
                let th = if ccw(sk, l1, l2).ok_or_else(invalid)? < 0.0 { -th } else { th };
                Ok(Constraint::Angle(l1, l2, th))
            }
            Dim::Length(l) => match sk.entity(l) {
                Some(Entity::Line { a, b }) => Ok(Constraint::Distance(PointRef::Point(*a), PointRef::Point(*b), pos(v)?)),
                _ => Err(invalid()),
            },
            Dim::Points(a, b) => Ok(Constraint::Distance(a, b, pos(v)?)),
            Dim::PointLine(p, l) => Ok(Constraint::DistancePointLine(p, l, pos(v)?)),
            Dim::Radius(c) => Ok(Constraint::Radius(c, pos(v)?)),
            Dim::Diameter(c) => Ok(Constraint::Diameter(c, pos(v)?)),
        }
    }

    /// The leader's end on the measured geometry: a segment's midpoint, the point, the rim toward the anchor.
    pub fn foot(self, sk: &Sketch, anchor: Vec2) -> Option<Vec2> {
        match self {
            Dim::Length(l) | Dim::Angle(l, _) => line_ends(sk, l).map(|(p, q)| midway(p, q)),
            Dim::Points(a, b) => Some(midway(sk.point(a)?, sk.point(b)?)),
            Dim::PointLine(p, _) => sk.point(p),
            Dim::Radius(c) | Dim::Diameter(c) => {
                let (o, r) = (sk.point(PointRef::Centre(c))?, sk.radius(c)?);
                let d = sub(anchor, o);
                let n = len(d);
                Some(if n > 0.0 { Vec2::new(o.x + d.x / n * r, o.y + d.y / n * r) } else { Vec2::new(o.x + r, o.y) })
            }
        }
    }

    /// The dimension a sketch_core constraint is, when it is one.
    pub fn of(c: &Constraint) -> Option<Dim> {
        match *c {
            Constraint::Distance(a, b, _) => Some(Dim::Points(a, b)),
            Constraint::DistancePointLine(p, l, _) => Some(Dim::PointLine(p, l)),
            Constraint::Angle(a, b, _) => Some(Dim::Angle(a, b)),
            Constraint::Radius(c, _) => Some(Dim::Radius(c)),
            Constraint::Diameter(c, _) => Some(Dim::Diameter(c)),
            _ => None,
        }
    }

    /// The value as drawn: "16", "R5", "D10", "90°".
    pub fn label(self, v: f64) -> String {
        let n = fmt_value(v);
        match self {
            Dim::Radius(_) => format!("R{n}"),
            Dim::Diameter(_) => format!("D{n}"),
            Dim::Angle(..) => format!("{n}°"),
            _ => n,
        }
    }
}

impl SketchFace {
    /// The line whose drawn segment passes within `SNAP_PX` of (x, y) (the nearest).
    fn line_at(&self, view: &View, x: f64, y: f64) -> Option<EntityId> {
        let mut best: Option<(f64, EntityId)> = None;
        for (i, e) in self.sketch.entities().iter().enumerate() {
            let id = EntityId(i as u32);
            if !matches!(e, Entity::Line { .. }) {
                continue;
            }
            let Some((p, q)) = line_ends(&self.sketch, id) else { continue };
            let (Some(a), Some(b)) = (self.to_screen(view, p), self.to_screen(view, q)) else { continue };
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((x - a.0) * dx + (y - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let d = (a.0 + t * dx - x).hypot(a.1 + t * dy - y);
            if d <= SNAP_PX && best.map_or(true, |(d0, _)| d < d0) {
                best = Some((d, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// The circle or arc whose rim passes within `SNAP_PX` of plane point `q` (the nearest; `line_marks`'s reach).
    fn round_at(&self, view: &View, x: f64, y: f64, q: Vec2) -> Option<EntityId> {
        let reach = SNAP_PX * self.mm_per_px(view, x, y);
        let mut best: Option<(f64, EntityId)> = None;
        for (i, e) in self.sketch.entities().iter().enumerate() {
            let id = EntityId(i as u32);
            if !matches!(e, Entity::Circle { .. } | Entity::Arc { .. }) {
                continue;
            }
            let (Some(c), Some(r)) = (self.sketch.point(PointRef::Centre(id)), self.sketch.radius(id)) else { continue };
            let d = (len(sub(q, c)) - r).abs();
            if d <= reach && best.map_or(true, |(d0, _)| d < d0) {
                best = Some((d, id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// A press on a placed dimension's value re-opens it for editing; false when none is there.
    fn reopen_at(&mut self, view: &View, x: f64, y: f64) -> bool {
        let hit = self.dims.iter().position(|d| {
            self.sketch.constraint(d.constraint).is_some() && self.px_dist(view, d.anchor, x, y).is_some_and(|p| p <= REOPEN_PX)
        });
        let Some(i) = hit else { return false };
        let d = self.dims[i];
        self.edit = Some(DimEdit { dim: d.dim, anchor: d.anchor, buffer: String::new(), reopened: Some(i), refusal: None });
        self.pick = None;
        true
    }

    /// The dimension tool's press: a point, else a line, else a rim. A fresh length not yet typed becomes the angle
    /// (a second line) or the point-to-line distance (a point), else is placed at the click; a first point waits for a
    /// second point (distance) or a line; a circle opens its diameter, an arc its radius, a line its length.
    fn dim_press(&mut self, view: &View, x: f64, y: f64, q: Vec2) -> bool {
        let point = match self.snap_at(view, x, y, &[], false) {
            Some((Snap::Point(id), _)) => Some(PointRef::Point(id)),
            _ => None,
        };
        let line = if point.is_none() { self.line_at(view, x, y) } else { None };
        let round = if point.is_none() && line.is_none() { self.round_at(view, x, y, q) } else { None };
        let fresh = match &self.edit {
            Some(DimEdit { dim: Dim::Length(l), reopened: None, buffer, refusal: None, .. }) if buffer.is_empty() => Some(*l),
            _ => None,
        };
        if let Some(l) = fresh {
            let dim = match (point, line) {
                (Some(p), _) => Some(Dim::PointLine(p, l)),
                (None, Some(l2)) if l2 != l => Some(Dim::Angle(l, l2)),
                _ => None,
            };
            if let Some(e) = self.edit.as_mut() {
                if let Some(d) = dim {
                    e.dim = d;
                }
                e.anchor = q;
            }
            return true;
        }
        let open = |dim: Dim| DimEdit { dim, anchor: q, buffer: String::new(), reopened: None, refusal: None };
        if let Some(p0) = self.pick.take() {
            let dim = match (point, line) {
                (Some(p), _) if p != p0 => Some(Dim::Points(p0, p)),
                (None, Some(l)) => Some(Dim::PointLine(p0, l)),
                _ => None,
            };
            self.edit = dim.map(open);
            return true;
        }
        let dim = match (point, line, round) {
            (Some(p), _, _) => {
                self.edit = None;
                self.pick = Some(p);
                return true;
            }
            (None, Some(l), _) => Some(Dim::Length(l)),
            (None, None, Some(c)) => Some(match self.sketch.entity(c) {
                Some(Entity::Circle { .. }) => Dim::Diameter(c),
                _ => Dim::Radius(c),
            }),
            _ => None,
        };
        self.edit = dim.map(open);
        true
    }

    /// The refusal stays at the dimension: its name, the typed text cleared.
    fn refuse(&mut self, err: String) -> Result<(), String> {
        if let Some(e) = self.edit.as_mut() {
            e.refusal = Some(refusal_word(&err));
            e.buffer.clear();
        }
        Err(err)
    }

    /// Enter: the typed value (else the value now) as the edit's constraint through `apply_as` with redundancy
    /// refused; a re-opened dimension's old constraint is removed first and put back when the new one is refused.
    fn commit_edit(&mut self) -> Result<(), String> {
        let Some(e) = self.edit.clone() else { return Ok(()) };
        let v = if e.buffer.is_empty() { e.dim.measure(&self.sketch) } else { e.buffer.parse::<f64>().ok() };
        let c = match v.ok_or_else(|| String::from("Invalid")).and_then(|v| e.dim.constraint(v, &self.sketch)) {
            Ok(c) => c,
            Err(w) => return self.refuse(w),
        };
        let old = e.reopened.and_then(|i| self.dims.get(i).copied()).and_then(|d| self.sketch.constraint(d.constraint).copied().map(|oc| (d.constraint, oc)));
        if let Some((id, _)) = old {
            self.sketch.remove(id);
        }
        let r = match self.apply_as(vec![c], true) {
            Ok(ids) if !ids.is_empty() => Ok(ids[0]),
            Ok(_) => Err(String::from("Invalid")),
            Err(err) => Err(err),
        };
        match r {
            Ok(id) => {
                let d = Dimension { constraint: id, dim: e.dim, anchor: e.anchor };
                match e.reopened.and_then(|i| self.dims.get_mut(i)) {
                    Some(slot) => *slot = d,
                    None => self.dims.push(d),
                }
                self.edit = None;
                Ok(())
            }
            Err(err) => {
                if let Some((_, oc)) = old {
                    if let Ok(id) = self.sketch.add(oc) {
                        self.issued = self.issued.max(id.0 + 1);
                        if let Some(slot) = e.reopened.and_then(|i| self.dims.get_mut(i)) {
                            slot.constraint = id;
                        }
                    }
                    let _ = self.solve();
                }
                self.refuse(err)
            }
        }
    }

    /// The dimension glyphs: every placed dimension whose constraint stands (the re-opened one as the edit), then
    /// the edit (its typed text, else its value now, else the refusal's name).
    pub fn dimension_glyphs(&self, view: &View) -> Vec<DimensionGlyph> {
        let scr = |q: Vec2| self.to_screen(view, q).map(|(x, y)| (x as f32, y as f32));
        let mut out = Vec::new();
        let reopened = self.edit.as_ref().and_then(|e| e.reopened);
        for (i, d) in self.dims.iter().enumerate() {
            if reopened == Some(i) || self.sketch.constraint(d.constraint).is_none() {
                continue;
            }
            let (Some(v), Some(f)) = (d.dim.measure(&self.sketch), d.dim.foot(&self.sketch, d.anchor)) else { continue };
            let (Some(a), Some(f)) = (scr(d.anchor), scr(f)) else { continue };
            out.push(DimensionGlyph { constraint: Some(d.constraint), anchor: a, foot: f, text: d.dim.label(v), editing: false, refused: false });
        }
        if let Some(e) = &self.edit {
            if let (Some(a), Some(f)) = (scr(e.anchor), e.dim.foot(&self.sketch, e.anchor).and_then(scr)) {
                let text = match (&e.refusal, e.buffer.is_empty()) {
                    (Some(w), _) => w.to_uppercase(),
                    (None, false) => e.buffer.clone(),
                    (None, true) => e.dim.measure(&self.sketch).map(|v| e.dim.label(v)).unwrap_or_default(),
                };
                let constraint = e.reopened.and_then(|i| self.dims.get(i)).map(|d| d.constraint);
                out.push(DimensionGlyph { constraint, anchor: a, foot: f, text, editing: true, refused: e.refusal.is_some() });
            }
        }
        out
    }
}

impl Design {
    /// The keys an open pull or dimension edit takes first: digits, `.`, a leading `-`, Backspace, Enter, Esc (and `d`
    /// toggling a round dimension). Any other key closes an edit uncommitted and falls through.
    fn typed_key(&mut self, k: &str) -> Option<bool> {
        let enter = matches!(k, "Return" | "KP_Enter" | "\r" | "\n");
        let back = matches!(k, "BackSpace" | "\u{8}" | "\u{7f}");
        let ch = {
            let mut it = k.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        };
        if self.pull.is_some() {
            if enter {
                self.commit_pull();
                return Some(true);
            }
            if !type_into(&mut self.pull_text, ch, back) {
                return None;
            }
            if let Ok(v) = self.pull_text.parse::<f64>() {
                if v.is_finite() {
                    if let Some(p) = self.pull.as_mut() {
                        p.distance = v;
                    }
                    self.pull_preview();
                }
            }
            return Some(true);
        }
        let face = self.face.as_mut()?;
        let edit = face.edit.as_mut()?;
        if matches!(k, "Escape" | "\u{1b}") {
            face.edit = None;
            return Some(true);
        }
        if enter {
            let r = face.commit_edit();
            self.note(r);
            self.journal.record(self.face.as_ref(), None); // VUGFACETREE (SR513): a dimension's Enter commits
            return Some(true);
        }
        if matches!(k, "d" | "D") {
            let toggled = match edit.dim {
                Dim::Radius(c) => Some(Dim::Diameter(c)),
                Dim::Diameter(c) => Some(Dim::Radius(c)),
                _ => None,
            };
            if let Some(d) = toggled {
                edit.dim = d;
                edit.refusal = None;
                return Some(true);
            }
        }
        if type_into(&mut edit.buffer, ch, back) {
            edit.refusal = None;
            return Some(true);
        }
        face.edit = None;
        None
    }

    /// The pull's preview at its distance: every Brep of the design plus this extrude (`pull_motion` and the typed
    /// pull, the one extrude road).
    fn pull_preview(&mut self) {
        let mut all: Vec<brep_core::Brep> = self.body.as_ref().map(|b| b.breps.clone()).unwrap_or_default();
        let Some(pull) = self.pull.as_mut() else { return };
        pull.preview = match extrude::extrude(&pull.profile, &pull.plane, pull.distance, geom_core::Tol::DEFAULT) {
            Ok(b) => {
                all.push(b);
                Body::from_breps(all, geom_core::Tol::DEFAULT).ok()
            }
            Err(_) => None,
        };
    }

    /// Commit the pull (a click or Enter): its preview becomes the body and the sketch closes; a pull that never left
    /// the plane is dropped.
    fn commit_pull(&mut self) {
        if let Some(pull) = self.pull.take() {
            if pull.preview.is_some() { self.journal.record(self.face.as_ref(), Some(&pull)); } // VUGFACETREE (SR513): the pull's extrude
            if let Some(b) = pull.preview {
                self.body = Some(b);
                self.face = None;
            }
        }
        self.pull_text.clear();
    }
}

/// The dimensions (leader, tick, value; the edit underlined in the accent, a refusal in the refusal colour) and the
/// typed pull distance at the pulled centroid.
fn dimension_overlay(view: &View, face: &SketchFace, fr: &mut Raster) {
    for g in face.dimension_glyphs(view) {
        let col = rgba(if g.refused {
            REFUSED
        } else if g.editing {
            ACTIVE
        } else {
            INK
        });
        raster::line(fr, g.anchor, g.foot, col);
        let (dx, dy) = (g.foot.0 - g.anchor.0, g.foot.1 - g.anchor.1);
        let l = dx.hypot(dy);
        if l > 0.0 {
            let (nx, ny) = (-dy / l * 3.0, dx / l * 3.0);
            raster::line(fr, (g.foot.0 - nx, g.foot.1 - ny), (g.foot.0 + nx, g.foot.1 + ny), col);
        }
        let (tx, ty) = (g.anchor.0 + 3.0, g.anchor.1 - 5.0);
        raster::text(fr, &g.text, tx, ty, col);
        if g.editing {
            let w = 8.0 * g.text.chars().count().max(1) as f32;
            raster::line(fr, (tx, ty + 12.0), (tx + w, ty + 12.0), col);
        }
    }
    if let Some(p) = view.design.pull.as_ref() {
        if !view.design.pull_text.is_empty() {
            if let Some((x, y)) = view.project_f64(p.centroid) {
                raster::text(fr, &view.design.pull_text, x as f32 + 6.0, y as f32 - 14.0, rgba(ACTIVE));
            }
        }
    }
}

// ---- VUGFACEPHOTO (SR506): the photo face's door — a measured sketch opened whole, the apply_as road named ----

impl SketchFace {
    /// A face over `sketch` (photo_core's measured sketch) on `plane`: its entities and constraints keep their ids, the
    /// face's origin point is appended and fixed at (0, 0), and the glyph walk's bound covers every measured
    /// constraint. Solved and settled.
    pub fn over(plane: Frame, mut sketch: Sketch) -> SketchFace {
        let mut f = SketchFace::new(plane);
        let origin = sketch.add_point(0.0, 0.0);
        let fix = sketch.add(Constraint::Fix(PointRef::Point(origin), Vec2::new(0.0, 0.0)));
        f.sketch = sketch;
        f.origin = origin;
        f.issued = fix.map(|id| id.0 + 1).unwrap_or(0);
        let _ = f.solve();
        f
    }

    /// `apply_as` with a redundant constraint refused (the typed-dimension rule): a proposal's constraint enters the
    /// sketch only here, and a refusal removes exactly what was added and names it.
    pub fn constrain(&mut self, cs: Vec<Constraint>) -> Result<Vec<ConstraintId>, String> {
        self.apply_as(cs, true)
    }

    /// One past the highest constraint id this face has added (every live constraint id is below it).
    pub fn issued(&self) -> u32 {
        self.issued
    }
}
