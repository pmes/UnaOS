// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, "CAD viewing, editing, and CAM/Slicing"), seam shared-core — VUG-MAKE-SLICE (LEDGER SR483): FDM slicing of a triangle soup into the `Program` the Marlin writer says.
//!
//! The pipeline, every step a value in and a value or a named [`Refusal`] out:
//!
//! - [`layers`]: layer tops at `z_min + first_layer + k * layer_h`; each layer is SECTIONED at its mid-height, so no
//!   plane sits on a flat cap of a part whose height is a multiple of the layer.
//! - [`section`]: the plane / triangle cut with NO epsilon. A vertex is "above" iff `v.z >= z` exactly (ON counts as
//!   above: the plane is `z + eps` symbolically, a simulation-of-simplicity perturbation). A triangle with mixed
//!   classes gives one segment, from its above->below edge crossing to its below->above crossing in its CCW edge
//!   order, so the solid is on the segment's left (outer loops CCW, holes CW). The crossing of an edge is computed
//!   from the edge's endpoints ordered by z, so the two triangles that share an edge produce the same bits, and the
//!   segments chain into loops by EXACT endpoint equality; a dead end refuses `OpenLoop { layer, gap }`.
//! - [`islands`]: even-odd nesting; even depth made CCW, odd CW; an island is an outer loop and its direct holes.
//! - perimeters by `offset::rings` to the LEFT of each loop (into the material for outers and holes alike), infill by
//!   [`infill`] (rectilinear, 45 / 135 degrees alternating, even-odd scanline), E by [`extrusion`].
//!
//! The caller hands over triangles: make_core does not depend on mesh_core. From a `mesh_core::Mesh` the caller
//! gathers `[p[t[0]], p[t[1]], p[t[2]]]` per triangle; an indexed mesh shares vertex bits, which is what lets the
//! exact chaining close. Timing is `plan::durations` (via `Program::validate`), the text is `gcode::write`.

use alloc::collections::BTreeMap;
use alloc::{format, vec::Vec};
use core::f64::consts::PI;
use libm::{ceil, cos, sin};

use crate::geom::{Length, Tol, Vec2, Vec3, Xy, signed_area2}; // Length / Xy: the SR473 traits (geom_core has mag; the fold composed SR483 onto the swap)
use crate::machine::{Machine, MachineKind};
use crate::program::{Heater, Move, Program};
use crate::validate::Refusal;

/// The process parameters of one FDM slice (the program's units: mm, mm/min, °C).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliceParams {
    /// The nozzle (a `Tool`, `ToolKind::Nozzle`).
    pub tool: u32,
    pub layer_h: f64,
    pub first_layer: f64,
    /// The extrusion width, also the perimeter stepover.
    pub width: f64,
    pub perimeters: u32,
    /// Sparse infill density in (0, 1]; the first `bottom_layers` and last `top_layers` layers are solid.
    pub density: f64,
    pub top_layers: u32,
    pub bottom_layers: u32,
    /// The filament diameter (the `Machine` carries none).
    pub filament: f64,
    pub print_feed: f64,
    pub first_layer_feed: f64,
    /// Retraction length on a long travel (0: none).
    pub retract: f64,
    pub retract_feed: f64,
    /// A travel longer than this (XY) retracts and hops.
    pub retract_min_travel: f64,
    pub z_hop: f64,
    pub hotend_c: f64,
    pub bed_c: f64,
}

impl SliceParams {
    /// A 0.4 mm nozzle printing PLA at 0.2 mm layers.
    pub const fn fdm(tool: u32) -> Self {
        SliceParams {
            tool,
            layer_h: 0.2,
            first_layer: 0.2,
            width: 0.4,
            perimeters: 2,
            density: 0.2,
            top_layers: 4,
            bottom_layers: 3,
            filament: 1.75,
            print_feed: 2400.0,
            first_layer_feed: 1200.0,
            retract: 0.8,
            retract_feed: 2100.0,
            retract_min_travel: 2.0,
            z_hop: 0.2,
            hotend_c: 210.0,
            bed_c: 60.0,
        }
    }

    fn check(&self, tol: Tol) -> Result<(), Refusal> {
        let bad = |what: &'static str| Err(Refusal::SliceInvalid { what });
        if !(self.layer_h > tol.lin) {
            return bad("layer height not positive");
        }
        if !(self.first_layer > tol.lin) {
            return bad("first layer height not positive");
        }
        if !(self.width > tol.lin) {
            return bad("extrusion width not positive");
        }
        if !(self.filament > tol.lin) {
            return bad("filament diameter not positive");
        }
        if !(self.density > 0.0 && self.density <= 1.0) {
            return bad("infill density outside (0, 1]");
        }
        let pos = |v: f64| v > 0.0 && v.is_finite();
        if !pos(self.print_feed) || !pos(self.first_layer_feed) || !pos(self.retract_feed) {
            return bad("feed not positive");
        }
        let nonneg = |v: f64| v >= 0.0 && v.is_finite();
        if !nonneg(self.retract) || !nonneg(self.retract_min_travel) || !nonneg(self.z_hop) {
            return bad("retract, travel or hop negative");
        }
        Ok(())
    }
}

/// An outer loop (CCW) and the holes directly inside it (CW).
#[derive(Clone, Debug, PartialEq)]
pub struct Island {
    pub outer: Vec<Vec2>,
    pub holes: Vec<Vec<Vec2>>,
}

/// One layer: its index, its top `z` (the nozzle height), its `height`, its islands, and a named degenerate when its
/// walls do not fit ("perimeter collapsed": the apex of a cone or a tetrahedron; "empty layer").
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub index: usize,
    pub z: f64,
    pub height: f64,
    pub islands: Vec<Island>,
    pub degenerate: Option<&'static str>,
}

/// The extruded filament length for a bead `len` long, `h` high and `w` wide from filament of diameter `d`.
pub fn extrusion(len: f64, h: f64, w: f64, d: f64) -> f64 {
    let r = d * 0.5;
    len * h * w / (PI * r * r)
}

type Key = (u64, u64);

/// -0.0 folded into 0.0, so equal points have equal keys.
fn norm(p: Vec2) -> Vec2 {
    Vec2::new(p.x + 0.0, p.y + 0.0)
}

fn key(p: Vec2) -> Key {
    (p.x.to_bits(), p.y.to_bits())
}

fn unkey(k: Key) -> Vec2 {
    Vec2::new(f64::from_bits(k.0), f64::from_bits(k.1))
}

/// Where the edge `ab` meets the plane: an endpoint itself when it lies on the plane, else the interpolation from the
/// lower endpoint (by z) to the upper, so both triangles of a shared edge produce the same bits.
fn crossing(a: Vec3, b: Vec3, z: f64) -> Vec2 {
    if a.z == z {
        return norm(a.xy());
    }
    if b.z == z {
        return norm(b.xy());
    }
    let (lo, hi) = if a.z < b.z { (a, b) } else { (b, a) };
    let t = (z - lo.z) / (hi.z - lo.z);
    norm(Vec2::new(lo.x + (hi.x - lo.x) * t, lo.y + (hi.y - lo.y) * t))
}

/// The closed loops of the soup's section by the plane at `z` (layer index `layer`, for the refusal).
pub fn section(tris: &[[Vec3; 3]], z: f64, layer: usize) -> Result<Vec<Vec<Vec2>>, Refusal> {
    // The segments, an exactly opposite pair cancelled (a ridge lying in the plane), counted per direction.
    let mut set: BTreeMap<(Key, Key), usize> = BTreeMap::new();
    for t in tris {
        let up = [t[0].z >= z, t[1].z >= z, t[2].z >= z];
        if up[0] == up[1] && up[1] == up[2] {
            continue;
        }
        let (mut from, mut to) = (None, None);
        for i in 0..3 {
            let j = (i + 1) % 3;
            if up[i] && !up[j] {
                from = Some(crossing(t[i], t[j], z));
            } else if !up[i] && up[j] {
                to = Some(crossing(t[i], t[j], z));
            }
        }
        let (Some(p), Some(q)) = (from, to) else { continue };
        let (kp, kq) = (key(p), key(q));
        if kp == kq {
            continue; // zero length: the triangle touches the plane at a point
        }
        let rev = (kq, kp);
        if let Some(c) = set.get_mut(&rev) {
            *c -= 1;
            if *c == 0 {
                set.remove(&rev);
            }
        } else {
            *set.entry((kp, kq)).or_insert(0) += 1;
        }
    }
    let mut segs: Vec<(Key, Key)> = Vec::new();
    for (s, c) in set {
        for _ in 0..c {
            segs.push(s);
        }
    }
    let mut by_start: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    for (i, s) in segs.iter().enumerate() {
        by_start.entry(s.0).or_default().push(i);
    }
    let mut used = alloc::vec![false; segs.len()];
    let mut loops = Vec::new();
    for s0 in 0..segs.len() {
        if used[s0] {
            continue;
        }
        used[s0] = true;
        let start = segs[s0].0;
        let mut lp = alloc::vec![unkey(start)];
        let mut end = segs[s0].1;
        while end != start {
            let next = by_start.get(&end).and_then(|v| v.iter().copied().find(|&j| !used[j]));
            match next {
                Some(j) => {
                    used[j] = true;
                    lp.push(unkey(end));
                    end = segs[j].1;
                }
                None => {
                    let e = unkey(end);
                    let mut gap = (unkey(start) - e).length();
                    for (j, s) in segs.iter().enumerate() {
                        if !used[j] {
                            gap = gap.min((unkey(s.0) - e).length());
                        }
                    }
                    return Err(Refusal::OpenLoop { layer, gap });
                }
            }
        }
        if lp.len() >= 3 {
            loops.push(lp);
        }
    }
    Ok(loops)
}

/// Even-odd: whether `p` is inside the closed polygon.
fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let n = poly.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x {
                c = !c;
            }
        }
    }
    c
}

/// The loops nested: depth = how many other loops contain a loop's first vertex; even depth made CCW, odd CW; each
/// even loop with the odd loops one level inside it.
pub fn islands(loops: Vec<Vec<Vec2>>) -> Vec<Island> {
    let n = loops.len();
    let depth: Vec<usize> =
        (0..n).map(|i| (0..n).filter(|&j| j != i && inside(&loops[j], loops[i][0])).count()).collect();
    let mut lps = loops;
    for (i, lp) in lps.iter_mut().enumerate() {
        let ccw = signed_area2(lp.as_slice()) > 0.0;
        if ccw != (depth[i] % 2 == 0) {
            lp.reverse();
        }
    }
    let mut out = Vec::new();
    for i in 0..n {
        if depth[i] % 2 != 0 {
            continue;
        }
        let holes =
            (0..n).filter(|&j| depth[j] == depth[i] + 1 && inside(&lps[i], lps[j][0])).map(|j| lps[j].clone()).collect();
        out.push(Island { outer: lps[i].clone(), holes });
    }
    out
}

/// Rectilinear infill of the region the loops bound (even-odd) at `angle` (radians): the span across the lines is cut
/// into `n = max(1, ceil(span * density / width))` equal cells and a line laid at each cell's middle, clipped by the
/// scanline, rows alternating direction. Returns the chords and the bead width `pitch * density`, so the deposited
/// cross-section is `density` x the region (exact by the midpoint rule where the chord length is linear per cell).
pub fn infill(boundary: &[Vec<Vec2>], angle: f64, density: f64, width: f64) -> (Vec<(Vec2, Vec2)>, f64) {
    let (c, s) = (cos(angle), sin(angle));
    // u along the lines, v across them.
    let pts: Vec<Vec<(f64, f64)>> =
        boundary.iter().map(|l| l.iter().map(|q| (q.x * c + q.y * s, -q.x * s + q.y * c)).collect()).collect();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for l in &pts {
        for &(_, v) in l {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    if !(hi > lo) || !(width > 0.0) {
        return (Vec::new(), 0.0);
    }
    let span = hi - lo;
    let n = ceil(span * density / width).max(1.0) as usize;
    let sp = span / n as f64;
    let mut chords = Vec::new();
    for j in 0..n {
        let v = lo + (j as f64 + 0.5) * sp;
        let mut us: Vec<f64> = Vec::new();
        for l in &pts {
            let m = l.len();
            for i in 0..m {
                let (a, b) = (l[i], l[(i + 1) % m]);
                if (a.1 <= v) != (b.1 <= v) {
                    us.push(a.0 + (v - a.1) * (b.0 - a.0) / (b.1 - a.1));
                }
            }
        }
        us.sort_by(|x, y| x.partial_cmp(y).unwrap_or(core::cmp::Ordering::Equal));
        let back = |u: f64| Vec2::new(u * c - v * s, u * s + v * c);
        let mut row: Vec<(Vec2, Vec2)> = us.chunks_exact(2).map(|w| (back(w[0]), back(w[1]))).collect();
        if j % 2 == 1 {
            row.reverse();
            for ch in row.iter_mut() {
                *ch = (ch.1, ch.0);
            }
        }
        chords.extend(row);
    }
    (chords, sp * density)
}

/// An island's walls: the perimeter loops in print order (inner to outer), the infill boundary (empty when none),
/// and whether a perimeter collapsed.
struct Walls {
    perimeters: Vec<Vec<Vec2>>,
    boundary: Vec<Vec<Vec2>>,
    collapsed: bool,
}

fn walls(isl: &Island, p: &SliceParams, tol: Tol) -> Walls {
    // VUGMAKEARCFIT (SR490): the perimeters are the one offset's rings at (k - 1/2)·width, flattened at SLICE_CHORD
    // (the slice says no arcs: Extrude moves only); an island the offset refuses, or an empty set, is collapsed.
    let collapsed_walls = Walls { perimeters: Vec::new(), boundary: Vec::new(), collapsed: true };
    let Ok(lps) = crate::offset::loops(&isl.outer, &isl.holes, tol) else { return collapsed_walls };
    let chord = p.width * SLICE_CHORD;
    let inset_all = |d: f64| -> Option<Vec<Vec<Vec2>>> {
        match crate::offset::rings(&lps, d, tol) {
            Ok(rs) if !rs.is_empty() => Some(rs.iter().map(|r| crate::offset::flatten(r, chord, tol)).collect()),
            _ => None,
        }
    };
    let mut rings: Vec<Vec<Vec<Vec2>>> = Vec::new();
    let mut collapsed = false;
    for k in 1..=p.perimeters {
        match inset_all((k as f64 - 0.5) * p.width) {
            Some(r) => rings.push(r),
            None => {
                collapsed = true;
                break;
            }
        }
    }
    let boundary = if collapsed { Vec::new() } else { inset_all(p.perimeters as f64 * p.width).unwrap_or_default() };
    let perimeters = rings.into_iter().rev().flatten().collect();
    Walls { perimeters, boundary, collapsed }
}

/// The most a flattened perimeter corner arc bulges past the arc, as a fraction of the bead width (VUGMAKEARCFIT SR490).
const SLICE_CHORD: f64 = 0.05;

/// The soup's layers with their islands and named degenerates.
pub fn layers(tris: &[[Vec3; 3]], p: &SliceParams, tol: Tol) -> Result<Vec<Layer>, Refusal> {
    p.check(tol)?;
    if tris.is_empty() {
        return Err(Refusal::SliceInvalid { what: "empty triangle soup" });
    }
    let (mut zmin, mut zmax) = (f64::INFINITY, f64::NEG_INFINITY);
    for t in tris {
        for v in t {
            if !(v.x.is_finite() && v.y.is_finite() && v.z.is_finite()) {
                return Err(Refusal::SliceInvalid { what: "non-finite vertex" });
            }
            zmin = zmin.min(v.z);
            zmax = zmax.max(v.z);
        }
    }
    let mut out = Vec::new();
    let mut k = 0usize;
    loop {
        let top = zmin + p.first_layer + (k as f64) * p.layer_h;
        if top > zmax + tol.lin {
            break;
        }
        let h = if k == 0 { p.first_layer } else { p.layer_h };
        let isl = islands(section(tris, top - h * 0.5, k)?);
        let mut degenerate = if isl.is_empty() { Some("empty layer") } else { None };
        if isl.iter().any(|i| walls(i, p, tol).collapsed) {
            degenerate = Some("perimeter collapsed");
        }
        out.push(Layer { index: k, z: top, height: h, islands: isl, degenerate });
        k += 1;
    }
    if out.is_empty() {
        return Err(Refusal::SliceInvalid { what: "part thinner than the first layer" });
    }
    Ok(out)
}

/// The move list under construction: the position, the absolute E.
struct Writer {
    moves: Vec<Move>,
    pos: Option<Vec3>,
    e: f64,
    p: SliceParams,
}

impl Writer {
    fn retract(&mut self, at: Vec3) {
        if self.p.retract > 0.0 {
            self.moves.push(Move::Extrude { to: at, e: self.e - self.p.retract, feed: self.p.retract_feed });
        }
    }

    fn prime(&mut self, at: Vec3) {
        if self.p.retract > 0.0 {
            self.moves.push(Move::Extrude { to: at, e: self.e, feed: self.p.retract_feed });
        }
    }

    fn travel(&mut self, to: Vec2, z: f64) {
        let t = Vec3::new(to.x, to.y, z);
        let hop = self.p.z_hop;
        match self.pos {
            None => {
                // After Home the position is unknown: come in above the layer, nothing to retract.
                self.moves.push(Move::Rapid { to: Vec3::new(t.x, t.y, z + hop) });
                self.moves.push(Move::Rapid { to: t });
            }
            Some(q) if q == t => return,
            Some(q) => {
                if Vec2::new(t.x - q.x, t.y - q.y).length() > self.p.retract_min_travel {
                    self.retract(q);
                    self.moves.push(Move::Rapid { to: Vec3::new(q.x, q.y, q.z.max(z) + hop) });
                    self.moves.push(Move::Rapid { to: Vec3::new(t.x, t.y, q.z.max(z) + hop) });
                    self.moves.push(Move::Rapid { to: t });
                    self.prime(t);
                } else {
                    self.moves.push(Move::Rapid { to: t });
                }
            }
        }
        self.pos = Some(t);
    }

    fn extrude(&mut self, to: Vec2, z: f64, h: f64, w: f64, feed: f64) {
        let t = Vec3::new(to.x, to.y, z);
        let q = self.pos.unwrap_or(t);
        let l = (t - q).length();
        if !(l > 0.0) {
            return;
        }
        self.e += extrusion(l, h, w, self.p.filament);
        self.moves.push(Move::Extrude { to: t, e: self.e, feed });
        self.pos = Some(t);
    }
}

/// The soup sliced into a Marlin-sayable program for the FDM machine `m`.
pub fn slice(tris: &[[Vec3; 3]], p: &SliceParams, m: &Machine, tol: Tol) -> Result<Program, Refusal> {
    if m.kind != MachineKind::Fdm3 {
        return Err(Refusal::SliceInvalid { what: "machine is not an FDM printer" });
    }
    m.tool(p.tool).ok_or(Refusal::UnknownTool { at: 0, tool: p.tool })?;
    let ls = layers(tris, p, tol)?;
    let mut w = Writer { moves: Vec::new(), pos: None, e: 0.0, p: *p };
    w.moves.push(Move::Comment(format!("VUGMAKESLICE: {} layers", ls.len())));
    if m.loaded != Some(p.tool) {
        w.moves.push(Move::ToolChange { tool: p.tool });
    }
    w.moves.push(Move::Heat { heater: Heater::Bed, celsius: p.bed_c, wait: true });
    w.moves.push(Move::Heat { heater: Heater::Hotend, celsius: p.hotend_c, wait: true });
    w.moves.push(Move::Home);
    w.moves.push(Move::SetE { e: 0.0 });
    let n = ls.len();
    for l in &ls {
        w.moves.push(Move::Comment(format!("layer {} z={:.4}", l.index, l.z)));
        let feed = if l.index == 0 { p.first_layer_feed } else { p.print_feed };
        let solid = l.index < p.bottom_layers as usize || l.index + p.top_layers as usize >= n;
        let density = if solid { 1.0 } else { p.density };
        let angle = if l.index % 2 == 0 { PI * 0.25 } else { PI * 0.75 };
        for isl in &l.islands {
            let wl = walls(isl, p, tol);
            for lp in &wl.perimeters {
                w.travel(lp[0], l.z);
                for i in 1..=lp.len() {
                    w.extrude(lp[i % lp.len()], l.z, l.height, p.width, feed);
                }
            }
            if !wl.boundary.is_empty() {
                let (chords, bead) = infill(&wl.boundary, angle, density, p.width);
                for (a, b) in chords {
                    w.travel(a, l.z);
                    w.extrude(b, l.z, l.height, bead, feed);
                }
            }
        }
    }
    if let Some(q) = w.pos {
        w.retract(q);
        w.moves.push(Move::Rapid { to: Vec3::new(q.x, q.y, q.z + p.z_hop) });
    }
    w.moves.push(Move::Heat { heater: Heater::Hotend, celsius: 0.0, wait: false });
    w.moves.push(Move::Heat { heater: Heater::Bed, celsius: 0.0, wait: false });
    let mut prog = Program::new(m.units);
    prog.moves = w.moves;
    Ok(prog)
}
