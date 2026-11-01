// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGSKETCH (SR465) M2: the drag solve keeps the rest of the sketch still.

use sketch_core::*;

const T: Tol = Tol::DEFAULT;

fn pt(sk: &Sketch, id: EntityId) -> Vec2 {
    sk.point(PointRef::Point(id)).unwrap()
}

#[test]
fn drag_one_corner_keeps_the_rest_still() {
    let mut sk = Sketch::new();
    let p = [(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)].map(|(x, y)| sk.add_point(x, y));
    let l = [0, 1, 2, 3].map(|i| sk.add_line(p[i], p[(i + 1) % 4]).unwrap());
    sk.add(Constraint::Horizontal(l[0])).unwrap();
    sk.add(Constraint::Vertical(l[1])).unwrap();
    sk.add(Constraint::Horizontal(l[2])).unwrap();
    sk.add(Constraint::Vertical(l[3])).unwrap();
    sk.add(Constraint::Fix(PointRef::Point(p[0]), Vec2::new(0.0, 0.0))).unwrap();
    let loose = sk.add_point(50.0, 50.0);
    let s = drag(&mut sk, PointRef::Point(p[2]), Vec2::new(20.0, 8.0), T).expect("drags");
    assert_eq!(s.dof, 2);
    let at = |id, x: f64, y: f64| {
        let v = pt(&sk, id);
        assert!((v.x - x).abs() <= T.lin && (v.y - y).abs() <= T.lin, "{id:?} at {v:?}, want ({x}, {y})");
    };
    at(p[2], 20.0, 8.0);
    at(p[0], 0.0, 0.0);
    at(p[1], 20.0, 0.0);
    at(p[3], 0.0, 8.0);
    assert_eq!(pt(&sk, loose), Vec2::new(50.0, 50.0), "an unrelated point does not move at all");
}

#[test]
fn drag_a_fixed_point_stays_fixed() {
    let mut sk = Sketch::new();
    let a = sk.add_point(1.0, 1.0);
    sk.add(Constraint::Fix(PointRef::Point(a), Vec2::new(1.0, 1.0))).unwrap();
    drag(&mut sk, PointRef::Point(a), Vec2::new(4.0, 4.0), T).expect("solves back to the fix");
    let v = pt(&sk, a);
    assert!((v.x - 1.0).abs() <= T.lin && (v.y - 1.0).abs() <= T.lin, "{v:?}");
}

#[test]
fn drag_an_arc_end_moves_the_arc() {
    let mut sk = Sketch::new();
    let c = sk.add_point(0.0, 0.0);
    let arc = sk.add_arc(c, 5.0, 0.0, 1.0).unwrap();
    sk.add(Constraint::Fix(PointRef::Point(c), Vec2::new(0.0, 0.0))).unwrap();
    drag(&mut sk, PointRef::End(arc), Vec2::new(0.0, 7.0), T).expect("drags");
    let e = sk.point(PointRef::End(arc)).unwrap();
    assert!(e.x.abs() < 1e-6 && (e.y - 7.0).abs() < 1e-6, "{e:?}");
    assert!((sk.radius(arc).unwrap() - 7.0).abs() < 1e-6);
}
