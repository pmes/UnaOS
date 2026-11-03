//! VUGINTENT (LEDGER SR505): a proposal is applied to a copy of the document, replayed, and accepted only inside the
//! measurement's confidence (BP-vug-7). A measured 49.7 x 30.2 rectangle (sigma 0.3) becomes exactly 50 x 30 through
//! accepted proposals; a dimension the measurement does not support, a conflicting constraint, a coordinate the
//! measurement did not give, a redundant constraint and a bad line are each refused by name.

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
    Measurement::Outline { pts: rect_outline(49.7, 30.2, 5.0), sigma_mm: SIGMA }
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
        Verdict::Refuse { why: Reason::Residual { max_mm, limit_mm } } => {
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
    let empty = Measurement::Outline { pts: Vec::new(), sigma_mm: SIGMA };
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
