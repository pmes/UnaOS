// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Geode's core (GEODEMOD, LEDGER SR157): the `.geode` v1 archive's ONE encoder and decoder. The host handler
//! (`handlers/geode`) links this crate; the kernel (`unaos/crates/kernel/src/fs/geodew.rs`) compiles
//! `src/archive.rs` by path, so both rings write and read the same bytes from the same source.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod archive;
pub use archive::*;
