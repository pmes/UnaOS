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

//! Deterministic simulation harness (host-only, behind the `std` feature).
//!
//! [`FakeClock`] and [`MockSink`] let the drive-service invariants be tested
//! with **zero real sleeps**: the test advances a simulated millisecond clock by
//! hand and reads back every setpoint the core wrote. Both share their state
//! through an `Rc` handle, so a test keeps a clone to drive/inspect after the
//! `DriveCore` has taken ownership of the originals — and, crucially, after the
//! core has been *dropped* (needed to witness the I7 drop-to-neutral guarantee).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::{Actuation, ActuatorSink, Clock};

/// A hand-advanced monotonic millisecond clock. Cloning yields another handle
/// onto the same time value.
#[derive(Clone, Debug, Default)]
pub struct FakeClock {
    now: Rc<Cell<u64>>,
}

impl FakeClock {
    /// A clock starting at `t = 0`.
    pub fn new() -> Self {
        Self {
            now: Rc::new(Cell::new(0)),
        }
    }

    /// Start at an explicit time.
    pub fn at(ms: u64) -> Self {
        Self {
            now: Rc::new(Cell::new(ms)),
        }
    }

    /// Advance simulated time by `ms` milliseconds.
    pub fn advance(&self, ms: u64) {
        self.now.set(self.now.get() + ms);
    }

    /// Set simulated time to an absolute value.
    pub fn set(&self, ms: u64) {
        self.now.set(ms);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.now.get()
    }
}

/// An [`ActuatorSink`] that records every setpoint it is handed. Clone it before
/// moving one copy into a [`DriveCore`](super::DriveCore); the retained clone
/// reads the trace, including the final neutral written on drop.
#[derive(Clone, Debug, Default)]
pub struct MockSink {
    log: Rc<RefCell<Vec<Actuation>>>,
}

impl MockSink {
    /// A sink with an empty trace.
    pub fn new() -> Self {
        Self {
            log: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// The full ordered trace of setpoints applied so far.
    pub fn trace(&self) -> Vec<Actuation> {
        self.log.borrow().clone()
    }

    /// The most recent setpoint, or `None` if nothing has been applied.
    pub fn last(&self) -> Option<Actuation> {
        self.log.borrow().last().copied()
    }

    /// Number of setpoints applied.
    pub fn len(&self) -> usize {
        self.log.borrow().len()
    }

    /// `true` when no setpoint has been applied.
    pub fn is_empty(&self) -> bool {
        self.log.borrow().is_empty()
    }

    /// `true` if every setpoint applied since construction has been exactly
    /// `neutral` (steering == throttle == `neutral_us`). Used to assert that a
    /// scenario never once left neutral.
    pub fn always_neutral(&self, neutral_us: u16) -> bool {
        self.log
            .borrow()
            .iter()
            .all(|a| a.steering_us == neutral_us && a.throttle_us == neutral_us)
    }
}

impl ActuatorSink for MockSink {
    fn apply(&mut self, out: Actuation) {
        self.log.borrow_mut().push(out);
    }
}
