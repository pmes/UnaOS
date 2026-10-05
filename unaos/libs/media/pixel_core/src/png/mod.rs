// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PNG — the decoder ([`decode`]) and the kernel's SHOTZIP encoder ([`encode`]), one module, one
//! inflater ([`crate::inflate`]).

mod decode;
pub mod encode;

pub use decode::decode;
