// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The f64 geometry scan_core uses: geom_core's (SR464), by path since VUGSHIMSWAP (SR473). `Vec3`, `Tol`, `Plane`
//! and `Transform` are its re-exports; [`from_axes`] is the one adapter, the camera frame from its axes, because
//! geom_core's `Transform` holds a quaternion and geom_core has no matrix -> quaternion yet (VUG-GEOM-FRAME owes it).

pub use geom_core::{Plane, Tol, Transform, Vec3};

use geom_core::Quat;

/// The rigid frame whose axes are `x`, `y`, `z` (the columns of its rotation, orthonormal and right-handed) at
/// `origin`: local -> world. Shepperd's method (the branch on the largest of `1 + trace` and the three diagonal
/// terms, so the quaternion is never zero), normalised; scale 1.
pub fn from_axes(x: Vec3, y: Vec3, z: Vec3, origin: Vec3) -> Transform {
    // m[row][col]: column 0 is x, column 1 is y, column 2 is z.
    let (m00, m01, m02) = (x.x, y.x, z.x);
    let (m10, m11, m12) = (x.y, y.y, z.y);
    let (m20, m21, m22) = (x.z, y.z, z.z);
    let tr = m00 + m11 + m22;
    let q = if tr > 0.0 {
        let s = libm::sqrt(tr + 1.0) * 2.0;
        Quat::new((m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s)
    } else if m00 > m11 && m00 > m22 {
        let s = libm::sqrt(1.0 + m00 - m11 - m22) * 2.0;
        Quat::new(0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s)
    } else if m11 > m22 {
        let s = libm::sqrt(1.0 + m11 - m00 - m22) * 2.0;
        Quat::new((m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s)
    } else {
        let s = libm::sqrt(1.0 + m22 - m00 - m11) * 2.0;
        Quat::new((m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s)
    };
    Transform { rot: q.normalize().unwrap_or(Quat::IDENTITY), scale: 1.0, trans: origin }
}
