//! VUGINTENT (LEDGER SR505): a proposal is applied to a copy of the document, replayed, and accepted only inside the
//! measurement's confidence (BP-vug-7). A measured 49.7 x 30.2 rectangle (sigma 0.3) becomes exactly 50 x 30 through
//! accepted proposals; a dimension the measurement does not support, a conflicting constraint, a coordinate the
//! measurement did not give, a redundant constraint and a bad line are each refused by name. VUGINTENTHOLES (SR507):
//! the outline is loops; a plate with two holes takes its round diameters and refuses a moved hole, and a replayed
//! hole with no measured loop is refused UnmatchedLoop by name. VUGINTENTPOINTS (SR523): the Points residual is
//! two-sided: a boss removed is refused, a pin grown where no scan point is is refused by the surface side, a partial
//! scan (no bottom) is not refused, and 100 000 points against a ~2k-triangle replay are timed.

use brep_core::Vec3;
use doc_core::{
    Cons, Doc, Fault, K_SIGMA, Measurement, Proposal, Reason, Stmt, Verdict, What, apply, read, replay, verify,
    verify_all,
};
use sketch_core::{Edge, Vec2, profiles};

/// The measured rectangle as drawn from the photo: four points at the measured corners, four lines, no constraints.
const BASE: &str = "vug-doc version=1 lin=1e-6 ang=1e-9
sketch s o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s x=0 y=0
point p2 sketch=s x=49.7 y=0
point p3 sketch=s x=49.7 y=30.2
point p4 sketch=s x=0 y=30.2
line l1 sketch=s a=p1 b=p2
line l2 sketch=s a=p2 b=p3
line l3 sketch=s a=p3 b=p4
line l4 sketch=s a=p4 b=p1
";

const SIX: [&str; 6] = [
    "horizontal h1 sketch=s line=l1",
    "horizontal h2 sketch=s line=l3",
    "vertical v1 sketch=s line=l2",
    "vertical v2 sketch=s line=l4",
    "distance d1 sketch=s a=p1 b=p2 d=50",
    "distance d2 sketch=s a=p2 b=p3 d=30",
];

const SIGMA: f64 = 0.3;

fn base() -> Doc {
    read(BASE).unwrap()
}

fn p(s: &str) -> Proposal {
    Proposal { stmt: s.into(), confidence: 0.9, why: "round dimension".into() }
}

fn ps(v: &[&str]) -> Vec<Proposal> {
    v.iter().map(|s| p(s)).collect()
}

/// The closed outline of a w x h rectangle from (0, 0), CCW, a sample every `step` mm.
fn rect_outline(w: f64, h: f64, step: f64) -> Vec<Vec2> {
    let corners = [Vec2::new(0.0, 0.0), Vec2::new(w, 0.0), Vec2::new(w, h), Vec2::new(0.0, h)];
    let mut out = Vec::new();
    for i in 0..4 {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        let n = ((b - a).mag() / step).ceil().max(1.0) as usize;
        for k in 0..n {
            out.push(a.lerp(b, k as f64 / n as f64));
        }
    }
    out
}

fn outline() -> Measurement {
    Measurement::outline(rect_outline(49.7, 30.2, 5.0), SIGMA)
}

fn accepted() -> Doc {
    let b = verify_all(&base(), &ps(&SIX), &outline(), base().tol);
    assert_eq!(b.accepted(), 6, "{}", b.text());
    b.doc
}

/// The replayed first sketch's profile edge lengths, sorted.
fn edge_lengths(doc: &Doc) -> Vec<f64> {
    let r = replay(doc, doc.tol);
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    let mut v: Vec<f64> = profiles(&r.sketches[0].sketch, doc.tol)[0]
        .outer
        .edges
        .iter()
        .map(|e| match e {
            Edge::Line { from, to, .. } => from.dist(*to),
            _ => panic!("arc in a rectangle"),
        })
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v
}

#[test]
fn round_dimensions_accepted_and_exact() {
    let d = base();
    let b = verify_all(&d, &ps(&SIX), &outline(), d.tol);
    assert_eq!(b.entries.len(), 6);
    for (s, _, v) in &b.entries {
        match v {
            Verdict::Accept { residual_mm, max_mm, .. } => {
                assert!(*residual_mm < SIGMA, "{s}: rms {residual_mm}");
                assert!(*max_mm <= K_SIGMA * SIGMA, "{s}: max {max_mm}");
            }
            Verdict::Refuse { why } => panic!("{s} refused: {}", why.text()),
        }
    }
    assert_eq!(b.doc.stmts.len(), d.stmts.len() + 6);
    let l = edge_lengths(&b.doc);
    let want = [30.0, 30.0, 50.0, 50.0];
    for (got, w) in l.iter().zip(want) {
        assert!((got - w).abs() <= 10.0 * d.tol.lin, "edges {l:?}");
    }
}

#[test]
fn unsupported_dimension_refused_by_residual() {
    let acc = accepted();
    let prop = p("distance d1 sketch=s a=p1 b=p2 d=52");
    // Same id: a changed dimension replaces d1 in place.
    assert_eq!(apply(&acc, &prop).unwrap().stmts.len(), acc.stmts.len());
    match verify(&acc, &prop, &outline(), acc.tol) {
        Verdict::Refuse { why: Reason::Residual { max_mm, limit_mm, .. } } => {
            assert_eq!(limit_mm, K_SIGMA * SIGMA);
            assert!(max_mm > limit_mm && max_mm < 2.0, "max {max_mm}");
        }
        v => panic!("{v:?}"),
    }
}

#[test]
fn conflicting_constraint_refused_by_name() {
    let acc = accepted();
    match verify(&acc, &p("distance d9 sketch=s a=p1 b=p2 d=48"), &outline(), acc.tol) {
        Verdict::Refuse { why: Reason::Replay(fs) } => {
            assert!(fs.iter().any(|f| f.fault == Fault::Conflicting), "{fs:?}");
            assert!(fs.iter().any(|f| f.id == "d9" || f.id == "d1"), "{fs:?}");
        }
        v => panic!("{v:?}"),
    }
}

#[test]
fn coordinate_not_measured_refused() {
    let d = base();
    match verify(&d, &p("fix f9 sketch=s p=p4 at=[0 100]"), &outline(), d.tol) {
        Verdict::Refuse { why: Reason::Residual { max_mm, .. } } => assert!(max_mm > 60.0, "max {max_mm}"),
        v => panic!("{v:?}"),
    }
}

#[test]
fn batch_later_sees_earlier() {
    let d = base();
    let b = verify_all(
        &d,
        &ps(&["distance d1 sketch=s a=p1 b=p2 d=50", "distance d1 sketch=s a=p1 b=p2 d=50.1"]),
        &outline(),
        d.tol,
    );
    assert_eq!(b.accepted(), 2, "{}", b.text());
    // The second replaced the statement the first put in.
    assert_eq!(b.doc.stmts.len(), d.stmts.len() + 1);
    let got = b.doc.stmts.iter().find_map(|s| match s {
        Stmt::Constraint { id, c: Cons::Distance(_, _, v), .. } if id == "d1" => Some(*v),
        _ => None,
    });
    assert_eq!(got, Some(50.1));
}

#[test]
fn echo_no_proposals() {
    let d = base();
    let b = verify_all(&d, &[], &outline(), d.tol);
    assert_eq!(b.text(), "intent: proposed=0 accepted=0 refused=0\n");
    assert_eq!(b.doc, d);
}

#[test]
fn bad_lines_refused_by_name() {
    let d = base();
    match verify(&d, &p("distance d9 sketch=s a=nope b=p2 d=1"), &outline(), d.tol) {
        Verdict::Refuse { why: Reason::Lex(r) } => assert_eq!(r.what, What::UnknownRef("nope".into())),
        v => panic!("{v:?}"),
    }
    for s in ["", "   ", "# a comment", "horizontal h9 sketch=s line=l1\nvertical v9 sketch=s line=l2"] {
        assert_eq!(verify(&d, &p(s), &outline(), d.tol), Verdict::Refuse { why: Reason::NotOneLine }, "{s:?}");
    }
    let empty = Measurement::outline(Vec::new(), SIGMA);
    assert_eq!(
        verify(&d, &p(SIX[0]), &empty, d.tol),
        Verdict::Refuse { why: Reason::BadMeasurement }
    );
}

#[test]
fn points_against_the_solid() {
    let text = format!(
        "{BASE}{}\nfix f1 sketch=s p=p1 at=[0 0]\nextrude e1 sketch=s outer=l1 dir=[0 0 1] length=6.2\n",
        SIX.join("\n")
    );
    let d = read(&text).unwrap();
    let mut pts = Vec::new();
    for i in 0..5 {
        for j in 0..3 {
            pts.push(Vec3::new(5.0 + 10.0 * i as f64, 5.0 + 10.0 * j as f64, 6.0));
        }
    }
    let m = Measurement::Points { pts, sigma_mm: SIGMA };
    match verify(&d, &p("extrude e1 sketch=s outer=l1 dir=[0 0 1] length=6"), &m, d.tol) {
        Verdict::Accept { residual_mm, .. } => assert!(residual_mm < 0.01, "rms {residual_mm}"),
        v => panic!("{v:?}"),
    }
    match verify(&d, &p("extrude e1 sketch=s outer=l1 dir=[0 0 1] length=8"), &m, d.tol) {
        Verdict::Refuse { why: Reason::Residual { max_mm, .. } } => {
            assert!((max_mm - 2.0).abs() < 0.1, "max {max_mm}")
        }
        v => panic!("{v:?}"),
    }
}

#[test]
fn redundant_refused() {
    let acc = accepted();
    assert_eq!(
        verify(&acc, &p("horizontal h9 sketch=s line=l1"), &outline(), acc.tol),
        Verdict::Refuse { why: Reason::Redundant }
    );
}

#[test]
fn batch_text_names_each() {
    let d = base();
    let b = verify_all(
        &d,
        &ps(&["horizontal h1 sketch=s line=l1", "fix f9 sketch=s p=p4 at=[0 100]"]),
        &outline(),
        d.tol,
    );
    let t = b.text();
    let lines: Vec<&str> = t.lines().collect();
    assert_eq!(lines[0], "intent: proposed=2 accepted=1 refused=1");
    assert!(lines[1].starts_with("accept horizontal h1 sketch=s line=l1 rms="), "{t}");
    assert!(lines[1].ends_with("confidence=0.9"), "{t}");
    assert!(lines[2].starts_with("refuse fix f9 sketch=s p=p4 at=[0 100] residual max="), "{t}");
}

// VUGINTENTHOLES (SR507): the outline as loops.

/// The measured plate's two holes: 6.4 mm across, centred on the plate's mid-height.
const HOLES: [(f64, f64); 2] = [(10.0, 15.1), (39.7, 15.1)];

/// The plate as drawn from the photo: BASE plus a centre point and a 3.2 circle per hole (ids k1 / k2: one
/// namespace, and SIX already takes h1 / h2).
fn plate() -> Doc {
    let mut t = String::from(BASE);
    for (k, (x, y)) in HOLES.iter().enumerate() {
        let n = k + 1;
        t.push_str(&format!("point c{n} sketch=s x={x} y={y}\ncircle k{n} sketch=s centre=c{n} r=3.2\n"));
    }
    read(&t).unwrap()
}

/// A closed CW circle polyline (a hole as photo_core gives it), `n` samples.
fn hole_outline(c: (f64, f64), r: f64, n: usize) -> Vec<Vec2> {
    (0..n)
        .map(|k| {
            let a = -std::f64::consts::TAU * k as f64 / n as f64;
            Vec2::new(c.0 + r * a.cos(), c.1 + r * a.sin())
        })
        .collect()
}

/// The measured plate: the 49.7 x 30.2 outer (CCW) then the two 6.4 holes (CW).
fn plate_outline() -> Measurement {
    let mut loops = vec![rect_outline(49.7, 30.2, 5.0)];
    for &c in &HOLES {
        loops.push(hole_outline(c, 3.2, 64));
    }
    Measurement::Outline { loops, sigma_mm: SIGMA }
}

#[test]
fn plate_with_holes_accepts_diameters_and_refuses_a_moved_hole() {
    let d = plate();
    let mut v: Vec<&str> = SIX.to_vec();
    v.push("diameter dh1 sketch=s circle=k1 d=6.5");
    v.push("diameter dh2 sketch=s circle=k2 d=6.5");
    let b = verify_all(&d, &ps(&v), &plate_outline(), d.tol);
    assert_eq!(b.accepted(), 8, "{}", b.text());
    for (s, _, v) in &b.entries {
        if let Verdict::Accept { max_mm, .. } = v {
            assert!(*max_mm <= K_SIGMA * SIGMA, "{s}: max {max_mm}");
        }
    }
    let l = edge_lengths(&b.doc);
    for (got, w) in l.iter().zip([30.0, 30.0, 50.0, 50.0]) {
        assert!((got - w).abs() <= 10.0 * d.tol.lin, "edges {l:?}");
    }
    // A 3 mm move of hole 1: paired with its measured hole, then past K_SIGMA x sigma.
    match verify(&b.doc, &p("fix f9 sketch=s p=c1 at=[13 15.1]"), &plate_outline(), b.doc.tol) {
        Verdict::Refuse { why: Reason::Residual { max_mm, limit_mm, .. } } => {
            assert_eq!(limit_mm, K_SIGMA * SIGMA);
            assert!(max_mm > 2.0 && max_mm < 4.0, "max {max_mm}");
        }
        v => panic!("{v:?}"),
    }
}

#[test]
fn hole_without_a_measured_loop_refused_by_name() {
    let d = read(&format!("{BASE}point c1 sketch=s x=10 y=15.1\ncircle k1 sketch=s centre=c1 r=3.2\n")).unwrap();
    let v = verify(&d, &p(SIX[0]), &outline(), d.tol);
    assert_eq!(v, Verdict::Refuse { why: Reason::UnmatchedLoop { replayed: 2, measured: 1 } });
    if let Verdict::Refuse { why } = v {
        assert_eq!(why.text(), "unmatched-loop replayed=2 measured=1");
    }
}

// VUGINTENTPOINTS (SR523): the two-sided Points residual.

/// The 50 x 30 x 6 plate (BASE + SIX + fix), a r 5 boss at (25, 15) on its top (length 8.2 as drawn), and a r 0.4 pin
/// at (40, 8) (length 0.5): three solids (the document has no booleans).
fn boss_doc() -> Doc {
    let text = format!(
        "{BASE}{}\nfix f1 sketch=s p=p1 at=[0 0]\nextrude e1 sketch=s outer=l1 dir=[0 0 1] length=6\n\
         sketch t o=[0 0 6] z=[0 0 1] x=[1 0 0]\npoint c sketch=t x=25 y=15\nfix f2 sketch=t p=c at=[25 15]\n\
         circle k sketch=t centre=c r=5\nextrude e2 sketch=t outer=k dir=[0 0 1] length=8.2\n\
         sketch u o=[0 0 6] z=[0 0 1] x=[1 0 0]\npoint q sketch=u x=40 y=8\nfix f3 sketch=u p=q at=[40 8]\n\
         circle n sketch=u centre=q r=0.4\nextrude e3 sketch=u outer=n dir=[0 0 1] length=0.5\n",
        SIX.join("\n")
    );
    read(&text).unwrap()
}

/// The part as scanned at 1 mm: the plate top outside the boss, the four plate walls, the boss wall and its top at
/// z = 14 (the real boss is 8 tall), the pin's top and wall. No bottom: it sat on the table.
fn boss_scan() -> Vec<Vec3> {
    let mut v = Vec::new();
    let r = |x: f64, y: f64, cx: f64, cy: f64| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
    for i in 0..50 {
        for j in 0..30 {
            let (x, y) = (i as f64 + 0.5, j as f64 + 0.5);
            if r(x, y, 25.0, 15.0) >= 5.0 && r(x, y, 40.0, 8.0) >= 0.4 {
                v.push(Vec3::new(x, y, 6.0));
            }
            if r(x, y, 25.0, 15.0) < 5.0 {
                v.push(Vec3::new(x, y, 14.0));
            }
        }
    }
    for k in 0..6 {
        let z = k as f64 + 0.5;
        for j in 0..30 {
            v.push(Vec3::new(0.0, j as f64 + 0.5, z));
            v.push(Vec3::new(50.0, j as f64 + 0.5, z));
        }
        for i in 0..50 {
            v.push(Vec3::new(i as f64 + 0.5, 0.0, z));
            v.push(Vec3::new(i as f64 + 0.5, 30.0, z));
        }
    }
    for k in 0..8 {
        for a in 0..32 {
            let t = a as f64 * std::f64::consts::TAU / 32.0;
            v.push(Vec3::new(25.0 + 5.0 * t.cos(), 15.0 + 5.0 * t.sin(), 6.5 + k as f64));
        }
    }
    v.push(Vec3::new(40.0, 8.0, 6.5));
    for a in 0..4 {
        let t = a as f64 * std::f64::consts::FRAC_PI_2;
        v.push(Vec3::new(40.0 + 0.4 * t.cos(), 8.0 + 0.4 * t.sin(), 6.25));
    }
    v
}

#[test]
fn points_two_sided_boss_removed_and_pin_grown() {
    let d = boss_doc();
    let m = Measurement::Points { pts: boss_scan(), sigma_mm: SIGMA };
    // (i) The boss's round height: accepted, the bottom unscanned (the document's own surface is never held against
    // a proposal) and the boss top moved 0.2 < sigma.
    match verify(&d, &p("extrude e2 sketch=t outer=k dir=[0 0 1] length=8"), &m, d.tol) {
        Verdict::Accept { max_mm, .. } => assert!(max_mm < 0.1, "max {max_mm}"),
        v => panic!("{v:?}"),
    }
    // (ii) The boss removed to a 0.5 mm stub: its scan points hang 7.5 mm above the model.
    match verify(&d, &p("extrude e2 sketch=t outer=k dir=[0 0 1] length=0.5"), &m, d.tol) {
        Verdict::Refuse { why: Reason::Residual { max_mm, .. } } => assert!(max_mm > 7.0, "max {max_mm}"),
        v => panic!("{v:?}"),
    }
    // (iii) The pin grown 9.5 mm where no scan point is: every point stays within 0.4 mm of the model (the forward
    // side alone accepts), the new pin top is ~9.5 mm from any point, so the refusal is the surface side's.
    match verify(&d, &p("extrude e3 sketch=u outer=n dir=[0 0 1] length=10"), &m, d.tol) {
        Verdict::Refuse { why: Reason::Residual { max_mm, .. } } => assert!(max_mm > 7.0, "max {max_mm}"),
        v => panic!("{v:?}"),
    }
}

#[test]
fn points_two_sided_100k_points_2k_triangles() {
    // A disc r 60 (drawn 60.02) extruded 20; sigma 0.01 -> chord 0.0025 -> 487 arc pieces, ~1944 triangles.
    let text = "vug-doc version=1 lin=1e-6 ang=1e-9
sketch s o=[0 0 0] z=[0 0 1] x=[1 0 0]
point c sketch=s x=0 y=0
fix f1 sketch=s p=c at=[0 0]
circle k sketch=s centre=c r=60.02
extrude e1 sketch=s outer=k dir=[0 0 1] length=20
";
    let d = read(text).unwrap();
    let sigma = 0.01;
    let (r, h, step) = (60.0f64, 20.0f64, 0.54f64);
    let mut pts = Vec::new();
    let nz = 36;
    let na = (std::f64::consts::TAU * r / step).round() as usize;
    for k in 0..nz {
        let z = (k as f64 + 0.5) * h / nz as f64;
        for a in 0..na {
            let t = a as f64 * std::f64::consts::TAU / na as f64;
            pts.push(Vec3::new(r * t.cos(), r * t.sin(), z));
        }
    }
    let n = (2.0 * r / step) as i32;
    for z in [0.0, h] {
        for i in 0..n {
            for j in 0..n {
                let (x, y) = (-r + (i as f64 + 0.5) * step, -r + (j as f64 + 0.5) * step);
                if (x * x + y * y).sqrt() < r - step * 0.5 {
                    pts.push(Vec3::new(x, y, z));
                }
            }
        }
    }
    assert!(pts.len() >= 100_000, "lattice {}", pts.len());
    pts.truncate(100_000);
    let chord = (sigma * 0.25f64).max(4.0 * d.tol.lin);
    let rep = replay(&d, d.tol);
    let tris = brep_core::tessellate(&rep.solids[0].brep, chord, d.tol).unwrap().mesh.triangles.len();
    let m = Measurement::Points { pts, sigma_mm: sigma };
    let clock = std::time::Instant::now();
    let v = verify(&d, &p("radius r1 sketch=s circle=k r=60"), &m, d.tol);
    let ms = clock.elapsed().as_secs_f64() * 1e3;
    println!("intent-points: n=100000 tris={tris} verify_ms={ms:.1}");
    assert!((1500..=2500).contains(&tris), "tris {tris}");
    match v {
        Verdict::Accept { max_mm, .. } => assert!(max_mm < 3.0 * sigma, "max {max_mm}"),
        v => panic!("{v:?}"),
    }
    assert!(ms < 20_000.0, "verify took {ms} ms: the BVH road is not in use");
}
