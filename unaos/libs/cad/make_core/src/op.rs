// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The manufacturing intent attached to the design (BP-vug-3) and the toolpath generators for it.
//!
//! Generated today: `Drill` (a peck cycle as explicit moves), `Contour` (one closed profile offset by the tool
//! radius), `Pocket2_5D` (level-set rings inside-out, islands merged, a helical entry, or the tool's ramp or helix at
//! its `ramp_deg`: VUGMAKERAMP SR532; the plunge where the tool cuts at its centre, `centre_cutting` by flag or kind:
//! VUGMAKETOOLCC SR538) and `Face` (zigzag passes over
//! a rectangle, entered from the side). The rest refuse by name with their owed id.

use alloc::vec::Vec;
use libm::ceil;

use crate::geom::{Length, Tol, Vec2, Vec3};
use crate::machine::{Machine, Tool};
use crate::offset::El; // VUGMAKEARCFIT (SR490): the one offset's pieces
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
    /// `chord`: the most material a flattened corner arc may leave (validated positive; the pocket emits its corner
    /// arcs exact as G2 / G3 since VUGMAKEARCFIT SR490, so it bounds only a consumer that flattens: the ramp entry
    /// rides the first ring flattened at `chord`, VUGMAKERAMP SR532); `climb`: climb milling with the M3 spindle.
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
    /// VUGMAKEREST (SR517): what a `Pocket2_5D` of the same outline and islands with tool `after` at `stepover` left
    /// (its corners and any core), cut with the smaller `tool` from the cleared side ([`crate::rest`]).
    Rest {
        outline: Vec<Vec2>,
        islands: Vec<Vec<Vec2>>,
        top: f64,
        depth: f64,
        after: u32,
        stepover: f64,
        climb: bool,
        tool: u32,
        feeds: Feeds,
    },
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
        // VUGMAKEREST (SR517): the rest of a pocket, cut with a smaller tool.
        Op::Rest { outline, islands, top, depth, after, stepover, climb, tool, feeds } => {
            crate::rest::rest(m, outline, islands, *top, *depth, *after, *stepover, *climb, *tool, feeds, tol)
        }
    }
}

/// The tool, a change to it if it is not loaded, and the spindle on.
pub(crate) fn prelude<'m>(m: &'m Machine, tool: u32, rpm: f64) -> Result<(Program, &'m Tool), Refusal> {
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

// ---- VUGMAKEARCFIT (LEDGER SR490): Contour through the one offset (`offset::rings` at d = r). ----

/// An arc whose chord is no longer than this is emitted as a line: on the dialect writers' 1e-4 grid its two ends
/// could round together and a controller would read a FULL turn (the sagitta at r >= 2 is under 1e-7).
const ARC_MIN_CHORD: f64 = 1e-3;

/// The ring's pieces at `z`: a segment as `Move::Linear`, an arc as `Move::Arc` (G2 / G3).
pub(crate) fn emit(p: &mut Program, ring: &[El], z: f64, feed: f64) {
    for e in ring {
        match *e {
            El::Arc { from, to, centre, cw } if (to - from).length() > ARC_MIN_CHORD => {
                p.moves.push(Move::Arc { to: Vec3::new(to.x, to.y, z), centre, cw, feed })
            }
            _ => {
                let to = e.end();
                p.moves.push(Move::Linear { to: Vec3::new(to.x, to.y, z), feed })
            }
        }
    }
}

/// The closed profile cut along the rings at the tool radius on `side` ([`crate::offset::rings`] over
/// [`crate::offset::side_loop`]): the corners that turn away from the tool get an exact arc about the vertex
/// (outside: convex corners), the others the offset lines' intersection; each ring run counter-clockwise from the
/// piece nearest the profile's first vertex offset along its first edge. No ring refuses `ToolTooLarge`; a feature
/// narrower than DC is not entered (what it leaves is VUG-MAKE-REST's); a self-crossing profile refuses
/// `ContourSelfIntersects`.
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
    let r = t.radius();
    let outside = side == Side::Outside;
    let lps = crate::offset::side_loop(profile, outside, tol)?;
    let mut rs = crate::offset::rings(&lps, r, tol)?;
    if rs.is_empty() {
        return Err(Refusal::ToolTooLarge { tool });
    }
    // The outside loop is clockwise (the cut side on its left); Contour runs counter-clockwise on both sides.
    let ccw: Vec<Vec2> = if outside { lps[0].iter().rev().copied().collect() } else { lps[0].clone() };
    let e0 = ccw[1] - ccw[0];
    let aim = ccw[0] + Vec2::new(e0.y, -e0.x) * ((if outside { r } else { -r }) / e0.length());
    for ring in rs.iter_mut() {
        if outside {
            *ring = crate::offset::reversed_ring(ring);
        }
        let mut k = 0;
        for i in 1..ring.len() {
            if (ring[i].start() - aim).length() < (ring[k].start() - aim).length() {
                k = i;
            }
        }
        ring.rotate_left(k);
    }
    let (mut p, _) = prelude(m, tool, f.rpm)?;
    let n = passes(&t, depth, tol);
    for ring in &rs {
        let start = ring[0].start();
        p.moves.push(Move::Rapid { to: Vec3::new(start.x, start.y, m.safe_z) });
        for k in 1..=n {
            let z = top - depth * (k as f64) / (n as f64);
            p.moves.push(Move::Linear { to: Vec3::new(start.x, start.y, z), feed: f.plunge });
            emit(&mut p, ring, z, f.feed);
            p.moves.push(Move::Rapid { to: Vec3::new(start.x, start.y, m.safe_z) });
        }
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
pub(crate) fn passes(t: &Tool, depth: f64, tol: Tol) -> usize {
    if t.apmx > tol.lin { ceil((depth - tol.lin) / t.apmx).max(1.0) as usize } else { 1 }
}

/// Whether the tool may plunge straight into solid: the tool's own `centre_cutting` flag first (VUGMAKETOOLCC SR538),
/// and with None the rule by kind: a ball end and a drill cut at the centre; a flat end is assumed not to.
pub(crate) fn centre_cutting(t: &Tool) -> bool {
    use crate::machine::ToolKind;
    t.centre_cutting.unwrap_or_else(|| matches!(t.kind, ToolKind::BallEnd | ToolKind::Drill))
}

/// Down from the safe Z into the cut at `z` beside ring point `c`: fed to the cleared `floor`, then a helix of radius
/// min(r/2, `room`) about `c` at the plunge feed (full turns to `z`, one flat turn, a line to `c`), or, with less than
/// r/4 of room, a straight plunge for a centre-cutting tool and `NoEntry` for any other. A tool with a `ramp_deg`
/// enters by [`ramp_entry`] instead (VUGMAKERAMP SR532: a ramp along `ring`, which starts at `c`, or a helix at the
/// angle), falling back to the plunge or `NoEntry` only when neither fits.
#[allow(clippy::too_many_arguments)]
fn enter(
    p: &mut Program,
    m: &Machine,
    t: &Tool,
    c: Vec2,
    ring: &[El],
    room: f64,
    chord: f64,
    floor: f64,
    z: f64,
    cw: bool,
    f: &Feeds,
    tol: Tol,
) -> Result<(), Refusal> {
    let r = t.radius();
    let rh = (0.5 * r).min(room);
    // VUGMAKERAMP (SR532): the entry chooser — the maker's angle governs when the tool carries one.
    let ramped = match t.ramp_deg {
        Some(a) => ramp_entry(p, m, t, ring, a, chord, floor, z, cw, f, tol)?,
        None => false,
    };
    if ramped {
        // Entered at the ramp angle: ramped along the ring or helixed about its deepest point.
    } else if t.ramp_deg.is_none() && rh >= 0.25 * r {
        helix(p, m, c, rh, HELIX_SLOPE, floor, z, cw, c, f, tol); // VUGMAKERAMP (SR532): the one helix
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
    if let Some(a) = t.ramp_deg {
        // VUGMAKERAMP (SR532): the ramp angle a maker can mean.
        if !(a > 0.0 && a < 90.0) {
            return Err(Refusal::BadParameter { what: "ramp angle outside (0, 90) degrees" });
        }
    }
    let lps = crate::offset::loops(outline, islands, tol)?;
    let mut levels = levels(&lps, r, stepover, tool, tol)?; // VUGMAKEREST (SR517): the ring set, shared with the rest
    if climb {
        for lv in levels.iter_mut() {
            for ring in lv.iter_mut() {
                *ring = crate::offset::reversed_ring(ring);
            }
        }
    }
    let (mut p, _) = prelude(m, tool, f.rpm)?;
    let n = passes(&t, depth, tol);
    let mut floor = top;
    for j in 1..=n {
        let z = top - depth * (j as f64) / (n as f64);
        let mut at: Option<Vec2> = None;
        let mut cut: Vec<Vec<El>> = Vec::new();
        for k in (0..levels.len()).rev() {
            let mut left: Vec<Vec<El>> = levels[k].clone();
            while !left.is_empty() {
                let mut best = (0usize, 0usize, left[0][0].start(), f64::INFINITY);
                if let Some(a) = at {
                    for (i, ring) in left.iter().enumerate() {
                        let (dd, e, q) = crate::offset::nearest(a, ring, tol);
                        if dd < best.3 {
                            best = (i, e, q, dd);
                        }
                    }
                }
                let (ri, e, q, dd) = best;
                let orig = left.swap_remove(ri);
                let mut ring = crate::offset::start_at(&orig, e, q, tol);
                let linked = match at {
                    Some(a) => dd <= t.dc + tol.lin && crate::offset::clearance(a, ring[0].start(), &lps, tol) >= r - tol.lin,
                    None => false,
                };
                if linked {
                    let s0 = ring[0].start();
                    p.moves.push(Move::Linear { to: Vec3::new(s0.x, s0.y, z), feed: f.feed });
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
                        None => enter(&mut p, m, &t, ring[0].start(), &ring, stepover * (k as f64), chord, floor, z, climb, f, tol)?,
                    }
                }
                let c = ring[0].start();
                emit(&mut p, &ring, z, f.feed); // VUGMAKEARCFIT (SR490): segments and exact corner arcs
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
/// point on `target` (a closest pair of two polylines has a vertex of one of them; with arcs the piece starts are the
/// candidates, VUG-MAKE-ARCFIT-DROP owed), whose link is no longer than DC and keeps the tool radius from the
/// boundary: (the drop point, the target piece, the target point).
fn cleared_drop(cut: &[Vec<El>], target: &[El], lps: &[Vec<Vec2>], t: &Tool, tol: Tol) -> Option<(Vec2, usize, Vec2)> {
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
            let (dd, e, q) = crate::offset::nearest(v.start(), target, tol);
            consider(dd, v.start(), e, q);
        }
        for (e, u) in target.iter().enumerate() {
            let (dd, _, pd) = crate::offset::nearest(u.start(), done, tol);
            consider(dd, pd, e, u.start());
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

// ---- VUGMAKEREST (LEDGER SR517): the pocket's ring set, read again by the rest. ----

/// The pocket's rings: level k the rings at distance r + k·stepover from the boundary `lps` ([`crate::offset::rings`]),
/// until a level is empty; none at all refuses `ToolTooLarge`.
pub(crate) fn levels(
    lps: &[Vec<Vec2>],
    r: f64,
    stepover: f64,
    tool: u32,
    tol: Tol,
) -> Result<Vec<Vec<Vec<El>>>, Refusal> {
    let mut levels: Vec<Vec<Vec<El>>> = Vec::new();
    loop {
        let d = r + stepover * (levels.len() as f64);
        let rs = crate::offset::rings(lps, d, tol)?;
        if rs.is_empty() {
            break;
        }
        levels.push(rs);
    }
    if levels.is_empty() {
        return Err(Refusal::ToolTooLarge { tool });
    }
    Ok(levels)
}

// ---- VUGMAKERAMP (LEDGER SR532): the ramp and helix entry at the tool's ramp angle. ----

/// The helix down into the cut at `z`: a rapid to `c` + (rh, 0) at the safe Z, fed to the cleared `floor`, full turns
/// about `c` at the plunge feed with a rise of `slope` over the run (pitch 2π·rh·slope) to `z`, one flat turn, and a
/// line at the cutting feed to `then`. `enter`'s helix about the ring point and the ramp angle's helix about the ring's
/// deepest point are both this one.
#[allow(clippy::too_many_arguments)]
fn helix(
    p: &mut Program,
    m: &Machine,
    c: Vec2,
    rh: f64,
    slope: f64,
    floor: f64,
    z: f64,
    cw: bool,
    then: Vec2,
    f: &Feeds,
    tol: Tol,
) {
    let s = c + Vec2::new(rh, 0.0);
    p.moves.push(Move::Rapid { to: Vec3::new(s.x, s.y, m.safe_z) });
    p.moves.push(Move::Linear { to: Vec3::new(s.x, s.y, floor), feed: f.plunge });
    let pitch = 2.0 * core::f64::consts::PI * rh * slope;
    let turns = ceil((floor - z - tol.lin) / pitch).max(1.0) as usize;
    for k in 1..=turns {
        let zk = floor - (floor - z) * (k as f64) / (turns as f64);
        p.moves.push(Move::Arc { to: Vec3::new(s.x, s.y, zk), centre: c, cw, feed: f.plunge });
    }
    p.moves.push(Move::Arc { to: Vec3::new(s.x, s.y, z), centre: c, cw, feed: f.plunge });
    p.moves.push(Move::Linear { to: Vec3::new(then.x, then.y, z), feed: f.feed });
}

/// The entry at the tool's ramp angle `deg` into `ring` (which the caller then cuts whole at `z`): with the ring
/// flattened at `chord` ([`crate::offset::flatten`], arcs circumscribed: never toward the boundary) of length P and the
/// ramp's run L = (floor - z) / tan(deg), a RAMP when P >= L (G1 with Z down the ring from its start, then the lap
/// flat); else a HELIX about the ring's deepest point at radius min(rho, r - tol) (under the tool radius: no post is
/// left at its centre) when that is at least `ARC_MIN_CHORD` (a smaller full-turn arc's centre offset is under the
/// writers' 1e-4 grid); else false (the caller plunges a centre-cutting tool or refuses `NoEntry`).
#[allow(clippy::too_many_arguments)]
fn ramp_entry(
    p: &mut Program,
    m: &Machine,
    t: &Tool,
    ring: &[El],
    deg: f64,
    chord: f64,
    floor: f64,
    z: f64,
    cw: bool,
    f: &Feeds,
    tol: Tol,
) -> Result<bool, Refusal> {
    let poly = crate::offset::flatten(ring, chord, tol);
    let n = poly.len();
    let per: f64 = (0..n).map(|i| (poly[(i + 1) % n] - poly[i]).length()).sum();
    let slope = libm::tan(deg * (core::f64::consts::PI / 180.0));
    let run = (floor - z) / slope;
    if per >= run - tol.lin {
        let s0 = poly[0];
        p.moves.push(Move::Rapid { to: Vec3::new(s0.x, s0.y, m.safe_z) });
        p.moves.push(Move::Linear { to: Vec3::new(s0.x, s0.y, floor), feed: f.plunge });
        ramp_emit(p, &poly, floor, z, run, f, tol);
        return Ok(true);
    }
    let (c, rho) = inscribed(&poly, per, tol)?;
    let rh = rho.min(t.radius() - tol.lin);
    if rh >= ARC_MIN_CHORD {
        helix(p, m, c, rh, slope, floor, z, cw, ring[0].start(), f, tol);
        return Ok(true);
    }
    Ok(false)
}

/// The ramp along the closed polyline `poly` from its first point at `floor`: G1 with Z falling linearly with the
/// path length (|dz / dxy| = (floor - z) / `run`), wrapping the ring if `run` asks for it, the segment where it
/// reaches `z` split there, then the rest of that lap flat at `z` back to the first point at the cutting feed.
fn ramp_emit(p: &mut Program, poly: &[Vec2], floor: f64, z: f64, run: f64, f: &Feeds, tol: Tol) {
    let n = poly.len();
    let (mut s, mut i, mut at) = (0.0, 0usize, poly[0]);
    while s < run {
        let b = poly[(i + 1) % n];
        let l = (b - at).length();
        if s + l >= run {
            let q = at + (b - at) * ((run - s) / l);
            p.moves.push(Move::Linear { to: Vec3::new(q.x, q.y, z), feed: f.plunge });
            at = q;
            break;
        }
        s += l;
        p.moves.push(Move::Linear { to: Vec3::new(b.x, b.y, floor - (floor - z) * s / run), feed: f.plunge });
        at = b;
        i += 1;
    }
    loop {
        i += 1;
        let b = poly[i % n];
        if (b - at).length() > tol.lin {
            p.moves.push(Move::Linear { to: Vec3::new(b.x, b.y, z), feed: f.feed });
        }
        at = b;
        if i % n == 0 {
            break;
        }
    }
}

/// The deepest point of the region inside the closed polyline `poly` (length `per`) and its distance to `poly`: the
/// offset ([`crate::offset::rings`], the one offset) bisected on d in [0, per / 2π] (no inscribed circle is longer
/// than its ring) to `tol.lin`, the point a vertex of the deepest non-empty ring, so its distance is exactly that d.
fn inscribed(poly: &[Vec2], per: f64, tol: Tol) -> Result<(Vec2, f64), Refusal> {
    let mut lp = poly.to_vec();
    if crate::geom::signed_area2(&lp) < 0.0 {
        lp.reverse();
    }
    let lps = alloc::vec![lp];
    let (mut lo, mut hi) = (0.0, per / (2.0 * core::f64::consts::PI));
    let mut best = (poly[0], 0.0);
    let mut k = 0;
    while hi - lo > tol.lin && k < 64 {
        let d = 0.5 * (lo + hi);
        let rs = crate::offset::rings(&lps, d, tol)?;
        match rs.first() {
            Some(ring) => {
                lo = d;
                best = (ring[0].start(), d);
            }
            None => hi = d,
        }
        k += 1;
    }
    Ok(best)
}
