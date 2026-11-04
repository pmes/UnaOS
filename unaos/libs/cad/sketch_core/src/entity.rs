// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The sketch: entities over one parameter vector, and the constraints on them.

use crate::constraint::Constraint;
use crate::geom::Vec2;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConstraintId(pub u32);

/// An entity; the `usize` fields are indices into the parameter vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Entity {
    /// `x` at `params[x]`, `y` at `params[x + 1]`.
    Point { x: usize },
    /// From point `a` to point `b`.
    Line { a: EntityId, b: EntityId },
    /// Centre point and the radius at `params[r]`.
    Circle { centre: EntityId, r: usize },
    /// Centre point, radius at `params[r]`, start angle at `params[r + 1]`, end angle at `params[r + 2]`; the arc runs
    /// CCW from start to end.
    Arc { centre: EntityId, r: usize },
    /// VUGSKETCHERASE (SR495): the tombstone of a removed entity. Its id is never reused and its parameters stay in
    /// place (frozen: no live constraint names them, so the solver never moves them and `dof` does not count them);
    /// every `need_*` refuses it, so nothing new can reference it.
    Removed,
}

/// A point of the sketch: a Point entity, a circle's or arc's centre, or a line's or arc's start / end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointRef {
    Point(EntityId),
    Centre(EntityId),
    Start(EntityId),
    End(EntityId),
}

/// A reference that names no entity of the right kind: the offending entity, refused by name, and the kind its slot
/// wanted (VUGSKETCHBADREFKIND, SR604: named at the one check site per slot, the `need_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadRef {
    pub id: EntityId,
    pub kind: RefKind,
}

// ---- VUGSKETCHBADREFKIND (LEDGER SR604): the kind a reference slot wants, named where it is checked ----

/// The kind of sketch entity a reference slot wants (moved from doc_core's replay, SR590, which re-exports it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// A point entity (a line's end, a circle's centre, `PointRef::Point`).
    Point,
    /// A line.
    Line,
    /// A circle or an arc (`PointRef::Centre`, a radius, a circle slot).
    Round,
    /// A line or an arc (`PointRef::Start` / `PointRef::End`).
    Ends,
    /// An arc (`Sketch::set_arc_angles`: a circle has no angles).
    Arc,
    /// Any live entity (`Sketch::remove_entity`: a tombstone or an unknown id).
    Live,
}

/// A declaration of a sketch entity outside the sketch (doc_core's statements): whether it declares an entity of kind
/// `k`. The declaring crate writes the table; [`RefKind::holds`] reads it.
pub trait Declares {
    fn declares(&self, k: RefKind) -> bool;
}

impl RefKind {
    /// `d` declares an entity of this kind (the table is `d`'s [`Declares`]).
    pub fn holds<D: Declares + ?Sized>(self, d: &D) -> bool {
        d.declares(self)
    }
}

/// Entities, constraints and the parameter vector. Ids are stable: a removed constraint leaves a hole, a removed
/// entity leaves [`Entity::Removed`] (VUGSKETCHERASE, SR495).
#[derive(Clone, Debug, Default)]
pub struct Sketch {
    pub(crate) entities: Vec<Entity>,
    pub(crate) constraints: Vec<Option<Constraint>>,
    pub(crate) params: Vec<f64>,
}

impl Sketch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_point(&mut self, x: f64, y: f64) -> EntityId {
        let i = self.params.len();
        self.params.push(x);
        self.params.push(y);
        self.push(Entity::Point { x: i })
    }

    pub fn add_line(&mut self, a: EntityId, b: EntityId) -> Result<EntityId, BadRef> {
        self.need_point(a)?;
        self.need_point(b)?;
        Ok(self.push(Entity::Line { a, b }))
    }

    pub fn add_circle(&mut self, centre: EntityId, r: f64) -> Result<EntityId, BadRef> {
        self.need_point(centre)?;
        let i = self.params.len();
        self.params.push(r);
        Ok(self.push(Entity::Circle { centre, r: i }))
    }

    /// An arc CCW from `start` to `end` (radians).
    pub fn add_arc(&mut self, centre: EntityId, r: f64, start: f64, end: f64) -> Result<EntityId, BadRef> {
        self.need_point(centre)?;
        let i = self.params.len();
        self.params.push(r);
        self.params.push(start);
        self.params.push(end);
        Ok(self.push(Entity::Arc { centre, r: i }))
    }

    /// Add a constraint after checking every entity it names is of the kind it needs.
    pub fn add(&mut self, c: Constraint) -> Result<ConstraintId, BadRef> {
        c.check(self)?;
        let id = ConstraintId(self.constraints.len() as u32);
        self.constraints.push(Some(c));
        Ok(id)
    }

    /// Remove a constraint; false when the id names none.
    pub fn remove(&mut self, id: ConstraintId) -> bool {
        if let Some(slot) = self.constraints.get_mut(id.0 as usize) {
            if slot.is_some() {
                *slot = None;
                return true;
            }
        }
        false
    }

    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(id.0 as usize)
    }

    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub fn constraint(&self, id: ConstraintId) -> Option<&Constraint> {
        self.constraints.get(id.0 as usize).and_then(|c| c.as_ref())
    }

    pub fn params(&self) -> &[f64] {
        &self.params
    }

    /// The point's position, or None for a reference of the wrong kind.
    pub fn point(&self, p: PointRef) -> Option<Vec2> {
        point_at(self, &self.params, p)
    }

    /// A circle's or arc's radius.
    pub fn radius(&self, id: EntityId) -> Option<f64> {
        match self.entity(id)? {
            Entity::Circle { r, .. } | Entity::Arc { r, .. } => Some(self.params[*r]),
            _ => None,
        }
    }

    fn push(&mut self, e: Entity) -> EntityId {
        self.entities.push(e);
        EntityId(self.entities.len() as u32 - 1)
    }

    pub(crate) fn need_point(&self, id: EntityId) -> Result<usize, BadRef> {
        match self.entity(id) {
            Some(Entity::Point { x }) => Ok(*x),
            _ => Err(BadRef { id, kind: RefKind::Point }),
        }
    }

    pub(crate) fn need_line(&self, id: EntityId) -> Result<(EntityId, EntityId), BadRef> {
        match self.entity(id) {
            Some(Entity::Line { a, b }) => Ok((*a, *b)),
            _ => Err(BadRef { id, kind: RefKind::Line }),
        }
    }

    /// A circle or an arc: (centre point, radius param, true for an arc).
    pub(crate) fn need_round(&self, id: EntityId) -> Result<(EntityId, usize, bool), BadRef> {
        match self.entity(id) {
            Some(Entity::Circle { centre, r }) => Ok((*centre, *r, false)),
            Some(Entity::Arc { centre, r }) => Ok((*centre, *r, true)),
            _ => Err(BadRef { id, kind: RefKind::Round }),
        }
    }

    pub(crate) fn need_ref(&self, p: PointRef) -> Result<(), BadRef> {
        match p {
            PointRef::Point(id) => self.need_point(id).map(|_| ()),
            PointRef::Centre(id) => self.need_round(id).map(|_| ()),
            PointRef::Start(id) | PointRef::End(id) => match self.entity(id) {
                Some(Entity::Line { .. }) | Some(Entity::Arc { .. }) => Ok(()),
                _ => Err(BadRef { id, kind: RefKind::Ends }),
            },
        }
    }

    /// The live entity owning each parameter; None for a removed entity's frozen parameter (VUGSKETCHERASE).
    pub(crate) fn param_owner(&self) -> Vec<Option<EntityId>> {
        let mut own = alloc::vec![None; self.params.len()];
        for (i, e) in self.entities.iter().enumerate() {
            let id = Some(EntityId(i as u32));
            match *e {
                Entity::Point { x } => {
                    own[x] = id;
                    own[x + 1] = id;
                }
                Entity::Circle { r, .. } => own[r] = id,
                Entity::Arc { r, .. } => {
                    own[r] = id;
                    own[r + 1] = id;
                    own[r + 2] = id;
                }
                Entity::Line { .. } | Entity::Removed => {}
            }
        }
        own
    }

    /// VUGSKETCHERASE: the parameters a live entity owns (the solver's and `dof`'s count; frozen ones excluded).
    pub(crate) fn live_params(&self) -> usize {
        self.param_owner().iter().filter(|o| o.is_some()).count()
    }
}

/// A point's position under parameters `x` (the solver evaluates trial vectors through this).
pub(crate) fn point_at(sk: &Sketch, x: &[f64], p: PointRef) -> Option<Vec2> {
    let pt = |id: EntityId| match sk.entity(id) {
        Some(Entity::Point { x: i }) => Some(Vec2::new(x[*i], x[*i + 1])),
        _ => None,
    };
    match p {
        PointRef::Point(id) => pt(id),
        PointRef::Centre(id) => match sk.entity(id)? {
            Entity::Circle { centre, .. } | Entity::Arc { centre, .. } => pt(*centre),
            _ => None,
        },
        PointRef::Start(id) | PointRef::End(id) => match *sk.entity(id)? {
            Entity::Line { a, b } => pt(if matches!(p, PointRef::Start(_)) { a } else { b }),
            Entity::Arc { centre, r } => {
                let c = pt(centre)?;
                let a = x[r + if matches!(p, PointRef::Start(_)) { 1 } else { 2 }];
                Some(Vec2::new(c.x + x[r] * libm::cos(a), c.y + x[r] * libm::sin(a)))
            }
            _ => None,
        },
    }
}

/// VUGSKETCHERASE (SR495): what [`Sketch::remove_entity`] removed — the entity first, then its dependants ascending;
/// the constraints ascending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removed {
    pub entities: Vec<EntityId>,
    pub constraints: Vec<ConstraintId>,
}

// VUGSKETCHERASE (SR495): removal and the direct parameter setters.
impl Sketch {
    /// Remove an entity by CASCADE: the entity, every entity built on it (a line with it as an end, a circle or arc
    /// with it as centre: only points are referenced by entities, so the cascade is one level and removing a line,
    /// circle or arc never removes its points), and every constraint naming any removed entity (the ones whose
    /// `check` fails once the entities are tombstones). Ids are never reused; the parameters stay in place, frozen.
    /// `BadRef` when the id names no live entity.
    pub fn remove_entity(&mut self, id: EntityId) -> Result<Removed, BadRef> {
        match self.entity(id) {
            None | Some(Entity::Removed) => return Err(BadRef { id, kind: RefKind::Live }),
            Some(_) => {}
        }
        let mut entities = alloc::vec![id];
        for (i, e) in self.entities.iter().enumerate() {
            let built_on = match *e {
                Entity::Line { a, b } => a == id || b == id,
                Entity::Circle { centre, .. } | Entity::Arc { centre, .. } => centre == id,
                Entity::Point { .. } | Entity::Removed => false,
            };
            if built_on {
                entities.push(EntityId(i as u32));
            }
        }
        for e in &entities {
            self.entities[e.0 as usize] = Entity::Removed;
        }
        let mut constraints = Vec::new();
        for i in 0..self.constraints.len() {
            let dead = match &self.constraints[i] {
                Some(c) => c.check(self).is_err(),
                None => false,
            };
            if dead {
                let cid = ConstraintId(i as u32);
                self.remove(cid);
                constraints.push(cid);
            }
        }
        Ok(Removed { entities, constraints })
    }

    /// Move a Point entity to (x, y). No solve: the caller solves.
    pub fn set_point(&mut self, id: EntityId, x: f64, y: f64) -> Result<(), BadRef> {
        let i = self.need_point(id)?;
        self.params[i] = x;
        self.params[i + 1] = y;
        Ok(())
    }

    /// Set a circle's or arc's radius. No solve: the caller solves (a non-finite value makes it refuse `Diverged`).
    pub fn set_radius(&mut self, id: EntityId, r: f64) -> Result<(), BadRef> {
        let (_, i, _) = self.need_round(id)?;
        self.params[i] = r;
        Ok(())
    }

    /// Set an arc's start and end angles (radians, CCW from start to end). No solve: the caller solves.
    pub fn set_arc_angles(&mut self, id: EntityId, start: f64, end: f64) -> Result<(), BadRef> {
        match self.need_round(id)? {
            (_, i, true) => {
                self.params[i + 1] = start;
                self.params[i + 2] = end;
                Ok(())
            }
            _ => Err(BadRef { id, kind: RefKind::Arc }),
        }
    }
}
