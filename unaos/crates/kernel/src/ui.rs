// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! UI-1 — the UI metrics layer: the ONE place on-screen UI geometry comes from.
//!
//! **THE METRICS RULE (standing directive, UI-1):** *no absolute pixel sizes in UI code.*
//! An integer `scale` is derived from the panel's real height at surface init, and every
//! UI dimension — glyph cell, line pitch, margins, cursor, meter geometry — derives from
//! [`Metrics`]. UI code never names a pixel count; it asks the metrics. That is what makes
//! the same console/meters read correctly on a 640×480 QEMU panel, the 1280×800 OVMF GUI,
//! and a 2880×1800 Retina panel alike.
//!
//! The one deliberate exception is the pre-heap boot console (`video/fbcon.rs`), which keeps
//! its own unscaled 8-px font: it exists to get *something* legible on screen before the
//! allocator (and thus this layer's consumers) are up, and it is out of the GUI's life cycle.
//!
//! Everything here is integer maths (no float in the kernel) and allocation-free, so the
//! metrics can be derived per call — there is no global to initialise and no state to go
//! stale if a future surface changes mode.

/// The base bitmap font cell in pixels (`font8x8` glyphs are 8×8) — the one place its size
/// is named. Every text metric is `BASE_CELL * scale` derived.
pub const BASE_CELL: usize = 8;

/// The panel height granted one extra scale step: `scale = clamp(height / SCALE_STEP, 1, 4)`.
/// ≤ 1799 rows render 1:1, 1800p-class panels (e.g. Retina 2880×1800) at 2×, capped at 4×.
pub const SCALE_STEP: usize = 900;

/// The scale cap — beyond 4× legibility gains nothing and glyph blocks get blocky-huge.
pub const SCALE_MAX: usize = 4;

/// Scale-derived UI geometry. Build one from the panel with [`Metrics::for_height`] (or, with
/// a PAL in hand, `pal.metrics()`); pass it down or re-derive at will — it is a pure function
/// of the panel height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    /// Integer glyph magnification (each font pixel renders as a `scale`×`scale` block).
    pub scale: usize,
    /// Glyph cell width in pixels (`BASE_CELL * scale`). The text advance per character —
    /// and the cursor is EXACTLY one `cell_w`×`cell_h` cell, by construction.
    pub cell_w: usize,
    /// Glyph cell height in pixels (`BASE_CELL * scale`).
    pub cell_h: usize,
    /// Text row pitch: `cell_h` plus half-cell leading (1.5-line rhythm). The input-line
    /// clear strip is exactly this tall.
    pub line_h: usize,
    /// Page margin — the left/top inset of screen-owning views. One line-height, so the
    /// margin breathes with the type.
    pub margin: usize,
}

impl Metrics {
    /// Derive the metrics for a panel `height` pixels tall.
    ///
    /// `scale = clamp(height / SCALE_STEP, 1, SCALE_MAX)` — 1 at ≤ 900p (and any degenerate
    /// 0-height headless surface), 2 at 1800p, capped at 4 — then every metric follows:
    /// `cell = BASE_CELL·scale`, `line_h = cell + cell/2`, `margin = line_h`.
    pub const fn for_height(height: usize) -> Metrics {
        let mut scale = height / SCALE_STEP;
        if scale < 1 {
            scale = 1;
        }
        if scale > SCALE_MAX {
            scale = SCALE_MAX;
        }
        let cell = BASE_CELL * scale;
        let line_h = cell + cell / 2;
        Metrics { scale, cell_w: cell, cell_h: cell, line_h, margin: line_h }
    }

    /// Width of `n` characters of text, in pixels (the advance is one cell per char).
    #[inline]
    pub const fn text_w(&self, n: usize) -> usize {
        n * self.cell_w
    }
}
