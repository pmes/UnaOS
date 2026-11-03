// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! make_core's witness (VUGMAKE, LEDGER SR467): the planner's time against a hand computation, the contour offset of
//! a square, every refusal by name, each dialect's round trip, and the review Helm will read.

use make_core::gcode::{self, Dialect, Realtime};
use make_core::plan::trapezoid;
use make_core::*;

const T: Tol = Tol::DEFAULT;

fn axis(id: AxisId, min: f64, max: f64) -> Axis {
    // 3000 mm/min feed, 100 mm/s² acceleration, 6000 mm/min (100 mm/s) rapid.
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
            Tool { number: 1, kind: ToolKind::Drill, dc: 3.0, lf: 30.0, apmx: 3.0, nof: 2, sig: 2.0944, ramp_deg: None, centre_cutting: None },
            Tool { number: 2, kind: ToolKind::FlatEnd, dc: 4.0, lf: 20.0, apmx: 2.0, nof: 3, sig: 0.0, ramp_deg: None, centre_cutting: None },
        ],
        stock: Some(Stock { shape: StockShape::Box { size: Vec3::new(50.0, 50.0, 10.0) }, origin: Vec3::new(0.0, 0.0, -10.0) }),
        fixtures: vec![Fixture {
            name: "vise jaw".to_string(),
            keepout: vec![Aabb3 { min: Vec3::new(60.0, 0.0, -10.0), max: Vec3::new(70.0, 50.0, 10.0) }],
        }],
        loaded: Some(1),
    }
}

fn feeds() -> Feeds {
    Feeds { feed: 600.0, plunge: 120.0, rpm: 1000.0 }
}

#[test]
fn machine_model_is_consistent() {
    assert_eq!(mill().check(T), Ok(()));
    let mut m = mill();
    m.envelope.max.x = 500.0;
    assert_eq!(m.check(T), Err(Refusal::MachineInvalid { what: "envelope outside axis travel" }));
}

#[test]
fn trapezoid_both_branches() {
    // 10 mm at 10 mm/s under 100 mm/s²: v²/a = 1 mm <= 10, so L/v + v/a = 1 + 0.1.
    assert!((trapezoid(10.0, 10.0, 100.0) - 1.1).abs() < 1e-12);
    // 1 mm at 100 mm/s: never reaches cruise, 2·sqrt(L/a) = 2·0.1.
    assert!((trapezoid(1.0, 100.0, 100.0) - 0.2).abs() < 1e-12);
}

#[test]
fn drill_time_against_a_hand_computation() {
    let m = mill();
    let op = Op::Drill { points: vec![Vec2::new(10.0, 10.0)], top: 0.0, depth: 6.0, peck: 3.0, tool: 1, feeds: feeds() };
    let p = generate(&op, &m, T).unwrap();
    let at = |z: f64| Vec3::new(10.0, 10.0, z);
    assert_eq!(
        p.moves,
        vec![
            Move::Spindle(Spin::Cw(1000.0)),
            Move::Rapid { to: at(5.0) },
            Move::Linear { to: at(-3.0), feed: 120.0 },
            Move::Rapid { to: at(5.0) },
            Move::Linear { to: at(-3.0), feed: 120.0 },
            Move::Linear { to: at(-6.0), feed: 120.0 },
            Move::Rapid { to: at(5.0) },
            Move::Spindle(Spin::Off),
        ]
    );
    let s = p.validate(&m, T).unwrap();
    // By hand, Z only (plunge 2 mm/s, rapid 100 mm/s, a = 100 mm/s²); the first rapid is untimed (start unknown):
    //   feed 8 mm:  8/2 + 2/100       = 4.02
    //   rapid 8 mm: 2·sqrt(8/100)     (never reaches 100 mm/s)
    //   feed 8 mm:  4.02 (the fed re-entry)
    //   feed 3 mm:  3/2 + 2/100       = 1.52
    //   rapid 11 mm: 2·sqrt(11/100)
    let hand = 4.02 + 2.0 * (0.08f64).sqrt() + 4.02 + 1.52 + 2.0 * (0.11f64).sqrt();
    assert!((s.time_s - hand).abs() < 1e-9, "{} vs {}", s.time_s, hand);
    assert_eq!(s.untimed, 1);
    assert_eq!(s.tools, vec![1]);
    assert_eq!(s.max_feed, 120.0);
}

fn square() -> Vec<Vec2> {
    vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)]
}

#[test]
fn contour_outside_square_is_a_larger_square_with_four_ccw_arcs() {
    let m = mill();
    let op = Op::Contour { profile: square(), top: 0.0, depth: 2.0, side: Side::Outside, tool: 2, feeds: feeds() };
    let p = generate(&op, &m, T).unwrap();
    let z = -2.0;
    let v = |x: f64, y: f64| Vec3::new(x, y, z);
    assert_eq!(
        p.moves,
        vec![
            Move::ToolChange { tool: 2 },
            Move::Spindle(Spin::Cw(1000.0)),
            Move::Rapid { to: Vec3::new(0.0, -2.0, 5.0) },
            Move::Linear { to: v(0.0, -2.0), feed: 120.0 },
            Move::Linear { to: v(10.0, -2.0), feed: 600.0 },
            Move::Arc { to: v(12.0, 0.0), centre: Vec2::new(10.0, 0.0), cw: false, feed: 600.0 },
            Move::Linear { to: v(12.0, 10.0), feed: 600.0 },
            Move::Arc { to: v(10.0, 12.0), centre: Vec2::new(10.0, 10.0), cw: false, feed: 600.0 },
            Move::Linear { to: v(0.0, 12.0), feed: 600.0 },
            Move::Arc { to: v(-2.0, 10.0), centre: Vec2::new(0.0, 10.0), cw: false, feed: 600.0 },
            Move::Linear { to: v(-2.0, 0.0), feed: 600.0 },
            Move::Arc { to: v(0.0, -2.0), centre: Vec2::new(0.0, 0.0), cw: false, feed: 600.0 },
            Move::Rapid { to: Vec3::new(0.0, -2.0, 5.0) },
            Move::Spindle(Spin::Off),
        ]
    );
    let s = p.validate(&m, T).unwrap();
    let b = s.bbox.unwrap();
    assert!((b.min.x + 2.0).abs() < 1e-9 && (b.max.x - 12.0).abs() < 1e-9);
    assert!((b.min.y + 2.0).abs() < 1e-9 && (b.max.y - 12.0).abs() < 1e-9);
}

#[test]
fn contour_inside_square_is_the_smaller_square_and_a_clockwise_profile_is_normalised() {
    let m = mill();
    let mut cw = square();
    cw.reverse();
    let op = Op::Contour { profile: cw, top: 0.0, depth: 4.0, side: Side::Inside, tool: 2, feeds: feeds() };
    let p = generate(&op, &m, T).unwrap();
    let lines: Vec<Vec3> = p
        .moves
        .iter()
        .filter_map(|mv| match mv {
            Move::Linear { to, feed } if *feed == 600.0 => Some(*to),
            _ => None,
        })
        .collect();
    // Two passes (APMX 2 over a 4 mm depth), four sides each, no arcs.
    assert_eq!(lines.len(), 8);
    assert!(!p.moves.iter().any(|mv| matches!(mv, Move::Arc { .. })));
    let xy: Vec<(f64, f64)> = lines[..4].iter().map(|v| (v.x, v.y)).collect();
    assert_eq!(xy, vec![(8.0, 2.0), (8.0, 8.0), (2.0, 8.0), (2.0, 2.0)]);
    assert_eq!(lines[0].z, -2.0);
    assert_eq!(lines[4].z, -4.0);
}

#[test]
fn contour_refusals_by_name() {
    let m = mill();
    let c = |profile: Vec<Vec2>, side| Op::Contour { profile, top: 0.0, depth: 1.0, side, tool: 2, feeds: feeds() };
    assert_eq!(
        generate(&c(square()[..2].to_vec(), Side::Outside), &m, T),
        Err(Refusal::ContourDegenerate { what: "fewer than 3 points" })
    );
    let tiny = vec![Vec2::new(0.0, 0.0), Vec2::new(3.0, 0.0), Vec2::new(3.0, 3.0), Vec2::new(0.0, 3.0)];
    assert_eq!(generate(&c(tiny, Side::Inside), &m, T), Err(Refusal::ToolTooLarge { tool: 2 }));
    let bow = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(10.0, 0.0), Vec2::new(0.0, 10.0)];
    assert_eq!(generate(&c(bow, Side::Outside), &m, T).err().map(|r| matches!(r, Refusal::ContourSelfIntersects | Refusal::ContourDegenerate { .. })), Some(true));
}

#[test]
fn unbuilt_ops_refuse_with_their_owed_id() {
    let m = mill();
    let laser = Op::LaserCut { polylines: vec![square()], passes: 1 };
    assert_eq!(generate(&laser, &m, T), Err(Refusal::OpOwed { op: "LaserCut", owed: "VUG-MAKE-LASER" }));
    let print = Op::PrintLayers { slices: vec![], tool: 1 };
    assert_eq!(generate(&print, &m, T), Err(Refusal::OpOwed { op: "PrintLayers", owed: "VUG-MAKE-SLICE" }));
}

fn prog(moves: Vec<Move>) -> Program {
    Program { units: Units::Mm, moves }
}

#[test]
fn a_move_outside_the_envelope_is_refused_by_name() {
    let m = mill();
    let p = prog(vec![Move::Rapid { to: Vec3::new(0.0, 0.0, 20.0) }, Move::Rapid { to: Vec3::new(150.0, 0.0, 20.0) }]);
    assert_eq!(p.validate(&m, T), Err(Refusal::OutsideEnvelope { at: 1, x: 150.0, y: 0.0, z: 20.0 }));
}

#[test]
fn the_other_refusals_by_name() {
    let m = mill();
    let up = Vec3::new(20.0, 20.0, 20.0);
    let fast = prog(vec![Move::Rapid { to: up }, Move::Linear { to: Vec3::new(30.0, 20.0, 20.0), feed: 5000.0 }]);
    assert_eq!(fast.validate(&m, T), Err(Refusal::FeedAboveMax { at: 1, feed: 5000.0, max: 3000.0 }));
    let dive = prog(vec![Move::Rapid { to: up }, Move::Rapid { to: Vec3::new(20.0, 20.0, 1.0) }]);
    assert_eq!(dive.validate(&m, T), Err(Refusal::RapidIntoStock { at: 1 }));
    let crash = prog(vec![Move::Rapid { to: Vec3::new(65.0, 20.0, 40.0) }, Move::Rapid { to: Vec3::new(65.0, 20.0, 8.0) }]);
    assert_eq!(crash.validate(&m, T), Err(Refusal::FixtureCollision { at: 1, fixture: "vise jaw".to_string() }));
    let who = prog(vec![Move::ToolChange { tool: 9 }]);
    assert_eq!(who.validate(&m, T), Err(Refusal::UnknownTool { at: 0, tool: 9 }));
    // A straight +Z retract out of the cut is not a dive.
    let out = prog(vec![Move::Rapid { to: up }, Move::Linear { to: Vec3::new(20.0, 20.0, -2.0), feed: 100.0 }, Move::Rapid { to: up }]);
    assert!(out.validate(&m, T).is_ok());
}

fn round_trip(p: &Program, d: Dialect) {
    let text = gcode::write(p, d).unwrap();
    let back = gcode::read(&text, d).unwrap();
    assert_eq!(&back, p, "{:?}\n{}", d, text);
    assert_eq!(gcode::write(&back, d).unwrap(), text);
}

fn cut() -> Vec<Move> {
    vec![
        Move::Comment("part 7".to_string()),
        Move::Rapid { to: Vec3::new(0.0, -2.0, 5.0) },
        Move::Linear { to: Vec3::new(0.0, -2.0, -1.5), feed: 120.0 },
        Move::Linear { to: Vec3::new(10.25, -2.0, -1.5), feed: 600.0 },
        Move::Arc { to: Vec3::new(12.25, 0.0, -1.5), centre: Vec2::new(10.25, 0.0), cw: false, feed: 600.0 },
        Move::Arc { to: Vec3::new(10.25, 2.0, -1.5), centre: Vec2::new(10.25, 0.0), cw: true, feed: 450.5 },
        Move::Dwell { s: 0.5 },
        Move::Coolant(Coolant::Off),
        Move::Rapid { to: Vec3::new(0.0, 0.0, 5.0) },
    ]
}

#[test]
fn grbl_round_trips_and_names_what_it_cannot_say() {
    let mut mv = vec![Move::Spindle(Spin::Cw(12000.0)), Move::Coolant(Coolant::Flood)];
    mv.extend(cut());
    mv.push(Move::Spindle(Spin::Off));
    round_trip(&prog(mv), Dialect::Grbl { laser: false });

    let mut beam = vec![Move::Power { pct: 40.0 }];
    beam.extend(cut());
    beam.push(Move::Power { pct: 0.0 });
    round_trip(&prog(beam), Dialect::Grbl { laser: true });

    let tc = prog(vec![Move::ToolChange { tool: 2 }]);
    assert_eq!(
        gcode::write(&tc, Dialect::Grbl { laser: false }),
        Err(Refusal::Unsayable { dialect: Dialect::Grbl { laser: false }, at: 0, what: "tool change (grbl 1.1 rejects M6)" })
    );
    assert_eq!(
        gcode::read("T2 M6\n", Dialect::Grbl { laser: false }),
        Err(Refusal::Unsupported { line: 1, word: "T2".to_string() })
    );
    assert_eq!(Realtime::FeedHold.byte(), b'!');
    assert_eq!(Realtime::SoftReset.byte(), 0x18);
    assert_eq!(Realtime::JogCancel.byte(), 0x85);
}

#[test]
fn linuxcnc_round_trips() {
    let mut mv = vec![Move::ToolChange { tool: 2 }, Move::Spindle(Spin::Ccw(8000.0)), Move::Coolant(Coolant::Mist)];
    mv.extend(cut());
    round_trip(&prog(mv), Dialect::LinuxCnc);
    let inch = Program { units: Units::Inch, moves: vec![Move::Rapid { to: Vec3::new(1.0, 2.0, 0.5) }] };
    round_trip(&inch, Dialect::LinuxCnc);
}

#[test]
fn marlin_round_trips() {
    let p = prog(vec![
        Move::Comment("PLA; layer 0".to_string()),
        Move::Heat { heater: Heater::Bed, celsius: 60.0, wait: false },
        Move::Heat { heater: Heater::Hotend, celsius: 210.0, wait: true },
        Move::Heat { heater: Heater::Bed, celsius: 60.0, wait: true },
        Move::Home,
        Move::SetE { e: 0.0 },
        Move::Rapid { to: Vec3::new(10.0, 10.0, 0.2) },
        Move::Extrude { to: Vec3::new(50.0, 10.0, 0.2), e: 1.3302, feed: 1800.0 },
        Move::Extrude { to: Vec3::new(50.0, 10.0, 0.2), e: 0.5302, feed: 2400.0 },
        Move::Dwell { s: 0.25 },
        Move::ToolChange { tool: 1 },
        Move::Linear { to: Vec3::new(10.0, 10.0, 5.0), feed: 3000.0 },
    ]);
    round_trip(&p, Dialect::Marlin);
    assert!(matches!(gcode::write(&p, Dialect::LinuxCnc), Err(Refusal::Unsayable { at: 1, .. })));
}

#[test]
fn reader_takes_the_r_form_relative_mode_and_names_what_it_refuses() {
    let d = Dialect::LinuxCnc;
    let p = gcode::read("G21 G90\nG0 X0 Y0 Z1\nG2 X10 Y0 R5 F300\nG91\nG1 X1 Y1 F100\nM30\nG0 X99\n", d).unwrap();
    assert_eq!(p.moves[1], Move::Arc { to: Vec3::new(10.0, 0.0, 1.0), centre: Vec2::new(5.0, 0.0), cw: true, feed: 300.0 });
    assert_eq!(p.moves[2], Move::Linear { to: Vec3::new(11.0, 1.0, 1.0), feed: 100.0 });
    assert_eq!(p.moves.len(), 3); // M30 ends the program
    assert_eq!(gcode::read("G18\n", d), Err(Refusal::Unsupported { line: 1, word: "G18".to_string() }));
    assert_eq!(gcode::read("G0 X1 Y1 Z1\nG55\n", d), Err(Refusal::Unsupported { line: 2, word: "G55".to_string() }));
    assert_eq!(gcode::read("X1 Y1 Z1\n", d), Err(Refusal::Parse { line: 1, what: "axis words with no motion mode" }));
    assert_eq!(gcode::read("G1 X1 Y1 Z1\n", d), Err(Refusal::Parse { line: 1, what: "feed unset" }));
    assert_eq!(gcode::read("G0 X1.2.3\n", d), Err(Refusal::Parse { line: 1, what: "number" }));
    let m = gcode::read("N10 G1 X1 Y2 Z3 F50*71 ; hi\n", Dialect::Marlin).unwrap();
    assert_eq!(m.moves, vec![Move::Comment("hi".to_string()), Move::Linear { to: Vec3::new(1.0, 2.0, 3.0), feed: 50.0 }]);
}

#[test]
fn review_of_a_safe_program_has_no_refusal_and_one_ask() {
    let m = mill();
    let p = prog(vec![
        Move::Spindle(Spin::Cw(10000.0)),
        Move::Rapid { to: Vec3::new(20.0, 20.0, 5.0) },
        Move::Linear { to: Vec3::new(20.0, 20.0, -1.0), feed: 100.0 },
        Move::Linear { to: Vec3::new(30.0, 20.0, -1.0), feed: 600.0 },
        Move::Rapid { to: Vec3::new(30.0, 20.0, 5.0) },
        Move::Spindle(Spin::Off),
    ]);
    let r = review(&p, &m, T);
    assert_eq!(r.refusals, vec![]);
    assert_eq!(r.asks, vec![Ask::FirstRun { digest: p.digest() }]);
    assert!(r.stats.time_s > 0.0);
}

#[test]
fn review_asks_for_a_tool_change_and_high_power_and_carries_refusals() {
    let m = mill();
    let op = Op::Contour { profile: square(), top: 0.0, depth: 1.0, side: Side::Outside, tool: 2, feeds: Feeds { feed: 600.0, plunge: 120.0, rpm: 20000.0 } };
    let p = generate(&op, &m, T).unwrap();
    let r = review(&p, &m, T);
    assert_eq!(r.refusals, vec![]);
    assert_eq!(
        r.asks,
        vec![
            Ask::FirstRun { digest: p.digest() },
            Ask::ToolChange { at: 0, tool: 2 },
            Ask::PowerAbove { at: 1, value: 20000.0, threshold: 18000.0 },
        ]
    );
    let bad = prog(vec![Move::Rapid { to: Vec3::new(150.0, 0.0, 20.0) }, Move::ToolChange { tool: 9 }]);
    assert_eq!(review(&bad, &m, T).refusals.len(), 2);
    assert_ne!(p.digest(), bad.digest());
}
