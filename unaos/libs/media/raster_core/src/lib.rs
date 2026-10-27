// CHARTER: Facet (Images) — shared-core (raster_core, LEDGER SR230)
// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `raster_core` — Facet's raster document model (RASTERCORE, LEDGER SR230).
//!
//! Facet (`handlers/facet`, CODEX §2 Images, "The Canvas. Raster/Vector engine") owns image editing; this crate is
//! the part of the editor both rings link (BP-handlers-4): the layered document, its masks and selections, the one
//! compositor, the edit commands with undo / redo, and its own file format. It draws no UI and does no I/O.
//!
//! * [`Document`] holds a canvas size, [`Layer`]s (bottom first) and an optional [`Selection`].
//! * Edits are [`Command`]s applied by [`Document::apply`]: pure, `&self -> Result<Document, Error>`, with
//!   [`Document::validate`] at the end (timeline_core's shape). [`History`] is the undo / redo stack.
//! * [`Document::composite`] flattens the layers through pixel_core's `blend_nonpremult` — the one src-over.
//! * [`container`]: the `UNARASTR` chunked file — a META chunk, one PNG per layer (pixel_core), raw masks, the
//!   selection.
//!
//! Design and measurements: `docs/dev/evidence/raster/RASTERCORE.md`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod command;
pub mod composite;
pub mod container;
pub mod document;
pub mod selection;

pub use command::{Command, History};
pub use container::{decode, encode};
pub use document::{BlendMode, Document, Layer, Mask};
pub use selection::Selection;

/// Why an edit, a validation or a decode failed. No `Display`: [`Error::word`] names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A zero canvas or layer dimension.
    Empty,
    /// A dimension beyond pixel_core's `MAX_DIM` / `MAX_PIXELS`.
    TooLarge,
    /// A layer index that names no layer.
    NoLayer,
    /// A pixel buffer whose length is not `width * height * 4` (or a paste's `w * h * 4`).
    Dimensions,
    /// A mask whose size is not its layer's, or whose coverage length is not `width * height`.
    Mask,
    /// A selection outside the canvas, or a seed off the canvas.
    Selection,
    /// A paste region outside its layer.
    Bounds,
    /// The container ended before a header or a chunk said it would.
    Truncated,
    /// The container does not start with `UNARASTR`.
    Magic,
    /// A container version this crate does not read.
    Version,
    /// The container breaks its own layout (named by the decode site through this one word).
    Malformed,
    /// pixel_core refused a layer's PNG.
    Png,
    /// pixel_core's encoder refused a layer.
    PngEncode,
}

impl Error {
    /// The one word for the error (the crate keeps no `Display`).
    pub fn word(&self) -> &'static str {
        match self {
            Error::Empty => "empty",
            Error::TooLarge => "too-large",
            Error::NoLayer => "no-layer",
            Error::Dimensions => "dimensions",
            Error::Mask => "mask",
            Error::Selection => "selection",
            Error::Bounds => "bounds",
            Error::Truncated => "truncated",
            Error::Magic => "magic",
            Error::Version => "version",
            Error::Malformed => "malformed",
            Error::Png => "png",
            Error::PngEncode => "png-encode",
        }
    }
}
