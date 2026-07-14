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

//! The kernel **helm** core — the hard control interlock beneath the helm
//! handler.
//!
//! Helm is the layer that does not negotiate: DISARM / MANUAL / AUTO with a
//! FAILSAFE latch, with the transmitter as the human estop. Where the
//! `principia` policy engine *states* the safety levels and the `helm` handler
//! *holds* control authority, this crate is the hard interlock beneath both —
//! embedded in Ring 0 so a kernel fault still parks the machine safely.
//!
//! Failsafes do not generalize — a rover's safe state is "stop"; a mill's is
//! "retract and stop the spindle" — so control lives in **per-machine domain
//! modules**, one machine class each. [`rover`] is the first: TALUS's
//! receiver→actuator safety state machine.
//!
//! # `no_std`
//!
//! The crate is `#![no_std]`-capable and `#![forbid(unsafe_code)]`. The
//! default-on `std` feature adds the host-only simulation harness used by the
//! invariant tests; the kernel embeds this crate with `default-features = false`.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

pub mod rover;
