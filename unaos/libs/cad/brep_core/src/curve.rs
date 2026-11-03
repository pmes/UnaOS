//! The analytic curves (line, circle, ellipse), the rational NURBS curve (de Boor evaluation only; NURBS surfaces are
//! VUG-BREP-NURBS-SURF, owed) and `Frame`, the orthonormal placement every curve and surface is written in.

use crate::Refusal;
use geom_core::{Degenerate, Vec3};
use alloc::vec::Vec;

/// A right-handed orthonormal frame: origin `o`, axes `x`, `y`, `z` (z = x × y).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub o: Vec3,
    pub x: Vec3,
    pub y: Vec3,
    pub z: Vec3,
}

impl Frame {
    /// The world frame.
    pub const WORLD: Frame = Frame { o: Vec3::ZERO, x: Vec3::X, y: Vec3::Y, z: Vec3::Z };

    /// A frame at `o` with axis `z`, and `x` the part of `x_hint` perpendicular to `z` (Gram-Schmidt). Refuses a zero
    /// axis or a hint parallel to the axis by name.
    pub fn new(o: Vec3, z: Vec3, x_hint: Vec3) -> Result<Frame, Refusal> {
        let z = z.normalize()?;
        let x = (x_hint - z * x_hint.dot(z)).normalize().map_err(|_| Refusal::Degenerate(Degenerate::Collinear))?;
        Ok(Frame { o, x, y: z.cross(x), z })
    }

    /// The point at local coordinates (a, b, c).
    pub fn at(&self, a: f64, b: f64, c: f64) -> Vec3 {
        self.o + self.x * a + self.y * b + self.z * c
    }

    /// The local coordinates of `p`.
    pub fn local(&self, p: Vec3) -> (f64, f64, f64) {
        let d = p - self.o;
        (d.dot(self.x), d.dot(self.y), d.dot(self.z))
    }

    /// The same axes at a new origin.
    pub fn moved(&self, o: Vec3) -> Frame {
        Frame { o, ..*self }
    }
}

/// A curve in 3D. Edges carry a parameter range [t0, t1] on it.
#[derive(Clone, Debug, PartialEq)]
pub enum Curve {
    /// `o + dir * t`, `dir` unit (t in millimetres).
    Line { o: Vec3, dir: Vec3 },
    /// `f.o + r (f.x cos t + f.y sin t)` (t in radians, counter-clockwise about f.z).
    Circle { f: Frame, r: f64 },
    /// `f.o + a f.x cos t + b f.y sin t`.
    Ellipse { f: Frame, a: f64, b: f64 },
    /// A rational B-spline curve.
    Nurbs(NurbsCurve),
}

impl Curve {
    pub fn point(&self, t: f64) -> Vec3 {
        match self {
            Curve::Line { o, dir } => *o + *dir * t,
            Curve::Circle { f, r } => f.o + (f.x * libm::cos(t) + f.y * libm::sin(t)) * *r,
            Curve::Ellipse { f, a, b } => f.o + f.x * (*a * libm::cos(t)) + f.y * (*b * libm::sin(t)),
            Curve::Nurbs(n) => n.point(t),
        }
    }

    /// dC/dt; `None` for a NURBS curve (its derivative is not built in this arc).
    pub fn tangent(&self, t: f64) -> Option<Vec3> {
        match self {
            Curve::Line { dir, .. } => Some(*dir),
            Curve::Circle { f, r } => Some((f.y * libm::cos(t) - f.x * libm::sin(t)) * *r),
            Curve::Ellipse { f, a, b } => Some(f.y * (*b * libm::cos(t)) - f.x * (*a * libm::sin(t))),
            Curve::Nurbs(_) => None,
        }
    }

    /// The largest radius of curvature, which sets the chord step (lines: none).
    pub(crate) fn max_radius(&self) -> Option<f64> {
        match self {
            Curve::Line { .. } | Curve::Nurbs(_) => None,
            Curve::Circle { r, .. } => Some(*r),
            Curve::Ellipse { a, b, .. } => {
                let (hi, lo) = if a > b { (*a, *b) } else { (*b, *a) };
                Some(hi * hi / lo)
            }
        }
    }
}

/// A rational B-spline curve: `degree`, a non-decreasing knot vector of `ctrl.len() + degree + 1` entries, control
/// points and positive weights. Evaluation is de Boor's algorithm in homogeneous coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct NurbsCurve {
    degree: usize,
    knots: Vec<f64>,
    ctrl: Vec<Vec3>,
    weights: Vec<f64>,
}

impl NurbsCurve {
    /// Refuses, by name, a degree of 0, too few control points, a knot count other than n + degree + 1, a decreasing or
    /// non-finite knot, an empty parameter range, a weight count mismatch, or a weight <= 0.
    pub fn new(degree: usize, knots: Vec<f64>, ctrl: Vec<Vec3>, weights: Vec<f64>) -> Result<NurbsCurve, Refusal> {
        let n = ctrl.len();
        if degree == 0 {
            return Err(Refusal::BadParam("nurbs degree 0"));
        }
        if n < degree + 1 {
            return Err(Refusal::BadParam("nurbs too few control points"));
        }
        if knots.len() != n + degree + 1 {
            return Err(Refusal::BadParam("nurbs knot count"));
        }
        if weights.len() != n {
            return Err(Refusal::BadParam("nurbs weight count"));
        }
        if knots.iter().any(|k| !k.is_finite()) || knots.windows(2).any(|w| w[1] < w[0]) {
            return Err(Refusal::BadParam("nurbs knots not non-decreasing"));
        }
        if knots[degree] >= knots[n] {
            return Err(Refusal::BadParam("nurbs empty parameter range"));
        }
        if weights.iter().any(|w| !(*w > 0.0) || !w.is_finite()) {
            return Err(Refusal::BadParam("nurbs weight not positive"));
        }
        Ok(NurbsCurve { degree, knots, ctrl, weights })
    }

    /// The parameter range [knots[p], knots[n]].
    pub fn range(&self) -> (f64, f64) {
        (self.knots[self.degree], self.knots[self.ctrl.len()])
    }

    /// The point at `t` (clamped to the range): de Boor on (w x, w y, w z, w), then the projection.
    pub fn point(&self, t: f64) -> Vec3 {
        let p = self.degree;
        let n = self.ctrl.len();
        let (lo, hi) = self.range();
        let t = t.max(lo).min(hi);
        let mut k = p;
        while k < n - 1 && t >= self.knots[k + 1] {
            k += 1;
        }
        let mut d: Vec<[f64; 4]> = (0..=p)
            .map(|j| {
                let c = self.ctrl[j + k - p];
                let w = self.weights[j + k - p];
                [c.x * w, c.y * w, c.z * w, w]
            })
            .collect();
        for r in 1..=p {
            for j in (r..=p).rev() {
                let i = j + k - p;
                let den = self.knots[i + p + 1 - r] - self.knots[i];
                let a = if den == 0.0 { 0.0 } else { (t - self.knots[i]) / den };
                for c in 0..4 {
                    d[j][c] = d[j - 1][c] * (1.0 - a) + d[j][c] * a;
                }
            }
        }
        let h = d[p];
        Vec3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3])
    }

    // VUGSTEP (SR482): read-only accessors, so an exchange writer (step_core) can write the curve back out.
    /// The degree p.
    pub fn degree(&self) -> usize {
        self.degree
    }
    /// The full knot vector (n + p + 1 entries, non-decreasing).
    pub fn knots(&self) -> &[f64] {
        &self.knots
    }
    /// The control points.
    pub fn ctrl(&self) -> &[Vec3] {
        &self.ctrl
    }
    /// The weights (all > 0).
    pub fn weights(&self) -> &[f64] {
        &self.weights
    }
}
