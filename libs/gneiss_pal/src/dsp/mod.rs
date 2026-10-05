// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Gneiss DSP — the signal-processing graph's codecs (CODEX §2). Each codec's core is a `no_std` crate
//! the kernel links too; this module is the host face every handler goes through (no app silos).

pub mod image;
