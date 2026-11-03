// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.

//! The gate-invariant test battery for the machine gate (VUGHELM, LEDGER SR475).
//!
//! Every invariant G1–G9 from `helm::machine`'s module doc gets its own named test below. Every review is make_core's
//! real `review` of a program on a mill in make_core's own test fixture shape — never a hand-built `Review`; G8's
//! unbound leg derives one from a real review by removing its `FirstRun` ask (VUG-HELM-BIND, LEDGER SR489).
//! G9 reviews one program on two machines (VUG-HELM-BIND-MACHINE, LEDGER SR498).

use helm::machine::{Approved, Level, Policy, Verdict, approve, gate};
use make_core::{
    Aabb3, Ask, Axis, AxisId, Fixture, Kinematics, Machine, MachineKind, Move, Power, PowerKind, Program, Review,
    Spin, Stock, StockShape, Tol, Tool, ToolKind, Units, Vec3, review,
};

const T: Tol = Tol::DEFAULT;
const LEVELS: [Level; 3] = [Level::Never, Level::Ask, Level::Pass];

fn axis(id: AxisId, min: f64, max: f64) -> Axis {
    Axis { id, min, max, max_feed: 3000.0, max_accel: 100.0, rapid: 6000.0 }
}

/// make_core's test mill: spindle asks above 18000 rpm, tools 1 (drill) and 2 (flat end), a vise jaw keep-out.
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
            Tool { number: 1, kind: ToolKind::Drill, dc: 3.0, lf: 30.0, apmx: 3.0, nof: 2, sig: 2.0944 },
            Tool { number: 2, kind: ToolKind::FlatEnd, dc: 4.0, lf: 20.0, apmx: 2.0, nof: 3, sig: 0.0 },
        ],
        stock: Some(Stock { shape: StockShape::Box { size: Vec3::new(50.0, 50.0, 10.0) }, origin: Vec3::new(0.0, 0.0, -10.0) }),
        fixtures: vec![Fixture {
            name: "vise jaw".to_string(),
            keepout: vec![Aabb3 { min: Vec3::new(60.0, 0.0, -10.0), max: Vec3::new(70.0, 50.0, 10.0) }],
        }],
        loaded: Some(1),
    }
}

fn prog(moves: Vec<Move>) -> Program {
    let mut p = Program::new(Units::Mm);
    p.moves = moves;
    p
}

/// A short slot at 10000 rpm (below the ask threshold): no refusal, the one FirstRun ask.
fn safe(x_end: f64) -> Program {
    prog(vec![
        Move::Spindle(Spin::Cw(10000.0)),
        Move::Rapid { to: Vec3::new(20.0, 20.0, 5.0) },
        Move::Linear { to: Vec3::new(20.0, 20.0, -1.0), feed: 100.0 },
        Move::Linear { to: Vec3::new(x_end, 20.0, -1.0), feed: 600.0 },
        Move::Rapid { to: Vec3::new(x_end, 20.0, 5.0) },
        Move::Spindle(Spin::Off),
    ])
}

/// The same slot after a tool change to 2 at 20000 rpm: FirstRun, ToolChange at 0, PowerAbove at 1.
fn busy() -> Program {
    let mut moves = vec![Move::ToolChange { tool: 2 }, Move::Spindle(Spin::Cw(20000.0))];
    moves.extend(safe(30.0).moves.into_iter().skip(1));
    prog(moves)
}

/// A rapid outside the envelope: one refusal, plus its FirstRun ask.
fn bad() -> Program {
    prog(vec![Move::Rapid { to: Vec3::new(150.0, 0.0, 20.0) }])
}

fn rev(p: &Program) -> Review {
    review(p, &mill(), T)
}

fn every_policy() -> Vec<Policy> {
    let mut v = Vec::new();
    for first_run in LEVELS {
        for tool_change in LEVELS {
            for power_above in LEVELS {
                v.push(Policy { first_run, tool_change, power_above });
            }
        }
    }
    v
}

// ---------------------------------------------------------------------------
// G1 — refusal-wins: any refusal is Refuse under every policy, approved or not.
// ---------------------------------------------------------------------------
#[test]
fn g1_a_refusal_cannot_be_passed_by_any_policy() {
    let p = bad();
    let r = rev(&p);
    assert_eq!(r.refusals.len(), 1, "the fixture refuses once: {:?}", r.refusals);
    let mut approved = Approved::default();
    approve(&mut approved, p.digest(), "peter", 1);
    let policies = every_policy();
    assert_eq!(policies.len(), 27);
    for pol in &policies {
        for a in [&Approved::default(), &approved] {
            match gate(&r, pol, a) {
                Verdict::Refuse { refusals, .. } => assert_eq!(refusals, r.refusals),
                v => panic!("{pol:?} let a refusal through as {v:?}"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// G2 — first-run remembered: an approved digest passes; a changed program asks again.
// ---------------------------------------------------------------------------
#[test]
fn g2_an_approved_digest_passes_and_a_changed_program_does_not() {
    let pol = Policy::default();
    let mut approved = Approved::default();
    let p = safe(30.0);
    let r = rev(&p);
    assert_eq!(gate(&r, &pol, &approved), Verdict::Ask(vec![Ask::FirstRun { digest: p.digest() }]));
    assert!(approve(&mut approved, p.digest(), "peter", 1_000));
    assert_eq!(gate(&r, &pol, &approved), Verdict::Pass { digest: p.digest(), machine: mill().digest() });
    // The same program reviewed again still passes.
    assert!(gate(&rev(&safe(30.0)), &pol, &approved).is_pass());
    // One move changed: a new digest, asked again.
    let q = safe(31.0);
    assert_ne!(p.digest(), q.digest());
    assert_eq!(gate(&rev(&q), &pol, &approved), Verdict::Ask(vec![Ask::FirstRun { digest: q.digest() }]));
}

// ---------------------------------------------------------------------------
// G3 — never-by-name: an ask at Level::Never refuses, carrying the ask.
// ---------------------------------------------------------------------------
#[test]
fn g3_never_refuses_the_ask_by_name() {
    let pol = Policy { tool_change: Level::Never, ..Policy::default() };
    let p = busy();
    let r = rev(&p);
    assert_eq!(r.refusals, vec![]);
    assert_eq!(
        gate(&r, &pol, &Approved::default()),
        Verdict::Refuse { refusals: vec![], never: vec![Ask::ToolChange { at: 0, tool: 2 }] }
    );
    // Approval of the digest does not lift a Never.
    let mut approved = Approved::default();
    approve(&mut approved, p.digest(), "peter", 1);
    assert!(matches!(gate(&r, &pol, &approved), Verdict::Refuse { .. }));
    // Beside a machine refusal, both are named.
    let b = bad();
    let pol = Policy { first_run: Level::Never, ..Policy::default() };
    let rb = rev(&b);
    assert_eq!(
        gate(&rb, &pol, &Approved::default()),
        Verdict::Refuse { refusals: rb.refusals.clone(), never: vec![Ask::FirstRun { digest: b.digest() }] }
    );
}

// ---------------------------------------------------------------------------
// G4 — pass-level drops: an ask at Level::Pass is not asked.
// ---------------------------------------------------------------------------
#[test]
fn g4_pass_level_drops_the_ask() {
    let pol = Policy { first_run: Level::Pass, tool_change: Level::Pass, power_above: Level::Pass };
    assert_eq!(gate(&rev(&busy()), &pol, &Approved::default()), Verdict::Pass { digest: busy().digest(), machine: mill().digest() });
    let pol = Policy { tool_change: Level::Pass, ..Policy::default() };
    let p = busy();
    assert_eq!(
        gate(&rev(&p), &pol, &Approved::default()),
        Verdict::Ask(vec![
            Ask::FirstRun { digest: p.digest() },
            Ask::PowerAbove { at: 1, value: 20000.0, threshold: 18000.0 },
        ])
    );
}

// ---------------------------------------------------------------------------
// G5 — default asks all, in review order.
// ---------------------------------------------------------------------------
#[test]
fn g5_the_default_policy_asks_every_ask_in_order() {
    let p = busy();
    let r = rev(&p);
    assert_eq!(r.refusals, vec![]);
    assert_eq!(
        gate(&r, &Policy::default(), &Approved::default()),
        Verdict::Ask(vec![
            Ask::FirstRun { digest: p.digest() },
            Ask::ToolChange { at: 0, tool: 2 },
            Ask::PowerAbove { at: 1, value: 20000.0, threshold: 18000.0 },
        ])
    );
}

// ---------------------------------------------------------------------------
// G6 — approval is per digest only: ToolChange and PowerAbove are asked every run.
// ---------------------------------------------------------------------------
#[test]
fn g6_an_approval_never_drops_a_tool_change_or_a_power_ask() {
    let p = busy();
    let mut approved = Approved::default();
    approve(&mut approved, p.digest(), "peter", 1);
    assert_eq!(
        gate(&rev(&p), &Policy::default(), &approved),
        Verdict::Ask(vec![
            Ask::ToolChange { at: 0, tool: 2 },
            Ask::PowerAbove { at: 1, value: 20000.0, threshold: 18000.0 },
        ])
    );
}

// ---------------------------------------------------------------------------
// G7 — approve is idempotent: one record per digest, the first person kept.
// ---------------------------------------------------------------------------
#[test]
fn g7_approve_is_idempotent() {
    let d = safe(30.0).digest();
    let mut approved = Approved::default();
    assert!(approve(&mut approved, d, "peter", 10));
    assert!(!approve(&mut approved, d, "una", 20));
    assert_eq!(approved.digests.len(), 1);
    assert_eq!(approved.digests[0].who, "peter");
    assert_eq!(approved.digests[0].when_us, 10);
    assert!(approved.covers(d));
    assert!(!approved.covers(d ^ 1));
}

// ---------------------------------------------------------------------------
// G8 — a pass names its program (VUG-HELM-BIND, SR489): the digest of the program reviewed; no program, no pass.
// ---------------------------------------------------------------------------
#[test]
fn g8_a_pass_names_the_program_it_passed() {
    let pol = Policy::default();
    let (p, q) = (safe(30.0), safe(31.0));
    assert_ne!(p.digest(), q.digest());
    let mut approved = Approved::default();
    approve(&mut approved, p.digest(), "peter", 1);
    approve(&mut approved, q.digest(), "peter", 2);
    // The same asks on two programs: each pass names its own program, and the two passes differ.
    let (vp, vq) = (gate(&rev(&p), &pol, &approved), gate(&rev(&q), &pol, &approved));
    assert_eq!(vp, Verdict::Pass { digest: p.digest(), machine: mill().digest() });
    assert_eq!(vq, Verdict::Pass { digest: q.digest(), machine: mill().digest() });
    assert_ne!(vp, vq, "a pass for one program is not a pass for another");
    // A pass at the all-Pass levels still names the program it was issued for.
    let all = Policy { first_run: Level::Pass, tool_change: Level::Pass, power_above: Level::Pass };
    let b = busy();
    assert_eq!(gate(&rev(&b), &all, &Approved::default()), Verdict::Pass { digest: b.digest(), machine: mill().digest() });
    // A review that names no program (its FirstRun removed) is Unbound, never a Pass, under every policy.
    let mut r = rev(&b);
    r.asks.retain(|a| !matches!(a, Ask::FirstRun { .. }));
    assert_eq!(gate(&r, &all, &Approved::default()), Verdict::Unbound);
    for pol in &every_policy() {
        let v = gate(&r, pol, &approved);
        assert!(!v.is_pass(), "{pol:?} passed a review that names no program as {v:?}");
    }
}

// ---------------------------------------------------------------------------
// G9 — a pass names its machine (VUG-HELM-BIND-MACHINE, SR498): the same program passed on two machines is two passes;
// the machine's identity ignores its setup and state.
// ---------------------------------------------------------------------------

/// A 100 mm desktop router: the mill's tools and spindle, a smaller envelope that still holds the `safe` slot.
fn router() -> Machine {
    Machine {
        axes: vec![axis(AxisId::X, -10.0, 110.0), axis(AxisId::Y, -10.0, 110.0), axis(AxisId::Z, -30.0, 40.0)],
        envelope: Aabb3 { min: Vec3::new(0.0, 0.0, -20.0), max: Vec3::new(100.0, 100.0, 30.0) },
        ..mill()
    }
}

#[test]
fn g9_a_pass_names_the_machine_it_passed() {
    let (a, b) = (mill(), router());
    assert_ne!(a.digest(), b.digest(), "two machines, two identities");
    let p = safe(30.0);
    let mut approved = Approved::default();
    approve(&mut approved, p.digest(), "peter", 1);
    let (ra, rb) = (review(&p, &a, T), review(&p, &b, T));
    assert_eq!((ra.refusals.len(), rb.refusals.len()), (0, 0), "{:?} {:?}", ra.refusals, rb.refusals);
    assert_eq!((ra.machine, rb.machine), (a.digest(), b.digest()), "the review names the machine it was made for");
    let pol = Policy::default();
    let (va, vb) = (gate(&ra, &pol, &approved), gate(&rb, &pol, &approved));
    assert_eq!(va, Verdict::Pass { digest: p.digest(), machine: a.digest() });
    assert_eq!(vb, Verdict::Pass { digest: p.digest(), machine: b.digest() });
    assert_ne!(va, vb, "a pass for one machine is not a pass for another");

    // The setup and the state are not the machine: the identity holds through them.
    let mut set = mill();
    set.stock = None;
    set.fixtures.clear();
    set.loaded = None;
    set.safe_z = 10.0;
    set.power.ask_above = 20000.0;
    assert_eq!(set.digest(), a.digest(), "stock, fixtures, loaded tool, safe Z and the ask threshold are not identity");
    // The identity is: an envelope, a tool or the power axis changed is another machine.
    let mut env = mill();
    env.envelope.max.x = 90.0;
    assert_ne!(env.digest(), a.digest());
    let mut tool = mill();
    tool.tools[1].dc = 6.0;
    assert_ne!(tool.digest(), a.digest());
    let mut power = mill();
    power.power.kind = PowerKind::Laser;
    assert_ne!(power.digest(), a.digest());
}
