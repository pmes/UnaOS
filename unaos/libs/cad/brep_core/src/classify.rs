//! CHARTER: Vug (CODEX §2, 3D) — shared-core: brep_core point classification (VUGBREP2, LEDGER SR481): where a point
//! lies against a face (inside, outside, on its boundary) and against a solid (by ray parity), the one judge the
//! booleans (src/bool.rs) read.
//!
//! - A planar face is read in its own frame (o, x, y·s, z·s), s = −1 when the face is reversed, so its outer loop is
//!   counter-clockwise (tess.rs's frame). Its loops become build.rs's `Seg2` pieces. In-face is 2D crossing parity:
//!   every straight edge is decided by `orient2d` (an `Uncertain` makes the ray ambiguous, never a sign), arcs in
//!   closed form with Tol bands. A ray within Tol of a vertex, tangent to an arc or along a line is ambiguous and the
//!   next fixed direction is tried; when every direction is ambiguous the answer is `Undecidable`.
//! - A curved face is its uv box; only the cylinder's (u, v) is inverted here (cone / sphere / torus: VUG-BREP-SSI).
//! - In-solid: `On` when the point lies on a face, else ray parity over the faces: a plane by its parameter, a
//!   cylinder by the quadratic (`line_cylinder`, also the booleans' edge/cylinder pierce), each hit then in-face.
//!   orient3d is not in the dep seam, so the ray's side of a plane is the closed-form parameter with the Tol band
//!   (VUG-BREP-SEAM owed).

use crate::Refusal;
use crate::build::Seg2;
use crate::curve::{Curve, Frame};
use crate::dep::{Arc2, Predicate, Tol, Vec2, Vec3, orient2d};
use crate::surface::Surface;
use crate::topo::{Brep, Face, UvBox};
use alloc::vec::Vec;
use core::f64::consts::{FRAC_PI_2, TAU};

/// A sub-face against the other solid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    In,
    Out,
    /// On a face of the other solid whose outward normal points the same way.
    OnSame,
    /// On a face of the other solid whose outward normal points the other way.
    OnOpposite,
}

/// A point against a face or a solid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    Inside,
    Outside,
    Boundary,
}

/// A face's geometry for classification.
#[derive(Clone, Debug)]
pub(crate) enum FaceGeo {
    /// The face frame (z the outward normal) and its loops' pieces in that frame.
    Planar { f: Frame, segs: Vec<Seg2> },
    /// A curved uv-rectangle face.
    Box { surface: Surface, uv: UvBox },
    /// A curved face with no uv box (not built by this crate).
    Other,
}

fn wrap(a: f64) -> f64 {
    let r = libm::fmod(a, TAU);
    if r < 0.0 { r + TAU } else { r }
}

/// The face frame of a planar face: outward z, the outer loop counter-clockwise about it.
pub(crate) fn plane_frame(face: &Face) -> Option<Frame> {
    let Surface::Plane { f } = face.surface else { return None };
    let s = if face.reversed { -1.0 } else { 1.0 };
    Some(Frame { o: f.o, x: f.x, y: f.y * s, z: f.z * s })
}

pub(crate) fn to2(f: &Frame, p: Vec3) -> Vec2 {
    let d = p - f.o;
    Vec2::new(d.dot(f.x), d.dot(f.y))
}

pub(crate) fn from2(f: &Frame, q: Vec2) -> Vec3 {
    f.o + f.x * q.x + f.y * q.y
}

/// The span (ta -> tb) of a curve lying in the plane of `f`, as a 2D piece in traversal order.
pub(crate) fn seg2(f: &Frame, c: &Curve, ta: f64, tb: f64) -> Result<Seg2, Refusal> {
    match c {
        Curve::Line { .. } => Ok(Seg2::Line(to2(f, c.point(ta)), to2(f, c.point(tb)))),
        Curve::Circle { f: cf, r } => {
            let s = if cf.z.dot(f.z) > 0.0 { 1.0 } else { -1.0 };
            let centre = to2(f, cf.o);
            let d = to2(f, c.point(ta)) - centre;
            Ok(Seg2::Arc(Arc2 { centre, radius: *r, start: libm::atan2(d.y, d.x), sweep: (tb - ta) * s }))
        }
        _ => Err(Refusal::Unsupported("ellipse or NURBS edge on a planar face in a boolean")),
    }
}

/// A face's classification geometry.
pub(crate) fn face_geo(b: &Brep, fi: u32) -> Result<FaceGeo, Refusal> {
    let face = &b.faces[fi as usize];
    if let Some(f) = plane_frame(face) {
        let mut segs = Vec::new();
        for lp in b.face_loops(fi) {
            for &ce in &b.loops[lp as usize].coedges {
                let e = &b.edges[b.coedges[ce as usize].edge as usize];
                let (ta, tb) = b.coedge_range(ce);
                segs.push(seg2(&f, &e.curve, ta, tb)?);
            }
        }
        return Ok(FaceGeo::Planar { f, segs });
    }
    Ok(match face.uv {
        Some(uv) => FaceGeo::Box { surface: face.surface, uv },
        None => FaceGeo::Other,
    })
}

/// The angle of `th` along an arc from its start, in its sweep sense, in [0, 2π).
fn arc_rel(k: &Arc2, th: f64) -> f64 {
    if k.sweep >= 0.0 { wrap(th - k.start) } else { wrap(k.start - th) }
}

fn seg_dist(s: &Seg2, q: Vec2) -> f64 {
    match s {
        Seg2::Line(a, b) => {
            let d = *b - *a;
            let l2 = d.mag_sq();
            let t = if l2 > 0.0 { ((q - *a).dot(d) / l2).max(0.0).min(1.0) } else { 0.0 };
            (q - (*a + d * t)).mag()
        }
        Seg2::Arc(k) => {
            let v = q - k.centre;
            if arc_rel(k, libm::atan2(v.y, v.x)) <= libm::fabs(k.sweep) {
                libm::fabs(v.mag() - k.radius)
            } else {
                (q - s.start()).mag().min((q - s.end()).mag())
            }
        }
    }
}

fn sign(p: Predicate) -> Option<i8> {
    match p {
        Predicate::Positive => Some(1),
        Predicate::Negative => Some(-1),
        _ => None,
    }
}

/// The hits of the ray q + s·d (d unit, s > Tol.lin) on one piece, pushed into `out`. Returns false when the ray is
/// ambiguous at this piece: through or within Tol of one of its ends, tangent to an arc, along a line, starting on it,
/// or an `Uncertain` predicate.
fn ray_hits(seg: &Seg2, q: Vec2, d: Vec2, tol: Tol, out: &mut Vec<f64>) -> bool {
    match seg {
        Seg2::Line(a, b) => {
            let q2 = q + d;
            let (sa, sb) = (sign(orient2d(q, q2, *a)), sign(orient2d(q, q2, *b)));
            let (Some(sa), Some(sb)) = (sa, sb) else {
                // An end on the ray's line (or undecided): ambiguous when that end is not behind q.
                let ahead = |p: Vec2| (p - q).dot(d) > -tol.lin;
                let za = sa.is_none() && ahead(*a);
                let zb = sb.is_none() && ahead(*b);
                return !(za || zb);
            };
            if sa == sb {
                return true;
            }
            let Some(so) = sign(orient2d(q, *a, *b)) else { return false };
            let e = *b - *a;
            let den = d.cross(e);
            let s = (*a - q).cross(e) / den;
            if so != sb {
                return true;
            }
            let h = q + d * s;
            if s <= tol.lin || (h - *a).mag() <= tol.lin || (h - *b).mag() <= tol.lin {
                if s > tol.lin {
                    out.push(s);
                }
                return false;
            }
            out.push(s);
            true
        }
        Seg2::Arc(k) => {
            let w = q - k.centre;
            let bb = d.dot(w);
            let perp = libm::fabs(d.cross(w));
            let tu = tol.lin / k.radius;
            let full = libm::fabs(k.sweep) >= TAU - tu;
            let in_arc = |s: f64| {
                let h = q + d * s - k.centre;
                let rel = arc_rel(k, libm::atan2(h.y, h.x));
                let near_end = rel <= tu || rel >= TAU - tu || libm::fabs(rel - libm::fabs(k.sweep)) <= tu;
                (rel <= libm::fabs(k.sweep), near_end && (!full || rel <= tu || rel >= TAU - tu))
            };
            if libm::fabs(perp - k.radius) <= tol.lin {
                // Tangent: ambiguous when the touch point is ahead and on the arc.
                let s = -bb;
                return !(s > -tol.lin && (in_arc(s).0 || in_arc(s).1));
            }
            if perp > k.radius {
                return true;
            }
            let half = libm::sqrt(k.radius * k.radius - perp * perp);
            let mut clean = true;
            for s in [-bb - half, -bb + half] {
                if s <= -tol.lin {
                    continue;
                }
                let (on, near) = in_arc(s);
                if s <= tol.lin {
                    if on || near {
                        clean = false;
                    }
                    continue;
                }
                if near {
                    clean = false;
                }
                if on {
                    out.push(s);
                }
            }
            clean
        }
    }
}

const DIRS2: [f64; 5] = [0.3711, 1.9123, 3.3547, 4.4321, 5.7781];

/// A 2D point against the region the pieces bound (outer counter-clockwise, holes clockwise).
pub(crate) fn where2(segs: &[Seg2], q: Vec2, tol: Tol) -> Result<Where, Refusal> {
    if segs.iter().any(|s| seg_dist(s, q) <= tol.lin) {
        return Ok(Where::Boundary);
    }
    let mut hits = Vec::new();
    for &a in &DIRS2 {
        let d = Vec2::new(libm::cos(a), libm::sin(a));
        hits.clear();
        if segs.iter().all(|s| ray_hits(s, q, d, tol, &mut hits)) {
            return Ok(if hits.len() % 2 == 1 { Where::Inside } else { Where::Outside });
        }
    }
    Err(Refusal::Undecidable)
}

/// A point strictly inside the region the pieces bound: from the midpoint of the longest piece, half way along its
/// inward normal to the nearest boundary hit.
pub(crate) fn inner_point(segs: &[Seg2], tol: Tol) -> Result<Vec2, Refusal> {
    let mut best: Option<(f64, Vec2, Vec2)> = None;
    for s in segs {
        let (len, m, n) = match s {
            Seg2::Line(a, b) => {
                let d = *b - *a;
                let l = d.mag();
                if l <= tol.lin {
                    continue;
                }
                (l, (*a + *b) * 0.5, Vec2::new(-d.y, d.x) * (1.0 / l))
            }
            Seg2::Arc(k) => {
                let th = k.start + k.sweep * 0.5;
                let e = Vec2::new(libm::cos(th), libm::sin(th));
                (k.radius * libm::fabs(k.sweep), k.centre + e * k.radius, if k.sweep > 0.0 { -e } else { e })
            }
        };
        if best.map_or(true, |b| len > b.0) {
            best = Some((len, m, n));
        }
    }
    let (_, m, n) = best.ok_or(Refusal::BadRef { what: "empty boolean region" })?;
    let mut hits = Vec::new();
    for s in segs {
        let _ = ray_hits(s, m, n, tol, &mut hits);
    }
    let near = hits.iter().copied().fold(f64::INFINITY, f64::min);
    if !near.is_finite() {
        return Err(Refusal::BadRef { what: "boolean region without a far side" });
    }
    Ok(m + n * (near * 0.5))
}

/// The signed area of a cycle of pieces (build.rs's term).
pub(crate) fn area2(segs: &[Seg2]) -> f64 {
    segs.iter().map(Seg2::area_term).sum()
}

/// The tangent angle of a piece at its start, in (−π, π].
pub(crate) fn start_angle(s: &Seg2) -> f64 {
    match s {
        Seg2::Line(a, b) => libm::atan2(b.y - a.y, b.x - a.x),
        Seg2::Arc(k) => {
            let th = k.start + if k.sweep > 0.0 { FRAC_PI_2 } else { -FRAC_PI_2 };
            libm::atan2(libm::sin(th), libm::cos(th))
        }
    }
}

/// A point already on a curved face's surface against its uv box (cylinder only).
fn in_box(s: &Surface, uv: &UvBox, p: Vec3, tol: Tol) -> Result<Where, Refusal> {
    let Surface::Cylinder { f, r } = s else {
        return Err(Refusal::Unsupported("classification against a cone, sphere or torus face"));
    };
    let (a, b, c) = f.local(p);
    let tu = tol.lin / *r;
    let mut u = libm::atan2(b, a);
    while u < uv.u0 - tu {
        u += TAU;
    }
    while u >= uv.u0 + TAU - tu {
        u -= TAU;
    }
    let inside = u > uv.u0 + tu && u < uv.u1 - tu && c > uv.v0 + tol.lin && c < uv.v1 - tol.lin;
    let near = u >= uv.u0 - tu && u <= uv.u1 + tu && c >= uv.v0 - tol.lin && c <= uv.v1 + tol.lin;
    Ok(if inside {
        Where::Inside
    } else if near {
        Where::Boundary
    } else {
        Where::Outside
    })
}

/// A point already on a face's surface (within Tol) against the face.
pub(crate) fn in_face(g: &FaceGeo, p: Vec3, tol: Tol) -> Result<Where, Refusal> {
    match g {
        FaceGeo::Planar { f, segs } => where2(segs, to2(f, p), tol),
        FaceGeo::Box { surface, uv } => in_box(surface, uv, p, tol),
        FaceGeo::Other => Err(Refusal::Unsupported("trimmed curved face")),
    }
}

/// The outward unit normal of a plane or cylinder face at a point on it.
pub(crate) fn outward(face: &Face, p: Vec3) -> Result<Vec3, Refusal> {
    let s = if face.reversed { -1.0 } else { 1.0 };
    match face.surface {
        Surface::Plane { f } => Ok(f.z * s),
        Surface::Cylinder { f, .. } => {
            let d = p - f.o;
            Ok((d - f.z * d.dot(f.z)).normalize()? * s)
        }
        _ => Err(Refusal::Unsupported("classification against a cone, sphere or torus face")),
    }
}

/// Where the line o + s·d meets a cylinder.
pub(crate) enum LineCyl {
    None,
    /// The line lies on the cylinder.
    On,
    /// Tangent at s.
    Tangent(f64),
    Two(f64, f64),
}

pub(crate) fn line_cylinder(o: Vec3, d: Vec3, f: &Frame, r: f64, tol: Tol) -> LineCyl {
    let (ox, oy, _) = f.local(o);
    let (dx, dy) = (d.dot(f.x), d.dot(f.y));
    let a = dx * dx + dy * dy;
    let lim = tol.ang * d.mag();
    if a <= lim * lim {
        return if libm::fabs(libm::hypot(ox, oy) - r) <= tol.lin { LineCyl::On } else { LineCyl::None };
    }
    let s0 = -(ox * dx + oy * dy) / a;
    let h = libm::hypot(ox + dx * s0, oy + dy * s0);
    if libm::fabs(h - r) <= tol.lin {
        return LineCyl::Tangent(s0);
    }
    if h > r {
        return LineCyl::None;
    }
    let half = libm::sqrt(r * r - h * h) / libm::sqrt(a);
    LineCyl::Two(s0 - half, s0 + half)
}

/// A point against a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Located {
    In,
    Out,
    /// On this face (its slot).
    On(u32),
}

const DIRS3: [(f64, f64, f64); 7] = [
    (0.5773, 0.3141, 0.7536),
    (-0.2718, 0.8284, 0.4142),
    (0.6180, -0.5257, 0.3090),
    (-0.7071, -0.1234, 0.6545),
    (0.1111, 0.2222, -0.9690),
    (0.8090, 0.4472, -0.3820),
    (-0.3333, -0.6667, -0.5555),
];

/// The faces a point is classified against, with their geometry.
pub(crate) struct Body<'a> {
    pub b: &'a Brep,
    pub faces: Vec<(u32, FaceGeo)>,
}

impl<'a> Body<'a> {
    pub(crate) fn new(b: &'a Brep, faces: impl Iterator<Item = u32>) -> Result<Self, Refusal> {
        let mut v = Vec::new();
        for f in faces {
            v.push((f, face_geo(b, f)?));
        }
        Ok(Body { b, faces: v })
    }

    /// `On` a face, else in or out by ray parity.
    pub(crate) fn locate(&self, p: Vec3, tol: Tol) -> Result<Located, Refusal> {
        for (fi, g) in &self.faces {
            if self.b.faces[*fi as usize].surface.distance(p) <= tol.lin && in_face(g, p, tol)? != Where::Outside {
                return Ok(Located::On(*fi));
            }
        }
        'dirs: for &(x, y, z) in &DIRS3 {
            let d = Vec3::new(x, y, z).normalize()?;
            let mut n = 0u32;
            for (fi, g) in &self.faces {
                let mut cand: [Option<f64>; 2] = [None, None];
                match self.b.faces[*fi as usize].surface {
                    Surface::Plane { f } => {
                        let dn = f.z.dot(d);
                        let dist = f.z.dot(p - f.o);
                        if libm::fabs(dn) <= tol.ang {
                            if libm::fabs(dist) <= tol.lin {
                                continue 'dirs;
                            }
                            continue;
                        }
                        cand[0] = Some(-dist / dn);
                    }
                    Surface::Cylinder { f, r } => match line_cylinder(p, d, &f, r, tol) {
                        LineCyl::None => continue,
                        LineCyl::On => continue 'dirs,
                        LineCyl::Tangent(s) => {
                            if s > -tol.lin && in_face(g, p + d * s, tol)? != Where::Outside {
                                continue 'dirs;
                            }
                            continue;
                        }
                        LineCyl::Two(s0, s1) => cand = [Some(s0), Some(s1)],
                    },
                    _ => return Err(Refusal::Unsupported("classification against a cone, sphere or torus face")),
                }
                for s in cand.into_iter().flatten() {
                    if s <= tol.lin {
                        continue;
                    }
                    match in_face(g, p + d * s, tol) {
                        Ok(Where::Inside) => n += 1,
                        Ok(Where::Outside) => {}
                        Ok(Where::Boundary) | Err(Refusal::Undecidable) => continue 'dirs,
                        Err(e) => return Err(e),
                    }
                }
            }
            return Ok(if n % 2 == 1 { Located::In } else { Located::Out });
        }
        Err(Refusal::Undecidable)
    }
}

/// Every face of every solid's shells.
pub(crate) fn all_faces(b: &Brep) -> Vec<u32> {
    let mut v = Vec::new();
    for s in &b.solids {
        for &sh in &s.shells {
            v.extend(b.shells[sh as usize].faces.iter().copied());
        }
    }
    v
}

/// A sub-face's sample `p` with outward normal `n` against a body.
pub(crate) fn class_of(body: &Body, p: Vec3, n: Vec3, tol: Tol) -> Result<Class, Refusal> {
    Ok(match body.locate(p, tol)? {
        Located::In => Class::In,
        Located::Out => Class::Out,
        Located::On(f) => {
            if outward(&body.b.faces[f as usize], p)?.dot(n) > 0.0 {
                Class::OnSame
            } else {
                Class::OnOpposite
            }
        }
    })
}

/// Where a point lies against a solid: on its boundary, inside or outside (ray parity over plane and cylinder faces).
pub fn point_in_solid(b: &Brep, p: Vec3, tol: Tol) -> Result<Where, Refusal> {
    let body = Body::new(b, all_faces(b).into_iter())?;
    Ok(match body.locate(p, tol)? {
        Located::In => Where::Inside,
        Located::Out => Where::Outside,
        Located::On(_) => Where::Boundary,
    })
}
