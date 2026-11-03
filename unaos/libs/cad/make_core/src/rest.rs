// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, "CAD viewing, editing, and CAM/Slicing"), seam shared-core — the rest a pocket leaves, measured exactly and cut with a smaller tool (VUGMAKEREST, LEDGER SR517).
//!
//! # The rest
//!
//! A pocket ([`crate::op::Op::Pocket2_5D`]) runs its tool's centre along its rings (level k at distance r + k·s from
//! the boundary, [`crate::op::levels`]); the material it removes is every point within r of a ring. What it LEAVES in
//! the profile P is
//!
//! ```text
//! rest = { p in P : dist(p, every ring) > r }
//! ```
//!
//! At level 0 alone that is P less (the inward offset by r) offset outward by r: the corners tighter than r. The core a
//! stepover past r leaves inside the innermost ring is the same expression over the whole ring set, so ONE
//! construction gives both. A stepover past 2r leaves a core between every pair of rings and refuses
//! `StepoverLeavesCore` before any geometry.
//!
//! The boundary of the rest is pieces of P's edges and of every ring's offsets by r on BOTH sides
//! ([`crate::offset::offset_right`], arcs exact), run through the one offset's [`crate::offset::split`] and
//! [`crate::offset::chain`] with this module's keep rule: an offset piece bounds the rest when its midpoint is r from
//! the rings (and no nearer), the rest side of it is farther than r, and it is not lying on a wall; an edge piece of P
//! when it is farther than r from the rings. Counter-clockwise loops are the regions (their index is the chain order);
//! a clockwise loop is a hole of the smallest region containing it. Areas are exact (segments and circular segments).
//!
//! # The cut
//!
//! A rest region is NOT pocketed as a closed domain: a 90° corner left by r = 5 has an inscribed radius of 0.86, so a
//! closed-domain pocket with r2 = 1 would have no ring at all. The smaller tool works from the cleared side (classic
//! rest machining): its path is P's OWN rings at r2 + k·r2 (a step of r2 leaves no core of its own), clipped to the
//! pieces within r2 of the region. A clipped path ends exactly r2 from the region, so its plunge is in air the pocket
//! cleared; a ring kept whole starts at its first piece start whose disc meets no rest, else plunges when the tool is
//! centre-cutting, else refuses `RestNoEntry { region, tool }` (the ramp is VUG-MAKE-RAMP's). A region no ring of the
//! smaller tool comes within r2 of refuses the same. A region that touches cleared air at its mouth (a slot narrower
//! than the smaller tool) IS reached there, and what the smaller tool cannot reach in it is the rest of the rest,
//! VUG-MAKE-REST-3D's.

use alloc::{format, vec::Vec};

use crate::geom::{Tol, Vec2, Vec3};
use crate::machine::{Machine, Tool};
use crate::offset::{self, El};
use crate::op::{Feeds, centre_cutting, emit, levels, passes, prelude};
use crate::program::{Move, Program, Spin};
use crate::validate::Refusal;

/// The probe step across a boundary piece toward the side it bounds, from the caller's `Tol` (BP-vug-2).
fn probe(tol: Tol) -> f64 {
    1e3 * tol.lin
}

/// One connected region of material a pocket left.
#[derive(Clone, Debug, PartialEq)]
pub struct RestRegion {
    /// Its exact area (holes subtracted), in the program's units squared.
    pub area: f64,
    /// How many holes it has.
    pub holes: usize,
    /// The outer loop (counter-clockwise) then each hole (clockwise); the region on the left of every piece.
    pub(crate) loops: Vec<Vec<El>>,
}

impl RestRegion {
    /// Its loops as closed polylines, each arc circumscribed with at most `chord` of bulge
    /// ([`crate::offset::flatten`]).
    pub fn polyline(&self, chord: f64, tol: Tol) -> Vec<Vec<Vec2>> {
        self.loops.iter().map(|l| offset::flatten(l, chord, tol)).collect()
    }

    /// The distance from `p` to the region: 0 inside it.
    fn dist(&self, p: Vec2, tol: Tol) -> f64 {
        let flat = self.polyline(probe(tol), tol);
        if offset::inside(p, &flat[0]) && !flat[1..].iter().any(|h| offset::inside(p, h)) {
            return 0.0;
        }
        offset::dist_els(p, &self.loops, tol)
    }
}

/// The regions a pocket of `outline` and `islands` with the tool `after` at `stepover` leaves (its corners and any
/// core), in their index order. Refuses `StepoverLeavesCore` past 2r, `ToolTooLarge` when the tool has no ring, and
/// the outline's own refusals ([`crate::offset::loops`]).
pub fn regions(
    outline: &[Vec2],
    islands: &[Vec<Vec2>],
    after: &Tool,
    stepover: f64,
    tol: Tol,
) -> Result<Vec<RestRegion>, Refusal> {
    let r = after.radius();
    if !(r > tol.lin) {
        return Err(Refusal::BadParameter { what: "tool radius not positive" });
    }
    if !(stepover > tol.lin) {
        return Err(Refusal::BadParameter { what: "stepover not positive" });
    }
    if stepover > 2.0 * r + tol.lin {
        return Err(Refusal::StepoverLeavesCore { tool: after.number });
    }
    let lps = offset::loops(outline, islands, tol)?;
    let set: Vec<Vec<El>> = levels(&lps, r, stepover, after.number, tol)?.into_iter().flatten().collect();
    rest_of(&lps, &set, r, tol)
}

/// The rest of the boundary `lps` against the rings `set` swept by a disc of radius `r`.
fn rest_of(lps: &[Vec<Vec2>], set: &[Vec<El>], r: f64, tol: Tol) -> Result<Vec<RestRegion>, Refusal> {
    let walls: Vec<Vec<El>> = lps
        .iter()
        .map(|lp| (0..lp.len()).map(|i| El::Line { from: lp[i], to: lp[(i + 1) % lp.len()] }).collect::<Vec<El>>())
        .collect();
    let mut segs: Vec<El> = Vec::new();
    let mut wall: Vec<bool> = Vec::new();
    for w in &walls {
        for e in w {
            segs.push(*e);
            wall.push(true);
        }
    }
    for ring in set {
        for side in [ring.clone(), offset::reversed_ring(ring)] {
            for e in offset::offset_right(&side, r, tol) {
                // Reversed: the band is on the left of both offsets, so the rest is on the left of the reversal.
                segs.push(e.reversed());
                wall.push(false);
            }
        }
    }
    let keep = |w: bool, e: &El| -> bool {
        let m = e.mid(tol);
        let q = m + offset::left_normal(e, m) * probe(tol);
        if offset::dist_els(q, set, tol) <= r {
            return false;
        }
        let dm = offset::dist_els(m, set, tol);
        if w { dm > r + tol.lin } else { dm >= r - tol.lin && offset::dist_els(m, &walls, tol) > tol.lin }
    };
    let pieces: Vec<El> =
        offset::split(&segs, tol).into_iter().filter(|(s, e)| keep(wall[*s], e)).map(|(_, e)| e).collect();
    let mut out: Vec<RestRegion> = Vec::new();
    let mut holes: Vec<(Vec<El>, f64)> = Vec::new();
    for l in offset::chain(&pieces, tol)? {
        let (a, _) = offset::measure(&l, tol);
        if a > 0.0 {
            out.push(RestRegion { area: a, holes: 0, loops: alloc::vec![l] });
        } else {
            holes.push((l, a));
        }
    }
    let outer: Vec<f64> = out.iter().map(|g| g.area).collect();
    for (h, a) in holes {
        let p = h[0].start();
        let mut best: Option<usize> = None;
        for (i, g) in out.iter().enumerate() {
            if offset::inside(p, &offset::flatten(&g.loops[0], probe(tol), tol))
                && best.map_or(true, |b| outer[i] < outer[b])
            {
                best = Some(i);
            }
        }
        if let Some(b) = best {
            out[b].area += a;
            out[b].holes += 1;
            out[b].loops.push(h);
        }
    }
    Ok(out)
}

/// The pieces of the closed `ring` within `r2` of the region `g`, as paths: (the pieces, closed). The ring is split at
/// every crossing with the region's offset by `r2` ([`crate::offset::offset_right`]), a piece kept by its midpoint,
/// consecutive kept pieces one path (wrapping round), a ring kept whole one closed path.
fn clip(ring: &[El], g: &RestRegion, r2: f64, tol: Tol) -> Vec<(Vec<El>, bool)> {
    let n = ring.len();
    let mut segs: Vec<El> = ring.to_vec();
    for l in &g.loops {
        segs.extend(offset::offset_right(l, r2, tol));
    }
    let pieces: Vec<El> = offset::split(&segs, tol).into_iter().filter(|(s, _)| *s < n).map(|(_, e)| e).collect();
    let keep: Vec<bool> = pieces.iter().map(|e| g.dist(e.mid(tol), tol) <= r2 + tol.lin).collect();
    if keep.iter().all(|&k| k) {
        return alloc::vec![(pieces, true)];
    }
    let Some(k0) = keep.iter().position(|&k| !k) else { return Vec::new() };
    let m = pieces.len();
    let mut out = Vec::new();
    let mut cur: Vec<El> = Vec::new();
    for t in 1..=m {
        let i = (k0 + t) % m;
        if keep[i] {
            cur.push(pieces[i]);
        } else if !cur.is_empty() {
            out.push((core::mem::take(&mut cur), false));
        }
    }
    out
}

/// The rest of a `Pocket2_5D` with tool `after` at `stepover`, cut with `tool` (see the module doc): one
/// `Move::Comment` naming each region and its area before its paths, every depth pass (APMX) inside-out, each path a
/// rapid at the safe Z, a plunge in air, its pieces (G1 / G2 / G3) and a retract.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rest(
    m: &Machine,
    outline: &[Vec2],
    islands: &[Vec<Vec2>],
    top: f64,
    depth: f64,
    after: u32,
    stepover: f64,
    climb: bool,
    tool: u32,
    f: &Feeds,
    tol: Tol,
) -> Result<Program, Refusal> {
    if !(depth > tol.lin) {
        return Err(Refusal::DepthAboveTop);
    }
    let t1 = *m.tool(after).ok_or(Refusal::UnknownTool { at: 0, tool: after })?;
    let t2 = *m.tool(tool).ok_or(Refusal::UnknownTool { at: 0, tool })?;
    let r2 = t2.radius();
    if !(r2 > tol.lin && r2 < t1.radius() - tol.lin) {
        return Err(Refusal::BadParameter { what: "rest tool not smaller than the first" });
    }
    let regs = regions(outline, islands, &t1, stepover, tol)?;
    let lps = offset::loops(outline, islands, tol)?;
    let lv2 = if regs.is_empty() { Vec::new() } else { levels(&lps, r2, r2, tool, tol)? };
    // Per region, its paths: P's r2 rings from the deepest level out, each clipped to within r2 of the region.
    let mut plan: Vec<Vec<(Vec<El>, bool)>> = Vec::with_capacity(regs.len());
    for (i, g) in regs.iter().enumerate() {
        let mut paths = Vec::new();
        for lv in lv2.iter().rev() {
            for ring in lv {
                paths.extend(clip(ring, g, r2, tol));
            }
        }
        if paths.is_empty() {
            return Err(Refusal::RestNoEntry { region: i, tool });
        }
        plan.push(paths);
    }
    // A start whose disc meets no rest region.
    let clear = |q: Vec2| regs.iter().all(|g| g.dist(q, tol) >= r2 - tol.lin);
    let (mut p, _) = prelude(m, tool, f.rpm)?;
    let n = passes(&t2, depth, tol);
    for j in 1..=n {
        let z = top - depth * (j as f64) / (n as f64);
        for (i, paths) in plan.iter().enumerate() {
            p.moves.push(Move::Comment(format!("rest region {} area={:.4}", i, regs[i].area)));
            for (path, closed) in paths {
                let mut path = if climb { offset::reversed_ring(path) } else { path.clone() };
                if *closed {
                    let first = (0..path.len()).find(|&k| clear(path[k].start()));
                    match first {
                        Some(k) => path.rotate_left(k),
                        None if centre_cutting(&t2) => {}
                        None => return Err(Refusal::RestNoEntry { region: i, tool }),
                    }
                } else if !clear(path[0].start()) && !centre_cutting(&t2) {
                    return Err(Refusal::RestNoEntry { region: i, tool });
                }
                let s = path[0].start();
                p.moves.push(Move::Rapid { to: Vec3::new(s.x, s.y, m.safe_z) });
                p.moves.push(Move::Linear { to: Vec3::new(s.x, s.y, z), feed: f.plunge });
                emit(&mut p, &path, z, f.feed);
                let e = path[path.len() - 1].end();
                p.moves.push(Move::Rapid { to: Vec3::new(e.x, e.y, m.safe_z) });
            }
        }
    }
    p.moves.push(Move::Spindle(Spin::Off));
    Ok(p)
}
