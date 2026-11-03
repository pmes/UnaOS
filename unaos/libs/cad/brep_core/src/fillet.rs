//! CHARTER: Vug (CODEX §2, 3D) — shared-core: brep_core fillets and chamfers (VUG-BREP-3, LEDGER SR502): a
//! constant-radius rolling-ball fillet or a two-distance chamfer of straight edges between two planar faces, convex or
//! concave, every other case refused by name.
//!
//! The road, in order (each step's refusal is the answer; nothing is nudged):
//! 1. The input validates; r (or d1, d2) is positive and finite; the list is non-empty, without repeats, of edge Ids.
//! 2. A vertex at the end of two or more listed edges refuses `CornerNotBlended { vertex }`: the vertex blend (and a
//!    chamfer's planar mitre) is VUG-BREP-3-CORNER, owed. Listed edges are therefore vertex-disjoint and are all
//!    planned against the INPUT before anything is built.
//! 3. Per edge e (v0 -> v1 along its line direction t): F1 is the face whose co-edge runs along t, F2 the other; both
//!    must be planes (`NotClosedForm` by pair name: the plane / cylinder edge is the torus blend, VUG-BREP-3-TORUS).
//!    The in-face direction w_i = n_i x d_i (the face lies left of its loop about its outward normal n_i); the edge
//!    is convex when n1 . w2 < 0, concave when > 0. phi = acos(w1 . w2). A fillet of radius r sets each face back by
//!    r / tan(phi / 2) and its ball centre sits on the bisector at r / sin(phi / 2) (the cylinder blend: its axis
//!    parallel to the edge); a chamfer sets F1 back by d1 and F2 by d2 and joins the trim lines with a plane.
//! 4. Per end vertex: exactly three edges (more is VUG-BREP-3-DEGREE, owed); the end face E (the third face) a plane;
//!    for a fillet E perpendicular to the edge (an oblique end is an ellipse on a trimmed curved face: VUG-BREP-SSI);
//!    the neighbours a (in F1) and b (in F2) lines. The trim points lie on a and b.
//! 5. `RadiusTooLarge { edge, max }`, `max` in the operation's own parameter (r; for a chamfer d1 at the given d2/d1),
//!    the smallest closed-form bound, refused when the parameter is within Tol.lin of it or past it: (i) each
//!    neighbour edge's trims, summed over every listed edge, stay inside its length; (ii) each changed region (the
//!    strip v0 v1 Q P in F1 / F2, the corner triangle V P1 P2 in E, which the arc stays inside) holds no other vertex
//!    of that face and no point of a circle edge of it. A line edge enters a region only through a vertex (it cannot
//!    cross a, b or e), so vertices and circle extremes are the whole test. (iii) Two listed edges' regions on one
//!    face that meet refuse `Contact { what: "fillet regions meet", a, b }`.
//! 6. Topology on a CLONE of the input, through the arena's own mint (so every untouched vertex, edge and face keeps
//!    its Id and Name, and the counter stays above every Id): the trim vertices, the two trim lines, the two end edges
//!    (arcs for a fillet, in the blend cylinder's own frame and u range so tess.rs's uv grid snaps onto them), the
//!    blend face; the neighbours shortened in place; e and its two vertices dropped and the arena rebuilt compacted
//!    (tess.rs and mass.rs walk every face); `validate` judges the result.
//!
//! Names: `OpKind::Fillet` / `OpKind::Chamfer` with `a` = the source edge's Id: the blend face `Role::Blend` b 0, the
//! end edges `Role::Blend` b 1 / 2, the trim lines `Role::Trim` b 1 (on F1) / 2 (on F2), the trim vertices
//! `Role::Trim` b 0 (P1) 1 (P2) 2 (Q1) 3 (Q2). The blend is found by `faces_named`.

use crate::Refusal;
use crate::build::Seg2;
use crate::classify::{Where, area2, plane_frame, to2, where2};
use crate::curve::{Curve, Frame};
use crate::surface::{Surface, SurfaceKind};
use crate::topo::{Brep, CoEdge, Entity, Id, Loop, Name, OpKind, Role, UvBox};
use crate::validate::validate;
use alloc::vec;
use alloc::vec::Vec;
use core::f64::consts::TAU;
use geom_core::{SegSeg2, Segment2, Tol, Vec2, Vec3, segment_segment2};

/// Fillet each listed edge with radius `r` (the cylinder blend of a plane / plane edge).
pub fn fillet(b: &Brep, edges: &[Id], r: f64, tol: Tol) -> Result<Brep, Refusal> {
    if !(r > 0.0) || !r.is_finite() {
        return Err(Refusal::BadParam("fillet radius"));
    }
    edit(b, edges, Kind::Fillet { r }, tol)
}

/// Chamfer each listed edge: `d1` back along the face whose co-edge runs along the edge's own direction, `d2` along
/// the other face; the two trim lines joined by a plane.
pub fn chamfer(b: &Brep, edges: &[Id], d1: f64, d2: f64, tol: Tol) -> Result<Brep, Refusal> {
    if !(d1 > 0.0) || !d1.is_finite() || !(d2 > 0.0) || !d2.is_finite() {
        return Err(Refusal::BadParam("chamfer distance"));
    }
    edit(b, edges, Kind::Chamfer { d1, d2 }, tol)
}

#[derive(Clone, Copy)]
enum Kind {
    Fillet { r: f64 },
    Chamfer { d1: f64, d2: f64 },
}

/// One end of a listed edge.
struct End {
    /// The vertex.
    v: u32,
    /// The end face E.
    face: u32,
    /// The neighbour edge on the F1 side, on the F2 side.
    adj: [u32; 2],
    /// Unit directions from the vertex along each neighbour.
    dir: [Vec3; 2],
    /// Distance along each neighbour per unit of the parameter.
    k: [f64; 2],
}

/// One listed edge, measured against the input.
struct Plan {
    e: u32,
    id: Id,
    /// F1, F2.
    f: [u32; 2],
    /// The in-face directions w1, w2.
    w: [Vec3; 2],
    t: Vec3,
    len: f64,
    convex: bool,
    phi: f64,
    /// Setback per unit of the parameter in F1, F2.
    kappa: [f64; 2],
    ends: [End; 2],
}

impl Plan {
    /// The trim point at end `j` on side `i` for parameter `rho`.
    fn point(&self, b: &Brep, j: usize, i: usize, rho: f64) -> Vec3 {
        let end = &self.ends[j];
        b.vertices[end.v as usize].p + end.dir[i] * (end.k[i] * rho)
    }
}

fn kind_of(b: &Brep, f: u32) -> SurfaceKind {
    b.faces[f as usize].surface.kind()
}

fn wrap(a: f64) -> f64 {
    let r = libm::fmod(a, TAU);
    if r < 0.0 { r + TAU } else { r }
}

fn edit(b: &Brep, ids: &[Id], kind: Kind, tol: Tol) -> Result<Brep, Refusal> {
    validate(b, tol)?;
    if ids.is_empty() {
        return Err(Refusal::BadParam("no edges"));
    }
    let mut es: Vec<u32> = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        if ids[..i].contains(id) {
            return Err(Refusal::BadParam("an edge listed twice"));
        }
        match b.find(*id) {
            Some(Entity::Edge(e)) => es.push(e),
            _ => return Err(Refusal::BadRef { what: "fillet edge" }),
        }
    }
    let mut seen = vec![false; b.vertices.len()];
    for &e in &es {
        // A closed edge (a cap circle) has one vertex at both ends: it is counted once.
        let ev = b.edges[e as usize].v;
        let ends: &[u32] = if ev[0] == ev[1] { &ev[..1] } else { &ev };
        for &v in ends {
            if seen[v as usize] {
                return Err(Refusal::CornerNotBlended { vertex: b.vertices[v as usize].id });
            }
            seen[v as usize] = true;
        }
    }

    // Each edge's (face, co-edge) uses and each vertex's edge count.
    let mut uses: Vec<Vec<(u32, u32)>> = vec![Vec::new(); b.edges.len()];
    for (ci, c) in b.coedges.iter().enumerate() {
        uses[c.edge as usize].push((b.loops[c.loop_ as usize].face, ci as u32));
    }
    let mut degree = vec![0u32; b.vertices.len()];
    for e in &b.edges {
        degree[e.v[0] as usize] += 1;
        if e.v[1] != e.v[0] {
            degree[e.v[1] as usize] += 1;
        }
    }

    let mut plans = Vec::new();
    for &e in &es {
        plans.push(plan(b, &uses, &degree, e, kind, tol)?);
    }
    let rho = match kind {
        Kind::Fillet { r } => r,
        Kind::Chamfer { d1, .. } => d1,
    };
    size(b, &plans, rho, tol)?;
    meet(b, &plans, rho, tol)?;
    build(b, &plans, kind, rho, tol)
}

/// Measure one listed edge (steps 3 and 4 of the module doc).
fn plan(b: &Brep, uses: &[Vec<(u32, u32)>], degree: &[u32], e: u32, kind: Kind, tol: Tol) -> Result<Plan, Refusal> {
    let edge = &b.edges[e as usize];
    let u = &uses[e as usize];
    if u.len() != 2 {
        return Err(Refusal::CoEdgeCount { edge: edge.id, uses: u.len() as u32 });
    }
    let fwd = if !b.coedges[u[0].1 as usize].reversed { 0 } else { 1 };
    let (c1, c2) = (u[fwd], u[1 - fwd]);
    let f = [c1.0, c2.0];
    let (k1, k2) = (kind_of(b, f[0]), kind_of(b, f[1]));
    if k1 != SurfaceKind::Plane || k2 != SurfaceKind::Plane {
        return Err(Refusal::NotClosedForm { a: k1, b: k2 });
    }
    if f[0] == f[1] {
        return Err(Refusal::Unsupported("fillet of a seam edge"));
    }
    let Curve::Line { dir: t, .. } = edge.curve else {
        return Err(Refusal::Unsupported("fillet of a curved edge between planes"));
    };
    let len = edge.t1 - edge.t0;
    let n1 = plane_frame(&b.faces[f[0] as usize]).ok_or(Refusal::BadRef { what: "fillet face" })?.z;
    let n2 = plane_frame(&b.faces[f[1] as usize]).ok_or(Refusal::BadRef { what: "fillet face" })?.z;
    let w = [n1.cross(t), n2.cross(-t)];
    let fold = n1.dot(w[1]);
    if libm::fabs(fold) <= tol.ang {
        return Err(Refusal::Unsupported("fillet of a smooth edge"));
    }
    let convex = fold < 0.0;
    let phi = libm::acos(w[0].dot(w[1]).clamp(-1.0, 1.0));
    let kappa = match kind {
        Kind::Fillet { .. } => {
            let s = 1.0 / libm::tan(phi * 0.5);
            [s, s]
        }
        Kind::Chamfer { d1, d2 } => [1.0, d2 / d1],
    };
    let ce = [c1.1, c2.1];
    let mut ends: Vec<End> = Vec::new();
    for j in 0..2 {
        let v = edge.v[j];
        if degree[v as usize] != 3 {
            return Err(Refusal::Unsupported("fillet end vertex where more than three faces meet"));
        }
        let mut adj = [0u32; 2];
        let mut dir = [Vec3::ZERO; 2];
        let mut k = [0.0f64; 2];
        for i in 0..2 {
            let c = ce[i];
            let lp = &b.loops[b.coedges[c as usize].loop_ as usize].coedges;
            let n = lp.len();
            let pos = lp.iter().position(|&x| x == c).ok_or(Refusal::BadRef { what: "fillet loop" })?;
            let (s, _) = b.coedge_ends(c);
            let nb = if s == v { lp[(pos + n - 1) % n] } else { lp[(pos + 1) % n] };
            let a = b.coedges[nb as usize].edge;
            let ae = &b.edges[a as usize];
            if !matches!(ae.curve, Curve::Line { .. }) {
                return Err(Refusal::Unsupported("fillet end edge not a line"));
            }
            let far = if ae.v[0] == v { ae.v[1] } else { ae.v[0] };
            let d = (b.vertices[far as usize].p - b.vertices[v as usize].p).normalize()?;
            let along = d.dot(w[i]);
            if along <= tol.ang {
                return Err(Refusal::Unsupported("fillet end edge along the edge"));
            }
            adj[i] = a;
            dir[i] = d;
            k[i] = kappa[i] / along;
        }
        // The end face: the neighbours' other face, the same one on both sides.
        let other = |a: u32, not: u32| uses[a as usize].iter().map(|x| x.0).find(|&x| x != not);
        let face = match (other(adj[0], f[0]), other(adj[1], f[1])) {
            (Some(x), Some(y)) if x == y && x != f[0] && x != f[1] => x,
            _ => return Err(Refusal::Unsupported("fillet end vertex where more than three faces meet")),
        };
        let ek = kind_of(b, face);
        if ek != SurfaceKind::Plane {
            let blend = match kind {
                Kind::Fillet { .. } => SurfaceKind::Cylinder,
                Kind::Chamfer { .. } => SurfaceKind::Plane,
            };
            return Err(Refusal::NotClosedForm { a: ek, b: blend });
        }
        if let Kind::Fillet { .. } = kind {
            let ne = plane_frame(&b.faces[face as usize]).ok_or(Refusal::BadRef { what: "fillet face" })?.z;
            if ne.cross(t).mag() > tol.ang {
                return Err(Refusal::Unsupported("fillet end face oblique to the edge"));
            }
        }
        ends.push(End { v, face, adj, dir, k });
    }
    let e1 = ends.pop().ok_or(Refusal::BadRef { what: "fillet end" })?;
    let e0 = ends.pop().ok_or(Refusal::BadRef { what: "fillet end" })?;
    Ok(Plan { e, id: edge.id, f, w, t, len, convex, phi, kappa, ends: [e0, e1] })
}

/// The points of a face's edges other than `skip` that may enter a region: every edge end, and for a circle edge the
/// point of its arc furthest along `toward` (when that angle is inside the arc).
fn foreign(b: &Brep, face: u32, skip: &[u32], toward: Vec3, tol: Tol) -> Result<Vec<Vec3>, Refusal> {
    let mut out = Vec::new();
    for lp in b.face_loops(face) {
        for &ce in &b.loops[lp as usize].coedges {
            let ei = b.coedges[ce as usize].edge;
            if skip.contains(&ei) {
                continue;
            }
            let e = &b.edges[ei as usize];
            for v in e.v {
                out.push(b.vertices[v as usize].p);
            }
            match &e.curve {
                Curve::Line { .. } => {}
                Curve::Circle { f, r } => {
                    let d = toward - f.z * toward.dot(f.z);
                    if d.mag() <= tol.ang {
                        continue;
                    }
                    let th = libm::atan2(d.dot(f.y), d.dot(f.x));
                    if wrap(th - e.t0) <= e.t1 - e.t0 {
                        out.push(f.o + d.normalize()? * *r);
                    }
                }
                _ => return Err(Refusal::Unsupported("fillet beside an ellipse or NURBS edge")),
            }
        }
    }
    Ok(out)
}

fn cross2(a: Vec2, b: Vec2) -> f64 {
    a.x * b.y - a.y * b.x
}

/// RadiusTooLarge (step 5 (i) and (ii)): each plan's smallest bound, in list order.
fn size(b: &Brep, plans: &[Plan], rho: f64, tol: Tol) -> Result<(), Refusal> {
    let mut cons = vec![0.0f64; b.edges.len()];
    for p in plans {
        for end in &p.ends {
            for i in 0..2 {
                cons[end.adj[i] as usize] += end.k[i];
            }
        }
    }
    for p in plans {
        let mut bound = f64::INFINITY;
        // (i) the neighbours' lengths.
        for end in &p.ends {
            for i in 0..2 {
                let a = &b.edges[end.adj[i] as usize];
                bound = bound.min((a.t1 - a.t0) / cons[end.adj[i] as usize]);
            }
        }
        let v0 = b.vertices[p.ends[0].v as usize].p;
        // (ii) the strips in F1 and F2: (sigma, h) = ((q - v0) . t, (q - v0) . w_i).
        for i in 0..2 {
            let skip = [p.e, p.ends[0].adj[i], p.ends[1].adj[i]];
            let wi = p.w[i];
            let a2 = Vec2::new(p.ends[0].dir[i].dot(p.t), p.ends[0].dir[i].dot(wi));
            let c2 = Vec2::new(p.ends[1].dir[i].dot(p.t), p.ends[1].dir[i].dot(wi));
            let l2 = Vec2::new(p.len, 0.0);
            let sa = if cross2(a2, l2) >= 0.0 { 1.0 } else { -1.0 };
            let sc = if cross2(c2, -l2) >= 0.0 { 1.0 } else { -1.0 };
            for q in foreign(b, p.f[i], &skip, -wi, tol)? {
                let d = q - v0;
                let q2 = Vec2::new(d.dot(p.t), d.dot(wi));
                let between = cross2(a2, q2) * sa >= -tol.lin && cross2(c2, q2 - l2) * sc >= -tol.lin;
                if !between || q2.y < -tol.lin {
                    continue;
                }
                bound = bound.min(q2.y.max(0.0) / p.kappa[i]);
            }
        }
        // (ii) the corner triangles in E: q - V = alpha A + beta B with A, B the trim vectors per unit parameter.
        for end in &p.ends {
            let vp = b.vertices[end.v as usize].p;
            let (aa, bb) = (end.dir[0] * end.k[0], end.dir[1] * end.k[1]);
            let ne = plane_frame(&b.faces[end.face as usize]).ok_or(Refusal::BadRef { what: "fillet face" })?.z;
            let mut m = ne.cross(bb - aa).normalize()?;
            if m.dot(aa) < 0.0 {
                m = -m;
            }
            let (a11, a12, a22) = (aa.dot(aa), aa.dot(bb), bb.dot(bb));
            let det = a11 * a22 - a12 * a12;
            if !(libm::fabs(det) > 0.0) {
                return Err(Refusal::Unsupported("fillet end edges parallel"));
            }
            let (ea, eb) = (tol.lin / libm::sqrt(a11), tol.lin / libm::sqrt(a22));
            for q in foreign(b, end.face, &end.adj, -m, tol)? {
                let d = q - vp;
                let (b1, b2) = (d.dot(aa), d.dot(bb));
                let al = (b1 * a22 - b2 * a12) / det;
                let be = (a11 * b2 - a12 * b1) / det;
                if al < -ea || be < -eb {
                    continue;
                }
                bound = bound.min((al + be).max(0.0));
            }
        }
        if rho > bound - tol.lin {
            return Err(Refusal::RadiusTooLarge { edge: p.id, max: bound });
        }
    }
    Ok(())
}

/// Step 5 (iii): two listed edges' regions on one face must not meet.
fn meet(b: &Brep, plans: &[Plan], rho: f64, tol: Tol) -> Result<(), Refusal> {
    // (face, plan, corners) per region.
    let mut regs: Vec<(u32, usize, Vec<Vec3>)> = Vec::new();
    for (pi, p) in plans.iter().enumerate() {
        let (v0, v1) = (b.vertices[p.ends[0].v as usize].p, b.vertices[p.ends[1].v as usize].p);
        for i in 0..2 {
            regs.push((p.f[i], pi, vec![v0, v1, p.point(b, 1, i, rho), p.point(b, 0, i, rho)]));
        }
        for (j, end) in p.ends.iter().enumerate() {
            let vp = b.vertices[end.v as usize].p;
            regs.push((end.face, pi, vec![vp, p.point(b, j, 0, rho), p.point(b, j, 1, rho)]));
        }
    }
    for x in 0..regs.len() {
        for y in x + 1..regs.len() {
            let (fx, px, cx) = (regs[x].0, regs[x].1, &regs[x].2);
            let (fy, py, cy) = (regs[y].0, regs[y].1, &regs[y].2);
            if fx != fy || px == py {
                continue;
            }
            let fr = plane_frame(&b.faces[fx as usize]).ok_or(Refusal::BadRef { what: "fillet face" })?;
            let px2: Vec<Vec2> = cx.iter().map(|q| to2(&fr, *q)).collect();
            let py2: Vec<Vec2> = cy.iter().map(|q| to2(&fr, *q)).collect();
            let hit = Refusal::Contact { what: "fillet regions meet", a: plans[px].id, b: plans[py].id };
            for i in 0..px2.len() {
                let s1 = Segment2::new(px2[i], px2[(i + 1) % px2.len()]);
                for k in 0..py2.len() {
                    let s2 = Segment2::new(py2[k], py2[(k + 1) % py2.len()]);
                    if segment_segment2(&s1, &s2, tol)? != SegSeg2::Disjoint {
                        return Err(hit);
                    }
                }
            }
            for (poly, pts) in [(&px2, &py2), (&py2, &px2)] {
                let segs = region(poly);
                for q in pts.iter() {
                    if where2(&segs, *q, tol)? != Where::Outside {
                        return Err(hit);
                    }
                }
            }
        }
    }
    Ok(())
}

/// A convex polygon as counter-clockwise line pieces.
fn region(poly: &[Vec2]) -> Vec<Seg2> {
    let n = poly.len();
    let segs: Vec<Seg2> = (0..n).map(|i| Seg2::Line(poly[i], poly[(i + 1) % n])).collect();
    if area2(&segs) >= 0.0 { segs } else { (0..n).rev().map(|i| Seg2::Line(poly[(i + 1) % n], poly[i])).collect() }
}

/// A straight edge from `a` to `q` between existing vertices.
fn line(w: &mut Brep, name: Name, a: Vec3, q: Vec3, va: u32, vb: u32) -> Result<u32, Refusal> {
    let d = q - a;
    let dir = d.normalize()?;
    Ok(w.add_edge(name, Curve::Line { o: a, dir }, 0.0, d.mag(), va, vb))
}

/// The (start, end) vertex of a use of an edge.
fn ends_of(w: &Brep, c: (u32, bool)) -> (u32, u32) {
    let e = &w.edges[c.0 as usize];
    if c.1 { (e.v[1], e.v[0]) } else { (e.v[0], e.v[1]) }
}

/// Order a face's co-edges into one closed loop by their vertices.
fn chain(w: &Brep, mut rest: Vec<(u32, bool)>) -> Result<Vec<(u32, bool)>, Refusal> {
    let bad = Refusal::BadRef { what: "fillet blend loop" };
    if rest.is_empty() {
        return Err(bad);
    }
    let mut out = vec![rest.remove(0)];
    while !rest.is_empty() {
        let (_, end) = ends_of(w, out[out.len() - 1]);
        let i = rest.iter().position(|&c| ends_of(w, c).0 == end).ok_or(bad)?;
        out.push(rest.remove(i));
    }
    if ends_of(w, out[out.len() - 1]).1 != ends_of(w, out[0]).0 {
        return Err(bad);
    }
    Ok(out)
}

/// Step 6: edit a clone through the arena's own mint, then rebuild it compacted and validate.
fn build(b: &Brep, plans: &[Plan], kind: Kind, rho: f64, tol: Tol) -> Result<Brep, Refusal> {
    let op = match kind {
        Kind::Fillet { .. } => OpKind::Fillet,
        Kind::Chamfer { .. } => OpKind::Chamfer,
    };
    let (nv0, ne0) = (b.vertices.len(), b.edges.len());
    let mut w = b.clone();
    let mut lists: Vec<Vec<Vec<(u32, bool)>>> = (0..b.faces.len() as u32)
        .map(|fi| {
            b.face_loops(fi)
                .map(|lp| {
                    b.loops[lp as usize]
                        .coedges
                        .iter()
                        .map(|&ce| {
                            let c = b.coedges[ce as usize];
                            (c.edge, c.reversed)
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let mut dead_v = vec![false; nv0];
    let mut dead_e = vec![false; ne0];
    // (neighbour edge, the vertex it leaves, the new vertex) for the shortening pass.
    let mut moves: Vec<(u32, u32, u32)> = Vec::new();
    let mut blends: Vec<(u32, u32)> = Vec::new();
    for p in plans {
        let src = u32::try_from(p.id.0).map_err(|_| Refusal::BadParam("an Id past u32 in a fillet name"))?;
        let nm = |role: Role, k: u32| Name { op, role, a: src, b: k };
        let (p1, p2, q1, q2) = (p.point(b, 0, 0, rho), p.point(b, 0, 1, rho), p.point(b, 1, 0, rho), p.point(b, 1, 1, rho));
        let vp1 = w.add_vertex(nm(Role::Trim, 0), p1);
        let vp2 = w.add_vertex(nm(Role::Trim, 1), p2);
        let vq1 = w.add_vertex(nm(Role::Trim, 2), q1);
        let vq2 = w.add_vertex(nm(Role::Trim, 3), q2);
        let l1 = line(&mut w, nm(Role::Trim, 1), p1, q1, vp1, vq1)?;
        let l2 = line(&mut w, nm(Role::Trim, 2), p2, q2, vp2, vq2)?;
        let v0 = b.vertices[p.ends[0].v as usize].p;
        let (a0, a1, surface, reversed, uv) = match kind {
            Kind::Fillet { r } => {
                let c0 = v0 + (p.w[0] + p.w[1]).normalize()? * (r / libm::sin(p.phi * 0.5));
                let x = p1 - c0;
                let z = if (p2 - c0).dot(p.t.cross(x)) < 0.0 { -p.t } else { p.t };
                let f0 = Frame::new(c0, z, x)?;
                let (lx, ly, _) = f0.local(p2);
                let alpha = libm::atan2(ly, lx);
                let c1 = c0 + p.t * p.len;
                let a0 = w.add_edge(nm(Role::Blend, 1), Curve::Circle { f: f0, r }, 0.0, alpha, vp1, vp2);
                let a1 = w.add_edge(nm(Role::Blend, 2), Curve::Circle { f: f0.moved(c1), r }, 0.0, alpha, vq1, vq2);
                let vz = (c1 - c0).dot(f0.z);
                let uv = UvBox { u0: 0.0, u1: alpha, v0: vz.min(0.0), v1: vz.max(0.0) };
                (a0, a1, Surface::Cylinder { f: f0, r }, !p.convex, Some(uv))
            }
            Kind::Chamfer { .. } => {
                let a0 = line(&mut w, nm(Role::Blend, 1), p1, p2, vp1, vp2)?;
                let a1 = line(&mut w, nm(Role::Blend, 2), q1, q2, vq1, vq2)?;
                let mut n = p.t.cross(p2 - p1).normalize()?;
                if (n.dot(v0 - p1) > 0.0) != p.convex {
                    n = -n;
                }
                (a0, a1, Surface::Plane { f: Frame::new(p1, n, p.t)? }, false, None)
            }
        };
        // F1 / F2: the edge's co-edge becomes its trim line, in the same sense (F1's runs along t, F2's against).
        let sense = [false, true];
        for (i, li) in [l1, l2].into_iter().enumerate() {
            let lp = lists[p.f[i] as usize].iter_mut().flatten().find(|c| c.0 == p.e);
            let c = lp.ok_or(Refusal::BadRef { what: "fillet face loop" })?;
            *c = (li, sense[i]);
        }
        // E: the end edge between the co-edge that ends at the vertex and the one that starts there.
        let mut end_sense = [false; 2];
        for (j, aj) in [a0, a1].into_iter().enumerate() {
            let end = &p.ends[j];
            let mut placed = false;
            for l in lists[end.face as usize].iter_mut() {
                let n = l.len();
                let at = (0..n).find(|&i| ends_of(&w, l[i]).1 == end.v && ends_of(&w, l[(i + 1) % n]).0 == end.v);
                if let Some(i) = at {
                    let rev = l[i].0 != end.adj[0];
                    l.insert(i + 1, (aj, rev));
                    end_sense[j] = rev;
                    placed = true;
                    break;
                }
            }
            if !placed {
                return Err(Refusal::BadRef { what: "fillet end face loop" });
            }
        }
        let lp = chain(&w, vec![(l1, !sense[0]), (a1, !end_sense[1]), (l2, !sense[1]), (a0, !end_sense[0])])?;
        let bf = w.add_face(nm(Role::Blend, 0), surface, reversed, uv, &[]);
        lists.push(vec![lp]);
        blends.push((bf, p.f[0]));
        for (j, (va, vb)) in [(vp1, vp2), (vq1, vq2)].into_iter().enumerate() {
            let end = &p.ends[j];
            moves.push((end.adj[0], end.v, va));
            moves.push((end.adj[1], end.v, vb));
        }
        dead_v[p.ends[0].v as usize] = true;
        dead_v[p.ends[1].v as usize] = true;
        dead_e[p.e as usize] = true;
    }

    // Shorten the neighbours: the end at the dropped vertex moves to its trim vertex.
    for (a, old, new) in moves {
        let np = w.vertices[new as usize].p;
        let ed = &mut w.edges[a as usize];
        let Curve::Line { o, dir } = ed.curve else { return Err(Refusal::BadRef { what: "fillet neighbour" }) };
        let tp = (np - o).dot(dir);
        if ed.v[0] == old {
            ed.v[0] = new;
            ed.t0 = tp;
        } else {
            ed.v[1] = new;
            ed.t1 = tp;
        }
    }

    // Rebuild compacted: the dropped vertices and edges leave, co-edges and loops are rewritten from the lists.
    let mut out = w.clone();
    let mut vmap = vec![u32::MAX; w.vertices.len()];
    out.vertices.clear();
    for (i, v) in w.vertices.iter().enumerate() {
        if i < nv0 && dead_v[i] {
            continue;
        }
        vmap[i] = out.vertices.len() as u32;
        out.vertices.push(v.clone());
    }
    let mut emap = vec![u32::MAX; w.edges.len()];
    out.edges.clear();
    for (i, e) in w.edges.iter().enumerate() {
        if i < ne0 && dead_e[i] {
            continue;
        }
        let mut e = e.clone();
        for k in 0..2 {
            let m = vmap[e.v[k] as usize];
            if m == u32::MAX {
                return Err(Refusal::BadRef { what: "fillet dropped vertex" });
            }
            e.v[k] = m;
        }
        emap[i] = out.edges.len() as u32;
        out.edges.push(e);
    }
    out.coedges.clear();
    out.loops.clear();
    for (fi, lps) in lists.iter().enumerate() {
        let mut ls = Vec::new();
        for l in lps {
            let li = out.loops.len() as u32;
            let mut ces = Vec::new();
            for &(e, rev) in l {
                let m = emap[e as usize];
                if m == u32::MAX {
                    return Err(Refusal::BadRef { what: "fillet dropped edge" });
                }
                out.coedges.push(CoEdge { edge: m, reversed: rev, loop_: li });
                ces.push((out.coedges.len() - 1) as u32);
            }
            out.loops.push(Loop { coedges: ces, face: fi as u32 });
            ls.push(li);
        }
        let face = &mut out.faces[fi];
        face.outer = ls.first().copied().unwrap_or(u32::MAX);
        face.inner = ls.iter().skip(1).copied().collect();
    }
    for (bf, f1) in blends {
        let sh = out.shells.iter().position(|s| s.faces.contains(&f1)).ok_or(Refusal::BadRef { what: "fillet shell" })?;
        out.shells[sh].faces.push(bf);
    }
    validate(&out, tol)?;
    Ok(out)
}
