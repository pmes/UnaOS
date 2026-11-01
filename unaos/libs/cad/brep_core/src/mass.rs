//! Mass properties in closed form per face, by the divergence theorem over the EXACT surfaces (not the tessellation,
//! whose error, about chord / size, cannot reach a 1e-9 relative bar). With x relative to a reference point c (the
//! vertex bounding-box centre, against cancellation), and n the outward unit normal:
//!
//!   V = 1/3 ∮ x·n dA,   ∫x_i dV = 1/2 ∮ x_i² n_i dA,   ∫x_i² dV = 1/3 ∮ x_i³ n_i dA,
//!   ∫xy dV = 1/2 ∮ x² y n_x dA,   ∫yz dV = 1/2 ∮ y² z n_y dA,   ∫zx dV = 1/2 ∮ z² x n_z dA.
//!
//! A planar face integrates by Green's theorem in its own (s, t) frame: ∬ g ds dt = ∮ (∫_0^s g(σ, t) dσ) dt. The
//! inner integral is 4-point Gauss-Legendre, exact because g is a polynomial of degree <= 3. The boundary integral
//! is 16-point GL: exact on lines, and on pieces of at most π/4 for arcs and ellipses. A curved face is a uv-rectangle,
//! integrated by tensor 16-point GL with n dA = ±(S_u × S_v) du dv: angles on pieces of at most π/4, linear directions
//! (a cylinder's or cone's v) exact. A curved face without a uv box is refused, never approximated.

use crate::Refusal;
use crate::curve::Curve;
use crate::dep::{Tol, Vec3};
use crate::quad::{nodes4, nodes16, pieces};
use crate::surface::Surface;
use crate::topo::Brep;
use core::f64::consts::FRAC_PI_4;

/// Volume, surface area, centroid and the inertia tensor about the centroid, for unit density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mass {
    pub volume: f64,
    pub area: f64,
    pub centroid: Vec3,
    /// [[Ixx, Ixy, Ixz], [Iyx, Iyy, Iyz], [Izx, Izy, Izz]] with Ixy = -∫xy dV (the products enter negated).
    pub inertia: [[f64; 3]; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Moments {
    pub area: f64,
    pub vol: f64,
    /// ∫x, ∫y, ∫z (dV).
    pub s: [f64; 3],
    /// ∫x², ∫y², ∫z².
    pub q: [f64; 3],
    /// ∫xy, ∫yz, ∫zx.
    pub p: [f64; 3],
}

impl Moments {
    fn add(&mut self, x: Vec3, n: Vec3, w: f64) {
        self.area += w;
        self.vol += x.dot(n) * w / 3.0;
        self.s[0] += x.x * x.x * n.x * w / 2.0;
        self.s[1] += x.y * x.y * n.y * w / 2.0;
        self.s[2] += x.z * x.z * n.z * w / 2.0;
        self.q[0] += x.x * x.x * x.x * n.x * w / 3.0;
        self.q[1] += x.y * x.y * x.y * n.y * w / 3.0;
        self.q[2] += x.z * x.z * x.z * n.z * w / 3.0;
        self.p[0] += x.x * x.x * x.y * n.x * w / 2.0;
        self.p[1] += x.y * x.y * x.z * n.y * w / 2.0;
        self.p[2] += x.z * x.z * x.x * n.z * w / 2.0;
    }

    fn plus(&mut self, o: &Moments) {
        self.area += o.area;
        self.vol += o.vol;
        for i in 0..3 {
            self.s[i] += o.s[i];
            self.q[i] += o.q[i];
            self.p[i] += o.p[i];
        }
    }
}

/// The reference point: the centre of the vertices' bounding box (the origin for a Brep with no vertex).
pub(crate) fn reference(b: &Brep) -> Vec3 {
    let mut it = b.vertices.iter().map(|v| v.p);
    let Some(first) = it.next() else { return Vec3::ZERO };
    let (mut lo, mut hi) = (first, first);
    for p in it {
        lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    (lo + hi) * 0.5
}

/// One loop of a planar face by Green's theorem, accumulated into `m`.
fn planar_loop(b: &Brep, face: u32, lp: u32, c: Vec3, m: &mut Moments) -> Result<(), Refusal> {
    let f = &b.faces[face as usize];
    let Surface::Plane { f: pf } = f.surface else { return Err(Refusal::BadRef { what: "planar loop on a curved face" }) };
    let sgn = if f.reversed { -1.0 } else { 1.0 };
    let (o, x, y, n) = (pf.o, pf.x, pf.y * sgn, pf.z * sgn);
    for &ce in &b.loops[lp as usize].coedges {
        let e = &b.edges[b.coedges[ce as usize].edge as usize];
        let (ta, tb) = b.coedge_range(ce);
        let max = match e.curve {
            Curve::Line { .. } => f64::INFINITY,
            Curve::Circle { .. } | Curve::Ellipse { .. } => FRAC_PI_4,
            Curve::Nurbs(_) => return Err(Refusal::Unsupported("NURBS edge on a planar face")),
        };
        for (p0, p1) in pieces(ta, tb, max) {
            for (t, w) in nodes16(p0, p1) {
                let pt = e.curve.point(t);
                let d = e.curve.tangent(t).unwrap_or(Vec3::ZERO);
                let rel = pt - o;
                let (s, tt, dt) = (rel.dot(x), rel.dot(y), d.dot(y));
                for (sig, wi) in nodes4(0.0, s) {
                    m.add(o + x * sig + y * tt - c, n, w * dt * wi);
                }
            }
        }
    }
    Ok(())
}

/// A curved uv-rectangle face by tensor Gauss-Legendre.
fn curved(b: &Brep, face: u32, c: Vec3, m: &mut Moments) -> Result<(), Refusal> {
    let f = &b.faces[face as usize];
    let uv = f.uv.ok_or(Refusal::Unsupported("trimmed curved face"))?;
    let sgn = if f.reversed { -1.0 } else { 1.0 };
    let s = &f.surface;
    let vmax = match s {
        Surface::Cylinder { .. } | Surface::Cone { .. } => f64::INFINITY,
        _ => FRAC_PI_4,
    };
    for (u0, u1) in pieces(uv.u0, uv.u1, FRAC_PI_4) {
        for (u, wu) in nodes16(u0, u1) {
            for (v0, v1) in pieces(uv.v0, uv.v1, vmax) {
                for (v, wv) in nodes16(v0, v1) {
                    let j = s.du(u, v).cross(s.dv(u, v));
                    let mag = j.mag();
                    if mag == 0.0 {
                        continue;
                    }
                    m.add(s.point(u, v) - c, j * (sgn / mag), wu * wv * mag);
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn face_moments(b: &Brep, face: u32, c: Vec3, m: &mut Moments) -> Result<(), Refusal> {
    match b.faces[face as usize].surface {
        Surface::Plane { .. } => {
            for lp in b.face_loops(face) {
                planar_loop(b, face, lp, c, m)?;
            }
            Ok(())
        }
        _ => curved(b, face, c, m),
    }
}

/// The signed area of one loop of a planar face about the face's outward normal (outer > 0, inner < 0).
pub(crate) fn loop_area(b: &Brep, face: u32, lp: u32) -> Result<f64, Refusal> {
    let mut m = Moments::default();
    planar_loop(b, face, lp, reference(b), &mut m)?;
    Ok(m.area)
}

/// The volume one shell encloses (negative when its faces point inward).
pub(crate) fn shell_volume(b: &Brep, shell: u32) -> Result<f64, Refusal> {
    let c = reference(b);
    let mut m = Moments::default();
    for &f in &b.shells[shell as usize].faces {
        face_moments(b, f, c, &mut m)?;
    }
    Ok(m.vol)
}

fn moments(b: &Brep) -> Result<(Moments, Vec3), Refusal> {
    let c = reference(b);
    let mut total = Moments::default();
    for s in &b.solids {
        for &sh in &s.shells {
            for &f in &b.shells[sh as usize].faces {
                let mut m = Moments::default();
                face_moments(b, f, c, &mut m)?;
                total.plus(&m);
            }
        }
    }
    Ok((total, c))
}

/// All mass properties. Refuses a Brep whose solids enclose no positive volume (> Tol.lin³).
pub fn properties(b: &Brep, tol: Tol) -> Result<Mass, Refusal> {
    let (m, c) = moments(b)?;
    if !(m.vol > tol.lin * tol.lin * tol.lin) {
        return Err(Refusal::BadParam("volume not positive"));
    }
    let v = m.vol;
    let g = Vec3::new(m.s[0] / v, m.s[1] / v, m.s[2] / v);
    let sxx = m.q[0] - v * g.x * g.x;
    let syy = m.q[1] - v * g.y * g.y;
    let szz = m.q[2] - v * g.z * g.z;
    let pxy = m.p[0] - v * g.x * g.y;
    let pyz = m.p[1] - v * g.y * g.z;
    let pzx = m.p[2] - v * g.z * g.x;
    Ok(Mass {
        volume: v,
        area: m.area,
        centroid: c + g,
        inertia: [[syy + szz, -pxy, -pzx], [-pxy, sxx + szz, -pyz], [-pzx, -pyz, sxx + syy]],
    })
}

/// The enclosed volume (signed: negative for an inside-out Brep).
pub fn volume(b: &Brep) -> Result<f64, Refusal> {
    Ok(moments(b)?.0.vol)
}

/// The total surface area.
pub fn area(b: &Brep) -> Result<f64, Refusal> {
    Ok(moments(b)?.0.area)
}

/// The centroid (unit density).
pub fn centroid(b: &Brep, tol: Tol) -> Result<Vec3, Refusal> {
    Ok(properties(b, tol)?.centroid)
}

/// The inertia tensor about the centroid (unit density).
pub fn inertia(b: &Brep, tol: Tol) -> Result<[[f64; 3]; 3], Refusal> {
    Ok(properties(b, tol)?.inertia)
}
