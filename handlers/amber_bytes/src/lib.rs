// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Amber Bytes — The Block (docs/CODEX.md §2: "Partitioning (GPT), Formatting,
//! Block-level recovery").
//!
//! The handler library (AMBER1, SR34): the disk-layout half of Amber Bytes over the shared cores —
//! `amber_core` (GPT, MBR, plan → disk, verify, recover, FAT32 format) and `unafs` (UnaFS format).
//! Nothing here encodes an on-disk structure; it decides WHAT to write, WHO may write it
//! ([`policy`]), proves the caller saw it ([`signer`]), and carries it on the bus ([`bus`]). The
//! CLIs are `amber` (`list / plan / apply / verify / recover`) and the forensic `amber_bytes`.

pub mod bus;
pub mod disks;
pub mod layout;
pub mod ops;
pub mod policy;
pub mod signer;

pub use bus::{Amber, Request, Response};
pub use layout::{Fs, LayoutSpec, PartSpec};
pub use signer::{Sha256Digest, Signer};
