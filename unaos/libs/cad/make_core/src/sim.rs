// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, "CAD viewing, editing, and CAM/Slicing"), seam shared-core — the cut simulated before the machine moves.
//!
//! VUGMAKESIM (LEDGER SR496): material removal on a signed-distance stock. Every CAD application engineers trust shows
//! the stock being removed before the first chip (VUG.md §4.1); [`crate::validate`]'s RapidIntoStock and
//! FixtureCollision stay the conservative pre-flight box tests, and this module reports what the swept volume of each
//! move actually does, beside them.
//!
//! # The standard
//!
//! - The stock is a DENSE lattice of signed distances ([`StockSdf`]): node `g` (integer) at `g · h` in the world, the
//!   sign scan_core's (negative inside material), each value clamped to the band ±2h. Initialised from the exact
//!   box / cylinder SDF. Storage is f32 like scan_core's `Voxel::sdf`; every distance is computed in f64 (BP-vug-1).
//!   It is the one named duplication of scan_core's storage: scan_core's `Tsdf` is written only by projective depth
//!   integration and has no writer a stock needs.
//! - A move subtracts its swept volume: `s' = max(s, -d_swept)` per node (the CSG difference on signed distance
//!   fields, Frisken et al. 2000; the SDF form of dexel milling simulation), only over the move's swept box grown by
//!   the band.
//! - The tool is a solid of revolution about the vertical axis through its tip, given by a MIRRORED profile in
//!   (ρ, z − tip): FlatEnd the rectangle |ρ| ≤ r, 0 ≤ z ≤ LF; BallEnd the 2D capsule of (0, r)–(0, LF) radius r (the
//!   ball and the shank; its top is rounded at LF); Drill the convex pentagon with the point at the tip and
//!   `hc = r / tan(SIG / 2)`. The 3D distance is the profile's exact 2D distance at ρ = the radial distance.
//! - The swept distance, per move class: horizontal — the profile at ρ = the plan distance to the path (a segment, or
//!   a planar arc in closed form): exact for a profile symmetric in ρ that only widens upward; vertical (and the
//!   placement of a move with no known start) — the profile at the lower tip with its constant-radius top stretched
//!   by |dz|; oblique — the minimum over the move parameter by golden section (the placed distance of a convex body
//!   along an affine path is convex in the parameter); a helical arc is first cut into chords at `chord`.
//! - The volume of a node is `occ(s) · h³`, `occ(s) = clamp(1/2 − s/h, 0, 1)` (the linear distance-to-fraction rule:
//!   exact for a face parallel to a lattice plane). Volumes are in the program's units cubed (mm³ for `Units::Mm`).
//!
//! The position before the first motion move (and after a `Home`) is the machine's, unknown to the program
//! ([`crate::plan`]'s convention): that move is a PLACEMENT of the tool at its end.

use alloc::{string::String, vec::Vec};
use core::f64::consts::PI;
use libm::{acos, atan2, ceil, cos, fabs, floor, hypot, sin, sqrt, tan};

use crate::geom::{Aabb3, Length, Tol, Vec2, Vec3, Xy, seg_dist2};
use crate::machine::{Fixture, Machine, Stock, StockShape, Tool, ToolKind};
use crate::plan::{arc_bbox, arc_sweep};
use crate::program::{Move, Program};

/// Every way the simulation says no.
#[derive(Clone, Debug, PartialEq)]
pub enum SimRefusal {
    /// No cell size given and no milling tool (flat end, ball end, drill) loaded or changed to.
    NoMillingTool,
    UnknownTool { at: usize, tool: u32 },
    /// A motion move with no tool in the spindle.
    NoTool { at: usize },
    /// A nozzle, a beam or a torch removes no material by its shape (VUG-MAKE-SIM-FDM, the laser owed).
    ToolNotSimulated { at: usize, tool: u32 },
    /// A move the removal model has no meaning for (`Extrude`: VUG-MAKE-SIM-FDM).
    NotSimulated { at: usize, what: &'static str },
    /// The lattice would hold more than `max` nodes (the memory bound: 4 bytes a node).
    TooLarge { cells: u64, max: u64 },
    BadParameter { what: &'static str },
}

/// The simulation's knobs. `cell` defaults to DC / 8 of the smallest milling tool the program uses; `chord` (the most
/// a chord of a helical arc departs from it) to `cell / 4`; `rapid_mm3` (a rapid removing more is a rapid cut) to half
/// a cell's volume.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimParams {
    pub cell: Option<f64>,
    pub chord: Option<f64>,
    pub max_cells: u64,
    pub mesh: bool,
    pub rapid_mm3: Option<f64>,
}

impl Default for SimParams {
    fn default() -> Self {
        SimParams { cell: None, chord: None, max_cells: 1 << 24, mesh: false, rapid_mm3: None }
    }
}

/// One motion move's numbers: the material it removed, and its radial engagement (ae / DC, the fraction of the
/// diameter across the motion that meets material before the move: the chip-load warning).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveSim {
    pub at: usize,
    pub removed_mm3: f64,
    pub engagement: f64,
}

/// A move whose swept volume reaches a fixture's keep-out box.
#[derive(Clone, Debug, PartialEq)]
pub struct Gouge {
    pub at: usize,
    pub fixture: String,
}

/// A rapid that removed material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RapidCut {
    pub at: usize,
    pub removed_mm3: f64,
}

/// What the cut did.
#[derive(Clone, Debug)]
pub struct Sim {
    pub removed_mm3: f64,
    pub per_move: Vec<MoveSim>,
    /// Every (move, fixture) contact, in move order. Contact is resolved to the cell: a graze shallower than half a
    /// cell's diagonal may pass (validate's grown-box test stays the conservative one).
    pub gouges: Vec<Gouge>,
    pub rapid_cuts: Vec<RapidCut>,
    pub max_engagement: f64,
    /// The stock's surface after the cut (scan_core's marching cubes), when asked for.
    pub mesh: Option<(Vec<Vec3>, Vec<[u32; 3]>)>,
    /// The stock after the cut.
    pub field: StockSdf,
}

impl Sim {
    /// The material left inside the closed `profile` (even-odd) between `z_lo` and `z_hi`: the number VUG-MAKE-REST
    /// reads.
    pub fn rest(&self, profile: &[Vec2], z_lo: f64, z_hi: f64) -> f64 {
        self.field.volume_where(|p| p.z >= z_lo && p.z <= z_hi && inside(p.xy(), profile))
    }

    /// The material left in the whole stock.
    pub fn remaining_mm3(&self) -> f64 {
        self.field.volume_where(|_| true)
    }
}

/// The stock as a dense lattice of band-clamped signed distances (negative inside material).
#[derive(Clone, Debug, PartialEq)]
pub struct StockSdf {
    h: f64,
    band: f64,
    g0: [i32; 3],
    n: [usize; 3],
    data: Vec<f32>,
}

impl StockSdf {
    /// The stock at node spacing `h`, its box grown by two nodes; refused above `max_cells` nodes.
    pub fn new(stock: &Stock, h: f64, max_cells: u64) -> Result<StockSdf, SimRefusal> {
        if !(h.is_finite() && h > 0.0) {
            return Err(SimRefusal::BadParameter { what: "cell not positive" });
        }
        let ok = |v: f64| v.is_finite() && v > 0.0;
        match stock.shape {
            StockShape::Box { size } if !(ok(size.x) && ok(size.y) && ok(size.z)) => {
                return Err(SimRefusal::BadParameter { what: "stock size not positive" });
            }
            StockShape::Cylinder { radius, height } if !(ok(radius) && ok(height)) => {
                return Err(SimRefusal::BadParameter { what: "stock size not positive" });
            }
            _ => {}
        }
        let b = stock.aabb();
        let lo = [floor((b.min.x - 2.0 * h) / h), floor((b.min.y - 2.0 * h) / h), floor((b.min.z - 2.0 * h) / h)];
        let hi = [ceil((b.max.x + 2.0 * h) / h), ceil((b.max.y + 2.0 * h) / h), ceil((b.max.z + 2.0 * h) / h)];
        let mut cells: u64 = 1;
        let mut n = [0usize; 3];
        for a in 0..3 {
            let span = hi[a] - lo[a] + 1.0;
            if !(span.is_finite() && span < 4.0e9) || lo[a] < i32::MIN as f64 || hi[a] > i32::MAX as f64 {
                return Err(SimRefusal::TooLarge { cells: u64::MAX, max: max_cells });
            }
            n[a] = span as usize;
            cells = cells.saturating_mul(span as u64);
        }
        if cells > max_cells {
            return Err(SimRefusal::TooLarge { cells, max: max_cells });
        }
        let band = 2.0 * h;
        let mut f = StockSdf { h, band, g0: [lo[0] as i32, lo[1] as i32, lo[2] as i32], n, data: Vec::new() };
        f.data.reserve(cells as usize);
        for k in 0..n[2] {
            for j in 0..n[1] {
                for i in 0..n[0] {
                    let s = stock_sdf(stock, f.pos(i, j, k));
                    f.data.push(s.clamp(-band, band) as f32);
                }
            }
        }
        Ok(f)
    }

    /// The node spacing.
    pub fn cell(&self) -> f64 {
        self.h
    }

    /// The lattice's node count per axis.
    pub fn dims(&self) -> [usize; 3] {
        self.n
    }

    fn pos(&self, i: usize, j: usize, k: usize) -> Vec3 {
        let h = self.h;
        Vec3::new(
            (self.g0[0] + i as i32) as f64 * h,
            (self.g0[1] + j as i32) as f64 * h,
            (self.g0[2] + k as i32) as f64 * h,
        )
    }

    fn idx(&self, i: usize, j: usize, k: usize) -> usize {
        i + self.n[0] * (j + self.n[1] * k)
    }

    /// The stored value at global node `g` (`None` outside the lattice).
    pub fn value(&self, g: (i32, i32, i32)) -> Option<f32> {
        let l = [g.0 - self.g0[0], g.1 - self.g0[1], g.2 - self.g0[2]];
        if l.iter().zip(self.n.iter()).any(|(v, n)| *v < 0 || *v as usize >= *n) {
            return None;
        }
        Some(self.data[self.idx(l[0] as usize, l[1] as usize, l[2] as usize)])
    }

    /// The trilinear sample at `p`; outside the lattice, air (the band).
    pub fn sample(&self, p: Vec3) -> f64 {
        let f = [p.x / self.h - self.g0[0] as f64, p.y / self.h - self.g0[1] as f64, p.z / self.h - self.g0[2] as f64];
        let mut i0 = [0usize; 3];
        let mut t = [0f64; 3];
        for a in 0..3 {
            let top = (self.n[a] - 1) as f64;
            if !(f[a] >= 0.0 && f[a] <= top) {
                return self.band;
            }
            let fl = floor(f[a]).min(top - 1.0);
            i0[a] = fl as usize;
            t[a] = f[a] - fl;
        }
        let v = |di: usize, dj: usize, dk: usize| self.data[self.idx(i0[0] + di, i0[1] + dj, i0[2] + dk)] as f64;
        let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
        let x00 = lerp(v(0, 0, 0), v(1, 0, 0), t[0]);
        let x10 = lerp(v(0, 1, 0), v(1, 1, 0), t[0]);
        let x01 = lerp(v(0, 0, 1), v(1, 0, 1), t[0]);
        let x11 = lerp(v(0, 1, 1), v(1, 1, 1), t[0]);
        lerp(lerp(x00, x10, t[1]), lerp(x01, x11, t[1]), t[2])
    }

    fn occ(&self, s: f64) -> f64 {
        (0.5 - s / self.h).clamp(0.0, 1.0)
    }

    fn volume_where<F: Fn(Vec3) -> bool>(&self, keep: F) -> f64 {
        let h3 = self.h * self.h * self.h;
        let mut v = 0.0;
        for k in 0..self.n[2] {
            for j in 0..self.n[1] {
                for i in 0..self.n[0] {
                    let s = self.data[self.idx(i, j, k)] as f64;
                    if s < 0.5 * self.h && keep(self.pos(i, j, k)) {
                        v += self.occ(s) * h3;
                    }
                }
            }
        }
        v
    }

    /// The node index range covering `[lo, hi]` along axis `a`, clipped to the lattice (`None` when disjoint).
    fn range(&self, a: usize, lo: f64, hi: f64) -> Option<(usize, usize)> {
        let g0 = self.g0[a] as f64;
        let i0 = (ceil(lo / self.h) - g0).max(0.0);
        let i1 = (floor(hi / self.h) - g0).min((self.n[a] - 1) as f64);
        if i0 > i1 { None } else { Some((i0 as usize, i1 as usize)) }
    }

    /// Subtract one swept piece; the volume it removed.
    fn carve(&mut self, sw: &Swept, tol: Tol) -> f64 {
        let b = sw.bounds();
        let g = self.band;
        let (Some(rx), Some(ry), Some(rz)) = (
            self.range(0, b.min.x - g, b.max.x + g),
            self.range(1, b.min.y - g, b.max.y + g),
            self.range(2, b.min.z - g, b.max.z + g),
        ) else {
            return 0.0;
        };
        let h3 = self.h * self.h * self.h;
        let mut removed = 0.0;
        for k in rz.0..=rz.1 {
            for j in ry.0..=ry.1 {
                for i in rx.0..=rx.1 {
                    let id = self.idx(i, j, k);
                    let s = self.data[id] as f64;
                    if s >= g {
                        continue;
                    }
                    let d = sw.distance(self.pos(i, j, k), tol);
                    let new = (-d).min(g);
                    if new > s {
                        let stored = new as f32;
                        removed += self.occ(s) - self.occ(stored as f64);
                        self.data[id] = stored;
                    }
                }
            }
        }
        removed * h3
    }

    /// The surface through scan_core's marching cubes (every node observed, weight 1).
    fn mesh(&self) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let (g0, n) = (self.g0, self.n);
        let origins = (0..n[2]).flat_map(move |k| {
            (0..n[1]).flat_map(move |j| {
                (0..n[0]).map(move |i| (g0[0] + i as i32, g0[1] + j as i32, g0[2] + k as i32))
            })
        });
        let s = scan_core::mc::march_cells(origins, |g| self.value(g).map(|v| (v, 1.0f32)), self.h);
        (s.positions, s.triangles)
    }
}

/// The exact signed distance of the stock (negative inside).
fn stock_sdf(stock: &Stock, p: Vec3) -> f64 {
    match stock.shape {
        StockShape::Box { size } => {
            let c = stock.origin + size * 0.5;
            let q = [fabs(p.x - c.x) - size.x * 0.5, fabs(p.y - c.y) - size.y * 0.5, fabs(p.z - c.z) - size.z * 0.5];
            let out = sqrt(q.iter().map(|v| v.max(0.0) * v.max(0.0)).sum::<f64>());
            out + q[0].max(q[1]).max(q[2]).min(0.0)
        }
        StockShape::Cylinder { radius, height } => {
            let dr = hypot(p.x - stock.origin.x, p.y - stock.origin.y) - radius;
            let dz = fabs(p.z - (stock.origin.z + height * 0.5)) - height * 0.5;
            box2(dr, dz)
        }
    }
}

/// The 2D box distance from the per-axis excesses.
fn box2(dx: f64, dz: f64) -> f64 {
    hypot(dx.max(0.0), dz.max(0.0)) + dx.max(dz).min(0.0)
}

/// The tool's mirrored profile in (ρ, z − tip).
#[derive(Clone, Copy, Debug)]
enum Profile {
    /// The rectangle (hc = 0) or the pentagon with the point at the tip (hc > 0), radius `r`, top at `top`.
    Poly { r: f64, hc: f64, top: f64 },
    /// The 2D capsule of (0, r)–(0, top), radius `r`.
    Capsule { r: f64, top: f64 },
}

impl Profile {
    fn of(t: &Tool, tol: Tol) -> Option<Profile> {
        let r = t.radius();
        match t.kind {
            ToolKind::FlatEnd => Some(Profile::Poly { r, hc: 0.0, top: t.lf.max(tol.lin) }),
            ToolKind::Drill => {
                let hc = if t.sig > tol.ang && t.sig < PI - tol.ang { r / tan(0.5 * t.sig) } else { 0.0 };
                Some(Profile::Poly { r, hc, top: t.lf.max(hc + tol.lin) })
            }
            ToolKind::BallEnd => Some(Profile::Capsule { r, top: t.lf.max(r) }),
            ToolKind::Nozzle | ToolKind::Beam | ToolKind::Torch => None,
        }
    }

    fn r(&self) -> f64 {
        match *self {
            Profile::Poly { r, .. } | Profile::Capsule { r, .. } => r,
        }
    }

    fn top(&self) -> f64 {
        match *self {
            Profile::Poly { top, .. } | Profile::Capsule { top, .. } => top,
        }
    }

    /// The same profile with its constant-radius top raised by `dz` (the vertical sweep).
    fn stretched(self, dz: f64) -> Profile {
        match self {
            Profile::Poly { r, hc, top } => Profile::Poly { r, hc, top: top + dz },
            Profile::Capsule { r, top } => Profile::Capsule { r, top: top + dz },
        }
    }

    /// The exact signed distance in the meridian plane (`z` from the tip).
    fn sdf(&self, rho: f64, z: f64) -> f64 {
        let rho = fabs(rho);
        match *self {
            Profile::Poly { r, hc, top } if hc <= 0.0 => box2(rho - r, (-z).max(z - top)),
            Profile::Poly { r, hc, top } => poly_sdf((rho, z), &[(0.0, 0.0), (r, hc), (r, top), (-r, top), (-r, hc)]),
            Profile::Capsule { r, top } => hypot(rho, z - z.clamp(r, top)) - r,
        }
    }
}

/// The exact signed distance to a closed polygon (negative inside): the nearest edge, and the sign by winding
/// crossings (the standard per-edge form).
fn poly_sdf(p: (f64, f64), v: &[(f64, f64)]) -> f64 {
    let dot = |a: (f64, f64), b: (f64, f64)| a.0 * b.0 + a.1 * b.1;
    let sub = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0, a.1 - b.1);
    let mut d = dot(sub(p, v[0]), sub(p, v[0]));
    let mut s = 1.0;
    let mut j = v.len() - 1;
    for i in 0..v.len() {
        let e = sub(v[j], v[i]);
        let w = sub(p, v[i]);
        let t = (dot(w, e) / dot(e, e)).clamp(0.0, 1.0);
        let b = (w.0 - e.0 * t, w.1 - e.1 * t);
        d = d.min(dot(b, b));
        let c = [p.1 >= v[i].1, p.1 < v[j].1, e.0 * w.1 > e.1 * w.0];
        if c.iter().all(|x| *x) || c.iter().all(|x| !*x) {
            s = -s;
        }
        j = i;
    }
    s * sqrt(d)
}

/// One swept piece of a move.
#[derive(Clone, Copy, Debug)]
enum Path {
    Line { a: Vec3, b: Vec3 },
    /// A planar arc at `z` from `s` to `e` about `c`, radius `rad`, `sweep` radians.
    Arc { s: Vec2, e: Vec2, c: Vec2, rad: f64, sweep: f64, cw: bool, z: f64 },
}

#[derive(Clone, Copy, Debug)]
struct Swept {
    path: Path,
    tool: Profile,
    tol: Tol,
}

/// Golden-section iterations for an oblique line: (0.618…)^40 ≈ 4e-9 of the parameter.
const GOLDEN_ITERS: usize = 40;

impl Swept {
    /// The box of the tool along the piece (before any band).
    fn bounds(&self) -> Aabb3 {
        let (r, top) = (self.tool.r(), self.tool.top());
        let (lo, hi) = match self.path {
            Path::Line { a, b } => (
                Vec3::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)),
                Vec3::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)),
            ),
            Path::Arc { s, e, c, cw, z, .. } => {
                let (l, h) = arc_bbox(s, e, c, cw, self.tol);
                (Vec3::new(l.x, l.y, z), Vec3::new(h.x, h.y, z))
            }
        };
        Aabb3 { min: Vec3::new(lo.x - r, lo.y - r, lo.z), max: Vec3::new(hi.x + r, hi.y + r, hi.z + top) }
    }

    /// The signed distance from `p` to the tool's swept volume along the piece.
    fn distance(&self, p: Vec3, tol: Tol) -> f64 {
        match self.path {
            Path::Line { a, b } => {
                let dxy = (b.xy() - a.xy()).length();
                let dz = b.z - a.z;
                if dxy <= tol.lin {
                    let zlo = a.z.min(b.z);
                    self.tool.stretched(fabs(dz)).sdf((p.xy() - a.xy()).length(), p.z - zlo)
                } else if fabs(dz) <= tol.lin {
                    self.tool.sdf(seg_dist2(p.xy(), a.xy(), b.xy()), p.z - a.z)
                } else {
                    let f = |t: f64| {
                        let c = a.lerp(b, t);
                        self.tool.sdf((p.xy() - c.xy()).length(), p.z - c.z)
                    };
                    let g = 0.5 * (sqrt(5.0) - 1.0);
                    let (mut lo, mut hi) = (0.0f64, 1.0f64);
                    let (mut x1, mut x2) = (hi - g * (hi - lo), lo + g * (hi - lo));
                    let (mut f1, mut f2) = (f(x1), f(x2));
                    for _ in 0..GOLDEN_ITERS {
                        if f1 <= f2 {
                            hi = x2;
                            x2 = x1;
                            f2 = f1;
                            x1 = hi - g * (hi - lo);
                            f1 = f(x1);
                        } else {
                            lo = x1;
                            x1 = x2;
                            f1 = f2;
                            x2 = lo + g * (hi - lo);
                            f2 = f(x2);
                        }
                    }
                    f1.min(f2).min(f(0.0)).min(f(1.0))
                }
            }
            Path::Arc { s, e, c, rad, sweep, cw, z } => {
                let q = p.xy();
                let ends = (q - s).length().min((q - e).length());
                let dc = (q - c).length();
                let rho = if dc <= tol.lin {
                    rad
                } else {
                    let on = c + (q - c) * (rad / dc);
                    if arc_sweep(s, on, c, cw, tol) <= sweep + tol.ang { fabs(dc - rad).min(ends) } else { ends }
                };
                self.tool.sdf(rho, p.z - z)
            }
        }
    }
}

/// Even-odd point in polygon.
fn inside(p: Vec2, ring: &[Vec2]) -> bool {
    let mut c = false;
    if ring.is_empty() {
        return false;
    }
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x {
                c = !c;
            }
        }
        j = i;
    }
    c
}

/// The pieces of one motion move from `s` (the start; equal to the end for a placement).
fn pieces(mv: &Move, s: Vec3, chord: f64, tol: Tol) -> Vec<Path> {
    let mut out = Vec::new();
    match *mv {
        Move::Rapid { to } | Move::Linear { to, .. } => out.push(Path::Line { a: s, b: to }),
        Move::Arc { to, centre, cw, .. } => {
            let rad = (s.xy() - centre).length();
            if rad <= tol.lin {
                out.push(Path::Line { a: s, b: to });
                return out;
            }
            let sweep = arc_sweep(s.xy(), to.xy(), centre, cw, tol);
            if fabs(to.z - s.z) <= tol.lin {
                out.push(Path::Arc { s: s.xy(), e: to.xy(), c: centre, rad, sweep, cw, z: s.z });
            } else {
                let step = if chord < rad { 2.0 * acos(1.0 - chord / rad) } else { 0.5 * PI };
                let n = (ceil(sweep / step) as usize).clamp(1, 1 << 16);
                let a0 = atan2(s.y - centre.y, s.x - centre.x);
                let dir = if cw { -1.0 } else { 1.0 };
                let at = |k: usize| {
                    if k == n {
                        return to;
                    }
                    let f = k as f64 / n as f64;
                    let a = a0 + dir * sweep * f;
                    Vec3::new(centre.x + rad * cos(a), centre.y + rad * sin(a), s.z + (to.z - s.z) * f)
                };
                let mut prev = s;
                for k in 1..=n {
                    let q = at(k);
                    out.push(Path::Line { a: prev, b: q });
                    prev = q;
                }
            }
        }
        _ => {}
    }
    out
}

/// The radial engagement before the move: 33 points across the diameter perpendicular to the motion at the move's
/// middle, half a cell above the tip; the fraction inside material.
fn engagement(f: &StockSdf, mv: &Move, s: Vec3, r: f64, tol: Tol) -> f64 {
    let (c, perp) = match *mv {
        Move::Rapid { to } | Move::Linear { to, .. } => {
            let d = to.xy() - s.xy();
            let l = d.length();
            if l <= tol.lin {
                return 0.0;
            }
            (s.lerp(to, 0.5), Vec2::new(-d.y / l, d.x / l))
        }
        Move::Arc { to, centre, cw, .. } => {
            let rad = (s.xy() - centre).length();
            if rad <= tol.lin {
                return 0.0;
            }
            let sweep = arc_sweep(s.xy(), to.xy(), centre, cw, tol);
            let a0 = atan2(s.y - centre.y, s.x - centre.x);
            let dir = if cw { -1.0 } else { 1.0 };
            let am = a0 + dir * 0.5 * sweep;
            let u = Vec2::new(cos(am), sin(am));
            (Vec3::new(centre.x + rad * u.x, centre.y + rad * u.y, 0.5 * (s.z + to.z)), u)
        }
        _ => return 0.0,
    };
    const N: usize = 33;
    let mut hit = 0usize;
    for i in 0..N {
        let w = -r + 2.0 * r * i as f64 / (N - 1) as f64;
        let p = Vec3::new(c.x + perp.x * w, c.y + perp.y * w, c.z + 0.5 * f.h);
        if f.sample(p) < 0.0 {
            hit += 1;
        }
    }
    hit as f64 / N as f64
}

/// Whether the swept piece reaches box `b`: the swept distance <= 0 on a lattice (spacing <= `h`) over the box's six
/// faces where they meet the piece's box grown by `h`, or the piece's start inside the box.
fn touches(sw: &Swept, b: &Aabb3, h: f64, tol: Tol) -> bool {
    let tb = sw.bounds();
    let clip = Aabb3 {
        min: Vec3::new(b.min.x.max(tb.min.x - h), b.min.y.max(tb.min.y - h), b.min.z.max(tb.min.z - h)),
        max: Vec3::new(b.max.x.min(tb.max.x + h), b.max.y.min(tb.max.y + h), b.max.z.min(tb.max.z + h)),
    };
    if clip.min.x > clip.max.x || clip.min.y > clip.max.y || clip.min.z > clip.max.z {
        return false;
    }
    let start = match sw.path {
        Path::Line { a, .. } => a,
        Path::Arc { s, z, .. } => Vec3::new(s.x, s.y, z),
    };
    if b.contains_point(start, tol) {
        return true;
    }
    let lo = [clip.min.x, clip.min.y, clip.min.z];
    let hi = [clip.max.x, clip.max.y, clip.max.z];
    let bl = [b.min.x, b.min.y, b.min.z];
    let bh = [b.max.x, b.max.y, b.max.z];
    let steps = |a: usize| (ceil((hi[a] - lo[a]) / h) as usize).clamp(1, 1 << 12);
    for ax in 0..3 {
        for v in [bl[ax], bh[ax]] {
            if v < lo[ax] || v > hi[ax] {
                continue;
            }
            let (u, w) = ((ax + 1) % 3, (ax + 2) % 3);
            let (nu, nw) = (steps(u), steps(w));
            for iu in 0..=nu {
                for iw in 0..=nw {
                    let mut c = [0.0f64; 3];
                    c[ax] = v;
                    c[u] = lo[u] + (hi[u] - lo[u]) * iu as f64 / nu as f64;
                    c[w] = lo[w] + (hi[w] - lo[w]) * iw as f64 / nw as f64;
                    if sw.distance(Vec3::new(c[0], c[1], c[2]), tol) <= 0.0 {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Simulate `p` cutting `stock` on `m` (its tool library and loaded tool), naming every contact with `fixtures`.
pub fn simulate(
    p: &Program,
    m: &Machine,
    stock: &Stock,
    fixtures: &[Fixture],
    params: SimParams,
    tol: Tol,
) -> Result<Sim, SimRefusal> {
    // The cell: DC / 8 of the smallest milling tool the program uses, unless given.
    let h = match params.cell {
        Some(c) if c.is_finite() && c > tol.lin => c,
        Some(_) => return Err(SimRefusal::BadParameter { what: "cell not above tol.lin" }),
        None => {
            let used = m.loaded.into_iter().chain(p.moves.iter().filter_map(|mv| match mv {
                Move::ToolChange { tool } => Some(*tool),
                _ => None,
            }));
            let dc = used
                .filter_map(|n| m.tool(n))
                .filter(|t| matches!(t.kind, ToolKind::FlatEnd | ToolKind::BallEnd | ToolKind::Drill))
                .map(|t| t.dc)
                .filter(|d| d.is_finite() && *d > tol.lin)
                .fold(f64::INFINITY, f64::min);
            if !dc.is_finite() {
                return Err(SimRefusal::NoMillingTool);
            }
            dc / 8.0
        }
    };
    let chord = match params.chord {
        Some(c) if c.is_finite() && c > 0.0 => c,
        Some(_) => return Err(SimRefusal::BadParameter { what: "chord not positive" }),
        None => h / 4.0,
    };
    let rapid_mm3 = params.rapid_mm3.unwrap_or(0.5 * h * h * h);
    let mut field = StockSdf::new(stock, h, params.max_cells)?;

    let mut tool: Option<&Tool> = m.loaded.and_then(|n| m.tool(n));
    let mut pos: Option<Vec3> = None;
    let mut per_move = Vec::new();
    let mut gouges: Vec<Gouge> = Vec::new();
    let mut rapid_cuts = Vec::new();
    let (mut removed_mm3, mut max_engagement) = (0.0f64, 0.0f64);

    for (i, mv) in p.moves.iter().enumerate() {
        let to = match mv {
            Move::Rapid { to } | Move::Linear { to, .. } | Move::Arc { to, .. } => *to,
            Move::ToolChange { tool: n } => {
                tool = Some(m.tool(*n).ok_or(SimRefusal::UnknownTool { at: i, tool: *n })?);
                continue;
            }
            Move::Home => {
                pos = None;
                continue;
            }
            Move::Extrude { .. } => return Err(SimRefusal::NotSimulated { at: i, what: "Extrude" }),
            _ => continue,
        };
        let t = tool.ok_or(SimRefusal::NoTool { at: i })?;
        let prof = Profile::of(t, tol).ok_or(SimRefusal::ToolNotSimulated { at: i, tool: t.number })?;
        // A move with no known start is a placement at its end.
        let (s, placed) = match pos {
            Some(s) => (s, mv.clone()),
            None => (to, Move::Linear { to, feed: 0.0 }),
        };
        let eng = engagement(&field, &placed, s, prof.r(), tol);
        let mut removed = 0.0;
        for path in pieces(&placed, s, chord, tol) {
            let sw = Swept { path, tool: prof, tol };
            for fx in fixtures {
                if gouges.iter().any(|g| g.at == i && g.fixture == fx.name) {
                    continue;
                }
                if fx.keepout.iter().any(|b| touches(&sw, b, h, tol)) {
                    gouges.push(Gouge { at: i, fixture: fx.name.clone() });
                }
            }
            removed += field.carve(&sw, tol);
        }
        if matches!(mv, Move::Rapid { .. }) && removed > rapid_mm3 {
            rapid_cuts.push(RapidCut { at: i, removed_mm3: removed });
        }
        removed_mm3 += removed;
        max_engagement = max_engagement.max(eng);
        per_move.push(MoveSim { at: i, removed_mm3: removed, engagement: eng });
        pos = Some(to);
    }

    let mesh = if params.mesh { Some(field.mesh()) } else { None };
    Ok(Sim { removed_mm3, per_move, gouges, rapid_cuts, max_engagement, mesh, field })
}
