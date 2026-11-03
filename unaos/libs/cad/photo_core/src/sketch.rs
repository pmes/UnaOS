// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Step 7: the sketch. The outline's primitives become sketch_core entities and the intent the measurement
//! supports becomes sketch_core's own constraints, applied exactly as inferred:
//! - consecutive lines share one point entity (the corner: no Coincident row);
//! - a line meeting an arc: the line's point `Coincident` with the arc's Start / End, plus `TangentLineCircle` when
//!   the junction was tangent within 3 σ; two arcs: their ends `Coincident`, plus `TangentCircles` when tangent;
//! - a line within [`HV_DEG`] of the paper's x / y axis: `Horizontal` / `Vertical`;
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
                } else if (ang - 90.0).abs() <= HV_DEG {
                    cons.push(Constraint::Vertical(e));
                }
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
    for lp in core::iter::once(&o.outer).chain(o.holes.iter()) {
        add_loop(&mut sk, lp, &mut cons, &mut rounds)?;
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
