// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGSKETCH (SR465) M2: the solve, DOF, redundancy named and conflicts refused by name.

use core::f64::consts::PI;
use sketch_core::*;

const T: Tol = Tol::DEFAULT;

fn near(a: Vec2, x: f64, y: f64, eps: f64) -> bool {
    (a.x - x).abs() <= eps && (a.y - y).abs() <= eps
}

fn pt(sk: &Sketch, id: EntityId) -> Vec2 {
    sk.point(PointRef::Point(id)).unwrap()
}

/// Four points (drawn roughly), four lines p0->p1->p2->p3->p0, two Horizontal and two Vertical.
fn rect(sk: &mut Sketch) -> ([EntityId; 4], [EntityId; 4]) {
    let p = [(0.3, -0.2), (9.1, 0.4), (10.6, 5.5), (-0.4, 4.7)].map(|(x, y)| sk.add_point(x, y));
    let l = [0, 1, 2, 3].map(|i| sk.add_line(p[i], p[(i + 1) % 4]).unwrap());
    sk.add(Constraint::Horizontal(l[0])).unwrap();
    sk.add(Constraint::Vertical(l[1])).unwrap();
    sk.add(Constraint::Horizontal(l[2])).unwrap();
    sk.add(Constraint::Vertical(l[3])).unwrap();
    (p, l)
}

fn full_rect(sk: &mut Sketch) -> ([EntityId; 4], [EntityId; 4]) {
    let (p, l) = rect(sk);
    sk.add(Constraint::Fix(PointRef::Point(p[0]), Vec2::new(0.0, 0.0))).unwrap();
    sk.add(Constraint::Distance(PointRef::Point(p[0]), PointRef::Point(p[1]), 10.0)).unwrap();
    sk.add(Constraint::Distance(PointRef::Point(p[1]), PointRef::Point(p[2]), 5.0)).unwrap();
    (p, l)
}

#[test]
fn rectangle_fully_constrained_has_no_dof() {
    let mut sk = Sketch::new();
    let (p, _) = full_rect(&mut sk);
    let s = solve(&mut sk, T).expect("solves");
    assert_eq!(s.dof, 0);
    assert!(s.redundant.is_empty(), "{:?}", s.redundant);
    assert!(s.residual <= 1.0);
    for (i, (x, y)) in [(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)].into_iter().enumerate() {
        assert!(near(pt(&sk, p[i]), x, y, 1e-6), "p{i} = {:?}", pt(&sk, p[i]));
    }
    let d = dof(&sk);
    assert_eq!(d.count, 0);
    assert!(d.free.is_empty(), "{:?}", d.free);
}

#[test]
fn under_constrained_rectangle_names_its_free_entities() {
    let mut sk = Sketch::new();
    let (p, l) = rect(&mut sk);
    sk.add(Constraint::Fix(PointRef::Point(p[0]), Vec2::new(0.0, 0.0))).unwrap();
    let s = solve(&mut sk, T).expect("solves");
    assert_eq!(s.dof, 2); // width and height
    let d = dof(&sk);
    assert_eq!(d.count, 2);
    assert!(!d.free.contains(&p[0]), "the fixed corner is not free: {:?}", d.free);
    for id in [p[1], p[2], p[3], l[0], l[1], l[2], l[3]] {
        assert!(d.free.contains(&id), "{id:?} free: {:?}", d.free);
    }
}

#[test]
fn slot_two_lines_two_tangent_arcs() {
    let mut sk = Sketch::new();
    let c1 = sk.add_point(0.2, -0.1);
    let c2 = sk.add_point(19.0, 0.6);
    let a1 = sk.add_arc(c1, 4.6, PI / 2.0 + 0.05, 1.5 * PI - 0.04).unwrap();
    let a2 = sk.add_arc(c2, 5.3, -PI / 2.0 + 0.03, PI / 2.0 - 0.06).unwrap();
    let b0 = sk.add_point(0.1, -4.8);
    let b1 = sk.add_point(19.5, -5.2);
    let t0 = sk.add_point(20.4, 5.1);
    let t1 = sk.add_point(-0.3, 4.9);
    let bot = sk.add_line(b0, b1).unwrap();
    let top = sk.add_line(t0, t1).unwrap();
    use Constraint::*;
    sk.add(Coincident(PointRef::End(a1), PointRef::Point(b0))).unwrap();
    sk.add(Coincident(PointRef::Start(a2), PointRef::Point(b1))).unwrap();
    sk.add(Coincident(PointRef::End(a2), PointRef::Point(t0))).unwrap();
    sk.add(Coincident(PointRef::Start(a1), PointRef::Point(t1))).unwrap();
    sk.add(TangentLineCircle(bot, a1)).unwrap();
    sk.add(TangentLineCircle(bot, a2)).unwrap();
    sk.add(TangentLineCircle(top, a2)).unwrap();
    sk.add(TangentLineCircle(top, a1)).unwrap();
    sk.add(EqualRadius(a1, a2)).unwrap();
    sk.add(Horizontal(bot)).unwrap();
    // Before the dimensions: the slot's shape is free (two centres and a radius, rotation pinned by Horizontal).
    let s = solve(&mut sk.clone(), T).expect("free slot solves");
    assert_eq!(s.dof, 4, "x, y, length and radius");
    assert!(s.redundant.is_empty(), "endpoint tangency is not redundant: {:?}", s.redundant);
    sk.add(Fix(PointRef::Centre(a1), Vec2::new(0.0, 0.0))).unwrap();
    sk.add(Radius(a1, 5.0)).unwrap();
    sk.add(Distance(PointRef::Centre(a1), PointRef::Centre(a2), 20.0)).unwrap();
    let s = solve(&mut sk, T).expect("slot solves");
    assert_eq!(s.dof, 0);
    assert!(s.redundant.is_empty(), "{:?}", s.redundant);
    assert!(near(pt(&sk, c2), 20.0, 0.0, 1e-6), "{:?}", pt(&sk, c2));
    assert!(near(pt(&sk, b0), 0.0, -5.0, 1e-6), "{:?}", pt(&sk, b0));
    assert!(near(pt(&sk, t0), 20.0, 5.0, 1e-6), "{:?}", pt(&sk, t0));
    assert!((sk.radius(a2).unwrap() - 5.0).abs() < 1e-6);
    let pr = profiles(&sk, T);
    assert_eq!(pr.len(), 1);
    assert_eq!(pr[0].outer.edges.len(), 4);
    assert!(pr[0].holes.is_empty());
    assert!((pr[0].outer.area - (200.0 + 25.0 * PI)).abs() < 1e-6, "{}", pr[0].outer.area);
}

#[test]
fn conflicting_pair_is_refused_by_name_and_the_sketch_is_unchanged() {
    let mut sk = Sketch::new();
    let p = sk.add_point(0.0, 0.0);
    let q = sk.add_point(12.0, 0.0);
    sk.add(Constraint::Fix(PointRef::Point(p), Vec2::new(0.0, 0.0))).unwrap();
    let a = sk.add(Constraint::Distance(PointRef::Point(p), PointRef::Point(q), 10.0)).unwrap();
    let b = sk.add(Constraint::Distance(PointRef::Point(p), PointRef::Point(q), 20.0)).unwrap();
    let before = sk.params().to_vec();
    assert_eq!(solve(&mut sk, T), Err(Refusal::Conflicting(vec![a, b])));
    assert_eq!(sk.params(), &before[..]);
    // Remove one and it solves.
    assert!(sk.remove(b));
    solve(&mut sk, T).expect("solves without the conflict");
    assert!(near(pt(&sk, q), 10.0, 0.0, 1e-6));
}

#[test]
fn redundant_constraint_is_named_not_refused_and_strict_refuses_it() {
    let mut sk = Sketch::new();
    let (_, l) = full_rect(&mut sk);
    let par = sk.add(Constraint::Parallel(l[0], l[2])).unwrap(); // implied by the two Horizontals
    let mut strict = sk.clone();
    let s = solve(&mut sk, T).expect("a redundant constraint is not a refusal");
    assert_eq!(s.redundant, vec![par]);
    assert_eq!(s.dof, 0);
    assert_eq!(solve_strict(&mut strict, T), Err(Refusal::Redundant(vec![par])));
}

#[test]
fn angle_perpendicular_midpoint_symmetric() {
    use Constraint::*;
    let mut sk = Sketch::new();
    let o = sk.add_point(0.0, 0.0);
    let a = sk.add_point(10.0, 1.0);
    let b = sk.add_point(1.0, 8.0);
    let c = sk.add_point(-3.0, 2.0);
    let la = sk.add_line(o, a).unwrap();
    let lb = sk.add_line(o, b).unwrap();
    let lc = sk.add_line(o, c).unwrap();
    sk.add(Fix(PointRef::Point(o), Vec2::new(0.0, 0.0))).unwrap();
    sk.add(Horizontal(la)).unwrap();
    sk.add(Angle(la, lb, PI / 3.0)).unwrap();
    sk.add(Perpendicular(la, lc)).unwrap();
    let m = sk.add_point(3.0, 3.0);
    sk.add(Midpoint(PointRef::Point(m), la)).unwrap();
    let axis0 = sk.add_point(0.0, -1.0);
    let axis1 = sk.add_point(0.0, 10.0);
    let axis = sk.add_line(axis0, axis1).unwrap();
    sk.add(Fix(PointRef::Point(axis0), Vec2::new(0.0, -1.0))).unwrap();
    sk.add(Fix(PointRef::Point(axis1), Vec2::new(0.0, 10.0))).unwrap();
    let p = sk.add_point(3.0, 1.0);
    let q = sk.add_point(-2.0, 2.0);
    sk.add(Fix(PointRef::Point(p), Vec2::new(3.0, 1.0))).unwrap();
    sk.add(Symmetric(PointRef::Point(p), PointRef::Point(q), axis)).unwrap();
    solve(&mut sk, T).expect("solves");
    let (pa, pb, pc) = (pt(&sk, a), pt(&sk, b), pt(&sk, c));
    assert!(pa.y.abs() < 1e-6);
    let ang = (pa.x * pb.y - pa.y * pb.x).atan2(pa.x * pb.x + pa.y * pb.y);
    assert!((ang - PI / 3.0).abs() < 1e-9, "{ang}");
    assert!((pa.x * pc.x + pa.y * pc.y).abs() / (pa.x.hypot(pa.y) * pc.x.hypot(pc.y)) < 1e-9);
    assert!(near(pt(&sk, m), pa.x / 2.0, pa.y / 2.0, 1e-6));
    assert!(near(pt(&sk, q), -3.0, 1.0, 1e-6), "{:?}", pt(&sk, q));
}

#[test]
fn circles_tangent_diameter_point_on_circle_and_line_distances() {
    use Constraint::*;
    let mut sk = Sketch::new();
    let o = sk.add_point(0.1, 0.0);
    let c1 = sk.add_circle(o, 4.0).unwrap();
    let k = sk.add_point(12.0, 1.0);
    let c2 = sk.add_circle(k, 3.0).unwrap();
    sk.add(Fix(PointRef::Point(o), Vec2::new(0.0, 0.0))).unwrap();
    sk.add(Diameter(c1, 10.0)).unwrap();
    sk.add(TangentCircles(c1, c2)).unwrap();
    let on = sk.add_point(1.0, 4.0);
    sk.add(PointOnCircle(PointRef::Point(on), c1)).unwrap();
    let x0 = sk.add_point(0.0, 0.0);
    let x1 = sk.add_point(10.0, 0.0);
    let ax = sk.add_line(x0, x1).unwrap();
    sk.add(Fix(PointRef::Point(x0), Vec2::new(0.0, 0.0))).unwrap();
    sk.add(Fix(PointRef::Point(x1), Vec2::new(10.0, 0.0))).unwrap();
    let d = sk.add_point(3.0, 4.0);
    sk.add(DistancePointLine(PointRef::Point(d), ax, 2.0)).unwrap();
    let e0 = sk.add_point(1.0, -3.0);
    let e1 = sk.add_point(4.0, -3.5);
    let le = sk.add_line(e0, e1).unwrap();
    sk.add(PointOnLine(PointRef::Point(e0), ax)).unwrap();
    sk.add(EqualLength(le, ax)).unwrap();
    solve(&mut sk, T).expect("solves");
    assert!((sk.radius(c1).unwrap() - 5.0).abs() < 1e-6);
    let (pk, r2) = (pt(&sk, k), sk.radius(c2).unwrap());
    assert!((pk.x.hypot(pk.y) - (5.0 + r2)).abs() < 1e-6, "external tangency kept");
    assert!((pt(&sk, on).x.hypot(pt(&sk, on).y) - 5.0).abs() < 1e-6);
    assert!((pt(&sk, d).y - 2.0).abs() < 1e-6, "the side it was drawn on");
    assert!(pt(&sk, e0).y.abs() < 1e-6);
    let (q0, q1) = (pt(&sk, e0), pt(&sk, e1));
    assert!(((q1.x - q0.x).hypot(q1.y - q0.y) - 10.0).abs() < 1e-6);
}

#[test]
fn bad_references_are_refused_by_name() {
    let mut sk = Sketch::new();
    let p = sk.add_point(0.0, 0.0);
    assert_eq!(sk.add_line(p, EntityId(9)), Err(BadRef(EntityId(9))));
    assert_eq!(sk.add(Constraint::Horizontal(p)), Err(BadRef(p)));
    assert_eq!(sk.add(Constraint::Radius(p, 1.0)), Err(BadRef(p)));
}

#[test]
fn degenerate_zero_length_line_does_not_panic() {
    let mut sk = Sketch::new();
    let a = sk.add_point(5.0, 5.0);
    let b = sk.add_point(5.0, 5.0);
    let c = sk.add_point(0.0, 0.0);
    let d = sk.add_point(3.0, 4.0);
    let l0 = sk.add_line(a, b).unwrap();
    let l1 = sk.add_line(c, d).unwrap();
    sk.add(Constraint::Parallel(l0, l1)).unwrap();
    sk.add(Constraint::Distance(PointRef::Point(a), PointRef::Point(b), 0.0)).unwrap();
    sk.add(Constraint::PointOnLine(PointRef::Point(c), l0)).unwrap();
    let _ = solve(&mut sk, T);
    let _ = dof(&sk);
    let _ = profiles(&sk, T);
}
