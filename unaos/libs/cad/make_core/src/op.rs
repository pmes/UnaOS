// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The manufacturing intent attached to the design (BP-vug-3) and the toolpath generators for it.
//!
//! Generated today: `Drill` (a peck cycle as explicit moves), `Contour` (one closed profile offset by the tool
//! radius), `Pocket2_5D` (level-set rings inside-out, islands merged, a helical entry) and `Face` (zigzag passes over
//! a rectangle, entered from the side). The rest refuse by name with their owed id.

use alloc::vec::Vec;
use libm::ceil;

use crate::geom::{Length, Tol, Vec2, Vec3, line_line2, seg_seg2, signed_area2};
use crate::machine::{Machine, Tool};
use crate::program::{Move, Program, Spin};
use crate::slice::SliceParams; // VUGMAKESLICE (SR483)
use crate::validate::Refusal;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Outside,
    Inside,
}

/// Feeds and speed for a cutting Op (units/min, rpm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feeds {
    pub feed: f64,
    pub plunge: f64,
    pub rpm: f64,
}

/// An operation on a feature or a body. `top` is the surface Z; `depth` is how far below it the cut goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Drill { points: Vec<Vec2>, top: f64, depth: f64, peck: f64, tool: u32, feeds: Feeds },
    Contour { profile: Vec<Vec2>, top: f64, depth: f64, side: Side, tool: u32, feeds: Feeds },
    /// `chord`: the most material a flattened corner arc may leave; `climb`: climb milling with the M3 spindle.
    Pocket2_5D {
        outline: Vec<Vec2>,
        islands: Vec<Vec<Vec2>>,
        top: f64,
        depth: f64,
        stepover: f64,
        chord: f64,
        climb: bool,
        tool: u32,
        feeds: Feeds,
    },
    /// The rectangle `lo`..`hi` faced by parallel passes along `along`.
    Face { lo: Vec2, hi: Vec2, top: f64, depth: f64, stepover: f64, along: FaceDir, tool: u32, feeds: Feeds },
    PrintLayers { slices: Vec<(f64, Vec<Vec<Vec2>>)>, tool: u32 },
    LaserCut { polylines: Vec<Vec<Vec2>>, passes: u32 },
    Slice { triangles: Vec<[Vec3; 3]>, params: SliceParams }, // VUGMAKESLICE (SR483): FDM slicing of a triangle soup
}

/// The toolpath for `op` on `m`, or the refusal by name.
pub fn generate(op: &Op, m: &Machine, tol: Tol) -> Result<Program, Refusal> {
    match op {
        Op::Drill { points, top, depth, peck, tool, feeds } => drill(m, points, *top, *depth, *peck, *tool, feeds, tol),
        Op::Contour { profile, top, depth, side, tool, feeds } => {
            contour(m, profile, *top, *depth, *side, *tool, feeds, tol)
        }
        Op::Pocket2_5D { outline, islands, top, depth, stepover, chord, climb, tool, feeds } => {
            pocket(m, outline, islands, *top, *depth, *stepover, *chord, *climb, *tool, feeds, tol)
        }
        Op::Face { lo, hi, top, depth, stepover, along, tool, feeds } => {
            face(m, *lo, *hi, *top, *depth, *stepover, *along, *tool, feeds, tol)
        }
        Op::PrintLayers { .. } => Err(Refusal::OpOwed { op: "PrintLayers", owed: "VUG-MAKE-SLICE" }),
        Op::LaserCut { .. } => Err(Refusal::OpOwed { op: "LaserCut", owed: "VUG-MAKE-LASER" }),
        Op::Slice { triangles, params } => crate::slice::slice(triangles, params, m, tol), // VUGMAKESLICE (SR483)
    }
}

/// The tool, a change to it if it is not loaded, and the spindle on.
fn prelude<'m>(m: &'m Machine, tool: u32, rpm: f64) -> Result<(Program, &'m Tool), Refusal> {
    let t = m.tool(tool).ok_or(Refusal::UnknownTool { at: 0, tool })?;
    let mut p = Program::new(m.units);
    if m.loaded != Some(tool) {
        p.moves.push(Move::ToolChange { tool });
    }
    p.moves.push(Move::Spindle(Spin::Cw(rpm)));
    Ok((p, t))
}

/// A peck cycle as explicit moves: feed down one peck, rapid out to the safe Z, FEED back to the last depth (not
/// G83's rapid: without VUG-MAKE-SIM the validator cannot know the hole is cut), the next peck, and so on.
#[allow(clippy::too_many_arguments)]
fn drill(
    m: &Machine,
    points: &[Vec2],
    top: f64,
    depth: f64,
    peck: f64,
    tool: u32,
    f: &Feeds,
    tol: Tol,
) -> Result<Program, Refusal> {
    if !(depth > tol.lin) {
        return Err(Refusal::DepthAboveTop);
    }
    let (mut p, _) = prelude(m, tool, f.rpm)?;
    let bottom = top - depth;
    let peck = if peck > tol.lin { peck } else { depth };
    for q in points {
        let at = |z: f64| Vec3::new(q.x, q.y, z);
        p.moves.push(Move::Rapid { to: at(m.safe_z) });
        let mut last: Option<f64> = None;
        let mut z = top;
        loop {
            let next = if z - peck <= bottom + tol.lin { bottom } else { z - peck };
            if let Some(l) = last {
                p.moves.push(Move::Linear { to: at(l), feed: f.plunge });
            }
            p.moves.push(Move::Linear { to: at(next), feed: f.plunge });
            p.moves.push(Move::Rapid { to: at(m.safe_z) });
            if next <= bottom {
                break;
            }
            last = Some(next);
            z = next;
        }
    }
    p.moves.push(Move::Spindle(Spin::Off));
    Ok(p)
}

/// One element of an offset path.
#[derive(Clone, Copy)]
enum El {
    Line { from: Vec2, to: Vec2 },
    Arc { from: Vec2, to: Vec2, centre: Vec2, cw: bool },
}

impl El {
    fn chord(&self) -> (Vec2, Vec2) {
        match *self {
            El::Line { from, to } | El::Arc { from, to, .. } => (from, to),
        }
    }
}

/// The closed profile (made counter-clockwise) offset by the tool radius to `side`: the side's corners that turn
/// away from the tool get an arc about the vertex (outside: convex corners, CCW arcs), the others the offset lines'
/// intersection. A collapsed edge refuses `ToolTooLarge`; crossing elements refuse `ContourSelfIntersects` (arcs are
/// tested by their chords).
fn offset_path(profile: &[Vec2], r: f64, side: Side, tool: u32, tol: Tol) -> Result<Vec<El>, Refusal> {
    let mut pts: Vec<Vec2> = profile.to_vec();
    if pts.len() >= 2 && (pts[0] - pts[pts.len() - 1]).length() <= tol.lin {
        pts.pop();
    }
    if pts.len() < 3 {
        return Err(Refusal::ContourDegenerate { what: "fewer than 3 points" });
    }
    let area = signed_area2(&pts);
    if libm::fabs(area) <= tol.lin * tol.lin {
        return Err(Refusal::ContourDegenerate { what: "zero area" });
    }
    if area < 0.0 {
        pts.reverse();
    }
    let n = pts.len();
    let mut dir = Vec::with_capacity(n);
    for i in 0..n {
        let d = pts[(i + 1) % n] - pts[i];
        let l = d.length();
        if l <= tol.lin {
            return Err(Refusal::ContourDegenerate { what: "zero-length edge" });
        }
        dir.push(d * (1.0 / l));
    }
    let s = if side == Side::Outside { 1.0 } else { -1.0 };
    // The outward normal of a CCW edge is its right-hand side.
    let off = |i: usize| Vec2::new(dir[i].y, -dir[i].x) * (s * r);
    let mut start = alloc::vec![Vec2::default(); n];
    let mut end = alloc::vec![Vec2::default(); n];
    let mut corner: Vec<Option<El>> = alloc::vec![None; n];
    for j in 0..n {
        let i = (j + n - 1) % n; // the edge into vertex j
        let v = pts[j];
        let turn = dir[i].cross(dir[j]);
        if libm::fabs(turn) <= tol.ang {
            if dir[i].dot(dir[j]) < 0.0 {
                return Err(Refusal::ContourDegenerate { what: "edge reverses" });
            }
            end[i] = v + off(j);
            start[j] = v + off(j);
        } else if (s > 0.0 && turn > 0.0) || (s < 0.0 && turn < 0.0) {
            end[i] = v + off(i);
            start[j] = v + off(j);
            corner[j] = Some(El::Arc { from: end[i], to: start[j], centre: v, cw: s < 0.0 });
        } else {
            let q = line_line2(pts[i] + off(i), dir[i], pts[j] + off(j), dir[j], tol)
                .ok_or(Refusal::ContourDegenerate { what: "parallel corner" })?;
            end[i] = q;
            start[j] = q;
        }
    }
    let mut els = Vec::with_capacity(2 * n);
    for i in 0..n {
        if (end[i] - start[i]).dot(dir[i]) <= tol.lin {
            return Err(Refusal::ToolTooLarge { tool });
        }
        els.push(El::Line { from: start[i], to: end[i] });
        if let Some(c) = corner[(i + 1) % n] {
            els.push(c);
        }
    }
    let k = els.len();
    for a in 0..k {
        for b in (a + 2)..k {
            if a == 0 && b == k - 1 {
                continue; // the closing pair is adjacent
            }
            let (p1, p2) = els[a].chord();
            let (p3, p4) = els[b].chord();
            if seg_seg2(p1, p2, p3, p4, tol) {
                return Err(Refusal::ContourSelfIntersects);
            }
        }
    }
    Ok(els)
}

#[allow(clippy::too_many_arguments)]
fn contour(
    m: &Machine,
    profile: &[Vec2],
    top: f64,
    depth: f64,
    side: Side,
    tool: u32,
    f: &Feeds,
    tol: Tol,
) -> Result<Program, Refusal> {
    if !(depth > tol.lin) {
        return Err(Refusal::DepthAboveTop);
    }
    let t = *m.tool(tool).ok_or(Refusal::UnknownTool { at: 0, tool })?;
    let els = offset_path(profile, t.radius(), side, tool, tol)?;
    let (mut p, _) = prelude(m, tool, f.rpm)?;
    let passes = if t.apmx > tol.lin { ceil((depth - tol.lin) / t.apmx).max(1.0) as usize } else { 1 };
    let start = els[0].chord().0;
    p.moves.push(Move::Rapid { to: Vec3::new(start.x, start.y, m.safe_z) });
    for k in 1..=passes {
        let z = top - depth * (k as f64) / (passes as f64);
        p.moves.push(Move::Linear { to: Vec3::new(start.x, start.y, z), feed: f.plunge });
        for e in &els {
            match *e {
                El::Line { to, .. } => p.moves.push(Move::Linear { to: Vec3::new(to.x, to.y, z), feed: f.feed }),
                El::Arc { to, centre, cw, .. } => {
                    p.moves.push(Move::Arc { to: Vec3::new(to.x, to.y, z), centre, cw, feed: f.feed })
                }
            }
        }
        p.moves.push(Move::Rapid { to: Vec3::new(start.x, start.y, m.safe_z) });
    }
    p.moves.push(Move::Spindle(Spin::Off));
    Ok(p)
}

// ---- VUGMAKEPOCKET (LEDGER SR479): 2.5D pocketing by level-set rings and facing by zigzag passes. ----

/// The direction a face pass runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceDir {
    X,
    Y,
}

/// The helix's rise over its run (~2.9°): pitch per turn = 2π·rh·HELIX_SLOPE.
const HELIX_SLOPE: f64 = 0.05;

/// The depth passes APMX allows (Contour's rule; one when APMX is unset).
fn passes(t: &Tool, depth: f64, tol: Tol) -> usize {
    if t.apmx > tol.lin { ceil((depth - tol.lin) / t.apmx).max(1.0) as usize } else { 1 }
}

/// Whether the tool may plunge straight into solid. `Tool` carries no centre-cutting flag (VUG-MAKE-TOOLCC): a ball
/// end and a drill cut at the centre; a flat end is assumed not to.
fn centre_cutting(t: &Tool) -> bool {
    matches!(t.kind, crate::machine::ToolKind::BallEnd | crate::machine::ToolKind::Drill)
}

/// Down from the safe Z into the cut at `z` beside ring point `c`: fed to the cleared `floor`, then a helix of radius
/// min(r/2, `room`) about `c` at the plunge feed (full turns to `z`, one flat turn, a line to `c`), or, with less than
/// r/4 of room, a straight plunge for a centre-cutting tool and `NoEntry` for any other.
#[allow(clippy::too_many_arguments)]
fn enter(
    p: &mut Program,
    m: &Machine,
    t: &Tool,
    c: Vec2,
    room: f64,
    floor: f64,
    z: f64,
    cw: bool,
    f: &Feeds,
    tol: Tol,
) -> Result<(), Refusal> {
    let r = t.radius();
    let rh = (0.5 * r).min(room);
    if rh >= 0.25 * r {
        let s = c + Vec2::new(rh, 0.0);
        p.moves.push(Move::Rapid { to: Vec3::new(s.x, s.y, m.safe_z) });
        p.moves.push(Move::Linear { to: Vec3::new(s.x, s.y, floor), feed: f.plunge });
        let pitch = 2.0 * core::f64::consts::PI * rh * HELIX_SLOPE;
        let turns = ceil((floor - z - tol.lin) / pitch).max(1.0) as usize;
        for k in 1..=turns {
            let zk = floor - (floor - z) * (k as f64) / (turns as f64);
            p.moves.push(Move::Arc { to: Vec3::new(s.x, s.y, zk), centre: c, cw, feed: f.plunge });
        }
        p.moves.push(Move::Arc { to: Vec3::new(s.x, s.y, z), centre: c, cw, feed: f.plunge });
        p.moves.push(Move::Linear { to: Vec3::new(c.x, c.y, z), feed: f.feed });
    } else if centre_cutting(t) {
        p.moves.push(Move::Rapid { to: Vec3::new(c.x, c.y, m.safe_z) });
        p.moves.push(Move::Linear { to: Vec3::new(c.x, c.y, floor), feed: f.plunge });
        p.moves.push(Move::Linear { to: Vec3::new(c.x, c.y, z), feed: f.plunge });
    } else {
        return Err(Refusal::NoEntry { tool: t.number });
    }
    Ok(())
}

/// The pocket: ring k at distance r + k·stepover from the outline and islands ([`crate::offset::rings`]), cut
/// inside-out per depth pass so the wall ring (distance r) is the last, finishing pass; climb runs each ring against
/// its domain-left direction. A ring is reached by a fed link when the link is no longer than DC and keeps r from the
/// boundary, else by a retract, a rapid at the safe Z and a fresh entry.
///
/// The fresh entry is first a CLEARED DROP ([`cleared_drop`]): down through a column already cut at this depth pass
/// (any tool may: nothing is left there to cut), then a fed link to the ring under the same length and clearance gate.
/// Only with no such drop does it helix in (room k·stepover at level k) or plunge or refuse `NoEntry`. Without the drop
/// a pocket whose level 0 holds two rings far apart (the wall ring and the ring about a central island) re-entered
/// its second ring with room 0, and a flat end refused an ordinary island pocket. The first ring of each depth pass
/// is the innermost ring's helix entry.
#[allow(clippy::too_many_arguments)]
fn pocket(
    m: &Machine,
    outline: &[Vec2],
    islands: &[Vec<Vec2>],
    top: f64,
    depth: f64,
    stepover: f64,
    chord: f64,
    climb: bool,
    tool: u32,
    f: &Feeds,
    tol: Tol,
) -> Result<Program, Refusal> {
    if !(depth > tol.lin) {
        return Err(Refusal::DepthAboveTop);
    }
    let t = *m.tool(tool).ok_or(Refusal::UnknownTool { at: 0, tool })?;
    let r = t.radius();
    if !(stepover > tol.lin && stepover <= t.dc + tol.lin) {
        return Err(Refusal::BadParameter { what: "stepover outside (0, DC]" });
    }
    if !(chord > tol.lin) {
        return Err(Refusal::BadParameter { what: "chord not positive" });
    }
    let lps = crate::offset::loops(outline, islands, tol)?;
    let mut levels: Vec<Vec<Vec<Vec2>>> = Vec::new();
    loop {
        let d = r + stepover * (levels.len() as f64);
        let rs = crate::offset::rings(&lps, d, chord, tol)?;
        if rs.is_empty() {
            break;
        }
        levels.push(rs);
    }
    if levels.is_empty() {
        return Err(Refusal::ToolTooLarge { tool });
    }
    if climb {
        for lv in levels.iter_mut() {
            for ring in lv.iter_mut() {
                ring.reverse();
            }
        }
    }
    let (mut p, _) = prelude(m, tool, f.rpm)?;
    let n = passes(&t, depth, tol);
    let mut floor = top;
    for j in 1..=n {
        let z = top - depth * (j as f64) / (n as f64);
        let mut at: Option<Vec2> = None;
        let mut cut: Vec<Vec<Vec2>> = Vec::new();
        for k in (0..levels.len()).rev() {
            let mut left: Vec<Vec<Vec2>> = levels[k].clone();
            while !left.is_empty() {
                let mut best = (0usize, 0usize, left[0][0], f64::INFINITY);
                if let Some(a) = at {
                    for (i, ring) in left.iter().enumerate() {
                        let (dd, e, q) = crate::offset::nearest(a, ring);
                        if dd < best.3 {
                            best = (i, e, q, dd);
                        }
                    }
                }
                let (ri, e, q, dd) = best;
                let orig = left.swap_remove(ri);
                let mut ring = crate::offset::start_at(&orig, e, q, tol);
                let linked = match at {
                    Some(a) => dd <= t.dc + tol.lin && crate::offset::clearance(a, ring[0], &lps, tol) >= r - tol.lin,
                    None => false,
                };
                if linked {
                    p.moves.push(Move::Linear { to: Vec3::new(ring[0].x, ring[0].y, z), feed: f.feed });
                } else {
                    if let Some(a) = at {
                        p.moves.push(Move::Rapid { to: Vec3::new(a.x, a.y, m.safe_z) });
                    }
                    match cleared_drop(&cut, &orig, &lps, &t, tol) {
                        Some((pd, e2, q2)) => {
                            ring = crate::offset::start_at(&orig, e2, q2, tol);
                            p.moves.push(Move::Rapid { to: Vec3::new(pd.x, pd.y, m.safe_z) });
                            p.moves.push(Move::Linear { to: Vec3::new(pd.x, pd.y, z), feed: f.plunge });
                            p.moves.push(Move::Linear { to: Vec3::new(q2.x, q2.y, z), feed: f.feed });
                        }
                        None => enter(&mut p, m, &t, ring[0], stepover * (k as f64), floor, z, climb, f, tol)?,
                    }
                }
                let c = ring[0];
                for v in &ring[1..] {
                    p.moves.push(Move::Linear { to: Vec3::new(v.x, v.y, z), feed: f.feed });
                }
                p.moves.push(Move::Linear { to: Vec3::new(c.x, c.y, z), feed: f.feed });
                cut.push(ring);
                at = Some(c);
            }
        }
        if let Some(a) = at {
            p.moves.push(Move::Rapid { to: Vec3::new(a.x, a.y, m.safe_z) });
        }
        floor = z;
    }
    p.moves.push(Move::Spindle(Spin::Off));
    Ok(p)
}

/// The cleared drop to `target`: the closest pair of a point on a ring already cut at this depth pass (`cut`) and a
/// point on `target` (a closest pair of two polylines has a vertex of one of them), whose link is no longer than DC
/// and keeps the tool radius from the boundary: (the drop point, the target edge, the target point).
fn cleared_drop(cut: &[Vec<Vec2>], target: &[Vec2], lps: &[Vec<Vec2>], t: &Tool, tol: Tol) -> Option<(Vec2, usize, Vec2)> {
    let r = t.radius();
    let mut best: Option<(f64, Vec2, usize, Vec2)> = None;
    let mut consider = |dd: f64, pd: Vec2, e: usize, q: Vec2| {
        if dd <= t.dc + tol.lin
            && best.map_or(true, |b| dd < b.0)
            && crate::offset::clearance(pd, q, lps, tol) >= r - tol.lin
        {
            best = Some((dd, pd, e, q));
        }
    };
    for done in cut {
        for v in done {
            let (dd, e, q) = crate::offset::nearest(*v, target);
            consider(dd, *v, e, q);
        }
        for (e, u) in target.iter().enumerate() {
            let (dd, _, pd) = crate::offset::nearest(*u, done);
            consider(dd, pd, e, *u);
        }
    }
    best.map(|b| (b.1, b.2, b.3))
}

/// The face: rows evenly spaced from `lo` to `hi` across `along`, no further apart than `stepover`, each run from 2r
/// beyond the rectangle to 2r beyond its far side (the lead-in: the pass enters the material from the side, never
/// from above), alternating direction, the step between rows outside the rectangle; one set per APMX depth pass.
#[allow(clippy::too_many_arguments)]
fn face(
    m: &Machine,
    lo: Vec2,
    hi: Vec2,
    top: f64,
    depth: f64,
    stepover: f64,
    along: FaceDir,
    tool: u32,
    f: &Feeds,
    tol: Tol,
) -> Result<Program, Refusal> {
    if !(depth > tol.lin) {
        return Err(Refusal::DepthAboveTop);
    }
    let t = *m.tool(tool).ok_or(Refusal::UnknownTool { at: 0, tool })?;
    if !(lo.x <= hi.x && lo.y <= hi.y) {
        return Err(Refusal::BadParameter { what: "face rectangle min above max" });
    }
    if !(stepover > tol.lin && stepover <= t.dc + tol.lin) {
        return Err(Refusal::BadParameter { what: "stepover outside (0, DC]" });
    }
    let r = t.radius();
    let (u0, u1, v0, v1) = match along {
        FaceDir::X => (lo.x, hi.x, lo.y, hi.y),
        FaceDir::Y => (lo.y, hi.y, lo.x, hi.x),
    };
    let at = |u: f64, v: f64, z: f64| match along {
        FaceDir::X => Vec3::new(u, v, z),
        FaceDir::Y => Vec3::new(v, u, z),
    };
    let (ua, ub) = (u0 - 2.0 * r, u1 + 2.0 * r);
    let span = v1 - v0;
    let rows = if span > tol.lin { ceil((span - tol.lin) / stepover).max(1.0) as usize + 1 } else { 1 };
    let (mut p, _) = prelude(m, tool, f.rpm)?;
    let n = passes(&t, depth, tol);
    for j in 1..=n {
        let z = top - depth * (j as f64) / (n as f64);
        p.moves.push(Move::Rapid { to: at(ua, v0, m.safe_z) });
        p.moves.push(Move::Linear { to: at(ua, v0, z), feed: f.plunge });
        let mut forward = true;
        for i in 0..rows {
            let v = if rows > 1 { v0 + span * (i as f64) / ((rows - 1) as f64) } else { v0 };
            let from = if forward { ua } else { ub };
            if i > 0 {
                p.moves.push(Move::Linear { to: at(from, v, z), feed: f.feed });
            }
            let to = if forward { ub } else { ua };
            p.moves.push(Move::Linear { to: at(to, v, z), feed: f.feed });
            forward = !forward;
        }
        let end = if forward { ua } else { ub };
        let v_end = if rows > 1 { v1 } else { v0 };
        p.moves.push(Move::Rapid { to: at(end, v_end, m.safe_z) });
    }
    p.moves.push(Move::Spindle(Spin::Off));
    Ok(p)
}
