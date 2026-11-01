//! The analytic surfaces, each written in a `Frame` with its own (u, v) parametrization, and the closed-form
//! surface/surface intersections. u is the angle about f.z on every surface of revolution.
//!
//! - Plane    S = o + u x + v y                                  (normal z)
//! - Cylinder S = o + r e(u) + v z                                (e(u) = x cos u + y sin u)
//! - Cone     S = o + (r0 + k v) e(u) + v z                       (k = dr/dv; the apex where r0 + k v = 0)
//! - Sphere   S = o + r (cos v e(u) + sin v z)                    (v the latitude in [-π/2, π/2])
//! - Torus    S = o + (R + r cos v) e(u) + r sin v z
//!
//! On each, S_u × S_v points away from the axis (outward for a solid of revolution with ρ > 0); a face whose material
//! lies the other way sets `Face::reversed`.

use crate::Refusal;
use crate::curve::{Curve, Frame};
use crate::dep::{Circle2, CircleCircle, Plane, PlanePlane, Tol, Vec2, Vec3, circle_circle2, plane_plane};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Torus,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Surface {
    Plane { f: Frame },
    Cylinder { f: Frame, r: f64 },
    Cone { f: Frame, r0: f64, k: f64 },
    Sphere { f: Frame, r: f64 },
    Torus { f: Frame, major: f64, minor: f64 },
}

fn e(f: &Frame, u: f64) -> Vec3 {
    f.x * libm::cos(u) + f.y * libm::sin(u)
}
fn e_du(f: &Frame, u: f64) -> Vec3 {
    f.y * libm::cos(u) - f.x * libm::sin(u)
}

impl Surface {
    pub fn kind(&self) -> SurfaceKind {
        match self {
            Surface::Plane { .. } => SurfaceKind::Plane,
            Surface::Cylinder { .. } => SurfaceKind::Cylinder,
            Surface::Cone { .. } => SurfaceKind::Cone,
            Surface::Sphere { .. } => SurfaceKind::Sphere,
            Surface::Torus { .. } => SurfaceKind::Torus,
        }
    }

    pub fn frame(&self) -> &Frame {
        match self {
            Surface::Plane { f }
            | Surface::Cylinder { f, .. }
            | Surface::Cone { f, .. }
            | Surface::Sphere { f, .. }
            | Surface::Torus { f, .. } => f,
        }
    }

    pub fn point(&self, u: f64, v: f64) -> Vec3 {
        match self {
            Surface::Plane { f } => f.o + f.x * u + f.y * v,
            Surface::Cylinder { f, r } => f.o + e(f, u) * *r + f.z * v,
            Surface::Cone { f, r0, k } => f.o + e(f, u) * (*r0 + *k * v) + f.z * v,
            Surface::Sphere { f, r } => f.o + (e(f, u) * libm::cos(v) + f.z * libm::sin(v)) * *r,
            Surface::Torus { f, major, minor } => {
                f.o + e(f, u) * (*major + *minor * libm::cos(v)) + f.z * (*minor * libm::sin(v))
            }
        }
    }

    /// ∂S/∂u.
    pub fn du(&self, u: f64, v: f64) -> Vec3 {
        match self {
            Surface::Plane { f } => f.x,
            Surface::Cylinder { f, r } => e_du(f, u) * *r,
            Surface::Cone { f, r0, k } => e_du(f, u) * (*r0 + *k * v),
            Surface::Sphere { f, r } => e_du(f, u) * (*r * libm::cos(v)),
            Surface::Torus { f, major, minor } => e_du(f, u) * (*major + *minor * libm::cos(v)),
        }
    }

    /// ∂S/∂v.
    pub fn dv(&self, u: f64, v: f64) -> Vec3 {
        match self {
            Surface::Plane { f } => f.y,
            Surface::Cylinder { f, .. } => f.z,
            Surface::Cone { f, k, .. } => e(f, u) * *k + f.z,
            Surface::Sphere { f, r } => (f.z * libm::cos(v) - e(f, u) * libm::sin(v)) * *r,
            Surface::Torus { f, minor, .. } => (f.z * libm::cos(v) - e(f, u) * libm::sin(v)) * *minor,
        }
    }

    /// The unit normal S_u × S_v / |S_u × S_v|; refused at a singular point (a pole, an apex).
    pub fn normal(&self, u: f64, v: f64) -> Result<Vec3, Refusal> {
        if let Surface::Plane { f } = self {
            return Ok(f.z);
        }
        Ok(self.du(u, v).cross(self.dv(u, v)).normalize()?)
    }

    /// The distance from `p` to the surface, in closed form per kind (the cone's is the meridian distance to its
    /// generator line, exact on the nappe the faces use).
    pub fn distance(&self, p: Vec3) -> f64 {
        let (a, b, c) = self.frame().local(p);
        let rho = libm::hypot(a, b);
        match self {
            Surface::Plane { .. } => libm::fabs(c),
            Surface::Cylinder { r, .. } => libm::fabs(rho - *r),
            Surface::Cone { r0, k, .. } => libm::fabs(rho - (*r0 + *k * c)) / libm::sqrt(1.0 + *k * *k),
            Surface::Sphere { r, .. } => libm::fabs(libm::sqrt(a * a + b * b + c * c) - *r),
            Surface::Torus { major, minor, .. } => libm::fabs(libm::hypot(rho - *major, c) - *minor),
        }
    }
}

/// The outcome of a surface/surface intersection.
#[derive(Clone, Debug, PartialEq)]
pub enum Intersection {
    Empty,
    Point(Vec3),
    Coincident,
    Curves(Vec<Curve>),
}

fn rank(k: SurfaceKind) -> u8 {
    match k {
        SurfaceKind::Plane => 0,
        SurfaceKind::Cylinder => 1,
        SurfaceKind::Sphere => 2,
        SurfaceKind::Cone => 3,
        SurfaceKind::Torus => 4,
    }
}

/// The closed-form intersection of two analytic surfaces (as unbounded surfaces). Built: plane/plane (geom_core
/// `plane_plane`), plane/cylinder (circle, zero / one / two lines, or an ellipse), plane/sphere (circle, point, empty),
/// cylinder/cylinder with parallel axes (geom_core `circle_circle2` in the cross-section). Every other pair is
/// `NotClosedForm` by name (VUG-BREP-SSI owed).
pub fn intersect(a: &Surface, b: &Surface, tol: Tol) -> Result<Intersection, Refusal> {
    let (a, b) = if rank(a.kind()) <= rank(b.kind()) { (a, b) } else { (b, a) };
    match (a, b) {
        (Surface::Plane { f: p }, Surface::Plane { f: q }) => {
            let pa = Plane::from_point_normal(p.o, p.z)?;
            let pb = Plane::from_point_normal(q.o, q.z)?;
            Ok(match plane_plane(&pa, &pb, tol)? {
                PlanePlane::Line(l) => Intersection::Curves(vec![Curve::Line { o: l.point, dir: l.dir }]),
                PlanePlane::Parallel => Intersection::Empty,
                PlanePlane::Coincident => Intersection::Coincident,
            })
        }
        (Surface::Plane { f: p }, Surface::Cylinder { f: c, r }) => plane_cylinder(p, c, *r, tol),
        (Surface::Plane { f: p }, Surface::Sphere { f: s, r }) => {
            let n = p.z;
            let delta = n.dot(s.o - p.o);
            let foot = s.o - n * delta;
            if libm::fabs(delta) > *r + tol.lin {
                Ok(Intersection::Empty)
            } else if libm::fabs(libm::fabs(delta) - *r) <= tol.lin {
                Ok(Intersection::Point(foot))
            } else {
                let rr = libm::sqrt(*r * *r - delta * delta);
                Ok(Intersection::Curves(vec![Curve::Circle { f: Frame::new(foot, n, p.x)?, r: rr }]))
            }
        }
        (Surface::Cylinder { f: c1, r: r1 }, Surface::Cylinder { f: c2, r: r2 }) => {
            if c1.z.cross(c2.z).mag() > tol.ang {
                return Err(Refusal::NotClosedForm { a: SurfaceKind::Cylinder, b: SurfaceKind::Cylinder });
            }
            let (x, y, _) = c1.local(c2.o);
            let k1 = Circle2 { centre: Vec2::new(0.0, 0.0), radius: *r1 };
            let k2 = Circle2 { centre: Vec2::new(x, y), radius: *r2 };
            let line = |q: Vec2| Curve::Line { o: c1.at(q.x, q.y, 0.0), dir: c1.z };
            Ok(match circle_circle2(&k1, &k2, tol)? {
                CircleCircle::Two(p, q) => Intersection::Curves(vec![line(p), line(q)]),
                CircleCircle::Tangent(p) => Intersection::Curves(vec![line(p)]),
                CircleCircle::Separate | CircleCircle::Contained => Intersection::Empty,
                CircleCircle::Coincident => Intersection::Coincident,
            })
        }
        _ => Err(Refusal::NotClosedForm { a: a.kind(), b: b.kind() }),
    }
}

fn plane_cylinder(p: &Frame, c: &Frame, r: f64, tol: Tol) -> Result<Intersection, Refusal> {
    let n = p.z;
    let cz = n.dot(c.z);
    let sin = n.cross(c.z).mag();
    if sin <= tol.ang {
        // Axis perpendicular to the plane: a circle of radius r.
        let t = n.dot(p.o - c.o) / cz;
        let centre = c.o + c.z * t;
        return Ok(Intersection::Curves(vec![Curve::Circle { f: Frame::new(centre, n, c.x)?, r }]));
    }
    if libm::fabs(cz) <= tol.ang {
        // Axis parallel to the plane: zero, one or two generator lines.
        let delta = n.dot(c.o - p.o);
        let foot = c.o - n * delta;
        if libm::fabs(delta) > r + tol.lin {
            return Ok(Intersection::Empty);
        }
        if libm::fabs(libm::fabs(delta) - r) <= tol.lin {
            return Ok(Intersection::Curves(vec![Curve::Line { o: foot, dir: c.z }]));
        }
        let h = libm::sqrt(r * r - delta * delta);
        let w = n.cross(c.z).normalize()?;
        return Ok(Intersection::Curves(vec![
            Curve::Line { o: foot + w * h, dir: c.z },
            Curve::Line { o: foot - w * h, dir: c.z },
        ]));
    }
    // Oblique: an ellipse centred where the axis meets the plane, semi-minor r across the axis, semi-major r / |n·z|.
    let t = n.dot(p.o - c.o) / cz;
    let centre = c.o + c.z * t;
    let minor_dir = c.z.cross(n).normalize()?;
    let major_dir = n.cross(minor_dir);
    let f = Frame::new(centre, n, major_dir)?;
    Ok(Intersection::Curves(vec![Curve::Ellipse { f, a: r / libm::fabs(cz), b: r }]))
}
