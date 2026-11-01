//! Closed-form intersections with named outcomes. Every epsilon comes from the `Tol` argument (BP-vug-2): a
//! parallel test compares the sine of the angle with `tol.ang`, a coincidence or tangency test compares a distance
//! with `tol.lin`. Raw input that cannot be computed on (a zero-length segment, a zero-area triangle, a
//! non-positive radius) is an `Err(Degenerate)`.

use crate::bounds::Aabb3;
use crate::prim::{Circle2, Line, Line2, Plane, Ray, Segment2};
use crate::tol::{Degenerate, Tol};
use crate::vec::{Vec2, Vec3};

/// Two infinite 2D lines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineLine2 {
    /// They cross at `p = a.at(t) = b.at(u)`.
    Point { p: Vec2, t: f64, u: f64 },
    Parallel,
    Coincident,
}

pub fn line_line2(a: &Line2, b: &Line2, tol: Tol) -> LineLine2 {
    let denom = a.dir.cross(b.dir);
    let w = b.point - a.point;
    if denom.abs() <= tol.ang {
        return if a.dir.cross(w).abs() <= tol.lin { LineLine2::Coincident } else { LineLine2::Parallel };
    }
    let t = w.cross(b.dir) / denom;
    let u = w.cross(a.dir) / denom;
    LineLine2::Point { p: a.at(t), t, u }
}

/// Two 2D segments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SegSeg2 {
    Disjoint,
    /// They meet at one point (a crossing, a touch, or a collinear overlap no longer than `tol.lin`).
    Point(Vec2),
    /// Collinear and overlapping along the segment between the two points.
    Overlap(Vec2, Vec2),
}

/// Refuses a segment no longer than `tol.lin` (`Degenerate::ZeroVector`).
pub fn segment_segment2(s1: &Segment2, s2: &Segment2, tol: Tol) -> Result<SegSeg2, Degenerate> {
    if !(s1.a.is_finite() && s1.b.is_finite() && s2.a.is_finite() && s2.b.is_finite()) {
        return Err(Degenerate::NonFinite);
    }
    let r = s1.b - s1.a;
    let s = s2.b - s2.a;
    let (l1, l2) = (r.magnitude(), s.magnitude());
    if l1 <= tol.lin || l2 <= tol.lin {
        return Err(Degenerate::ZeroVector);
    }
    let w = s2.a - s1.a;
    let denom = r.cross(s);
    if (denom / (l1 * l2)).abs() <= tol.ang {
        if (r.cross(w) / l1).abs() > tol.lin {
            return Ok(SegSeg2::Disjoint);
        }
        // Collinear: project s2 onto s1's parameter.
        let l1sq = l1 * l1;
        let t0 = w.dot(r) / l1sq;
        let t1 = (s2.b - s1.a).dot(r) / l1sq;
        let (lo, hi) = (t0.min(t1).max(0.0), t0.max(t1).min(1.0));
        let slack = tol.lin / l1;
        if lo > hi + slack {
            return Ok(SegSeg2::Disjoint);
        }
        if (hi - lo) * l1 <= tol.lin {
            return Ok(SegSeg2::Point(s1.at(((lo + hi) * 0.5).clamp(0.0, 1.0))));
        }
        return Ok(SegSeg2::Overlap(s1.at(lo), s1.at(hi)));
    }
    let t = w.cross(s) / denom;
    let u = w.cross(r) / denom;
    let (et, eu) = (tol.lin / l1, tol.lin / l2);
    if t < -et || t > 1.0 + et || u < -eu || u > 1.0 + eu {
        return Ok(SegSeg2::Disjoint);
    }
    Ok(SegSeg2::Point(s1.at(t.clamp(0.0, 1.0))))
}

/// A line and a plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LinePlane {
    /// They meet at `p = line.at(t)`.
    Point { p: Vec3, t: f64 },
    Parallel,
    InPlane,
}

pub fn line_plane(l: &Line, pl: &Plane, tol: Tol) -> LinePlane {
    let denom = pl.normal.dot(l.dir);
    let dist = pl.signed_distance(l.point);
    if denom.abs() <= tol.ang {
        return if dist.abs() <= tol.lin { LinePlane::InPlane } else { LinePlane::Parallel };
    }
    let t = -dist / denom;
    LinePlane::Point { p: l.at(t), t }
}

/// Two planes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlanePlane {
    /// They meet along this line, its direction `a.normal x b.normal` (normalised).
    Line(Line),
    Parallel,
    Coincident,
}

pub fn plane_plane(a: &Plane, b: &Plane, tol: Tol) -> PlanePlane {
    let dir = a.normal.cross(b.normal);
    let m2 = dir.mag_sq();
    if libm::sqrt(m2) <= tol.ang {
        let gap = if a.normal.dot(b.normal) > 0.0 { a.d - b.d } else { a.d + b.d };
        return if gap.abs() <= tol.lin { PlanePlane::Coincident } else { PlanePlane::Parallel };
    }
    // The point of the line nearest the origin: n1 . p = d1, n2 . p = d2, dir . p = 0.
    let p = (b.normal.cross(dir) * a.d + dir.cross(a.normal) * b.d) / m2;
    match Line::new(p, dir) {
        Ok(l) => PlanePlane::Line(l),
        Err(_) => PlanePlane::Parallel,
    }
}

/// A ray and a triangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RayTri {
    /// `p = ray.at(t) = (1 - u - v) a + u b + v c`; edges and vertices count as hits.
    Hit { t: f64, u: f64, v: f64, p: Vec3 },
    Miss,
    /// The ray runs in the triangle's plane direction (within `tol.ang`).
    Parallel,
}

fn tri_degenerate(a: Vec3, b: Vec3, c: Vec3, tol: Tol) -> Result<(Vec3, Vec3, f64), Degenerate> {
    if !(a.is_finite() && b.is_finite() && c.is_finite()) {
        return Err(Degenerate::NonFinite);
    }
    let (e1, e2) = (b - a, c - a);
    let l = e1.magnitude().max(e2.magnitude()).max((c - b).magnitude());
    // The triangle's smallest height (twice the area over the longest edge) must exceed tol.lin.
    if l <= tol.lin || e1.cross(e2).magnitude() / l <= tol.lin {
        return Err(Degenerate::ZeroArea);
    }
    Ok((e1, e2, l))
}

/// Moller-Trumbore. A hit within `tol.lin` of an edge or behind the origin by at most `tol.lin` counts.
pub fn ray_triangle(ray: &Ray, a: Vec3, b: Vec3, c: Vec3, tol: Tol) -> Result<RayTri, Degenerate> {
    let (e1, e2, l) = tri_degenerate(a, b, c, tol)?;
    let pvec = ray.dir.cross(e2);
    let det = e1.dot(pvec);
    if det.abs() <= tol.ang * e1.cross(e2).magnitude() {
        return Ok(RayTri::Parallel);
    }
    let inv = 1.0 / det;
    let tvec = ray.origin - a;
    let u = tvec.dot(pvec) * inv;
    let qvec = tvec.cross(e1);
    let v = ray.dir.dot(qvec) * inv;
    let t = e2.dot(qvec) * inv;
    let slack = tol.lin / l;
    if u < -slack || v < -slack || u + v > 1.0 + slack || t < -tol.lin {
        return Ok(RayTri::Miss);
    }
    Ok(RayTri::Hit { t, u, v, p: ray.at(t) })
}

/// A ray and an axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RayBox {
    /// The ray is inside the box for `t` in `[t_enter, t_exit]`; `t_enter = 0` when the origin is inside.
    Hit { t_enter: f64, t_exit: f64 },
    Miss,
}

/// The slab test; a zero direction component is the in-slab test (never a 0/0). Touching a face is a hit.
pub fn ray_aabb(ray: &Ray, b: &Aabb3) -> RayBox {
    let o = [ray.origin.x, ray.origin.y, ray.origin.z];
    let d = [ray.dir.x, ray.dir.y, ray.dir.z];
    let lo = [b.min.x, b.min.y, b.min.z];
    let hi = [b.max.x, b.max.y, b.max.z];
    let (mut tmin, mut tmax) = (0.0f64, f64::INFINITY);
    for i in 0..3 {
        if d[i] == 0.0 {
            if o[i] < lo[i] || o[i] > hi[i] {
                return RayBox::Miss;
            }
            continue;
        }
        let (mut t1, mut t2) = ((lo[i] - o[i]) / d[i], (hi[i] - o[i]) / d[i]);
        if t1 > t2 {
            core::mem::swap(&mut t1, &mut t2);
        }
        tmin = tmin.max(t1);
        tmax = tmax.min(t2);
        if tmin > tmax {
            return RayBox::Miss;
        }
    }
    RayBox::Hit { t_enter: tmin, t_exit: tmax }
}

/// A ray and a sphere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RaySphere {
    /// The line meets the sphere at `t_near <= t_far`; `t_near < 0` when the origin is inside.
    Hit { t_near: f64, t_far: f64 },
    /// The ray grazes the sphere (closest approach within `tol.lin` of the radius).
    Tangent { t: f64 },
    Miss,
}

pub fn ray_sphere(ray: &Ray, centre: Vec3, radius: f64, tol: Tol) -> Result<RaySphere, Degenerate> {
    if !(centre.is_finite() && radius.is_finite()) {
        return Err(Degenerate::NonFinite);
    }
    if radius <= 0.0 {
        return Err(Degenerate::ZeroRadius);
    }
    let tc = (centre - ray.origin).dot(ray.dir);
    let dist = centre.dist(ray.at(tc));
    if dist > radius + tol.lin {
        return Ok(RaySphere::Miss);
    }
    if (dist - radius).abs() <= tol.lin {
        return Ok(if tc < -tol.lin { RaySphere::Miss } else { RaySphere::Tangent { t: tc } });
    }
    let half = libm::sqrt((radius - dist) * (radius + dist));
    let (t_near, t_far) = (tc - half, tc + half);
    if t_far < -tol.lin {
        return Ok(RaySphere::Miss);
    }
    Ok(RaySphere::Hit { t_near, t_far })
}

/// Two circles in the plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CircleCircle {
    /// Two crossings; the first is on the left of the centre line from `a` to `b`.
    Two(Vec2, Vec2),
    Tangent(Vec2),
    /// Apart, outside each other.
    Separate,
    /// One inside the other without touching (or concentric with different radii).
    Contained,
    Coincident,
}

pub fn circle_circle2(a: &Circle2, b: &Circle2, tol: Tol) -> CircleCircle {
    let w = b.centre - a.centre;
    let d = w.magnitude();
    let (r1, r2) = (a.radius, b.radius);
    if d <= tol.lin {
        return if (r1 - r2).abs() <= tol.lin { CircleCircle::Coincident } else { CircleCircle::Contained };
    }
    let u = w / d;
    let outer = r1 + r2;
    let inner = (r1 - r2).abs();
    if d > outer + tol.lin {
        return CircleCircle::Separate;
    }
    if d < inner - tol.lin {
        return CircleCircle::Contained;
    }
    // Distance from a's centre to the chord, along u.
    let along = (d * d + r1 * r1 - r2 * r2) / (2.0 * d);
    if (d - outer).abs() <= tol.lin || (d - inner).abs() <= tol.lin {
        return CircleCircle::Tangent(a.centre + u * along.max(-r1).min(r1));
    }
    let h = libm::sqrt((r1 * r1 - along * along).max(0.0));
    let m = a.centre + u * along;
    CircleCircle::Two(m + u.perp() * h, m - u.perp() * h)
}

/// A 2D line and a circle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineCircle {
    /// Two crossings, in the line's direction order.
    Two(Vec2, Vec2),
    Tangent(Vec2),
    Miss,
}

pub fn line_circle2(l: &Line2, c: &Circle2, tol: Tol) -> LineCircle {
    let tc = (c.centre - l.point).dot(l.dir);
    let q = l.at(tc);
    let dist = c.centre.dist(q);
    if dist > c.radius + tol.lin {
        return LineCircle::Miss;
    }
    if (dist - c.radius).abs() <= tol.lin {
        return LineCircle::Tangent(q);
    }
    let h = libm::sqrt((c.radius - dist) * (c.radius + dist));
    LineCircle::Two(l.at(tc - h), l.at(tc + h))
}
