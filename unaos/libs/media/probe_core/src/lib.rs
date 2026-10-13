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

//! # probe_core — what a probe hands the machine
//!
//! CHARTER: Sonde — shared-core (R112: the probes' frame formats are one no_std core both rings link).
//!
//! Sonde (R112, CODEX Amendment IV drafted) is the handler of PROBES: capture devices that measure the
//! world. This crate is the part of it that is the same on every ring and every OS — the vocabulary a
//! probe's output is spoken in, with no device, no bus and no screen in it:
//!
//! | module | what it models |
//! |---|---|
//! | [`frame`] | a [`Frame`]: format (Z16 / Y8 / Y16 / YUY2), size, stride, timestamp, the stream it came from |
//! | [`depth`] | the depth scale (Z16 units → metres), the pinhole [`Intrinsics`], deprojection, a point cloud |
//! | [`ramp`] | the faces' pixels: depth as a colour ramp, Y8 as grey, YUY2 as RGB (all RGBA8) |
//! | [`imu`] | an [`ImuSample`] and the D400 HID motion report it is read from |
//! | [`replay`] | deterministic synthetic frames — the fixture a host without a camera is fed |
//! | [`wire`] | one frame as bytes: the 80-byte `PRB1` header + data the kernel publishes at `/dev/probe/depth0` (SONDE2, SR105) |
//!
//! The first probe is the Intel RealSense D435i (REALSENSE, LEDGER SR92). Every hardware fact taken from
//! librealsense (Apache-2.0, v2.55.1) is cited by file and line where it is used, R110's shape: the fact
//! and its name enter the tree, no code is copied and no function is transcribed. The deprojection is
//! written from the pinhole camera model, not from `rs2_deproject_pixel_to_point`.
//!
//! `no_std` is real: no `f32::sqrt`, `floor` or `powf` (those live in `std`); everything here is the four
//! arithmetic operations and integer shifts, so the same code runs in the kernel.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod depth;
pub mod frame;
pub mod imu;
pub mod ramp;
pub mod replay;
pub mod wire;

pub use depth::{DepthScale, Intrinsics, Point3, Z16Stats};
pub use frame::{Frame, FrameError, PixelFormat, StreamKind};
pub use imu::{ImuKind, ImuSample};
