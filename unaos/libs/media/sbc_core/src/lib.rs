// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! SBC (A2DP, rmbp-ledger B739): the low-complexity subband codec every A2DP sink must decode, written from the
//! A2DP specification's SBC chapter (frame syntax, the analysis filter and its prototype coefficients, scale
//! factors, joint stereo, the loudness/SNR bit allocation, the CRC-8 x^8+x^4+x^3+x^2+1). `no_std`, no `alloc`, no
//! dependencies, integer arithmetic only.
//!
//! CHARTER: Stria (A/V) — seam `shared-core` (a `no_std` lib both rings link; the kernel's A2DP sink includes
//! [`sbc`]'s one source file by `#[path]`).
//!
//! The encoder lives in `src/sbc.rs` alone (no `crate::` path in it) so the kernel can include the same file.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod sbc;
pub use sbc::*;

#[cfg(test)]
mod tests;
