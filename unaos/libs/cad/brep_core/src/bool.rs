//! CHARTER: Vug (CODEX §2, 3D) — shared-core: brep_core booleans (VUG-BREP-2, LEDGER SR481): `union`, `difference`
//! and `intersection` of two solids whose meeting faces are planes and cylinders, every other case refused by name.
//!
//! The road, in order (each step's refusal is the answer; nothing is nudged):
//! 1. Both operands validate. Face pairs whose boxes overlap go through THE closed-form table, `surface::intersect`
//!    (`NotClosedForm` by pair name propagates); coincident surfaces refuse `Contact "coincident faces"`
//!    (VUG-BREP-COPLANAR owed), an ellipse or a section on a cone / sphere / torus face refuses `Unsupported`.
//! 2. A vertex on a face of the other operand refuses `Contact "vertex on face"`.
//! 3. Pierce: every edge against every face of the other operand it may meet; a crossing inside the face is a NEW
//!    vertex, one on the face's boundary refuses `Contact "edge meets edge"`, an edge lying in or tangent to the face
//!    refuses by name.
//! 4. Sections: on each intersection curve the pierce points of both faces' edges, sorted; each interval whose
//!    midpoint lies inside both faces is a section edge. Sections end only at pierce vertices (one operand's faces
//!    meet only at its edges), so they never need splitting.
//! 5. Every source edge splits at its pierce points; every face splits by its sections: a planar face by a 2D dart
//!    arrangement, a cylinder face (a uv-rectangle: mass.rs and tess.rs read no other curved face) only by full-span
//!    iso lines into its uv grid; an untouched face is copied.
//! 6. Each sub-face's interior sample is classified against the other operand (classify.rs) and the table selects:
//!    union A Out / OnSame + B Out, intersection A In / OnSame + B In, difference A Out / OnOpposite + B In flipped.
//! 7. Sew: every result edge used once each way, else `NotClosed`; shells are the connected components, a shell of
//!    positive volume starts a solid, a void joins the solid that contains it; `validate` judges the result.
//!
//! Names: a kept sub-face keeps its source face's `Name` (the persistent reference across a boolean, resolved by
//! [`faces_named`]); a kept piece of a source edge keeps its edge's; a section edge is `Boolean / Section` with the
//! two source face Ids, a pierce vertex `Boolean / Section` with (edge Id, face Id). Result Ids are minted fresh in a
//! fixed order, so the same inputs give the same Ids; Ids across two operands collide (VUG-BREP-NAMES owed).
//! A union of disjoint solids is two solids of one shell each; an empty result is an empty Brep.

use crate::Refusal;
use crate::build::Seg2;
use crate::classify::{
    Body, Class, FaceGeo, LineCyl, Located, Where, all_faces, area2, class_of, face_geo, from2, in_face, inner_point,
    line_cylinder, outward, plane_frame, seg2, start_angle, where2,
};
use crate::curve::Curve;
use crate::dep::{Tol, Vec3};
use crate::mass::shell_volume;
use crate::surface::{Intersection, Surface, intersect};
use crate::topo::{Brep, Id, Name, OpKind, Role, UvBox};
use crate::validate::validate;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::f64::consts::{PI, TAU};

/// The boolean operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Union,
    Difference,
    Intersection,
}

/// A ∪ B.
pub fn union(a: &Brep, b: &Brep, tol: Tol) -> Result<Brep, Refusal> {
    boolean(a, b, Op::Union, tol)
}

/// A − B.
pub fn difference(a: &Brep, b: &Brep, tol: Tol) -> Result<Brep, Refusal> {
    boolean(a, b, Op::Difference, tol)
}

/// A ∩ B.
pub fn intersection(a: &Brep, b: &Brep, tol: Tol) -> Result<Brep, Refusal> {
    boolean(a, b, Op::Intersection, tol)
}

/// The faces (slots) carrying `name`: several when a boolean split the face.
pub fn faces_named(b: &Brep, name: Name) -> Vec<u32> {
    (0..b.faces.len() as u32).filter(|&i| b.faces[i as usize].name == name).collect()
}

type Bx = (Vec3, Vec3);

struct WVert {
    p: Vec3,
    name: Name,
}

struct WEdge {
    curve: Curve,
    t0: f64,
    t1: f64,
    v: [u32; 2],
    name: Name,
}

struct Pierce {
    /// The operand of the edge.
    k: usize,
    edge: u32,
    /// The face of the other operand it crosses.
    face: u32,
    t: f64,
    v: u32,
    p: Vec3,
}

struct Sub {
    k: usize,
    src: u32,
    surface: Surface,
    reversed: bool,
    uv: Option<UvBox>,
    loops: Vec<Vec<(u32, bool)>>,
    sample: Vec3,
    normal: Vec3,
}

fn low(id: Id) -> Result<u32, Refusal> {
    u32::try_from(id.0).map_err(|_| Refusal::BadParam("an Id past u32 in a section name"))
}

fn section_name(a: Id, b: Id) -> Result<Name, Refusal> {
    Ok(Name { op: OpKind::Boolean, role: Role::Section, a: low(a)?, b: low(b)? })
}

/// The Contact refusal with `a` the first operand's entity.
fn contact(what: &'static str, k: usize, mine: Id, other: Id) -> Refusal {
    if k == 0 { Refusal::Contact { what, a: mine, b: other } } else { Refusal::Contact { what, a: other, b: mine } }
}

fn grow(bx: &mut Bx, p: Vec3) {
    bx.0 = Vec3::new(bx.0.x.min(p.x), bx.0.y.min(p.y), bx.0.z.min(p.z));
    bx.1 = Vec3::new(bx.1.x.max(p.x), bx.1.y.max(p.y), bx.1.z.max(p.z));
}

fn pad(bx: Bx, d: f64) -> Bx {
    let v = Vec3::new(d, d, d);
    (bx.0 - v, bx.1 + v)
}

fn overlap(a: &Bx, b: &Bx) -> bool {
    a.0.x <= b.1.x && b.0.x <= a.1.x && a.0.y <= b.1.y && b.0.y <= a.1.y && a.0.z <= b.1.z && b.0.z <= a.1.z
}

fn contains(a: &Bx, p: Vec3) -> bool {
    overlap(a, &(p, p))
}

/// A conservative box of an edge: 32 samples on a curve plus their sagitta, and Tol.
fn edge_box(br: &Brep, ei: usize, tol: Tol) -> Bx {
    let e = &br.edges[ei];
    let p0 = e.curve.point(e.t0);
    let mut bx = (p0, p0);
    let n = match e.curve {
        Curve::Line { .. } => 1,
        _ => 32,
    };
    for i in 1..=n {
        grow(&mut bx, e.curve.point(e.t0 + (e.t1 - e.t0) * i as f64 / n as f64));
    }
    let sag = e.curve.max_radius().map_or(0.0, |r| r * (1.0 - libm::cos(PI / 32.0)));
    pad(bx, tol.lin + sag)
}

/// A conservative box of a face: its edges' boxes, and for a curved face a 16 x 16 uv grid plus its sagitta.
fn face_box(br: &Brep, fi: u32, tol: Tol) -> Bx {
    let f = &br.faces[fi as usize];
    let mut bx: Option<Bx> = None;
    for lp in br.face_loops(fi) {
        for &ce in &br.loops[lp as usize].coedges {
            let eb = edge_box(br, br.coedges[ce as usize].edge as usize, tol);
            match bx.as_mut() {
                Some(x) => {
                    grow(x, eb.0);
                    grow(x, eb.1);
                }
                None => bx = Some(eb),
            }
        }
    }
    let mut bx = bx.unwrap_or((Vec3::ZERO, Vec3::ZERO));
    if let Some(uv) = f.uv {
        let s = &f.surface;
        let n = 16;
        for i in 0..=n {
            for j in 0..=n {
                let u = uv.u0 + (uv.u1 - uv.u0) * i as f64 / n as f64;
                let v = uv.v0 + (uv.v1 - uv.v0) * j as f64 / n as f64;
                grow(&mut bx, s.point(u, v));
            }
        }
        let k = 1.0 - libm::cos(PI / 16.0);
        let r = match *s {
            Surface::Plane { .. } => 0.0,
            Surface::Cylinder { r, .. } | Surface::Sphere { r, .. } => r,
            Surface::Cone { r0, k: kk, .. } => libm::fabs(r0) + libm::fabs(kk) * libm::fabs(uv.v0).max(libm::fabs(uv.v1)),
            Surface::Torus { major, minor, .. } => major + minor,
        };
        bx = pad(bx, r * k);
    }
    pad(bx, tol.lin)
}

/// How a curve meets a surface.
enum Meet {
    /// The curve lies on the surface.
    On,
    /// Curve parameters, each with `true` when the touch is tangent.
    At(Vec<(f64, bool)>),
}

/// Edge curve x surface in closed form: line / plane, line / cylinder, circle / plane, and circle / cylinder through
/// `surface::intersect` (the circle read as the cylinder of its own axis).
fn curve_surface(c: &Curve, s: &Surface, tol: Tol) -> Result<Meet, Refusal> {
    match (c, s) {
        (Curve::Line { o, dir }, Surface::Plane { f }) => {
            let dn = f.z.dot(*dir);
            let dist = f.z.dot(*o - f.o);
            if libm::fabs(dn) <= tol.ang {
                return Ok(if libm::fabs(dist) <= tol.lin { Meet::On } else { Meet::At(Vec::new()) });
            }
            Ok(Meet::At(vec![(-dist / dn, false)]))
        }
        (Curve::Line { o, dir }, Surface::Cylinder { f, r }) => Ok(match line_cylinder(*o, *dir, f, *r, tol) {
            LineCyl::None => Meet::At(Vec::new()),
            LineCyl::On => Meet::On,
            LineCyl::Tangent(t) => Meet::At(vec![(t, true)]),
            LineCyl::Two(t0, t1) => Meet::At(vec![(t0, false), (t1, false)]),
        }),
        (Curve::Circle { f: cf, r }, Surface::Plane { f }) => {
            // a cos t + b sin t = c over the circle's points against the plane.
            let n = f.z;
            let (aa, bb, cc) = (n.dot(cf.x) * *r, n.dot(cf.y) * *r, n.dot(f.o - cf.o));
            let rr = libm::hypot(aa, bb);
            if rr <= tol.ang * *r || rr + libm::fabs(cc) <= tol.lin {
                return Ok(if libm::fabs(cc) + rr <= tol.lin { Meet::On } else { Meet::At(Vec::new()) });
            }
            if libm::fabs(cc) > rr + tol.lin {
                return Ok(Meet::At(Vec::new()));
            }
            let phi = libm::atan2(bb, aa);
            if libm::fabs(libm::fabs(cc) - rr) <= tol.lin {
                return Ok(Meet::At(vec![(if cc > 0.0 { phi } else { phi + PI }, true)]));
            }
            let ac = libm::acos((cc / rr).max(-1.0).min(1.0));
            Ok(Meet::At(vec![(phi - ac, false), (phi + ac, false)]))
        }
        (Curve::Circle { f: cf, r }, Surface::Cylinder { .. }) => {
            let own = Surface::Cylinder { f: *cf, r: *r };
            Ok(match intersect(&own, s, tol)? {
                Intersection::Coincident => Meet::On,
                Intersection::Curves(ls) => {
                    let tangent = ls.len() == 1;
                    let mut v = Vec::new();
                    for l in ls {
                        if let Curve::Line { o, dir } = l {
                            let den = dir.dot(cf.z);
                            let p = o + dir * ((cf.o - o).dot(cf.z) / den);
                            let (x, y, _) = cf.local(p);
                            v.push((libm::atan2(y, x), tangent));
                        }
                    }
                    Meet::At(v)
                }
                _ => Meet::At(Vec::new()),
            })
        }
        _ => Err(Refusal::Unsupported("an ellipse or NURBS edge, or an edge against a cone, sphere or torus face")),
    }
}

fn wrap(a: f64) -> f64 {
    let r = libm::fmod(a, TAU);
    if r < 0.0 { r + TAU } else { r }
}

/// The parameters strictly inside edge `ei` (of operand `k`) where it crosses face `fo` of the other operand.
fn edge_meets(src: &Brep, ei: usize, k: usize, oth: &Brep, fo: usize, g: &FaceGeo, tol: Tol) -> Result<Vec<f64>, Refusal> {
    let e = &src.edges[ei];
    let face = &oth.faces[fo];
    let span = e.t1 - e.t0;
    let mut out = Vec::new();
    match curve_surface(&e.curve, &face.surface, tol)? {
        Meet::On => {
            for i in 1..4 {
                let p = e.curve.point(e.t0 + span * i as f64 / 4.0);
                if in_face(g, p, tol)? != Where::Outside {
                    return Err(contact("edge on face", k, e.id, face.id));
                }
            }
        }
        Meet::At(ts) => {
            let (p0, p1) = (src.vertices[e.v[0] as usize].p, src.vertices[e.v[1] as usize].p);
            for (t, tangent) in ts {
                let t = match e.curve {
                    Curve::Line { .. } => t,
                    _ => e.t0 + wrap(t - e.t0),
                };
                if !(t > e.t0 && t < e.t1) {
                    continue;
                }
                let p = e.curve.point(t);
                if p.dist(p0) <= tol.lin || p.dist(p1) <= tol.lin {
                    continue; // a vertex: the vertex-on-face step owns it
                }
                match in_face(g, p, tol)? {
                    Where::Outside => {}
                    Where::Boundary => return Err(contact("edge meets edge", k, e.id, face.id)),
                    Where::Inside => {
                        if tangent {
                            return Err(contact("edge tangent to face", k, e.id, face.id));
                        }
                        out.push(t);
                    }
                }
            }
        }
    }
    Ok(out)
}

fn curve_dist(c: &Curve, p: Vec3) -> f64 {
    match c {
        Curve::Line { o, dir } => {
            let d = p - *o;
            (d - *dir * d.dot(*dir)).mag()
        }
        Curve::Circle { f, r } => {
            let (x, y, z) = f.local(p);
            libm::hypot(libm::hypot(x, y) - *r, z)
        }
        _ => f64::INFINITY,
    }
}

fn curve_param(c: &Curve, p: Vec3) -> f64 {
    match c {
        Curve::Line { o, dir } => (p - *o).dot(*dir),
        Curve::Circle { f, .. } => {
            let (x, y, _) = f.local(p);
            wrap(libm::atan2(y, x))
        }
        _ => 0.0,
    }
}

fn param_tol(c: &Curve, tol: Tol) -> f64 {
    match c {
        Curve::Circle { r, .. } => tol.lin / *r,
        _ => tol.lin,
    }
}

fn by_first(x: &(f64, u32), y: &(f64, u32)) -> Ordering {
    x.0.partial_cmp(&y.0).unwrap_or(Ordering::Equal)
}

fn boolean(a: &Brep, b: &Brep, op: Op, tol: Tol) -> Result<Brep, Refusal> {
    validate(a, tol)?;
    validate(b, tol)?;
    let ops = [a, b];
    let (na, nb) = (a.faces.len(), b.faces.len());

    // Per face: classification geometry and box; the edges each face uses and the faces each edge borders.
    let mut geo: [Vec<FaceGeo>; 2] = [Vec::new(), Vec::new()];
    let mut fbox: [Vec<Bx>; 2] = [Vec::new(), Vec::new()];
    let mut fedges: [Vec<Vec<u32>>; 2] = [Vec::new(), Vec::new()];
    let mut efaces: [Vec<Vec<u32>>; 2] = [Vec::new(), Vec::new()];
    for k in 0..2 {
        let s = ops[k];
        efaces[k] = vec![Vec::new(); s.edges.len()];
        for fi in 0..s.faces.len() as u32 {
            geo[k].push(face_geo(s, fi)?);
            fbox[k].push(face_box(s, fi, tol));
            let mut es: Vec<u32> = Vec::new();
            for lp in s.face_loops(fi) {
                for &ce in &s.loops[lp as usize].coedges {
                    let e = s.coedges[ce as usize].edge;
                    if !es.contains(&e) {
                        es.push(e);
                        efaces[k][e as usize].push(fi);
                    }
                }
            }
            fedges[k].push(es);
        }
    }

    // 1. Face pairs through the closed-form table.
    let mut sects: Vec<(usize, usize, Curve)> = Vec::new();
    let mut met = vec![vec![false; nb]; na];
    for fa in 0..na {
        for fb in 0..nb {
            if !overlap(&fbox[0][fa], &fbox[1][fb]) {
                continue;
            }
            let (sa, sb) = (&a.faces[fa].surface, &b.faces[fb].surface);
            let (ida, idb) = (a.faces[fa].id, b.faces[fb].id);
            match intersect(sa, sb, tol)? {
                Intersection::Empty => {}
                Intersection::Coincident => return Err(contact("coincident faces", 0, ida, idb)),
                Intersection::Point(p) => {
                    if in_face(&geo[0][fa], p, tol)? != Where::Outside && in_face(&geo[1][fb], p, tol)? != Where::Outside {
                        return Err(contact("faces touch at a point", 0, ida, idb));
                    }
                }
                Intersection::Curves(cs) => {
                    let ok = |s: &Surface| matches!(s, Surface::Plane { .. } | Surface::Cylinder { .. });
                    for c in cs {
                        if let Curve::Ellipse { .. } = c {
                            return Err(Refusal::Unsupported("oblique section of a curved face"));
                        }
                        if !ok(sa) || !ok(sb) {
                            return Err(Refusal::Unsupported("section on a cone, sphere or torus face"));
                        }
                        sects.push((fa, fb, c));
                    }
                    met[fa][fb] = true;
                }
            }
        }
    }

    // 2. A vertex on a face of the other operand.
    for k in 0..2 {
        let o = 1 - k;
        for v in &ops[k].vertices {
            for (fo, f) in ops[o].faces.iter().enumerate() {
                if contains(&fbox[o][fo], v.p) && f.surface.distance(v.p) <= tol.lin && in_face(&geo[o][fo], v.p, tol)? != Where::Outside {
                    return Err(contact("vertex on face", k, v.id, f.id));
                }
            }
        }
    }

    // 3. Pierce: edges against the faces of the other operand that their own faces met.
    let mut wv: Vec<WVert> = Vec::new();
    for s in ops {
        wv.extend(s.vertices.iter().map(|v| WVert { p: v.p, name: v.name }));
    }
    let vbase = [0u32, a.vertices.len() as u32];
    let mut pierce: Vec<Pierce> = Vec::new();
    for k in 0..2 {
        let (src, oth, o) = (ops[k], ops[1 - k], 1 - k);
        for ei in 0..src.edges.len() {
            let ebox = edge_box(src, ei, tol);
            for fo in 0..oth.faces.len() {
                let meets = efaces[k][ei].iter().any(|&fs| if k == 0 { met[fs as usize][fo] } else { met[fo][fs as usize] });
                if !meets || !overlap(&ebox, &fbox[o][fo]) {
                    continue;
                }
                for t in edge_meets(src, ei, k, oth, fo, &geo[o][fo], tol)? {
                    let p = src.edges[ei].curve.point(t);
                    pierce.push(Pierce { k, edge: ei as u32, face: fo as u32, t, v: wv.len() as u32, p });
                    wv.push(WVert { p, name: section_name(src.edges[ei].id, oth.faces[fo].id)? });
                }
            }
        }
    }

    // 4. Sections.
    let mut we: Vec<WEdge> = Vec::new();
    let mut sections: Vec<(u32, u32, u32)> = Vec::new();
    for (fa, fb, c) in &sects {
        let (fa, fb) = (*fa, *fb);
        let mut pts: Vec<(f64, u32)> = Vec::new();
        for pc in &pierce {
            let mine = if pc.k == 0 {
                pc.face as usize == fb && fedges[0][fa].contains(&pc.edge)
            } else {
                pc.face as usize == fa && fedges[1][fb].contains(&pc.edge)
            };
            if mine && curve_dist(c, pc.p) <= tol.lin {
                pts.push((curve_param(c, pc.p), pc.v));
            }
        }
        pts.sort_by(by_first);
        let mut spans: Vec<(f64, f64, u32, u32)> = Vec::new();
        if let Curve::Circle { .. } = c {
            if pts.is_empty() {
                spans.push((0.0, TAU, u32::MAX, u32::MAX));
            }
            for i in 0..pts.len() {
                let (t0, v0) = pts[i];
                let (t1, v1) = if i + 1 < pts.len() { pts[i + 1] } else { (pts[0].0 + TAU, pts[0].1) };
                spans.push((t0, t1, v0, v1));
            }
        } else {
            for w in pts.windows(2) {
                spans.push((w[0].0, w[1].0, w[0].1, w[1].1));
            }
        }
        let (ida, idb) = (a.faces[fa].id, b.faces[fb].id);
        for (t0, t1, v0, v1) in spans {
            if t1 - t0 <= param_tol(c, tol) {
                continue;
            }
            let m = c.point((t0 + t1) * 0.5);
            let (wa, wb) = (in_face(&geo[0][fa], m, tol)?, in_face(&geo[1][fb], m, tol)?);
            if wa == Where::Outside || wb == Where::Outside {
                continue;
            }
            if wa == Where::Boundary || wb == Where::Boundary {
                return Err(contact("section along an edge", 0, ida, idb));
            }
            if outward(&a.faces[fa], m)?.cross(outward(&b.faces[fb], m)?).mag() <= tol.ang {
                return Err(contact("tangent faces", 0, ida, idb));
            }
            let name = section_name(ida, idb)?;
            let (v0, v1) = if v0 == u32::MAX {
                wv.push(WVert { p: c.point(t0), name });
                ((wv.len() - 1) as u32, (wv.len() - 1) as u32)
            } else {
                (v0, v1)
            };
            we.push(WEdge { curve: c.clone(), t0, t1, v: [v0, v1], name });
            sections.push((fa as u32, fb as u32, (we.len() - 1) as u32));
        }
    }

    // 5. Source edges split at their pierce points.
    let mut pieces: [Vec<Vec<u32>>; 2] = [Vec::new(), Vec::new()];
    for k in 0..2 {
        for (ei, e) in ops[k].edges.iter().enumerate() {
            let mut ts: Vec<(f64, u32)> = vec![(e.t0, vbase[k] + e.v[0])];
            let mut cuts: Vec<(f64, u32)> =
                pierce.iter().filter(|pc| pc.k == k && pc.edge == ei as u32).map(|pc| (pc.t, pc.v)).collect();
            cuts.sort_by(by_first);
            ts.extend(cuts);
            ts.push((e.t1, vbase[k] + e.v[1]));
            let mut row = Vec::new();
            for w in ts.windows(2) {
                we.push(WEdge { curve: e.curve.clone(), t0: w[0].0, t1: w[1].0, v: [w[0].1, w[1].1], name: e.name });
                row.push((we.len() - 1) as u32);
            }
            pieces[k].push(row);
        }
    }

    // 5. Faces split by their sections.
    let mut subs: Vec<Sub> = Vec::new();
    for k in 0..2 {
        let src = ops[k];
        for fi in 0..src.faces.len() {
            let face = &src.faces[fi];
            let secs: Vec<u32> =
                sections.iter().filter(|s| if k == 0 { s.0 as usize == fi } else { s.1 as usize == fi }).map(|s| s.2).collect();
            let pierced = fedges[k][fi].iter().any(|&e| pieces[k][e as usize].len() > 1);
            if secs.is_empty() && !pierced {
                let mut loops = Vec::new();
                for lp in src.face_loops(fi as u32) {
                    let l: Vec<(u32, bool)> = src.loops[lp as usize]
                        .coedges
                        .iter()
                        .map(|&ce| {
                            let c = src.coedges[ce as usize];
                            (pieces[k][c.edge as usize][0], c.reversed)
                        })
                        .collect();
                    loops.push(l);
                }
                let (sample, normal) = match &geo[k][fi] {
                    FaceGeo::Planar { f, segs } => (from2(f, inner_point(segs, tol)?), f.z),
                    FaceGeo::Box { surface, uv } => {
                        let (um, vm) = ((uv.u0 + uv.u1) * 0.5, (uv.v0 + uv.v1) * 0.5);
                        let sgn = if face.reversed { -1.0 } else { 1.0 };
                        (surface.point(um, vm), surface.normal(um, vm)? * sgn)
                    }
                    FaceGeo::Other => return Err(Refusal::Unsupported("trimmed curved face")),
                };
                subs.push(Sub { k, src: fi as u32, surface: face.surface, reversed: face.reversed, uv: face.uv, loops, sample, normal });
                continue;
            }
            let bound: Vec<u32> = fedges[k][fi].iter().flat_map(|&e| pieces[k][e as usize].iter().copied()).collect();
            match (face.surface, face.uv) {
                (Surface::Plane { .. }, _) => split_planar(src, k, fi, &bound, &secs, &we, &geo[k][fi], tol, &mut subs)?,
                (Surface::Cylinder { .. }, Some(_)) => split_cylinder(src, k, fi, &bound, &secs, &we, tol, &mut subs)?,
                _ => return Err(Refusal::Unsupported("split of a cone, sphere or torus face")),
            }
        }
    }

    // 6. Classify and select.
    let body = [Body::new(a, all_faces(a).into_iter())?, Body::new(b, all_faces(b).into_iter())?];
    let mut kept: Vec<(Sub, bool)> = Vec::new();
    for s in subs {
        let c = class_of(&body[1 - s.k], s.sample, s.normal, tol)?;
        let keep = match (op, s.k, c) {
            (Op::Union, 0, Class::Out | Class::OnSame) | (Op::Union, 1, Class::Out) => Some(false),
            (Op::Intersection, 0, Class::In | Class::OnSame) | (Op::Intersection, 1, Class::In) => Some(false),
            (Op::Difference, 0, Class::Out | Class::OnOpposite) => Some(false),
            (Op::Difference, 1, Class::In) => Some(true),
            _ => None,
        };
        if let Some(flip) = keep {
            kept.push((s, flip));
        }
    }
    if kept.is_empty() {
        return Ok(Brep::new());
    }

    // 7. Sew and emit.
    let mut out = Brep::new();
    let mut vmap = vec![u32::MAX; wv.len()];
    let mut emap = vec![u32::MAX; we.len()];
    let mut uses: Vec<[u32; 2]> = Vec::new();
    for (s, flip) in &kept {
        let mut loops: Vec<Vec<(u32, bool)>> = Vec::new();
        for l in &s.loops {
            let mut lp: Vec<(u32, bool)> = Vec::new();
            for &(w, rev) in l {
                let wi = w as usize;
                if emap[wi] == u32::MAX {
                    let e = &we[wi];
                    let mut vv = [0u32; 2];
                    for i in 0..2 {
                        let x = e.v[i] as usize;
                        if vmap[x] == u32::MAX {
                            vmap[x] = out.add_vertex(wv[x].name, wv[x].p);
                        }
                        vv[i] = vmap[x];
                    }
                    emap[wi] = out.add_edge(e.name, e.curve.clone(), e.t0, e.t1, vv[0], vv[1]);
                    uses.push([0, 0]);
                }
                lp.push((emap[wi], rev));
            }
            if *flip {
                lp.reverse();
                for c in &mut lp {
                    c.1 = !c.1;
                }
            }
            for &(e, r) in &lp {
                uses[e as usize][r as usize] += 1;
            }
            loops.push(lp);
        }
        let name = ops[s.k].faces[s.src as usize].name;
        out.add_face(name, s.surface, s.reversed != *flip, s.uv, &loops);
    }
    for (e, u) in uses.iter().enumerate() {
        if u[0] != 1 || u[1] != 1 {
            return Err(Refusal::NotClosed { edge: out.edges[e].id });
        }
    }

    // Shells: connected components over shared edges.
    let nf = out.faces.len();
    let mut parent: Vec<usize> = (0..nf).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut first = vec![usize::MAX; out.edges.len()];
    for fi in 0..nf {
        for lp in out.face_loops(fi as u32) {
            for &ce in &out.loops[lp as usize].coedges {
                let e = out.coedges[ce as usize].edge as usize;
                if first[e] == usize::MAX {
                    first[e] = fi;
                } else {
                    let (ra, rb) = (root(&mut parent, first[e]), root(&mut parent, fi));
                    parent[ra] = rb;
                }
            }
        }
    }
    let mut comps: Vec<(usize, Vec<u32>)> = Vec::new();
    for fi in 0..nf {
        let r = root(&mut parent, fi);
        match comps.iter_mut().find(|c| c.0 == r) {
            Some(c) => c.1.push(fi as u32),
            None => comps.push((r, vec![fi as u32])),
        }
    }
    let shells: Vec<u32> = comps.into_iter().map(|c| out.add_shell(c.1)).collect();
    let mut vol = Vec::new();
    for &sh in &shells {
        vol.push(shell_volume(&out, sh)?);
    }
    let mut solids: Vec<Vec<u32>> = Vec::new();
    for (i, &sh) in shells.iter().enumerate() {
        if vol[i] > 0.0 {
            solids.push(vec![sh]);
        }
    }
    for (i, &sh) in shells.iter().enumerate() {
        if vol[i] > 0.0 {
            continue;
        }
        let f0 = out.shells[sh as usize].faces[0];
        let ce = out.loops[out.faces[f0 as usize].outer as usize].coedges[0];
        let p = out.vertices[out.edges[out.coedges[ce as usize].edge as usize].v[0] as usize].p;
        let mut home = None;
        for (si, s) in solids.iter().enumerate() {
            let body = Body::new(&out, out.shells[s[0] as usize].faces.iter().copied())?;
            if body.locate(p, tol)? == Located::In {
                home = Some(si);
                break;
            }
        }
        match home {
            Some(si) => solids[si].push(sh),
            None => return Err(Refusal::InsideOut { shell: sh }),
        }
    }
    for s in solids {
        out.add_solid(s);
    }
    validate(&out, tol)?;
    Ok(out)
}

/// A planar face split by a 2D dart arrangement of its edge pieces and sections.
#[allow(clippy::too_many_arguments)]
fn split_planar(
    src: &Brep,
    k: usize,
    fi: usize,
    bound: &[u32],
    secs: &[u32],
    we: &[WEdge],
    g: &FaceGeo,
    tol: Tol,
    subs: &mut Vec<Sub>,
) -> Result<(), Refusal> {
    let face = &src.faces[fi];
    let fr = plane_frame(face).ok_or(Refusal::BadRef { what: "planar split of a curved face" })?;
    let ws: Vec<u32> = bound.iter().chain(secs.iter()).copied().collect();
    let nd = ws.len() * 2;
    // Dart 2i runs piece i forward, 2i + 1 backward; the twin of d is d ^ 1.
    let mut segs: Vec<Seg2> = Vec::with_capacity(nd);
    let mut from: Vec<u32> = Vec::with_capacity(nd);
    for d in 0..nd {
        let w = &we[ws[d / 2] as usize];
        let fwd = d % 2 == 0;
        let (ta, tb) = if fwd { (w.t0, w.t1) } else { (w.t1, w.t0) };
        segs.push(seg2(&fr, &w.curve, ta, tb)?);
        from.push(if fwd { w.v[0] } else { w.v[1] });
    }
    // Outgoing darts per vertex, sorted counter-clockwise by the tangent angle at their start.
    let mut outd: Vec<(u32, f64, usize)> = (0..nd).map(|d| (from[d], start_angle(&segs[d]), d)).collect();
    outd.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.partial_cmp(&y.1).unwrap_or(Ordering::Equal)));
    let (mut pos, mut gs, mut gl) = (vec![0usize; nd], vec![0usize; nd], vec![0usize; nd]);
    let mut i = 0;
    while i < nd {
        let mut j = i;
        while j < nd && outd[j].0 == outd[i].0 {
            j += 1;
        }
        for x in i..j {
            if x + 1 < j && outd[x + 1].1 - outd[x].1 <= tol.ang {
                return Err(contact("tangent curves meet", k, face.id, face.id));
            }
            pos[outd[x].2] = x;
            gs[outd[x].2] = i;
            gl[outd[x].2] = j - i;
        }
        if j - i > 1 && outd[i].1 + TAU - outd[j - 1].1 <= tol.ang {
            return Err(contact("tangent curves meet", k, face.id, face.id));
        }
        i = j;
    }
    // next(d): the outgoing dart just clockwise of d's twin at d's head (the region stays on the left).
    let next = |d: usize| -> usize {
        let t = d ^ 1;
        let (p, s, l) = (pos[t], gs[t], gl[t]);
        outd[s + (p - s + l - 1) % l].2
    };
    let mut seen = vec![false; nd];
    let mut cycles: Vec<Vec<usize>> = Vec::new();
    for d0 in 0..nd {
        if seen[d0] {
            continue;
        }
        let mut cyc = Vec::new();
        let mut d = d0;
        while !seen[d] {
            seen[d] = true;
            cyc.push(d);
            d = next(d);
        }
        if d != d0 {
            return Err(Refusal::BadRef { what: "boolean arrangement cycle" });
        }
        cycles.push(cyc);
    }
    let cseg = |c: &Vec<usize>| -> Vec<Seg2> { c.iter().map(|&d| segs[d]).collect() };
    let areas: Vec<f64> = cycles.iter().map(|c| area2(&cseg(c))).collect();
    let pos_c: Vec<usize> = (0..cycles.len()).filter(|&c| areas[c] > 0.0).collect();
    // Each negative cycle (a component's boundary) joins the smallest positive cycle that strictly contains it.
    let mut holes: Vec<Vec<usize>> = vec![Vec::new(); cycles.len()];
    for c in 0..cycles.len() {
        if areas[c] >= 0.0 {
            continue;
        }
        let q = segs[cycles[c][0]].start();
        let mut best: Option<usize> = None;
        for &p in &pos_c {
            if where2(&cseg(&cycles[p]), q, tol)? == Where::Inside && best.map_or(true, |bp| areas[p] < areas[bp]) {
                best = Some(p);
            }
        }
        if let Some(p) = best {
            holes[p].push(c);
        }
    }
    for &p in &pos_c {
        let mut region = cseg(&cycles[p]);
        for &h in &holes[p] {
            region.extend(cseg(&cycles[h]));
        }
        let sample = from2(&fr, inner_point(&region, tol)?);
        match in_face(g, sample, tol)? {
            Where::Inside => {}
            Where::Outside => continue, // an original hole of the face
            Where::Boundary => return Err(contact("sub-face sample on a boundary", k, face.id, face.id)),
        }
        let lp = |c: usize| -> Vec<(u32, bool)> { cycles[c].iter().map(|&d| (ws[d / 2], d % 2 == 1)).collect() };
        let mut loops = vec![lp(p)];
        for &h in &holes[p] {
            loops.push(lp(h));
        }
        subs.push(Sub { k, src: fi as u32, surface: face.surface, reversed: face.reversed, uv: None, loops, sample, normal: fr.z });
    }
    Ok(())
}

/// A cylinder face (a uv-rectangle) split by full-span iso sections into its uv grid.
#[allow(clippy::too_many_arguments)]
fn split_cylinder(
    src: &Brep,
    k: usize,
    fi: usize,
    bound: &[u32],
    secs: &[u32],
    we: &[WEdge],
    tol: Tol,
    subs: &mut Vec<Sub>,
) -> Result<(), Refusal> {
    let face = &src.faces[fi];
    let (Surface::Cylinder { f, r }, Some(uv)) = (face.surface, face.uv) else {
        return Err(Refusal::BadRef { what: "cylinder split of another face" });
    };
    let s = face.surface;
    let tu = tol.lin / r;
    let mut us = vec![uv.u0, uv.u1];
    let mut vs = vec![uv.v0, uv.v1];
    for &w in secs {
        let e = &we[w as usize];
        match &e.curve {
            Curve::Circle { f: cf, r: cr } if cf.z.cross(f.z).mag() <= tol.ang && libm::fabs(*cr - r) <= tol.lin => {
                if libm::fabs(libm::fabs(e.t1 - e.t0) - (uv.u1 - uv.u0)) > tu {
                    return Err(Refusal::Unsupported("partial cut across a curved face"));
                }
                vs.push((cf.o - f.o).dot(f.z));
            }
            Curve::Line { o, dir } if dir.cross(f.z).mag() <= tol.ang => {
                let (x, y, _) = f.local(*o);
                let mut u = libm::atan2(y, x);
                while u < uv.u0 - tu {
                    u += TAU;
                }
                while u >= uv.u0 + TAU - tu {
                    u -= TAU;
                }
                let za = (e.curve.point(e.t0) - f.o).dot(f.z);
                let zb = (e.curve.point(e.t1) - f.o).dot(f.z);
                if libm::fabs(za.min(zb) - uv.v0) > tol.lin || libm::fabs(za.max(zb) - uv.v1) > tol.lin {
                    return Err(Refusal::Unsupported("partial cut across a curved face"));
                }
                us.push(u);
            }
            _ => return Err(Refusal::Unsupported("oblique cut on a curved face")),
        }
    }
    let lvl = |v: &mut Vec<f64>, t: f64| {
        v.sort_by(|x, y| x.partial_cmp(y).unwrap_or(Ordering::Equal));
        v.dedup_by(|x, y| libm::fabs(*x - *y) <= t);
    };
    lvl(&mut us, tu);
    lvl(&mut vs, tol.lin);
    let cand: Vec<u32> = bound.iter().chain(secs.iter()).copied().collect();
    // The ONE piece whose midpoint is the side's midpoint; reversed when it runs against the side.
    let side = |ua: f64, va: f64, ub: f64, vb: f64| -> Result<(u32, bool), Refusal> {
        let (um, vm) = ((ua + ub) * 0.5, (va + vb) * 0.5);
        let mid = s.point(um, vm);
        let dir = s.du(um, vm) * (ub - ua) + s.dv(um, vm) * (vb - va);
        for &w in &cand {
            let e = &we[w as usize];
            let tm = (e.t0 + e.t1) * 0.5;
            if e.curve.point(tm).dist(mid) <= tol.lin {
                let tg = e.curve.tangent(tm).unwrap_or(Vec3::ZERO);
                return Ok((w, tg.dot(dir) < 0.0));
            }
        }
        Err(Refusal::Unsupported("pierced curved face boundary off the cut grid"))
    };
    let sgn = if face.reversed { -1.0 } else { 1.0 };
    for j in 0..vs.len() - 1 {
        for i in 0..us.len() - 1 {
            let (u0, u1, v0, v1) = (us[i], us[i + 1], vs[j], vs[j + 1]);
            // Counter-clockwise about S_u x S_v: +u, +v, -u, -v; reversed with the face.
            let mut lp = vec![side(u0, v0, u1, v0)?, side(u1, v0, u1, v1)?, side(u1, v1, u0, v1)?, side(u0, v1, u0, v0)?];
            if face.reversed {
                lp.reverse();
                for c in &mut lp {
                    c.1 = !c.1;
                }
            }
            let (um, vm) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
            subs.push(Sub {
                k,
                src: fi as u32,
                surface: s,
                reversed: face.reversed,
                uv: Some(UvBox { u0, u1, v0, v1 }),
                loops: vec![lp],
                sample: s.point(um, vm),
                normal: s.normal(um, vm)? * sgn,
            });
        }
    }
    Ok(())
}
