// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Step 7: the sketch. The outline's primitives become sketch_core entities and the intent the measurement
//! supports becomes sketch_core's own constraints, applied exactly as inferred:
//! - consecutive lines share one point entity (the corner: no Coincident row);
//! - a line meeting an arc: the line's point `Coincident` with the arc's Start / End, plus `TangentLineCircle` when
//!   the junction was tangent within 3 σ; two arcs: their ends `Coincident`, plus `TangentCircles` when tangent;
//! - a line within [`HV_DEG`] of the paper's x / y axis: `Horizontal` / `Vertical`;
//! - a thin side's two lines (fit's `Outline.thin`, VUGPHOTOSIDETHINPARALLEL SR661): `Parallel`, unless both already
//!   took `Horizontal` / `Vertical` (then Parallel is a redundant row);
//! - off the paper's axes (VUGPHOTOOFFAXIS SR670, a line with H / V gets nothing more): `Perpendicular` for two
//!   consecutive lines within [`HV_DEG`] of 90°, then `Parallel` for two non-consecutive lines within [`HV_DEG`] of
//!   parallel, each only when no row already holds the pair's angle (one class per held angle; an obround's two lines
//!   sharing two tangent arcs are already parallel); Symmetric is owed (VUG-PHOTO-SYMMETRIC);
//! - circles and arcs whose radii agree within 3 σ: `EqualRadius` to the first of the class; whose centres agree
//!   within 3 σ: Concentric, which sketch_core spells `Coincident(Centre(a), Centre(b))` (it has no Concentric).
//!
//! An arc traversed clockwise enters sketch_core (whose arcs run CCW from start to end) with its ends swapped. The
//! sketch is solved with the caller's `Tol` (BP-vug-2) and its profiles found. The dimensions are NOT asserted:
//! the measured sizes stay as geometry, for the person (or a verified proposal, BP-vug-7) to dimension.

use alloc::vec;
use alloc::vec::Vec;

use sketch_core::{Constraint, EntityId, PointRef, profiles, solve};

use crate::fit::{BAND_SIGMA, Outline, Prim};
use crate::{Profile, Refusal, Sketch, Solved, Tol, Vec2};

/// A line within this of the paper's axes is Horizontal / Vertical (degrees).
pub const HV_DEG: f64 = 3.0;

/// The inferred sketch, solved, and its profiles.
#[derive(Clone, Debug)]
pub struct Sketched {
    pub sketch: Sketch,
    pub solved: Solved,
    pub profiles: Vec<Profile>,
}

fn is_line(p: &Prim) -> bool {
    matches!(p, Prim::Line { .. })
}

fn end_of(p: &Prim) -> Vec2 {
    match *p {
        Prim::Line { b, .. } => b,
        Prim::Arc { to, .. } => to,
        Prim::Circle { centre, r, .. } => centre + Vec2::new(r, 0.0),
    }
}

fn enter_ref(e: EntityId, ccw: bool) -> PointRef {
    if ccw { PointRef::Start(e) } else { PointRef::End(e) }
}

fn leave_ref(e: EntityId, ccw: bool) -> PointRef {
    if ccw { PointRef::End(e) } else { PointRef::Start(e) }
}

fn bad<T>(_: T) -> Refusal {
    Refusal::Uncertain
}

fn add_loop(
    sk: &mut Sketch,
    prims: &[Prim],
    thin: &[(usize, usize)],
    cons: &mut Vec<Constraint>,
    rounds: &mut Vec<(EntityId, Vec2, f64)>,
) -> Result<(), Refusal> {
    let np = prims.len();
    if np == 1 {
        let Prim::Circle { centre, r, .. } = prims[0] else { return Err(Refusal::Uncertain) };
        let c = sk.add_point(centre.x, centre.y);
        let e = sk.add_circle(c, r).map_err(bad)?;
        rounds.push((e, centre, r));
        return Ok(());
    }
    let mut jp: Vec<Option<EntityId>> = vec![None; np];
    for k in 0..np {
        if is_line(&prims[k]) || is_line(&prims[(k + 1) % np]) {
            let p = end_of(&prims[k]);
            jp[k] = Some(sk.add_point(p.x, p.y));
        }
    }
    let mut ids: Vec<EntityId> = Vec::with_capacity(np);
    let mut axis = vec![false; np]; // VUGPHOTOSIDETHINPARALLEL (SR661): the line took Horizontal / Vertical
    let mut deg = vec![0.0f64; np]; // VUGPHOTOOFFAXIS (SR670): each line's direction in [0, 180) degrees
    for k in 0..np {
        match prims[k] {
            Prim::Line { a, b, .. } => {
                let pa = jp[(k + np - 1) % np].ok_or(Refusal::Uncertain)?;
                let pb = jp[k].ok_or(Refusal::Uncertain)?;
                let e = sk.add_line(pa, pb).map_err(bad)?;
                let d = b - a;
                let mut ang = libm::atan2(d.y, d.x) * 180.0 / core::f64::consts::PI;
                if ang < 0.0 {
                    ang += 180.0;
                }
                if ang.min(180.0 - ang) <= HV_DEG {
                    cons.push(Constraint::Horizontal(e));
                    axis[k] = true;
                } else if (ang - 90.0).abs() <= HV_DEG {
                    cons.push(Constraint::Vertical(e));
                    axis[k] = true;
                }
                deg[k] = ang; // VUGPHOTOOFFAXIS (SR670)
                ids.push(e);
            }
            Prim::Arc { centre, r, from, to, ccw, .. } => {
                let c = sk.add_point(centre.x, centre.y);
                let (s, t) = if ccw { (from, to) } else { (to, from) };
                let a0 = libm::atan2(s.y - centre.y, s.x - centre.x);
                let a1 = libm::atan2(t.y - centre.y, t.x - centre.x);
                let e = sk.add_arc(c, r, a0, a1).map_err(bad)?;
                rounds.push((e, centre, r));
                ids.push(e);
            }
            Prim::Circle { .. } => return Err(Refusal::Uncertain),
        }
    }
    // VUGPHOTOSIDETHINPARALLEL (SR661): a thin side's two lines are Parallel unless both are already on an axis.
    for &(i, j) in thin {
        if i < np && j < np && is_line(&prims[i]) && is_line(&prims[j]) && !(axis[i] && axis[j]) {
            cons.push(Constraint::Parallel(ids[i], ids[j]));
        }
    }
    off_axis(prims, &ids, &axis, &deg, thin, cons); // VUGPHOTOOFFAXIS (SR670)
    for k in 0..np {
        let (p, q) = (&prims[k], &prims[(k + 1) % np]);
        let (ep, eq) = (ids[k], ids[(k + 1) % np]);
        match (*p, *q) {
            (Prim::Line { .. }, Prim::Line { .. }) => {}
            (Prim::Line { .. }, Prim::Arc { ccw, tangent_in, .. }) => {
                let pt = jp[k].ok_or(Refusal::Uncertain)?;
                cons.push(Constraint::Coincident(PointRef::Point(pt), enter_ref(eq, ccw)));
                if tangent_in {
                    cons.push(Constraint::TangentLineCircle(ep, eq));
                }
            }
            (Prim::Arc { ccw, tangent_out, .. }, Prim::Line { .. }) => {
                let pt = jp[k].ok_or(Refusal::Uncertain)?;
                cons.push(Constraint::Coincident(PointRef::Point(pt), leave_ref(ep, ccw)));
                if tangent_out {
                    cons.push(Constraint::TangentLineCircle(eq, ep));
                }
            }
            (Prim::Arc { ccw: c1, tangent_out, .. }, Prim::Arc { ccw: c2, .. }) => {
                cons.push(Constraint::Coincident(leave_ref(ep, c1), enter_ref(eq, c2)));
                if tangent_out {
                    cons.push(Constraint::TangentCircles(ep, eq));
                }
            }
            _ => return Err(Refusal::Uncertain),
        }
    }
    Ok(())
}

/// The outline as a solved sketch_core sketch with the inferred constraints, and its profiles.
pub fn to_sketch(o: &Outline, tol: Tol) -> Result<Sketched, Refusal> {
    let band = BAND_SIGMA * o.sigma_mm;
    let mut sk = Sketch::new();
    let mut cons: Vec<Constraint> = Vec::new();
    let mut rounds: Vec<(EntityId, Vec2, f64)> = Vec::new();
    for (li, lp) in core::iter::once(&o.outer).chain(o.holes.iter()).enumerate() {
        add_loop(&mut sk, lp, o.thin.get(li).map_or(&[][..], |t| t.as_slice()), &mut cons, &mut rounds)?;
    }
    for i in 0..rounds.len() {
        if let Some(j) = (0..i).find(|&j| (rounds[j].2 - rounds[i].2).abs() <= band) {
            cons.push(Constraint::EqualRadius(rounds[j].0, rounds[i].0));
        }
        if let Some(j) = (0..i).find(|&j| rounds[j].1.dist(rounds[i].1) <= band) {
            cons.push(Constraint::Coincident(PointRef::Centre(rounds[j].0), PointRef::Centre(rounds[i].0)));
        }
    }
    for c in cons {
        sk.add(c).map_err(bad)?;
    }
    let solved = solve(&mut sk, tol).map_err(Refusal::Sketch)?;
    let profiles = profiles(&sk, tol);
    Ok(Sketched { sketch: sk, solved, profiles })
}

// ---- VUGPHOTOOFFAXIS (SR670): Perpendicular / Parallel between a loop's own sides off the paper's axes (named tail) ----

/// The class representative of `k` (path-halving union-find over the loop's prims plus one paper root).
fn class_of(up: &mut [usize], mut k: usize) -> usize {
    while up[k] != k {
        up[k] = up[up[k]];
        k = up[k];
    }
    k
}

/// The arcs tangent to line `k` at its two junctions (the previous arc's `tangent_out`, the next arc's `tangent_in`).
fn tangent_arcs(prims: &[Prim], k: usize) -> [Option<usize>; 2] {
    let np = prims.len();
    let (pk, nk) = ((k + np - 1) % np, (k + 1) % np);
    let before = matches!(prims[pk], Prim::Arc { tangent_out: true, .. }).then_some(pk);
    let after = matches!(prims[nk], Prim::Arc { tangent_in: true, .. }).then_some(nk);
    [before, after]
}

/// One loop's off-axis relations, after the H / V arm and SR661's thin Parallel. A class is a set of lines whose
/// relative angles rows already hold (the paper root holds every axis line); a relation joining two classes is an
/// independent row, one inside a class would be redundant and is not pushed. Perpendicular first (consecutive lines),
/// then Parallel (non-consecutive lines, not two lines sharing two tangent arcs: the tangencies with EqualRadius hold
/// them parallel). A line with Horizontal / Vertical takes nothing more. The tolerance is [`HV_DEG`] on the H / V
/// arm's own angle.
fn off_axis(prims: &[Prim], ids: &[EntityId], axis: &[bool], deg: &[f64], thin: &[(usize, usize)], cons: &mut Vec<Constraint>) {
    let np = prims.len();
    let root = np;
    let mut up: Vec<usize> = (0..=np).collect();
    for k in 0..np {
        if axis[k] {
            let a = class_of(&mut up, k);
            up[a] = root;
        }
    }
    for &(i, j) in thin {
        if i < np && j < np && is_line(&prims[i]) && is_line(&prims[j]) && !(axis[i] && axis[j]) {
            let (a, b) = (class_of(&mut up, i), class_of(&mut up, j));
            if a != b {
                up[a] = b;
            }
        }
    }
    let free = |k: usize| is_line(&prims[k]) && !axis[k];
    let acute = |i: usize, j: usize| {
        let d = (deg[i] - deg[j]).abs();
        d.min(180.0 - d)
    };
    for k in 0..np {
        let q = (k + 1) % np;
        if q == k || !free(k) || !free(q) || acute(k, q) < 90.0 - HV_DEG {
            continue;
        }
        let (a, b) = (class_of(&mut up, k), class_of(&mut up, q));
        if a != b {
            cons.push(Constraint::Perpendicular(ids[k], ids[q]));
            up[a] = b;
        }
    }
    for i in 0..np {
        for j in (i + 1)..np {
            if j == (i + 1) % np || i == (j + 1) % np || !free(i) || !free(j) || acute(i, j) > HV_DEG {
                continue;
            }
            let (ti, tj) = (tangent_arcs(prims, i), tangent_arcs(prims, j));
            let shared = ti.iter().flatten().filter(|&&r| tj.iter().flatten().any(|&s| s == r)).count();
            if shared >= 2 {
                continue;
            }
            let (a, b) = (class_of(&mut up, i), class_of(&mut up, j));
            if a != b {
                cons.push(Constraint::Parallel(ids[i], ids[j]));
                up[a] = b;
            }
        }
    }
}
