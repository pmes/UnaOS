//! The ONE geometry seam of mesh_core: every vector, tolerance, box, ray and the two intersections mesh_core uses come
//! through this file and nowhere else (BP-vug-1, BP-vug-2: no private epsilon, every threshold is a `Tol` field).
//!
//! VUGSHIMSWAP (SR473): geom_core (SR464) is the geometry, by path. The types are its re-exports; the fns below are
//! thin adapters from geom_core's names and named outcomes onto the shapes bvh.rs and lib.rs call. The intersections
//! are geom_core's definitions (BP-vug-2): a triangle hit within `tol.lin` of an edge counts, `t >= -tol.lin` counts,
//! a triangle whose height is at most `tol.lin` is no hit; a box entry is clamped at `t = 0`.

pub use geom_core::{Aabb3, Mat4, Ray, Tol, Vec3};

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

// VUGSCANDEVIATION (SR510): the closest-point query's two geometry calls (`Bvh::closest`), through this seam.

/// geom_core's closest point of triangle `abc` to `p` (Ericson §5.1.5); a degenerate or non-finite triangle is `None`.
pub(crate) fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3, tol: Tol) -> Option<Vec3> {
    geom_core::closest_point_triangle(p, a, b, c, tol).ok().map(|(q, _)| q)
}

/// The squared distance from `p` to the box (0 inside it).
pub(crate) fn aabb_dist_sq(p: Vec3, b: &Aabb3) -> f64 {
    let d = |x: f64, lo: f64, hi: f64| if x < lo { lo - x } else if x > hi { x - hi } else { 0.0 };
    let (dx, dy, dz) = (d(p.x, b.min.x, b.max.x), d(p.y, b.min.y, b.max.y), d(p.z, b.min.z, b.max.z));
    dx * dx + dy * dy + dz * dz
}
