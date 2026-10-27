// CHARTER: Facet (Images) — shared-core (brush_core, LEDGER SR231)
// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `brush_core` — Facet's brush engine (BRUSHCORE, LEDGER SR231).
//!
//! Facet (`handlers/facet`, CODEX §2 Images) paints with this crate; raster_core (SR230) holds the document. A
//! stroke becomes dabs, a dab becomes a coverage stamp, a stamp is composited onto an RGBA8 buffer through
//! raster_core's one blend rule. It draws no UI and does no I/O.
//!
//! * [`Brush`] and [`PressureCurve`]: diameter, hardness, spacing, opacity, flow, pressure, blend mode.
//! * [`dabs`]: walk a [`Stroke`] and emit a [`Dab`] every `spacing * diameter` px of arc length.
//! * [`stamp`]: a dab's coverage mask ([`Stamp`]) — hardness falloff, anti-aliased rim, integer square root.
//! * [`apply`] / [`paint`]: composite through `raster_core::composite::blend_pixel`; the eraser is
//!   [`BlendMode::Erase`] (destination-out).
//!
//! Design and measurements: `docs/dev/evidence/raster/BRUSHCORE.md`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod apply;
pub mod brush;
pub mod stamp;
pub mod stroke;

pub use apply::{apply, paint};
pub use brush::{Brush, PressureCurve};
pub use raster_core::BlendMode;
pub use stamp::{Stamp, isqrt, stamp};
pub use stroke::{Dab, Stroke, StrokePoint, dabs};

/// `floor(v)` for an f32 in i32 range (`f32::floor` is std-only).
#[inline]
pub(crate) fn floor(v: f32) -> i32 {
    let t = v as i32;
    if (t as f32) > v { t - 1 } else { t }
}

/// `round(v)` half up, to i64 (`f32::round` is std-only).
#[inline]
pub(crate) fn round(v: f32) -> i64 {
    let s = v + 0.5;
    let t = s as i64;
    if (t as f32) > s { t - 1 } else { t }
}

/// `round(a * b / 255)` for bytes (raster_core's `mul255`, which is crate-private there).
#[inline]
pub(crate) fn mul255(a: u32, b: u32) -> u32 {
    (a * b + 127) / 255
}
