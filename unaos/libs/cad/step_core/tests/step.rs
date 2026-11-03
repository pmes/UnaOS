//! VUGSTEP (LEDGER SR482): the hand-written AP214 cube reads to a valid 1e9 mm³ Brep; an INCH file scales; brep_core
//! extrudes, revolves and primitives write -> read -> mass equal to 1e-9 relative; a cylinder round-trips its
//! CYLINDRICAL_SURFACE and two CIRCLE edges; the writer is deterministic; an unsupported entity refuses by name and
//! instance; a mangled file refuses by line and column.

use brep_core::dep::{Arc2, Edge as PEdge, EntityId, Loop as PLoop, Profile, Tol, Vec2, Vec3};
use brep_core::primitives::{box_, cone, cylinder, sphere, torus};
use brep_core::{Brep, Curve, Frame, OpKind, Role, extrude, properties, revolve, validate};
use std::f64::consts::{PI, TAU};
use step_core::{Header, Refusal, p21, read, write};

const T: Tol = Tol::DEFAULT;
const CUBE: &str = include_str!("fixtures/cube.step");

fn hdr() -> Header<'static> {
    Header {
        name: "part",
        timestamp: "2026-11-01T00:00:00",
        author: "Vug",
        organization: "UnaOS",
        originating_system: "step_core tests",
        description: "VUGSTEP round trip",
    }
}

fn rel(a: f64, b: f64) -> f64 {
    ((a - b) / b).abs()
}

fn lines(pts: &[(f64, f64)]) -> PLoop {
    let n = pts.len();
    let mut area = 0.0;
    let edges = (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            area += (a.0 * b.1 - b.0 * a.1) / 2.0;
            PEdge::Line { from: Vec2::new(a.0, a.1), to: Vec2::new(b.0, b.1), src: EntityId(i as u32) }
        })
        .collect();
    PLoop { edges, area }
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<(f64, f64)> {
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

fn xz() -> Frame {
    Frame::new(Vec3::ZERO, -Vec3::Y, Vec3::X).unwrap()
}

/// The 1-based line of the first line starting with `prefix`.
fn line_of(text: &str, prefix: &str) -> u32 {
    text.lines().position(|l| l.starts_with(prefix)).expect(prefix) as u32 + 1
}

/// write -> read -> validate, and the mass properties equal to 1e-9 relative (the centroid to 1e-9 of the size).
fn round_trip(b: &Brep, what: &str) -> Brep {
    validate(b, T).unwrap_or_else(|e| panic!("{what}: the source does not validate: {e:?}"));
    let text = write(std::slice::from_ref(b), &hdr(), T).unwrap_or_else(|e| panic!("{what}: write: {e:?}"));
    let back = read(&text, T).unwrap_or_else(|e| panic!("{what}: read: {e:?}"));
    assert_eq!(back.len(), 1, "{what}: one Brep");
    let r = &back[0];
    validate(r, T).unwrap_or_else(|e| panic!("{what}: the read Brep does not validate: {e:?}"));
    assert_eq!(
        (r.vertices.len(), r.edges.len(), r.faces.len(), r.loops.len()),
        (b.vertices.len(), b.edges.len(), b.faces.len(), b.loops.len()),
        "{what}: the arena counts"
    );
    let (m0, m1) = (properties(b, T).unwrap(), properties(r, T).unwrap());
    assert!(rel(m1.volume, m0.volume) < 1e-9, "{what}: volume {} vs {}", m1.volume, m0.volume);
    assert!(rel(m1.area, m0.area) < 1e-9, "{what}: area {} vs {}", m1.area, m0.area);
    let size = m0.volume.cbrt();
    assert!(m1.centroid.dist(m0.centroid) < 1e-9 * size, "{what}: centroid {:?} vs {:?}", m1.centroid, m0.centroid);
    back.into_iter().next().unwrap()
}

#[test]
fn hand_written_cube_reads_and_validates() {
    let bs = read(CUBE, T).unwrap();
    assert_eq!(bs.len(), 1);
    let b = &bs[0];
    let r = validate(b, T).unwrap();
    assert_eq!((r.vertices, r.edges, r.faces, r.genus.clone()), (8, 12, 6, vec![0]));
    assert!(rel(properties(b, T).unwrap().volume, 1e9) < 1e-12);
    assert!(b.edges.iter().all(|e| matches!(e.curve, Curve::Line { .. })));
    // The names carry the instance numbers: the vertices are #40..#47 in order, the first face is #100.
    let n = b.vertices[0].name;
    assert_eq!((n.op, n.role, n.a, n.b), (OpKind::Import, Role::Entity, 40, 0));
    assert_eq!(b.faces[0].name.a, 100);
    // The bottom face is against its plane (same_sense .F.).
    assert!(b.faces[0].reversed);
}

#[test]
fn an_inch_file_scales_by_25_4() {
    let inch = CUBE
        .replace(
            "#12=( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );",
            "#12=( CONVERSION_BASED_UNIT('INCH',#17) LENGTH_UNIT() NAMED_UNIT(#18) );\n\
             #17=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#19);\n\
             #18=DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.);\n\
             #19=( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );",
        )
        .replace("1000.", "1.");
    let bs = read(&inch, T).unwrap();
    validate(&bs[0], T).unwrap();
    let v = properties(&bs[0], T).unwrap().volume;
    assert!(rel(v, 25.4f64.powi(3)) < 1e-12, "{v}");
    // Metres through the SI prefix: no prefix is 1000 mm.
    let metre = CUBE.replace("SI_UNIT(.MILLI.,.METRE.) );\n#13", "SI_UNIT($,.METRE.) );\n#13").replace("1000.", "1.");
    let v = properties(&read(&metre, T).unwrap()[0], T).unwrap().volume;
    assert!(rel(v, 1e9) < 1e-12, "{v}");
}

#[test]
fn extruded_square_with_a_round_hole_round_trips() {
    let hole = PLoop {
        edges: vec![PEdge::Arc {
            arc: Arc2 { centre: Vec2::new(20.0, 20.0), radius: 5.0, start: 0.0, sweep: -TAU },
            src: EntityId(9),
        }],
        area: -PI * 25.0,
    };
    let p = Profile { outer: lines(&rect(0.0, 0.0, 40.0, 40.0)), holes: vec![] };
    round_trip(&extrude(&p, &Frame::WORLD, Vec3::Z, 25.0, T).unwrap(), "extruded square");
    let p = Profile { outer: lines(&rect(0.0, 0.0, 40.0, 40.0)), holes: vec![hole] };
    round_trip(&extrude(&p, &Frame::WORLD, Vec3::Z, 25.0, T).unwrap(), "extruded square with a hole");
}

#[test]
fn revolved_rectangles_round_trip() {
    let p = Profile { outer: lines(&rect(10.0, 0.0, 20.0, 30.0)), holes: vec![] };
    for angle in [TAU, 2.0, 4.5] {
        let b = revolve(&p, &xz(), Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), angle, T).unwrap();
        round_trip(&b, &format!("revolve {angle}"));
    }
}

#[test]
fn a_cylinder_round_trips_its_surface_and_two_circles() {
    let b = cylinder(12.0, 30.0, T).unwrap();
    let text = write(std::slice::from_ref(&b), &hdr(), T).unwrap();
    assert_eq!(text.matches("CYLINDRICAL_SURFACE(").count(), 1, "{text}");
    assert_eq!(text.matches("CIRCLE(").count(), 2, "{text}");
    let r = round_trip(&b, "cylinder");
    let circles = r.edges.iter().filter(|e| matches!(e.curve, Curve::Circle { .. })).count();
    let lines = r.edges.iter().filter(|e| matches!(e.curve, Curve::Line { .. })).count();
    assert_eq!((circles, lines), (2, 1));
}

#[test]
fn cone_sphere_and_torus_round_trip() {
    round_trip(&cone(10.0, 0.0, 20.0, T).unwrap(), "pointed cone");
    round_trip(&cone(10.0, 4.0, 20.0, T).unwrap(), "frustum");
    round_trip(&sphere(7.0, T).unwrap(), "sphere");
    round_trip(&torus(20.0, 5.0, T).unwrap(), "torus");
}

#[test]
fn the_writer_is_deterministic() {
    let b = box_(10.0, 20.0, 30.0, T).unwrap();
    let a = write(std::slice::from_ref(&b), &hdr(), T).unwrap();
    assert_eq!(a, write(std::slice::from_ref(&b), &hdr(), T).unwrap());
    // Read back into the same arena order, the box writes the same bytes again.
    let again = write(&read(&a, T).unwrap(), &hdr(), T).unwrap();
    assert_eq!(a, again);
}

#[test]
fn a_b_spline_surface_refuses_by_instance() {
    let bad = CUBE.replace(
        "#117=PLANE('',#118);",
        "#117=B_SPLINE_SURFACE_WITH_KNOTS('',1,1,((#34,#35),(#37,#36)),.UNSPECIFIED.,.F.,.F.,.F.,(2),(2),(0.),(1.),\
         .UNSPECIFIED.);",
    );
    assert_eq!(
        read(&bad, T),
        Err(Refusal::Unsupported { entity: "B_SPLINE_SURFACE_WITH_KNOTS".into(), instance: 117 })
    );
}

#[test]
fn mangled_files_refuse_by_line_and_column() {
    let head = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('','',(''),(''),'','','');\n\
                FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n";
    let tail = "ENDSEC;\nEND-ISO-10303-21;\n";
    // An unterminated string: the apostrophe at line 8, column 12.
    let t = format!("{head}#1=PRODUCT('cube);\n{tail}");
    assert_eq!(read(&t, T), Err(Refusal::UnterminatedString { line: 8, col: 12 }));
    // A reference to an undefined instance: #999 at line 8, column 23.
    let t = format!("{head}#1=PRODUCT_CONTEXT('',#999,'mechanical');\n{tail}");
    assert_eq!(read(&t, T), Err(Refusal::Undefined { instance: 999, line: 8, col: 23 }));
    // A missing ';' after an instance.
    let t = format!("{head}#1=PRODUCT_CONTEXT('',$,'mechanical')\n{tail}");
    assert_eq!(read(&t, T), Err(Refusal::Syntax { line: 9, col: 1, what: "';'" }));
    // A CARTESIAN_POINT with one parameter.
    let bad = CUBE.replace("#31=CARTESIAN_POINT('P2',(1000.,0.,0.));", "#31=CARTESIAN_POINT((1000.,0.,0.));");
    assert_eq!(
        read(&bad, T),
        Err(Refusal::ParamCount {
            entity: "CARTESIAN_POINT".into(),
            instance: 31,
            line: line_of(&bad, "#31="),
            col: 1,
            expected: 2,
            found: 1
        })
    );
    // An entity name nobody defines, where a face belongs.
    let bad = CUBE.replace("#120=ADVANCED_FACE(", "#120=FANCY_FACE(");
    assert_eq!(
        read(&bad, T),
        Err(Refusal::UnknownEntity { entity: "FANCY_FACE".into(), instance: 120, line: line_of(&bad, "#120="), col: 1 })
    );
    // No shape representation at all.
    let t = format!("{head}#1=APPLICATION_CONTEXT('x');\n{tail}");
    assert_eq!(read(&t, T), Err(Refusal::NoShape));
}

#[test]
fn string_escapes_round_trip() {
    assert_eq!(p21::string("W\u{fc}rfel 'A' \\ \u{1F600}"), "'W\\X2\\00FC\\X0\\rfel ''A'' \\\\ \\X4\\0001F600\\X0\\'");
    assert_eq!(p21::real(1000.0), "1000.0");
    assert_eq!(p21::real(1e-7), "1.E-7");
    let b = box_(1.0, 1.0, 1.0, T).unwrap();
    let h = Header { name: "W\u{fc}rfel", ..hdr() };
    let text = write(std::slice::from_ref(&b), &h, T).unwrap();
    let t = p21::parse(&text).unwrap();
    let prod = t.data.values().find(|i| i.is("PRODUCT")).unwrap();
    assert_eq!(prod.record("PRODUCT").unwrap().params[0], p21::Param::Str("W\u{fc}rfel".into()));
    // \S\ and \X\ decode through code page A (ISO 8859-1).
    let s = text.replace("FILE_DESCRIPTION(('VUGSTEP round trip')", "FILE_DESCRIPTION(('\\S\\d\\X\\E9')");
    let t = p21::parse(&s).unwrap();
    assert_eq!(t.header("FILE_DESCRIPTION").unwrap().params[0], p21::Param::List(vec![p21::Param::Str("\u{e4}\u{e9}".into())]));
}
