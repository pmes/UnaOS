//! VUGINTENTSCAN (LEDGER SR508): a proposal verified against scan_core's shapes (copied as `ShapeRef`s), the
//! measurement table Vein reads, the ISO 262 / 273 hole sizes, and the offline proposer whose every proposal is
//! disposed by `verify_all` (BP-vug-7).

use brep_core::Vec3;
use doc_core::{
    Class, Doc, Measurement, Proposal, Reason, ShapeParams, ShapeRef, Verdict, clearance_for, measurements_text,
    nearest_standard, read, replay, suggest, verify, verify_all,
};
use doc_core::{Lib, Stmt, Term, apply, apply_with, read_with}; // VUGINTENTSETORDER (SR551); VUGMAKEMACHINELIBHELD (SR556): What no longer used

const SIGMA: f64 = 0.3;
const SIGMA_DEG: f64 = 0.5;

/// A 120 x 120 x 10 plate with an r 39.74 hole at its centre, drawn at the measured values.
const HOLE_PLATE: &str = "vug-doc version=1 lin=1e-6 ang=1e-9
sketch s o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s x=0 y=0
point p2 sketch=s x=120 y=0
point p3 sketch=s x=120 y=120
point p4 sketch=s x=0 y=120
line l1 sketch=s a=p1 b=p2
line l2 sketch=s a=p2 b=p3
line l3 sketch=s a=p3 b=p4
line l4 sketch=s a=p4 b=p1
point pc sketch=s x=60 y=60
circle c1 sketch=s centre=pc r=39.74
extrude e1 sketch=s outer=l1 dir=[0 0 1] length=10
";

fn v(x: f64, y: f64, z: f64) -> Vec3 {
    Vec3::new(x, y, z)
}

fn plane(n: Vec3, d: f64, support: u32, rms_mm: f64) -> ShapeRef {
    ShapeRef { params: ShapeParams::Plane { normal: n, d }, support, rms_mm, extent: None }
}

fn cyl(p: Vec3, r: f64, support: u32, rms_mm: f64) -> ShapeRef {
    ShapeRef { params: ShapeParams::Cylinder { point: p, axis: v(0.0, 0.0, 1.0), radius: r }, support, rms_mm, extent: None }
}

/// The SR497 fixture's values: the top plane and the r 39.74 cylinder.
fn fixture_shapes() -> Vec<ShapeRef> {
    vec![plane(v(0.0, 0.0, 1.0), 10.0, 5000, 0.4), cyl(v(60.0, 60.0, 5.0), 39.74, 1200, 0.6)]
}

fn shapes(s: Vec<ShapeRef>) -> Measurement {
    Measurement::Shapes { shapes: s, sigma_mm: SIGMA, sigma_deg: SIGMA_DEG }
}

fn p(s: &str) -> Proposal {
    Proposal { stmt: s.into(), confidence: 0.9, why: "test".into(), shapes: Vec::new() }
}

fn doc(text: &str) -> Doc {
    let d = read(text).unwrap();
    let r = replay(&d, d.tol);
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    assert_eq!(r.solids.len(), 1);
    d
}

#[test]
fn shapes_accept_the_round_diameter_and_refuse_a_wrong_one() {
    let d = doc(HOLE_PLATE);
    let m = shapes(fixture_shapes());
    match verify(&d, &p("diameter i1 sketch=s circle=c1 d=80"), &m, d.tol) {
        Verdict::Accept { max_mm, residual_sigma, .. } => {
            assert!((max_mm - 0.26).abs() < 1e-6, "max {}", max_mm);
            assert!(residual_sigma < 3.0);
        }
        v => panic!("d=80 refused: {:?}", v),
    }
    match verify(&d, &p("diameter i1 sketch=s circle=c1 d=70"), &m, d.tol) {
        Verdict::Refuse { why: Reason::Residual { max_mm, limit_mm, .. } } => {
            assert!((max_mm - 4.74).abs() < 1e-6, "max {}", max_mm);
            assert!((limit_mm - 0.9).abs() < 1e-12);
        }
        v => panic!("d=70 not a Residual refusal: {:?}", v),
    }
}

/// VUG-INTENT-SCAN-UNMATCHED (SR515): a shape with no face of its kind (a TSDF-rounded corner, an open rim) is left out
/// of the residual and reported before the proposals, never a refusal of the batch.
#[test]
fn a_shape_with_no_face_of_its_kind_is_reported_not_refused() {
    let d = doc(HOLE_PLATE);
    let mut s = fixture_shapes();
    s.push(ShapeRef { params: ShapeParams::Sphere { centre: v(60.0, 60.0, 30.0), radius: 5.0 }, support: 300, rms_mm: 0.2, extent: None });
    let m = shapes(s);
    match verify(&d, &p("diameter i1 sketch=s circle=c1 d=80"), &m, d.tol) {
        Verdict::Accept { max_mm, .. } => assert!((max_mm - 0.26).abs() < 1e-6, "max {}", max_mm),
        v => panic!("refused over an unmatched shape: {:?}", v),
    }
    let b = verify_all(&d, &[p("diameter i1 sketch=s circle=c1 d=80")], &m, d.tol);
    assert_eq!(b.unmatched.len(), 1);
    assert_eq!(b.unmatched[0].0, 2);
    let t = b.text();
    assert!(t.starts_with("intent: proposed=1 accepted=1 refused=0\nunmatched sphere #2 support=300 rms=0.2\naccept "), "{}", t);
    // Every shape matched: no unmatched line.
    let b = verify_all(&d, &[p("diameter i1 sketch=s circle=c1 d=80")], &shapes(fixture_shapes()), d.tol);
    assert!(b.unmatched.is_empty() && !b.text().contains("unmatched"), "{}", b.text());
    // A non-positive angular sigma is a bad measurement.
    let bad = Measurement::Shapes { shapes: fixture_shapes(), sigma_mm: SIGMA, sigma_deg: 0.0 };
    assert_eq!(
        verify(&d, &p("diameter i1 sketch=s circle=c1 d=80"), &bad, d.tol),
        Verdict::Refuse { why: Reason::BadMeasurement }
    );
}

/// No measured shape has a face of its kind: the proposal cannot be judged and is refused by name (the first shape's).
#[test]
fn no_shape_with_a_face_of_its_kind_is_refused_by_name() {
    let d = doc(HOLE_PLATE);
    let only = vec![ShapeRef { params: ShapeParams::Sphere { centre: v(60.0, 60.0, 30.0), radius: 5.0 }, support: 300, rms_mm: 0.2, extent: None }];
    let why = match verify(&d, &p("diameter i1 sketch=s circle=c1 d=80"), &shapes(only), d.tol) {
        Verdict::Refuse { why } => why,
        v => panic!("accepted: {:?}", v),
    };
    assert_eq!(why, Reason::UnmatchedShape { index: 0, kind: "sphere" });
    assert_eq!(why.text(), "unmatched-shape sphere #0");
}

#[test]
fn the_measurement_table_is_deterministic() {
    let m = shapes(fixture_shapes());
    let t = measurements_text(&m);
    assert_eq!(
        t,
        "shapes: n=2 sigma=0.3 sigma_deg=0.5\n\
         plane n=(0,0,1) d=10 support=5000 rms=0.4\n\
         cylinder axis=(60,60,5) dir=(0,0,1) r=39.74 support=1200 rms=0.6\n"
    );
    assert_eq!(t, measurements_text(&m.clone()));
    let cone = Measurement::Shapes {
        shapes: vec![ShapeRef {
            params: ShapeParams::Cone { apex: v(0.0, 0.0, -0.0), axis: v(0.0, 0.0, 1.0), half_angle: core::f64::consts::FRAC_PI_4 },
            support: 10,
            rms_mm: 0.05,
            extent: None,
        }],
        sigma_mm: 0.25,
        sigma_deg: 1.0,
    };
    assert_eq!(
        measurements_text(&cone),
        "shapes: n=1 sigma=0.25 sigma_deg=1\ncone apex=(0,0,0) dir=(0,0,1) half_angle_deg=45 support=10 rms=0.05\n"
    );
}

/// The 49.7 x 30.2 x 6 plate with two 6.4 holes, drawn at the measured values, and its scan's shapes.
const HOLES: &str = "vug-doc version=1 lin=1e-6 ang=1e-9
sketch s o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s x=0 y=0
point p2 sketch=s x=49.7 y=0
point p3 sketch=s x=49.7 y=30.2
point p4 sketch=s x=0 y=30.2
line l1 sketch=s a=p1 b=p2
line l2 sketch=s a=p2 b=p3
line l3 sketch=s a=p3 b=p4
line l4 sketch=s a=p4 b=p1
point h1 sketch=s x=10 y=15.1
point h2 sketch=s x=39.7 y=15.1
circle k1 sketch=s centre=h1 r=3.2
circle k2 sketch=s centre=h2 r=3.2
extrude e1 sketch=s outer=l1 dir=[0 0 1] length=6
";

fn box_planes(w: f64, h: f64, t: f64) -> Vec<ShapeRef> {
    vec![
        plane(v(0.0, 0.0, 1.0), 0.0, 4000, 0.2),
        plane(v(0.0, 0.0, 1.0), t, 4000, 0.2),
        plane(v(1.0, 0.0, 0.0), 0.0, 900, 0.2),
        plane(v(1.0, 0.0, 0.0), w, 900, 0.2),
        plane(v(0.0, 1.0, 0.0), 0.0, 900, 0.2),
        plane(v(0.0, 1.0, 0.0), h, 900, 0.2),
    ]
}

fn stmts(ps: &[Proposal]) -> Vec<&str> {
    ps.iter().map(|p| p.stmt.as_str()).collect()
}

#[test]
fn suggest_reads_the_plate_and_every_proposal_is_accepted() {
    let d = doc(HOLES);
    let mut s = box_planes(49.7, 30.2, 6.0);
    s.push(cyl(v(10.0, 15.1, 3.0), 3.2, 400, 0.3));
    s.push(cyl(v(39.7, 15.1, 3.0), 3.2, 400, 0.3));
    let m = shapes(s);
    let ps = suggest(&d, &m);
    assert_eq!(
        stmts(&ps),
        [
            "horizontal i1 sketch=s line=l1",
            "vertical i2 sketch=s line=l2",
            "horizontal i3 sketch=s line=l3",
            "vertical i4 sketch=s line=l4",
            "distance i5 sketch=s a=p1 b=p2 d=50",
            "distance i6 sketch=s a=p2 b=p3 d=30",
            "diameter i7 sketch=s circle=k1 d=6.6",
            "equal_radius i8 sketch=s a=k1 b=k2",
            // VUGINTENTPOSITIONS (SR527): the holes mirror about the l2 / l4 midline; h1 sits on the datum grid.
            "point i9 sketch=s x=24.85 y=0",
            "midpoint i10 sketch=s p=i9 line=l1",
            "point i11 sketch=s x=24.85 y=30.2",
            "midpoint i12 sketch=s p=i11 line=l3",
            "line i13 sketch=s a=i9 b=i11",
            "symmetric i14 sketch=s a=h1 b=h2 line=i13",
            "distance_point_line i15 sketch=s p=h1 line=l4 d=10",
            "distance_point_line i16 sketch=s p=h1 line=l1 d=15",
        ]
    );
    assert!(ps[6].why.contains("ISO 273 medium clearance for M6"), "{}", ps[6].why);
    assert!(ps.iter().all(|p| (0.0..=1.0).contains(&p.confidence)));
    assert!((ps[6].confidence - (1.0 - 0.2 / SIGMA)).abs() < 1e-9);
    let b = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(b.accepted(), ps.len(), "{}", b.text());
}

/// A 100 x 100 x 8 plate with four 6.4 holes on a measured r 20.05 circle about (50, 50).
const BOLTS: &str = "vug-doc version=1 lin=1e-6 ang=1e-9
sketch s o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s x=0 y=0
point p2 sketch=s x=100 y=0
point p3 sketch=s x=100 y=100
point p4 sketch=s x=0 y=100
line l1 sketch=s a=p1 b=p2
line l2 sketch=s a=p2 b=p3
line l3 sketch=s a=p3 b=p4
line l4 sketch=s a=p4 b=p1
point h1 sketch=s x=70.05 y=50
point h2 sketch=s x=50 y=70.05
point h3 sketch=s x=29.95 y=50
point h4 sketch=s x=50 y=29.95
circle k1 sketch=s centre=h1 r=3.2
circle k2 sketch=s centre=h2 r=3.2
circle k3 sketch=s centre=h3 r=3.2
circle k4 sketch=s centre=h4 r=3.2
extrude e1 sketch=s outer=l1 dir=[0 0 1] length=8
";

#[test]
fn a_bolt_circle_is_one_statement_set_and_accepted() {
    let d = doc(BOLTS);
    let mut s = box_planes(100.0, 100.0, 8.0);
    for c in [(70.05, 50.0), (50.0, 70.05), (29.95, 50.0), (50.0, 29.95)] {
        s.push(cyl(v(c.0, c.1, 4.0), 3.2, 400, 0.3));
    }
    let m = shapes(s);
    let ps = suggest(&d, &m);
    let set: Vec<&Proposal> = ps.iter().filter(|p| p.why.starts_with("bolt-circle")).collect();
    assert_eq!(set.len(), 5, "{:?}", stmts(&ps));
    assert!(set.iter().all(|p| p.why == set[0].why), "one set shares one why");
    assert!(set[0].why.starts_with("bolt-circle n=4 r=20"), "{}", set[0].why);
    assert!(set[0].stmt.starts_with("point "), "{}", set[0].stmt);
    let pc = set[0].stmt.split(' ').nth(1).unwrap();
    for (k, h) in set[1..].iter().zip(["h1", "h2", "h3", "h4"]) {
        assert!(k.stmt.starts_with("distance "), "{}", k.stmt);
        assert!(k.stmt.ends_with(&format!("a={} b={} d=20", pc, h)), "{}", k.stmt);
    }
    // The bolt set is contiguous and last.
    let first = ps.iter().position(|p| p.why.starts_with("bolt-circle")).unwrap();
    assert_eq!(first + 5, ps.len());
    let b = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(b.accepted(), ps.len(), "{}", b.text());
}

// ---- VUGINTENTPOSITIONS (SR527): round hole positions, edge distances, symmetry about the midline ----

/// A 50 x 30 x 6 plate drawn at round values with two r 3.2 holes measured at (12.4, 10.1) and (37.6, 10.1).
const POSITIONS: &str = "vug-doc version=1 lin=1e-6 ang=1e-9
sketch s o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s x=0 y=0
point p2 sketch=s x=50 y=0
point p3 sketch=s x=50 y=30
point p4 sketch=s x=0 y=30
line l1 sketch=s a=p1 b=p2
line l2 sketch=s a=p2 b=p3
line l3 sketch=s a=p3 b=p4
line l4 sketch=s a=p4 b=p1
point h1 sketch=s x=12.4 y=10.1
point h2 sketch=s x=37.6 y=10.1
circle k1 sketch=s centre=h1 r=3.2
circle k2 sketch=s centre=h2 r=3.2
extrude e1 sketch=s outer=l1 dir=[0 0 1] length=6
";

#[test]
fn positions_propose_the_mirror_pair_and_round_edge_distances() {
    let d = doc(POSITIONS);
    let mut s = box_planes(50.0, 30.0, 6.0);
    s.push(cyl(v(12.4, 10.1, 3.0), 3.2, 400, 0.3));
    s.push(cyl(v(37.6, 10.1, 3.0), 3.2, 400, 0.3));
    let m = Measurement::Shapes { shapes: s, sigma_mm: 0.5, sigma_deg: SIGMA_DEG };
    let ps = suggest(&d, &m);
    let pos: Vec<&Proposal> = ps
        .iter()
        .filter(|p| p.why.starts_with("symmetric") || p.why.starts_with("datum grid") || p.why.starts_with("edge distance"))
        .collect();
    let got: Vec<&str> = pos.iter().map(|p| p.stmt.as_str()).collect();
    // The midline set (ONE why), then h1's two edge distances; h2 is the mirror member and gets none.
    let n = ps.len();
    assert_eq!(
        got,
        [
            format!("point i{} sketch=s x=25 y=0", n - 7),
            format!("midpoint i{} sketch=s p=i{} line=l1", n - 6, n - 7),
            format!("point i{} sketch=s x=25 y=30", n - 5),
            format!("midpoint i{} sketch=s p=i{} line=l3", n - 4, n - 5),
            format!("line i{} sketch=s a=i{} b=i{}", n - 3, n - 7, n - 5),
            format!("symmetric i{} sketch=s a=h1 b=h2 line=i{}", n - 2, n - 3),
            format!("distance_point_line i{} sketch=s p=h1 line=l4 d=12.5", n - 1),
            format!("distance_point_line i{} sketch=s p=h1 line=l1 d=10", n),
        ],
        "{:?}",
        stmts(&ps)
    );
    assert!(pos[..6].iter().all(|p| p.why == pos[0].why), "one set shares one why");
    assert!(pos[0].why.starts_with("symmetric: h1 h2 mirror about the midline of l2 l4"), "{}", pos[0].why);
    assert!(pos[6].why.starts_with("datum grid: h1"), "{}", pos[6].why);
    assert!(pos.iter().all(|p| p.confidence > 0.5), "{:?}", pos.iter().map(|p| p.confidence).collect::<Vec<_>>());
    assert!((pos[6].confidence - 0.8).abs() < 1e-9 && (pos[7].confidence - 0.8).abs() < 1e-9);
    let b = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(b.accepted(), ps.len(), "{}", b.text());
}

/// One r 3.2 hole at (13.37, 9.2) read at sigma 0.1: neither its coordinates nor its edge distances are round.
#[test]
fn an_unround_position_proposes_nothing() {
    let text = POSITIONS
        .replace("point h1 sketch=s x=12.4 y=10.1", "point h1 sketch=s x=13.37 y=9.2")
        .replace("point h2 sketch=s x=37.6 y=10.1\n", "")
        .replace("circle k2 sketch=s centre=h2 r=3.2\n", "");
    let d = doc(&text);
    let mut s = box_planes(50.0, 30.0, 6.0);
    s.push(cyl(v(13.37, 9.2, 3.0), 3.2, 400, 0.3));
    let m = Measurement::Shapes { shapes: s, sigma_mm: 0.1, sigma_deg: SIGMA_DEG };
    let ps = suggest(&d, &m);
    assert!(ps.iter().all(|p| !p.stmt.contains("p=h1") && !p.stmt.contains("a=h1")), "{:?}", stmts(&ps));
    assert!(
        ps.iter().all(|p| !p.why.starts_with("symmetric") && !p.why.starts_with("datum grid") && !p.why.starts_with("edge distance")),
        "{:?}",
        stmts(&ps)
    );
    // The plate's own round proposals still come.
    assert!(stmts(&ps).contains(&"distance i5 sketch=s a=p1 b=p2 d=50"), "{:?}", stmts(&ps));
}

#[test]
fn the_iso_tables() {
    assert_eq!(clearance_for(6.6), Some(("M6", Class::Medium)));
    assert_eq!(clearance_for(6.4), Some(("M6", Class::Fine)));
    assert_eq!(clearance_for(13.5), Some(("M12", Class::Medium)));
    assert_eq!(clearance_for(6.5), None);
    let s = nearest_standard(6.4, 0.3).unwrap();
    assert_eq!((s.thread, s.class, s.hole_mm, s.pitch_mm), ("M6", Class::Medium, 6.6, 1.0));
    let s = nearest_standard(6.4, 0.05).unwrap();
    assert_eq!((s.thread, s.class, s.hole_mm), ("M6", Class::Fine, 6.4));
    let s = nearest_standard(5.0, 0.1).unwrap();
    assert_eq!((s.thread, s.class, s.hole_mm), ("M6", Class::TapDrill, 5.0));
    assert_eq!(nearest_standard(7.7, 0.1), None);
    assert_eq!(nearest_standard(6.4, f64::NAN), None);
}

// ---- VUGINTENTFEATURES (SR536): a boss or a pocket from a cylinder standing on a document face ----

/// A `w` x `h` x `t` plate with no holes, drawn at round values.
fn plate(w: f64, h: f64, t: f64) -> String {
    format!(
        "vug-doc version=1 lin=1e-6 ang=1e-9
sketch s o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s x=0 y=0
point p2 sketch=s x={w} y=0
point p3 sketch=s x={w} y={h}
point p4 sketch=s x=0 y={h}
line l1 sketch=s a=p1 b=p2
line l2 sketch=s a=p2 b=p3
line l3 sketch=s a=p3 b=p4
line l4 sketch=s a=p4 b=p1
extrude e1 sketch=s outer=l1 dir=[0 0 1] length={t}
"
    )
}

/// The proposals whose why starts with `kind:` (a feature set: four statements sharing one why).
fn feature_set<'a>(ps: &'a [Proposal], kind: &str) -> Vec<&'a Proposal> {
    ps.iter().filter(|p| p.why.starts_with(kind)).collect()
}

#[test]
fn a_boss_on_the_plate_proposes_one_extrude_set() {
    let d = doc(&plate(80.0, 60.0, 10.0));
    // The boss: r 7.95, its support centroid 6.05 mm above the plate top (a 12.1 mm boss); no boss-top plane (option C:
    // a face only the set models would refuse its leading lines by Residual, VUG-INTENT-SET-VERIFY owed).
    let mut s = box_planes(80.0, 60.0, 10.0);
    s.push(cyl(v(40.0, 30.0, 16.05), 7.95, 900, 0.2));
    let m = shapes(s);
    let ps = suggest(&d, &m);
    let set = feature_set(&ps, "boss:");
    assert_eq!(set.len(), 4, "{:?}", stmts(&ps));
    assert!(feature_set(&ps, "pocket:").is_empty(), "{:?}", stmts(&ps));
    let a = set[0].stmt.split(' ').nth(1).unwrap();
    let n: u32 = a[1..].parse().unwrap();
    let (b, c, x) = (format!("i{}", n + 1), format!("i{}", n + 2), format!("i{}", n + 3));
    assert_eq!(
        set.iter().map(|p| p.stmt.as_str()).collect::<Vec<_>>(),
        [
            format!("sketch {a} o=[40 30 10] z=[0 0 1] x=[1 0 0]"),
            format!("point {b} sketch={a} x=0 y=0"),
            format!("circle {c} sketch={a} centre={b} r=8"),
            format!("extrude {x} sketch={a} outer={c} dir=[0 0 1] length=12"),
        ]
    );
    assert!(set.iter().all(|p| p.why == set[0].why), "one set shares one why");
    assert!(set[0].why.contains("the support centroid"), "{}", set[0].why);
    assert!(set.iter().all(|p| p.confidence > 0.5), "{}", set[0].confidence);
    let len: f64 = set[3].stmt.rsplit("length=").next().unwrap().parse().unwrap();
    assert!((len - 12.1).abs() <= SIGMA, "the boss height {} within sigma of the measured 12.1", len);
    // The set comes after every sketch rule.
    assert_eq!(ps.iter().position(|p| p.why.starts_with("boss:")).unwrap() + 4, ps.len());
    let bt = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(bt.accepted(), ps.len(), "{}", bt.text());
    // Refused: a pin whose span a measured plane crosses (centroid at z=30, a face at z=20) has no end on the plate top
    // within 3 sigma, so no feature is proposed.
    let mut s = box_planes(80.0, 60.0, 10.0);
    s.push(plane(v(0.0, 0.0, 1.0), 20.0, 300, 0.2));
    s.push(cyl(v(40.0, 30.0, 30.0), 4.0, 600, 0.2));
    let ps = suggest(&d, &shapes(s));
    assert!(feature_set(&ps, "boss:").is_empty() && feature_set(&ps, "pocket:").is_empty(), "{:?}", stmts(&ps));
}

#[test]
fn a_through_hole_proposes_the_pocket_set_with_the_plate_thickness() {
    let d = doc(&plate(60.0, 40.0, 8.0));
    // The top seen more than the bottom; the r 3.28 hole's support centroid mid-plate, below the top plane.
    let mut s = box_planes(60.0, 40.0, 8.0);
    s[0] = plane(v(0.0, 0.0, 1.0), 0.0, 3000, 0.2);
    s[1] = plane(v(0.0, 0.0, 1.0), 8.0, 5000, 0.2);
    s.push(cyl(v(30.0, 20.0, 4.0), 3.28, 500, 0.2));
    let m = shapes(s);
    let ps = suggest(&d, &m);
    let set = feature_set(&ps, "pocket:");
    assert_eq!(set.len(), 4, "{:?}", stmts(&ps));
    assert!(feature_set(&ps, "boss:").is_empty(), "{:?}", stmts(&ps));
    let a = set[0].stmt.split(' ').nth(1).unwrap();
    let n: u32 = a[1..].parse().unwrap();
    let (b, c, x) = (format!("i{}", n + 1), format!("i{}", n + 2), format!("i{}", n + 3));
    assert_eq!(
        set.iter().map(|p| p.stmt.as_str()).collect::<Vec<_>>(),
        [
            format!("sketch {a} o=[30 20 8] z=[0 0 -1] x=[1 0 0]"),
            format!("point {b} sketch={a} x=0 y=0"),
            format!("circle {c} sketch={a} centre={b} r=3.3"),
            format!("extrude {x} sketch={a} outer={c} dir=[0 0 -1] length=8"),
        ]
    );
    assert!(set[0].why.contains("on plane #1") && set[0].why.contains("far-end plane #0"), "{}", set[0].why);
    assert!(set[0].why.contains("ISO 273 medium clearance for M6"), "{}", set[0].why);
    assert!((set[0].confidence - (1.0 - 0.04 / SIGMA)).abs() < 1e-9, "{}", set[0].confidence);
    let bt = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(bt.accepted(), ps.len(), "{}", bt.text());
}

// ---- VUGINTENTSETVERIFY (SR541): a statement set judged as one unit ----

/// The plate with a 12.1 mm boss (r 7.95, support centroid 6.05 above the plate top) AND its top plane measured at
/// z = 22.1 (#6): a face only the set's extrude models.
fn boss_with_top() -> (Doc, Measurement) {
    let d = doc(&plate(80.0, 60.0, 10.0));
    let mut s = box_planes(80.0, 60.0, 10.0);
    s.push(plane(v(0.0, 0.0, 1.0), 22.1, 700, 0.2));
    s.push(cyl(v(40.0, 30.0, 16.05), 7.95, 900, 0.2));
    (d, shapes(s))
}

fn residual_max(v: &Verdict) -> f64 {
    match v {
        Verdict::Refuse { why: Reason::Residual { max_mm, .. } } => *max_mm,
        v => panic!("not a Residual refusal: {:?}", v),
    }
}

#[test]
fn a_boss_with_its_top_measured_is_accepted_as_one_set() {
    let (d, m) = boss_with_top();
    let ps = suggest(&d, &m);
    let set = feature_set(&ps, "boss:");
    assert_eq!(set.len(), 4, "{:?}", stmts(&ps));
    assert!(set[0].why.contains("far-end plane #6"), "{}", set[0].why);
    assert!(set[3].stmt.ends_with("length=12"), "{}", set[3].stmt);
    // VUGINTENTSETORDER (SR551): the boss top no document face models within the band is left out of the leading
    // line's residual (the SR536 gap closed: it was refused by Residual 12.1).
    assert!(matches!(verify(&d, set[0], &m, d.tol), Verdict::Accept { .. }));
    let first = ps.len() - 4;
    let b = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(b.sets.len(), 1, "{}", b.text());
    let (start, steps) = &b.sets[0];
    assert_eq!(*start, first);
    assert_eq!(steps.len(), 4);
    assert!(steps.iter().all(|v| matches!(v, Verdict::Accept { .. })), "{:?}", steps);
    for (s, _, v) in &b.entries[first..] {
        match v {
            Verdict::Accept { max_mm, .. } => assert!(*max_mm <= 3.0 * SIGMA, "{s}: max {max_mm}"),
            v => panic!("{s}: the set refused: {v:?}\n{}", b.text()),
        }
    }
    // VUGINTENTSETORDER (SR551): the plate's own single proposals AND the set.
    assert_eq!(b.accepted(), ps.len(), "{}", b.text());
    assert_eq!(b.doc.stmts.len(), d.stmts.len() + ps.len());
    // The set's graph is the reader's: the extrude names its sketch and its circle.
    let id = |p: &Proposal| p.stmt.split(' ').nth(1).unwrap().to_string();
    let ex = b.doc.stmts.iter().find(|s| s.id() == Some(id(set[3]).as_str())).unwrap();
    assert_eq!(ex.refs(), [id(set[0]), id(set[2])]);
    assert!(matches!(ex, Stmt::Extrude { .. }));
    let t = b.text();
    let lines: Vec<&str> = t.lines().collect();
    let sk = lines.iter().find(|l| l.starts_with(&format!("accept {}", set[0].stmt))).unwrap();
    assert!(sk.ends_with(" set=1 step=accept"), "{t}");
    let ex = lines.iter().find(|l| l.starts_with(&format!("accept {}", set[3].stmt))).unwrap();
    assert!(ex.ends_with(" set=1 step=accept"), "{t}");
    assert_eq!(lines.iter().filter(|l| l.starts_with("accept ")).count(), b.accepted(), "{t}");
    // A single proposal's line carries no set suffix.
    assert!(lines.iter().filter(|l| l.contains(" set=")).count() == 4, "{t}");
}

#[test]
fn a_set_with_a_wrong_extrude_is_refused_whole_with_its_reason() {
    let (d, m) = boss_with_top();
    let mut ps = suggest(&d, &m);
    let n = ps.len();
    assert!(ps[n - 1].stmt.starts_with("extrude ") && ps[n - 1].stmt.ends_with("length=12"), "{:?}", stmts(&ps));
    ps[n - 1].stmt = ps[n - 1].stmt.replace("length=12", "length=20");
    let b = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(b.sets.len(), 1, "{}", b.text());
    let (start, steps) = &b.sets[0];
    assert_eq!(*start, n - 4);
    // The boss top at z = 30 against the measured 22.1: the extrude's own reason is the set's.
    let Verdict::Refuse { why } = &steps[3] else { panic!("the wrong extrude accepted: {:?}", steps[3]) };
    assert!((residual_max(&steps[3]) - 7.9).abs() < 1e-6, "{:?}", steps[3]);
    for (s, _, v) in &b.entries[n - 4..] {
        assert_eq!(v, &Verdict::Refuse { why: why.clone() }, "{s}");
    }
    assert!(b.entries[n - 4..].iter().all(|(_, _, v)| (residual_max(v) - 7.9).abs() < 1e-6));
    if let Reason::Residual { limit_mm, .. } = why {
        assert!((limit_mm - 0.9).abs() < 1e-12);
    }
    // Nothing of the set entered the document.
    assert!(b.doc.stmts.len() == d.stmts.len() + b.accepted() && b.accepted() + 4 <= n, "{}", b.text());
    assert!(!b.doc.stmts.iter().any(|s| s.id() == stmts(&ps)[n - 1].split(' ').nth(1)), "{}", b.text());
}

// ---- VUGINTENTSETORDER (SR551): a library part takes a proposal through apply_with ----

/// A shelf of one machine library by name (the face's lookup beside the part, in memory).
struct Shelf(&'static str, String);

impl Lib for Shelf {
    fn resolve(&self, name: &str) -> Option<String> {
        (name == self.0).then(|| self.1.clone())
    }
}

#[test]
fn a_library_part_takes_a_proposal_through_apply_with() {
    let lib = "vug-doc version=1 lin=1e-6 ang=1e-9
machine mill name=\"Shop mill\" kind=mill3 kinematics=cartesian units=mm envelope=[[-50 -50 -60] [100 100 50]] safe_z=10 power=spindle power_max=24000 ask_above=20000
axis axis=x min=-50 max=100 max_feed=5000 max_accel=500 rapid=8000
axis axis=y min=-50 max=100 max_feed=5000 max_accel=500 rapid=8000
axis axis=z min=-60 max=50 max_feed=2000 max_accel=300 rapid=4000
tool number=1 kind=flat_end dc=6 lf=40 apmx=2 nof=2 sig=0
";
    let shelf = Shelf("shop-mill", lib.to_string());
    let part = format!("{}machine mill lib=shop-mill loaded=1\n", plate(80.0, 60.0, 10.0));
    let d = read_with(&part, &shelf).unwrap_or_else(|r| panic!("read_with: {r:?}"));
    let prop = p("distance i1 sketch=s a=p1 b=p2 d=80");
    // VUGMAKEMACHINELIBHELD (SR556): the plain road re-reads from the library the part holds, as apply_with does.
    let d2 = apply_with(&d, &prop, &shelf).unwrap_or_else(|r| panic!("apply_with: {}", r.text()));
    assert_eq!(apply(&d, &prop), Ok(d2.clone()), "plain apply takes the library part");
    assert_eq!(d2.stmts.len(), d.stmts.len() + 1);
    assert!(matches!(d2.stmts.last(), Some(Stmt::Constraint { .. })));
    assert_eq!(d2.stmts.last().unwrap().refs(), ["s", "p1", "p2"]);
    let Some(Stmt::Machine(m)) = d2.stmts.iter().find(|s| matches!(s, Stmt::Machine(_))) else { panic!("machine") };
    assert_eq!(m.lib.as_ref().map(|l| l.name.as_str()), Some("shop-mill"));
    assert_eq!(m.loaded, Some(1));
}

// ---- VUGINTENTTHICKNESS (SR557): an extrude judged by a measured thickness ----

/// The 80 x 60 plate's sketch with no extrude (a photo's sketch before its side photo).
fn plate_sketch() -> Doc {
    let text = plate(80.0, 60.0, 10.0);
    let text: String = text.lines().filter(|l| !l.starts_with("extrude ")).map(|l| format!("{l}\n")).collect();
    let d = read(&text).unwrap();
    assert!(replay(&d, d.tol).solids.is_empty());
    d
}

fn thickness(mm: f64) -> Measurement {
    Measurement::Thickness { mm, sigma_mm: SIGMA, along: v(0.0, 0.0, 1.0) }
}

#[test]
fn a_thickness_accepts_the_measured_extrude_and_refuses_one_2mm_off() {
    let d = plate_sketch();
    let m = thickness(10.0);
    match verify(&d, &p("extrude e1 sketch=s outer=l1 dir=[0 0 1] length=10"), &m, d.tol) {
        Verdict::Accept { max_mm, .. } => assert!(max_mm < 1e-6, "max {max_mm}"),
        v => panic!("length=10 refused: {v:?}"),
    }
    // The outline is the same either way: only the thickness tells 12 from 10.
    match verify(&d, &p("extrude e1 sketch=s outer=l1 dir=[0 0 1] length=12"), &m, d.tol) {
        Verdict::Refuse { why: Reason::Residual { max_mm, limit_mm, .. } } => {
            assert!((max_mm - 2.0).abs() < 1e-6, "max {max_mm}");
            assert!((limit_mm - 3.0 * SIGMA).abs() < 1e-12, "limit {limit_mm}");
        }
        v => panic!("length=12 not a Residual refusal: {v:?}"),
    }
}

#[test]
fn a_thickness_with_no_solid_is_unmatched_and_tabled() {
    let d = plate_sketch();
    let m = thickness(10.0);
    match verify(&d, &p("distance i1 sketch=s a=p1 b=p2 d=80"), &m, d.tol) {
        Verdict::Refuse { why: Reason::UnmatchedShape { kind, .. } } => assert_eq!(kind, "thickness"),
        v => panic!("no solid not unmatched: {v:?}"),
    }
    assert_eq!(measurements_text(&m), "thickness: mm=10 sigma=0.3 along=(0,0,1)\n");
}

// ---- VUGMAKEMACHINELIBHELD (SR556): a library part through plain apply and verify_all ----

#[test]
fn a_library_part_takes_a_proposal_through_plain_apply_and_verify_all() {
    let lib = "vug-doc version=1 lin=1e-6 ang=1e-9
machine mill name=\"Shop mill\" kind=mill3 kinematics=cartesian units=mm envelope=[[-50 -50 -60] [100 100 50]] safe_z=10 power=spindle power_max=24000 ask_above=20000
axis axis=x min=-50 max=100 max_feed=5000 max_accel=500 rapid=8000
tool number=1 kind=flat_end dc=6 lf=40 apmx=2 nof=2 sig=0
";
    let shelf = Shelf("shop-mill", lib.to_string());
    let d = read_with(&format!("{HOLE_PLATE}machine mill lib=shop-mill loaded=1\n"), &shelf).unwrap_or_else(|r| panic!("read_with: {r:?}"));
    let plain = doc(HOLE_PLATE);
    let ps = [p("diameter i1 sketch=s circle=c1 d=80")];
    let m = shapes(fixture_shapes());
    // verify takes the library part (no UnknownLib) and judges it as the plain plate.
    assert_eq!(verify(&d, &ps[0], &m, d.tol), verify(&plain, &ps[0], &m, plain.tol));
    let b = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(b.accepted(), 1, "{}", b.text());
    assert_eq!(b.text(), verify_all(&plain, &ps, &m, plain.tol).text(), "the same batch as the plain plate");
    assert_eq!(b.doc.stmts.len(), d.stmts.len() + 1);
    let Some(Stmt::Machine(mm)) = b.doc.stmts.iter().find(|s| matches!(s, Stmt::Machine(_))) else { panic!("machine") };
    assert_eq!(mm.lib.as_ref().map(|l| (l.name.as_str(), l.stmts.len())), Some(("shop-mill", 2)), "the library stays held");
}

// ---- VUGINTENTUNDOSET (SR563): a Residual names the measurement term of its max ----

#[test]
fn a_residual_names_its_term() {
    let d = plate_sketch();
    match verify(&d, &p("extrude e1 sketch=s outer=l1 dir=[0 0 1] length=12"), &thickness(10.0), d.tol) {
        Verdict::Refuse { why } => {
            assert!(matches!(why, Reason::Residual { term: Term::Thickness, .. }), "{why:?}");
            assert!(why.text().ends_with(" term=thickness"), "{}", why.text());
        }
        v => panic!("length=12 not refused: {v:?}"),
    }
    let pt = brep_core::geom_core::Vec2::new;
    let outline = Measurement::outline(vec![pt(0.0, 0.0), pt(80.0, 0.0), pt(80.0, 60.0), pt(0.0, 60.0)], SIGMA);
    match verify(&d, &p("distance i1 sketch=s a=p1 b=p2 d=86"), &outline, d.tol) {
        Verdict::Refuse { why } => assert!(matches!(why, Reason::Residual { term: Term::Outline, .. }), "{why:?}"),
        v => panic!("d=86 not refused: {v:?}"),
    }
}

// ---- VUGINTENTFEATURESEXTENT (SR653): a cylinder that carries its support's axial extent ----

/// The real boss is 8 tall (top at z = 18) on the 80 x 60 x 10 plate; its lower wall occluded, so the support centroid
/// sits high (z = 15, t = 5) and the support spans z 10.4 .. 18.0 (extent [-4.6, 3.0] from the point).
#[test]
fn a_boss_carries_its_axial_extent() {
    let d = doc(&plate(80.0, 60.0, 10.0));
    let measured = ShapeRef { extent: Some([-4.6, 3.0]), ..cyl(v(40.0, 30.0, 15.0), 7.95, 900, 0.2) };
    let inferred = cyl(v(40.0, 30.0, 15.0), 7.95, 900, 0.2);
    // A: no boss-top plane. The SR536 road (no extent) reads the centroid as mid-extent: length=10; the extent: 8.
    let mut s = box_planes(80.0, 60.0, 10.0);
    s.push(inferred);
    let ps = suggest(&d, &shapes(s));
    let set = feature_set(&ps, "boss:");
    assert_eq!(set.len(), 4, "{:?}", stmts(&ps));
    assert!(set[3].stmt.ends_with("dir=[0 0 1] length=10"), "{}", set[3].stmt);
    assert!(set[0].why.contains("the support centroid"), "{}", set[0].why);
    let mut s = box_planes(80.0, 60.0, 10.0);
    s.push(measured);
    let m = shapes(s);
    let ps = suggest(&d, &m);
    let set = feature_set(&ps, "boss:");
    assert_eq!(set.len(), 4, "{:?}", stmts(&ps));
    assert!(set[0].stmt.ends_with("o=[40 30 10] z=[0 0 1] x=[1 0 0]"), "{}", set[0].stmt);
    assert!(set[2].stmt.ends_with(" r=8"), "{}", set[2].stmt);
    assert!(set[3].stmt.ends_with("dir=[0 0 1] length=8"), "{}", set[3].stmt);
    assert!(set[0].why.contains("the support's axial extent (near end 0.400 mm off)"), "{}", set[0].why);
    let bt = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(bt.accepted(), ps.len(), "{}", bt.text());
    // B: the boss top measured at z = 18 (#6). The extent's far end meets it (e = 0): length=8 from the far-end plane;
    // the SR536 road on the same shapes reads it as a plane crossing the span (e = |5 - 8/2| = 1 > 0.9, 8 < 10 - 1.8)
    // and proposes no set.
    let mut s = box_planes(80.0, 60.0, 10.0);
    s.push(plane(v(0.0, 0.0, 1.0), 18.0, 700, 0.2));
    s.push(measured);
    let m = shapes(s);
    let ps = suggest(&d, &m);
    let set = feature_set(&ps, "boss:");
    assert_eq!(set.len(), 4, "{:?}", stmts(&ps));
    assert!(set[3].stmt.ends_with("dir=[0 0 1] length=8"), "{}", set[3].stmt);
    assert!(set[0].why.contains("far-end plane #6 (near end 0.000 mm off)"), "{}", set[0].why);
    let bt = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(bt.accepted(), ps.len(), "{}", bt.text());
    let mut s = box_planes(80.0, 60.0, 10.0);
    s.push(plane(v(0.0, 0.0, 1.0), 18.0, 700, 0.2));
    s.push(inferred);
    let ps = suggest(&d, &shapes(s));
    assert!(feature_set(&ps, "boss:").is_empty() && feature_set(&ps, "pocket:").is_empty(), "{:?}", stmts(&ps));
}

// ---- VUGINTENTFEATURESEXTENTTABLE (SR665): the extent in the table Vein reads; a near end off the plane proposes nothing ----

/// A shape that carries its axial extent prints ` len=<mm> ends=<a>,<b>` on its line; a shape without one prints the
/// SR508 line byte for byte.
#[test]
fn the_measurement_table_carries_a_shapes_axial_extent() {
    let m = shapes(vec![
        plane(v(0.0, 0.0, 1.0), 10.0, 5000, 0.4),
        ShapeRef { extent: Some([-4.6, 3.0]), ..cyl(v(40.0, 30.0, 15.0), 7.95, 900, 0.2) },
        ShapeRef {
            params: ShapeParams::Cone { apex: v(0.0, 0.0, 0.0), axis: v(0.0, 0.0, 1.0), half_angle: core::f64::consts::FRAC_PI_4 },
            support: 10,
            rms_mm: 0.05,
            extent: Some([0.5, 4.25]),
        },
    ]);
    assert_eq!(
        measurements_text(&m),
        "shapes: n=3 sigma=0.3 sigma_deg=0.5\n\
         plane n=(0,0,1) d=10 support=5000 rms=0.4\n\
         cylinder axis=(40,30,15) dir=(0,0,1) r=7.95 support=900 rms=0.2 len=7.6 ends=-4.6,3\n\
         cone apex=(0,0,0) dir=(0,0,1) half_angle_deg=45 support=10 rms=0.05 len=3.75 ends=0.5,4.25\n"
    );
}

/// The SR653 plate (80 x 60 x 10, top z = 10) at sigma 0.3: K_SIGMA x sigma = 0.9. A support whose near end sits 2 mm
/// above the plate (point raised to z 17, t = 7, extent [-5, 1]: near 2.000, far 8.000) does not start at that plane and
/// proposes no set; nor does one whose near end runs 2 mm through it (z 15, extent [-7, 3]: near -2); the control at
/// near 0.850 (z 15, extent [-4.15, 3]) still proposes the boss, length=8.
#[test]
fn a_near_end_off_the_plane_proposes_no_feature() {
    let d = doc(&plate(80.0, 60.0, 10.0));
    let none = |p: Vec3, e: [f64; 2]| {
        let mut s = box_planes(80.0, 60.0, 10.0);
        s.push(ShapeRef { extent: Some(e), ..cyl(p, 7.95, 900, 0.2) });
        let ps = suggest(&d, &shapes(s));
        assert!(feature_set(&ps, "boss:").is_empty() && feature_set(&ps, "pocket:").is_empty(), "{:?}", stmts(&ps));
    };
    none(v(40.0, 30.0, 17.0), [-5.0, 1.0]);
    none(v(40.0, 30.0, 15.0), [-7.0, 3.0]);
    let mut s = box_planes(80.0, 60.0, 10.0);
    s.push(ShapeRef { extent: Some([-4.15, 3.0]), ..cyl(v(40.0, 30.0, 15.0), 7.95, 900, 0.2) });
    let ps = suggest(&d, &shapes(s));
    let set = feature_set(&ps, "boss:");
    assert_eq!(set.len(), 4, "{:?}", stmts(&ps));
    assert!(set[3].stmt.ends_with("dir=[0 0 1] length=8"), "{}", set[3].stmt);
    assert!(set[0].why.contains("the support's axial extent (near end 0.850 mm off)"), "{}", set[0].why);
}

// ---- VUGINTENTFEATURESSLOT (SR666): a slot from two cylinders and their two walls ----

/// The 80 x 60 x 10 plate with a 30 x 10 obround pocket 5 deep at its centre, as scan_core's detect returns it: the
/// box planes (#0-#5), the pocket floor at z = 5 (#6), the two walls of ONE normal sign (`canonical`) at y = 25 (#7) and
/// y = 35 (#8), and the two half-cylinders r 5 (#9, #10) whose axis points sit 20 apart at mid-depth.
fn slot_pocket(walls: bool) -> Vec<ShapeRef> {
    let mut s = box_planes(80.0, 60.0, 10.0);
    s.push(plane(v(0.0, 0.0, 1.0), 5.0, 600, 0.2));
    if walls {
        s.push(plane(v(0.0, 1.0, 0.0), 25.0, 400, 0.2));
        s.push(plane(v(0.0, 1.0, 0.0), 35.0, 400, 0.2));
    }
    s.push(cyl(v(30.0, 30.0, 7.5), 5.0, 300, 0.2));
    s.push(cyl(v(50.0, 30.0, 7.5), 5.0, 300, 0.2));
    s
}

#[test]
fn an_obround_pocket_proposes_one_slot_set() {
    let d = doc(&plate(80.0, 60.0, 10.0));
    let m = shapes(slot_pocket(true));
    let ps = suggest(&d, &m);
    let set = feature_set(&ps, "slot pocket:");
    assert_eq!(set.len(), 21, "{:?}", stmts(&ps)); // VUGINTENTFEATURESSLOTCONSTRAIN (SR675): 12 + the nine constraints
    // The slot's cylinders are taken: no single pocket or boss, and no boss slot.
    assert!(feature_set(&ps, "pocket:").is_empty() && feature_set(&ps, "boss:").is_empty(), "{:?}", stmts(&ps));
    assert!(feature_set(&ps, "slot boss:").is_empty(), "{:?}", stmts(&ps));
    let a = set[0].stmt.split(' ').nth(1).unwrap();
    let n: u32 = a[1..].parse().unwrap();
    let id = |k: u32| format!("i{}", n + k);
    let (c1, c2, q1, q2, q3, q4, l1, k1, l2, k2, x) = (id(1), id(2), id(3), id(4), id(5), id(6), id(7), id(8), id(9), id(10), id(11));
    let got: Vec<&str> = set.iter().map(|p| p.stmt.as_str()).collect();
    assert_eq!(
        vec![got[0], got[1], got[2], got[3], got[4], got[5], got[6], got[7], got[9], got[20]], // SR675: the extrude stays last
        [
            format!("sketch {a} o=[40 30 10] z=[0 0 -1] x=[1 0 0]"),
            format!("point {c1} sketch={a} x=-10 y=0"),
            format!("point {c2} sketch={a} x=10 y=0"),
            format!("point {q1} sketch={a} x=-10 y=-5"),
            format!("point {q2} sketch={a} x=10 y=-5"),
            format!("point {q3} sketch={a} x=10 y=5"),
            format!("point {q4} sketch={a} x=-10 y=5"),
            format!("line {l1} sketch={a} a={q1} b={q2}"),
            format!("line {l2} sketch={a} a={q3} b={q4}"),
            format!("extrude {x} sketch={a} outer={l1} dir=[0 0 -1] length=5"),
        ]
    );
    assert!(got[8].starts_with(&format!("arc {k1} sketch={a} centre={c2} r=5 start=-1.5707963")), "{}", got[8]);
    assert!(got[10].starts_with(&format!("arc {k2} sketch={a} centre={c1} r=5 start=1.5707963")), "{}", got[10]);
    assert!(got[10].contains(" end=4.712388"), "{}", got[10]);
    // VUGINTENTFEATURESSLOTCONSTRAIN (SR675): the nine constraints between the second arc and the extrude, ids after x.
    let t = |k: u32| id(11 + k);
    assert_eq!(
        got[11..20].to_vec(),
        [
            format!("coincident {} sketch={a} a={k1}.start b={q2}", t(1)),
            format!("coincident {} sketch={a} a={k1}.end b={q3}", t(2)),
            format!("coincident {} sketch={a} a={k2}.start b={q4}", t(3)),
            format!("coincident {} sketch={a} a={k2}.end b={q1}", t(4)),
            format!("tangent_line_circle {} sketch={a} line={l1} circle={k1}", t(5)),
            format!("tangent_line_circle {} sketch={a} line={l2} circle={k1}", t(6)),
            format!("tangent_line_circle {} sketch={a} line={l2} circle={k2}", t(7)),
            format!("tangent_line_circle {} sketch={a} line={l1} circle={k2}", t(8)),
            format!("parallel {} sketch={a} a={l1} b={l2}", t(9)),
        ]
    );
    assert!(set.iter().all(|p| p.why == set[0].why), "one set shares one why");
    let why = &set[0].why;
    assert!(why.contains("cylinders #9 and #10 on plane #1") && why.contains("walls #8 and #7"), "{why}");
    assert!(why.contains("length 20 from 20.000 mm, width 10 from 10.000 mm"), "{why}");
    assert!(why.contains("far-end plane #6 (near end 0.000 mm off), height 5"), "{why}");
    assert!((set[0].confidence - 1.0).abs() < 1e-12, "{}", set[0].confidence);
    // The set comes after every sketch rule, and verify_all takes the plate's singles and the slot as one set.
    assert_eq!(ps.iter().position(|p| p.why.starts_with("slot pocket:")).unwrap() + 21, ps.len()); // SR675: 21 statements
    let bt = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(bt.accepted(), ps.len(), "{}", bt.text());
    assert_eq!(bt.sets.len(), 1, "{}", bt.text());
    // The walls discriminate: the same cylinders with no walls are two pockets, no slot.
    let ps = suggest(&d, &shapes(slot_pocket(false)));
    assert!(ps.iter().all(|p| !p.why.starts_with("slot ")), "{:?}", stmts(&ps));
    assert_eq!(feature_set(&ps, "pocket:").len(), 8, "{:?}", stmts(&ps));
}

/// Two equal r 5 bosses 6 tall on the plate, axes parallel and 30 apart, no walls between them: the walls are what
/// discriminates (equal radii, parallel axes and d > 2 sigma all hold), so they stay two bosses and no slot.
#[test]
fn two_equal_bosses_stay_two_bosses() {
    let d = doc(&plate(80.0, 60.0, 10.0));
    let mut s = box_planes(80.0, 60.0, 10.0);
    s.push(cyl(v(25.0, 30.0, 13.0), 5.0, 400, 0.2));
    s.push(cyl(v(55.0, 30.0, 13.0), 5.0, 400, 0.2));
    let m = shapes(s);
    let ps = suggest(&d, &m);
    assert!(ps.iter().all(|p| !p.why.starts_with("slot ")), "{:?}", stmts(&ps));
    assert!(feature_set(&ps, "pocket:").is_empty(), "{:?}", stmts(&ps));
    let set = feature_set(&ps, "boss:");
    assert_eq!(set.len(), 8, "{:?}", stmts(&ps));
    assert!(set[0].why.contains("cylinder #6 on plane #1") && set[4].why.contains("cylinder #7 on plane #1"), "{}", set[0].why);
    for (k, x0) in [(0, 25), (4, 55)] {
        let a = set[k].stmt.split(' ').nth(1).unwrap();
        let n: u32 = a[1..].parse().unwrap();
        let (b, c, x) = (format!("i{}", n + 1), format!("i{}", n + 2), format!("i{}", n + 3));
        assert_eq!(
            set[k..k + 4].iter().map(|p| p.stmt.as_str()).collect::<Vec<_>>(),
            [
                format!("sketch {a} o=[{x0} 30 10] z=[0 0 1] x=[1 0 0]"),
                format!("point {b} sketch={a} x=0 y=0"),
                format!("circle {c} sketch={a} centre={b} r=5"),
                format!("extrude {x} sketch={a} outer={c} dir=[0 0 1] length=6"),
            ]
        );
    }
    // VUGINTENTSETSIBLINGS (SR674): each set names the cylinder it models, and is judged with the one its sibling set
    // names left out (SR551's rule alone took the other, still unmodelled boss 30 mm off into each set's residual and
    // refused both by Residual 30): both sets and the plate's own singles are accepted.
    assert!(set[..4].iter().all(|p| p.shapes == [6]) && set[4..].iter().all(|p| p.shapes == [7]), "{:?}", set);
    let bt = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(bt.sets.len(), 2, "{}", bt.text());
    for (start, _) in &bt.sets {
        for (st, _, v) in &bt.entries[*start..*start + 4] {
            match v {
                Verdict::Accept { max_mm, .. } => assert!(*max_mm <= 3.0 * SIGMA, "{st}: max {max_mm}"),
                v => panic!("{st}: the set refused: {v:?}\n{}", bt.text()),
            }
        }
    }
    assert_eq!(bt.accepted(), ps.len(), "{}", bt.text());
    // The leftover control (SR551 kept): a cylinder NO proposal names (r 3, its axis 45 deg off z in xz, so no plane
    // stands it and no slot pairs it) still counts in each set's residual, so both sets are refused by Residual.
    let mut s = box_planes(80.0, 60.0, 10.0);
    s.push(cyl(v(25.0, 30.0, 13.0), 5.0, 400, 0.2));
    s.push(cyl(v(55.0, 30.0, 13.0), 5.0, 400, 0.2));
    s.push(ShapeRef { params: ShapeParams::Cylinder { point: v(40.0, 45.0, 20.0), axis: v(1.0, 0.0, 1.0), radius: 3.0 }, support: 300, rms_mm: 0.2, extent: None });
    let m = shapes(s);
    let ps = suggest(&d, &m);
    assert_eq!(feature_set(&ps, "boss:").len(), 8, "{:?}", stmts(&ps));
    assert!(ps.iter().all(|p| !p.shapes.contains(&8)), "{:?}", stmts(&ps));
    let bt = verify_all(&d, &ps, &m, d.tol);
    assert_eq!(bt.sets.len(), 2, "{}", bt.text());
    for (start, _) in &bt.sets {
        for (st, _, v) in &bt.entries[*start..*start + 4] {
            assert!(residual_max(v) > 3.0 * SIGMA, "{st}: {v:?}");
        }
    }
    assert_eq!(bt.accepted(), ps.len() - 8, "{}", bt.text());
}

// ---- VUGINTENTFEATURESSLOTCONSTRAIN (SR675): the slot set's constraints keep the obround under a drag ----

/// The slot pocket's set replayed: dof 5 (3 in the plane, length, width) with no redundant row; a corner dragged 1 mm
/// through sketch_core's own drag keeps both radii equal, each line tangent to each arc at its end, the lines parallel,
/// and the profile one closed loop of four edges.
#[test]
fn a_dragged_slot_corner_keeps_the_obround() {
    use sketch_core::{Entity, EntityId, PointRef, Vec2, drag, profiles};
    let d = doc(&plate(80.0, 60.0, 10.0));
    let ps = suggest(&d, &shapes(slot_pocket(true)));
    let set = feature_set(&ps, "slot pocket:");
    assert_eq!(set.len(), 21, "{:?}", stmts(&ps));
    let mut c = d.clone();
    for p in &set {
        c = apply(&c, p).unwrap_or_else(|e| panic!("{}: {e:?}", p.stmt));
    }
    let r = replay(&c, c.tol);
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    let sid = set[0].stmt.split(' ').nth(1).unwrap();
    let so = r.sketches.iter().find(|s| s.id == sid).expect("the slot sketch");
    assert_eq!(so.solved.dof, 5, "x, y, rotation, length and width");
    assert!(so.solved.redundant.is_empty(), "{:?}", so.solved.redundant);
    let mut sk = so.sketch.clone();
    let mut lines: Vec<(EntityId, EntityId, EntityId)> = Vec::new();
    let mut arcs: Vec<EntityId> = Vec::new();
    for (i, e) in sk.entities().iter().enumerate() {
        match *e {
            Entity::Line { a, b } => lines.push((EntityId(i as u32), a, b)),
            Entity::Arc { .. } => arcs.push(EntityId(i as u32)),
            _ => {}
        }
    }
    assert_eq!((lines.len(), arcs.len()), (2, 2));
    // The corner q2: the first line's end (the first arc's start).
    let corner = lines[0].2;
    let at = sk.point(PointRef::Point(corner)).unwrap();
    let to = Vec2::new(at.x + 0.6, at.y - 0.8);
    drag(&mut sk, PointRef::Point(corner), to, c.tol).expect("the dragged slot solves");
    let pt = |sk: &sketch_core::Sketch, p: PointRef| sk.point(p).unwrap();
    let got = pt(&sk, PointRef::Point(corner));
    assert!((got.x - to.x).abs() < 1e-6 && (got.y - to.y).abs() < 1e-6, "{got:?}");
    let (r1, r2) = (sk.radius(arcs[0]).unwrap(), sk.radius(arcs[1]).unwrap());
    assert!((r1 - r2).abs() < 1e-6, "radii {r1} {r2}");
    // Each arc's centre is r from BOTH lines (tangent to each), and the lines are parallel.
    let ends = |sk: &sketch_core::Sketch, l: &(EntityId, EntityId, EntityId)| (pt(sk, PointRef::Point(l.1)), pt(sk, PointRef::Point(l.2)));
    let unit = |a: Vec2, b: Vec2| {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let n = (dx * dx + dy * dy).sqrt();
        (dx / n, dy / n)
    };
    let (a0, b0) = ends(&sk, &lines[0]);
    let (a1, b1) = ends(&sk, &lines[1]);
    let (u0, u1) = (unit(a0, b0), unit(a1, b1));
    assert!((u0.0 * u1.1 - u0.1 * u1.0).abs() < 1e-6, "parallel {u0:?} {u1:?}");
    for &k in &arcs {
        let cc = pt(&sk, PointRef::Centre(k));
        let rk = sk.radius(k).unwrap();
        for (a, u) in [(a0, u0), (a1, u1)] {
            let dist = (u.0 * (cc.y - a.y) - u.1 * (cc.x - a.x)).abs();
            assert!((dist - rk).abs() < 1e-6, "centre {cc:?} {dist} from a line, r {rk}");
        }
    }
    // Each arc end sits on a line end.
    let line_ends = [a0, b0, a1, b1];
    for &k in &arcs {
        for p in [PointRef::Start(k), PointRef::End(k)] {
            let e = pt(&sk, p);
            assert!(line_ends.iter().any(|q| (q.x - e.x).abs() < 1e-6 && (q.y - e.y).abs() < 1e-6), "{p:?} at {e:?}");
        }
    }
    let pr = profiles(&sk, c.tol);
    assert_eq!(pr.len(), 1);
    assert_eq!(pr[0].outer.edges.len(), 4);
}
