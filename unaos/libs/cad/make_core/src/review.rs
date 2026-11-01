// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The review Helm's gate reads (CODEX §5: Principia's levels pass / ask / refuse; VUG-HELM owed).
//!
//! A `Review` states facts — what is refused by name, what a person must confirm — and nothing else: make_core holds
//! no level type (Principia states the levels, Helm applies them) and NO STOP AUTHORITY. E-stop, safe torque off and
//! the limit switches are hardwired (a safety relay or PLC to ISO 13849), outside this crate and the app.

use alloc::vec::Vec;

use crate::geom::Tol;
use crate::machine::{Machine, PowerKind};
use crate::program::{Heater, Move, Program, Spin};
use crate::validate::{Refusal, Stats, check};

/// What a person must confirm before the program runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// Always asked: make_core keeps no history; Helm drops it for a digest it has already approved.
    FirstRun { digest: u64 },
    ToolChange { at: usize, tool: u32 },
    /// A power value above the machine's `ask_above` (in the power axis' own unit).
    PowerAbove { at: usize, value: f64, threshold: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    pub stats: Stats,
    pub refusals: Vec<Refusal>,
    pub asks: Vec<Ask>,
}

/// Every refusal and every ask for running `p` on `m`.
pub fn review(p: &Program, m: &Machine, tol: Tol) -> Review {
    let mut refusals = Vec::new();
    let stats = check(p, m, tol, &mut refusals);
    let mut asks = alloc::vec![Ask::FirstRun { digest: p.digest() }];
    let th = m.power.ask_above;
    for (i, mv) in p.moves.iter().enumerate() {
        let value = match mv {
            Move::ToolChange { tool } => {
                asks.push(Ask::ToolChange { at: i, tool: *tool });
                None
            }
            Move::Spindle(Spin::Cw(r) | Spin::Ccw(r)) if m.power.kind == PowerKind::Spindle => Some(*r),
            Move::Power { pct } if matches!(m.power.kind, PowerKind::Laser | PowerKind::Torch) => Some(*pct),
            Move::Heat { heater: Heater::Hotend, celsius, .. } if m.power.kind == PowerKind::Extruder => Some(*celsius),
            _ => None,
        };
        if let Some(v) = value {
            if v > th + tol.lin {
                asks.push(Ask::PowerAbove { at: i, value: v, threshold: th });
            }
        }
    }
    Review { stats, refusals, asks }
}
