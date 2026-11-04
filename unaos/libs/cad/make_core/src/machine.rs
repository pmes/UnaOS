// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The machine as a declarative model the app owns (research principle 4): the shape follows MDES (ModuleWorks'
//! Manufacturing Data Exchange Specification: a kinematic chain of joints with travel and limits, the controller's
//! rates) and the tool follows ISO 13399's property symbols — cited, not copied; their import is VUG-MAKE-MDES.

use alloc::{string::String, vec::Vec};

use crate::geom::{Aabb3, Tol, Vec3};
use crate::validate::Refusal;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Units {
    Mm,
    Inch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineKind {
    Mill3,
    Lathe2,
    Fdm3,
    Laser2,
    Plasma2,
}

/// The kinematic chain. Cartesian only today (CoreXY, delta and five-axis chains are MDES's next shapes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kinematics {
    Cartesian,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisId {
    X,
    Y,
    Z,
    A,
    B,
    C,
}

/// One joint (MDES): its travel and its controller limits, in the machine's units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub id: AxisId,
    pub min: f64,
    pub max: f64,
    /// Highest programmable feed, units/min.
    pub max_feed: f64,
    /// Acceleration limit, units/s².
    pub max_accel: f64,
    /// Rapid (G0) rate, units/min.
    pub rapid: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerKind {
    Spindle,
    Extruder,
    Laser,
    Torch,
}

/// The one power axis: a spindle (rpm), an extruder's hotend (°C), a laser or a torch (percent).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Power {
    pub kind: PowerKind,
    /// The ceiling in the axis' own unit.
    pub max: f64,
    /// Above this value (same unit) the review asks a person to confirm.
    pub ask_above: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    FlatEnd,
    BallEnd,
    Drill,
    Nozzle,
    Beam,
    Torch,
}

/// A tool, its fields named by ISO 13399's property symbols.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tool {
    /// The T number.
    pub number: u32,
    pub kind: ToolKind,
    /// DC — cutting diameter (a nozzle's bore, a beam's kerf).
    pub dc: f64,
    /// LF — functional length (tip to the holder's gauge line).
    pub lf: f64,
    /// APMX — maximum depth of cut per pass.
    pub apmx: f64,
    /// NOF — number of flutes.
    pub nof: u8,
    /// SIG — point angle, radians (drills).
    pub sig: f64,
    /// VUGMAKERAMP (SR532): the maker's maximum ramp angle, degrees, in (0, 90): a pocket entry ramps along its first
    /// ring or helixes no steeper ([`crate::op`]); None keeps the centre-cutting rule only (a ball end or a drill
    /// plunges, a flat end with no room for the helix refuses `NoEntry`).
    pub ramp_deg: Option<f64>,
    /// VUGMAKETOOLCC (SR538): whether the tool cuts at its centre and may plunge straight into solid, as its maker says:
    /// Some(true) yes, Some(false) no, None by kind (a ball end and a drill yes, every other kind no). Read only through
    /// [`crate::op`]'s `centre_cutting`.
    pub centre_cutting: Option<bool>,
}

impl Tool {
    pub fn radius(&self) -> f64 {
        self.dc * 0.5
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StockShape {
    /// A block; `origin` is its minimum corner.
    Box { size: Vec3 },
    /// An upright cylinder; `origin` is the centre of its base.
    Cylinder { radius: f64, height: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stock {
    pub shape: StockShape,
    pub origin: Vec3,
}

impl Stock {
    pub fn aabb(&self) -> Aabb3 {
        match self.shape {
            StockShape::Box { size } => Aabb3 { min: self.origin, max: self.origin + size },
            StockShape::Cylinder { radius, height } => Aabb3 {
                min: self.origin - Vec3::new(radius, radius, 0.0),
                max: self.origin + Vec3::new(radius, radius, height),
            },
        }
    }
}

/// A fixture as the boxes no tool may enter.
#[derive(Clone, Debug, PartialEq)]
pub struct Fixture {
    pub name: String,
    pub keepout: Vec<Aabb3>,
}

/// The machine, its tool library and the setup on its table.
#[derive(Clone, Debug, PartialEq)]
pub struct Machine {
    pub kind: MachineKind,
    pub kinematics: Kinematics,
    pub axes: Vec<Axis>,
    pub power: Power,
    /// The work envelope in program coordinates (one setup, G54 as identity).
    pub envelope: Aabb3,
    /// The rapid floor: below this Z no rapid may cross the stock.
    pub safe_z: f64,
    pub units: Units,
    pub tools: Vec<Tool>,
    pub stock: Option<Stock>,
    pub fixtures: Vec<Fixture>,
    /// The tool in the spindle when the program starts.
    pub loaded: Option<u32>,
}

impl Machine {
    pub fn axis(&self, id: AxisId) -> Option<&Axis> {
        self.axes.iter().find(|a| a.id == id)
    }

    pub fn tool(&self, number: u32) -> Option<&Tool> {
        self.tools.iter().find(|t| t.number == number)
    }

    /// The model's own consistency: limits positive, the envelope inside the linear axes' travel, the safe Z inside
    /// the envelope, tool numbers unique.
    pub fn check(&self, tol: Tol) -> Result<(), Refusal> {
        let e = &self.envelope;
        if e.min.x > e.max.x || e.min.y > e.max.y || e.min.z > e.max.z {
            return Err(Refusal::MachineInvalid { what: "envelope min above max" });
        }
        for a in &self.axes {
            if a.min > a.max {
                return Err(Refusal::MachineInvalid { what: "axis min above max" });
            }
            let ok = |v: f64| v.is_finite() && v > 0.0;
            if !ok(a.max_feed) || !ok(a.max_accel) || !ok(a.rapid) {
                return Err(Refusal::MachineInvalid { what: "axis limit not positive" });
            }
            let span = match a.id {
                AxisId::X => Some((e.min.x, e.max.x)),
                AxisId::Y => Some((e.min.y, e.max.y)),
                AxisId::Z => Some((e.min.z, e.max.z)),
                _ => None,
            };
            if let Some((lo, hi)) = span {
                if lo < a.min - tol.lin || hi > a.max + tol.lin {
                    return Err(Refusal::MachineInvalid { what: "envelope outside axis travel" });
                }
            }
        }
        if self.safe_z < e.min.z - tol.lin || self.safe_z > e.max.z + tol.lin {
            return Err(Refusal::MachineInvalid { what: "safe_z outside envelope" });
        }
        for (i, t) in self.tools.iter().enumerate() {
            if self.tools[..i].iter().any(|o| o.number == t.number) {
                return Err(Refusal::MachineInvalid { what: "tool number repeated" });
            }
        }
        Ok(())
    }
}

// --- VUGHELMBINDMACHINE (LEDGER SR498): the machine's identity (named tail hunk) ---

impl Machine {
    /// FNV-1a 64 (`Program::digest`'s hasher and shape) over the machine's IDENTITY: kind, kinematics, every axis,
    /// the power axis' kind and ceiling, the envelope, the units and every tool, in order. The setup and the state are
    /// not identity and are left out: stock, fixtures, the loaded tool, safe Z and the power ask threshold may change
    /// between a review and a run without the machine changing. Helm's pass names this digest and Comscan's desk
    /// refuses a pass naming another machine (VUG-HELM-BIND-MACHINE).
    pub fn digest(&self) -> u64 {
        let mut h = crate::program::Fnv::new();
        h.byte(1);
        h.byte(self.kind as u8);
        h.byte(2);
        h.byte(self.kinematics as u8);
        h.byte(3);
        h.bytes(&(self.axes.len() as u64).to_le_bytes());
        for a in &self.axes {
            h.byte(a.id as u8);
            h.f(a.min);
            h.f(a.max);
            h.f(a.max_feed);
            h.f(a.max_accel);
            h.f(a.rapid);
        }
        h.byte(4);
        h.byte(self.power.kind as u8);
        h.f(self.power.max);
        h.byte(5);
        h.v3(self.envelope.min);
        h.v3(self.envelope.max);
        h.byte(6);
        h.byte(self.units as u8);
        h.byte(7);
        h.bytes(&(self.tools.len() as u64).to_le_bytes());
        for t in &self.tools {
            h.bytes(&t.number.to_le_bytes());
            h.byte(t.kind as u8);
            h.f(t.dc);
            h.f(t.lf);
            h.f(t.apmx);
            h.byte(t.nof);
            h.f(t.sig);
        }
        h.0
    }
}
