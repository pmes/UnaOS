//! The ONE geometry seam of mesh_core: every vector, tolerance, box, ray and the two intersections mesh_core uses come
//! through this file and nowhere else (BP-vug-1, BP-vug-2: no private epsilon, every threshold is a `Tol` field).
//!
//! SHIM: geom_core (LEDGER SR464, branch exec-rmbp-vuggeom) had no commit when VUGMESH (SR466) built, so this file
//! carries the minimal subset itself. The seat swaps this ONE file for `pub use geom_core::{Vec3, Tol, Ray, Aabb3};`
//! plus thin adapters for `comp`, `aabb_of`, `aabb_union`, `ray_triangle` and `ray_aabb` onto geom_core's names; no
//! other file in the crate defines geometry.

use core::ops::{Add, Mul, Neg, Sub};

/// An f64 3-vector (the euclase names: `dot`, `cross`, `mag_sq`, `magnitude`, `normalize`, `lerp`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3 { x, y, z }
    }
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Self) -> Self {
        Vec3::new(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    pub fn mag_sq(self) -> f64 {
        self.dot(self)
    }
    pub fn magnitude(self) -> f64 {
        libm::sqrt(self.mag_sq())
    }
    /// The unit vector, or ZERO for a zero (or non-finite) vector: never a NaN out of a zero in.
    pub fn normalize(self) -> Self {
        let m = self.magnitude();
        if m > 0.0 && m.is_finite() { self * (1.0 / m) } else { Vec3::ZERO }
    }
    pub fn lerp(self, o: Self, t: f64) -> Self {
        self + (o - self) * t
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f64> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f64) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}
impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// The one tolerance policy (BP-vug-2): `lin` in millimetres, `ang` in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tol {
    pub lin: f64,
    pub ang: f64,
}

impl Tol {
    /// geom_core's launch values (1e-6 mm, 1e-9 rad); geom_core's own doc carries the reasoning.
    pub const DEFAULT: Tol = Tol { lin: 1e-6, ang: 1e-9 };
}

/// A ray `origin + t * dir`, t >= 0 (dir need not be unit; t is in units of |dir|).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

/// An axis-aligned box, `min <= max` per axis when built from points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb3 {
    pub min: Vec3,
    pub max: Vec3,
}

/// Component `i` (0 = x, 1 = y, else z).
pub(crate) fn comp(v: Vec3, i: usize) -> f64 {
    match i {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

/// The box of a non-empty point set (`None` for an empty set).
pub(crate) fn aabb_of(pts: &[Vec3]) -> Option<Aabb3> {
    let (first, rest) = pts.split_first()?;
    let mut b = Aabb3 { min: *first, max: *first };
    for p in rest {
        b = aabb_union(b, Aabb3 { min: *p, max: *p });
    }
    Some(b)
}

pub(crate) fn aabb_union(a: Aabb3, b: Aabb3) -> Aabb3 {
    Aabb3 {
        min: Vec3::new(a.min.x.min(b.min.x), a.min.y.min(b.min.y), a.min.z.min(b.min.z)),
        max: Vec3::new(a.max.x.max(b.max.x), a.max.y.max(b.max.y), a.max.z.max(b.max.z)),
    }
}

/// Moller-Trumbore, two-sided: `Some((t, u, v))` with t >= 0 and the hit `a + u(b-a) + v(c-a)`. A ray within `tol.ang`
/// of the triangle's plane (|det| <= tol.ang * |e1 x e2| * |dir|, the sine of the ray/plane angle) or a degenerate
/// triangle (|e1 x e2| = 0) is no hit.
pub(crate) fn ray_triangle(ray: &Ray, a: Vec3, b: Vec3, c: Vec3, tol: Tol) -> Option<(f64, f64, f64)> {
    let e1 = b - a;
    let e2 = c - a;
    let p = ray.dir.cross(e2);
    let det = e1.dot(p);
    let scale = e1.cross(e2).magnitude() * ray.dir.magnitude();
    if !(det.abs() > tol.ang * scale) || scale == 0.0 {
        return None;
    }
    let inv = 1.0 / det;
    let s = ray.origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = ray.dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    if !(t >= 0.0) {
        return None;
    }
    Some((t, u, v))
}

/// The slab test: `Some((t_near, t_far))` where the ray's line meets the box with t_far >= max(t_near, 0); an axis the
/// ray is parallel to (dir component exactly 0) is a containment test on the origin.
pub(crate) fn ray_aabb(ray: &Ray, b: &Aabb3) -> Option<(f64, f64)> {
    let mut tn = f64::NEG_INFINITY;
    let mut tf = f64::INFINITY;
    for i in 0..3 {
        let o = comp(ray.origin, i);
        let d = comp(ray.dir, i);
        let (lo, hi) = (comp(b.min, i), comp(b.max, i));
        if d == 0.0 {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d;
        let (mut t0, mut t1) = ((lo - o) * inv, (hi - o) * inv);
        if t0 > t1 {
            core::mem::swap(&mut t0, &mut t1);
        }
        tn = tn.max(t0);
        tf = tf.min(t1);
    }
    if tn <= tf && tf >= 0.0 { Some((tn, tf)) } else { None }
}
