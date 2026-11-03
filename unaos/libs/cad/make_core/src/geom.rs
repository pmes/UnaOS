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
