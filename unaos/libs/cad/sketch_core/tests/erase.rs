// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGSKETCHERASE (SR495): removal by cascade leaves stable ids and a solvable sketch with the dof of what is left;
//! the existing constraint `remove` gives its dof back; the parameter setters write without solving.

use core::f64::consts::PI;
use sketch_core::*;

const T: Tol = Tol::DEFAULT;

fn near(a: Vec2, x: f64, y: f64, eps: f64) -> bool {
    (a.x - x).abs() <= eps && (a.y - y).abs() <= eps
}

fn pt(sk: &Sketch, id: EntityId) -> Vec2 {
    sk.point(PointRef::Point(id)).unwrap()
}

/// Points 0..3, lines 4..7 (p0->p1->p2->p3->p0); constraints 0 H(l0), 1 V(l1), 2 H(l2), 3 V(l3), 4 Fix p0,
/// 5 Distance p0-p1 10, 6 Distance p1-p2 5.
fn full_rect(sk: &mut Sketch) -> ([EntityId; 4], [EntityId; 4]) {
    let p = [(0.3, -0.2), (9.1, 0.4), (10.6, 5.5), (-0.4, 4.7)].map(|(x, y)| sk.add_point(x, y));
    let l = [0, 1, 2, 3].map(|i| sk.add_line(p[i], p[(i + 1) % 4]).unwrap());
    sk.add(Constraint::Horizontal(l[0])).unwrap();
    sk.add(Constraint::Vertical(l[1])).unwrap();
    sk.add(Constraint::Horizontal(l[2])).unwrap();
    sk.add(Constraint::Vertical(l[3])).unwrap();
    sk.add(Constraint::Fix(PointRef::Point(p[0]), Vec2::new(0.0, 0.0))).unwrap();
    sk.add(Constraint::Distance(PointRef::Point(p[0]), PointRef::Point(p[1]), 10.0)).unwrap();
    sk.add(Constraint::Distance(PointRef::Point(p[1]), PointRef::Point(p[2]), 5.0)).unwrap();
    (p, l)
}

#[test]
fn removing_a_point_cascades_its_lines_and_constraints_and_still_solves() {
    let mut sk = Sketch::new();
    let (p, l) = full_rect(&mut sk);
    assert_eq!(solve(&mut sk, T).expect("solves").dof, 0);
    let n = sk.entities().len();
    let r = sk.remove_entity(p[2]).expect("a live point");
    assert_eq!(r.entities, vec![p[2], l[1], l[2]], "the point, then the two lines built on it");
    assert_eq!(r.constraints, vec![ConstraintId(1), ConstraintId(2), ConstraintId(6)], "V(l1), H(l2), Distance p1-p2");
    assert_eq!(sk.entities().len(), n, "a tombstone, not a renumbering");
    for e in &r.entities {
        assert_eq!(sk.entity(*e), Some(&Entity::Removed));
    }
    for c in &r.constraints {
        assert!(sk.constraint(*c).is_none());
    }
    assert!(sk.point(PointRef::Point(p[2])).is_none());
    let s = solve(&mut sk, T).expect("what is left still solves");
    assert_eq!(s.dof, 1, "p3 slides along the vertical l3");
    let d = dof(&sk);
    assert_eq!(d.count, 1);
    assert_eq!(d.free, vec![p[3], l[3]]);
    assert!(near(pt(&sk, p[0]), 0.0, 0.0, 1e-6));
    assert!(near(pt(&sk, p[1]), 10.0, 0.0, 1e-6));
    assert!(profiles(&sk, T).is_empty(), "the loop is open");
}

#[test]
fn removing_a_constraint_gives_back_its_dof() {
    let mut sk = Sketch::new();
    let (p, _) = full_rect(&mut sk);
    assert_eq!(solve(&mut sk, T).expect("solves").dof, 0);
    assert!(sk.remove(ConstraintId(6)), "the existing remove is the constraint remover");
    assert_eq!(dof(&sk).count, 1);
    assert_eq!(solve(&mut sk, T).expect("solves").dof, 1);
    assert!(!sk.remove(ConstraintId(6)), "a hole is removed once");
    sk.remove_entity(p[2]).expect("a live point");
    assert_eq!(sk.remove_entity(p[2]), Err(BadRef { id: p[2], kind: RefKind::Live }), "a tombstone is no live entity");
    assert_eq!(sk.remove_entity(EntityId(99)), Err(BadRef { id: EntityId(99), kind: RefKind::Live }));
}

#[test]
fn set_radius_then_solve_holds_a_radius_constraint() {
    let mut sk = Sketch::new();
    let c = sk.add_point(1.0, 2.0);
    let circle = sk.add_circle(c, 5.0).unwrap();
    sk.add(Constraint::Fix(PointRef::Point(c), Vec2::new(1.0, 2.0))).unwrap();
    sk.add(Constraint::Radius(circle, 5.0)).unwrap();
    let c2 = sk.add_point(20.0, 0.0);
    let free = sk.add_circle(c2, 3.0).unwrap();
    sk.set_radius(circle, 9.0).unwrap();
    sk.set_radius(free, 9.0).unwrap();
    assert_eq!(sk.radius(circle), Some(9.0), "a direct write: no solve");
    solve(&mut sk, T).expect("solves");
    assert!((sk.radius(circle).unwrap() - 5.0).abs() < 1e-6, "the Radius constraint holds");
    assert_eq!(sk.radius(free), Some(9.0), "nothing constrains it: the minimum move leaves it");
    let line = sk.add_line(c, c2).unwrap();
    assert_eq!(sk.set_point(line, 0.0, 0.0), Err(BadRef { id: line, kind: RefKind::Point }));
    assert_eq!(sk.set_radius(line, 1.0), Err(BadRef { id: line, kind: RefKind::Round }));
    assert_eq!(sk.set_arc_angles(free, 0.0, 1.0), Err(BadRef { id: free, kind: RefKind::Arc }), "a circle has no angles");
    sk.set_point(c2, 21.0, 1.0).unwrap();
    assert!(near(pt(&sk, c2), 21.0, 1.0, 0.0));
    let arc = sk.add_arc(c2, 2.0, 0.0, 1.0).unwrap();
    sk.set_arc_angles(arc, 0.0, PI / 2.0).unwrap();
    assert!(near(sk.point(PointRef::Start(arc)).unwrap(), 23.0, 1.0, 1e-12));
    assert!(near(sk.point(PointRef::End(arc)).unwrap(), 21.0, 3.0, 1e-12));
}

#[test]
fn ids_stay_stable_across_a_removal() {
    let mut sk = Sketch::new();
    let p0 = sk.add_point(0.0, 0.0);
    let p1 = sk.add_point(5.0, 0.0);
    let line = sk.add_line(p0, p1).unwrap();
    let c = sk.add(Constraint::Distance(PointRef::Point(p0), PointRef::Point(p1), 5.0)).unwrap();
    let r = sk.remove_entity(p0).unwrap();
    assert_eq!(r, Removed { entities: vec![p0, line], constraints: vec![c] });
    let p2 = sk.add_point(1.0, 1.0);
    assert_eq!(p2, EntityId(3), "ids are never reused");
    assert_eq!(sk.add_line(p0, p1), Err(BadRef { id: p0, kind: RefKind::Point }));
    assert_eq!(sk.add(Constraint::Fix(PointRef::Point(p0), Vec2::new(0.0, 0.0))), Err(BadRef { id: p0, kind: RefKind::Point }));
    assert_eq!(sk.add(Constraint::Fix(PointRef::Point(p1), Vec2::new(5.0, 0.0))), Ok(ConstraintId(1)), "nor are constraint ids");
    assert!(sk.remove(ConstraintId(1)));
    assert_eq!(sk.params().len(), 6, "the frozen parameters stay in place");
    let d = dof(&sk);
    assert_eq!(d.count, 4, "only the live points count");
    assert_eq!(d.free, vec![p1, p2]);
    assert_eq!(solve(&mut sk, T).expect("solves").dof, 4);
    assert!(near(pt(&sk, p1), 5.0, 0.0, 0.0));
}
