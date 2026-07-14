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

//! # helm — "The Wheel"
//!
//! Design-stage handler. helm holds **control authority** over every physical
//! action an AI initiates: nothing an AI drives reaches the hardware without
//! passing through helm's gate. For each action it reads `principia`'s
//! user-chosen safety levels and decides **pass / ask / refuse**. The wheel and
//! the captain's voice at one station — direct human control and commanded
//! intent, one authority deciding which is in effect.
//!
//! helm is the **authority** layer of the safety stack (law → authority →
//! interlock): principia *states* the law, helm *holds* the authority, and the
//! kernel helm core at `unaos/libs/sys/helm` is the hard **interlock** beneath
//! that does not negotiate (DISARM / MANUAL / AUTO + FAILSAFE latch). This
//! Ring 3 handler is distinct from that Ring 0 core. See
//! `docs/dev/USERLAND/RECONCILIATION-2026-07.md` and the CODEX charter entry
//! (Amendment I, "The Wheel").
//!
//! **Not yet implemented.** This crate contains no working code. The planned
//! interface is the `ignite(...)` async entry-point convention used by the
//! other handlers, producing `bandy::SMessage` pass/ask/refuse decisions onto
//! the Synapse bus; it is described here as intent, not a shipped contract.
