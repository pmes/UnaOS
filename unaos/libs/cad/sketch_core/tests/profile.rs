// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGSKETCH (SR465) M2: profiles — closed loops, holes by nesting, orientation normalised, exact areas.

use core::f64::consts::PI;
use sketch_core::*;

const T: Tol = Tol::DEFAULT;

fn square(sk: &mut Sketch, pts: [(f64, f64); 4]) {
    let p = pts.map(|(x, y)| sk.add_point(x, y));
    for i in 0..4 {
        sk.add_line(p[i], p[(i + 1) % 4]).unwrap();
    }
}

#[test]
fn square_with_a_circular_hole_and_an_island() {
    let mut sk = Sketch::new();
    // Drawn clockwise on purpose: the outer loop comes back CCW.
    square(&mut sk, [(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)]);
    let c = sk.add_point(5.0, 5.0);
    sk.add_circle(c, 2.0).unwrap();
    // An island inside the hole (CCW as drawn).
    square(&mut sk, [(4.5, 4.5), (5.5, 4.5), (5.5, 5.5), (4.5, 5.5)]);
    // An open chain bounds nothing.
    let d0 = sk.add_point(20.0, 0.0);
    let d1 = sk.add_point(30.0, 0.0);
    sk.add_line(d0, d1).unwrap();
    let pr = profiles(&sk, T);
    assert_eq!(pr.len(), 2, "{pr:?}");
    let outer = pr.iter().find(|p| (p.outer.area - 100.0).abs() < 1e-9).expect("the square");
    assert_eq!(outer.outer.edges.len(), 4);
    assert_eq!(outer.holes.len(), 1);
    assert!((outer.holes[0].area + 4.0 * PI).abs() < 1e-9, "hole CW: {}", outer.holes[0].area);
    let island = pr.iter().find(|p| (p.outer.area - 1.0).abs() < 1e-9).expect("the island");
    assert!(island.holes.is_empty());
    // The outer loop is continuous: each edge starts where the last ended.
    let e = &outer.outer.edges;
    for i in 0..e.len() {
        let (a, b) = (e[i].end(), e[(i + 1) % e.len()].start());
        assert!((a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
    }
}

#[test]
fn semicircle_area_is_exact() {
    let mut sk = Sketch::new();
    let c = sk.add_point(0.0, 0.0);
    let a = sk.add_point(3.0, 0.0);
    let b = sk.add_point(-3.0, 0.0);
    sk.add_arc(c, 3.0, 0.0, PI).unwrap();
    sk.add_line(b, a).unwrap();
    let pr = profiles(&sk, T);
    assert_eq!(pr.len(), 1);
    assert_eq!(pr[0].outer.edges.len(), 2);
    assert!((pr[0].outer.area - 4.5 * PI).abs() < 1e-9, "{}", pr[0].outer.area);
}

#[test]
fn a_branch_bounds_nothing() {
    let mut sk = Sketch::new();
    let p = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0)].map(|(x, y)| sk.add_point(x, y));
    sk.add_line(p[0], p[1]).unwrap();
    sk.add_line(p[1], p[2]).unwrap();
    sk.add_line(p[2], p[0]).unwrap();
    let spur = sk.add_point(9.0, 9.0);
    sk.add_line(p[2], spur).unwrap();
    assert!(profiles(&sk, T).is_empty(), "a degree-3 node drops its chain");
}
