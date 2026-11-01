//! The primitives: rays, lines, segments, planes, circles and arcs. Constructors normalise directions and refuse a
//! zero direction, a non-positive radius or a non-finite input by [`Degenerate`].

use core::f64::consts::TAU;

use crate::tol::{Degenerate, Tol};
use crate::vec::{Vec2, Vec3};

fn finite(ok: bool) -> Result<(), Degenerate> {
    if ok { Ok(()) } else { Err(Degenerate::NonFinite) }
}

/// A half-line `origin + t * dir`, `t >= 0`; `dir` is unit so `t` is millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn new(origin: Vec3, dir: Vec3) -> Result<Self, Degenerate> {
        finite(origin.is_finite())?;
        Ok(Ray { origin, dir: dir.normalize()? })
    }
    pub fn at(self, t: f64) -> Vec3 {
        self.origin + self.dir * t
    }
}

/// An infinite line `point + t * dir`, `dir` unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub point: Vec3,
    pub dir: Vec3,
}

impl Line {
    pub fn new(point: Vec3, dir: Vec3) -> Result<Self, Degenerate> {
        finite(point.is_finite())?;
        Ok(Line { point, dir: dir.normalize()? })
    }
    pub fn through(a: Vec3, b: Vec3) -> Result<Self, Degenerate> {
        Self::new(a, b - a)
    }
    pub fn at(self, t: f64) -> Vec3 {
        self.point + self.dir * t
    }
}

/// A 3D segment from `a` to `b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: Vec3,
    pub b: Vec3,
}

impl Segment {
    pub const fn new(a: Vec3, b: Vec3) -> Self {
        Segment { a, b }
    }
    pub fn len(self) -> f64 {
        self.a.dist(self.b)
    }
    /// `a + t * (b - a)`.
    pub fn at(self, t: f64) -> Vec3 {
        self.a.lerp(self.b, t)
    }
}

/// An infinite 2D line `point + t * dir`, `dir` unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line2 {
    pub point: Vec2,
    pub dir: Vec2,
}

impl Line2 {
    pub fn new(point: Vec2, dir: Vec2) -> Result<Self, Degenerate> {
        finite(point.is_finite())?;
        Ok(Line2 { point, dir: dir.normalize()? })
    }
    pub fn through(a: Vec2, b: Vec2) -> Result<Self, Degenerate> {
        Self::new(a, b - a)
    }
    pub fn at(self, t: f64) -> Vec2 {
        self.point + self.dir * t
    }
}

/// A 2D segment from `a` to `b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment2 {
    pub a: Vec2,
    pub b: Vec2,
}

impl Segment2 {
    pub const fn new(a: Vec2, b: Vec2) -> Self {
        Segment2 { a, b }
    }
    pub fn len(self) -> f64 {
        self.a.dist(self.b)
    }
    pub fn at(self, t: f64) -> Vec2 {
        self.a.lerp(self.b, t)
    }
}

/// The plane `normal . p = d`, `normal` unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub d: f64,
}

impl Plane {
    pub fn from_point_normal(p: Vec3, n: Vec3) -> Result<Self, Degenerate> {
        finite(p.is_finite())?;
        let normal = n.normalize()?;
        Ok(Plane { normal, d: normal.dot(p) })
    }
    /// The plane through three points, its normal `(b - a) x (c - a)`; refuses collinear points (`b` within
    /// `tol.lin` of `a`, or `c` within `tol.lin` of the line `ab`).
    pub fn from_points(a: Vec3, b: Vec3, c: Vec3, tol: Tol) -> Result<Self, Degenerate> {
        finite(a.is_finite() && b.is_finite() && c.is_finite())?;
        let ab = b - a;
        let l = ab.magnitude();
        let n = ab.cross(c - a);
        if l <= tol.lin || n.magnitude() / l <= tol.lin {
            return Err(Degenerate::Collinear);
        }
        Self::from_point_normal(a, n)
    }
    /// Positive on the side the normal points to.
    pub fn signed_distance(self, p: Vec3) -> f64 {
        self.normal.dot(p) - self.d
    }
    pub fn project(self, p: Vec3) -> Vec3 {
        p - self.normal * self.signed_distance(p)
    }
}

/// A full circle in the plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle2 {
    pub centre: Vec2,
    pub radius: f64,
}

impl Circle2 {
    pub fn new(centre: Vec2, radius: f64) -> Result<Self, Degenerate> {
        finite(centre.is_finite() && radius.is_finite())?;
        if radius <= 0.0 {
            return Err(Degenerate::ZeroRadius);
        }
        Ok(Circle2 { centre, radius })
    }
    pub fn point_at(self, theta: f64) -> Vec2 {
        self.centre + Vec2::new(libm::cos(theta), libm::sin(theta)) * self.radius
    }
}

/// A circular arc: from angle `start` (radians, from +x) through the signed `sweep` (positive counterclockwise).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc2 {
    pub centre: Vec2,
    pub radius: f64,
    pub start: f64,
    pub sweep: f64,
}

impl Arc2 {
    /// Refuses a non-positive radius, a zero sweep and a sweep past a full turn.
    pub fn new(centre: Vec2, radius: f64, start: f64, sweep: f64) -> Result<Self, Degenerate> {
        finite(centre.is_finite() && radius.is_finite() && start.is_finite() && sweep.is_finite())?;
        if radius <= 0.0 {
            return Err(Degenerate::ZeroRadius);
        }
        if sweep == 0.0 || sweep.abs() > TAU {
            return Err(Degenerate::ZeroVector);
        }
        Ok(Arc2 { centre, radius, start, sweep })
    }
    /// `t` in [0, 1] along the sweep.
    pub fn point_at(self, t: f64) -> Vec2 {
        let a = self.start + self.sweep * t;
        self.centre + Vec2::new(libm::cos(a), libm::sin(a)) * self.radius
    }
    pub fn start_point(self) -> Vec2 {
        self.point_at(0.0)
    }
    pub fn end_point(self) -> Vec2 {
        self.point_at(1.0)
    }
    pub fn len(self) -> f64 {
        self.radius * self.sweep.abs()
    }
    /// Whether the direction `theta` (radians) lies on the arc, within `tol.ang`.
    pub fn contains_angle(self, theta: f64, tol: Tol) -> bool {
        if self.sweep.abs() >= TAU - tol.ang {
            return true;
        }
        // The angle from the start, measured in the sweep's direction, folded into [0, TAU).
        let rel = if self.sweep > 0.0 { theta - self.start } else { self.start - theta };
        let mut r = rel % TAU;
        if r < 0.0 {
            r += TAU;
        }
        r <= self.sweep.abs() + tol.ang || r >= TAU - tol.ang
    }
}

/// A circle in space: centre, unit normal of its plane, radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle3 {
    pub centre: Vec3,
    pub normal: Vec3,
    pub radius: f64,
}

impl Circle3 {
    pub fn new(centre: Vec3, normal: Vec3, radius: f64) -> Result<Self, Degenerate> {
        finite(centre.is_finite() && radius.is_finite())?;
        if radius <= 0.0 {
            return Err(Degenerate::ZeroRadius);
        }
        Ok(Circle3 { centre, normal: normal.normalize()?, radius })
    }
    /// The point at angle `theta` in the frame (`u`, `normal x u`), `u` = `normal.any_perpendicular()`.
    pub fn point_at(self, theta: f64) -> Vec3 {
        let u = self.normal.any_perpendicular().unwrap_or(Vec3::X);
        let v = self.normal.cross(u);
        self.centre + (u * libm::cos(theta) + v * libm::sin(theta)) * self.radius
    }
    pub fn plane(self) -> Plane {
        Plane { normal: self.normal, d: self.normal.dot(self.centre) }
    }
}
