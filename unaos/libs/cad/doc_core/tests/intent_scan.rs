//! VUGINTENTSCAN (LEDGER SR508): a proposal verified against scan_core's shapes (copied as `ShapeRef`s), the
//! measurement table Vein reads, the ISO 262 / 273 hole sizes, and the offline proposer whose every proposal is
//! disposed by `verify_all` (BP-vug-7).

use brep_core::Vec3;
use doc_core::{
    Class, Doc, Measurement, Proposal, Reason, ShapeParams, ShapeRef, Verdict, clearance_for, measurements_text,
    nearest_standard, read, replay, suggest, verify, verify_all,
};

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
    ShapeRef { params: ShapeParams::Plane { normal: n, d }, support, rms_mm }
}

fn cyl(p: Vec3, r: f64, support: u32, rms_mm: f64) -> ShapeRef {
    ShapeRef { params: ShapeParams::Cylinder { point: p, axis: v(0.0, 0.0, 1.0), radius: r }, support, rms_mm }
}

/// The SR497 fixture's values: the top plane and the r 39.74 cylinder.
fn fixture_shapes() -> Vec<ShapeRef> {
    vec![plane(v(0.0, 0.0, 1.0), 10.0, 5000, 0.4), cyl(v(60.0, 60.0, 5.0), 39.74, 1200, 0.6)]
}

fn shapes(s: Vec<ShapeRef>) -> Measurement {
    Measurement::Shapes { shapes: s, sigma_mm: SIGMA, sigma_deg: SIGMA_DEG }
}

fn p(s: &str) -> Proposal {
    Proposal { stmt: s.into(), confidence: 0.9, why: "test".into() }
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
        Verdict::Refuse { why: Reason::Residual { max_mm, limit_mm } } => {
            assert!((max_mm - 4.74).abs() < 1e-6, "max {}", max_mm);
            assert!((limit_mm - 0.9).abs() < 1e-12);
        }
        v => panic!("d=70 not a Residual refusal: {:?}", v),
    }
}

#[test]
fn a_shape_with_no_face_of_its_kind_is_unmatched_by_name() {
    let d = doc(HOLE_PLATE);
    let mut s = fixture_shapes();
    s.push(ShapeRef { params: ShapeParams::Sphere { centre: v(60.0, 60.0, 30.0), radius: 5.0 }, support: 300, rms_mm: 0.2 });
    let why = match verify(&d, &p("diameter i1 sketch=s circle=c1 d=80"), &shapes(s), d.tol) {
        Verdict::Refuse { why } => why,
        v => panic!("accepted: {:?}", v),
    };
    assert_eq!(why, Reason::UnmatchedShape { index: 2, kind: "sphere" });
    assert_eq!(why.text(), "unmatched-shape sphere #2");
    // A non-positive angular sigma is a bad measurement.
    let bad = Measurement::Shapes { shapes: fixture_shapes(), sigma_mm: SIGMA, sigma_deg: 0.0 };
    assert_eq!(
        verify(&d, &p("diameter i1 sketch=s circle=c1 d=80"), &bad, d.tol),
        Verdict::Refuse { why: Reason::BadMeasurement }
    );
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
