// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, "CAD viewing, editing, and CAM/Slicing"), seam shared-core — the Make half of Design -> Make.
//!
//! `make_core` (VUGMAKE, LEDGER SR467) is the PLANNER. It owns the machine as a declarative model ([`Machine`]: the
//! axes as MDES-style joints with travel, feed and acceleration limits, the one power axis, the envelope, the safe Z,
//! the tool library with ISO 13399 property symbols, stock and fixtures — cited, not copied), the manufacturing intent
//! attached to the design ([`Op`], BP-vug-3), the machine-neutral [`Program`] IR, its kinematic pass
//! ([`plan::durations`]), the dialect text ([`gcode`]) and the [`Review`] Helm's gate reads (CODEX §5).
//!
//! # The executor boundary
//!
//! make_core plans; it never executes. A [`Program`] is the move list a dumb, clock-synchronised executor (an MCU, an
//! FPGA, a drive: Klipper's host/MCU split) will consume as a timestamped queue — each move carries the planner's
//! duration from [`plan::durations`] (trapezoid, no jerk; owed VUG-MAKE-QUEUE turns it into the queue). Today the
//! executor is a controller that interprets one of the [`gcode::Dialect`]s, pumped by Comscan (VUG-COMSCAN), with
//! grbl's out-of-band [`gcode::Realtime`] bytes beside the text. Nothing here has a clock, I/O or a thread.
//!
//! # No stop authority
//!
//! A [`Review`] is advice to Helm's gate: refusals and asks, by name. It carries NO stop authority. E-stop, safe torque
//! off (STO) and the limit switches are hardwired — a safety relay or PLC to ISO 13849 — never in this crate, never in
//! the app, never in Helm's software alone.
//!
//! # Purity
//!
//! Every function is a value in, a value or a named [`Refusal`] out; every comparison takes the caller's
//! [`Tol`] (BP-vug-2). Geometry is f64 (BP-vug-1).

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod gcode;
pub mod geom;
pub mod machine;
pub mod op;
pub mod plan;
pub mod program;
pub mod review;
pub mod slice; // VUGMAKESLICE (SR483)
pub mod validate;

pub use geom::{Aabb3, Tol, Vec2, Vec3};
pub use machine::{
    Axis, AxisId, Fixture, Kinematics, Machine, MachineKind, Power, PowerKind, Stock, StockShape, Tool, ToolKind,
    Units,
};
pub use op::{Feeds, Op, Side, generate};
pub use program::{Coolant, Heater, Move, Program, Spin};
pub use review::{Ask, Review, review};
pub use validate::{Refusal, Stats};
pub use slice::{Island, Layer, SliceParams}; // VUGMAKESLICE (SR483)
