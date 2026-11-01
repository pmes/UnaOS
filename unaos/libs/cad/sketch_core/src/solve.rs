// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The solver: ONE weighted min-norm Levenberg-Marquardt iteration serves solve and drag, then ONE diagnosis by the
//! ordered rank of the Jacobian names what is redundant, what conflicts and what is still free.
//!
//! The step is `δ = −W⁻¹Jᵀ(JW⁻¹Jᵀ + λ(diag + I))⁻¹ r`. With λ small it is the Gauss-Newton minimum-norm step (an
//! under-determined sketch moves as little as possible: the "sticky" prior SolveSpace and Onshape describe); λ grows
//! ×10 on a rejected step and shrinks ÷10 on an accepted one, and as it grows the step turns into gradient descent on
//! ½‖r‖², so an inconsistent system settles at its least-squares point, where `Jᵀr = 0` exhibits the dependency.

use crate::constraint::{Cap, Kind};
use crate::entity::{ConstraintId, Entity, EntityId, PointRef, Sketch};
use crate::geom::{Tol, Vec2};
use crate::linalg::{self, Mat, PARTICIPATION, RANK_TOL};
use alloc::vec;
use alloc::vec::Vec;

/// The iteration cap (planegcs' default is the same order: 100).
pub const MAX_ITERS: u32 = 100;
/// A dragged parameter costs this much more to move than any other. The dragged point ends short of its target by
/// about move / DRAG_WEIGHT (the weighted min-norm split), so 1e9 keeps a 10 mm drag within 1e-8 mm, under
/// `Tol::DEFAULT.lin`; and 1e-9 against 1 keeps the damped system well inside f64's range.
const DRAG_WEIGHT: f64 = 1e9;
const LAMBDA0: f64 = 1e-9;
const LAMBDA_MIN: f64 = 1e-15;
const LAMBDA_MAX: f64 = 1e12;
/// The least-squares point is reached when ‖Jᵀr‖ is this small against ‖r‖ · the largest row norm; only then is an
/// unsatisfied, rank-deficient system a CONFLICT rather than a solve that did not finish.
const STATIONARY: f64 = 1e-6;

/// A solve that met every constraint within `Tol`.
#[derive(Clone, Debug, PartialEq)]
pub struct Solved {
    /// Iterations taken (accepted and rejected steps).
    pub iterations: u32,
    /// The largest residual in units of its tolerance (`|r| / Tol.lin` or `|r| / Tol.ang`); ≤ 1.
    pub residual: f64,
    /// Degrees of freedom left: parameters minus the Jacobian's rank.
    pub dof: usize,
    /// Constraints implied by earlier ones (consistent): named, not refused; the later-added member of each set.
    pub redundant: Vec<ConstraintId>,
}

/// A solve that did not happen; the sketch is left as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// These constraints cannot all hold (sorted).
    Conflicting(Vec<ConstraintId>),
    /// [`solve_strict`] only: these constraints are implied by earlier ones.
    Redundant(Vec<ConstraintId>),
    /// The iteration did not reach a solution or a least-squares point (or met a non-finite value, or a drag named
    /// no point).
    Diverged,
}

/// Degrees of freedom: the count and the entities that can still move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dof {
    pub count: usize,
    pub free: Vec<EntityId>,
}

struct System {
    r: Vec<f64>,
    kind: Vec<Kind>,
    owner: Vec<u32>,
    j: Mat,
}

impl System {
    fn finite(&self) -> bool {
        self.r.iter().all(|v| v.is_finite()) && self.j.a.iter().all(|v| v.is_finite())
    }
    fn cost(&self) -> f64 {
        linalg::dot(&self.r, &self.r)
    }
    fn scaled(&self, i: usize, tol: Tol) -> f64 {
        libm::fabs(self.r[i]) / if self.kind[i] == Kind::Lin { tol.lin } else { tol.ang }
    }
    fn scaled_max(&self, tol: Tol) -> f64 {
        let mut m = 0.0;
        for i in 0..self.r.len() {
            let s = self.scaled(i, tol);
            if !(s <= m) {
                m = s;
            }
        }
        m
    }
}

fn caps(sk: &Sketch, x: &[f64]) -> Vec<Cap> {
    sk.constraints.iter().map(|c| c.as_ref().map_or(Cap::None, |c| c.capture(sk, x))).collect()
}

fn system(sk: &Sketch, x: &[f64], caps: &[Cap]) -> System {
    let n = x.len();
    let mut r = Vec::new();
    let mut kind = Vec::new();
    let mut owner = Vec::new();
    let mut rows: Vec<f64> = Vec::new();
    for (ci, c) in sk.constraints.iter().enumerate() {
        let Some(c) = c else { continue };
        let e = c.eval(sk, x, caps[ci]);
        for k in 0..e.m {
            let (d, kd) = e.rows[k];
            let base = rows.len();
            rows.resize(base + n, 0.0);
            for s in 0..e.n {
                rows[base + e.slots[s]] += d.g[s];
            }
            r.push(d.v);
            kind.push(kd);
            owner.push(ci as u32);
        }
    }
    let m = r.len();
    System { r, kind, owner, j: Mat { rows: m, cols: n, a: rows } }
}

/// The shared iteration: from `x` with weights `w`, until every row is within `tol`, the cap, or λ runs out.
fn run(sk: &Sketch, mut x: Vec<f64>, w: &[f64], tol: Tol) -> Result<(Vec<f64>, System, u32), Refusal> {
    let cp = caps(sk, &x);
    let mut sys = system(sk, &x, &cp);
    let mut lam = LAMBDA0;
    let mut it = 0u32;
    let n = x.len();
    loop {
        if !sys.finite() {
            return Err(Refusal::Diverged);
        }
        if sys.scaled_max(tol) <= 1.0 || it >= MAX_ITERS {
            break;
        }
        it += 1;
        let m = sys.r.len();
        let mut a = vec![0.0; m * m];
        for i in 0..m {
            let ri = sys.j.row(i);
            for k in i..m {
                let rk = sys.j.row(k);
                let mut s = 0.0;
                for j in 0..n {
                    s += ri[j] * rk[j] / w[j];
                }
                a[i * m + k] = s;
                a[k * m + i] = s;
            }
        }
        for i in 0..m {
            a[i * m + i] += lam * (a[i * m + i] + 1.0);
        }
        let mut y = sys.r.clone();
        if !linalg::cholesky_solve(&mut a, m, &mut y) {
            lam *= 10.0;
            if lam > LAMBDA_MAX {
                break;
            }
            continue;
        }
        let mut xn = x.clone();
        for j in 0..n {
            let mut s = 0.0;
            for i in 0..m {
                s += sys.j.a[i * n + j] * y[i];
            }
            xn[j] -= s / w[j];
        }
        let sn = system(sk, &xn, &cp);
        if sn.finite() && sn.cost() < sys.cost() {
            x = xn;
            sys = sn;
            lam = if lam / 10.0 > LAMBDA_MIN { lam / 10.0 } else { LAMBDA_MIN };
        } else {
            lam *= 10.0;
            if lam > LAMBDA_MAX {
                break;
            }
        }
    }
    Ok((x, sys, it))
}

fn owners_sorted(mut v: Vec<u32>) -> Vec<ConstraintId> {
    v.sort_unstable();
    v.dedup();
    v.into_iter().map(ConstraintId).collect()
}

fn stationary(sys: &System) -> bool {
    let (m, n) = (sys.j.rows, sys.j.cols);
    let mut g = vec![0.0; n];
    let mut big: f64 = 0.0;
    for i in 0..m {
        let row = sys.j.row(i);
        big = big.max(linalg::norm(row));
        for j in 0..n {
            g[j] += row[j] * sys.r[i];
        }
    }
    linalg::norm(&g) <= STATIONARY * linalg::norm(&sys.r) * big
}

fn classify(sys: &System, it: u32, n: usize, tol: Tol, strict: bool) -> Result<Solved, Refusal> {
    let rank = linalg::rank(&sys.j);
    let dof = n - rank.indep.len();
    if sys.scaled_max(tol) <= 1.0 {
        let redundant = owners_sorted(rank.dep.iter().map(|(k, _)| sys.owner[*k]).collect());
        if strict && !redundant.is_empty() {
            return Err(Refusal::Redundant(redundant));
        }
        return Ok(Solved { iterations: it, residual: sys.scaled_max(tol), dof, redundant });
    }
    if rank.dep.is_empty() || !stationary(sys) {
        return Err(Refusal::Diverged);
    }
    let mut group = Vec::new();
    for i in 0..sys.r.len() {
        if sys.scaled(i, tol) > 1.0 {
            group.push(sys.owner[i]);
        }
    }
    for (k, coef) in &rank.dep {
        if sys.scaled(*k, tol) > 1.0 {
            group.push(sys.owner[*k]);
            for (ii, c) in coef.iter().enumerate() {
                if libm::fabs(*c) > PARTICIPATION {
                    group.push(sys.owner[rank.indep[ii]]);
                }
            }
        }
    }
    Err(Refusal::Conflicting(owners_sorted(group)))
}

fn finish(sk: &mut Sketch, x0: Vec<f64>, w: &[f64], tol: Tol, strict: bool) -> Result<Solved, Refusal> {
    let n = x0.len();
    let (x, sys, it) = run(sk, x0, w, tol)?;
    let out = classify(&sys, it, n, tol, strict)?;
    sk.params = x;
    Ok(out)
}

/// Solve the sketch. A consistent redundant constraint is named in [`Solved::redundant`], never refused.
pub fn solve(sk: &mut Sketch, tol: Tol) -> Result<Solved, Refusal> {
    let w = vec![1.0; sk.params.len()];
    let x0 = sk.params.clone();
    finish(sk, x0, &w, tol, false)
}

/// Solve, refusing a sketch with any redundant constraint (`Refusal::Redundant`): for a document that must be
/// exactly constrained. The same solve as [`solve`].
pub fn solve_strict(sk: &mut Sketch, tol: Tol) -> Result<Solved, Refusal> {
    let w = vec![1.0; sk.params.len()];
    let x0 = sk.params.clone();
    finish(sk, x0, &w, tol, true)
}

/// Drag `p` to `to`: the point is preset at the target and its parameters weighted heavy, so the rest of the sketch
/// absorbs the change and everything the constraints do not tie to it stays exactly where it was.
pub fn drag(sk: &mut Sketch, p: PointRef, to: Vec2, tol: Tol) -> Result<Solved, Refusal> {
    let n = sk.params.len();
    let mut x0 = sk.params.clone();
    let mut w = vec![1.0; n];
    let point = |sk: &Sketch, id: EntityId| sk.need_point(id).ok();
    let target = match p {
        PointRef::Point(id) => point(sk, id),
        PointRef::Centre(id) => sk.need_round(id).ok().and_then(|(c, _, _)| point(sk, c)),
        PointRef::Start(id) | PointRef::End(id) => match sk.entity(id) {
            Some(Entity::Line { a, b }) => point(sk, if matches!(p, PointRef::Start(_)) { *a } else { *b }),
            Some(Entity::Arc { centre, r }) => {
                let (r, ci) = (*r, point(sk, *centre).ok_or(Refusal::Diverged)?);
                let ai = r + if matches!(p, PointRef::Start(_)) { 1 } else { 2 };
                let (dx, dy) = (to.x - x0[ci], to.y - x0[ci + 1]);
                let old = x0[ai];
                let d = libm::atan2(dy, dx) - old;
                let pi = core::f64::consts::PI;
                x0[r] = libm::sqrt(dx * dx + dy * dy);
                x0[ai] = old + (d - 2.0 * pi * libm::floor((d + pi) / (2.0 * pi)));
                w[r] = DRAG_WEIGHT;
                w[ai] = DRAG_WEIGHT;
                None
            }
            _ => return Err(Refusal::Diverged),
        },
    };
    match (p, target) {
        (_, Some(i)) => {
            x0[i] = to.x;
            x0[i + 1] = to.y;
            w[i] = DRAG_WEIGHT;
            w[i + 1] = DRAG_WEIGHT;
        }
        (PointRef::Start(id) | PointRef::End(id), None) if matches!(sk.entity(id), Some(Entity::Arc { .. })) => {}
        _ => return Err(Refusal::Diverged),
    }
    finish(sk, x0, &w, tol, false)
}

/// Degrees of freedom at the sketch's current parameters, and the entities that can still move (a parameter is free
/// when it has a component outside the constraint rows' span; a line is free when either end is).
pub fn dof(sk: &Sketch) -> Dof {
    let x = &sk.params;
    let cp = caps(sk, x);
    let sys = system(sk, x, &cp);
    let rank = linalg::rank(&sys.j);
    let n = x.len();
    let own = sk.param_owner();
    let mut free: Vec<EntityId> = Vec::new();
    for j in 0..n {
        let mut s = 0.0;
        for q in &rank.q {
            s += q[j] * q[j];
        }
        if 1.0 - s > RANK_TOL {
            free.push(own[j]);
        }
    }
    for (i, e) in sk.entities.iter().enumerate() {
        if let Entity::Line { a, b } = e {
            if free.contains(a) || free.contains(b) {
                free.push(EntityId(i as u32));
            }
        }
    }
    free.sort_unstable();
    free.dedup();
    Dof { count: n - rank.indep.len(), free }
}
