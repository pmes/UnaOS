// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGMAKESIM (LEDGER SR496): material removal on the signed-distance stock — a pocket removes its area less the
//! corner radii, a face its slab, a drill its hole with the point; a contour past a clamp names the gouge, a rapid
//! through stock names the rapid cut, a full-width slot engages the whole diameter; the mesh holds the volume left;
//! the refusals by name.

use std::f64::consts::PI;

use make_core::op::FaceDir;
use make_core::*;

const T: Tol = Tol::DEFAULT;

fn axis(id: AxisId, min: f64, max: f64) -> Axis {
    Axis { id, min, max, max_feed: 3000.0, max_accel: 100.0, rapid: 6000.0 }
}

fn mill() -> Machine {
    Machine {
        kind: MachineKind::Mill3,
        kinematics: Kinematics::Cartesian,
        axes: vec![axis(AxisId::X, -50.0, 200.0), axis(AxisId::Y, -50.0, 200.0), axis(AxisId::Z, -50.0, 60.0)],
        power: Power { kind: PowerKind::Spindle, max: 24000.0, ask_above: 18000.0 },
        envelope: Aabb3 { min: Vec3::new(-20.0, -20.0, -20.0), max: Vec3::new(100.0, 100.0, 50.0) },
        safe_z: 5.0,
        units: Units::Mm,
        tools: vec![
            Tool { number: 2, kind: ToolKind::FlatEnd, dc: 4.0, lf: 20.0, apmx: 2.0, nof: 3, sig: 0.0, ramp_deg: None, centre_cutting: None },
            Tool { number: 3, kind: ToolKind::BallEnd, dc: 4.0, lf: 20.0, apmx: 2.0, nof: 2, sig: 0.0, ramp_deg: None, centre_cutting: None },
            Tool { number: 4, kind: ToolKind::Drill, dc: 4.0, lf: 20.0, apmx: 10.0, nof: 2, sig: 118.0f64.to_radians(), ramp_deg: None, centre_cutting: None },
            Tool { number: 5, kind: ToolKind::Nozzle, dc: 0.4, lf: 10.0, apmx: 0.2, nof: 1, sig: 0.0, ramp_deg: None, centre_cutting: None },
        ],
        stock: Some(stock()),
        fixtures: vec![],
        loaded: Some(2),
    }
}

/// 50 x 50 x 10 with its top at z = 0.
fn stock() -> Stock {
    Stock { shape: StockShape::Box { size: Vec3::new(50.0, 50.0, 10.0) }, origin: Vec3::new(0.0, 0.0, -10.0) }
}

fn feeds() -> Feeds {
    Feeds { feed: 600.0, plunge: 120.0, rpm: 1000.0 }
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Vec2> {
    vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]
}

fn sim(p: &Program, m: &Machine, fixtures: &[Fixture], params: SimParams) -> Sim {
    simulate(p, m, &stock(), fixtures, params, T).expect("simulates")
}

fn close(got: f64, want: f64, rel: f64, what: &str) {
    let e = (got - want).abs() / want;
    assert!(e <= rel, "{what}: got {got}, want {want} (relative error {e} above {rel})");
}

fn to_of(mv: &Move) -> Option<Vec3> {
    match mv {
        Move::Rapid { to } | Move::Linear { to, .. } | Move::Arc { to, .. } => Some(*to),
        _ => None,
    }
}

#[test]
fn pocket_removes_area_less_corner_radii() {
    let m = mill();
    let op = Op::Pocket2_5D {
        outline: rect(5.0, 5.0, 45.0, 45.0),
        islands: vec![],
        top: 0.0,
        depth: 2.0,
        stepover: 1.5,
        chord: 0.01,
        climb: false,
        tool: 2,
        feeds: feeds(),
    };
    let p = generate(&op, &m, T).expect("pocket");
    let s = sim(&p, &m, &[], SimParams::default());
    // DC 4 -> cell 0.5; the lattice is the stock grown by two nodes: x, y nodes -2..=102, z nodes -22..=2.
    assert_eq!(s.field.cell(), 0.5);
    assert_eq!(s.field.dims(), [105, 105, 25]);
    let r = 2.0;
    let want = 2.0 * (40.0 * 40.0 - (4.0 - PI) * r * r);
    close(s.removed_mm3, want, 0.02, "pocket volume");
    let per: f64 = s.per_move.iter().map(|m| m.removed_mm3).sum();
    close(per, s.removed_mm3, 1e-9, "per-move sum");
    assert!(s.rapid_cuts.is_empty(), "{:?}", s.rapid_cuts);
    assert!(s.gouges.is_empty());
    // No core left: the interior, above the floor's half-cells, is air.
    let inner = 36.0 * 36.0 * 1.75;
    let rest = s.rest(&rect(7.0, 7.0, 43.0, 43.0), -1.75, 0.0);
    assert!(rest < 0.005 * inner, "rest {rest}");
    // Outside the pocket nothing was cut (the region's edges between nodes: 7 x 97 nodes, 3.5 cells of depth).
    let wall = s.rest(&rect(45.75, 0.75, 49.25, 49.25), -1.75, 0.0);
    close(wall, 3.5 * 48.5 * 1.75, 0.01, "untouched stock beside the pocket");
}

#[test]
fn contour_past_a_clamp_names_the_gouge() {
    let m = mill();
    let clamp = Fixture {
        name: "clamp".into(),
        keepout: vec![Aabb3 { min: Vec3::new(20.0, -8.0, -5.0), max: Vec3::new(30.0, -3.0, 3.0) }],
    };
    let fx = [clamp];
    let outside = Op::Contour {
        profile: rect(0.0, 0.0, 50.0, 50.0),
        top: 0.0,
        depth: 2.0,
        side: Side::Outside,
        tool: 2,
        feeds: feeds(),
    };
    let p = generate(&outside, &m, T).expect("contour");
    let s = sim(&p, &m, &fx, SimParams::default());
    let g = s.gouges.first().expect("a gouge");
    assert_eq!(g.fixture, "clamp");
    // The gouging move runs on (or plunges onto) the offset bottom edge, y = -r.
    let to = to_of(&p.moves[g.at]).expect("a motion move");
    assert!((to.y + 2.0).abs() < 1e-6, "gouge at move {} ending {:?}", g.at, to);
    // The inside contour stays 3 mm clear of the clamp.
    let inside = Op::Contour {
        profile: rect(0.0, 0.0, 50.0, 50.0),
        top: 0.0,
        depth: 2.0,
        side: Side::Inside,
        tool: 2,
        feeds: feeds(),
    };
    let q = generate(&inside, &m, T).expect("contour");
    assert!(sim(&q, &m, &fx, SimParams::default()).gouges.is_empty());
}

#[test]
fn rapid_through_stock_names_the_rapid_cut() {
    let m = mill();
    let mut p = Program::new(Units::Mm);
    p.moves.push(Move::Rapid { to: Vec3::new(-5.0, 25.0, 5.0) });
    p.moves.push(Move::Rapid { to: Vec3::new(-5.0, 25.0, -1.0) });
    p.moves.push(Move::Rapid { to: Vec3::new(55.0, 25.0, -1.0) });
    let s = sim(&p, &m, &[], SimParams::default());
    assert_eq!(s.rapid_cuts.len(), 1, "{:?}", s.rapid_cuts);
    assert_eq!(s.rapid_cuts[0].at, 2);
    assert!(s.rapid_cuts[0].removed_mm3 > 50.0 * 4.0 * 1.0 * 0.9);
}

#[test]
fn face_removes_the_top_slab_and_the_mesh_holds_the_rest() {
    let m = mill();
    let op = Op::Face {
        lo: Vec2::new(0.0, 0.0),
        hi: Vec2::new(50.0, 50.0),
        top: 0.0,
        depth: 1.0,
        stepover: 3.0,
        along: FaceDir::X,
        tool: 2,
        feeds: feeds(),
    };
    let p = generate(&op, &m, T).expect("face");
    let s = sim(&p, &m, &[], SimParams { mesh: true, ..SimParams::default() });
    close(s.removed_mm3, 50.0 * 50.0 * 1.0, 0.01, "face slab");
    let left = s.remaining_mm3();
    close(left, 50.0 * 50.0 * 9.0, 0.01, "stock left");
    let (pos, tris) = s.mesh.as_ref().expect("mesh");
    assert!(!tris.is_empty());
    // The divergence theorem over the marched surface (outward normals).
    let mut vol = 0.0;
    for t in tris {
        let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        vol += a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x) + a.z * (b.x * c.y - b.y * c.x);
    }
    vol /= 6.0;
    close(vol.abs(), left, 0.01, "mesh volume");
}

#[test]
fn drill_removes_its_hole_with_the_point() {
    let m = mill();
    let op = Op::Drill { points: vec![Vec2::new(25.0, 25.0)], top: 0.0, depth: 5.0, peck: 0.0, tool: 4, feeds: feeds() };
    let p = generate(&op, &m, T).expect("drill");
    let s = sim(&p, &m, &[], SimParams::default());
    let r: f64 = 2.0;
    let hc = r / (0.5 * 118.0f64.to_radians()).tan();
    let want = PI * r * r * (5.0 - hc) + PI * r * r * hc / 3.0;
    close(s.removed_mm3, want, 0.02, "drilled hole");
    assert!(s.rapid_cuts.is_empty(), "the retract cuts nothing: {:?}", s.rapid_cuts);
}

#[test]
fn full_width_slot_engages_the_whole_diameter() {
    let m = mill();
    let mut p = Program::new(Units::Mm);
    p.moves.push(Move::Rapid { to: Vec3::new(-5.0, 25.0, 5.0) });
    p.moves.push(Move::Linear { to: Vec3::new(-5.0, 25.0, -1.0), feed: 120.0 });
    p.moves.push(Move::Linear { to: Vec3::new(55.0, 25.0, -1.0), feed: 600.0 });
    let s = sim(&p, &m, &[], SimParams::default());
    let slot = s.per_move.iter().find(|m| m.at == 2).expect("the slot move");
    assert_eq!(slot.engagement, 1.0);
    assert_eq!(s.max_engagement, 1.0);
    assert_eq!(s.per_move.iter().find(|m| m.at == 1).unwrap().engagement, 0.0);
    close(s.removed_mm3, 50.0 * 4.0 * 1.0, 0.02, "slot volume");
}

#[test]
fn refusals_by_name() {
    let m = mill();
    let mut p = Program::new(Units::Mm);
    p.moves.push(Move::Linear { to: Vec3::new(10.0, 10.0, -1.0), feed: 100.0 });
    let small = SimParams { max_cells: 1000, ..SimParams::default() };
    assert!(matches!(simulate(&p, &m, &stock(), &[], small, T), Err(SimRefusal::TooLarge { max: 1000, .. })));
    let mut fdm = mill();
    fdm.loaded = Some(5);
    let cell = SimParams { cell: Some(0.5), ..SimParams::default() };
    assert!(matches!(
        simulate(&p, &fdm, &stock(), &[], cell, T),
        Err(SimRefusal::ToolNotSimulated { at: 0, tool: 5 })
    ));
    assert!(matches!(simulate(&p, &fdm, &stock(), &[], SimParams::default(), T), Err(SimRefusal::NoMillingTool)));
    let mut none = mill();
    none.loaded = None;
    assert!(matches!(simulate(&p, &none, &stock(), &[], cell, T), Err(SimRefusal::NoTool { at: 0 })));
}
