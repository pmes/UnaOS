// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `sketch_core` — the 2D constraint solver every Vug sketch runs (VUGSKETCH, LEDGER SR465).
//!
//! CHARTER: Vug — shared-core. Vug (`handlers/vug`, CODEX §2 3D, "CAD viewing, editing, and CAM/Slicing") owns the
//! sketch; this crate is the part both rings link (BP-handlers-4, BP-vug-4): pure, f64 (BP-vug-1), every tolerance a
//! `Tol` argument (BP-vug-2). "A sketch should feel like drawing" (docs/dev/USERLAND/VUG.md §3.2): the solver keeps
//! the rest still while one thing is dragged, and over-constraint is NAMED, never a red mystery.
//!
//! * The model ([`Sketch`]): entities (Point, Line, Circle, Arc) whose coordinates are entries of ONE f64 parameter
//!   vector; [`Constraint`]s, each a residual with exact analytic Jacobian rows (the formulation planegcs and
//!   SolveSpace share; cited, no code taken).
//! * [`solve`] / [`solve_strict`] / [`drag`]: ONE mechanism, the weighted min-norm Levenberg-Marquardt step
//!   `δ = −W⁻¹Jᵀ(JW⁻¹Jᵀ + λI)⁻¹r`. An under-determined sketch moves as little as possible (the "sticky" prior);
//!   a drag presets the dragged point and weights its parameters heavy so the rest absorbs the change. Params are
//!   written back only on Ok.
//! * The diagnosis: the Jacobian's rank by ORDERED modified Gram-Schmidt, so of a dependent set the constraint
//!   added LAST is named: redundant when consistent ([`Solved::redundant`]; [`solve_strict`] refuses it), conflicting
//!   when not ([`Refusal::Conflicting`]); [`dof`] counts parameters minus rank and names the free entities.
//! * [`profiles`]: closed loops of lines and arcs, outer CCW with holes CW, exact signed areas — what VUG-BREP's
//!   extrude and revolve take.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use geom_core as geom;
pub use geom::{Arc2, Tol, Vec2};

mod constraint;
mod entity;
mod linalg;
mod profile;
mod solve;

pub use constraint::Constraint;
pub use entity::{BadRef, ConstraintId, Entity, EntityId, PointRef, Sketch};
pub use profile::{Edge, Loop, Profile, profiles};
pub use solve::{Dof, Refusal, Solved, dof, drag, solve, solve_strict};
