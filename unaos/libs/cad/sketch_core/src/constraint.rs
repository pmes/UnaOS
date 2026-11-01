// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Constraints: each is one or two residual rows with exact analytic Jacobian rows. The derivatives are carried by a
//! small forward-mode dual number over the (at most twelve) parameters a constraint touches — the same closed-form
//! derivatives a hand-written row would hold, with no finite differences.

use crate::entity::{BadRef, Entity, EntityId, PointRef, Sketch};
use crate::geom::Vec2;
use core::f64::consts::PI;
use core::ops::{Add, Div, Mul, Neg, Sub};

/// Parameter slots one constraint may touch. The widest constraint (Symmetric over two arc ends and a line) touches
/// twelve; sixteen leaves headroom.
pub(crate) const K: usize = 16;

/// A norm or divisor below this is clamped to it: a numeric guard (it keeps a zero-length line from dividing by
/// zero, and its square, 1e-300, is still a normal f64), not a geometric tolerance.
pub(crate) const NORM_GUARD: f64 = 1e-150;

/// A dual number: a value and its gradient over the constraint's local parameter slots.
#[derive(Clone, Copy, Debug)]
pub(crate) struct D {
    pub v: f64,
    pub g: [f64; K],
}

impl D {
    pub fn c(v: f64) -> D {
        D { v, g: [0.0; K] }
    }
    fn map(self, v: f64, k: f64) -> D {
        let mut g = [0.0; K];
        for i in 0..K {
            g[i] = self.g[i] * k;
        }
        D { v, g }
    }
    pub fn sqrt(self) -> D {
        let s = libm::sqrt(if self.v > 0.0 { self.v } else { 0.0 });
        if s < NORM_GUARD { D::c(s) } else { self.map(s, 0.5 / s) }
    }
    pub fn sin(self) -> D {
        self.map(libm::sin(self.v), libm::cos(self.v))
    }
    pub fn cos(self) -> D {
        self.map(libm::cos(self.v), -libm::sin(self.v))
    }
    pub fn atan2(y: D, x: D) -> D {
        let v = libm::atan2(y.v, x.v);
        let mut d = x.v * x.v + y.v * y.v;
        if d < NORM_GUARD {
            d = NORM_GUARD;
        }
        let mut g = [0.0; K];
        for i in 0..K {
            g[i] = (x.v * y.g[i] - y.v * x.g[i]) / d;
        }
        D { v, g }
    }
    /// The value clamped away from zero (positive), the gradient kept.
    pub fn guarded(self) -> D {
        if libm::fabs(self.v) < NORM_GUARD { D { v: NORM_GUARD, g: self.g } } else { self }
    }
}

impl Add for D {
    type Output = D;
    fn add(self, o: D) -> D {
        let mut g = self.g;
        for i in 0..K {
            g[i] += o.g[i];
        }
        D { v: self.v + o.v, g }
    }
}
impl Sub for D {
    type Output = D;
    fn sub(self, o: D) -> D {
        let mut g = self.g;
        for i in 0..K {
            g[i] -= o.g[i];
        }
        D { v: self.v - o.v, g }
    }
}
impl Neg for D {
    type Output = D;
    fn neg(self) -> D {
        self.map(-self.v, -1.0)
    }
}
impl Mul for D {
    type Output = D;
    fn mul(self, o: D) -> D {
        let mut g = [0.0; K];
        for i in 0..K {
            g[i] = self.g[i] * o.v + self.v * o.g[i];
        }
        D { v: self.v * o.v, g }
    }
}
impl Mul<f64> for D {
    type Output = D;
    fn mul(self, k: f64) -> D {
        self.map(self.v * k, k)
    }
}
impl Sub<f64> for D {
    type Output = D;
    fn sub(self, k: f64) -> D {
        D { v: self.v - k, g: self.g }
    }
}
impl Div for D {
    type Output = D;
    fn div(self, o: D) -> D {
        let b2 = o.v * o.v;
        let mut g = [0.0; K];
        for i in 0..K {
            g[i] = (self.g[i] * o.v - self.v * o.g[i]) / b2;
        }
        D { v: self.v / o.v, g }
    }
}

#[derive(Clone, Copy)]
struct P2 {
    x: D,
    y: D,
}

fn sub(a: P2, b: P2) -> P2 {
    P2 { x: a.x - b.x, y: a.y - b.y }
}
fn dot(a: P2, b: P2) -> D {
    a.x * b.x + a.y * b.y
}
fn cross(a: P2, b: P2) -> D {
    a.x * b.y - a.y * b.x
}
fn len(a: P2) -> D {
    dot(a, a).sqrt()
}
/// Signed distance of `p` from the line a->b (positive on the left).
fn sd(p: P2, a: P2, b: P2) -> D {
    let u = sub(b, a);
    cross(u, sub(p, a)) / len(u).guarded()
}

/// The unit of a residual row: millimetres (`Tol.lin`) or radians / a sine or cosine (`Tol.ang`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Lin,
    Ang,
}

/// What a constraint fixes from the geometry at solve entry, so its residual is smooth and keeps the side the
/// drawing is on: a sign, an internal tangency, or the shared endpoint of an endpoint tangency.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Cap {
    None,
    /// The sign of a signed distance (a tangent line's side, a point-line distance's side); for circles, external.
    Side(f64),
    /// Internal circle tangency with the sign of r1 - r2.
    Internal(f64),
    /// Line/arc tangency at a shared (Coincident) endpoint: the arc's end.
    EndTangent(PointRef),
    /// Arc/arc tangency at a shared endpoint: the first arc's end.
    EndTangentCircles(PointRef),
}

/// The evaluation context: the trial parameters and the constraint's local slot table.
pub(crate) struct Ctx<'a> {
    x: &'a [f64],
    pub slots: [usize; K],
    pub n: usize,
}

impl<'a> Ctx<'a> {
    fn new(x: &'a [f64]) -> Self {
        Ctx { x, slots: [0; K], n: 0 }
    }
    fn var(&mut self, p: usize) -> D {
        let mut d = D::c(self.x.get(p).copied().unwrap_or(f64::NAN));
        for i in 0..self.n {
            if self.slots[i] == p {
                d.g[i] = 1.0;
                return d;
            }
        }
        // K covers the widest constraint (twelve slots) by construction; a full table cannot occur.
        if self.n < K {
            self.slots[self.n] = p;
            d.g[self.n] = 1.0;
            self.n += 1;
        }
        d
    }
    fn nan() -> P2 {
        P2 { x: D::c(f64::NAN), y: D::c(f64::NAN) }
    }
    fn point(&mut self, sk: &Sketch, id: EntityId) -> P2 {
        match sk.entity(id) {
            Some(Entity::Point { x }) => {
                let i = *x;
                P2 { x: self.var(i), y: self.var(i + 1) }
            }
            _ => Self::nan(),
        }
    }
    fn pt(&mut self, sk: &Sketch, p: PointRef) -> P2 {
        match p {
            PointRef::Point(id) => self.point(sk, id),
            PointRef::Centre(id) => match sk.need_round(id) {
                Ok((c, _, _)) => self.point(sk, c),
                Err(_) => Self::nan(),
            },
            PointRef::Start(id) | PointRef::End(id) => {
                let start = matches!(p, PointRef::Start(_));
                match sk.entity(id) {
                    Some(Entity::Line { a, b }) => {
                        let (a, b) = (*a, *b);
                        self.point(sk, if start { a } else { b })
                    }
                    Some(Entity::Arc { centre, r }) => {
                        let (centre, r) = (*centre, *r);
                        let c = self.point(sk, centre);
                        let rad = self.var(r);
                        let ang = self.var(r + if start { 1 } else { 2 });
                        P2 { x: c.x + rad * ang.cos(), y: c.y + rad * ang.sin() }
                    }
                    _ => Self::nan(),
                }
            }
        }
    }
    fn line(&mut self, sk: &Sketch, id: EntityId) -> (P2, P2) {
        match sk.need_line(id) {
            Ok((a, b)) => (self.point(sk, a), self.point(sk, b)),
            Err(_) => (Self::nan(), Self::nan()),
        }
    }
    fn round(&mut self, sk: &Sketch, id: EntityId) -> (P2, D) {
        match sk.need_round(id) {
            Ok((c, r, _)) => (self.point(sk, c), self.var(r)),
            Err(_) => (Self::nan(), D::c(f64::NAN)),
        }
    }
}

/// One constraint's rows (at most two) and its slot table.
pub(crate) struct Eval {
    pub slots: [usize; K],
    pub n: usize,
    pub rows: [(D, Kind); 2],
    pub m: usize,
}

impl Eval {
    fn push(&mut self, r: D, k: Kind) {
        if self.m < 2 {
            self.rows[self.m] = (r, k);
            self.m += 1;
        }
    }
}

/// A constraint. Lines are `EntityId`s of Line entities; "round" ids are Circles or Arcs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Constraint {
    Coincident(PointRef, PointRef),
    PointOnLine(PointRef, EntityId),
    PointOnCircle(PointRef, EntityId),
    Horizontal(EntityId),
    Vertical(EntityId),
    Parallel(EntityId, EntityId),
    Perpendicular(EntityId, EntityId),
    /// A line tangent to a circle or arc.
    TangentLineCircle(EntityId, EntityId),
    /// Two circles / arcs tangent (externally or internally, as drawn).
    TangentCircles(EntityId, EntityId),
    EqualLength(EntityId, EntityId),
    EqualRadius(EntityId, EntityId),
    /// Point to point distance (mm).
    Distance(PointRef, PointRef, f64),
    /// Point to line distance (mm), on the side the point is drawn.
    DistancePointLine(PointRef, EntityId, f64),
    /// The angle from the first line to the second (radians, CCW).
    Angle(EntityId, EntityId, f64),
    Radius(EntityId, f64),
    Diameter(EntityId, f64),
    Fix(PointRef, Vec2),
    /// The point is the line's midpoint.
    Midpoint(PointRef, EntityId),
    /// The two points are mirror images about the line.
    Symmetric(PointRef, PointRef, EntityId),
}

fn sign(v: f64) -> f64 {
    if v < 0.0 { -1.0 } else { 1.0 }
}

/// The endpoints a Coincident constraint may join for an endpoint tangency.
fn line_ends(sk: &Sketch, l: EntityId) -> [PointRef; 4] {
    let (a, b) = sk.need_line(l).unwrap_or((l, l));
    [PointRef::Point(a), PointRef::Point(b), PointRef::Start(l), PointRef::End(l)]
}

/// The first of `left` that a Coincident joins to any of `right`.
fn shared(sk: &Sketch, left: &[PointRef], right: &[PointRef]) -> Option<PointRef> {
    for c in sk.constraints.iter().flatten() {
        if let Constraint::Coincident(p, q) = *c {
            for &l in left {
                if (p == l && right.contains(&q)) || (q == l && right.contains(&p)) {
                    return Some(l);
                }
            }
        }
    }
    None
}

impl Constraint {
    /// Every entity named is of the kind the constraint needs.
    pub(crate) fn check(&self, sk: &Sketch) -> Result<(), BadRef> {
        use Constraint::*;
        match *self {
            Coincident(p, q) | Distance(p, q, _) => {
                sk.need_ref(p)?;
                sk.need_ref(q)
            }
            PointOnLine(p, l) | DistancePointLine(p, l, _) | Midpoint(p, l) => {
                sk.need_ref(p)?;
                sk.need_line(l).map(|_| ())
            }
            PointOnCircle(p, c) => {
                sk.need_ref(p)?;
                sk.need_round(c).map(|_| ())
            }
            Horizontal(l) | Vertical(l) => sk.need_line(l).map(|_| ()),
            Parallel(a, b) | Perpendicular(a, b) | EqualLength(a, b) | Angle(a, b, _) => {
                sk.need_line(a)?;
                sk.need_line(b).map(|_| ())
            }
            TangentLineCircle(l, c) => {
                sk.need_line(l)?;
                sk.need_round(c).map(|_| ())
            }
            TangentCircles(a, b) | EqualRadius(a, b) => {
                sk.need_round(a)?;
                sk.need_round(b).map(|_| ())
            }
            Radius(c, _) | Diameter(c, _) => sk.need_round(c).map(|_| ()),
            Fix(p, _) => sk.need_ref(p),
            Symmetric(p, q, l) => {
                sk.need_ref(p)?;
                sk.need_ref(q)?;
                sk.need_line(l).map(|_| ())
            }
        }
    }

    /// What this constraint fixes from the geometry at `x` (solve entry).
    pub(crate) fn capture(&self, sk: &Sketch, x: &[f64]) -> Cap {
        use Constraint::*;
        let mut cx = Ctx::new(x);
        match *self {
            TangentLineCircle(l, c) => {
                if let Ok((_, _, true)) = sk.need_round(c) {
                    if let Some(e) = shared(sk, &[PointRef::Start(c), PointRef::End(c)], &line_ends(sk, l)) {
                        return Cap::EndTangent(e);
                    }
                }
                let (a, b) = cx.line(sk, l);
                let (cc, _) = cx.round(sk, c);
                Cap::Side(sign(sd(cc, a, b).v))
            }
            TangentCircles(c1, c2) => {
                if let (Ok((_, _, true)), Ok((_, _, true))) = (sk.need_round(c1), sk.need_round(c2)) {
                    let left = [PointRef::Start(c1), PointRef::End(c1)];
                    let right = [PointRef::Start(c2), PointRef::End(c2)];
                    if let Some(e) = shared(sk, &left, &right) {
                        return Cap::EndTangentCircles(e);
                    }
                }
                let (p1, r1) = cx.round(sk, c1);
                let (p2, r2) = cx.round(sk, c2);
                let d = len(sub(p1, p2)).v;
                let (r1, r2) = (libm::fabs(r1.v), libm::fabs(r2.v));
                let big = if r1 > r2 { r1 } else { r2 };
                if d < big { Cap::Internal(sign(r1 - r2)) } else { Cap::Side(1.0) }
            }
            DistancePointLine(p, l, _) => {
                let pp = cx.pt(sk, p);
                let (a, b) = cx.line(sk, l);
                Cap::Side(sign(sd(pp, a, b).v))
            }
            _ => Cap::None,
        }
    }

    /// The residual rows at `x`.
    pub(crate) fn eval(&self, sk: &Sketch, x: &[f64], cap: Cap) -> Eval {
        use Constraint::*;
        use Kind::*;
        let mut cx = Ctx::new(x);
        let mut out = Eval { slots: [0; K], n: 0, rows: [(D::c(0.0), Lin); 2], m: 0 };
        match *self {
            Coincident(p, q) => {
                let (a, b) = (cx.pt(sk, p), cx.pt(sk, q));
                out.push(a.x - b.x, Lin);
                out.push(a.y - b.y, Lin);
            }
            PointOnLine(p, l) => {
                let pp = cx.pt(sk, p);
                let (a, b) = cx.line(sk, l);
                out.push(sd(pp, a, b), Lin);
            }
            PointOnCircle(p, c) => {
                let pp = cx.pt(sk, p);
                let (cc, r) = cx.round(sk, c);
                out.push(len(sub(pp, cc)) - r, Lin);
            }
            Horizontal(l) => {
                let (a, b) = cx.line(sk, l);
                out.push(a.y - b.y, Lin);
            }
            Vertical(l) => {
                let (a, b) = cx.line(sk, l);
                out.push(a.x - b.x, Lin);
            }
            Parallel(l1, l2) | Perpendicular(l1, l2) => {
                let (a1, b1) = cx.line(sk, l1);
                let (a2, b2) = cx.line(sk, l2);
                let (u1, u2) = (sub(b1, a1), sub(b2, a2));
                let den = (len(u1) * len(u2)).guarded();
                let num = if matches!(self, Parallel(..)) { cross(u1, u2) } else { dot(u1, u2) };
                out.push(num / den, Ang);
            }
            TangentLineCircle(l, c) => {
                let (a, b) = cx.line(sk, l);
                let (cc, r) = cx.round(sk, c);
                match cap {
                    Cap::EndTangent(e) => {
                        let ep = cx.pt(sk, e);
                        let (w, u) = (sub(ep, cc), sub(b, a));
                        out.push(dot(w, u) / (len(w) * len(u)).guarded(), Ang);
                    }
                    Cap::Side(s) => out.push(sd(cc, a, b) * s - r, Lin),
                    _ => out.push(sd(cc, a, b) - r, Lin),
                }
            }
            TangentCircles(c1, c2) => {
                let (p1, r1) = cx.round(sk, c1);
                let (p2, r2) = cx.round(sk, c2);
                match cap {
                    Cap::EndTangentCircles(e) => {
                        let ep = cx.pt(sk, e);
                        let (w1, w2) = (sub(ep, p1), sub(ep, p2));
                        out.push(cross(w1, w2) / (len(w1) * len(w2)).guarded(), Ang);
                    }
                    Cap::Internal(s) => out.push(len(sub(p1, p2)) - (r1 - r2) * s, Lin),
                    _ => out.push(len(sub(p1, p2)) - (r1 + r2), Lin),
                }
            }
            EqualLength(l1, l2) => {
                let (a1, b1) = cx.line(sk, l1);
                let (a2, b2) = cx.line(sk, l2);
                out.push(len(sub(b1, a1)) - len(sub(b2, a2)), Lin);
            }
            EqualRadius(c1, c2) => {
                let (_, r1) = cx.round(sk, c1);
                let (_, r2) = cx.round(sk, c2);
                out.push(r1 - r2, Lin);
            }
            Distance(p, q, d) => {
                let (a, b) = (cx.pt(sk, p), cx.pt(sk, q));
                out.push(len(sub(a, b)) - d, Lin);
            }
            DistancePointLine(p, l, d) => {
                let pp = cx.pt(sk, p);
                let (a, b) = cx.line(sk, l);
                let s = if let Cap::Side(s) = cap { s } else { 1.0 };
                out.push(sd(pp, a, b) * s - d, Lin);
            }
            Angle(l1, l2, th) => {
                let (a1, b1) = cx.line(sk, l1);
                let (a2, b2) = cx.line(sk, l2);
                let (u1, u2) = (sub(b1, a1), sub(b2, a2));
                let ang = D::atan2(cross(u1, u2), dot(u1, u2));
                let diff = ang.v - th;
                let wrapped = diff - 2.0 * PI * libm::floor((diff + PI) / (2.0 * PI));
                out.push(ang - (th + (diff - wrapped)), Ang);
            }
            Radius(c, v) => {
                let (_, r) = cx.round(sk, c);
                out.push(r - v, Lin);
            }
            Diameter(c, v) => {
                let (_, r) = cx.round(sk, c);
                out.push(r * 2.0 - v, Lin);
            }
            Fix(p, at) => {
                let pp = cx.pt(sk, p);
                out.push(pp.x - at.x, Lin);
                out.push(pp.y - at.y, Lin);
            }
            Midpoint(p, l) => {
                let pp = cx.pt(sk, p);
                let (a, b) = cx.line(sk, l);
                out.push(pp.x - (a.x + b.x) * 0.5, Lin);
                out.push(pp.y - (a.y + b.y) * 0.5, Lin);
            }
            Symmetric(p, q, l) => {
                let (pp, qq) = (cx.pt(sk, p), cx.pt(sk, q));
                let (a, b) = cx.line(sk, l);
                let m = P2 { x: (pp.x + qq.x) * 0.5, y: (pp.y + qq.y) * 0.5 };
                let u = sub(b, a);
                out.push(sd(m, a, b), Lin);
                out.push(dot(sub(qq, pp), u) / len(u).guarded(), Lin);
            }
        }
        out.slots = cx.slots;
        out.n = cx.n;
        out
    }
}
