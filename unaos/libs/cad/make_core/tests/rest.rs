// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGMAKEREST (LEDGER SR517): the rest a pocket leaves, exact — a square's four corners (4 - π) r² and the core a
//! stepover past r leaves, by hand; a stepover past 2r refused `StepoverLeavesCore`; the rest cut with a smaller tool
//! validated, inside the walls by its radius, through grbl, and removing in the simulation what the construction says
//! is left; a closed core the flat end cannot enter refused `RestNoEntry` by region, a ball end entering it.

use std::f64::consts::PI;

use make_core::gcode::{self, Dialect};
use make_core::rest::regions;
use make_core::*;

const T: Tol = Tol::DEFAULT;

fn axis(id: AxisId, min: f64, max: f64) -> Axis {
    Axis { id, min, max, max_feed: 3000.0, max_accel: 100.0, rapid: 6000.0 }
}

/// 50 x 50 x 2 with its top at z = 0.
fn stock() -> Stock {
    Stock { shape: StockShape::Box { size: Vec3::new(50.0, 50.0, 2.0) }, origin: Vec3::new(0.0, 0.0, -2.0) }
}

/// Tool 1 a 10 mm flat end (r1 = 5), tool 2 a 2 mm flat end (r2 = 1), tool 3 a 2 mm ball end; tool 1 loaded.
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
            Tool { number: 1, kind: ToolKind::FlatEnd, dc: 10.0, lf: 20.0, apmx: 5.0, nof: 3, sig: 0.0 },
            Tool { number: 2, kind: ToolKind::FlatEnd, dc: 2.0, lf: 10.0, apmx: 5.0, nof: 2, sig: 0.0 },
            Tool { number: 3, kind: ToolKind::BallEnd, dc: 2.0, lf: 10.0, apmx: 5.0, nof: 2, sig: 0.0 },
        ],
        stock: Some(stock()),
        fixtures: vec![],
        loaded: Some(1),
    }
}

fn feeds() -> Feeds {
    Feeds { feed: 600.0, plunge: 120.0, rpm: 1000.0 }
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Vec2> {
    vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]
}

fn tool(m: &Machine, n: u32) -> Tool {
    *m.tool(n).unwrap()
}

fn rest_op(outline: Vec<Vec2>, stepover: f64, tool: u32) -> Op {
    Op::Rest { outline, islands: vec![], top: 0.0, depth: 1.0, after: 1, stepover, climb: false, tool, feeds: feeds() }
}

fn pocket_op(outline: Vec<Vec2>, stepover: f64) -> Op {
    Op::Pocket2_5D {
        outline,
        islands: vec![],
        top: 0.0,
        depth: 1.0,
        stepover,
        chord: 0.01,
        climb: false,
        tool: 1,
        feeds: feeds(),
    }
}

fn close(got: f64, want: f64, rel: f64, what: &str) {
    let e = (got - want).abs() / want;
    assert!(e <= rel, "{what}: got {got}, want {want} (relative error {e} above {rel})");
}

#[test]
fn a_square_pocketed_with_r5_leaves_four_corners_of_4_minus_pi_r_squared() {
    let m = mill();
    // Rings at 5 / 9 / 13 / 17 (half-widths 15 / 11 / 7 / 3): s < 2r and the innermost ring's band reaches the centre.
    let g = regions(&rect(-20.0, -20.0, 20.0, 20.0), &[], &tool(&m, 1), 4.0, T).expect("regions");
    assert_eq!(g.len(), 4, "{g:?}");
    let total: f64 = g.iter().map(|r| r.area).sum();
    assert!((total - (4.0 - PI) * 25.0).abs() <= 1e-6, "total {total}");
    for r in &g {
        assert!((r.area - (1.0 - PI / 4.0) * 25.0).abs() <= 1e-6, "corner {}", r.area);
        assert_eq!(r.holes, 0);
    }
}

#[test]
fn a_stepover_past_the_diameter_refuses_stepover_leaves_core() {
    let m = mill();
    let out = rect(-20.0, -20.0, 20.0, 20.0);
    assert_eq!(regions(&out, &[], &tool(&m, 1), 12.0, T), Err(Refusal::StepoverLeavesCore { tool: 1 }));
    assert_eq!(generate(&rest_op(out, 12.0, 2), &m, T), Err(Refusal::StepoverLeavesCore { tool: 1 }));
}

#[test]
fn a_stepover_past_r_leaves_a_core_found_by_the_same_construction() {
    let m = mill();
    // 42 x 42 with s = 8: rings at 5 and 13 (half-widths 16 and 8), none at 21; the bands overlap (8 <= 2r); the
    // points more than 5 inside the half-8 ring are the 6 x 6 core.
    let g = regions(&rect(-21.0, -21.0, 21.0, 21.0), &[], &tool(&m, 1), 8.0, T).expect("regions");
    assert_eq!(g.len(), 5, "{g:?}");
    let total: f64 = g.iter().map(|r| r.area).sum();
    assert!((total - ((4.0 - PI) * 25.0 + 36.0)).abs() <= 1e-6, "total {total}");
    let core: Vec<_> = g.iter().filter(|r| (r.area - 36.0).abs() <= 1e-6).collect();
    assert_eq!(core.len(), 1, "{g:?}");
    assert_eq!(core[0].holes, 0);
}

#[test]
fn the_rest_cut_validates_stays_inside_the_walls_and_round_trips_grbl() {
    let m = mill();
    let p = generate(&rest_op(rect(5.0, 5.0, 45.0, 45.0), 4.0, 2), &m, T).expect("rest");
    p.validate(&m, T).expect("validates");
    assert_eq!(p.moves[0], Move::ToolChange { tool: 2 });
    let named = p.moves.iter().filter(|mv| matches!(mv, Move::Comment(c) if c.starts_with("rest region "))).count();
    assert_eq!(named, 4);
    let mut cutting = 0;
    for mv in &p.moves {
        let to = match mv {
            Move::Linear { to, .. } | Move::Arc { to, .. } => *to,
            _ => continue,
        };
        if to.z < 0.0 {
            cutting += 1;
            let wall = (to.x - 5.0).min(45.0 - to.x).min(to.y - 5.0).min(45.0 - to.y);
            assert!(wall >= 1.0 - 1e-6, "{to:?} is {wall} from the wall");
        }
    }
    assert!(cutting > 0);
    let d = Dialect::Grbl { laser: false };
    let text = gcode::write(&p, d).unwrap();
    let back = gcode::read(&text, d).unwrap();
    assert_eq!(gcode::write(&back, d).unwrap(), text);
}

#[test]
fn the_simulated_rest_cut_removes_what_the_construction_left() {
    let m = mill();
    let out = rect(5.0, 5.0, 45.0, 45.0);
    let pocket = generate(&pocket_op(out.clone(), 4.0), &m, T).expect("pocket");
    let rest = generate(&rest_op(out.clone(), 4.0, 2), &m, T).expect("rest");
    let mut both = pocket.clone();
    both.moves.extend(rest.moves.iter().cloned());
    let params = SimParams { cell: Some(0.25), ..SimParams::default() };
    let s1 = simulate(&pocket, &m, &stock(), &[], params, T).expect("simulates the pocket");
    let s2 = simulate(&both, &m, &stock(), &[], params, T).expect("simulates pocket and rest");
    let left: f64 = regions(&out, &[], &tool(&m, 1), 4.0, T).unwrap().iter().map(|r| r.area).sum();
    // What the rest tool leaves of it: P's own corners at r2 = 1.
    let want = (left - (4.0 - PI) * 1.0) * 1.0;
    close(s2.removed_mm3 - s1.removed_mm3, want, 0.05, "the rest cut's volume");
    assert!(s2.gouges.is_empty() && s2.rapid_cuts.is_empty(), "{:?} {:?}", s2.gouges, s2.rapid_cuts);
    // Beside the pocket nothing more was cut.
    let strip = rect(45.75, 0.75, 49.25, 49.25);
    close(s2.rest(&strip, -1.5, 0.0), s1.rest(&strip, -1.5, 0.0), 0.01, "the stock beside the pocket");
}

#[test]
fn a_core_the_flat_end_cannot_enter_refuses_rest_no_entry_by_region() {
    let m = mill();
    let out = rect(-21.0, -21.0, 21.0, 21.0);
    let g = regions(&out, &[], &tool(&m, 1), 8.0, T).expect("regions");
    let core = g.iter().position(|r| (r.area - 36.0).abs() <= 1e-6).expect("the core");
    // P's r2 rings at 19 and 20 (half-widths 2 and 1) lie inside the core: every start is in material.
    assert_eq!(generate(&rest_op(out.clone(), 8.0, 2), &m, T), Err(Refusal::RestNoEntry { region: core, tool: 2 }));
    // A ball end cuts at its centre and plunges.
    assert!(generate(&rest_op(out.clone(), 8.0, 3), &m, T).is_ok());
    // The rest tool must be the smaller one.
    assert_eq!(
        generate(&rest_op(out, 8.0, 1), &m, T),
        Err(Refusal::BadParameter { what: "rest tool not smaller than the first" })
    );
}
