// CHARTER: Facet (Images) — shared-core (adjust_core, LEDGER SR232)
// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `adjust_core` — Facet's image adjustments as pure functions (ADJUSTCORE, LEDGER SR232).
//!
//! Facet (`handlers/facet`, CODEX §2 Images, "The Canvas. Raster/Vector engine") owns image editing; this crate is
//! the adjustments part both rings link (BP-handlers-4). Every function has ONE shape:
//! `fn(px: &[u8], w: u32, h: u32, params) -> Result<Vec<u8>, Error>` over straight (non-premultiplied) RGBA8,
//! row-major, `w * h * 4` bytes (raster_core's layer layout, restated: this crate has no dependency).
//!
//! * [`levels`] / [`curves`] — per-channel 256-entry tables.
//! * [`hue_sat`], [`invert`], [`threshold`] — colour.
//! * [`box_blur`], [`gaussian_blur`] — separable, edge clamp; they blur alpha too.
//! * [`unsharp`] — original + amount x (original - gaussian), thresholded.
//!
//! Everything but the two blurs touches R, G, B only and copies A. No float: integer arithmetic with named scales.
//! Design and measurements: `docs/dev/evidence/raster/ADJUSTCORE.md`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod blur;
pub mod color;
pub mod curves;
pub mod levels;
pub mod sharpen;

pub use blur::{GAUSS_SHIFT, box_blur, gaussian_blur, gaussian_kernel};
pub use color::{HUE_CIRCLE, HueSat, hue_sat, invert, luma, threshold};
pub use curves::{Curves, curves};
pub use levels::{Levels, levels, levels_lut};
pub use sharpen::{Unsharp, unsharp};

/// Why an adjustment refused. No `Display`: [`Error::word`] names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The buffer is not `w * h * 4` bytes (or `w * h * 4` overflows `usize`).
    BadLength,
    /// A parameter outside its documented range.
    BadParam,
}

impl Error {
    /// The one word for logs and wire lines.
    pub fn word(&self) -> &'static str {
        match self {
            Error::BadLength => "bad-length",
            Error::BadParam => "bad-param",
        }
    }
}

/// THE length check: `px.len() == w * h * 4`, the product taken with `checked_mul`.
pub(crate) fn check(px: &[u8], w: u32, h: u32) -> Result<(), Error> {
    let n = (w as usize)
        .checked_mul(h as usize)
        .and_then(|p| p.checked_mul(4))
        .ok_or(Error::BadLength)?;
    if px.len() == n { Ok(()) } else { Err(Error::BadLength) }
}

/// Apply one table to R, G, B (alpha copied). Shared by levels and curves.
pub(crate) fn map_rgb(px: &[u8], lut: [&[u8; 256]; 3]) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::with_capacity(px.len());
    for p in px.chunks_exact(4) {
        out.push(lut[0][p[0] as usize]);
        out.push(lut[1][p[1] as usize]);
        out.push(lut[2][p[2] as usize]);
        out.push(p[3]);
    }
    out
}

/// Signed division rounding half away from zero (`d > 0`).
pub(crate) fn div_round(n: i64, d: i64) -> i64 {
    if n >= 0 { (n + d / 2) / d } else { -((-n + d / 2) / d) }
}

/// Clamp to a byte.
pub(crate) fn clamp_u8(v: i64) -> u8 {
    v.clamp(0, 255) as u8
}
