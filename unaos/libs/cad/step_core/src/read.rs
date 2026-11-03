//! The AP214 reader: every ADVANCED_BREP_SHAPE_REPRESENTATION in the file becomes one brep_core `Brep`, each of its
//! MANIFOLD_SOLID_BREP items one solid, scaled from the representation context's length unit to millimetres.
//!
//! The entities read (ISO 10303-42 geometry and topology as AP214 uses them, by their public EXPRESS names):
//! CARTESIAN_POINT, DIRECTION, VECTOR, AXIS2_PLACEMENT_3D; PLANE, CYLINDRICAL_SURFACE, CONICAL_SURFACE,
//! SPHERICAL_SURFACE, TOROIDAL_SURFACE; LINE, CIRCLE, ELLIPSE, B_SPLINE_CURVE_WITH_KNOTS (and its complex rational
//! form), SURFACE_CURVE / SEAM_CURVE (their 3D curve); VERTEX_POINT, EDGE_CURVE, ORIENTED_EDGE, EDGE_LOOP,
//! FACE_OUTER_BOUND, FACE_BOUND, ADVANCED_FACE, CLOSED_SHELL, MANIFOLD_SOLID_BREP; the unit context (SI_UNIT,
//! CONVERSION_BASED_UNIT and its measure with unit). A known AP214 entity outside that set, met on the way from a
//! shape representation to its geometry, refuses `Unsupported { entity, instance }`; any other name refuses
//! `UnknownEntity` with its position. Entities no shape representation reaches (the product chain, presentation) are
//! not geometry and are not read; a solid or a surface model left outside every representation is refused.
//!
//! The mappings onto brep_core:
//! - Part 42 parametrizes each elementary surface exactly as brep_core's `Surface` does (σ = C + ρ(v)(x cos u + y sin u)
//!   + ..., normal σ_u × σ_v), so ADVANCED_FACE.same_sense is `!Face::reversed`, and CONICAL_SURFACE(radius, semi_angle)
//!   is `Cone { r0: radius, k: tan(semi_angle) }`;
//! - EDGE_CURVE(start, end, curve, same_sense): the brep edge runs along the curve, so its vertices are (start, end)
//!   when same_sense, else (end, start); an ORIENTED_EDGE traverses it reversed when its orientation differs from
//!   same_sense; the parameter range is the projection of the vertices (a CIRCLE's angle in its placement, a closed
//!   edge spanning 2π);
//! - brep_core integrates and tessellates a curved face over its uv rectangle (`UvBox`), so the reader rebuilds that
//!   rectangle from the face's boundary and refuses a curved face that is not one (a trimmed curved face) rather than
//!   build a Brep whose mass would be silently wrong.
//!
//! Arena order: vertices and edges are minted in instance-number order, faces in their shell's order, so a file this
//! crate wrote reads back into the same arena order.

use crate::Refusal;
use crate::p21::{self, Body, Instance, Param, Table};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use brep_core::dep::{Tol, Vec3};
use brep_core::{Brep, Curve, Frame, Name, NurbsCurve, OpKind, Role, Surface, UvBox};
use core::f64::consts::{FRAC_PI_2, TAU};

/// Known AP214 entities the read subset lacks: met in a geometry position, each refuses by name.
const UNSUPPORTED: &[&str] = &[
    "B_SPLINE_SURFACE",
    "B_SPLINE_SURFACE_WITH_KNOTS",
    "BEZIER_SURFACE",
    "UNIFORM_SURFACE",
    "QUASI_UNIFORM_SURFACE",
    "RATIONAL_B_SPLINE_SURFACE",
    "OFFSET_SURFACE",
    "SURFACE_OF_REVOLUTION",
    "SURFACE_OF_LINEAR_EXTRUSION",
    "DEGENERATE_TOROIDAL_SURFACE",
    "RECTANGULAR_TRIMMED_SURFACE",
    "CURVE_BOUNDED_SURFACE",
    "ORIENTED_SURFACE",
    "B_SPLINE_CURVE",
    "BEZIER_CURVE",
    "UNIFORM_CURVE",
    "QUASI_UNIFORM_CURVE",
    "RATIONAL_B_SPLINE_CURVE",
    "TRIMMED_CURVE",
    "POLYLINE",
    "PCURVE",
    "COMPOSITE_CURVE",
    "OFFSET_CURVE_3D",
    "PARABOLA",
    "HYPERBOLA",
    "INTERSECTION_CURVE",
    "VERTEX_LOOP",
    "POLY_LOOP",
    "FACE_SURFACE",
    "ORIENTED_FACE",
    "ORIENTED_CLOSED_SHELL",
    "OPEN_SHELL",
    "CONNECTED_FACE_SET",
    "BREP_WITH_VOIDS",
    "FACETED_BREP",
    "SHELL_BASED_SURFACE_MODEL",
    "MAPPED_ITEM",
    "SUBEDGE",
    "ORIENTED_PATH",
    "AXIS1_PLACEMENT",
    "AXIS2_PLACEMENT_2D",
];

/// The subset itself: one of these in the wrong position refuses as `Unsupported` there, by its name.
const SUBSET: &[&str] = &[
    "CARTESIAN_POINT",
    "DIRECTION",
    "VECTOR",
    "AXIS2_PLACEMENT_3D",
    "PLANE",
    "CYLINDRICAL_SURFACE",
    "CONICAL_SURFACE",
    "SPHERICAL_SURFACE",
    "TOROIDAL_SURFACE",
    "LINE",
    "CIRCLE",
    "ELLIPSE",
    "B_SPLINE_CURVE_WITH_KNOTS",
    "SURFACE_CURVE",
    "SEAM_CURVE",
    "VERTEX_POINT",
    "EDGE_CURVE",
    "ORIENTED_EDGE",
    "EDGE_LOOP",
    "FACE_OUTER_BOUND",
    "FACE_BOUND",
    "ADVANCED_FACE",
    "CLOSED_SHELL",
    "MANIFOLD_SOLID_BREP",
];

/// Top-level geometry that must sit in a shape representation this reader takes.
const ROOTS: &[&str] = &[
    "BREP_WITH_VOIDS",
    "FACETED_BREP",
    "SHELL_BASED_SURFACE_MODEL",
    "GEOMETRIC_CURVE_SET",
    "MAPPED_ITEM",
    "MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
    "FACETED_BREP_SHAPE_REPRESENTATION",
    "GEOMETRICALLY_BOUNDED_SURFACE_SHAPE_REPRESENTATION",
    "GEOMETRICALLY_BOUNDED_WIREFRAME_SHAPE_REPRESENTATION",
];

const ABSR: &str = "ADVANCED_BREP_SHAPE_REPRESENTATION";

/// Read every advanced B-rep shape representation of a Part 21 AP214 file, in instance order, one `Brep` each.
pub fn read(text: &str, tol: Tol) -> Result<Vec<Brep>, Refusal> {
    let t = p21::parse(text)?;
    let rd = Rd { t: &t, tol };
    let mut out = Vec::new();
    let mut consumed = BTreeSet::new();
    for i in t.data.values() {
        if i.is(ABSR) {
            out.push(rd.shape(i, &mut consumed)?);
        }
    }
    if out.is_empty() {
        return Err(Refusal::NoShape);
    }
    for i in t.data.values() {
        if consumed.contains(&i.id) {
            continue;
        }
        if i.is("MANIFOLD_SOLID_BREP") {
            return Err(Refusal::Units { instance: i.id, what: "solid outside a shape representation" });
        }
        if let Some(r) = ROOTS.iter().find(|r| i.is(r)) {
            return Err(Refusal::Unsupported { entity: (*r).to_string(), instance: i.id });
        }
    }
    Ok(out)
}

/// The length scale to millimetres and the plane-angle scale to radians of one representation context.
#[derive(Clone, Copy)]
struct Units {
    len: f64,
    ang: f64,
}

struct EdgeRec {
    s: u64,
    e: u64,
    curve: Curve,
    same: bool,
    solid: u32,
}

struct FaceRec {
    id: u64,
    surface: Surface,
    same: bool,
    /// Outer loop first: (EDGE_CURVE instance, ORIENTED_EDGE orientation after the bound's own orientation).
    loops: Vec<Vec<(u64, bool)>>,
    solid: u32,
}

#[derive(Default)]
struct Gather {
    verts: BTreeMap<u64, (Vec3, u32)>,
    edges: BTreeMap<u64, EdgeRec>,
}

fn untyped(mut p: &Param) -> &Param {
    while let Param::Typed(_, q) = p {
        p = q;
    }
    p
}

/// The SI prefixes of ISO 10303-41 `si_prefix`.
fn prefix(p: &str) -> Option<f64> {
    Some(match p {
        "EXA" => 1e18,
        "PETA" => 1e15,
        "TERA" => 1e12,
        "GIGA" => 1e9,
        "MEGA" => 1e6,
        "KILO" => 1e3,
        "HECTO" => 1e2,
        "DECA" => 1e1,
        "DECI" => 1e-1,
        "CENTI" => 1e-2,
        "MILLI" => 1e-3,
        "MICRO" => 1e-6,
        "NANO" => 1e-9,
        "PICO" => 1e-12,
        "FEMTO" => 1e-15,
        "ATTO" => 1e-18,
        _ => return None,
    })
}

struct Rd<'a> {
    t: &'a Table,
    tol: Tol,
}

impl<'a> Rd<'a> {
    fn inst(&self, id: u64) -> Result<&'a Instance, Refusal> {
        self.t.get(id).ok_or(Refusal::Undefined { instance: id, line: 0, col: 0 })
    }

    /// The refusal for an instance in a position whose entity is not taken there.
    fn refuse(&self, i: &Instance) -> Refusal {
        for r in i.records() {
            if UNSUPPORTED.contains(&r.name.as_str()) || SUBSET.contains(&r.name.as_str()) {
                return Refusal::Unsupported { entity: r.name.clone(), instance: i.id };
            }
        }
        Refusal::UnknownEntity { entity: i.name(), instance: i.id, line: i.line, col: i.col }
    }

    /// The parameters of record `name` of `i`, refused unless there are exactly `n`.
    fn rec(&self, i: &'a Instance, name: &str, n: usize) -> Result<&'a [Param], Refusal> {
        let r = i.record(name).ok_or_else(|| self.refuse(i))?;
        if r.params.len() != n {
            return Err(Refusal::ParamCount {
                entity: name.to_string(),
                instance: i.id,
                line: i.line,
                col: i.col,
                expected: n,
                found: r.params.len(),
            });
        }
        Ok(&r.params)
    }

    fn bad(&self, i: &Instance, name: &str, index: usize, want: &'static str) -> Refusal {
        Refusal::ParamType { entity: name.to_string(), instance: i.id, line: i.line, col: i.col, index, want }
    }

    fn real(&self, i: &Instance, name: &str, ps: &[Param], k: usize) -> Result<f64, Refusal> {
        match ps.get(k).map(untyped) {
            Some(Param::Real(x)) if x.is_finite() => Ok(*x),
            Some(Param::Int(n)) => Ok(*n as f64),
            _ => Err(self.bad(i, name, k, "real")),
        }
    }

    fn int(&self, i: &Instance, name: &str, ps: &[Param], k: usize) -> Result<i64, Refusal> {
        match ps.get(k).map(untyped) {
            Some(Param::Int(n)) => Ok(*n),
            _ => Err(self.bad(i, name, k, "integer")),
        }
    }

    fn rf(&self, i: &Instance, name: &str, ps: &[Param], k: usize) -> Result<u64, Refusal> {
        match ps.get(k) {
            Some(Param::Ref { id, .. }) => Ok(*id),
            _ => Err(self.bad(i, name, k, "instance reference")),
        }
    }

    fn opt_rf(&self, i: &Instance, name: &str, ps: &[Param], k: usize) -> Result<Option<u64>, Refusal> {
        match ps.get(k) {
            Some(Param::Null) => Ok(None),
            _ => self.rf(i, name, ps, k).map(Some),
        }
    }

    fn list<'b>(&self, i: &Instance, name: &str, ps: &'b [Param], k: usize) -> Result<&'b [Param], Refusal> {
        match ps.get(k) {
            Some(Param::List(v)) => Ok(v),
            _ => Err(self.bad(i, name, k, "list")),
        }
    }

    fn boolean(&self, i: &Instance, name: &str, ps: &[Param], k: usize) -> Result<bool, Refusal> {
        match ps.get(k) {
            Some(Param::Enum(e)) if e == "T" => Ok(true),
            Some(Param::Enum(e)) if e == "F" => Ok(false),
            _ => Err(self.bad(i, name, k, "boolean .T. or .F.")),
        }
    }

    // ---- units ----

    /// The length and plane-angle units of a representation context's GLOBAL_UNIT_ASSIGNED_CONTEXT.
    fn units(&self, ctx: u64) -> Result<Units, Refusal> {
        const G: &str = "GLOBAL_UNIT_ASSIGNED_CONTEXT";
        let c = self.inst(ctx)?;
        if !c.is(G) {
            return Err(Refusal::Units { instance: ctx, what: "context without GLOBAL_UNIT_ASSIGNED_CONTEXT" });
        }
        let ps = self.rec(c, G, 1)?;
        let us = self.list(c, G, ps, 0)?;
        let (mut len, mut ang) = (None, None);
        for k in 0..us.len() {
            let u = self.rf(c, G, us, k)?;
            let ui = self.inst(u)?;
            if ui.is("LENGTH_UNIT") {
                len = Some(self.scale(u, false, 0)?);
            } else if ui.is("PLANE_ANGLE_UNIT") {
                ang = Some(self.scale(u, true, 0)?);
            }
        }
        Ok(Units {
            len: len.ok_or(Refusal::Units { instance: ctx, what: "no length unit" })?,
            ang: ang.ok_or(Refusal::Units { instance: ctx, what: "no plane angle unit" })?,
        })
    }

    /// One unit's factor: millimetres per unit (length) or radians per unit (angle), through at most 8 conversions.
    fn scale(&self, u: u64, angle: bool, depth: u32) -> Result<f64, Refusal> {
        if depth > 8 {
            return Err(Refusal::Units { instance: u, what: "unit chain deeper than 8" });
        }
        let ui = self.inst(u)?;
        if let Some(r) = ui.record("SI_UNIT") {
            // The partial of a complex instance carries (prefix, name); the simple entity (dimensions, prefix, name).
            let (pre, name) = match r.params.len() {
                2 => (&r.params[0], &r.params[1]),
                3 => (&r.params[1], &r.params[2]),
                n => {
                    return Err(Refusal::ParamCount {
                        entity: "SI_UNIT".to_string(),
                        instance: u,
                        line: ui.line,
                        col: ui.col,
                        expected: 2,
                        found: n,
                    });
                }
            };
            let f = match pre {
                Param::Null => 1.0,
                Param::Enum(p) => prefix(p).ok_or(Refusal::Units { instance: u, what: "unknown SI prefix" })?,
                _ => return Err(self.bad(ui, "SI_UNIT", 0, "SI prefix")),
            };
            return match name {
                Param::Enum(n) if !angle && n == "METRE" => Ok(1000.0 * f),
                Param::Enum(n) if angle && n == "RADIAN" => Ok(f),
                _ => Err(Refusal::Units {
                    instance: u,
                    what: if angle { "plane angle unit not RADIAN" } else { "length unit not METRE" },
                }),
            };
        }
        if ui.is("CONVERSION_BASED_UNIT") {
            const C: &str = "CONVERSION_BASED_UNIT";
            let ps = self.rec(ui, C, 2)?;
            let m = self.rf(ui, C, ps, 1)?;
            let mi = self.inst(m)?;
            let names = ["MEASURE_WITH_UNIT", "LENGTH_MEASURE_WITH_UNIT", "PLANE_ANGLE_MEASURE_WITH_UNIT"];
            let r = mi
                .records()
                .iter()
                .find(|r| names.contains(&r.name.as_str()) && r.params.len() == 2)
                .ok_or(Refusal::Units { instance: m, what: "conversion factor not a measure with unit" })?;
            let v = self.real(mi, &r.name, &r.params, 0)?;
            let base = self.rf(mi, &r.name, &r.params, 1)?;
            return Ok(v * self.scale(base, angle, depth + 1)?);
        }
        Err(Refusal::Units { instance: u, what: "unit neither SI_UNIT nor CONVERSION_BASED_UNIT" })
    }

    // ---- geometry ----

    fn point(&self, id: u64, u: Units) -> Result<Vec3, Refusal> {
        const N: &str = "CARTESIAN_POINT";
        let i = self.inst(id)?;
        let ps = self.rec(i, N, 2)?;
        let c = self.list(i, N, ps, 1)?;
        if c.len() != 3 {
            return Err(self.bad(i, N, 1, "three coordinates"));
        }
        Ok(Vec3::new(self.real(i, N, c, 0)? * u.len, self.real(i, N, c, 1)? * u.len, self.real(i, N, c, 2)? * u.len))
    }

    fn direction(&self, id: u64) -> Result<Vec3, Refusal> {
        const N: &str = "DIRECTION";
        let i = self.inst(id)?;
        let ps = self.rec(i, N, 2)?;
        let c = self.list(i, N, ps, 1)?;
        if c.len() != 3 {
            return Err(self.bad(i, N, 1, "three direction ratios"));
        }
        Ok(Vec3::new(self.real(i, N, c, 0)?, self.real(i, N, c, 1)?, self.real(i, N, c, 2)?))
    }

    /// AXIS2_PLACEMENT_3D(name, location, axis, ref_direction) as a `Frame` (axis `$` = z, ref_direction `$` = x, or
    /// y when the axis is near x).
    fn placement(&self, id: u64, u: Units) -> Result<Frame, Refusal> {
        const N: &str = "AXIS2_PLACEMENT_3D";
        let i = self.inst(id)?;
        let ps = self.rec(i, N, 4)?;
        let o = self.point(self.rf(i, N, ps, 1)?, u)?;
        let z = match self.opt_rf(i, N, ps, 2)? {
            None => Vec3::Z,
            Some(d) => self.direction(d)?,
        };
        let x = match self.opt_rf(i, N, ps, 3)? {
            Some(d) => self.direction(d)?,
            None if libm::fabs(z.x) < 0.9 * z.mag() => Vec3::X,
            None => Vec3::Y,
        };
        Frame::new(o, z, x).map_err(|refusal| Refusal::Geometry { instance: id, refusal })
    }

    fn positive(&self, id: u64, x: f64, what: &'static str) -> Result<f64, Refusal> {
        if x > 0.0 && x.is_finite() {
            Ok(x)
        } else {
            Err(Refusal::Geometry { instance: id, refusal: brep_core::Refusal::BadParam(what) })
        }
    }

    fn curve(&self, id: u64, u: Units, depth: u32) -> Result<Curve, Refusal> {
        let i = self.inst(id)?;
        if i.is("B_SPLINE_CURVE_WITH_KNOTS") {
            return self.bspline(i, u);
        }
        let Body::Simple(r) = &i.body else { return Err(self.refuse(i)) };
        match r.name.as_str() {
            "LINE" => {
                let ps = self.rec(i, "LINE", 3)?;
                let o = self.point(self.rf(i, "LINE", ps, 1)?, u)?;
                let v = self.rf(i, "LINE", ps, 2)?;
                let vi = self.inst(v)?;
                let vps = self.rec(vi, "VECTOR", 3)?;
                let d = self.direction(self.rf(vi, "VECTOR", vps, 1)?)?;
                let m = self.positive(v, d.mag(), "zero line direction")?;
                Ok(Curve::Line { o, dir: d * (1.0 / m) })
            }
            "CIRCLE" => {
                let ps = self.rec(i, "CIRCLE", 3)?;
                let f = self.placement(self.rf(i, "CIRCLE", ps, 1)?, u)?;
                let r = self.positive(id, self.real(i, "CIRCLE", ps, 2)? * u.len, "circle radius")?;
                Ok(Curve::Circle { f, r })
            }
            "ELLIPSE" => {
                let ps = self.rec(i, "ELLIPSE", 4)?;
                let f = self.placement(self.rf(i, "ELLIPSE", ps, 1)?, u)?;
                let a = self.positive(id, self.real(i, "ELLIPSE", ps, 2)? * u.len, "ellipse semi_axis_1")?;
                let b = self.positive(id, self.real(i, "ELLIPSE", ps, 3)? * u.len, "ellipse semi_axis_2")?;
                Ok(Curve::Ellipse { f, a, b })
            }
            n @ ("SURFACE_CURVE" | "SEAM_CURVE") => {
                if depth >= 8 {
                    return Err(Refusal::Unsupported { entity: format!("{n} nested past 8"), instance: id });
                }
                let ps = self.rec(i, n, 4)?;
                self.curve(self.rf(i, n, ps, 1)?, u, depth + 1)
            }
            _ => Err(self.refuse(i)),
        }
    }

    /// B_SPLINE_CURVE_WITH_KNOTS, simple (name, degree, control_points_list, curve_form, closed_curve,
    /// self_intersect, knot_multiplicities, knots, knot_spec) or complex (the B_SPLINE_CURVE, B_SPLINE_CURVE_WITH_KNOTS
    /// and optional RATIONAL_B_SPLINE_CURVE partials): the knots expanded by their multiplicities into `NurbsCurve`.
    fn bspline(&self, i: &'a Instance, u: Units) -> Result<Curve, Refusal> {
        const K: &str = "B_SPLINE_CURVE_WITH_KNOTS";
        const B: &str = "B_SPLINE_CURVE";
        const W: &str = "RATIONAL_B_SPLINE_CURVE";
        let (degree, ctrl, mults, knots, weights) = match &i.body {
            Body::Simple(_) => {
                let ps = self.rec(i, K, 9)?;
                let d = self.int(i, K, ps, 1)?;
                ((d, K), (self.list(i, K, ps, 2)?, K), (self.list(i, K, ps, 6)?, K), (self.list(i, K, ps, 7)?, K), None)
            }
            Body::Complex(_) => {
                let bs = self.rec(i, B, 5)?;
                let ks = self.rec(i, K, 3)?;
                let w = if i.is(W) { Some(self.list(i, W, self.rec(i, W, 1)?, 0)?) } else { None };
                (
                    (self.int(i, B, bs, 0)?, B),
                    (self.list(i, B, bs, 1)?, B),
                    (self.list(i, K, ks, 0)?, K),
                    (self.list(i, K, ks, 1)?, K),
                    w,
                )
            }
        };
        let Ok(p) = usize::try_from(degree.0) else { return Err(self.bad(i, degree.1, 0, "degree >= 0")) };
        let mut pts = Vec::new();
        for k in 0..ctrl.0.len() {
            pts.push(self.point(self.rf(i, ctrl.1, ctrl.0, k)?, u)?);
        }
        if mults.0.len() != knots.0.len() {
            return Err(self.bad(i, K, 0, "one multiplicity per knot"));
        }
        let mut kv = Vec::new();
        for k in 0..knots.0.len() {
            let m = self.int(i, mults.1, mults.0, k)?;
            let x = self.real(i, knots.1, knots.0, k)?;
            if !(1..=64).contains(&m) {
                return Err(self.bad(i, mults.1, k, "multiplicity 1..=64"));
            }
            kv.extend(core::iter::repeat_n(x, m as usize));
        }
        let ws = match weights {
            None => vec![1.0; pts.len()],
            Some(w) => {
                let mut v = Vec::new();
                for k in 0..w.len() {
                    v.push(self.real(i, W, w, k)?);
                }
                v
            }
        };
        NurbsCurve::new(p, kv, pts, ws)
            .map(Curve::Nurbs)
            .map_err(|refusal| Refusal::Geometry { instance: i.id, refusal })
    }

    fn surface(&self, id: u64, u: Units) -> Result<Surface, Refusal> {
        let i = self.inst(id)?;
        let Body::Simple(r) = &i.body else { return Err(self.refuse(i)) };
        let n = r.name.as_str();
        match n {
            "PLANE" => {
                let ps = self.rec(i, n, 2)?;
                Ok(Surface::Plane { f: self.placement(self.rf(i, n, ps, 1)?, u)? })
            }
            "CYLINDRICAL_SURFACE" => {
                let ps = self.rec(i, n, 3)?;
                let f = self.placement(self.rf(i, n, ps, 1)?, u)?;
                Ok(Surface::Cylinder { f, r: self.positive(id, self.real(i, n, ps, 2)? * u.len, "cylinder radius")? })
            }
            "CONICAL_SURFACE" => {
                let ps = self.rec(i, n, 4)?;
                let f = self.placement(self.rf(i, n, ps, 1)?, u)?;
                let r0 = self.real(i, n, ps, 2)? * u.len;
                let a = self.real(i, n, ps, 3)? * u.ang;
                if !(libm::fabs(a) < FRAC_PI_2 - self.tol.ang) {
                    return Err(Refusal::Geometry { instance: id, refusal: brep_core::Refusal::BadParam("cone semi_angle") });
                }
                Ok(Surface::Cone { f, r0, k: libm::tan(a) })
            }
            "SPHERICAL_SURFACE" => {
                let ps = self.rec(i, n, 3)?;
                let f = self.placement(self.rf(i, n, ps, 1)?, u)?;
                Ok(Surface::Sphere { f, r: self.positive(id, self.real(i, n, ps, 2)? * u.len, "sphere radius")? })
            }
            "TOROIDAL_SURFACE" => {
                let ps = self.rec(i, n, 4)?;
                let f = self.placement(self.rf(i, n, ps, 1)?, u)?;
                let major = self.positive(id, self.real(i, n, ps, 2)? * u.len, "torus major_radius")?;
                let minor = self.positive(id, self.real(i, n, ps, 3)? * u.len, "torus minor_radius")?;
                Ok(Surface::Torus { f, major, minor })
            }
            _ => Err(self.refuse(i)),
        }
    }

    // ---- topology ----

    fn edge(&self, ec: u64, solid: u32, u: Units, g: &mut Gather) -> Result<(), Refusal> {
        const N: &str = "EDGE_CURVE";
        if g.edges.contains_key(&ec) {
            return Ok(());
        }
        let ei = self.inst(ec)?;
        let ps = self.rec(ei, N, 5)?;
        let (s, e) = (self.rf(ei, N, ps, 1)?, self.rf(ei, N, ps, 2)?);
        let curve = self.curve(self.rf(ei, N, ps, 3)?, u, 0)?;
        let same = self.boolean(ei, N, ps, 4)?;
        for v in [s, e] {
            if !g.verts.contains_key(&v) {
                let vi = self.inst(v)?;
                let vps = self.rec(vi, "VERTEX_POINT", 2)?;
                let p = self.point(self.rf(vi, "VERTEX_POINT", vps, 1)?, u)?;
                g.verts.insert(v, (p, solid));
            }
        }
        g.edges.insert(ec, EdgeRec { s, e, curve, same, solid });
        Ok(())
    }

    fn face(&self, fid: u64, solid: u32, u: Units, g: &mut Gather) -> Result<FaceRec, Refusal> {
        const F: &str = "ADVANCED_FACE";
        let fi = self.inst(fid)?;
        let fps = self.rec(fi, F, 4)?;
        let bounds = self.list(fi, F, fps, 1)?;
        let surface = self.surface(self.rf(fi, F, fps, 2)?, u)?;
        let same = self.boolean(fi, F, fps, 3)?;
        let mut outer = None;
        let mut others = Vec::new();
        for k in 0..bounds.len() {
            let bi = self.inst(self.rf(fi, F, bounds, k)?)?;
            let (bn, is_outer) = if bi.is("FACE_OUTER_BOUND") {
                ("FACE_OUTER_BOUND", true)
            } else if bi.is("FACE_BOUND") {
                ("FACE_BOUND", false)
            } else {
                return Err(self.refuse(bi));
            };
            let bps = self.rec(bi, bn, 3)?;
            let li = self.inst(self.rf(bi, bn, bps, 1)?)?;
            let ori = self.boolean(bi, bn, bps, 2)?;
            let lps = self.rec(li, "EDGE_LOOP", 2)?;
            let oes = self.list(li, "EDGE_LOOP", lps, 1)?;
            let mut ces = Vec::new();
            for j in 0..oes.len() {
                let oi = self.inst(self.rf(li, "EDGE_LOOP", oes, j)?)?;
                let ops = self.rec(oi, "ORIENTED_EDGE", 5)?;
                let ec = self.rf(oi, "ORIENTED_EDGE", ops, 3)?;
                let o = self.boolean(oi, "ORIENTED_EDGE", ops, 4)?;
                self.edge(ec, solid, u, g)?;
                ces.push((ec, o));
            }
            if ces.is_empty() {
                return Err(self.bad(li, "EDGE_LOOP", 1, "at least one oriented edge"));
            }
            if !ori {
                ces.reverse();
                for c in ces.iter_mut() {
                    c.1 = !c.1;
                }
            }
            if is_outer && outer.is_none() {
                outer = Some(ces);
            } else {
                others.push(ces);
            }
        }
        let mut loops: Vec<Vec<(u64, bool)>> = outer.into_iter().collect();
        loops.extend(others);
        if loops.is_empty() {
            return Err(self.bad(fi, F, 1, "at least one bound"));
        }
        Ok(FaceRec { id: fid, surface, same, loops, solid })
    }

    /// One ADVANCED_BREP_SHAPE_REPRESENTATION(name, items, context_of_items) as one Brep.
    fn shape(&self, absr: &'a Instance, consumed: &mut BTreeSet<u64>) -> Result<Brep, Refusal> {
        let ps = self.rec(absr, ABSR, 3)?;
        let items = self.list(absr, ABSR, ps, 1)?;
        let u = self.units(self.rf(absr, ABSR, ps, 2)?)?;
        let mut msbs = Vec::new();
        for k in 0..items.len() {
            let it = self.rf(absr, ABSR, items, k)?;
            let ii = self.inst(it)?;
            if ii.is("MANIFOLD_SOLID_BREP") {
                msbs.push(it);
            } else if ii.is("AXIS2_PLACEMENT_3D") {
                // A placement item (the representation's origin), not geometry.
            } else {
                return Err(self.refuse(ii));
            }
        }
        let mut g = Gather::default();
        let mut faces: Vec<FaceRec> = Vec::new();
        let mut shells: Vec<Vec<usize>> = Vec::new();
        for (si, &m) in msbs.iter().enumerate() {
            consumed.insert(m);
            let si = si as u32;
            let mi = self.inst(m)?;
            let mps = self.rec(mi, "MANIFOLD_SOLID_BREP", 2)?;
            let shi = self.inst(self.rf(mi, "MANIFOLD_SOLID_BREP", mps, 1)?)?;
            let sps = self.rec(shi, "CLOSED_SHELL", 2)?;
            let fl = self.list(shi, "CLOSED_SHELL", sps, 1)?;
            let mut fidx = Vec::new();
            for k in 0..fl.len() {
                let fid = self.rf(shi, "CLOSED_SHELL", fl, k)?;
                fidx.push(faces.len());
                faces.push(self.face(fid, si, u, &mut g)?);
            }
            shells.push(fidx);
        }
        self.mint(&g, &faces, &shells)
    }

    /// The arena: vertices and edges in instance order, faces in shell order, then the shells and solids.
    fn mint(&self, g: &Gather, faces: &[FaceRec], shells: &[Vec<usize>]) -> Result<Brep, Refusal> {
        let nm = |id: u64, solid: u32| Name {
            op: OpKind::Import,
            role: Role::Entity,
            a: u32::try_from(id).unwrap_or(u32::MAX),
            b: solid,
        };
        let mut b = Brep::new();
        let mut vmap = BTreeMap::new();
        for (&id, &(p, si)) in &g.verts {
            vmap.insert(id, b.add_vertex(nm(id, si), p));
        }
        let mut emap = BTreeMap::new();
        for (&id, er) in &g.edges {
            let (a, z) = if er.same { (er.s, er.e) } else { (er.e, er.s) };
            let (t0, t1) = self.params(&er.curve, g.verts[&a].0, g.verts[&z].0, a == z);
            emap.insert(id, b.add_edge(nm(id, er.solid), er.curve.clone(), t0, t1, vmap[&a], vmap[&z]));
        }
        let mut fmap = Vec::new();
        for f in faces {
            let loops: Vec<Vec<(u32, bool)>> = f
                .loops
                .iter()
                .map(|l| l.iter().map(|&(ec, o)| (emap[&ec], o != g.edges[&ec].same)).collect())
                .collect();
            let uv = match f.surface {
                Surface::Plane { .. } => None,
                _ => Some(uv_box(&b, &f.surface, !f.same, &loops, self.tol).map_err(|why| Refusal::Unsupported {
                    entity: format!("ADVANCED_FACE ({why})"),
                    instance: f.id,
                })?),
            };
            fmap.push(b.add_face(nm(f.id, f.solid), f.surface, !f.same, uv, &loops));
        }
        for sh in shells {
            let s = b.add_shell(sh.iter().map(|&k| fmap[k]).collect());
            b.add_solid(vec![s]);
        }
        Ok(b)
    }

    /// The parameters (t0, t1) of the vertices `pa`, `pz` on `c` (t1 > t0; a closed edge spans the whole curve).
    fn params(&self, c: &Curve, pa: Vec3, pz: Vec3, closed: bool) -> (f64, f64) {
        let angles = |f: &Frame, a: f64, b: f64| {
            let ang = |p: Vec3| {
                let (x, y, _) = f.local(p);
                libm::atan2(y / b, x / a)
            };
            let t0 = ang(pa);
            let t1 = if closed {
                t0 + TAU
            } else {
                let mut t = ang(pz);
                while t <= t0 + self.tol.ang {
                    t += TAU;
                }
                t
            };
            (t0, t1)
        };
        match c {
            Curve::Line { o, dir } => ((pa - *o).dot(*dir), (pz - *o).dot(*dir)),
            Curve::Circle { f, r } => angles(f, *r, *r),
            Curve::Ellipse { f, a, b } => angles(f, *a, *b),
            Curve::Nurbs(n) => {
                let (lo, hi) = n.range();
                if closed { (lo, hi) } else { (self.project(n, pa, false), self.project(n, pz, true)) }
            }
        }
    }

    /// The parameter of `p` on a B-spline curve: a range end when `p` is on it (the high end first when `hi`), else
    /// the nearest of 257 samples refined by ternary search (validate's VertexOffCurve judges the result).
    fn project(&self, n: &NurbsCurve, p: Vec3, hi_first: bool) -> f64 {
        let (lo, hi) = n.range();
        let near = |t: f64| n.point(t).dist(p);
        let ends = if hi_first { [hi, lo] } else { [lo, hi] };
        for t in ends {
            if near(t) <= self.tol.lin {
                return t;
            }
        }
        const S: usize = 256;
        let (mut best, mut bd) = (lo, f64::INFINITY);
        for k in 0..=S {
            let t = lo + (hi - lo) * k as f64 / S as f64;
            let d = near(t);
            if d < bd {
                bd = d;
                best = t;
            }
        }
        let h = (hi - lo) / S as f64;
        let (mut a, mut z) = ((best - h).max(lo), (best + h).min(hi));
        for _ in 0..100 {
            let (m1, m2) = (a + (z - a) / 3.0, z - (z - a) / 3.0);
            if near(m1) < near(m2) {
                z = m2;
            } else {
                a = m1;
            }
        }
        (a + z) * 0.5
    }
}

// ---- the uv rectangle of a curved face ----

/// Samples per co-edge along the boundary (a full circle: π/16 apart, well inside the unwrap's ±π).
const SAMPLES: usize = 32;

fn wrap_pi(x: f64) -> f64 {
    x - TAU * libm::round(x / TAU)
}

/// (u, v, ρ) of a point in the surface's own parametrization; u is undefined on the axis (a pole, an apex).
fn coord(s: &Surface, p: Vec3, tol: Tol) -> (Option<f64>, f64, f64) {
    let (a, b, c) = s.frame().local(p);
    let rho = libm::hypot(a, b);
    let u = if rho > tol.lin { Some(libm::atan2(b, a)) } else { None };
    let v = match s {
        Surface::Plane { .. } => 0.0,
        Surface::Cylinder { .. } | Surface::Cone { .. } => c,
        Surface::Sphere { .. } => libm::atan2(c, rho),
        Surface::Torus { major, .. } => libm::atan2(c, rho - *major),
    };
    (u, v, rho)
}

/// One boundary loop sampled in order, u unwrapped; `wind` is its net u turns.
struct Walk {
    u: Vec<Option<f64>>,
    v: Vec<f64>,
    rho: Vec<f64>,
    wind: i64,
}

/// The signed area of the loop in (u, v), a pole sample standing for the segment along v = pole between its
/// neighbours' u.
fn uv_area(u: &[Option<f64>], v: &[f64]) -> f64 {
    let n = u.len();
    let mut pts: Vec<(f64, f64)> = Vec::new();
    for i in 0..n {
        match u[i] {
            Some(x) => pts.push((x, v[i])),
            None => {
                let prev = (1..n).find_map(|k| u[(i + n - k) % n]);
                let next = (1..n).find_map(|k| u[(i + k) % n]);
                if let (Some(a), Some(b)) = (prev, next) {
                    pts.push((a, v[i]));
                    pts.push((b, v[i]));
                }
            }
        }
    }
    let m = pts.len();
    (0..m).map(|i| pts[i].0 * pts[(i + 1) % m].1 - pts[(i + 1) % m].0 * pts[i].1).sum::<f64>() * 0.5
}

fn span(u: &[Option<f64>]) -> f64 {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for x in u.iter().flatten() {
        lo = lo.min(*x);
        hi = hi.max(*x);
    }
    hi - lo
}

fn shift(u: &mut [Option<f64>], from: usize, by: f64) {
    for x in u[from..].iter_mut().flatten() {
        *x += by;
    }
}

fn walk(b: &Brep, s: &Surface, lp: &[(u32, bool)], reversed: bool, tol: Tol) -> Result<Walk, &'static str> {
    let mut raw: Vec<(Option<f64>, f64, f64)> = Vec::new();
    for &(e, rev) in lp {
        let ed = &b.edges[e as usize];
        let (ta, tb) = if rev { (ed.t1, ed.t0) } else { (ed.t0, ed.t1) };
        for k in 0..SAMPLES {
            let t = ta + (tb - ta) * k as f64 / SAMPLES as f64;
            raw.push(coord(s, ed.curve.point(t), tol));
        }
    }
    let first = raw.iter().position(|r| r.0.is_some()).ok_or("boundary on the axis")?;
    raw.rotate_left(first);
    let n = raw.len();
    let u_first = raw[0].0.unwrap_or(0.0);
    let mut u = vec![None; n];
    let mut acc = u_first;
    u[0] = Some(acc);
    let mut pending = false;
    let mut jumps = Vec::new();
    for i in 1..n {
        match raw[i].0 {
            Some(x) => {
                if pending {
                    jumps.push(i);
                    pending = false;
                }
                acc += wrap_pi(x - acc);
                u[i] = Some(acc);
            }
            None => pending = true,
        }
    }
    if pending {
        jumps.push(n);
    }
    let net = acc + wrap_pi(u_first - acc) - u_first;
    let netk = libm::round(net / TAU) as i64;
    let mut v: Vec<f64> = raw.iter().map(|r| r.1).collect();
    let rho: Vec<f64> = raw.iter().map(|r| r.2).collect();
    if let Surface::Torus { .. } = s {
        // The tube angle is periodic too: unwrap it, and a loop must not wind about the tube.
        let mut a = v[0];
        for x in v.iter_mut().skip(1) {
            a += wrap_pi(*x - a);
            *x = a;
        }
        if libm::fabs(a + wrap_pi(v[0] - a) - v[0]) > 1.0 {
            return Err("seamless torus face");
        }
    }
    let wind = match jumps.len() {
        0 => netk,
        // One pole crossing: its jump is the one that closes the loop.
        1 => {
            shift(&mut u, jumps[0], -(netk as f64) * TAU);
            0
        }
        // Two (a lune between the poles): the jumps whose loop runs counter-clockwise in (u, v) about the outward
        // normal, i.e. counter-clockwise when the face is not reversed.
        2 => {
            let want = if reversed { -1.0 } else { 1.0 };
            let mut chosen = None;
            for m in [-1i64, 0, 1] {
                let mut c = u.clone();
                shift(&mut c, jumps[0], m as f64 * TAU);
                shift(&mut c, jumps[1], (-netk - m) as f64 * TAU);
                if uv_area(&c, &v) * want > 0.0 && span(&c) <= TAU + tol.ang {
                    chosen = Some(c);
                    break;
                }
            }
            u = chosen.ok_or("pole crossings")?;
            0
        }
        _ => return Err("more than two pole crossings"),
    };
    Ok(Walk { u, v, rho, wind })
}

/// The uv rectangle a curved face's boundary bounds, refused (with the reason) when the boundary is not one.
fn uv_box(b: &Brep, s: &Surface, reversed: bool, loops: &[Vec<(u32, bool)>], tol: Tol) -> Result<UvBox, &'static str> {
    let ws: Vec<Walk> = loops.iter().map(|l| walk(b, s, l, reversed, tol)).collect::<Result<_, _>>()?;
    let (u0, u1, full) = if ws.len() == 1 {
        if ws[0].wind != 0 {
            return Err("seamless periodic face");
        }
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for x in ws[0].u.iter().flatten() {
            lo = lo.min(*x);
            hi = hi.max(*x);
        }
        (lo, hi, false)
    } else if ws.len() == 2 && matches!(s, Surface::Cylinder { .. }) && ws.iter().all(|w| w.wind != 0) {
        // A seamless cylinder band: two circles, each winding once.
        let a = ws[0].u[0].unwrap_or(0.0);
        (a, a + TAU, true)
    } else {
        return Err("trimmed curved face");
    };
    if !(u1 - u0 > tol.ang) || u1 - u0 > TAU + tol.ang {
        return Err("trimmed curved face");
    }
    let (mut v0, mut v1) = (f64::INFINITY, f64::NEG_INFINITY);
    for w in &ws {
        for x in &w.v {
            v0 = v0.min(*x);
            v1 = v1.max(*x);
        }
    }
    if !(v1 > v0) {
        return Err("trimmed curved face");
    }
    // The metric of v (millimetres per unit v) for the on-boundary test.
    let mv = match s {
        Surface::Cone { k, .. } => libm::sqrt(1.0 + k * k),
        Surface::Sphere { r, .. } => *r,
        Surface::Torus { minor, .. } => *minor,
        _ => 1.0,
    };
    for w in &ws {
        for i in 0..w.v.len() {
            let on_v = libm::fabs(w.v[i] - v0) * mv <= tol.lin || libm::fabs(w.v[i] - v1) * mv <= tol.lin;
            let on_u = match w.u[i] {
                None => true,
                Some(x) => {
                    !full && (libm::fabs(x - u0) * w.rho[i] <= tol.lin || libm::fabs(x - u1) * w.rho[i] <= tol.lin)
                }
            };
            if !(on_u || on_v) {
                return Err("trimmed curved face");
            }
        }
    }
    Ok(UvBox { u0, u1, v0, v1 })
}
