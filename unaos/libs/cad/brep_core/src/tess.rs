//! Tessellation into ONE mesh_core `Mesh`, with a stated chord error and the face Id of every triangle.
//!
//! THE SAMPLING RULE. One angular step Δ serves the whole Brep: the smallest of 2·acos(1 − (chord/2)/r) over every
//! curved radius (circle and ellipse edges at their largest radius of curvature; a cylinder's r; a cone's largest
//! |r| on its face; a sphere's r; a torus's major + minor around the axis and its minor across), capped at π/2.
//! - Every arc of sweep σ is cut into ceil(σ/Δ) equal pieces, so a revolve's u-grid and an extrude's arc sides fall
//!   exactly on their boundary edges' samples. A line is one piece.
//! - Each edge is sampled ONCE, and every face that uses it reads the same nodes, so the mesh is watertight.
//! - A curved face is its uv grid: boundary nodes snapped to the shared edge samples, poles and apexes collapsing to
//!   their vertex, index-degenerate triangles dropped.
//! - A planar face is the ear-clipped polygon of its loops' samples (zero chord: it IS the plane).
//!
//! The per-direction sagitta is at most chord/2. THE WITNESS: each curved triangle's centroid and edge midpoints are
//! measured against the exact `Surface::distance`. A miss halves Δ and redoes the pass, at most six times, then
//! refuses `ChordUnreachable`. This is a stated rule with a measured witness, not a proof.

use crate::Refusal;
use crate::curve::Curve;
use crate::dep::{Mesh, Tol, Vec2, Vec3};
use crate::earcut::{EarFail, earcut};
use crate::surface::Surface;
use crate::topo::{Brep, Id};
use alloc::vec;
use alloc::vec::Vec;
use core::f64::consts::FRAC_PI_2;

/// The mesh, the face Id of each triangle (`face_of[t]` for `mesh.triangles[t]`), and the chord it was made to.
#[derive(Clone, Debug, PartialEq)]
pub struct Tessellation {
    pub mesh: Mesh,
    pub face_of: Vec<Id>,
    pub chord: f64,
}

fn step_for(r: f64, chord: f64) -> f64 {
    let h = chord * 0.5;
    if h >= r { FRAC_PI_2 } else { (2.0 * libm::acos(1.0 - h / r)).min(FRAC_PI_2) }
}

fn base_step(b: &Brep, chord: f64) -> Result<f64, Refusal> {
    let mut d = FRAC_PI_2;
    for e in &b.edges {
        if let Curve::Nurbs(_) = e.curve {
            return Err(Refusal::Unsupported("NURBS edge tessellation"));
        }
        if let Some(r) = e.curve.max_radius() {
            d = d.min(step_for(r, chord));
        }
    }
    for f in &b.faces {
        match f.surface {
            Surface::Plane { .. } => {}
            Surface::Cylinder { r, .. } => d = d.min(step_for(r, chord)),
            Surface::Cone { r0, k, .. } => {
                let uv = f.uv.ok_or(Refusal::Unsupported("trimmed curved face"))?;
                let r = libm::fabs(r0 + k * uv.v0).max(libm::fabs(r0 + k * uv.v1));
                d = d.min(step_for(r, chord));
            }
            Surface::Sphere { r, .. } => d = d.min(step_for(r, chord)),
            Surface::Torus { major, minor, .. } => {
                d = d.min(step_for(major + minor, chord)).min(step_for(minor, chord));
            }
        }
    }
    Ok(d)
}

fn pieces(span: f64, step: f64) -> Result<usize, Refusal> {
    let n = libm::ceil(libm::fabs(span) / step).max(1.0);
    if n > (1u32 << 20) as f64 {
        return Err(Refusal::BadParam("chord too fine"));
    }
    Ok(n as usize)
}

enum PassErr {
    Refuse(Refusal),
    Retry(Id),
}

impl From<Refusal> for PassErr {
    fn from(r: Refusal) -> Self {
        PassErr::Refuse(r)
    }
}

fn coedge_nodes(b: &Brep, es: &[Vec<u32>], ce: u32) -> Vec<u32> {
    let c = b.coedges[ce as usize];
    let s = &es[c.edge as usize];
    if c.reversed { s.iter().rev().copied().collect() } else { s.clone() }
}

fn pass(b: &Brep, step: f64, chord: f64, tol: Tol) -> Result<Tessellation, PassErr> {
    let mut pos: Vec<Vec3> = b.vertices.iter().map(|v| v.p).collect();
    let mut es: Vec<Vec<u32>> = Vec::with_capacity(b.edges.len());
    for e in &b.edges {
        let n = match e.curve {
            Curve::Line { .. } => 1,
            _ => pieces(e.t1 - e.t0, step)?,
        };
        let mut s = vec![e.v[0]];
        for k in 1..n {
            pos.push(e.curve.point(e.t0 + (e.t1 - e.t0) * k as f64 / n as f64));
            s.push((pos.len() - 1) as u32);
        }
        s.push(e.v[1]);
        es.push(s);
    }

    let mut tris: Vec<[u32; 3]> = Vec::new();
    let mut face_of: Vec<Id> = Vec::new();
    for (fi, f) in b.faces.iter().enumerate() {
        let fi = fi as u32;
        match (f.surface, f.uv) {
            (Surface::Plane { f: pf }, None) => {
                let sgn = if f.reversed { -1.0 } else { 1.0 };
                let (o, x, y) = (pf.o, pf.x, pf.y * sgn);
                let mut rings: Vec<Vec<(u32, Vec2)>> = Vec::new();
                for lp in b.face_loops(fi) {
                    let mut ring = Vec::new();
                    for &ce in &b.loops[lp as usize].coedges {
                        let s = coedge_nodes(b, &es, ce);
                        for &node in &s[..s.len() - 1] {
                            let d = pos[node as usize] - o;
                            ring.push((node, Vec2::new(d.dot(x), d.dot(y))));
                        }
                    }
                    rings.push(ring);
                }
                let got = earcut(&rings[0], &rings[1..]).map_err(|e| match e {
                    EarFail::Undecided => Refusal::Undecidable,
                    EarFail::Stuck => Refusal::Triangulation { face: f.id },
                })?;
                for t in got {
                    tris.push(t);
                    face_of.push(f.id);
                }
            }
            (s, Some(uv)) => {
                let nu = pieces(uv.u1 - uv.u0, step)?;
                let nv = match s {
                    Surface::Cylinder { .. } | Surface::Cone { .. } => 1,
                    _ => pieces(uv.v1 - uv.v0, step)?,
                };
                let mut bn: Vec<u32> = Vec::new();
                for lp in b.face_loops(fi) {
                    for &ce in &b.loops[lp as usize].coedges {
                        bn.extend(coedge_nodes(b, &es, ce));
                    }
                }
                let mut idx = vec![0u32; (nu + 1) * (nv + 1)];
                for j in 0..=nv {
                    for i in 0..=nu {
                        let u = uv.u0 + (uv.u1 - uv.u0) * i as f64 / nu as f64;
                        let v = uv.v0 + (uv.v1 - uv.v0) * j as f64 / nv as f64;
                        let p = s.point(u, v);
                        let slot = j * (nu + 1) + i;
                        if i == 0 || i == nu || j == 0 || j == nv {
                            let mut best = (f64::INFINITY, 0u32);
                            for &node in &bn {
                                let dd = pos[node as usize].dist(p);
                                if dd < best.0 {
                                    best = (dd, node);
                                }
                            }
                            let snap = tol.lin.max(1e-9 * (1.0 + p.mag()));
                            if best.0 > snap {
                                return Err(PassErr::Refuse(Refusal::Triangulation { face: f.id }));
                            }
                            idx[slot] = best.1;
                        } else {
                            pos.push(p);
                            idx[slot] = (pos.len() - 1) as u32;
                        }
                    }
                }
                for j in 0..nv {
                    for i in 0..nu {
                        let at = |ii: usize, jj: usize| idx[jj * (nu + 1) + ii];
                        let (n00, n10, n11, n01) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                        for t in [[n00, n10, n11], [n00, n11, n01]] {
                            let t = if f.reversed { [t[0], t[2], t[1]] } else { t };
                            if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
                                continue;
                            }
                            let (a, bb, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
                            let probes = [(a + bb + c) / 3.0, (a + bb) * 0.5, (bb + c) * 0.5, (c + a) * 0.5];
                            if probes.iter().any(|q| s.distance(*q) > chord) {
                                return Err(PassErr::Retry(f.id));
                            }
                            tris.push(t);
                            face_of.push(f.id);
                        }
                    }
                }
            }
            _ => return Err(PassErr::Refuse(Refusal::Unsupported("trimmed curved face"))),
        }
    }
    Ok(Tessellation { mesh: Mesh { positions: pos, triangles: tris }, face_of, chord })
}

/// Tessellate every face to within `chord` mm of its exact surface (the rule and witness in the module doc).
pub fn tessellate(b: &Brep, chord: f64, tol: Tol) -> Result<Tessellation, Refusal> {
    if !(chord > tol.lin) || !chord.is_finite() {
        return Err(Refusal::BadParam("chord"));
    }
    let mut step = base_step(b, chord)?;
    let mut last = None;
    for _ in 0..7 {
        match pass(b, step, chord, tol) {
            Ok(t) => return Ok(t),
            Err(PassErr::Refuse(r)) => return Err(r),
            Err(PassErr::Retry(id)) => {
                last = Some(id);
                step *= 0.5;
            }
        }
    }
    Err(Refusal::ChordUnreachable { face: last.unwrap_or(Id(0)) })
}
