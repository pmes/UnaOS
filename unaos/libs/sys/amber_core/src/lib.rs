// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Amber Bytes — shared-core
//!
//! **amber_core** — the pure disk-layout logic of the installer ensemble (audit B296; RULINGS R79):
//! one GPT encoder/decoder, one partition planner, one FAT32 BPB layout and one clone plan, linked by
//! BOTH rings — the kernel's `install/` engine (Ring 0) and the host's `tools/una-card` +
//! `handlers/amber_bytes` (Ring 3). Before this crate the GPT existed three times (the kernel's
//! `install/gpt.rs`, the Python `make-x86-card.py`, and nothing at all in Amber Bytes); the image a
//! host writes and the layout the kernel installs are now ONE code path (R28).
//!
//! No I/O lives here: every function takes and returns bytes. A caller reads sectors, hands them to
//! [`gpt::Header::decode`] / [`gpt::decode_array`], and writes what [`gpt::Image`] and [`fat32`]
//! produce. [`kat::run`] is the known-answer suite, callable in-kernel (`tests install`) and by
//! `cargo test -p amber_core`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod clone;
pub mod crc32;
pub mod fat32;
pub mod gpt;
pub mod kat;
pub mod plan;

pub use clone::ClonePlan;
pub use crc32::crc32;
pub use plan::{Part, PartKind, PartReq, Plan, PlanError, Size};

/// Bytes in one MiB, for the plan's human-facing sizes.
pub const MIB: u64 = 1024 * 1024;

/// Sectors → whole MiB (rounded down), at 512-byte sectors.
pub const fn sectors_mib(sectors: u64) -> u64 {
    sectors * gpt::SECTOR as u64 / MIB
}
