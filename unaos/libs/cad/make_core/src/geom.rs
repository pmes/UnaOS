// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! THE GEOM SEAM — the one file through which make_core touches geometry.
//!
//! VUGSHIMSWAP (SR473): geom_core (SR464) is the geometry, by path. `Tol`, `Vec2`, `Vec3`, `Aabb3` and the polygon
//! area are its re-exports; the four fns are adapters from geom_core's primitives and named outcomes onto the shapes
//! op.rs and validate.rs call; `Length` / `Xy` name geom_core's `magnitude` and a Vec2 constructor for the existing call
//! sites (VUG-MAKE-GEOMNAMES owes their rename). Nothing outside this file writes vector math (BP-vug-2).

pub use geom_core::area as signed_area2;
pub use geom_core::{Aabb3, Tol, Vec2, Vec3};

use geom_core::{Line2, LineLine2, Ray, RayBox, SegSeg2, Segment2};

/// `length` = geom_core's `magnitude`.
pub(crate) trait Length {
    fn length(self) -> f64;
}

impl Length for Vec2 {
    fn length(self) -> f64 {
        self.magnitude()
    }
}

impl Length for Vec3 {
    fn length(self) -> f64 {
        self.magnitude()
    }
}

/// The plan view of a point.
pub(crate) trait Xy {
    fn xy(self) -> Vec2;
}

impl Xy for Vec3 {
    fn xy(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }
}

/// The intersection of the lines `p + t·d` and `q + s·e` (geom_core's `line_line2` on unit directions, so its
/// `tol.ang` test is the sine of the angle between them); `None` when parallel, coincident or a direction is zero.
pub fn line_line2(p: Vec2, d: Vec2, q: Vec2, e: Vec2, tol: Tol) -> Option<Vec2> {
    let (a, b) = (Line2::new(p, d).ok()?, Line2::new(q, e).ok()?);
    match geom_core::line_line2(&a, &b, tol) {
        LineLine2::Point { p, .. } => Some(p),
        LineLine2::Parallel | LineLine2::Coincident => None,
    }
}

/// Whether the closed segments `ab` and `cd` meet (geom_core's `segment_segment2`: touching within `tol.lin` counts).
/// A segment geom_core refuses (no longer than `tol.lin`, or non-finite) answers `true`: the caller refuses the
/// contour rather than pass what cannot be checked.
pub fn seg_seg2(a: Vec2, b: Vec2, c: Vec2, d: Vec2, tol: Tol) -> bool {
    match geom_core::segment_segment2(&Segment2 { a, b }, &Segment2 { a: c, b: d }, tol) {
        Ok(SegSeg2::Disjoint) => false,
        Ok(SegSeg2::Point(_) | SegSeg2::Overlap(..)) | Err(_) => true,
    }
}

/// Whether the segment `ab` meets the box: geom_core's slab test on the ray `a + t(b - a)`, entered by `t = 1`.
pub fn seg_aabb3(a: Vec3, b: Vec3, bx: &Aabb3) -> bool {
    match geom_core::ray_aabb(&Ray { origin: a, dir: b - a }, bx) {
        RayBox::Hit { t_enter, .. } => t_enter <= 1.0,
        RayBox::Miss => false,
    }
}

// ---- VUGMAKEARCFIT (LEDGER SR490): the circle crossings offset.rs's arc pieces are split at. ----

/// Where the line through `a` and `b` meets the circle about `c` of radius `r` (geom_core's `line_circle2`): the first
/// `n` points, in the line's direction order, a tangent counted once; none for a degenerate line or circle.
pub fn line_circle2(a: Vec2, b: Vec2, c: Vec2, r: f64, tol: Tol) -> ([Vec2; 2], usize) {
    let (Ok(l), Ok(k)) = (Line2::through(a, b), geom_core::Circle2::new(c, r)) else { return ([a; 2], 0) };
    match geom_core::line_circle2(&l, &k, tol) {
        geom_core::LineCircle::Two(p, q) => ([p, q], 2),
        geom_core::LineCircle::Tangent(p) => ([p, p], 1),
        geom_core::LineCircle::Miss => ([a; 2], 0),
    }
}

/// Where the circles about `c1` (radius `r1`) and `c2` (radius `r2`) meet (geom_core's `circle_circle2`): the first `n`
/// points, a tangent counted once; none when apart, nested, coincident or degenerate.
pub fn circle_circle2(c1: Vec2, r1: f64, c2: Vec2, r2: f64, tol: Tol) -> ([Vec2; 2], usize) {
    let (Ok(a), Ok(b)) = (geom_core::Circle2::new(c1, r1), geom_core::Circle2::new(c2, r2)) else { return ([c1; 2], 0) };
    match geom_core::circle_circle2(&a, &b, tol) {
        geom_core::CircleCircle::Two(p, q) => ([p, q], 2),
        geom_core::CircleCircle::Tangent(p) => ([p, p], 1),
        _ => ([c1; 2], 0),
    }
}
