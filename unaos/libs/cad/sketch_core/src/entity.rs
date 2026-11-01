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
}

/// A point of the sketch: a Point entity, a circle's or arc's centre, or a line's or arc's start / end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointRef {
    Point(EntityId),
    Centre(EntityId),
    Start(EntityId),
    End(EntityId),
}

/// A reference that names no entity of the right kind: the offending entity, refused by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadRef(pub EntityId);

/// Entities, constraints and the parameter vector. Ids are stable: a removed constraint leaves a hole.
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
            _ => Err(BadRef(id)),
        }
    }

    pub(crate) fn need_line(&self, id: EntityId) -> Result<(EntityId, EntityId), BadRef> {
        match self.entity(id) {
            Some(Entity::Line { a, b }) => Ok((*a, *b)),
            _ => Err(BadRef(id)),
        }
    }

    /// A circle or an arc: (centre point, radius param, true for an arc).
    pub(crate) fn need_round(&self, id: EntityId) -> Result<(EntityId, usize, bool), BadRef> {
        match self.entity(id) {
            Some(Entity::Circle { centre, r }) => Ok((*centre, *r, false)),
            Some(Entity::Arc { centre, r }) => Ok((*centre, *r, true)),
            _ => Err(BadRef(id)),
        }
    }

    pub(crate) fn need_ref(&self, p: PointRef) -> Result<(), BadRef> {
        match p {
            PointRef::Point(id) => self.need_point(id).map(|_| ()),
            PointRef::Centre(id) => self.need_round(id).map(|_| ()),
            PointRef::Start(id) | PointRef::End(id) => match self.entity(id) {
                Some(Entity::Line { .. }) | Some(Entity::Arc { .. }) => Ok(()),
                _ => Err(BadRef(id)),
            },
        }
    }

    /// The entity owning each parameter.
    pub(crate) fn param_owner(&self) -> Vec<EntityId> {
        let mut own = alloc::vec![EntityId(0); self.params.len()];
        for (i, e) in self.entities.iter().enumerate() {
            let id = EntityId(i as u32);
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
                Entity::Line { .. } => {}
            }
        }
        own
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
