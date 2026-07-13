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

//! The safety-invariant test battery for the drive-service core.
//!
//! Every invariant I1–I8 from `docs/dev/USERLAND/ENDURO_SAFETY.md` gets its own
//! named test below; the safety-interlock checklist maps each bench precondition
//! to the test of the same number. All tests run on a simulated clock and mock
//! sink — deterministic, zero real sleeps.

use drive::sim::{FakeClock, MockSink};
use drive::{
    Actuation, CmdQueue, Command, Config, DriveCore, Frame, State, CMD_CAPACITY, NUM_CHANNELS,
};

const NEUTRAL: Actuation = Actuation {
    steering_us: 1500,
    throttle_us: 1500,
};

/// Encode a channel set into a real i-BUS servo frame and decode it back through
/// the parser, so every test frame is exactly what the wire would deliver
/// (checksum-valid; the range flag falls out of the channel values).
fn make_frame(channels: &[u16; NUM_CHANNELS]) -> Frame {
    let bytes = ibus::encode_servo_frame(channels);
    let mut p = ibus::Parser::new();
    let mut out = None;
    for &b in &bytes {
        if let Some(f) = p.push(b) {
            out = Some(f);
        }
    }
    out.expect("encode_servo_frame yields a decodable servo frame")
}

/// Channels with everything at neutral except the mode / steer / throttle sticks
/// (at the *default* config indices, which the tests use).
fn chans(mode_us: u16, steer_us: u16, throttle_us: u16) -> [u16; NUM_CHANNELS] {
    let c = Config::default();
    let mut ch = [1500u16; NUM_CHANNELS];
    ch[c.mode_ch] = mode_us;
    ch[c.steer_ch] = steer_us;
    ch[c.throttle_ch] = throttle_us;
    ch
}

// Mode-channel µs that classify to each state under the default config.
const DISARM_US: u16 = 1000; // < mode_lo (1250)
const MANUAL_US: u16 = 1500; // between
const AUTO_US: u16 = 2000; // > mode_hi (1750)

fn new_core() -> (DriveCore<MockSink, FakeClock>, MockSink, FakeClock) {
    let sink = MockSink::new();
    let log = sink.clone();
    let clk = FakeClock::new();
    let core = DriveCore::new(sink, clk.clone(), Config::default());
    (core, log, clk)
}

// ---------------------------------------------------------------------------
// I1 — neutral-unless-armed
// ---------------------------------------------------------------------------
#[test]
fn i1_neutral_unless_armed() {
    let (mut core, log, _clk) = new_core();
    // A DISARM frame with the sticks deflected must still yield exact neutral.
    core.on_frame(&make_frame(&chans(DISARM_US, 2000, 1900)));
    core.tick();
    assert_eq!(core.state(), State::Disarm);
    assert!(!core.is_armed());
    assert_eq!(core.last_output(), NEUTRAL);
    // Nothing but neutral has ever reached the sink.
    assert!(log.always_neutral(1500));
}

// ---------------------------------------------------------------------------
// I2 — deadman: >500 ms without a fresh valid frame ⇒ neutral + FAILSAFE latch,
//      and a late frame does NOT silently resume (re-arm required).
// ---------------------------------------------------------------------------
#[test]
fn i2_deadman_latches_failsafe_and_requires_rearm() {
    let (mut core, _log, clk) = new_core();

    // Arm (MANUAL, throttle neutral) and drive.
    core.on_frame(&make_frame(&chans(MANUAL_US, 1600, 1500)));
    core.tick();
    assert!(core.is_armed());
    assert_eq!(core.last_output().steering_us, 1600);

    // Signal loss: no frames, advance past the 500 ms window.
    clk.advance(501);
    core.tick();
    assert!(core.is_failsafe(), "deadman must latch failsafe");
    assert!(!core.is_armed());
    assert_eq!(core.last_output(), NEUTRAL);

    // A late valid frame must NOT clear the latch.
    clk.advance(10);
    core.on_frame(&make_frame(&chans(MANUAL_US, 1600, 1500)));
    core.tick();
    assert!(core.is_failsafe(), "a late frame must not silently resume");
    assert_eq!(core.last_output(), NEUTRAL);

    // Explicit DISARM clears the latch; only then can a deliberate arm resume.
    core.on_frame(&make_frame(&chans(DISARM_US, 1600, 1500)));
    core.tick();
    assert!(!core.is_failsafe());
    assert_eq!(core.state(), State::Disarm);

    core.on_frame(&make_frame(&chans(MANUAL_US, 1600, 1500)));
    core.tick();
    assert!(core.is_armed(), "re-arm succeeds after disarm");
    assert_eq!(core.last_output().steering_us, 1600);
}

// ---------------------------------------------------------------------------
// I3 — throttle cap: saturating, at the output boundary, both stick and AUTO.
// ---------------------------------------------------------------------------
#[test]
fn i3_throttle_cap_never_exceeded() {
    let cfg = Config::default();
    let (mut core, _log, clk) = new_core();

    // Arm at neutral throttle, then command throttle above the cap via sticks.
    core.on_frame(&make_frame(&chans(MANUAL_US, 1500, 1500)));
    core.tick();
    assert!(core.is_armed());

    clk.advance(20);
    core.on_frame(&make_frame(&chans(MANUAL_US, 1500, 2000))); // throttle 2000 > cap
    core.tick();
    assert_eq!(
        core.last_output().throttle_us,
        cfg.throttle_cap_us,
        "stick throttle saturates at the cap"
    );

    // Sweep every in-band throttle value: no tick may ever exceed the cap.
    for t in 900u16..=2100 {
        clk.advance(20);
        core.on_frame(&make_frame(&chans(MANUAL_US, 1500, t)));
        core.tick();
        assert!(
            core.last_output().throttle_us <= cfg.throttle_cap_us,
            "throttle {} produced output above cap",
            t
        );
    }
}

// ---------------------------------------------------------------------------
// I4 — disarm wins immediately from MANUAL, AUTO, and FAILSAFE, same tick.
// ---------------------------------------------------------------------------
#[test]
fn i4_disarm_from_every_state() {
    // From MANUAL.
    {
        let (mut core, _log, _clk) = new_core();
        core.on_frame(&make_frame(&chans(MANUAL_US, 1600, 1500)));
        core.tick();
        assert!(core.is_armed());
        core.on_frame(&make_frame(&chans(DISARM_US, 1600, 1500)));
        core.tick();
        assert_eq!(core.state(), State::Disarm);
        assert_eq!(core.last_output(), NEUTRAL);
    }
    // From AUTO.
    {
        let (mut core, _log, clk) = new_core();
        core.on_frame(&make_frame(&chans(AUTO_US, 1500, 1500)));
        core.tick();
        assert_eq!(core.state(), State::Auto);
        clk.advance(20);
        core.on_frame(&make_frame(&chans(DISARM_US, 1500, 1500)));
        core.tick();
        assert_eq!(core.state(), State::Disarm);
        assert_eq!(core.last_output(), NEUTRAL);
    }
    // From FAILSAFE.
    {
        let (mut core, _log, clk) = new_core();
        core.on_frame(&make_frame(&chans(MANUAL_US, 1600, 1500)));
        core.tick();
        clk.advance(501);
        core.tick();
        assert!(core.is_failsafe());
        core.on_frame(&make_frame(&chans(DISARM_US, 1600, 1500)));
        core.tick();
        assert!(!core.is_failsafe(), "disarm clears the failsafe latch");
        assert_eq!(core.state(), State::Disarm);
        assert_eq!(core.last_output(), NEUTRAL);
    }
}

// ---------------------------------------------------------------------------
// I5 — deliberate arm: DISARM→armed requires throttle-at-neutral in the frame.
// ---------------------------------------------------------------------------
#[test]
fn i5_deliberate_arm_requires_throttle_neutral() {
    let (mut core, _log, _clk) = new_core();

    // Boot with throttle high while selecting MANUAL: must NOT arm.
    core.on_frame(&make_frame(&chans(MANUAL_US, 1600, 1900)));
    core.tick();
    assert_eq!(core.state(), State::Disarm, "throttle-high arm is refused");
    assert!(!core.is_armed());
    assert_eq!(core.last_output(), NEUTRAL);

    // Now with throttle at neutral: arms.
    core.on_frame(&make_frame(&chans(MANUAL_US, 1600, 1500)));
    core.tick();
    assert!(core.is_armed());
    assert_eq!(core.state(), State::Manual);

    // Same rule guards AUTO. Fresh core, throttle-high into AUTO: refused.
    let (mut core2, _l2, _c2) = new_core();
    core2.on_frame(&make_frame(&chans(AUTO_US, 1500, 1900)));
    core2.tick();
    assert_eq!(core2.state(), State::Disarm);
    core2.on_frame(&make_frame(&chans(AUTO_US, 1500, 1500)));
    core2.tick();
    assert_eq!(core2.state(), State::Auto);
}

// ---------------------------------------------------------------------------
// I6 — invalid-range input never actuates and counts toward the deadman.
// ---------------------------------------------------------------------------
#[test]
fn i6_invalid_input_never_actuates_counts_deadman() {
    let (mut core, _log, clk) = new_core();

    // Arm and drive with a valid frame (steer 1600) at t=0.
    core.on_frame(&make_frame(&chans(MANUAL_US, 1600, 1500)));
    core.tick();
    assert!(core.is_armed());
    assert_eq!(core.last_output().steering_us, 1600);
    assert_eq!(core.stats().frames_accepted, 1);

    // An out-of-range frame (steer 5000) is refused: it never updates the sticks
    // and never refreshes the deadman.
    clk.advance(100);
    core.on_frame(&make_frame(&chans(MANUAL_US, 5000, 1500)));
    assert_eq!(core.stats().frames_rejected, 1);
    assert_eq!(core.stats().frames_accepted, 1, "invalid frame not accepted");
    core.tick();
    assert_eq!(
        core.last_output().steering_us,
        1600,
        "invalid frame must not reach the sink"
    );

    // Only invalid frames now arrive; the deadman still trips (last VALID = t=0).
    clk.advance(450); // t = 550 > 500 since last valid at t=0
    core.on_frame(&make_frame(&chans(MANUAL_US, 5000, 1500)));
    core.tick();
    assert!(
        core.is_failsafe(),
        "invalid frames count toward the deadman, they do not reset it"
    );
    assert_eq!(core.last_output(), NEUTRAL);
}

// ---------------------------------------------------------------------------
// I7 — panic/drop ⇒ neutral. Two witnesses: a plain scope drop and a panic
//      unwind (the kernel-panic-handler seam calls the same force_neutral).
// ---------------------------------------------------------------------------
#[test]
fn i7_drop_forces_neutral() {
    let sink = MockSink::new();
    let log = sink.clone();
    let clk = FakeClock::new();
    {
        let mut core = DriveCore::new(sink, clk.clone(), Config::default());
        core.on_frame(&make_frame(&chans(MANUAL_US, 1700, 1500)));
        core.tick();
        assert!(core.is_armed());
        assert_ne!(log.last().unwrap(), NEUTRAL, "driving before the drop");
    } // core dropped here
    assert_eq!(
        log.last().unwrap(),
        NEUTRAL,
        "Drop must force the sink neutral (I7)"
    );
}

#[test]
fn i7_panic_unwind_forces_neutral() {
    let sink = MockSink::new();
    let log = sink.clone();
    let clk = FakeClock::new();
    let clk2 = clk.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let mut core = DriveCore::new(sink, clk2, Config::default());
        core.on_frame(&make_frame(&chans(MANUAL_US, 1700, 1500)));
        core.tick();
        assert!(core.is_armed());
        panic!("simulated kernel fault mid-drive");
    }));
    assert!(result.is_err(), "the closure panicked as intended");
    assert_eq!(
        log.last().unwrap(),
        NEUTRAL,
        "an unwinding panic drops the core to neutral (I7)"
    );
}

// ---------------------------------------------------------------------------
// I8 — bounded command channel: fixed capacity, drop-oldest, never blocks.
// ---------------------------------------------------------------------------
#[test]
fn i8_command_channel_bounded_drop_oldest() {
    let mut q: CmdQueue<4> = CmdQueue::new();
    assert_eq!(q.capacity(), 4);
    for i in 0..4u16 {
        q.push(Command::new(1500, 1500 + i));
    }
    assert_eq!(q.len(), 4);
    assert_eq!(q.dropped(), 0);

    // Overflow: the OLDEST (throttle 1500) is dropped, the newest is retained.
    q.push(Command::new(1500, 9999));
    assert_eq!(q.len(), 4, "capacity is never exceeded");
    assert_eq!(q.dropped(), 1);

    assert_eq!(q.pop().unwrap().throttle_us, 1501, "oldest survivor after drop");
    assert_eq!(q.pop().unwrap().throttle_us, 1502);
    assert_eq!(q.pop().unwrap().throttle_us, 1503);
    assert_eq!(q.pop().unwrap().throttle_us, 9999, "newest retained");
    assert!(q.pop().is_none());
    assert!(q.is_empty());
}

#[test]
fn i8_auto_consumes_commands_and_caps() {
    let cfg = Config::default();
    let (mut core, _log, clk) = new_core();

    // Arm into AUTO (throttle neutral to arm).
    core.on_frame(&make_frame(&chans(AUTO_US, 1500, 1500)));
    core.tick();
    assert_eq!(core.state(), State::Auto);

    // Overflowing the channel never blocks and stays bounded.
    for i in 0..(CMD_CAPACITY as u16 + 5) {
        core.push_command(Command::new(1550, 1600 + i));
    }
    assert_eq!(core.commands_queued(), CMD_CAPACITY);

    // AUTO consumes one command per tick; the value is capped at the boundary.
    core.push_command(Command::new(1550, 2000)); // above cap
    // (drops one more oldest; still bounded)
    assert_eq!(core.commands_queued(), CMD_CAPACITY);

    clk.advance(20);
    core.on_frame(&make_frame(&chans(AUTO_US, 1500, 1500)));
    core.tick();
    assert_eq!(core.last_output().steering_us, 1550);
    assert!(core.last_output().throttle_us <= cfg.throttle_cap_us);

    // Drain the queue, then AUTO holds neutral when the channel is empty.
    while core.commands_queued() > 0 {
        clk.advance(20);
        core.on_frame(&make_frame(&chans(AUTO_US, 1500, 1500)));
        core.tick();
    }
    clk.advance(20);
    core.on_frame(&make_frame(&chans(AUTO_US, 1500, 1500)));
    core.tick();
    assert_eq!(core.last_output(), NEUTRAL, "empty AUTO channel holds neutral");
}

// ---------------------------------------------------------------------------
// Scenario — arm → drive → signal-loss → deadman → re-arm, asserting the full
// sink trace end to end.
// ---------------------------------------------------------------------------
#[test]
fn scenario_full_sink_trace() {
    let (mut core, log, clk) = new_core();

    // t=0: arm + drive.
    core.on_frame(&make_frame(&chans(MANUAL_US, 1600, 1500)));
    core.tick();
    // t=20, t=40: steer sweep while armed.
    clk.advance(20);
    core.on_frame(&make_frame(&chans(MANUAL_US, 1650, 1500)));
    core.tick();
    clk.advance(20);
    core.on_frame(&make_frame(&chans(MANUAL_US, 1700, 1500)));
    core.tick();

    // Signal loss from t=40; at t=560 the deadman (last valid t=40) trips.
    clk.advance(520);
    core.tick();

    // Late frame at t=580 does not resume.
    clk.advance(20);
    core.on_frame(&make_frame(&chans(MANUAL_US, 1600, 1500)));
    core.tick();

    // Disarm at t=600 clears the latch.
    clk.advance(20);
    core.on_frame(&make_frame(&chans(DISARM_US, 1600, 1500)));
    core.tick();

    // Re-arm at t=620.
    clk.advance(20);
    core.on_frame(&make_frame(&chans(MANUAL_US, 1600, 1500)));
    core.tick();

    let drive = |s: u16| Actuation {
        steering_us: s,
        throttle_us: 1500,
    };
    let expected = vec![
        NEUTRAL,     // DriveCore::new pre-drives neutral
        drive(1600), // t=0  armed
        drive(1650), // t=20
        drive(1700), // t=40
        NEUTRAL,     // t=560 deadman → failsafe neutral
        NEUTRAL,     // t=580 late frame ignored
        NEUTRAL,     // t=600 disarm
        drive(1600), // t=620 re-armed
    ];
    assert_eq!(core.last_output(), drive(1600));
    assert_eq!(log.trace(), expected, "full sink trace matches the scenario");
}
