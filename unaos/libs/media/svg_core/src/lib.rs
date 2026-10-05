// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: shared-core (media)
//!
//! SVGCORE (LEDGER SR52): UnaOS renders SVG from the specifications. `no_std` + `alloc`, no `unsafe`; the one
//! dependency is UnaOS's own font_core.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod color;
pub mod css;
pub mod fmath;
pub mod geom;
pub mod paint;
pub mod raster;
pub mod stroke;
pub mod style;
pub mod xml;
