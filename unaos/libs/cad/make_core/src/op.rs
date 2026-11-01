// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The manufacturing intent attached to the design (BP-vug-3) and the toolpath generators for it.
//!
//! Generated today: `Drill` (a peck cycle as explicit moves) and `Contour` (one closed profile offset by the tool
//! radius). The rest refuse by name with their owed id.

use alloc::vec::Vec;
use libm::ceil;

use crate::geom::{Tol, Vec2, Vec3, line_line2, seg_seg2, signed_area2};
use crate::machine::{Machine, Tool};
use crate::program::{Move, Program, Spin};
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
    Pocket2_5D { outline: Vec<Vec2>, islands: Vec<Vec<Vec2>>, depth: f64, stepover: f64, tool: u32 },
    Face { area: Vec<Vec2>, depth: f64 },
    PrintLayers { slices: Vec<(f64, Vec<Vec<Vec2>>)>, tool: u32 },
    LaserCut { polylines: Vec<Vec<Vec2>>, passes: u32 },
}

/// The toolpath for `op` on `m`, or the refusal by name.
pub fn generate(op: &Op, m: &Machine, tol: Tol) -> Result<Program, Refusal> {
    match op {
        Op::Drill { points, top, depth, peck, tool, feeds } => drill(m, points, *top, *depth, *peck, *tool, feeds, tol),
        Op::Contour { profile, top, depth, side, tool, feeds } => {
            contour(m, profile, *top, *depth, *side, *tool, feeds, tol)
        }
        Op::Pocket2_5D { .. } => Err(Refusal::OpOwed { op: "Pocket2_5D", owed: "VUG-MAKE-POCKET" }),
        Op::Face { .. } => Err(Refusal::OpOwed { op: "Face", owed: "VUG-MAKE-POCKET" }),
        Op::PrintLayers { .. } => Err(Refusal::OpOwed { op: "PrintLayers", owed: "VUG-MAKE-SLICE" }),
        Op::LaserCut { .. } => Err(Refusal::OpOwed { op: "LaserCut", owed: "VUG-MAKE-LASER" }),
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
