// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Comscan (CODEX §2, "IO Bridge ... Controls hardware"; §5.3 "takes the approved stream and pumps it directly
//! to the hardware"), seam shared-core — the pump the host handler and the kernel's Comscan face both link.
//!
//! `pump_core` (VUGCOMSCAN1, LEDGER SR476) streams the text make_core's [`make_core::gcode::write`] produced to a
//! controller and reads what it says back. It owns the streaming protocol and nothing else: the dialect enum, the
//! realtime byte table and the writer are make_core's; whether a program may run is Helm's (the handler's only entry
//! is Helm's `Verdict::Pass`, BP-vug-3); the wire is the caller's [`Transport`] (a serial port, a USB CDC-ACM pipe,
//! a mock).
//!
//! # The protocols (cited)
//!
//! grbl 1.1 (gnea/grbl `doc/markdown/interface.md`): "Grbl has a 128 character serial receive buffer"; the
//! character-counting protocol "always ensures Grbl's serial read buffer is filled, while never overflowing it"; every
//! line is answered `ok` or `error:X`; "All real-time command characters can be sent at any time and are never placed
//! in Grbl's RX serial buffer". The window here is [`pump::GRBL_RX`] = 127 (the doc's "stores up to 127 characters").
//! Marlin (RepRap wiki, G-code "Checking"): send-response, `ok` per line, `Resend: N` after a line fault.
//!
//! # No stop authority
//!
//! A feed hold or a soft reset sent from here is a request to a controller. E-stop, safe torque off and the limit
//! switches are hardwired (ISO 13849), never in this crate.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod grbl;
#[cfg(feature = "std")]
pub mod mock;
pub mod pump;

pub use grbl::{State, StatusReport, StatusRefusal, alarm_name, error_name, parse_status};
pub use pump::{Event, Halt, Progress, Pump, PumpRefusal, PumpState};

/// The link went away (the port closed, the cable came out).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    Closed,
}

/// The wire to a controller, supplied by the caller. Neither call blocks.
pub trait Transport {
    /// Write as much of `bytes` as the link takes now; the count taken.
    fn write(&mut self, bytes: &[u8]) -> Result<usize, LinkError>;
    /// Read what has arrived into `buf`; the count read (0 = nothing yet).
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, LinkError>;
}
