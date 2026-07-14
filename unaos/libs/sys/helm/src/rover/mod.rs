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
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! The arch-neutral rover control core — TALUS's safety state machine.
//!
//! This is the first per-machine domain of the kernel [`helm`](crate) core: the
//! rover's DISARM/MANUAL/AUTO interlock with a FAILSAFE latch. Failsafes do not
//! generalize (a rover's safe state is "stop"), so each machine class is its own
//! domain module under `helm`.
//!
//! ROADMAP §4 puts UnaOS between the RC receiver and the actuators. This module
//! is the safety-critical middle: it takes decoded [`ibus::Frame`]s in, runs a
//! DISARM / MANUAL / AUTO state machine with a FAILSAFE latch, and drives an
//! injected [`ActuatorSink`] in microseconds. The transmitter is the human
//! override and estop at every stage — the mode channel is authoritative.
//!
//! # Design constraints
//!
//! - `#![no_std]`, `#![forbid(unsafe_code)]`, no external dependencies (only our
//!   own `ibus`). The kernel rover service embeds this same core later.
//! - **Injected seams** so tests are deterministic with zero real sleeps: a
//!   monotonic [`Clock`] (ms) and an [`ActuatorSink`] (steering µs + throttle
//!   µs). The default-on `std` feature adds the [`sim`] harness (FakeClock +
//!   MockSink).
//! - The harness calls [`DriveCore::tick`] at a fixed cadence (20 ms) and
//!   [`DriveCore::on_frame`] as frames arrive (~7 ms).
//!
//! # The safety invariants (each proved by a named test in `tests/`)
//!
//! - **I1 neutral-unless-armed** — the sink receives exact neutral in
//!   DISARM/FAILSAFE, always.
//! - **I2 deadman** — >500 ms without a fresh *valid* frame while armed forces
//!   neutral and latches FAILSAFE; re-arm is required.
//! - **I3 throttle cap** — a saturating clamp at the output boundary, applied
//!   last; no path emits throttle above the cap while armed.
//! - **I4 disarm-from-every-state** — a DISARM mode channel wins immediately,
//!   from MANUAL, AUTO, and FAILSAFE, in the same tick it is seen.
//! - **I5 deliberate arm** — DISARM→MANUAL/AUTO requires throttle-at-neutral in
//!   the same frame, and that frame must be fresh (within the deadman window);
//!   a boot-with-throttle-high must not drive, nor may a stale frame arm.
//! - **I6 invalid-input-never-actuates** — an out-of-range frame (checksum
//!   failures never even reach here) never touches the sink and does not refresh
//!   the deadman; it counts toward it.
//! - **I7 panic/drop ⇒ neutral** — [`DriveCore`]'s `Drop` forces the sink
//!   neutral; the kernel panic handler calls the same [`DriveCore::force_neutral`].
//! - **I8 bounded command channel** — AUTO commands queue in a fixed-capacity,
//!   drop-oldest ring ([`CmdQueue`]); it never blocks the tick.

mod command;
#[cfg(feature = "std")]
pub mod sim;

pub use command::{CmdQueue, Command};
pub use ibus::{Frame, NUM_CHANNELS};

/// Capacity of the AUTO command channel (invariant I8). Fixed at compile time.
pub const CMD_CAPACITY: usize = 16;

/// An actuator setpoint: steering + throttle, both as servo pulse widths in µs.
/// The sink speaks µs and nothing else — the actuation backend (PCA9685 over
/// I2C, a later arc) translates µs to duty; the core never assumes Tegra PWM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Actuation {
    /// Steering servo pulse, µs.
    pub steering_us: u16,
    /// Throttle/ESC pulse, µs.
    pub throttle_us: u16,
}

/// A monotonic millisecond clock. Injected so tests drive a deterministic
/// [`sim::FakeClock`] — never wall-clock time (a hard-won rule: timing derived
/// from real time makes safety tests flaky).
pub trait Clock {
    /// Monotonically non-decreasing milliseconds since some fixed origin.
    fn now_ms(&self) -> u64;
}

/// The actuator output boundary. The core owns its sink so `Drop` can force it
/// neutral (I7). A real implementation writes PWM; the mock records setpoints.
pub trait ActuatorSink {
    /// Apply a setpoint. Called at most once per [`DriveCore::tick`], plus once
    /// on `force_neutral` / drop.
    fn apply(&mut self, out: Actuation);
}

/// The armed/disarmed mode selected by the 3-position transmitter channel.
/// FAILSAFE is a *latch* layered on top of this (see [`DriveCore::is_failsafe`]),
/// not a variant here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Outputs neutral; the safe resting state and the only state you re-arm from.
    Disarm,
    /// Steering + throttle follow the transmitter's stick channels.
    Manual,
    /// Steering + throttle follow the bounded autonomy command channel.
    Auto,
}

/// Drive-service configuration. Plain data with safe defaults; every field is
/// explicit so a bench can pin thresholds without touching code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Channel index of the 3-position mode switch.
    pub mode_ch: usize,
    /// Channel index of the steering stick.
    pub steer_ch: usize,
    /// Channel index of the throttle stick.
    pub throttle_ch: usize,

    /// Neutral pulse width, µs (steering centre / throttle stop).
    pub neutral_us: u16,
    /// Upper throttle saturation, µs (I3). No armed output exceeds this.
    pub throttle_cap_us: u16,
    /// Lower throttle saturation, µs (reverse/brake limit).
    pub throttle_floor_us: u16,
    /// Lower steering clamp, µs.
    pub steer_min_us: u16,
    /// Upper steering clamp, µs.
    pub steer_max_us: u16,

    /// Deadman window, ms: armed + no fresh valid frame beyond this ⇒ failsafe.
    pub deadman_ms: u64,

    /// Mode channel below this ⇒ DISARM.
    pub mode_lo_us: u16,
    /// Mode channel above this ⇒ AUTO. Between the two ⇒ MANUAL.
    pub mode_hi_us: u16,
    /// Throttle must be within ±this of neutral to permit a deliberate arm (I5).
    pub arm_neutral_tol_us: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode_ch: 4,
            steer_ch: 0,
            throttle_ch: 1,
            neutral_us: 1500,
            throttle_cap_us: 1700,
            throttle_floor_us: 1300,
            steer_min_us: 1000,
            steer_max_us: 2000,
            deadman_ms: 500,
            mode_lo_us: 1250,
            mode_hi_us: 1750,
            arm_neutral_tol_us: 60,
        }
    }
}

/// Cheap tallies for a witness / the future kernel drive service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Valid, in-range frames accepted (refreshed the deadman).
    pub frames_accepted: u32,
    /// Out-of-range frames rejected (counted toward the deadman, never actuated).
    pub frames_rejected: u32,
    /// Times FAILSAFE has latched.
    pub failsafes: u32,
}

/// The drive-service core: state machine + safety envelope over an injected
/// clock and actuator sink.
///
/// # Kernel seam (I7)
///
/// The core **owns** its sink; `Drop` calls [`force_neutral`](Self::force_neutral),
/// so an unwind or a scope exit always leaves the actuator at neutral. The
/// kernel's panic handler hooks the identical path: it holds the drive instance
/// and calls `force_neutral` before halting, so a kernel fault parks the wheels
/// exactly as a host-side drop does.
#[derive(Debug)]
pub struct DriveCore<S: ActuatorSink, C: Clock> {
    cfg: Config,
    clock: C,
    sink: S,

    state: State,
    failsafe: bool,

    /// Channels of the last accepted valid frame (source for MANUAL sticks and
    /// the mode/throttle used by transitions). Never cleared by a bad frame.
    latest: Option<[u16; NUM_CHANNELS]>,
    /// Timestamp (ms) of the last accepted valid frame; drives the deadman.
    last_valid_ms: Option<u64>,

    cmd: CmdQueue<CMD_CAPACITY>,
    last_out: Actuation,
    stats: Stats,
}

impl<S: ActuatorSink, C: Clock> DriveCore<S, C> {
    /// Build a core in DISARM with the sink pre-driven to neutral (fail-safe
    /// from tick zero — the actuator is defined before the first frame).
    pub fn new(sink: S, clock: C, cfg: Config) -> Self {
        let neutral = Actuation {
            steering_us: cfg.neutral_us,
            throttle_us: cfg.neutral_us,
        };
        let mut core = Self {
            cfg,
            clock,
            sink,
            state: State::Disarm,
            failsafe: false,
            latest: None,
            last_valid_ms: None,
            cmd: CmdQueue::new(),
            last_out: neutral,
            stats: Stats::default(),
        };
        core.sink.apply(neutral);
        core
    }

    /// The configuration in force.
    #[inline]
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The current mode (DISARM/MANUAL/AUTO). Orthogonal to the failsafe latch.
    #[inline]
    pub fn state(&self) -> State {
        self.state
    }

    /// `true` when actually driving: armed mode **and** not failsafe-latched.
    #[inline]
    pub fn is_armed(&self) -> bool {
        !self.failsafe && matches!(self.state, State::Manual | State::Auto)
    }

    /// `true` while the failsafe latch is set (a re-arm via DISARM is required).
    #[inline]
    pub fn is_failsafe(&self) -> bool {
        self.failsafe
    }

    /// The last setpoint written to the sink.
    #[inline]
    pub fn last_output(&self) -> Actuation {
        self.last_out
    }

    /// Running tallies.
    #[inline]
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Commands currently queued in the AUTO channel.
    #[inline]
    pub fn commands_queued(&self) -> usize {
        self.cmd.len()
    }

    /// Enqueue an AUTO command (I8). Never blocks; a full queue drops its oldest
    /// entry. The command is only *ever* actuated in AUTO, and only after the
    /// full safety envelope (arm state, deadman, throttle cap).
    pub fn push_command(&mut self, cmd: Command) {
        self.cmd.push(cmd);
    }

    /// Ingest a decoded frame from the i-BUS parser. Only checksum-valid frames
    /// reach here; a frame whose range flag is clear ([`Frame::in_range`] is
    /// `false`) is **refused** — it neither updates the sticks nor refreshes the
    /// deadman (I6). A valid, in-range frame becomes the latest and stamps the
    /// deadman with the injected clock's current time.
    pub fn on_frame(&mut self, frame: &Frame) {
        if !frame.in_range() {
            self.stats.frames_rejected = self.stats.frames_rejected.wrapping_add(1);
            return;
        }
        self.latest = Some(*frame.channels());
        self.last_valid_ms = Some(self.clock.now_ms());
        self.stats.frames_accepted = self.stats.frames_accepted.wrapping_add(1);
    }

    /// Run one control cycle: deadman check, mode transition, output. The
    /// harness calls this at a fixed 20 ms cadence. Reads the injected clock for
    /// `now`; writes the sink exactly once.
    pub fn tick(&mut self) {
        let now = self.clock.now_ms();

        // I2 deadman: while armed, a stale (or absent) valid frame latches
        // failsafe. Checked before the transition so a live disarm can still
        // clear the latch this same tick (I4).
        if self.is_armed() {
            let stale = match self.last_valid_ms {
                Some(t) => now.saturating_sub(t) > self.cfg.deadman_ms,
                None => true,
            };
            if stale {
                self.enter_failsafe();
            }
        }

        // Mode transitions from the latest valid frame (I4 disarm-wins,
        // I5 deliberate-arm). The mode read fails toward DISARM on a
        // misconfigured channel index, and arming additionally requires the
        // frame to be fresh (within the deadman window).
        if let Some(ch) = self.latest {
            let target = self.mode_state(&ch);
            let throttle_us = self.channel(&ch, self.cfg.throttle_ch);
            self.apply_mode(target, throttle_us, now);
        }

        // Output. I1: exact neutral unless truly armed. I3: throttle cap applied
        // last, only on the armed path.
        let out = if !self.is_armed() {
            self.neutral()
        } else {
            let raw = match self.state {
                State::Manual => match self.latest {
                    Some(ch) => Actuation {
                        steering_us: self.channel(&ch, self.cfg.steer_ch),
                        throttle_us: self.channel(&ch, self.cfg.throttle_ch),
                    },
                    None => self.neutral(),
                },
                State::Auto => match self.cmd.pop() {
                    Some(c) => Actuation {
                        steering_us: c.steering_us,
                        throttle_us: c.throttle_us,
                    },
                    // AUTO holds neutral when the command channel is empty.
                    None => self.neutral(),
                },
                State::Disarm => self.neutral(),
            };
            self.saturate(raw)
        };

        self.sink.apply(out);
        self.last_out = out;
    }

    /// Force the actuator to neutral and park the machine safely. Called by
    /// `Drop` and intended to be the kernel panic handler's hook (I7). Latches
    /// failsafe and disarms so nothing silently resumes.
    pub fn force_neutral(&mut self) {
        self.state = State::Disarm;
        self.failsafe = true;
        self.cmd.clear();
        let n = self.neutral();
        self.sink.apply(n);
        self.last_out = n;
    }

    // --- internals ---------------------------------------------------------

    /// Safe channel read for the *stick* channels: an out-of-range index
    /// (misconfiguration) reads as neutral rather than panicking — a panic in
    /// the control loop is itself a hazard. The MODE channel deliberately does
    /// NOT use this (neutral classifies as MANUAL, an armed-capable state);
    /// it goes through [`mode_state`](Self::mode_state), which fails to DISARM.
    #[inline]
    fn channel(&self, ch: &[u16; NUM_CHANNELS], idx: usize) -> u16 {
        ch.get(idx).copied().unwrap_or(self.cfg.neutral_us)
    }

    /// Fail-toward-safe mode read: an out-of-range mode-channel index (a
    /// misconfigured [`Config::mode_ch`]) classifies as DISARM, never as an
    /// armed-capable state.
    #[inline]
    fn mode_state(&self, ch: &[u16; NUM_CHANNELS]) -> State {
        match ch.get(self.cfg.mode_ch) {
            Some(&mode_us) => self.classify(mode_us),
            None => State::Disarm,
        }
    }

    #[inline]
    fn neutral(&self) -> Actuation {
        Actuation {
            steering_us: self.cfg.neutral_us,
            throttle_us: self.cfg.neutral_us,
        }
    }

    fn enter_failsafe(&mut self) {
        if !self.failsafe {
            self.stats.failsafes = self.stats.failsafes.wrapping_add(1);
        }
        self.failsafe = true;
        // Stale autonomy intent must not survive a signal loss.
        self.cmd.clear();
    }

    fn classify(&self, mode_us: u16) -> State {
        if mode_us < self.cfg.mode_lo_us {
            State::Disarm
        } else if mode_us > self.cfg.mode_hi_us {
            State::Auto
        } else {
            State::Manual
        }
    }

    fn throttle_at_neutral(&self, throttle_us: u16) -> bool {
        throttle_us.abs_diff(self.cfg.neutral_us) <= self.cfg.arm_neutral_tol_us
    }

    /// `true` when the last accepted valid frame is fresh, i.e. within the
    /// deadman window of `now`. Arming off a stale frame is refused (the deadman
    /// itself only runs while armed, so this is the defense-in-depth guard for
    /// the DISARM→armed edge).
    fn frame_fresh(&self, now: u64) -> bool {
        match self.last_valid_ms {
            Some(t) => now.saturating_sub(t) <= self.cfg.deadman_ms,
            None => false,
        }
    }

    fn apply_mode(&mut self, target: State, throttle_us: u16, now: u64) {
        match target {
            State::Disarm => {
                // I4: DISARM wins immediately from any state and clears the latch.
                self.state = State::Disarm;
                self.failsafe = false;
                self.cmd.clear();
            }
            target @ (State::Manual | State::Auto) => {
                if self.failsafe {
                    // Latched: a re-arm requires an explicit DISARM first (I2).
                    // Ignore arm requests until the operator disarms.
                } else if self.state == State::Disarm {
                    // I5: deliberate arm — throttle at neutral in THIS frame,
                    // and the frame itself must be fresh (within the deadman
                    // window). A stale frame must not arm even at neutral.
                    if self.frame_fresh(now) && self.throttle_at_neutral(throttle_us) {
                        self.state = target;
                    }
                    // else: refuse; stay DISARM (boot-with-throttle-high case,
                    // or a stale/frozen receiver feed).
                } else {
                    // Already armed: MANUAL<->AUTO switching is unrestricted.
                    self.state = target;
                }
            }
        }
    }

    /// I3 throttle cap + steering clamp, saturating, at the output boundary.
    fn saturate(&self, a: Actuation) -> Actuation {
        Actuation {
            steering_us: a
                .steering_us
                .clamp(self.cfg.steer_min_us, self.cfg.steer_max_us),
            throttle_us: a
                .throttle_us
                .clamp(self.cfg.throttle_floor_us, self.cfg.throttle_cap_us),
        }
    }
}

impl<S: ActuatorSink, C: Clock> Drop for DriveCore<S, C> {
    /// I7: leaving scope (including an unwinding panic on the host) forces the
    /// actuator neutral. The sink is a field, so it is still live here.
    fn drop(&mut self) {
        self.force_neutral();
    }
}
