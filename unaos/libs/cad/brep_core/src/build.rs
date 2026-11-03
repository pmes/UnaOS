//! Extrude and revolve: the ONE road by which brep_core writes topology (the primitives are calls into it).
//!
//! The profile arrives as sketch_core's `Profile` (outer counter-clockwise, holes clockwise) and is read ONCE, by
//! `loops_of`, into the internal `Seg2` loops. Nothing else in the crate reads sketch_core's shape.
//!
//! Orientation by construction:
//! - Extrude: in the working frame W (whose z is on the extrude side), a side face's loop runs bottom edge, up,
//!   top edge back, down. With the material on the left of the profile, that loop is counter-clockwise about the
//!   outward normal t × d. The bottom cap takes the profile reversed, the top cap takes it as is.
//! - Revolve: in meridian coordinates (ρ, z) with S(θ, s) = A + ρ(s) e_ρ(θ) + z(s) D, we have
//!   S_θ × S_s = ρ (z' e_ρ − ρ' D) = ρ n_out for a counter-clockwise (ρ, z) loop. So each swept face's
//!   uv-rectangle boundary (start circle +θ, the segment at the end angle, end circle −θ, the segment at angle 0
//!   reversed) is outward, and `Face::reversed` records where the surface's own parametrization points the other way.

use crate::Refusal;
use crate::curve::{Curve, Frame};
use crate::dep::{Arc2, Edge as PEdge, Loop as PLoop, Profile, Tol, Vec2, Vec3};
use crate::surface::Surface;
use crate::topo::{Brep, Name, OpKind, Role, UvBox};
use alloc::vec;
use alloc::vec::Vec;
use core::f64::consts::{FRAC_PI_2, PI, TAU};

/// One profile segment in 2D.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Seg2 {
    Line(Vec2, Vec2),
    Arc(Arc2),
}

fn polar(c: Vec2, r: f64, a: f64) -> Vec2 {
    Vec2::new(c.x + r * libm::cos(a), c.y + r * libm::sin(a))
}

fn wrap(a: f64) -> f64 {
    let r = libm::fmod(a, TAU);
    if r < 0.0 { r + TAU } else { r }
}

/// True when the arc's sweep passes angle `th` (start included, end excluded).
fn contains_angle(k: &Arc2, th: f64) -> bool {
    let rel = if k.sweep >= 0.0 { th - k.start } else { k.start - th };
    wrap(rel) < libm::fabs(k.sweep)
}

impl Seg2 {
    pub(crate) fn start(&self) -> Vec2 {
        match self {
            Seg2::Line(a, _) => *a,
            Seg2::Arc(k) => polar(k.centre, k.radius, k.start),
        }
    }
    pub(crate) fn end(&self) -> Vec2 {
        match self {
            Seg2::Line(_, b) => *b,
            Seg2::Arc(k) => polar(k.centre, k.radius, k.start + k.sweep),
        }
    }
    fn reversed(&self) -> Seg2 {
        match self {
            Seg2::Line(a, b) => Seg2::Line(*b, *a),
            Seg2::Arc(k) => Seg2::Arc(Arc2 { start: k.start + k.sweep, sweep: -k.sweep, ..*k }),
        }
    }
    /// The segment's share of the loop's signed area (Green): the chord's shoelace term, plus the circular segment
    /// r²/2 (φ − sin φ) for an arc of signed sweep φ.
    pub(crate) fn area_term(&self) -> f64 {
        let chord = self.start().cross(self.end()) * 0.5;
        match self {
            Seg2::Line(..) => chord,
            Seg2::Arc(k) => chord + k.radius * k.radius * 0.5 * (k.sweep - libm::sin(k.sweep)),
        }
    }
    /// The image under q -> ((q − o)·ex, (q − o)·ey), with (ex, ey) orthonormal (det ±1).
    fn mapped(&self, o: Vec2, ex: Vec2, ey: Vec2) -> Seg2 {
        let m = |q: Vec2| Vec2::new((q - o).dot(ex), (q - o).dot(ey));
        match self {
            Seg2::Line(a, b) => Seg2::Line(m(*a), m(*b)),
            Seg2::Arc(k) => {
                let d = Vec2::new(libm::cos(k.start), libm::sin(k.start));
                let start = libm::atan2(d.dot(ey), d.dot(ex));
                Seg2::Arc(Arc2 { centre: m(k.centre), radius: k.radius, start, sweep: k.sweep * ex.cross(ey) })
            }
        }
    }
}

fn reverse_loop(l: &[Seg2]) -> Vec<Seg2> {
    l.iter().rev().map(Seg2::reversed).collect()
}

fn loop_area(l: &[Seg2]) -> f64 {
    l.iter().map(Seg2::area_term).sum()
}

/// THE adapter: sketch_core's `Profile` into the internal loops (outer first, then the holes).
pub(crate) fn loops_of(p: &Profile) -> Vec<Vec<Seg2>> {
    let conv = |l: &PLoop| -> Vec<Seg2> {
        l.edges
            .iter()
            .map(|e| match e {
                PEdge::Line { from, to, .. } => Seg2::Line(*from, *to),
                PEdge::Arc { arc, .. } => Seg2::Arc(*arc),
            })
            .collect()
    };
    core::iter::once(&p.outer).chain(p.holes.iter()).map(conv).collect()
}

/// Each loop non-empty and closed within Tol.lin, no zero-length segment, the outer loop counter-clockwise and the
/// holes clockwise.
pub(crate) fn check_loops(loops: &[Vec<Seg2>], tol: Tol) -> Result<(), Refusal> {
    if loops.is_empty() || loops.iter().any(|l| l.is_empty()) {
        return Err(Refusal::BadParam("empty profile loop"));
    }
    for l in loops {
        for s in l {
            match s {
                Seg2::Line(a, b) => {
                    if (*b - *a).mag() <= tol.lin {
                        return Err(Refusal::BadParam("zero-length profile line"));
                    }
                }
                Seg2::Arc(k) => {
                    if !(k.radius > tol.lin) || !k.radius.is_finite() {
                        return Err(Refusal::BadParam("profile arc radius"));
                    }
                    if !(libm::fabs(k.sweep) > tol.ang) || libm::fabs(k.sweep) > TAU + tol.ang {
                        return Err(Refusal::BadParam("profile arc sweep"));
                    }
                }
            }
        }
        for i in 0..l.len() {
            if (l[i].end() - l[(i + 1) % l.len()].start()).mag() > tol.lin {
                return Err(Refusal::BadParam("profile loop open"));
            }
        }
    }
    if !(loop_area(&loops[0]) > 0.0) || loops[1..].iter().any(|l| !(loop_area(l) < 0.0)) {
        return Err(Refusal::ProfileOrientation);
    }
    Ok(())
}

/// The edge of segment `s` embedded by `pf` (2D (a, b) -> pf.at(a, b, 0)), from vertex `va` to `vb` in the segment's
/// direction. Returns (edge, forward): forward when the edge's own direction is the segment's.
fn seg_edge(b: &mut Brep, name: Name, s: &Seg2, pf: &Frame, va: u32, vb: u32) -> Result<(u32, bool), Refusal> {
    match s {
        Seg2::Line(p, q) => {
            let a = pf.at(p.x, p.y, 0.0);
            let dv = pf.at(q.x, q.y, 0.0) - a;
            let dir = dv.normalize()?;
            Ok((b.add_edge(name, Curve::Line { o: a, dir }, 0.0, dv.mag(), va, vb), true))
        }
        Seg2::Arc(k) => {
            let f = pf.moved(pf.at(k.centre.x, k.centre.y, 0.0));
            let fwd = k.sweep > 0.0;
            let (t0, t1) = if fwd { (k.start, k.start + k.sweep) } else { (k.start + k.sweep, k.start) };
            let (v0, v1) = if fwd { (va, vb) } else { (vb, va) };
            Ok((b.add_edge(name, Curve::Circle { f, r: k.radius }, t0, t1, v0, v1), fwd))
        }
    }
}

fn nm(op: OpKind, role: Role, a: usize, b: usize) -> Name {
    Name { op, role, a: a as u32, b: b as u32 }
}

/// Extrude a sketch profile placed by `frame` (the sketch's x, y; its normal z) along `dir` by `length` mm.
/// Line segments sweep planes, arcs sweep cylinders (an arc with `dir` off the normal is `ObliqueArc`), holes become
/// inner loops on both caps.
pub fn extrude(profile: &Profile, frame: &Frame, dir: Vec3, length: f64, tol: Tol) -> Result<Brep, Refusal> {
    let loops = loops_of(profile);
    check_loops(&loops, tol)?;
    extrude_loops(&loops, frame, dir, length, tol)
}

pub(crate) fn extrude_loops(
    loops: &[Vec<Seg2>],
    frame: &Frame,
    dir: Vec3,
    length: f64,
    tol: Tol,
) -> Result<Brep, Refusal> {
    const OP: OpKind = OpKind::Extrude;
    if !(length > tol.lin) || !length.is_finite() {
        return Err(Refusal::BadParam("extrude length"));
    }
    let d = dir.normalize()?;
    let c = d.dot(frame.z);
    if libm::fabs(c) <= tol.ang {
        return Err(Refusal::BadParam("extrude direction in the sketch plane"));
    }
    // The working frame W has its z on the extrude side. Below the plane, mirror y (a reflection), and reverse the
    // loops so they are counter-clockwise again.
    let (w, loops): (Frame, Vec<Vec<Seg2>>) = if c > 0.0 {
        (*frame, loops.to_vec())
    } else {
        let w = Frame { o: frame.o, x: frame.x, y: -frame.y, z: -frame.z };
        let (o, ex, ey) = (Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, -1.0));
        let m = loops
            .iter()
            .map(|l| reverse_loop(&l.iter().map(|s| s.mapped(o, ex, ey)).collect::<Vec<_>>()))
            .collect();
        (w, m)
    };
    if loops.iter().flatten().any(|s| matches!(s, Seg2::Arc(_))) && d.cross(w.z).mag() > tol.ang {
        return Err(Refusal::ObliqueArc);
    }
    let off = d * length;
    let top = w.moved(w.o + off);
    let mut b = Brep::new();

    let mut vb: Vec<Vec<u32>> = Vec::new();
    let mut vt: Vec<Vec<u32>> = Vec::new();
    for (li, l) in loops.iter().enumerate() {
        let row = l.iter().enumerate().map(|(j, s)| b.add_vertex(nm(OP, Role::CapStart, li, j), w.at(s.start().x, s.start().y, 0.0)));
        vb.push(row.collect());
    }
    for (li, l) in loops.iter().enumerate() {
        let row = l.iter().enumerate().map(|(j, s)| b.add_vertex(nm(OP, Role::CapEnd, li, j), top.at(s.start().x, s.start().y, 0.0)));
        vt.push(row.collect());
    }

    let mut eb: Vec<Vec<(u32, bool)>> = Vec::new();
    let mut et: Vec<Vec<(u32, bool)>> = Vec::new();
    let mut ev: Vec<Vec<u32>> = Vec::new();
    for (li, l) in loops.iter().enumerate() {
        let n = l.len();
        let mut row = Vec::new();
        for (j, s) in l.iter().enumerate() {
            row.push(seg_edge(&mut b, nm(OP, Role::CapStart, li, j), s, &w, vb[li][j], vb[li][(j + 1) % n])?);
        }
        eb.push(row);
    }
    for (li, l) in loops.iter().enumerate() {
        let n = l.len();
        let mut row = Vec::new();
        for (j, s) in l.iter().enumerate() {
            row.push(seg_edge(&mut b, nm(OP, Role::CapEnd, li, j), s, &top, vt[li][j], vt[li][(j + 1) % n])?);
        }
        et.push(row);
    }
    for (li, l) in loops.iter().enumerate() {
        let mut row = Vec::new();
        for (j, s) in l.iter().enumerate() {
            let o = w.at(s.start().x, s.start().y, 0.0);
            row.push(b.add_edge(nm(OP, Role::Sweep, li, j), Curve::Line { o, dir: d }, 0.0, length, vb[li][j], vt[li][j]));
        }
        ev.push(row);
    }

    let mut faces = Vec::new();
    // Bottom cap: outward −W.z, the profile reversed. A co-edge traversing a segment backward has reversed = forward.
    let bot = Surface::Plane { f: Frame { o: w.o, x: w.x, y: -w.y, z: -w.z } };
    let bl: Vec<Vec<(u32, bool)>> = eb.iter().map(|row| row.iter().rev().map(|&(e, f)| (e, f)).collect()).collect();
    faces.push(b.add_face(nm(OP, Role::CapStart, 0, 0), bot, false, None, &bl));
    // Top cap: outward +W.z, the profile as is.
    let tl: Vec<Vec<(u32, bool)>> = et.iter().map(|row| row.iter().map(|&(e, f)| (e, !f)).collect()).collect();
    faces.push(b.add_face(nm(OP, Role::CapEnd, 0, 0), Surface::Plane { f: top }, false, None, &tl));
    // Sides: bottom edge forward, up at the segment's end, top edge backward, down at its start.
    for (li, l) in loops.iter().enumerate() {
        let n = l.len();
        for (j, s) in l.iter().enumerate() {
            let k = (j + 1) % n;
            let lp = vec![vec![(eb[li][j].0, !eb[li][j].1), (ev[li][k], false), (et[li][j].0, et[li][j].1), (ev[li][j], true)]];
            let (surface, reversed, uv) = match s {
                Seg2::Line(p, q) => {
                    let a = w.at(p.x, p.y, 0.0);
                    let t = (w.at(q.x, q.y, 0.0) - a).normalize()?;
                    let z = t.cross(d).normalize()?;
                    (Surface::Plane { f: Frame { o: a, x: t, y: z.cross(t), z } }, false, None)
                }
                Seg2::Arc(k) => {
                    let f = w.moved(w.at(k.centre.x, k.centre.y, 0.0));
                    let (u0, u1) = if k.sweep > 0.0 { (k.start, k.start + k.sweep) } else { (k.start + k.sweep, k.start) };
                    let uv = UvBox { u0, u1, v0: 0.0, v1: off.dot(w.z) };
                    (Surface::Cylinder { f, r: k.radius }, k.sweep < 0.0, Some(uv))
                }
            };
            faces.push(b.add_face(nm(OP, Role::Side, li, j), surface, reversed, uv, &lp));
        }
    }
    let sh = b.add_shell(faces);
    b.add_solid(vec![sh]);
    Ok(b)
}

/// Revolve a sketch profile placed by `frame` about the axis through `axis_o` along `axis_d` (both in sketch
/// coordinates, a line in the sketch plane) by `angle` radians (0 < angle <= 2π). A segment parallel to the axis
/// sweeps a cylinder, one perpendicular a plane, an oblique one a cone, an arc centred on the axis a sphere, and any
/// other arc a torus. A segment on the axis sweeps nothing. A profile with material on both sides of the axis is
/// `ProfileCrossesAxis`.
pub fn revolve(
    profile: &Profile,
    frame: &Frame,
    axis_o: Vec2,
    axis_d: Vec2,
    angle: f64,
    tol: Tol,
) -> Result<Brep, Refusal> {
    let loops = loops_of(profile);
    check_loops(&loops, tol)?;
    revolve_loops(&loops, frame, axis_o, axis_d, angle, tol)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Axis,
    Plane,
    Cyl,
    Cone,
    Sphere,
    Torus,
}

fn kind(s: &Seg2, tol: Tol) -> Kind {
    match s {
        Seg2::Line(a, b) => {
            if a.x <= tol.lin && b.x <= tol.lin {
                Kind::Axis
            } else if libm::fabs(a.x - b.x) <= tol.lin {
                Kind::Cyl
            } else if libm::fabs(a.y - b.y) <= tol.lin {
                Kind::Plane
            } else {
                Kind::Cone
            }
        }
        Seg2::Arc(k) => {
            if libm::fabs(k.centre.x) <= tol.lin {
                Kind::Sphere
            } else {
                Kind::Torus
            }
        }
    }
}

fn curved(k: Kind) -> bool {
    matches!(k, Kind::Cyl | Kind::Cone | Kind::Sphere | Kind::Torus)
}

/// The (min, max) of q·perp − axis_o·perp over a segment.
fn rho_range(s: &Seg2, axis_o: Vec2, perp: Vec2) -> (f64, f64) {
    let f = |q: Vec2| (q - axis_o).dot(perp);
    let (a, b) = (f(s.start()), f(s.end()));
    let (mut lo, mut hi) = (a.min(b), a.max(b));
    if let Seg2::Arc(k) = s {
        let th = libm::atan2(perp.y, perp.x);
        let c = f(k.centre);
        if contains_angle(k, th) {
            hi = hi.max(c + k.radius);
        }
        if contains_angle(k, th + PI) {
            lo = lo.min(c - k.radius);
        }
    }
    (lo, hi)
}

pub(crate) fn revolve_loops(
    loops: &[Vec<Seg2>],
    frame: &Frame,
    axis_o: Vec2,
    axis_d: Vec2,
    angle: f64,
    tol: Tol,
) -> Result<Brep, Refusal> {
    const OP: OpKind = OpKind::Revolve;
    if !(angle > tol.ang) || angle > TAU + tol.ang || !angle.is_finite() {
        return Err(Refusal::BadParam("revolve angle"));
    }
    let full = angle >= TAU - tol.ang;
    let alpha = if full { TAU } else { angle };
    let ad = axis_d.normalize()?;
    let perp = Vec2::new(-ad.y, ad.x);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for s in loops.iter().flatten() {
        let (a, b) = rho_range(s, axis_o, perp);
        lo = lo.min(a);
        hi = hi.max(b);
    }
    if hi > tol.lin && lo < -tol.lin {
        return Err(Refusal::ProfileCrossesAxis);
    }
    if hi <= tol.lin && lo >= -tol.lin {
        return Err(Refusal::BadParam("profile on the axis"));
    }
    let sigma = if hi > tol.lin { 1.0 } else { -1.0 };
    // Meridian coordinates (ρ, z): ρ along σ·perp, z along the axis. The map's det is −σ; a reflection reverses
    // the loops so the outer loop stays counter-clockwise.
    let (ex, ey) = (perp * sigma, ad);
    let mut ml: Vec<Vec<Seg2>> = loops.iter().map(|l| l.iter().map(|s| s.mapped(axis_o, ex, ey)).collect()).collect();
    if ex.cross(ey) < 0.0 {
        ml = ml.iter().map(|l| reverse_loop(l)).collect();
    }
    for s in ml.iter().flatten() {
        if let Seg2::Arc(k) = s {
            let inner_min = k.centre.x - k.radius;
            if k.centre.x > tol.lin && contains_angle(k, PI) && inner_min <= tol.lin {
                return Err(Refusal::Unsupported("arc touching the axis inside its sweep"));
            }
        }
    }

    let a3 = frame.at(axis_o.x, axis_o.y, 0.0);
    let d3 = (frame.x * ad.x + frame.y * ad.y).normalize()?;
    let p3 = ((frame.x * perp.x + frame.y * perp.y) * sigma).normalize()?;
    let fr = Frame { o: a3, x: p3, y: d3.cross(p3), z: d3 };
    let e_rho = |th: f64| p3 * libm::cos(th) + fr.y * libm::sin(th);
    let meridian = |th: f64| {
        let x = e_rho(th);
        Frame { o: a3, x, y: d3, z: x.cross(d3) }
    };
    let m0 = meridian(0.0);
    let m1 = meridian(alpha);
    let on_axis = |q: Vec2| q.x <= tol.lin;
    let sweep_curve = |q: Vec2| Curve::Circle { f: fr.moved(a3 + d3 * q.y), r: q.x };

    let mut b = Brep::new();
    let kinds: Vec<Vec<Kind>> = ml.iter().map(|l| l.iter().map(|s| kind(s, tol)).collect()).collect();

    // Vertices at angle 0 (and, for a partial revolve, at the end angle; on-axis vertices are shared).
    let mut v0: Vec<Vec<Option<u32>>> = Vec::new();
    for (li, l) in ml.iter().enumerate() {
        let n = l.len();
        let mut row = Vec::new();
        for (j, s) in l.iter().enumerate() {
            let q = s.start();
            let keep = !full || !on_axis(q) || curved(kinds[li][(j + n - 1) % n]) || curved(kinds[li][j]);
            row.push(if keep { Some(b.add_vertex(nm(OP, Role::CapStart, li, j), m0.at(q.x, q.y, 0.0))) } else { None });
        }
        v0.push(row);
    }
    let mut v1: Vec<Vec<Option<u32>>> = v0.clone();
    if !full {
        for (li, l) in ml.iter().enumerate() {
            for (j, s) in l.iter().enumerate() {
                let q = s.start();
                if !on_axis(q) {
                    v1[li][j] = Some(b.add_vertex(nm(OP, Role::CapEnd, li, j), m1.at(q.x, q.y, 0.0)));
                }
            }
        }
    }
    let need = |v: Option<u32>| v.ok_or(Refusal::BadRef { what: "revolve vertex" });

    // Profile edges at angle 0 (partial: all of them, the cap boundary; full: the seams of curved faces), and at
    // the end angle (partial only; an axis segment's edge is shared by both caps).
    let mut e0: Vec<Vec<Option<(u32, bool)>>> = Vec::new();
    for (li, l) in ml.iter().enumerate() {
        let n = l.len();
        let mut row = Vec::new();
        for (j, s) in l.iter().enumerate() {
            let k = kinds[li][j];
            if full && !curved(k) {
                row.push(None);
                continue;
            }
            let role = if full { Role::Seam } else { Role::CapStart };
            let (va, vb) = (need(v0[li][j])?, need(v0[li][(j + 1) % n])?);
            row.push(Some(seg_edge(&mut b, nm(OP, role, li, j), s, &m0, va, vb)?));
        }
        e0.push(row);
    }
    let mut e1: Vec<Vec<Option<(u32, bool)>>> = e0.clone();
    if !full {
        for (li, l) in ml.iter().enumerate() {
            let n = l.len();
            for (j, s) in l.iter().enumerate() {
                if kinds[li][j] != Kind::Axis {
                    let (va, vb) = (need(v1[li][j])?, need(v1[li][(j + 1) % n])?);
                    e1[li][j] = Some(seg_edge(&mut b, nm(OP, Role::CapEnd, li, j), s, &m1, va, vb)?);
                }
            }
        }
    }
    // The circle (arc) each off-axis vertex sweeps.
    let mut sw: Vec<Vec<Option<u32>>> = Vec::new();
    for (li, l) in ml.iter().enumerate() {
        let mut row = Vec::new();
        for (j, s) in l.iter().enumerate() {
            let q = s.start();
            row.push(if on_axis(q) {
                None
            } else {
                let (va, vb) = (need(v0[li][j])?, need(v1[li][j])?);
                Some(b.add_edge(nm(OP, Role::Sweep, li, j), sweep_curve(q), 0.0, alpha, va, vb))
            });
        }
        sw.push(row);
    }
    let need_e = |e: Option<(u32, bool)>| e.ok_or(Refusal::BadRef { what: "revolve edge" });

    let mut faces = Vec::new();
    if !full {
        // Cap at angle 0: outward −e_θ(0) = P × D; the profile as is.
        let c0 = Frame { o: a3, x: p3, y: d3, z: p3.cross(d3) };
        let mut l0 = Vec::new();
        for row in &e0 {
            let mut lp = Vec::new();
            for &e in row {
                let (e, f) = need_e(e)?;
                lp.push((e, !f));
            }
            l0.push(lp);
        }
        faces.push(b.add_face(nm(OP, Role::CapStart, 0, 0), Surface::Plane { f: c0 }, false, None, &l0));
        // Cap at the end angle: outward +e_θ(α); the profile reversed.
        let xa = e_rho(alpha);
        let c1 = Frame { o: a3, x: xa, y: -d3, z: xa.cross(-d3) };
        let mut l1 = Vec::new();
        for row in &e1 {
            let mut lp = Vec::new();
            for &e in row.iter().rev() {
                let (e, f) = need_e(e)?;
                lp.push((e, f));
            }
            l1.push(lp);
        }
        faces.push(b.add_face(nm(OP, Role::CapEnd, 0, 0), Surface::Plane { f: c1 }, false, None, &l1));
    }

    for (li, l) in ml.iter().enumerate() {
        let n = l.len();
        for (j, s) in l.iter().enumerate() {
            let k = kinds[li][j];
            if k == Kind::Axis {
                continue;
            }
            let jb = (j + 1) % n;
            let (qa, qb) = (s.start(), s.end());
            let (sa, sb) = (sw[li][j], sw[li][jb]);
            let loops_f: Vec<Vec<(u32, bool)>> = if full && k == Kind::Plane {
                // A full planar ring: circles only, the larger one outer.
                let ca = sa.map(|e| (e, false));
                let cb = sb.map(|e| (e, true));
                match (ca, cb) {
                    (Some(x), Some(y)) => {
                        if qb.x > qa.x { vec![vec![y], vec![x]] } else { vec![vec![x], vec![y]] }
                    }
                    (Some(x), None) | (None, Some(x)) => vec![vec![x]],
                    (None, None) => return Err(Refusal::BadRef { what: "revolve planar face" }),
                }
            } else {
                let mut lp = Vec::new();
                if let Some(e) = sa {
                    lp.push((e, false));
                }
                let (end_e, end_f) = need_e(e1[li][j])?;
                lp.push((end_e, !end_f));
                if let Some(e) = sb {
                    lp.push((e, true));
                }
                let (st_e, st_f) = need_e(e0[li][j])?;
                lp.push((st_e, st_f));
                vec![lp]
            };
            let (surface, uv) = match (k, s) {
                (Kind::Cyl, Seg2::Line(a, bq)) => (
                    Surface::Cylinder { f: fr, r: a.x },
                    Some(UvBox { u0: 0.0, u1: alpha, v0: a.y.min(bq.y), v1: a.y.max(bq.y) }),
                ),
                (Kind::Cone, Seg2::Line(a, bq)) => {
                    let h = bq.y - a.y;
                    (
                        Surface::Cone { f: fr.moved(fr.at(0.0, 0.0, a.y)), r0: a.x, k: (bq.x - a.x) / h },
                        Some(UvBox { u0: 0.0, u1: alpha, v0: h.min(0.0), v1: h.max(0.0) }),
                    )
                }
                (Kind::Plane, Seg2::Line(a, _)) => (Surface::Plane { f: fr.moved(fr.at(0.0, 0.0, a.y)) }, None),
                (Kind::Sphere, Seg2::Arc(c)) => {
                    let s0 = libm::atan2(libm::sin(c.start), libm::cos(c.start));
                    let (x, y) = (s0, s0 + c.sweep);
                    let (v0, v1) = (x.min(y).max(-FRAC_PI_2), x.max(y).min(FRAC_PI_2));
                    (Surface::Sphere { f: fr.moved(fr.at(0.0, 0.0, c.centre.y)), r: c.radius }, Some(UvBox { u0: 0.0, u1: alpha, v0, v1 }))
                }
                (Kind::Torus, Seg2::Arc(c)) => {
                    let (x, y) = (c.start, c.start + c.sweep);
                    (
                        Surface::Torus { f: fr.moved(fr.at(0.0, 0.0, c.centre.y)), major: c.centre.x, minor: c.radius },
                        Some(UvBox { u0: 0.0, u1: alpha, v0: x.min(y), v1: x.max(y) }),
                    )
                }
                _ => return Err(Refusal::BadRef { what: "revolve segment kind" }),
            };
            // Outward normal at mid-angle from the meridian tangent: n_out ∝ t_z e_ρ − t_ρ D.
            let tm = match s {
                Seg2::Line(a, bq) => *bq - *a,
                Seg2::Arc(c) => {
                    let m = c.start + c.sweep * 0.5;
                    Vec2::new(-libm::sin(m), libm::cos(m)) * if c.sweep > 0.0 { 1.0 } else { -1.0 }
                }
            };
            let th = alpha * 0.5;
            let n_out = e_rho(th) * tm.y - d3 * tm.x;
            let sn = match uv {
                Some(bx) => {
                    let vm = (bx.v0 + bx.v1) * 0.5;
                    surface.du(th, vm).cross(surface.dv(th, vm))
                }
                None => surface.frame().z,
            };
            let reversed = sn.dot(n_out) < 0.0;
            let _ = (qa, qb);
            faces.push(b.add_face(nm(OP, Role::Side, li, j), surface, reversed, uv, &loops_f));
        }
    }
    let sh = b.add_shell(faces);
    b.add_solid(vec![sh]);
    Ok(b)
}
