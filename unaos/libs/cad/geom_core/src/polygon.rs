//! Area, centroid and orientation of a simple polygon given as its vertex loop (no repeated closing vertex).

use crate::predicates::{Predicate, Unsure, orient2d};
use crate::tol::{Degenerate, Tol};
use crate::vec::Vec2;

/// The signed area (shoelace, about the first vertex): positive for a counterclockwise loop; 0 under 3 vertices.
pub fn area(pts: &[Vec2]) -> f64 {
    let Some(&p0) = pts.first() else { return 0.0 };
    if pts.len() < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for w in pts[1..].windows(2) {
        s += (w[0] - p0).cross(w[1] - p0);
    }
    s * 0.5
}

/// The area centroid. Refuses fewer than 3 vertices, a non-finite vertex, and a loop whose area over its perimeter
/// (its mean width) is no more than `tol.lin`.
pub fn centroid(pts: &[Vec2], tol: Tol) -> Result<Vec2, Degenerate> {
    if pts.len() < 3 {
        return Err(Degenerate::TooFewPoints);
    }
    if !pts.iter().all(|p| p.is_finite()) {
        return Err(Degenerate::NonFinite);
    }
    let p0 = pts[0];
    let mut a2 = 0.0;
    let mut acc = Vec2::ZERO;
    for w in pts[1..].windows(2) {
        let (q0, q1) = (w[0] - p0, w[1] - p0);
        let cr = q0.cross(q1);
        a2 += cr;
        acc += (q0 + q1) * cr;
    }
    let mut perimeter = 0.0;
    for i in 0..pts.len() {
        perimeter += pts[i].dist(pts[(i + 1) % pts.len()]);
    }
    if (a2 * 0.5).abs() <= tol.lin * perimeter {
        return Err(Degenerate::ZeroArea);
    }
    Ok(p0 + acc / (3.0 * a2))
}

/// The loop's orientation, exactly: `orient2d` at the lowest (then leftmost) vertex with its nearest distinct
/// neighbours. Positive = counterclockwise. Valid for a simple polygon; fewer than 3 distinct vertices, or a spike
/// at the extreme vertex, answers `Zero`; a non-finite vertex answers `Uncertain(NonFinite)`.
pub fn is_ccw(pts: &[Vec2]) -> Predicate {
    if !pts.iter().all(|p| p.is_finite()) {
        return Predicate::Uncertain(Unsure::NonFinite);
    }
    let n = pts.len();
    if n < 3 {
        return Predicate::Zero;
    }
    let mut i = 0;
    for (j, p) in pts.iter().enumerate() {
        let q = pts[i];
        if p.y < q.y || (p.y == q.y && p.x < q.x) {
            i = j;
        }
    }
    let pi = pts[i];
    let next = (1..n).map(|k| pts[(i + k) % n]).find(|&q| q != pi);
    let prev = (1..n).map(|k| pts[(i + n - k) % n]).find(|&q| q != pi);
    match (prev, next) {
        (Some(a), Some(c)) => orient2d(a, pi, c),
        _ => Predicate::Zero,
    }
}
