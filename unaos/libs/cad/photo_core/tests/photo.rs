// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGPHOTOGRID (LEDGER SR503): photo_core against its synthetic fixture (`photo_core::fixture`) and the steps' KATs.

use photo_core::fixture::{self, CORNER_R, HOLE_D, HOLES, PART_H, PART_W, Spec};
use photo_core::homography::{apply, dlt};
use photo_core::{Measured, PhotoParams, Prim, Refusal, Tol, Vec2, contour, fit, measure, silhouette};
use sketch_core::{Constraint, Entity, EntityId, PointRef, dof, profiles, solve};

fn run(s: &Spec) -> Result<Measured, Refusal> {
    let png = fixture::png(s);
    let img = pixel_core::decode(&png).expect("pixel_core decodes the fixture PNG");
    measure(&img, PhotoParams::new(5.0), Tol::DEFAULT)
}

/// The outer loop's lines, by length.
fn lines(m: &Measured) -> Vec<(Vec2, Vec2)> {
    m.shape.outer.iter().filter_map(|p| if let Prim::Line { a, b, .. } = *p { Some((a, b)) } else { None }).collect()
}

fn nearest_len(ls: &[(Vec2, Vec2)], want: f64) -> (Vec2, Vec2) {
    *ls.iter().min_by(|x, y| (x.0.dist(x.1) - want).abs().total_cmp(&(y.0.dist(y.1) - want).abs())).unwrap()
}

fn shared(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> (Vec2, Vec2, Vec2) {
    // (common corner, a's other end, b's other end)
    let pairs = [(a.0, a.1, b.0, b.1), (a.0, a.1, b.1, b.0), (a.1, a.0, b.0, b.1), (a.1, a.0, b.1, b.0)];
    let best = pairs.iter().min_by(|x, y| x.0.dist(x.2).total_cmp(&y.0.dist(y.2))).unwrap();
    ((best.0 + best.2) * 0.5, best.1, best.3)
}

#[test]
fn good_photo_measures_the_part() {
    let m = run(&Spec::good()).expect("the good photo measures");
    eprintln!(
        "photo: grid={}x{} pitch_px={:.2} residual_px={:.4} residual_mm={:.4} sigma_mm={:.4} tilt_deg={:.2} lens_hint={:.4} line_width_mm={:.3} inter={} inliers={} prims={:?} dof={}",
        m.grid.0, m.grid.1, m.pitch_px, m.residual_px, m.residual_mm, m.sigma_mm, m.tilt_deg, m.lens_hint,
        m.line_width_mm, m.intersections, m.inliers, m.shape.counts(), m.solved.dof
    );
    assert!(m.sigma_mm < 0.3, "sigma_mm {}", m.sigma_mm);
    assert!(m.tilt_deg > 8.0 && m.tilt_deg < 16.0, "tilt {}", m.tilt_deg);
    assert_eq!(m.shape.counts(), (4, 1, 2), "{:?}", m.shape);
    assert_eq!(m.loops.len(), 3);
    assert_eq!(m.loops[0], m.outline);

    // The homography agrees with the truth up to the lattice's own frame: check the scale (mm per mm) instead.
    let h0 = apply(&m.h, Vec2::new(0.0, 0.0)).unwrap();
    let h1 = apply(&m.h, Vec2::new(5.0, 0.0)).unwrap();
    assert!((h0.dist(h1) - m.pitch_px).abs() < 0.25 * m.pitch_px);

    // Align the three sharp corners to the truth (2D Kabsch; a proper rotation, never a mirror).
    let ls = lines(&m);
    let l50 = nearest_len(&ls, PART_W);
    let l30 = nearest_len(&ls, PART_H);
    let (o, ex, ey) = shared(l50, l30);
    let (ux, uy) = (ex - o, ey - o);
    assert!(ux.cross(uy) > 0.0, "the measured frame is mirrored");
    let src = [o, ex, ey];
    let dst = [Vec2::new(0.0, 0.0), Vec2::new(PART_W, 0.0), Vec2::new(0.0, PART_H)];
    let cs = (src[0] + src[1] + src[2]) / 3.0;
    let cd = (dst[0] + dst[1] + dst[2]) / 3.0;
    let (mut sc, mut ss) = (0.0, 0.0);
    for k in 0..3 {
        let (a, b) = (src[k] - cs, dst[k] - cd);
        sc += a.dot(b);
        ss += a.cross(b);
    }
    let th = ss.atan2(sc);
    let to_part = |p: Vec2| {
        let q = p - cs;
        Vec2::new(th.cos() * q.x - th.sin() * q.y, th.sin() * q.x + th.cos() * q.y) + cd
    };
    for k in 0..3 {
        assert!(to_part(src[k]).dist(dst[k]) < 0.5, "corner {k}: {:?}", to_part(src[k]));
    }
    let worst = m.outline.iter().map(|&p| fixture::outline_distance(to_part(p))).fold(0.0f64, f64::max);
    assert!(worst < 0.5, "outline off the truth by {worst} mm");

    let arc_r = m.shape.outer.iter().find_map(|p| if let Prim::Arc { r, .. } = *p { Some(r) } else { None }).unwrap();
    assert!((arc_r - CORNER_R).abs() < 1.0, "corner radius {arc_r}");
    for hole in &m.shape.holes {
        let Prim::Circle { centre, r, .. } = hole[0] else { panic!("hole is not a circle: {hole:?}") };
        assert!((2.0 * r - HOLE_D).abs() < 0.5, "hole diameter {}", 2.0 * r);
        let c = to_part(centre);
        let near = HOLES.iter().map(|&(x, y)| c.dist(Vec2::new(x, y))).fold(f64::INFINITY, f64::min);
        assert!(near < 0.5, "hole centre {c:?}");
    }

    // The inferred sketch: H x2, V x2, two Coincident + two Tangent at the arc, EqualRadius: dof 10.
    assert_eq!(m.solved.dof, 10, "dof");
    assert_eq!(m.profiles.len(), 1);
    assert_eq!(m.profiles[0].holes.len(), 2);

    // Dimension it to dof 0: Fix the L50 / L30 corner, the two sides, R10, one hole's diameter, the hole centres.
    let mut sk = m.sketch.clone();
    let ents = sk.entities().to_vec();
    let id = |i: usize| EntityId(i as u32);
    let mut l_ids = Vec::new();
    let mut circles = Vec::new();
    let mut arc = None;
    for (i, e) in ents.iter().enumerate() {
        match e {
            Entity::Line { .. } => l_ids.push(id(i)),
            Entity::Circle { .. } => circles.push(id(i)),
            Entity::Arc { .. } => arc = Some(id(i)),
            _ => {}
        }
    }
    let len_of = |sk: &sketch_core::Sketch, e: EntityId| {
        sk.point(PointRef::Start(e)).unwrap().dist(sk.point(PointRef::End(e)).unwrap())
    };
    let pick = |want: f64| *l_ids.iter().min_by(|&&a, &&b| (len_of(&sk, a) - want).abs().total_cmp(&(len_of(&sk, b) - want).abs())).unwrap();
    let (s50, s30) = (pick(PART_W), pick(PART_H));
    let corner = sk.point(PointRef::Start(s50)).unwrap();
    let c50 = if sk.point(PointRef::Start(s30)).unwrap().dist(corner) < 1.0 || sk.point(PointRef::End(s30)).unwrap().dist(corner) < 1.0 {
        PointRef::Start(s50)
    } else {
        PointRef::End(s50)
    };
    let fixed = sk.point(c50).unwrap();
    sk.add(Constraint::Fix(c50, fixed)).unwrap();
    sk.add(Constraint::Distance(PointRef::Start(s50), PointRef::End(s50), PART_W)).unwrap();
    sk.add(Constraint::Distance(PointRef::Start(s30), PointRef::End(s30), PART_H)).unwrap();
    sk.add(Constraint::Radius(arc.unwrap(), CORNER_R)).unwrap();
    sk.add(Constraint::Diameter(circles[0], HOLE_D)).unwrap();
    let side = |sk: &sketch_core::Sketch, c: EntityId| sk.point(PointRef::Centre(c)).unwrap().dist(fixed);
    let (near, far) = if side(&sk, circles[0]) < side(&sk, circles[1]) { (circles[0], circles[1]) } else { (circles[1], circles[0]) };
    sk.add(Constraint::DistancePointLine(PointRef::Centre(near), s30, HOLES[0].0)).unwrap();
    sk.add(Constraint::DistancePointLine(PointRef::Centre(far), s30, HOLES[1].0)).unwrap();
    sk.add(Constraint::DistancePointLine(PointRef::Centre(near), s50, HOLES[0].1)).unwrap();
    sk.add(Constraint::DistancePointLine(PointRef::Centre(far), s50, HOLES[1].1)).unwrap();
    let solved = solve(&mut sk, Tol::DEFAULT).expect("the dimensioned sketch solves");
    assert_eq!(solved.dof, 0, "dof after ten dimensions");
    assert_eq!(dof(&sk).count, 0);
    let pr = profiles(&sk, Tol::DEFAULT);
    assert_eq!(pr.len(), 1);
    assert_eq!(pr[0].holes.len(), 2);
    assert!((pr[0].outer.area.abs() - (PART_W * PART_H - (1.0 - std::f64::consts::PI / 4.0) * CORNER_R * CORNER_R)).abs() < 1e-3);
}

#[test]
fn unsquare_refuses() {
    match run(&Spec::unsquare()) {
        Err(Refusal::GridUnsquare { deg }) => assert!(deg > 20.0, "deg {deg}"),
        other => panic!("expected GridUnsquare, got {:?}", other.map(|m| m.tilt_deg)),
    }
}

#[test]
fn no_grid_refuses() {
    match run(&Spec::no_grid()) {
        Err(Refusal::NoGrid { .. }) => {}
        other => panic!("expected NoGrid, got {:?}", other.map(|m| m.grid)),
    }
}

#[test]
fn clipped_refuses() {
    match run(&Spec::clipped()) {
        Err(Refusal::PartClipped) => {}
        other => panic!("expected PartClipped, got {:?}", other.map(|m| m.outline.len())),
    }
}

#[test]
fn dlt_kat() {
    let s = Spec::good();
    let h = fixture::homography(&s);
    let mut paper = Vec::new();
    let mut px = Vec::new();
    for i in -4..=4 {
        for j in -3..=3 {
            let p = Vec2::new(40.0 + i as f64 * 5.0, 40.0 + j as f64 * 5.0);
            paper.push(p);
            px.push(apply(&h, p).unwrap());
        }
    }
    let got = dlt(&paper, &px).unwrap();
    for (p, x) in paper.iter().zip(&px) {
        assert!(apply(&got, *p).unwrap().dist(*x) < 1e-6);
    }
}

#[test]
fn pratt_kat() {
    let pts: Vec<Vec2> = (0..40)
        .map(|k| {
            let t = 0.2 + 1.3 * k as f64 / 39.0;
            let wobble = if k % 2 == 0 { 0.02 } else { -0.02 };
            Vec2::new(3.0 + (10.0 + wobble) * t.cos(), -2.0 + (10.0 + wobble) * t.sin())
        })
        .collect();
    let c = fit::fit_circle(&pts).unwrap();
    assert!(c.c.dist(Vec2::new(3.0, -2.0)) < 0.05, "{c:?}");
    assert!((c.r - 10.0).abs() < 0.05, "{c:?}");
    assert!(c.max < 0.05);
}

#[test]
fn otsu_kat() {
    let mut hist = [0u64; 256];
    hist[40] = 300;
    hist[41] = 100;
    hist[240] = 900;
    hist[200] = 50;
    let t = silhouette::otsu(&hist);
    assert!((41..200).contains(&t), "threshold {t}");
}

#[test]
fn dp_kat() {
    // A 10 x 4 rectangle sampled every 0.5 with a 0.01 zigzag: DP at 0.1 keeps exactly the four corners.
    let mut pts = Vec::new();
    let corners = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 4.0), Vec2::new(0.0, 4.0)];
    for k in 0..4 {
        let (a, b) = (corners[k], corners[(k + 1) % 4]);
        let n = (a.dist(b) / 0.5) as usize;
        for s in 0..n {
            let t = s as f64 / n as f64;
            let z = if s % 2 == 1 { 0.01 } else { 0.0 };
            pts.push(a + (b - a) * t + (b - a).perp() * (z / a.dist(b)));
        }
    }
    let kept = contour::simplify(&pts, 0.1);
    assert_eq!(kept.len(), 4, "{kept:?}");
    for c in corners {
        assert!(kept.iter().any(|k| k.dist(c) < 1e-9));
    }
}
