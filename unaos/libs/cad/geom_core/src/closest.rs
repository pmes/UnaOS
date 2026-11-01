//! Closest points on lines, segments, planes and triangles.

use crate::prim::{Line, Plane, Segment};
use crate::tol::{Degenerate, Tol};
use crate::vec::Vec3;

/// The point of `l` nearest `p`, and its parameter.
pub fn closest_point_line(p: Vec3, l: &Line) -> (Vec3, f64) {
    let t = (p - l.point).dot(l.dir);
    (l.at(t), t)
}

/// The point of `s` nearest `p`, and its parameter in [0, 1]; a zero-length segment answers `(s.a, 0)`.
pub fn closest_point_segment(p: Vec3, s: &Segment) -> (Vec3, f64) {
    let ab = s.b - s.a;
    let l2 = ab.mag_sq();
    if l2 == 0.0 || !l2.is_finite() {
        return (s.a, 0.0);
    }
    let t = ((p - s.a).dot(ab) / l2).clamp(0.0, 1.0);
    (s.at(t), t)
}

/// The foot of `p` on the plane, and the signed distance (positive on the normal's side).
pub fn closest_point_plane(p: Vec3, pl: &Plane) -> (Vec3, f64) {
    (pl.project(p), pl.signed_distance(p))
}

/// Which feature of a triangle a closest point lies on. Edges: 0 = ab, 1 = bc, 2 = ca.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriFeature {
    Vertex(u8),
    Edge(u8),
    Face,
}

/// The point of triangle `abc` nearest `p` and the feature it lies on (Ericson, Real-Time Collision Detection,
/// §5.1.5: the Voronoi regions of the vertices, then the edges, then the face). A zero-area triangle is refused.
pub fn closest_point_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3, tol: Tol) -> Result<(Vec3, TriFeature), Degenerate> {
    if !p.is_finite() {
        return Err(Degenerate::NonFinite);
    }
    let l = (b - a).magnitude().max((c - a).magnitude()).max((c - b).magnitude());
    if !(a.is_finite() && b.is_finite() && c.is_finite()) {
        return Err(Degenerate::NonFinite);
    }
    if l <= tol.lin || (b - a).cross(c - a).magnitude() / l <= tol.lin {
        return Err(Degenerate::ZeroArea);
    }
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return Ok((a, TriFeature::Vertex(0)));
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return Ok((b, TriFeature::Vertex(1)));
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return Ok((a + ab * v, TriFeature::Edge(0)));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return Ok((c, TriFeature::Vertex(2)));
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return Ok((a + ac * w, TriFeature::Edge(2)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return Ok((b + (c - b) * w, TriFeature::Edge(1)));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    Ok((a + ab * v + ac * w, TriFeature::Face))
}
