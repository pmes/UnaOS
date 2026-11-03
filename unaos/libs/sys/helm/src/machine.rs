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

//! CHARTER: Helm (CODEX §2 Amendment I), seam shared-core — the machine gate a make_core program passes before any
//! machine moves (CODEX §5: "the stream passes its gate (Principia's levels: pass / ask / refuse)").
//!
//! The second per-machine domain of the [`helm`](crate) core (VUGHELM, LEDGER SR475). Where [`crate::rover`] holds a
//! live control loop, this module holds the gate in front of a planned program: it reads make_core's [`Review`] (the
//! facts: refusals by name, asks by name) against a [`Policy`] of [`Level`]s and the caller's [`Approved`] record, and
//! returns one [`Verdict`]. Comscan (VUG-COMSCAN) pumps a stream on a [`Verdict::Pass`] for the program it names and
//! nothing else.
//!
//! # The levels
//!
//! Principia's spectrum (`handlers/principia/README.md`, "Safety levels — the law an AI runs under"): **never** (the
//! AI may not act), **ask** (a person confirms before each action), **autonomous within bounds** ([`Level::Pass`]).
//! The levels are keyed by make_core's ask kinds ([`AskKind`]) and by nothing else. A refusal is a fact about the
//! machine, not a preference: there is no Level a refusal can be given, so no policy can override one.
//! Principia publishing the levels is owed (VUG-HELM-PRINCIPIA); until then the caller holds the [`Policy`] value.
//!
//! # The first-run memory
//!
//! make_core asks [`Ask::FirstRun`] for every program because it keeps no history. A person's approval of a program
//! digest ([`make_core::Program::digest`]) is a durable, preference-like fact, so it belongs in Principia's store,
//! never in a dotfile: this module keeps it as a value the caller persists ([`Approved`] in and out), and [`approve`]
//! is the one way a digest becomes remembered. An approval covers [`Ask::FirstRun`] only; a tool change or a power
//! value at [`Level::Ask`] is asked on every run.
//!
//! # Purity
//!
//! No clock (the caller passes `when_us`), no I/O, no lock, no global: values in, a [`Verdict`] out. There is no
//! `sim` harness because there is nothing to fake. The gate carries no stop authority: E-stop, safe torque off and
//! the limit switches are hardwired (make_core's crate doc).
//!
//! # The gate invariants (each proved by a named test in `tests/machine_invariants.rs`)
//!
//! - **G1 refusal-wins** — a review with any refusal is [`Verdict::Refuse`] under every policy, approved or not.
//! - **G2 first-run remembered** — an approved digest drops its `FirstRun`; a changed program asks again.
//! - **G3 never-by-name** — an ask whose level is [`Level::Never`] refuses, carrying that ask by name.
//! - **G4 pass-level drops** — an ask whose level is [`Level::Pass`] is not asked.
//! - **G5 default asks all** — the default policy asks every ask, in review order.
//! - **G6 approval is per digest only** — an approval never drops a `ToolChange` or a `PowerAbove`.
//! - **G7 approve is idempotent** — a digest approved twice is one record, the first person kept.
//! - **G8 a pass names its program** — a Pass carries the digest of the program reviewed (the review's own
//!   `FirstRun`); a review naming no program is [`Verdict::Unbound`], never Pass (VUG-HELM-BIND, LEDGER SR489).

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub use make_core::{Ask, Refusal, Review};

/// A level on Principia's spectrum, for one ask kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The AI may not act: the ask refuses, by name.
    Never,
    /// A person confirms before the program runs.
    Ask,
    /// Autonomous within bounds: the ask is not raised.
    Pass,
}

/// The kinds of [`Ask`] make_core names. The match in [`AskKind::of`] is exhaustive: a new make_core ask fails this
/// crate's build rather than passing silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskKind {
    FirstRun,
    ToolChange,
    PowerAbove,
}

impl AskKind {
    /// The kind of an ask.
    pub fn of(a: &Ask) -> AskKind {
        match a {
            Ask::FirstRun { .. } => AskKind::FirstRun,
            Ask::ToolChange { .. } => AskKind::ToolChange,
            Ask::PowerAbove { .. } => AskKind::PowerAbove,
        }
    }
}

/// The level per ask kind. Refusal classes are deliberately not keys: a refusal is never overridable by policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub first_run: Level,
    pub tool_change: Level,
    pub power_above: Level,
}

impl Default for Policy {
    /// Every ask is asked.
    fn default() -> Self {
        Policy { first_run: Level::Ask, tool_change: Level::Ask, power_above: Level::Ask }
    }
}

impl Policy {
    /// The level in force for `a`.
    pub fn level(&self, a: &Ask) -> Level {
        match AskKind::of(a) {
            AskKind::FirstRun => self.first_run,
            AskKind::ToolChange => self.tool_change,
            AskKind::PowerAbove => self.power_above,
        }
    }
}

/// One remembered first run: who approved which program digest, and when (caller's microseconds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub digest: u64,
    pub who: String,
    pub when_us: u64,
}

/// The first-run memory, owned and persisted by the caller (Principia's store, VUG-HELM-PRINCIPIA).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Approved {
    pub digests: Vec<Approval>,
}

impl Approved {
    /// `true` when a person has approved the program with this digest.
    pub fn covers(&self, digest: u64) -> bool {
        self.digests.iter().any(|a| a.digest == digest)
    }
}

// VUG-HELM-BIND (SR489): the pass names what it passed. The digest is the reviewed program's
// (`make_core::Program::digest`), read from the review's own `Ask::FirstRun`: make_core's `review` is the one fn that
// sees the Program and always asks its first run, so the gate binds without the caller pairing anything. Comscan
// pumps a `Pass` only for the program whose digest it carries. The machine is not in the pass yet: make_core's
// `Machine` has no identity (VUG-HELM-BIND-MACHINE owed).
/// The gate's answer. Comscan pumps on `Pass` for the program it names and nothing else.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Nothing refuses and nothing is left to ask, for the program with this digest.
    Pass { digest: u64 },
    /// A person must confirm these, in review order.
    Ask(Vec<Ask>),
    /// The machine's refusals (never overridable) and the asks the policy holds at [`Level::Never`], both by name.
    Refuse { refusals: Vec<Refusal>, never: Vec<Ask> },
    /// Nothing refuses or asks, but the review names no program (it carries no [`Ask::FirstRun`], which make_core's
    /// `review` always asks): there is nothing a pass could name, so nothing passes.
    Unbound,
}

impl Verdict {
    /// `true` only for [`Verdict::Pass`].
    pub fn is_pass(&self) -> bool {
        matches!(self, Verdict::Pass { .. })
    }
}

/// The digest of the program a review was made for: its first `FirstRun` ask (VUG-HELM-BIND).
fn reviewed_digest(review: &Review) -> Option<u64> {
    review.asks.iter().find_map(|a| match a {
        Ask::FirstRun { digest } => Some(*digest),
        _ => None,
    })
}

/// The gate: any refusal or any ask at [`Level::Never`] refuses; else the asks at [`Level::Ask`] that `approved` does
/// not cover are asked; else the program passes, naming its digest (or is [`Verdict::Unbound`] when the review names
/// no program).
pub fn gate(review: &Review, policy: &Policy, approved: &Approved) -> Verdict {
    let never: Vec<Ask> = review.asks.iter().filter(|a| policy.level(a) == Level::Never).cloned().collect();
    if !review.refusals.is_empty() || !never.is_empty() {
        return Verdict::Refuse { refusals: review.refusals.clone(), never };
    }
    let asks: Vec<Ask> = review
        .asks
        .iter()
        .filter(|a| policy.level(a) == Level::Ask)
        .filter(|a| !matches!(a, Ask::FirstRun { digest } if approved.covers(*digest)))
        .cloned()
        .collect();
    if !asks.is_empty() {
        return Verdict::Ask(asks);
    }
    match reviewed_digest(review) {
        Some(digest) => Verdict::Pass { digest },
        None => Verdict::Unbound,
    }
}

/// Remember that `who` approved the program with `digest` at `when_us`: the one way a first run becomes remembered.
/// Idempotent: an already-covered digest keeps its first record and returns `false`.
pub fn approve(approved: &mut Approved, digest: u64, who: &str, when_us: u64) -> bool {
    if approved.covers(digest) {
        return false;
    }
    approved.digests.push(Approval { digest, who: String::from(who), when_us });
    true
}
