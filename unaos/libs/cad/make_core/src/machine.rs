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
