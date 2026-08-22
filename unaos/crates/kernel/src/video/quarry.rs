// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! QUARRY — the file manager's KNOB SEAM. The module itself is compiled with the desktop furniture
//! family (x86 `wc` **or** aarch64 `pidesk`, the same gate `dock`/`strip`/`menubar`/`crystal` carry),
//! but the implementation behind it is armed by `UNAOS_QUARRY=1` (the `quarry` feature) and nothing
//! else.
//!
//! ### Why the seam is here rather than at each call site
//!
//! Quarry's two input hooks live in `arch/aarch64/syscall.rs` — the key router and the click router —
//! and that file is compiled into the **knob-off `kernel8.img`**, whose byte-identity proof a single
//! ADDED line breaks (panic `Location` records embed file line numbers; PI-DESK measured a `cfg`-only
//! `wm.rs` edit moving the knob-off hash for exactly this reason). So the hooks had to be folded into
//! the `#[cfg(feature = "pidesk")]` one-liners that were already there, and a folded call cannot carry
//! a second `cfg` of its own without becoming a second line.
//!
//! The seam resolves that: the NAME `video::quarry::key_route` exists whenever `pidesk` does, and this
//! file decides whether it reaches [`live`] or a `false`. With `UNAOS_QUARRY` off the stubs below are
//! `#[inline(always)] false` / no-ops, the implementation is not compiled at all, and the two edited
//! router lines are the same length they were — so knob-off byte-identity is untouched and a
//! `pidesk`-only build carries no file manager.

/// The implementation. Compiled only when `UNAOS_QUARRY=1` armed the `quarry` feature.
#[cfg(feature = "quarry")]
pub mod live;

#[cfg(feature = "quarry")]
pub use live::{close, is_open, key_route, open, press_route, request_open, service, OWNER};

#[cfg(all(feature = "quarry", feature = "witness"))]
pub use live::{selftest, selftest_result};

// ── Knob OFF ────────────────────────────────────────────────────────────────────────────────────
//
// One `false` and four empty bodies. Every one is `#[inline(always)]`, so the folded router calls
// compile to nothing at all rather than to a call the linker has to keep.

#[cfg(not(feature = "quarry"))]
#[inline(always)]
pub fn is_open() -> bool {
    false
}

#[cfg(not(feature = "quarry"))]
#[inline(always)]
pub fn key_route(_ev: crate::pal::Event) -> bool {
    false
}

#[cfg(not(feature = "quarry"))]
#[inline(always)]
pub fn press_route(_x: i32, _y: i32) -> bool {
    false
}

#[cfg(not(feature = "quarry"))]
#[inline(always)]
pub fn open() {}

#[cfg(not(feature = "quarry"))]
#[inline(always)]
pub fn close() {}

#[cfg(not(feature = "quarry"))]
#[inline(always)]
pub fn request_open() {}

#[cfg(not(feature = "quarry"))]
#[inline(always)]
pub fn service() {}
