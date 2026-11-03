// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGMAKEPOCKET (LEDGER SR479): the pocket's ring count against a hand computation, every move of a pocket inside
//! the outline inset by the tool radius and outside every island inset, a face covering its rectangle, the
//! refusals by name, climb against conventional, and each program validated and round-tripped through grbl.

use std::collections::BTreeSet;

use make_core::gcode::{self, Dialect};
use make_core::op::FaceDir;
use make_core::plan::arc_sweep;
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
            Tool { number: 2, kind: ToolKind::FlatEnd, dc: 4.0, lf: 20.0, apmx: 2.0, nof: 3, sig: 0.0 },
            Tool { number: 3, kind: ToolKind::BallEnd, dc: 4.0, lf: 20.0, apmx: 2.0, nof: 2, sig: 0.0 },
        ],
        stock: Some(Stock { shape: StockShape::Box { size: Vec3::new(50.0, 50.0, 10.0) }, origin: Vec3::new(0.0, 0.0, -10.0) }),
        fixtures: vec![],
        loaded: Some(2),
    }
}

fn feeds() -> Feeds {
    Feeds { feed: 600.0, plunge: 120.0, rpm: 1000.0 }
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Vec2> {
    vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)]
}

fn pocket(outline: Vec<Vec2>, islands: Vec<Vec<Vec2>>, depth: f64, climb: bool, tool: u32) -> Op {
    Op::Pocket2_5D { outline, islands, top: 0.0, depth, stepover: 2.0, chord: 0.01, climb, tool, feeds: feeds() }
}

fn inside(p: (f64, f64), ring: &[Vec2]) -> bool {
    let mut c = false;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        if (a.y > p.1) != (b.y > p.1) {
            let x = a.x + (p.1 - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.0 < x {
                c = !c;
            }
        }
    }
    c
}

fn dist_to(p: (f64, f64), ring: &[Vec2]) -> f64 {
    let mut m = f64::INFINITY;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        let (ex, ey) = (b.x - a.x, b.y - a.y);
        let t = (((p.0 - a.x) * ex + (p.1 - a.y) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        let (fx, fy) = (a.x + ex * t - p.0, a.y + ey * t - p.1);
        m = m.min((fx * fx + fy * fy).sqrt());
    }
    m
}

/// Points along every motion move (lines and arcs, helical included), with their Z.
fn samples(p: &Program) -> Vec<(f64, f64, f64)> {
    let mut out = Vec::new();
    let mut pos: Option<Vec3> = None;
    for mv in &p.moves {
        match mv {
            Move::Rapid { to } | Move::Linear { to, .. } => {
                if let Some(s) = pos {
                    for k in 0..=16 {
                        let t = k as f64 / 16.0;
                        out.push((s.x + (to.x - s.x) * t, s.y + (to.y - s.y) * t, s.z + (to.z - s.z) * t));
                    }
                }
                pos = Some(*to);
            }
            Move::Arc { to, centre, cw, .. } => {
                let s = pos.unwrap();
                let (dx, dy) = (s.x - centre.x, s.y - centre.y);
                let r = (dx * dx + dy * dy).sqrt();
                let a0 = dy.atan2(dx);
                let sw = arc_sweep(Vec2::new(s.x, s.y), Vec2::new(to.x, to.y), *centre, *cw, T);
                let sg = if *cw { -1.0 } else { 1.0 };
                for k in 0..=32 {
                    let t = k as f64 / 32.0;
                    let a = a0 + sg * sw * t;
                    out.push((centre.x + r * a.cos(), centre.y + r * a.sin(), s.z + (to.z - s.z) * t));
                }
                pos = Some(*to);
            }
            _ => {}
        }
    }
    out
}

/// Every sampled point below the top lies inside the outline and outside every island, at least `r` from each.
fn assert_inset(p: &Program, outline: &[Vec2], islands: &[Vec<Vec2>], r: f64) {
    for (x, y, z) in samples(p) {
        if z >= 0.0 {
            continue;
        }
        assert!(inside((x, y), outline), "({x}, {y}, {z}) outside the outline");
        assert!(dist_to((x, y), outline) >= r - 1e-6, "({x}, {y}, {z}) within r of the wall");
        for isl in islands {
            assert!(!inside((x, y), isl), "({x}, {y}, {z}) inside an island");
            assert!(dist_to((x, y), isl) >= r - 1e-6, "({x}, {y}, {z}) within r of an island");
        }
    }
}

fn close(a: &Move, b: &Move) -> bool {
    let v = |p: &Vec3, q: &Vec3| (p.x - q.x).abs() <= 1e-4 && (p.y - q.y).abs() <= 1e-4 && (p.z - q.z).abs() <= 1e-4;
    match (a, b) {
        (Move::Rapid { to: p }, Move::Rapid { to: q }) => v(p, q),
        (Move::Linear { to: p, feed: f }, Move::Linear { to: q, feed: g }) => v(p, q) && (f - g).abs() <= 1e-4,
        (Move::Arc { to: p, centre: c, cw, feed: f }, Move::Arc { to: q, centre: d, cw: dw, feed: g }) => {
            v(p, q) && (c.x - d.x).abs() <= 2e-4 && (c.y - d.y).abs() <= 2e-4 && cw == dw && (f - g).abs() <= 1e-4
        }
        _ => a == b,
    }
}

/// grbl's text is stable through a read, and the read program equals the original on the writer's 1e-4 grid.
fn grbl_round_trip(p: &Program) {
    let d = Dialect::Grbl { laser: false };
    let text = gcode::write(p, d).unwrap();
    let back = gcode::read(&text, d).unwrap();
    assert_eq!(gcode::write(&back, d).unwrap(), text);
    assert_eq!(back.moves.len(), p.moves.len());
    for (a, b) in p.moves.iter().zip(back.moves.iter()) {
        assert!(close(a, b), "{a:?} vs {b:?}");
    }
}

#[test]
fn square_pocket_has_the_hand_computed_rings_and_stays_inside_the_inset() {
    let m = mill();
    let outline = rect(5.0, 5.0, 45.0, 45.0);
    let p = generate(&pocket(outline.clone(), vec![], 1.0, true, 2), &m, T).unwrap();
    // By hand: DC 4 (r = 2), stepover 2: ring k at d = 2 + 2k while d < 20 (the half side), k = 0..8 — nine
    // squares of half side 20 - d = 18, 16, ..., 2 about (25, 25). Every fed line at the floor ends on one of them.
    let mut radii = BTreeSet::new();
    for mv in &p.moves {
        if let Move::Linear { to, feed } = mv {
            if *feed == 600.0 && (to.z + 1.0).abs() < 1e-12 {
                let cheb = (to.x - 25.0).abs().max((to.y - 25.0).abs());
                radii.insert((cheb * 1e6).round() as i64);
            }
        }
    }
    let want: BTreeSet<i64> = (1..=9).map(|k| 2_000_000 * k).collect();
    assert_eq!(radii, want);
    assert_inset(&p, &outline, &[], 2.0);
    // The entry is a helix at the plunge feed, never a straight plunge with the flat end.
    assert!(p.moves.iter().any(|mv| matches!(mv, Move::Arc { feed, .. } if *feed == 120.0)));
    let s = p.validate(&m, T).unwrap();
    assert!(s.time_s > 0.0);
    grbl_round_trip(&p);
}

#[test]
fn a_pocket_with_an_island_never_enters_the_island_inset() {
    let m = mill();
    let outline = rect(5.0, 5.0, 45.0, 45.0);
    let island = rect(20.0, 20.0, 30.0, 30.0);
    let p = generate(&pocket(outline.clone(), vec![island.clone()], 3.0, false, 2), &m, T).unwrap();
    assert_inset(&p, &outline, &[island], 2.0);
    // Two depth passes (APMX 2 over 3 mm).
    let floors: BTreeSet<i64> = samples(&p).iter().filter(|s| s.2 < 0.0).map(|s| (s.2 * 1e6).round() as i64).collect();
    assert_eq!(floors.iter().next(), Some(&-3_000_000));
    assert!(p.validate(&m, T).unwrap().time_s > 0.0);
    grbl_round_trip(&p);
}

#[test]
fn island_rings_carry_their_corner_arcs_as_g2_g3() {
    // VUGMAKEARCFIT (SR490). A pocket's convex corners take the mitre (the plain square's rings are sharp squares);
    // arcs come where the boundary turns AWAY from the offset: the island's corners. By hand, DC 4, stepover 2, the
    // 40 mm square with the central 10 mm island: d = 2, 4, 6 each hold the wall ring and the island ring (four quarter
    // arcs of radius d about the island corners); d = 8 holds four corner rings, each two wall-offset lines closed by
    // ONE arc of radius 8 about an island corner, from (13, 20 - sqrt 15) to (20 - sqrt 15, 13) for the corner (20, 20):
    // sweep 2 (atan(7 / sqrt 15) - pi/4); nothing is 10 from the boundary (the corner clearance peaks at 8.79).
    let m = mill();
    let outline = rect(5.0, 5.0, 45.0, 45.0);
    let island = rect(20.0, 20.0, 30.0, 30.0);
    let p = generate(&pocket(outline.clone(), vec![island.clone()], 1.0, false, 2), &m, T).unwrap();
    let mut sweep: std::collections::BTreeMap<(usize, i64), f64> = std::collections::BTreeMap::new();
    let mut pos: Option<Vec3> = None;
    for mv in &p.moves {
        match mv {
            Move::Arc { to, centre, cw, feed } => {
                let s = pos.unwrap();
                if *feed == 600.0 {
                    assert!(s.z == -1.0 && to.z == -1.0, "a fed arc off the floor: {mv:?}");
                    let c = island
                        .iter()
                        .position(|q| (q.x - centre.x).abs() < 1e-9 && (q.y - centre.y).abs() < 1e-9)
                        .unwrap_or_else(|| panic!("a fed arc about no island corner: {mv:?}"));
                    let r = ((s.x - centre.x).powi(2) + (s.y - centre.y).powi(2)).sqrt();
                    let sw = arc_sweep(Vec2::new(s.x, s.y), Vec2::new(to.x, to.y), *centre, *cw, T);
                    *sweep.entry((c, (r * 1e6).round() as i64)).or_insert(0.0) += sw;
                }
                pos = Some(*to);
            }
            Move::Rapid { to } | Move::Linear { to, .. } => pos = Some(*to),
            _ => {}
        }
    }
    let radii: BTreeSet<i64> = sweep.keys().map(|k| k.1).collect();
    assert_eq!(radii, [2, 4, 6, 8].iter().map(|d| d * 1_000_000).collect::<BTreeSet<i64>>());
    let r8 = 2.0 * ((7.0 / 15f64.sqrt()).atan() - std::f64::consts::FRAC_PI_4);
    for c in 0..4 {
        for (d, want) in [(2, std::f64::consts::FRAC_PI_2), (4, std::f64::consts::FRAC_PI_2), (6, std::f64::consts::FRAC_PI_2), (8, r8)] {
            let got = sweep.get(&(c, d * 1_000_000)).copied().unwrap_or(0.0);
            assert!((got - want).abs() < 1e-3, "corner {c} radius {d}: sweep {got} vs {want}");
        }
    }
    let text = gcode::write(&p, Dialect::Grbl { laser: false }).unwrap();
    let fed = text.lines().filter(|l| (l.starts_with("G2") || l.starts_with("G3")) && l.ends_with("F600")).count();
    assert!(fed >= 16, "{fed} fed G2 / G3 lines");
    assert_inset(&p, &outline, &[island], 2.0);
    assert!(p.validate(&m, T).is_ok());
    grbl_round_trip(&p);
}

#[test]
fn an_island_near_the_wall_merges_with_the_outline_rings() {
    // The island stands 3 mm off the left wall: narrower than DC 4, so from the first ring on the outline's ring and
    // the island's are one ring around the island's right side, and the channel is never entered.
    let m = mill();
    let outline = rect(5.0, 5.0, 45.0, 45.0);
    let island = rect(8.0, 20.0, 18.0, 30.0);
    let p = generate(&pocket(outline.clone(), vec![island.clone()], 1.0, true, 2), &m, T).unwrap();
    assert_inset(&p, &outline, &[island], 2.0);
    for (x, y, z) in samples(&p) {
        if z < 0.0 {
            assert!(!(x < 8.0 && y > 20.0 && y < 30.0), "({x}, {y}) in the channel");
        }
    }
    assert!(p.validate(&m, T).is_ok());
}

#[test]
fn climb_runs_the_wall_clockwise_and_conventional_counter_clockwise() {
    let m = mill();
    let wall_area = |climb: bool| {
        let p = generate(&pocket(rect(5.0, 5.0, 45.0, 45.0), vec![], 1.0, climb, 2), &m, T).unwrap();
        // The wall ring (half side 18) is cut last: its fed points at the floor, in order.
        let pts: Vec<(f64, f64)> = p
            .moves
            .iter()
            .filter_map(|mv| match mv {
                Move::Linear { to, feed } if *feed == 600.0 && (to.z + 1.0).abs() < 1e-12 => {
                    let cheb = (to.x - 25.0).abs().max((to.y - 25.0).abs());
                    if (cheb - 18.0).abs() < 1e-6 { Some((to.x, to.y)) } else { None }
                }
                _ => None,
            })
            .collect();
        let mut a = 0.0;
        for i in 0..pts.len() {
            let (p0, p1) = (pts[i], pts[(i + 1) % pts.len()]);
            a += p0.0 * p1.1 - p1.0 * p0.1;
        }
        a * 0.5
    };
    assert!(wall_area(true) < -1.0);
    assert!(wall_area(false) > 1.0);
}

#[test]
fn a_face_covers_its_rectangle() {
    let m = mill();
    for along in [FaceDir::X, FaceDir::Y] {
        let op = Op::Face {
            lo: Vec2::new(5.0, 5.0),
            hi: Vec2::new(45.0, 45.0),
            top: 0.0,
            depth: 0.5,
            stepover: 3.0,
            along,
            tool: 2,
            feeds: feeds(),
        };
        let p = generate(&op, &m, T).unwrap();
        // The cutting lines at the face's floor.
        let mut segs = Vec::new();
        let mut pos: Option<Vec3> = None;
        for mv in &p.moves {
            match mv {
                Move::Rapid { to } => pos = Some(*to),
                Move::Linear { to, feed } => {
                    if let Some(s) = pos {
                        if *feed == 600.0 && (s.z + 0.5).abs() < 1e-12 && (to.z + 0.5).abs() < 1e-12 {
                            segs.push(vec![Vec2::new(s.x, s.y), Vec2::new(to.x, to.y)]);
                        }
                    }
                    pos = Some(*to);
                }
                _ => {}
            }
        }
        // Every 1 mm cell of the rectangle is within the tool radius of a pass.
        for i in 0..40 {
            for j in 0..40 {
                let c = (5.5 + i as f64, 5.5 + j as f64);
                let near = segs.iter().any(|s| {
                    let (a, b) = (s[0], s[1]);
                    let (ex, ey) = (b.x - a.x, b.y - a.y);
                    let t = (((c.0 - a.x) * ex + (c.1 - a.y) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
                    let (fx, fy) = (a.x + ex * t - c.0, a.y + ey * t - c.1);
                    (fx * fx + fy * fy).sqrt() <= 2.0 + 1e-9
                });
                assert!(near, "{along:?}: cell {c:?} untouched");
            }
        }
        assert!(p.validate(&m, T).unwrap().time_s > 0.0);
        grbl_round_trip(&p);
    }
}

#[test]
fn pocket_and_face_refusals_by_name() {
    let m = mill();
    let tiny = rect(10.0, 10.0, 13.0, 13.0);
    assert_eq!(generate(&pocket(tiny, vec![], 1.0, true, 2), &m, T), Err(Refusal::ToolTooLarge { tool: 2 }));
    let outline = rect(5.0, 5.0, 45.0, 45.0);
    let across = rect(40.0, 40.0, 50.0, 50.0);
    assert_eq!(
        generate(&pocket(outline.clone(), vec![across], 1.0, true, 2), &m, T),
        Err(Refusal::IslandOutsideOutline { island: 0 })
    );
    let away = rect(60.0, 60.0, 70.0, 70.0);
    assert_eq!(
        generate(&pocket(outline.clone(), vec![rect(20.0, 20.0, 22.0, 22.0), away], 1.0, true, 2), &m, T),
        Err(Refusal::IslandOutsideOutline { island: 1 })
    );
    let bow = vec![Vec2::new(5.0, 5.0), Vec2::new(25.0, 15.0), Vec2::new(25.0, 5.0), Vec2::new(5.0, 25.0)];
    assert_eq!(generate(&pocket(bow, vec![], 1.0, true, 2), &m, T), Err(Refusal::OutlineSelfIntersects));
    let mut wide = pocket(outline.clone(), vec![], 1.0, true, 2);
    if let Op::Pocket2_5D { stepover, .. } = &mut wide {
        *stepover = 5.0;
    }
    assert_eq!(generate(&wide, &m, T), Err(Refusal::BadParameter { what: "stepover outside (0, DC]" }));
    // A 5 mm pocket: only the wall ring (half side 0.5), no room for a helix: the flat end has no way in, the ball
    // end plunges.
    let snug = rect(10.0, 10.0, 15.0, 15.0);
    assert_eq!(generate(&pocket(snug.clone(), vec![], 1.0, true, 2), &m, T), Err(Refusal::NoEntry { tool: 2 }));
    let ball = generate(&pocket(snug.clone(), vec![], 1.0, true, 3), &m, T).unwrap();
    assert!(!ball.moves.iter().any(|mv| matches!(mv, Move::Arc { .. })));
    assert_inset(&ball, &snug, &[], 2.0);
    assert!(ball.validate(&m, T).is_ok());
    let flipped = Op::Face {
        lo: Vec2::new(45.0, 5.0),
        hi: Vec2::new(5.0, 45.0),
        top: 0.0,
        depth: 0.5,
        stepover: 3.0,
        along: FaceDir::X,
        tool: 2,
        feeds: feeds(),
    };
    assert_eq!(generate(&flipped, &m, T), Err(Refusal::BadParameter { what: "face rectangle min above max" }));
}
