//! The AP214 writer: brep_core `Brep`s into one Part 21 file, deterministic (the same Breps and Header write the
//! same bytes). Instances are numbered in a fixed order: the shared unit context and application context first; then
//! per Brep its arena in order (vertices, edges, faces with their loops, shells, solids), its
//! ADVANCED_BREP_SHAPE_REPRESENTATION and its product chain (PRODUCT ... SHAPE_DEFINITION_REPRESENTATION), so every
//! reference points backward.
//!
//! Lengths are written in millimetres (SI_UNIT(.MILLI.,.METRE.)), angles in radians; REALs in Rust's shortest
//! round-trip spelling, so a written coordinate reads back bit-exact. Every edge is written along its own curve
//! (EDGE_CURVE same_sense .T., vertices v[0] then v[1]); a co-edge's ORIENTED_EDGE orientation is `!reversed`, and a
//! face's same_sense is `!Face::reversed` (Part 42's parametrizations are brep_core's).

use crate::Refusal;
use crate::p21::{real, string};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use brep_core::dep::{Tol, Vec3};
use brep_core::{Brep, Curve, Frame, NurbsCurve, Surface};
use core::fmt::Write as _;

/// The HEADER section's text fields (the caller's: the crate has no clock, so the timestamp is given).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Header<'a> {
    /// FILE_NAME.name, also the PRODUCT and representation name.
    pub name: &'a str,
    /// FILE_NAME.time_stamp (ISO 8601).
    pub timestamp: &'a str,
    pub author: &'a str,
    pub organization: &'a str,
    pub originating_system: &'a str,
    /// FILE_DESCRIPTION.description.
    pub description: &'a str,
}

/// The AP214 IS schema identifier (ISO 10303-214, `automotive_design`).
const SCHEMA: &str = "AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }";

struct Out {
    s: String,
    n: u64,
}

impl Out {
    fn add(&mut self, body: &str) -> u64 {
        self.n += 1;
        let _ = writeln!(self.s, "#{}={};", self.n, body);
        self.n
    }
}

fn r(x: f64) -> Result<String, Refusal> {
    if x.is_finite() { Ok(real(x)) } else { Err(Refusal::Unwritable { what: "non-finite value" }) }
}

fn refs(ids: &[u64]) -> String {
    let mut s = String::from("(");
    for (k, id) in ids.iter().enumerate() {
        if k > 0 {
            s.push(',');
        }
        let _ = write!(s, "#{id}");
    }
    s.push(')');
    s
}

fn bool_(b: bool) -> &'static str {
    if b { ".T." } else { ".F." }
}

fn point(o: &mut Out, p: Vec3) -> Result<u64, Refusal> {
    let body = format!("CARTESIAN_POINT('',({},{},{}))", r(p.x)?, r(p.y)?, r(p.z)?);
    Ok(o.add(&body))
}

fn direction(o: &mut Out, d: Vec3) -> Result<u64, Refusal> {
    let body = format!("DIRECTION('',({},{},{}))", r(d.x)?, r(d.y)?, r(d.z)?);
    Ok(o.add(&body))
}

fn placement(o: &mut Out, f: &Frame) -> Result<u64, Refusal> {
    let p = point(o, f.o)?;
    let z = direction(o, f.z)?;
    let x = direction(o, f.x)?;
    Ok(o.add(&format!("AXIS2_PLACEMENT_3D('',#{p},#{z},#{x})")))
}

fn nurbs(o: &mut Out, n: &NurbsCurve) -> Result<u64, Refusal> {
    let mut pts = Vec::new();
    for c in n.ctrl() {
        pts.push(point(o, *c)?);
    }
    // The knot vector as distinct knots with multiplicities.
    let (mut mults, mut knots): (Vec<usize>, Vec<f64>) = (Vec::new(), Vec::new());
    for &k in n.knots() {
        match knots.last() {
            Some(&l) if l == k => *mults.last_mut().unwrap_or(&mut 0) += 1,
            _ => {
                knots.push(k);
                mults.push(1);
            }
        }
    }
    let mut ms = String::from("(");
    for (i, m) in mults.iter().enumerate() {
        let _ = write!(ms, "{}{m}", if i > 0 { "," } else { "" });
    }
    ms.push(')');
    let mut ks = String::from("(");
    for (i, k) in knots.iter().enumerate() {
        let _ = write!(ks, "{}{}", if i > 0 { "," } else { "" }, r(*k)?);
    }
    ks.push(')');
    let p = n.degree();
    let cl = refs(&pts);
    if n.weights().iter().all(|w| *w == 1.0) {
        return Ok(o.add(&format!("B_SPLINE_CURVE_WITH_KNOTS('',{p},{cl},.UNSPECIFIED.,.F.,.F.,{ms},{ks},.UNSPECIFIED.)")));
    }
    let mut ws = String::from("(");
    for (i, w) in n.weights().iter().enumerate() {
        let _ = write!(ws, "{}{}", if i > 0 { "," } else { "" }, r(*w)?);
    }
    ws.push(')');
    Ok(o.add(&format!(
        "( BOUNDED_CURVE() B_SPLINE_CURVE({p},{cl},.UNSPECIFIED.,.F.,.F.) B_SPLINE_CURVE_WITH_KNOTS({ms},{ks},.UNSPECIFIED.) \
         CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE({ws}) REPRESENTATION_ITEM('') )"
    )))
}

fn curve(o: &mut Out, c: &Curve) -> Result<u64, Refusal> {
    match c {
        Curve::Line { o: p, dir } => {
            let pt = point(o, *p)?;
            let d = direction(o, *dir)?;
            let v = o.add(&format!("VECTOR('',#{d},1.)"));
            Ok(o.add(&format!("LINE('',#{pt},#{v})")))
        }
        Curve::Circle { f, r: rad } => {
            let a = placement(o, f)?;
            let body = format!("CIRCLE('',#{a},{})", r(*rad)?);
            Ok(o.add(&body))
        }
        Curve::Ellipse { f, a, b } => {
            let pl = placement(o, f)?;
            let body = format!("ELLIPSE('',#{pl},{},{})", r(*a)?, r(*b)?);
            Ok(o.add(&body))
        }
        Curve::Nurbs(n) => nurbs(o, n),
    }
}

fn surface(o: &mut Out, s: &Surface) -> Result<u64, Refusal> {
    match s {
        Surface::Plane { f } => {
            let a = placement(o, f)?;
            Ok(o.add(&format!("PLANE('',#{a})")))
        }
        Surface::Cylinder { f, r: rad } => {
            let a = placement(o, f)?;
            let body = format!("CYLINDRICAL_SURFACE('',#{a},{})", r(*rad)?);
            Ok(o.add(&body))
        }
        Surface::Cone { f, r0, k } => {
            if *k == 0.0 {
                let a = placement(o, f)?;
                let body = format!("CYLINDRICAL_SURFACE('',#{a},{})", r(*r0)?);
                return Ok(o.add(&body));
            }
            // Part 42 wants a non-negative radius and a positive semi-angle: a narrowing cone is written in the frame
            // (x, -y, -z) (u -> -u, v -> -v: the same points and the same σ_u × σ_v), and a negative radius at the
            // placement moves the placement to the apex.
            let (mut f, mut r0, mut k) = (*f, *r0, *k);
            if k < 0.0 {
                f = Frame { o: f.o, x: f.x, y: -f.y, z: -f.z };
                k = -k;
            }
            if r0 < 0.0 {
                f = f.moved(f.o + f.z * (-r0 / k));
                r0 = 0.0;
            }
            let a = placement(o, &f)?;
            let body = format!("CONICAL_SURFACE('',#{a},{},{})", r(r0)?, r(libm::atan(k))?);
            Ok(o.add(&body))
        }
        Surface::Sphere { f, r: rad } => {
            let a = placement(o, f)?;
            let body = format!("SPHERICAL_SURFACE('',#{a},{})", r(*rad)?);
            Ok(o.add(&body))
        }
        Surface::Torus { f, major, minor } => {
            let a = placement(o, f)?;
            let body = format!("TOROIDAL_SURFACE('',#{a},{},{})", r(*major)?, r(*minor)?);
            Ok(o.add(&body))
        }
    }
}

/// One Brep's arena, then its shape representation; returns the representation's instance number.
fn brep(o: &mut Out, b: &Brep, name: &str, ctx: u64) -> Result<u64, Refusal> {
    if b.solids.is_empty() {
        return Err(Refusal::Unwritable { what: "brep with no solid" });
    }
    for s in &b.solids {
        match s.shells.len() {
            1 => {}
            0 => return Err(Refusal::Unwritable { what: "solid with no shell" }),
            _ => return Err(Refusal::Unwritable { what: "solid with a void shell" }),
        }
    }
    for (si, _) in b.shells.iter().enumerate() {
        if !b.solids.iter().any(|s| s.shells.contains(&(si as u32))) {
            return Err(Refusal::Unwritable { what: "shell outside every solid" });
        }
    }
    for fi in 0..b.faces.len() {
        if !b.shells.iter().any(|s| s.faces.contains(&(fi as u32))) {
            return Err(Refusal::Unwritable { what: "face outside every shell" });
        }
    }
    let mut vs = Vec::new();
    for v in &b.vertices {
        let p = point(o, v.p)?;
        vs.push(o.add(&format!("VERTEX_POINT('',#{p})")));
    }
    let mut es = Vec::new();
    for e in &b.edges {
        let (v0, v1) = (e.v[0] as usize, e.v[1] as usize);
        if v0 >= vs.len() || v1 >= vs.len() {
            return Err(Refusal::Unwritable { what: "edge vertex out of range" });
        }
        let c = curve(o, &e.curve)?;
        es.push(o.add(&format!("EDGE_CURVE('',#{},#{},#{c},.T.)", vs[v0], vs[v1])));
    }
    let mut fs = Vec::new();
    for (fi, f) in b.faces.iter().enumerate() {
        let s = surface(o, &f.surface)?;
        let mut bounds = Vec::new();
        for (k, lp) in b.face_loops(fi as u32).enumerate() {
            let Some(l) = b.loops.get(lp as usize) else { return Err(Refusal::Unwritable { what: "face loop out of range" }) };
            let mut oes = Vec::new();
            for &ce in &l.coedges {
                let Some(c) = b.coedges.get(ce as usize) else {
                    return Err(Refusal::Unwritable { what: "co-edge out of range" });
                };
                let Some(&e) = es.get(c.edge as usize) else { return Err(Refusal::Unwritable { what: "edge out of range" }) };
                oes.push(o.add(&format!("ORIENTED_EDGE('',*,*,#{e},{})", bool_(!c.reversed))));
            }
            let el = o.add(&format!("EDGE_LOOP('',{})", refs(&oes)));
            let kind = if k == 0 { "FACE_OUTER_BOUND" } else { "FACE_BOUND" };
            bounds.push(o.add(&format!("{kind}('',#{el},.T.)")));
        }
        fs.push(o.add(&format!("ADVANCED_FACE('',{},#{s},{})", refs(&bounds), bool_(!f.reversed))));
    }
    let mut shs = Vec::new();
    for sh in &b.shells {
        let ids: Vec<u64> = sh.faces.iter().map(|&f| fs[f as usize]).collect();
        shs.push(o.add(&format!("CLOSED_SHELL('',{})", refs(&ids))));
    }
    let mut msbs = Vec::new();
    for s in &b.solids {
        let Some(&sh) = shs.get(s.shells[0] as usize) else { return Err(Refusal::Unwritable { what: "shell out of range" }) };
        msbs.push(o.add(&format!("MANIFOLD_SOLID_BREP('',#{sh})")));
    }
    Ok(o.add(&format!("ADVANCED_BREP_SHAPE_REPRESENTATION({},{},#{ctx})", string(name), refs(&msbs))))
}

/// Write `breps` as one AP214 file: one ADVANCED_BREP_SHAPE_REPRESENTATION and one product per Brep, each solid a
/// MANIFOLD_SOLID_BREP over its CLOSED_SHELL. Refuses (named) a Brep it cannot write honestly: a non-finite value, a
/// solid with a void shell (BREP_WITH_VOIDS is VUG-STEP-VOIDS, owed), a shell or face outside every solid.
pub fn write(breps: &[Brep], header: &Header, tol: Tol) -> Result<String, Refusal> {
    let mut o = Out { s: String::new(), n: 0 };
    o.s.push_str("ISO-10303-21;\nHEADER;\n");
    let _ = writeln!(o.s, "FILE_DESCRIPTION(({}),'2;1');", string(header.description));
    let _ = writeln!(
        o.s,
        "FILE_NAME({},{},({}),({}),'step_core 0.1',{},'');",
        string(header.name),
        string(header.timestamp),
        string(header.author),
        string(header.organization),
        string(header.originating_system)
    );
    let _ = writeln!(o.s, "FILE_SCHEMA(({}));", string(SCHEMA));
    o.s.push_str("ENDSEC;\nDATA;\n");

    let mm = o.add("( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) )");
    let rad = o.add("( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) )");
    let sr = o.add("( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() )");
    let unc = o.add(&format!(
        "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE({}),#{mm},'distance_accuracy_value','confusion accuracy')",
        r(tol.lin)?
    ));
    let ctx = o.add(&format!(
        "( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc})) \
         GLOBAL_UNIT_ASSIGNED_CONTEXT((#{mm},#{rad},#{sr})) REPRESENTATION_CONTEXT('','') )"
    ));
    let app = o.add("APPLICATION_CONTEXT('core data for automotive mechanical design processes')");
    o.add(&format!("APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})"));

    for (i, b) in breps.iter().enumerate() {
        let name = if breps.len() == 1 { String::from(header.name) } else { format!("{} {}", header.name, i + 1) };
        let absr = brep(&mut o, b, &name, ctx)?;
        let n = string(&name);
        let pc = o.add(&format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
        let p = o.add(&format!("PRODUCT({n},{n},'',(#{pc}))"));
        o.add(&format!("PRODUCT_RELATED_PRODUCT_CATEGORY('part',$,(#{p}))"));
        let pdf = o.add(&format!("PRODUCT_DEFINITION_FORMATION('','',#{p})"));
        let pdc = o.add(&format!("PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"));
        let pd = o.add(&format!("PRODUCT_DEFINITION('design','',#{pdf},#{pdc})"));
        let pds = o.add(&format!("PRODUCT_DEFINITION_SHAPE('','',#{pd})"));
        o.add(&format!("SHAPE_DEFINITION_REPRESENTATION(#{pds},#{absr})"));
    }
    o.s.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    Ok(o.s)
}
