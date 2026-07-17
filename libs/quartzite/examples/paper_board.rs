// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! The SURFACE-1 taste-gate host.
//!
//! Opens one window holding the paper sample board — a grid of real text views,
//! each rendering the same paragraph over a candidate procedural paper surface.
//! Rows are the three algorithms (grain / laid / blotch); the first column is
//! the honest no-texture control, the rest are rising subtlety levels, each
//! labelled with its exact parameters.
//!
//! Run it (from the repo root, on the Mac Peter judges on):
//!
//! ```text
//! cargo run -p quartzite --example paper_board
//! ```
//!
//! The board is deterministic — the same pixels every launch. There are no
//! knobs by design: the arc ships an honest sample *space*, and the aesthetic
//! is Peter's to pick from these real pixels.

fn main() {
    #[cfg(target_os = "macos")]
    {
        quartzite::Backend::new_vessel(
            "org.unaos.quartzite.paper-board",
            "SURFACE-1 · paper sample board",
            quartzite::platforms::macos::paper_board::BOARD_SIZE,
            |window| quartzite::platforms::macos::paper_board::bootstrap_paper_board(window),
        )
        .run();
    }

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!(
            "paper_board: the SURFACE-1 sample board is macOS-first \
             (that is the machine the taste-gate runs on)."
        );
    }
}

