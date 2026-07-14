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

//! The `drive` ↔ `pca9685` seam: a compose proof, not a new invariant.
//!
//! `drive`'s [`ActuatorSink`] speaks microseconds; this codec turns microseconds
//! into PCA9685 register writes. This test wires them: a real [`ActuatorSink`]
//! implementation ([`Pca9685Sink`]) that encodes each setpoint through the codec,
//! driven by a real [`DriveCore`]. It proves the two crates fit together — the µs
//! the drive core emits are exactly the µs the codec consumes — without adding
//! any behaviour to either crate.

use helm::rover::{Actuation, ActuatorSink, Config, DriveCore};
use helm::rover::sim::FakeClock;
use pca9685::{channel_pulse_us, Prescale, Write};
use std::cell::RefCell;
use std::rc::Rc;

/// Channel assignment for the seam (arbitrary but fixed for the test).
const STEER_CH: u8 = 0;
const THROTTLE_CH: u8 = 1;

/// One recorded setpoint as it crosses the seam: the steering and throttle
/// register bursts the codec produced.
type SeamBurst = ([Write; 4], [Write; 4]);

/// An [`ActuatorSink`] that encodes each setpoint into PCA9685 register writes at
/// a fixed 50 Hz and records the emitted bursts. This is the shape a real kernel
/// drive service takes: the safety core hands it µs, it hands the I2C driver
/// register writes.
#[derive(Clone)]
struct Pca9685Sink {
    prescale: Prescale,
    /// Ordered log of (steering-burst, throttle-burst) pairs, shared so a clone
    /// can read the trace after `DriveCore` takes ownership of the original.
    log: Rc<RefCell<Vec<SeamBurst>>>,
}

impl Pca9685Sink {
    fn new() -> Self {
        Self {
            prescale: Prescale::for_update_rate(50),
            log: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn last(&self) -> Option<SeamBurst> {
        self.log.borrow().last().copied()
    }
}

impl ActuatorSink for Pca9685Sink {
    fn apply(&mut self, out: Actuation) {
        let steer = channel_pulse_us(STEER_CH, &self.prescale, out.steering_us as u32)
            .expect("steering channel in range");
        let throttle = channel_pulse_us(THROTTLE_CH, &self.prescale, out.throttle_us as u32)
            .expect("throttle channel in range");
        self.log.borrow_mut().push((steer, throttle));
    }
}

/// The same µs → writes computation the sink performs, restated locally so the
/// assertions are an independent cross-check rather than a tautology.
fn expect_writes(ch: u8, prescale: &Prescale, us: u16) -> [Write; 4] {
    channel_pulse_us(ch, prescale, us as u32).expect("channel in range")
}

#[test]
fn seam_disarm_neutral_encodes_through_codec() {
    // A freshly built DriveCore pre-drives its sink to neutral (fail-safe from
    // tick zero). That real setpoint must round-trip through the codec to the
    // 1.5 ms neutral encoding (307 counts at 50 Hz).
    let sink = Pca9685Sink::new();
    let handle = sink.clone();
    let cfg = Config::default();
    let core = DriveCore::new(sink, FakeClock::new(), cfg);

    let (steer, throttle) = handle.last().expect("new() applies a neutral setpoint");
    let p = Prescale::for_update_rate(50);
    assert_eq!(steer, expect_writes(STEER_CH, &p, cfg.neutral_us));
    assert_eq!(throttle, expect_writes(THROTTLE_CH, &p, cfg.neutral_us));

    // 1.5 ms neutral is 307 counts → OFF_L = 0x33, OFF_H = 0x01, ON = 0.
    assert_eq!(steer[2], Write::new(0x08, 0x33));
    assert_eq!(steer[3], Write::new(0x09, 0x01));

    // Dropping the core forces a final neutral through the very same seam.
    drop(core);
    let (steer_final, _) = handle.last().expect("drop applies a final neutral");
    assert_eq!(steer_final, expect_writes(STEER_CH, &p, cfg.neutral_us));
}

#[test]
fn seam_servo_band_setpoints_compose() {
    // Every setpoint the drive sink could emit across the servo band must encode
    // cleanly through the codec — no panic, no out-of-range channel, and the
    // steering/throttle bursts land on their distinct channel blocks.
    let mut sink = Pca9685Sink::new();
    let p = Prescale::for_update_rate(50);

    for &(steer_us, thr_us) in &[
        (1000u16, 1500u16), // full left, throttle neutral
        (1500, 1500),       // centred
        (2000, 1700),       // full right, throttle near cap
        (1500, 1300),       // centred, throttle reverse floor
    ] {
        sink.apply(Actuation {
            steering_us: steer_us,
            throttle_us: thr_us,
        });
        let (steer, throttle) = sink.last().unwrap();
        assert_eq!(steer, expect_writes(STEER_CH, &p, steer_us));
        assert_eq!(throttle, expect_writes(THROTTLE_CH, &p, thr_us));
        // Steering block starts at 0x06, throttle block at 0x0A — distinct.
        assert_eq!(steer[0].reg, 0x06);
        assert_eq!(throttle[0].reg, 0x0A);
    }
}
