// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PNG-8/RGB encoder (SHOTZIP) — MOVED, byte-for-byte, to the shared no_std core
//! `unaos/libs/media/pixel_core/src/png/encode.rs` (PIXELCORE, SR25), next to the PNG decoder and the
//! one inflater. This module is the re-export that keeps every kernel caller (`prtscr`'s
//! `PngEncoder`/`PngError`/`CHUNK`/`MAX_CHAIN`, `facet`'s `crc32`/`adler32`) on its old path.
pub use pixel_core::png::encode::*;
