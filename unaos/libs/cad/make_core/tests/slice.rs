// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUG-MAKE-SLICE's witness (VUGMAKESLICE, LEDGER SR483): the exact section of a cube, a holed cube and a
//! tetrahedron, the open soup refused by layer, E against the cube's volume, the Marlin round trip and the planner's
//! durations.

use std::f64::consts::PI;

use make_core::gcode::{self, Dialect};
use make_core::geom::signed_area2;
use make_core::slice::{layers, slice};
use make_core::*;

const T: Tol = Tol::DEFAULT;

fn axis(id: AxisId, min: f64, max: f64) -> Axis {
    Axis { id, min, max, max_feed: 12000.0, max_accel: 1500.0, rapid: 9000.0 }
}

fn printer() -> Machine {
    Machine {
        kind: MachineKind::Fdm3,
        kinematics: Kinematics::Cartesian,
        axes: vec![axis(AxisId::X, 0.0, 220.0), axis(AxisId::Y, 0.0, 220.0), axis(AxisId::Z, 0.0, 250.0)],
        power: Power { kind: PowerKind::Extruder, max: 280.0, ask_above: 260.0 },
        envelope: Aabb3 { min: Vec3::new(0.0, 0.0, 0.0), max: Vec3::new(220.0, 220.0, 250.0) },
        safe_z: 5.0,
        units: Units::Mm,
        tools: vec![Tool { number: 0, kind: ToolKind::Nozzle, dc: 0.4, lf: 0.0, apmx: 0.0, nof: 0, sig: 0.0, ramp_deg: None }],
        stock: None,
        fixtures: vec![],
        loaded: Some(0),
    }
}

fn v3(p: Vec2, z: f64) -> Vec3 {
    Vec3::new(p.x, p.y, z)
}

/// A prism over a convex CCW `outer` with an optional CCW-ordered `hole` (one hole corner per outer corner), walls
/// outward, caps up and down: every shared vertex has the same bits.
fn prism(outer: &[Vec2], hole: Option<&[Vec2]>, z0: f64, z1: f64) -> Vec<[Vec3; 3]> {
    let mut t = Vec::new();
    let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
        t.push([a, b, c]);
        t.push([a, c, d]);
    };
    let n = outer.len();
    for i in 0..n {
        let (a, b) = (outer[i], outer[(i + 1) % n]);
        quad(v3(a, z0), v3(b, z0), v3(b, z1), v3(a, z1));
    }
    match hole {
        Some(h) => {
            for i in 0..n {
                // The hole's walls face into the hole: walk it CW.
                let (a, b) = (h[(i + 1) % n], h[i]);
                quad(v3(a, z0), v3(b, z0), v3(b, z1), v3(a, z1));
            }
            for i in 0..n {
                let (o0, o1, h0, h1) = (outer[i], outer[(i + 1) % n], h[i], h[(i + 1) % n]);
                quad(v3(o0, z1), v3(o1, z1), v3(h1, z1), v3(h0, z1));
                quad(v3(h0, z0), v3(h1, z0), v3(o1, z0), v3(o0, z0));
            }
        }
        None => {
            for i in 1..n - 1 {
                t.push([v3(outer[0], z1), v3(outer[i], z1), v3(outer[i + 1], z1)]);
                t.push([v3(outer[0], z0), v3(outer[i + 1], z0), v3(outer[i], z0)]);
            }
        }
    }
    t
}

fn square(x0: f64, y0: f64, s: f64) -> Vec<Vec2> {
    vec![Vec2::new(x0, y0), Vec2::new(x0 + s, y0), Vec2::new(x0 + s, y0 + s), Vec2::new(x0, y0 + s)]
}

fn cube() -> Vec<[Vec3; 3]> {
    prism(&square(50.0, 50.0, 20.0), None, 0.0, 20.0)
}

fn bounds(lp: &[Vec2]) -> (Vec2, Vec2) {
    let mut lo = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for q in lp {
        lo = Vec2::new(lo.x.min(q.x), lo.y.min(q.y));
        hi = Vec2::new(hi.x.max(q.x), hi.y.max(q.y));
    }
    (lo, hi)
}

#[test]
fn cube_layers() {
    let ls = layers(&cube(), &SliceParams::fdm(0), T).unwrap();
    assert_eq!(ls.len(), 100);
    assert!((ls[99].z - 20.0).abs() < 1e-9);
    for l in &ls {
        assert_eq!(l.islands.len(), 1, "layer {}", l.index);
        let isl = &l.islands[0];
        assert!(isl.holes.is_empty());
        assert!((signed_area2(&isl.outer) - 400.0).abs() < 1e-9, "layer {}", l.index);
        assert_eq!(bounds(&isl.outer), (Vec2::new(50.0, 50.0), Vec2::new(70.0, 70.0)));
        assert_eq!(l.degenerate, None);
    }
}

#[test]
fn hole_orientation() {
    let hole = square(56.0, 56.0, 8.0);
    let tris = prism(&square(50.0, 50.0, 20.0), Some(&hole), 0.0, 20.0);
    let ls = layers(&tris, &SliceParams::fdm(0), T).unwrap();
    assert_eq!(ls.len(), 100);
    for l in &ls {
        assert_eq!(l.islands.len(), 1, "layer {}", l.index);
        let isl = &l.islands[0];
        assert!((signed_area2(&isl.outer) - 400.0).abs() < 1e-9);
        assert_eq!(isl.holes.len(), 1);
        assert!((signed_area2(&isl.holes[0]) + 64.0).abs() < 1e-9);
    }
}

#[test]
fn tetra_shrinks_apex_degenerate() {
    let (a, b, c, d) =
        (Vec3::new(50.0, 50.0, 0.0), Vec3::new(70.0, 50.0, 0.0), Vec3::new(60.0, 67.0, 0.0), Vec3::new(60.0, 56.0, 10.0));
    let tris = vec![[a, c, b], [a, b, d], [b, c, d], [c, a, d]];
    let ls = layers(&tris, &SliceParams::fdm(0), T).unwrap();
    assert_eq!(ls.len(), 50);
    let mut last = f64::INFINITY;
    for l in &ls {
        assert_eq!(l.islands.len(), 1);
        let o = &l.islands[0].outer;
        assert_eq!(o.len(), 3, "layer {}", l.index);
        let ar = signed_area2(o);
        assert!(ar > 0.0 && ar < last, "layer {}", l.index);
        last = ar;
    }
    assert_eq!(ls[0].degenerate, None);
    assert_eq!(ls[49].degenerate, Some("perimeter collapsed"));
}

#[test]
fn open_soup_refuses() {
    let mut tris = cube();
    tris.remove(0);
    match layers(&tris, &SliceParams::fdm(0), T) {
        Err(Refusal::OpenLoop { layer: 0, gap }) => assert!(gap > 0.0),
        other => panic!("{other:?}"),
    }
}

/// The filament pushed on moves that go somewhere (retract / prime pairs excluded).
fn deposited_e(p: &Program) -> f64 {
    let (mut pos, mut e, mut sum) = (None::<Vec3>, 0.0, 0.0);
    for mv in &p.moves {
        match mv {
            Move::Extrude { to, e: e1, .. } => {
                if pos.is_some_and(|q| q != *to) {
                    sum += e1 - e;
                }
                e = *e1;
                pos = Some(*to);
            }
            Move::SetE { e: e1 } => e = *e1,
            Move::Rapid { to } | Move::Linear { to, .. } => pos = Some(*to),
            Move::Home => pos = None,
            _ => {}
        }
    }
    sum
}

#[test]
fn e_equals_cube_volume() {
    let mut p = SliceParams::fdm(0);
    p.density = 1.0;
    let prog = slice(&cube(), &p, &printer(), T).unwrap();
    let r = p.filament * 0.5;
    let vol = deposited_e(&prog) * PI * r * r;
    assert!((vol - 8000.0).abs() / 8000.0 < 1e-6, "{vol}");
}

#[test]
fn marlin_round_trip() {
    let prog = slice(&cube(), &SliceParams::fdm(0), &printer(), T).unwrap();
    let text = gcode::write(&prog, Dialect::Marlin).unwrap();
    let back = gcode::read(&text, Dialect::Marlin).unwrap();
    assert_eq!(gcode::write(&back, Dialect::Marlin).unwrap(), text);
    assert!(text.starts_with("G21\nG90\nM82\n; VUGMAKESLICE: 100 layers\n"));
}

#[test]
fn validates_timed() {
    let m = printer();
    let prog = slice(&cube(), &SliceParams::fdm(0), &m, T).unwrap();
    let st = prog.validate(&m, T).unwrap();
    assert!(st.time_s > 0.0);
    let mut pos = None::<Vec3>;
    for (i, mv) in prog.moves.iter().enumerate() {
        match mv {
            Move::Extrude { to, .. } => {
                if pos.is_some_and(|q| q != *to) {
                    assert!(st.durations[i] > 0.0, "move {i}");
                }
                pos = Some(*to);
            }
            Move::Rapid { to } => pos = Some(*to),
            Move::Home => pos = None,
            _ => {}
        }
    }
}

#[test]
fn generate_matches() {
    let (tris, p, m) = (cube(), SliceParams::fdm(0), printer());
    let op = Op::Slice { triangles: tris.clone(), params: p };
    assert_eq!(generate(&op, &m, T), slice(&tris, &p, &m, T));
}

#[test]
fn bad_params_refuse() {
    let (tris, m) = (cube(), printer());
    let mut p = SliceParams::fdm(0);
    p.layer_h = 0.0;
    assert_eq!(slice(&tris, &p, &m, T), Err(Refusal::SliceInvalid { what: "layer height not positive" }));
    let mut p = SliceParams::fdm(0);
    p.density = 1.5;
    assert_eq!(slice(&tris, &p, &m, T), Err(Refusal::SliceInvalid { what: "infill density outside (0, 1]" }));
    let mut mill = printer();
    mill.kind = MachineKind::Mill3;
    assert_eq!(
        slice(&tris, &SliceParams::fdm(0), &mill, T),
        Err(Refusal::SliceInvalid { what: "machine is not an FDM printer" })
    );
    assert_eq!(slice(&tris, &SliceParams::fdm(7), &m, T), Err(Refusal::UnknownTool { at: 0, tool: 7 }));
    assert_eq!(slice(&[], &SliceParams::fdm(0), &m, T), Err(Refusal::SliceInvalid { what: "empty triangle soup" }));
}
