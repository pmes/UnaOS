// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D) — shared-core: doc_core's ONE sketch writer, a sketch_core sketch to its statements.
//!
//! VUGDOCFROMSKETCH (LEDGER SR516): [`from_sketch`] is the inverse of [`crate::replay`]'s sketch walk. It writes the
//! sketch statement on its plane, then every live entity in SLOT order (a tombstone, `Entity::Removed`, is skipped
//! and its slot's name is never reused), then every live constraint below `issued` in id order. Replay re-adds the
//! entities in document order, so the replayed sketch's dense `EntityId(n)` is the n-th live slot. Names come from a
//! [`SketchNames`]: vug-view's journal names a face slot with its sketch's prefix; [`PlainNames`] is the plain
//! `s` / `e<slot>` / `k<id>` that tools/photo and the face's one-sketch document use. [`sketch_cons`] /
//! [`sketch_pref`] are THE sketch_core -> text constraint map (replay's `constraint` is its inverse).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use sketch_core::{Constraint, ConstraintId, Entity, EntityId, PointRef, Sketch};

use crate::{Cons, PRef, Stmt};

/// The text ids a written sketch takes: the sketch statement's, an entity's by its slot (tombstones keep theirs),
/// a constraint's by its id.
pub trait SketchNames {
    fn sketch(&self) -> String;
    fn entity(&self, slot: EntityId) -> String;
    fn constraint(&self, id: ConstraintId) -> String;
}

/// The plain names: sketch `s`, entity `e<slot>`, constraint `k<id>`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlainNames;

impl SketchNames for PlainNames {
    fn sketch(&self) -> String {
        String::from("s")
    }
    fn entity(&self, slot: EntityId) -> String {
        format!("e{}", slot.0)
    }
    fn constraint(&self, id: ConstraintId) -> String {
        format!("k{}", id.0)
    }
}

/// A sketch_core point reference as doc_core's, entities named by `name`.
pub fn sketch_pref(p: PointRef, name: &dyn Fn(EntityId) -> String) -> PRef {
    match p {
        PointRef::Point(e) => PRef::Point(name(e)),
        PointRef::Centre(e) => PRef::Centre(name(e)),
        PointRef::Start(e) => PRef::Start(name(e)),
        PointRef::End(e) => PRef::End(name(e)),
    }
}

/// A sketch_core constraint as doc_core's text form, entities named by `name` (THE map).
pub fn sketch_cons(c: &Constraint, name: &dyn Fn(EntityId) -> String) -> Cons {
    let p = |r: PointRef| sketch_pref(r, name);
    match *c {
        Constraint::Coincident(a, b) => Cons::Coincident(p(a), p(b)),
        Constraint::PointOnLine(q, l) => Cons::PointOnLine(p(q), name(l)),
        Constraint::PointOnCircle(q, r) => Cons::PointOnCircle(p(q), name(r)),
        Constraint::Horizontal(l) => Cons::Horizontal(name(l)),
        Constraint::Vertical(l) => Cons::Vertical(name(l)),
        Constraint::Parallel(a, b) => Cons::Parallel(name(a), name(b)),
        Constraint::Perpendicular(a, b) => Cons::Perpendicular(name(a), name(b)),
        Constraint::TangentLineCircle(l, r) => Cons::TangentLineCircle(name(l), name(r)),
        Constraint::TangentCircles(a, b) => Cons::TangentCircles(name(a), name(b)),
        Constraint::EqualLength(a, b) => Cons::EqualLength(name(a), name(b)),
        Constraint::EqualRadius(a, b) => Cons::EqualRadius(name(a), name(b)),
        Constraint::Distance(a, b, d) => Cons::Distance(p(a), p(b), d),
        Constraint::DistancePointLine(q, l, d) => Cons::DistancePointLine(p(q), name(l), d),
        Constraint::Angle(a, b, t) => Cons::Angle(name(a), name(b), t),
        Constraint::Radius(r, v) => Cons::Radius(name(r), v),
        Constraint::Diameter(r, v) => Cons::Diameter(name(r), v),
        Constraint::Fix(q, at) => Cons::Fix(p(q), [at.x, at.y]),
        Constraint::Midpoint(q, l) => Cons::Midpoint(p(q), name(l)),
        Constraint::Symmetric(a, b, l) => Cons::Symmetric(p(a), p(b), name(l)),
    }
}

/// The sketch's statements: the sketch on `plane` (`Frame::new(o, z, x)`'s origin, normal, x hint), every live entity
/// in slot order, every live constraint below `issued` in id order, each named by `names`.
pub fn from_sketch(sketch: &Sketch, plane: ([f64; 3], [f64; 3], [f64; 3]), issued: u32, names: &dyn SketchNames) -> Vec<Stmt> {
    let s = names.sketch();
    let mut out = Vec::new();
    out.push(Stmt::Sketch { id: s.clone(), o: plane.0, z: plane.1, x: plane.2 });
    let pr = sketch.params();
    let name = |e: EntityId| names.entity(e);
    for (i, e) in sketch.entities().iter().enumerate() {
        let id = name(EntityId(i as u32));
        let st = match *e {
            Entity::Point { x } => Stmt::Point { id, sketch: s.clone(), at: [pr[x], pr[x + 1]] },
            Entity::Line { a, b } => Stmt::Line { id, sketch: s.clone(), a: name(a), b: name(b) },
            Entity::Circle { centre, r } => Stmt::Circle { id, sketch: s.clone(), centre: name(centre), r: pr[r] },
            Entity::Arc { centre, r } => {
                Stmt::Arc { id, sketch: s.clone(), centre: name(centre), r: pr[r], start: pr[r + 1], end: pr[r + 2] }
            }
            Entity::Removed => continue,
        };
        out.push(st);
    }
    for i in 0..issued {
        if let Some(c) = sketch.constraint(ConstraintId(i)) {
            out.push(Stmt::Constraint { id: names.constraint(ConstraintId(i)), sketch: s.clone(), c: sketch_cons(c, &name) });
        }
    }
    out
}
