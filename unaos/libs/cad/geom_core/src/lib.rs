// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `geom_core` — Vug's f64 geometry foundation (VUG-GEOM, LEDGER SR464).
//!
//! CHARTER: Vug — shared-core. Vug (`handlers/vug`, CODEX §2 3D, "CAD viewing, editing, and CAM/Slicing") owns CAD;
//! this crate is the part every CAD core links (BP-handlers-4, docs/dev/blueprint/vug.md): nothing geometric is
//! written twice above it (BP-vug-1, BP-vug-2).
//!
//! * Geometry is `f64` (BP-vug-1). The names mirror euclase's `f32` types (`Vec3::mag_sq`, `Mat4::transform_point3`,
//!   `Quat::from_axis_angle`, ...); the ONE conversion to `f32` is `to_f32()` returning plain arrays, called at the
//!   render seam only (mesh_core's buffers). euclase's `normalize` returns zero for a zero vector; here it is
//!   [`Degenerate::ZeroVector`].
//! * One tolerance policy (BP-vug-2): every comparison that needs an epsilon takes a [`Tol`].
//! * Every fn is pure and never panics on degenerate input: a named [`Degenerate`], a named outcome enum, or a
//!   [`Predicate::Uncertain`].
//! * [`predicates`]: Shewchuk's orient2d / orient3d / incircle as a filtered fast path with an EXACT expansion
//!   fallback, after an exact power-of-two rescale, so a sign is never wrong; the only refusals are named.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod bounds;
pub mod closest;
pub mod intersect;
pub mod mat;
pub mod polygon;
pub mod predicates;
pub mod prim;
pub mod quat;
pub mod tol;
pub mod transform;
pub mod vec;

pub use bounds::{Aabb2, Aabb3, Interval};
pub use closest::{TriFeature, closest_point_line, closest_point_plane, closest_point_segment, closest_point_triangle};
pub use intersect::{
    CircleCircle, LineCircle, LineLine2, LinePlane, PlanePlane, RayBox, RaySphere, RayTri, SegSeg2, circle_circle2,
    line_circle2, line_line2, line_plane, plane_plane, ray_aabb, ray_sphere, ray_triangle, segment_segment2,
};
pub use mat::{Mat3, Mat4};
pub use polygon::{area, centroid, is_ccw};
pub use predicates::{Predicate, Unsure, incircle, orient2d, orient3d};
pub use prim::{Arc2, Circle2, Circle3, Line, Line2, Plane, Ray, Segment, Segment2};
pub use quat::Quat;
pub use tol::{Degenerate, Tol};
pub use transform::Transform;
pub use vec::{Vec2, Vec3};
