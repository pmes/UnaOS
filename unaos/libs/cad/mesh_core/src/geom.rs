//! The ONE geometry seam of mesh_core: every vector, tolerance, box, ray and the two intersections mesh_core uses come
//! through this file and nowhere else (BP-vug-1, BP-vug-2: no private epsilon, every threshold is a `Tol` field).
//!
//! VUGSHIMSWAP (SR473): geom_core (SR464) is the geometry, by path. The types are its re-exports; the fns below are
//! thin adapters from geom_core's names and named outcomes onto the shapes bvh.rs and lib.rs call. The intersections
//! are geom_core's definitions (BP-vug-2): a triangle hit within `tol.lin` of an edge counts, `t >= -tol.lin` counts,
//! a triangle whose height is at most `tol.lin` is no hit; a box entry is clamped at `t = 0`.

pub use geom_core::{Aabb3, Ray, Tol, Vec3};

use geom_core::{RayBox, RayTri};

/// Component `i` (0 = x, 1 = y, else z).
pub(crate) fn comp(v: Vec3, i: usize) -> f64 {
    match i {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

/// The box of a non-empty point set (`None` for an empty set): geom_core's `Aabb3::union` folded over the points (a
/// non-finite vertex is not refused here; `Mesh::validate` names it).
pub(crate) fn aabb_of(pts: &[Vec3]) -> Option<Aabb3> {
    let (first, rest) = pts.split_first()?;
    Some(rest.iter().fold(Aabb3 { min: *first, max: *first }, |b, p| b.union(Aabb3 { min: *p, max: *p })))
}

pub(crate) fn aabb_union(a: Aabb3, b: Aabb3) -> Aabb3 {
    a.union(b)
}

/// geom_core's Moller-Trumbore: `Some((t, u, v))` with the hit `a + u(b-a) + v(c-a)`; a miss, a ray parallel to the
/// triangle's plane or a degenerate triangle is `None`.
pub(crate) fn ray_triangle(ray: &Ray, a: Vec3, b: Vec3, c: Vec3, tol: Tol) -> Option<(f64, f64, f64)> {
    match geom_core::ray_triangle(ray, a, b, c, tol) {
        Ok(RayTri::Hit { t, u, v, .. }) => Some((t, u, v)),
        _ => None,
    }
}

/// geom_core's slab test: `Some((t_enter, t_exit))`, `t_enter >= 0`.
pub(crate) fn ray_aabb(ray: &Ray, b: &Aabb3) -> Option<(f64, f64)> {
    match geom_core::ray_aabb(ray, b) {
        RayBox::Hit { t_enter, t_exit } => Some((t_enter, t_exit)),
        RayBox::Miss => None,
    }
}
