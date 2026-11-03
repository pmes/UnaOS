// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A program checked against the machine: every refusal by name with the move index, and the planner's stats.

use alloc::{string::String, vec::Vec};
use libm::fabs;

use crate::gcode::Dialect;
use crate::geom::{Aabb3, Tol, Vec3, seg_aabb3};
use crate::machine::{Machine, Units};
use crate::plan::{arc_bbox, arc_caps, caps, durations};
use crate::program::{Move, Program};

/// Every way make_core says no. `at` is the move index; `line` is a 1-based text line.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    OutsideEnvelope { at: usize, x: f64, y: f64, z: f64 },
    FeedAboveMax { at: usize, feed: f64, max: f64 },
    /// A feed that is zero, negative or not finite.
    BadFeed { at: usize, feed: f64 },
    /// A rapid below the safe Z over the stock (a straight +Z retract is allowed).
    RapidIntoStock { at: usize },
    FixtureCollision { at: usize, fixture: String },
    UnknownTool { at: usize, tool: u32 },
    UnitsMismatch { program: Units, machine: Units },
    MachineInvalid { what: &'static str },
    /// An Op whose generator is owed, with the owed id.
    OpOwed { op: &'static str, owed: &'static str },
    ContourDegenerate { what: &'static str },
    ContourSelfIntersects,
    ToolTooLarge { tool: u32 },
    /// The cut's bottom is not below its top (depth not positive).
    DepthAboveTop,
    /// A move the dialect cannot say.
    Unsayable { dialect: Dialect, at: usize, what: &'static str },
    /// A word the reader does not take, by name.
    Unsupported { line: usize, word: String },
    Parse { line: usize, what: &'static str },
    // VUGMAKEPOCKET (SR479): the pocket and face generators' refusals.
    /// An island not strictly inside the outline, crossing it, or inside another island (`island` is its index).
    IslandOutsideOutline { island: usize },
    /// A pocket loop crosses itself, or one island crosses another.
    OutlineSelfIntersects,
    /// The offset's pieces did not chain into closed rings (refused, never an open toolpath).
    NoRings,
    /// No way into the cut: no room for the helix and the tool is not centre-cutting (the ramp entry is owed).
    NoEntry { tool: u32 },
    /// An Op parameter out of its range, by name.
    BadParameter { what: &'static str },
}

/// The planner's numbers for a program.
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    /// The sum of `durations` (an upper bound: see [`crate::plan`]).
    pub time_s: f64,
    pub durations: Vec<f64>,
    pub bbox: Option<Aabb3>,
    pub max_feed: f64,
    /// The loaded tool, then each tool changed to, once each in order.
    pub tools: Vec<u32>,
    pub untimed: u32,
}

/// One pass over the program: every refusal pushed to `out`, the stats returned.
pub fn check(p: &Program, m: &Machine, tol: Tol, out: &mut Vec<Refusal>) -> Stats {
    if p.units != m.units {
        out.push(Refusal::UnitsMismatch { program: p.units, machine: m.units });
    }
    if let Err(r) = m.check(tol) {
        out.push(r);
    }
    let (durs, untimed) = durations(p, m, tol);
    let stock = m.stock.map(|s| s.aabb());
    let mut tool = m.loaded.and_then(|n| m.tool(n));
    let mut tools: Vec<u32> = m.loaded.into_iter().collect();
    let mut bbox: Option<Aabb3> = None;
    let mut max_feed = 0.0f64;
    let mut pos: Option<Vec3> = None;

    for (i, mv) in p.moves.iter().enumerate() {
        let (to, sweep_box, feed, rapid) = match mv {
            Move::Rapid { to } => (*to, None, None, true),
            Move::Linear { to, feed } | Move::Extrude { to, feed, .. } => (*to, None, Some(*feed), false),
            Move::Arc { to, centre, cw, feed } => {
                let s = pos.unwrap_or(*to);
                let (lo, hi) = arc_bbox(s.xy(), to.xy(), *centre, *cw, tol);
                let b = Aabb3::new(Vec3::new(lo.x, lo.y, s.z.min(to.z)), Vec3::new(hi.x, hi.y, s.z.max(to.z)));
                (*to, Some(b), Some(*feed), false)
            }
            Move::ToolChange { tool: n } => {
                match m.tool(*n) {
                    Some(t) => tool = Some(t),
                    None => out.push(Refusal::UnknownTool { at: i, tool: *n }),
                }
                if !tools.contains(n) {
                    tools.push(*n);
                }
                continue;
            }
            Move::Home => {
                pos = None;
                continue;
            }
            _ => continue,
        };
        let from = pos.unwrap_or(to);
        let swept = sweep_box.unwrap_or(Aabb3::from_point(from).union(&Aabb3::from_point(to)));

        // The envelope: the end point, then the arc's extremes.
        if !m.envelope.contains(to, tol) {
            out.push(Refusal::OutsideEnvelope { at: i, x: to.x, y: to.y, z: to.z });
        } else if let Some(b) = sweep_box {
            for c in [b.min, b.max] {
                if !m.envelope.contains(c, tol) {
                    out.push(Refusal::OutsideEnvelope { at: i, x: c.x, y: c.y, z: c.z });
                    break;
                }
            }
        }
        bbox = Some(match bbox {
            Some(b) => b.union(&swept),
            None => swept,
        });

        // The feed against the directional limit.
        if let Some(f) = feed {
            if !(f > 0.0 && f.is_finite()) {
                out.push(Refusal::BadFeed { at: i, feed: f });
            } else {
                let cap = if sweep_box.is_some() {
                    arc_caps(m).0
                } else {
                    let d = to - from;
                    let l = d.length();
                    if l > tol.lin { caps(m, d * (1.0 / l), false).0 } else { f64::INFINITY }
                };
                if f > cap + tol.lin {
                    out.push(Refusal::FeedAboveMax { at: i, feed: f, max: cap });
                }
                max_feed = max_feed.max(f);
            }
        }

        // A rapid below the safe Z over the stock.
        if rapid {
            if let Some(st) = stock {
                let low = from.z.min(to.z);
                let retract = fabs(to.x - from.x) <= tol.lin && fabs(to.y - from.y) <= tol.lin && to.z > from.z;
                if low < m.safe_z - tol.lin && !retract {
                    let foot = Aabb3::new(
                        Vec3::new(st.min.x, st.min.y, low - 1.0),
                        Vec3::new(st.max.x, st.max.y, from.z.max(to.z) + 1.0),
                    );
                    if seg_aabb3(from, to, &foot) {
                        out.push(Refusal::RapidIntoStock { at: i });
                    }
                }
            }
        }

        // The tool (radius in XY, functional length upward from the tip) against every keep-out box.
        let (r, lf) = tool.map(|t| (t.radius(), t.lf)).unwrap_or((0.0, 0.0));
        'fx: for fx in &m.fixtures {
            for b in &fx.keepout {
                let grown = Aabb3::new(
                    Vec3::new(b.min.x - r, b.min.y - r, b.min.z - lf),
                    Vec3::new(b.max.x + r, b.max.y + r, b.max.z),
                );
                let hit = match sweep_box {
                    Some(sb) => sb.intersects(&grown),
                    None => seg_aabb3(from, to, &grown),
                };
                if hit {
                    out.push(Refusal::FixtureCollision { at: i, fixture: fx.name.clone() });
                    break 'fx;
                }
            }
        }
        pos = Some(to);
    }

    Stats { time_s: durs.iter().sum(), durations: durs, bbox, max_feed, tools, untimed }
}

impl Program {
    /// The first refusal, or the stats of a program the machine can run.
    pub fn validate(&self, m: &Machine, tol: Tol) -> Result<Stats, Refusal> {
        let mut v = Vec::new();
        let s = check(self, m, tol, &mut v);
        match v.into_iter().next() {
            Some(r) => Err(r),
            None => Ok(s),
        }
    }
}
