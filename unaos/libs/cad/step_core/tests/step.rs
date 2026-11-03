//! VUGSTEP (LEDGER SR482): the hand-written AP214 cube reads to a valid 1e9 mm³ Brep; an INCH file scales; brep_core
//! extrudes, revolves and primitives write -> read -> mass equal to 1e-9 relative; a cylinder round-trips its
//! CYLINDRICAL_SURFACE and two CIRCLE edges; the writer is deterministic; an unsupported entity refuses by name and
//! instance; a mangled file refuses by line and column.

use brep_core::sketch_core::{Edge as PEdge, Loop as PLoop};
use brep_core::{Arc2, EntityId, Profile, Tol, Vec2, Vec3};
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

// VUGBREP3TORUSSTEP (SR530): the torus fillet (VUGBREP3TORUS SR519) and the cone chamfer of a boss / hole rim meet
// STEP. write -> read keeps validate, the genus, the arena counts, the mass (`round_trip`), every vertex and edge, and per
// face its surface (radii, frame) and uv spans; the Names (Import / Entity), the face order and a whole turn of a
// parameter start (the seam's placement in u, a torus's v) may change.

use brep_core::{Id, Surface, chamfer, difference, fillet, union};
use std::collections::{BTreeMap, BTreeSet};

/// An angle start compared modulo TAU: T.lin per millimetre of radius (every radius here is 2 mm or more).
const ANG: f64 = 1e-6;

/// The radius `r` cylinder from `at` up `h` along +z: `primitives::cylinder`'s own revolve, placed by its frame.
fn placed_cylinder(r: f64, h: f64, at: Vec3) -> Brep {
    let p = Profile { outer: lines(&rect(0.0, 0.0, r, h)), holes: vec![] };
    let f = Frame::new(at, -Vec3::Y, Vec3::X).unwrap();
    revolve(&p, &f, Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), TAU, T).unwrap()
}

/// brep_core tests/fillet.rs `boss`: the R 10 cylinder from z 5 to 30 at (20, 20) united with the 40 x 40 x 10 plate.
fn boss() -> Brep {
    let tool = placed_cylinder(10.0, 25.0, Vec3::new(20.0, 20.0, 5.0));
    union(&box_(40.0, 40.0, 10.0, T).unwrap(), &tool, T).expect("union")
}

/// brep_core tests/fillet.rs `holed`: the 40 x 40 x 10 plate with the R 10 through hole at (20, 20).
fn holed() -> Brep {
    let tool = placed_cylinder(10.0, 12.0, Vec3::new(20.0, 20.0, -1.0));
    difference(&box_(40.0, 40.0, 10.0, T).unwrap(), &tool, T).expect("difference")
}

/// The Id of the one radius-`r` circle edge at height `z`.
fn rim(b: &Brep, z: f64, r: f64) -> Id {
    let hits: Vec<Id> = b
        .edges
        .iter()
        .filter(|e| matches!(e.curve, Curve::Circle { r: cr, .. } if (cr - r).abs() < 1e-9))
        .filter(|e| (b.vertices[e.v[0] as usize].p.z - z).abs() < 1e-9)
        .map(|e| e.id)
        .collect();
    assert_eq!(hits.len(), 1, "one radius {r} circle edge at z {z}");
    hits[0]
}

/// The spandrel (the r x r corner square less the quarter disc): its area and its centroid's distance from the corner.
fn spandrel(r: f64) -> (f64, f64) {
    ((1.0 - PI / 4.0) * r * r, r * (10.0 - 3.0 * PI) / (12.0 - 3.0 * PI))
}

fn same_mod_tau(a: f64, b: f64, eps: f64) -> bool {
    let d = (a - b) / TAU;
    ((d - d.round()) * TAU).abs() <= eps
}

fn near12(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * a.abs().max(1.0)
}

fn frame_eq(a: &Frame, b: &Frame) -> bool {
    a.o.dist(b.o) < 1e-9 && a.z.dot(b.z) > 1.0 - 1e-12 && a.x.dot(b.x) > 1.0 - 1e-12
}

fn surface_eq(a: &Surface, b: &Surface) -> bool {
    match (a, b) {
        (Surface::Plane { f: fa }, Surface::Plane { f: fb }) => frame_eq(fa, fb),
        (Surface::Cylinder { f: fa, r: ra }, Surface::Cylinder { f: fb, r: rb }) => frame_eq(fa, fb) && near12(*ra, *rb),
        (Surface::Cone { f: fa, r0: ra, k: ka }, Surface::Cone { f: fb, r0: rb, k: kb }) => {
            frame_eq(fa, fb) && near12(*ra, *rb) && near12(*ka, *kb)
        }
        (Surface::Sphere { f: fa, r: ra }, Surface::Sphere { f: fb, r: rb }) => frame_eq(fa, fb) && near12(*ra, *rb),
        (Surface::Torus { f: fa, major: ma, minor: na }, Surface::Torus { f: fb, major: mb, minor: nb }) => {
            frame_eq(fa, fb) && near12(*ma, *mb) && near12(*na, *nb)
        }
        _ => false,
    }
}

/// The edge indices a face's loops use (faces mint in shell order, edges in arena order: this matches a face across
/// the round trip).
fn face_key(b: &Brep, fi: usize) -> BTreeSet<u32> {
    let mut k = BTreeSet::new();
    for l in b.face_loops(fi as u32) {
        for &c in &b.loops[l as usize].coedges {
            k.insert(b.coedges[c as usize].edge);
        }
    }
    k
}

/// The measured list beyond `round_trip`: the genus, every vertex and edge by arena index, every face (matched by its
/// edges) with its `reversed`, surface and uv spans; a parameter start may move by a whole turn.
fn compare(src: &Brep, back: &Brep, what: &str) {
    assert_eq!(validate(src, T).unwrap().genus, validate(back, T).unwrap().genus, "{what}: genus");
    for (i, (a, b)) in src.vertices.iter().zip(&back.vertices).enumerate() {
        assert!(a.p.dist(b.p) < 1e-9, "{what}: vertex {i} {:?} vs {:?}", a.p, b.p);
    }
    for (i, (a, b)) in src.edges.iter().zip(&back.edges).enumerate() {
        assert_eq!(a.v, b.v, "{what}: edge {i} vertices");
        let (sa, sb) = (a.t1 - a.t0, b.t1 - b.t0);
        assert!((sa - sb).abs() < 1e-9, "{what}: edge {i} span {sa} vs {sb}");
        match (&a.curve, &b.curve) {
            (Curve::Line { o: oa, dir: da }, Curve::Line { o: ob, dir: db }) => {
                assert!(oa.dist(*ob) < 1e-9 && da.dot(*db) > 1.0 - 1e-12, "{what}: edge {i} line");
            }
            (Curve::Circle { f: fa, r: ra }, Curve::Circle { f: fb, r: rb }) => {
                assert!(near12(*ra, *rb), "{what}: edge {i} radius {ra} vs {rb}");
                assert!(fa.o.dist(fb.o) < 1e-9 && fa.z.dot(fb.z) > 1.0 - 1e-12, "{what}: edge {i} circle frame");
                assert!(same_mod_tau(a.t0, b.t0, T.lin / ra), "{what}: edge {i} start {} vs {}", a.t0, b.t0);
            }
            (x, y) => panic!("{what}: edge {i} curve {x:?} vs {y:?}"),
        }
    }
    let mut by_key: BTreeMap<BTreeSet<u32>, usize> = BTreeMap::new();
    for fi in 0..back.faces.len() {
        assert!(by_key.insert(face_key(back, fi), fi).is_none(), "{what}: read face {fi} shares its edges");
    }
    for (fi, f) in src.faces.iter().enumerate() {
        let k = face_key(src, fi);
        let Some(&bi) = by_key.get(&k) else { panic!("{what}: no read face over the edges {k:?} of face {fi}") };
        let g = &back.faces[bi];
        assert_eq!(f.reversed, g.reversed, "{what}: face {fi} reversed");
        assert!(surface_eq(&f.surface, &g.surface), "{what}: face {fi} surface {:?} vs {:?}", f.surface, g.surface);
        match (f.uv, g.uv) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                assert!(((a.u1 - a.u0) - (b.u1 - b.u0)).abs() < 1e-9, "{what}: face {fi} u span {a:?} vs {b:?}");
                assert!(((a.v1 - a.v0) - (b.v1 - b.v0)).abs() < 1e-9, "{what}: face {fi} v span {a:?} vs {b:?}");
                assert!(same_mod_tau(a.u0, b.u0, ANG), "{what}: face {fi} u0 {a:?} vs {b:?}");
                let v0 = if matches!(f.surface, Surface::Torus { .. }) {
                    same_mod_tau(a.v0, b.v0, ANG)
                } else {
                    (a.v0 - b.v0).abs() < 1e-9
                };
                assert!(v0, "{what}: face {fi} v0 {a:?} vs {b:?}");
            }
            (a, b) => panic!("{what}: face {fi} uv {a:?} vs {b:?}"),
        }
    }
}

/// write (exactly one `entity` in the text) -> read -> `round_trip` -> `compare`; the read Brep.
fn blend_round_trip(src: &Brep, entity: &str, what: &str) -> Brep {
    let text = write(std::slice::from_ref(src), &hdr(), T).unwrap_or_else(|e| panic!("{what}: write: {e:?}"));
    assert_eq!(text.matches(entity).count(), 1, "{what}: one {entity}");
    let back = round_trip(src, what);
    compare(src, &back, what);
    back
}

/// The one face of `b` on a surface `pick` accepts; its Name is the import's (the blend's Fillet / Chamfer Name is a
/// legitimate change).
fn the_face<'a>(b: &'a Brep, pick: impl Fn(&Surface) -> bool, what: &str) -> &'a brep_core::Face {
    let hits: Vec<&brep_core::Face> = b.faces.iter().filter(|f| pick(&f.surface)).collect();
    assert_eq!(hits.len(), 1, "{what}: one blend face");
    assert_eq!((hits[0].name.op, hits[0].name.role), (OpKind::Import, Role::Entity), "{what}: the import's Name");
    hits[0]
}

fn torus_check(b: &Brep, major: f64, minor: f64, reversed: bool, what: &str) {
    let f = the_face(b, |s| matches!(s, Surface::Torus { .. }), what);
    let Surface::Torus { major: m, minor: n, .. } = f.surface else { unreachable!() };
    assert!(near12(m, major) && near12(n, minor), "{what}: major {m} minor {n}");
    assert_eq!(f.reversed, reversed, "{what}: reversed");
    let uv = f.uv.expect("a torus uv box");
    assert!((uv.v1 - uv.v0 - PI / 2.0).abs() < 1e-9, "{what}: the quarter {uv:?}");
}

#[test]
fn filleted_boss_rims_round_trip_their_torus_quarters() {
    let (area, d) = spandrel(2.0);
    let a = boss();
    // (rim height, -1 convex / +1 concave, major, reversed)
    for (z, s, major, reversed, what) in [(30.0, -1.0, 8.0, false, "boss top rim"), (10.0, 1.0, 12.0, true, "boss base rim")] {
        let r = fillet(&a, &[rim(&a, z, 10.0)], 2.0, T).expect(what);
        let back = blend_round_trip(&r, "TOROIDAL_SURFACE(", what);
        torus_check(&back, major, 2.0, reversed, what);
        let want = 16000.0 + 2000.0 * PI + s * 2.0 * PI * (10.0 + s * d) * area;
        assert!(rel(properties(&back, T).unwrap().volume, want) < 1e-6, "{what}: Pappus");
    }
}

#[test]
fn filleted_hole_rim_round_trips_the_outer_torus() {
    let (area, d) = spandrel(2.0);
    let a = holed();
    let r = fillet(&a, &[rim(&a, 10.0, 10.0)], 2.0, T).expect("fillet");
    let back = blend_round_trip(&r, "TOROIDAL_SURFACE(", "hole rim");
    torus_check(&back, 12.0, 2.0, false, "hole rim");
    assert_eq!(validate(&back, T).unwrap().genus, vec![1]);
    let want = 16000.0 - 1000.0 * PI - 2.0 * PI * (10.0 + d) * area;
    assert!(rel(properties(&back, T).unwrap().volume, want) < 1e-6, "hole rim: Pappus");
}

#[test]
fn chamfered_boss_rim_round_trips_its_cone_frustum() {
    let a = boss();
    let (d1, d2) = (2.0, 3.0);
    let r = chamfer(&a, &[rim(&a, 30.0, 10.0)], d1, d2, T).expect("chamfer");
    let back = blend_round_trip(&r, "CONICAL_SURFACE(", "boss rim chamfer");
    let f = the_face(&back, |s| matches!(s, Surface::Cone { .. }), "boss rim chamfer");
    let Surface::Cone { r0, k, .. } = f.surface else { unreachable!() };
    assert!(near12(r0, 8.0) && near12(k, 2.0 / 3.0), "r0 {r0} k {k}");
    let uv = f.uv.expect("a cone uv box");
    assert!((uv.v1 - uv.v0 - d2).abs() < 1e-9, "the frustum {uv:?}");
    let want = 16000.0 + 2000.0 * PI - 2.0 * PI * (10.0 - d1 / 3.0) * (d1 * d2 / 2.0);
    assert!(rel(properties(&back, T).unwrap().volume, want) < 1e-6, "boss rim chamfer: Pappus");
}
