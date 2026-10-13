// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VP9CORE (LEDGER SR116): a VP9 decoder written from the VP9 Bitstream & Decoding Process
//! Specification (v0.6, 2016-03-31), `no_std` + `alloc`, no dependencies.
//!
//! CHARTER: Stria (A/V) — seam `shared-core` (a `no_std` lib both rings link, beside vp8_core and
//! av1_core).

#![no_std]
#![forbid(unsafe_code)]
// The decoder indexes arrays the way the spec's pseudo-code does; iterator rewrites would hide that.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod bits;
pub mod bool_decoder;
pub mod consts;
pub mod frame;
pub mod header;
pub mod inter;
pub mod intra;
pub mod loopfilter;
pub mod probs;
pub mod tables;
pub mod transform;

/// Every way a decode can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Ran out of bytes.
    Truncated,
    /// The bitstream violates the specification.
    Invalid(&'static str),
    /// A valid stream this decoder refuses by name (profiles 1 and 3: 4:2:2 / 4:4:0 / 4:4:4).
    Unsupported(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => f.write_str("vp9: truncated"),
            Error::Invalid(s) => write!(f, "vp9: invalid ({s})"),
            Error::Unsupported(s) => write!(f, "vp9: unsupported ({s})"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;
