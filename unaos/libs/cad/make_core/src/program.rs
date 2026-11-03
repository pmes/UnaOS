// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The `Program` IR: the machine-neutral move list (BP-vug-3, the sheet's Vug -> Helm -> Comscan seam). Helm reads
//! this, never the text; each dialect writer in [`crate::gcode`] says what its controller can say of it.

use alloc::{string::String, vec::Vec};

use crate::geom::{Vec2, Vec3};
use crate::machine::Units;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spin {
    Cw(f64),
    Ccw(f64),
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coolant {
    Mist,
    Flood,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heater {
    Hotend,
    Bed,
}

/// One planned step. Positions are absolute in the program's units; feeds are units/min.
#[derive(Clone, Debug, PartialEq)]
pub enum Move {
    Rapid { to: Vec3 },
    Linear { to: Vec3, feed: f64 },
    /// An arc in the XY plane (G17) about `centre`, helical when `to.z` differs from the start's Z.
    Arc { to: Vec3, centre: Vec2, cw: bool, feed: f64 },
    ToolChange { tool: u32 },
    Spindle(Spin),
    /// The laser / torch level, percent of the power axis.
    Power { pct: f64 },
    Dwell { s: f64 },
    Coolant(Coolant),
    Comment(String),
    /// Marlin's G28 (the machine homes; the position is unknown to the program until the next full move).
    Home,
    /// Marlin's M104 / M109 (hotend) and M140 / M190 (bed); `wait` is the 9x form.
    Heat { heater: Heater, celsius: f64, wait: bool },
    /// A move with the extruder's E (absolute, M82).
    Extrude { to: Vec3, e: f64, feed: f64 },
    /// Marlin's G92 E.
    SetE { e: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub units: Units,
    pub moves: Vec<Move>,
}

impl Program {
    pub fn new(units: Units) -> Self {
        Program { units, moves: Vec::new() }
    }

    /// FNV-1a (64-bit) over a canonical byte encoding of the moves (f64 by its bits): the key Helm remembers a
    /// first run by (VUG-HELM). Two programs with equal moves have equal digests on every host.
    pub fn digest(&self) -> u64 {
        let mut h = Fnv::new();
        h.byte(match self.units {
            Units::Mm => 0,
            Units::Inch => 1,
        });
        for m in &self.moves {
            match m {
                Move::Rapid { to } => {
                    h.byte(1);
                    h.v3(*to);
                }
                Move::Linear { to, feed } => {
                    h.byte(2);
                    h.v3(*to);
                    h.f(*feed);
                }
                Move::Arc { to, centre, cw, feed } => {
                    h.byte(3);
                    h.v3(*to);
                    h.f(centre.x);
                    h.f(centre.y);
                    h.byte(*cw as u8);
                    h.f(*feed);
                }
                Move::ToolChange { tool } => {
                    h.byte(4);
                    h.bytes(&tool.to_le_bytes());
                }
                Move::Spindle(s) => {
                    h.byte(5);
                    match s {
                        Spin::Cw(r) => {
                            h.byte(0);
                            h.f(*r);
                        }
                        Spin::Ccw(r) => {
                            h.byte(1);
                            h.f(*r);
                        }
                        Spin::Off => h.byte(2),
                    }
                }
                Move::Power { pct } => {
                    h.byte(6);
                    h.f(*pct);
                }
                Move::Dwell { s } => {
                    h.byte(7);
                    h.f(*s);
                }
                Move::Coolant(c) => {
                    h.byte(8);
                    h.byte(*c as u8);
                }
                Move::Comment(t) => {
                    h.byte(9);
                    h.bytes(&(t.len() as u64).to_le_bytes());
                    h.bytes(t.as_bytes());
                }
                Move::Home => h.byte(10),
                Move::Heat { heater, celsius, wait } => {
                    h.byte(11);
                    h.byte(*heater as u8);
                    h.f(*celsius);
                    h.byte(*wait as u8);
                }
                Move::Extrude { to, e, feed } => {
                    h.byte(12);
                    h.v3(*to);
                    h.f(*e);
                    h.f(*feed);
                }
                Move::SetE { e } => {
                    h.byte(13);
                    h.f(*e);
                }
            }
        }
        h.0
    }
}

/// FNV-1a 64 over bytes: make_core's one hasher (`Program::digest`, `Machine::digest`; VUGHELMBINDMACHINE SR498 made
/// it crate-visible so the machine identity shares it).
pub(crate) struct Fnv(pub(crate) u64);

impl Fnv {
    /// The FNV-1a 64 offset basis.
    pub(crate) fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    pub(crate) fn byte(&mut self, b: u8) {
        self.0 ^= b as u64;
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
    }
    pub(crate) fn bytes(&mut self, bs: &[u8]) {
        for b in bs {
            self.byte(*b);
        }
    }
    pub(crate) fn f(&mut self, v: f64) {
        self.bytes(&v.to_bits().to_le_bytes());
    }
    pub(crate) fn v3(&mut self, v: Vec3) {
        self.f(v.x);
        self.f(v.y);
        self.f(v.z);
    }
}
