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

//! # junct — "The Receiver"
//!
//! Design-stage handler. junct is the communications aggregation handler: it
//! abstracts human conversation networks (Matrix, Email, IRC, RSS) into a
//! single **Stream**, so the rest of the system sees one inbound conversation
//! surface instead of a drawer full of fragmented apps.
//!
//! junct is symmetric with `vein`: same shape, same shared chat view, a
//! different party on the far end (junct = human <-> human, vein = human <-> AI).
//! Like every handler it is **headless** — it has no chat UI of its own; that
//! is the shared chat view's job. See
//! `docs/dev/USERLAND/RECONCILIATION-2026-07.md` and the CODEX charter entry
//! ("The Receiver").
//!
//! **Not yet implemented.** This crate contains no working code. The planned
//! interface is the `ignite(...)` async entry-point convention used by the
//! other handlers, producing `bandy::SMessage` onto the Synapse bus; it is
//! described here as intent, not a shipped contract.
