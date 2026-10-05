// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SELFHOST-2's streaming gzip (RFC 1952) + zlib (RFC 1950) + DEFLATE (RFC 1951) decoder MOVED, unchanged
// in behaviour, to the shared no_std core `unaos/libs/media/pixel_core/src/inflate.rs` (PIXELCORE, SR25):
// ONE inflater in the tree, which the kernel (SELFHOST-2's tar walk, FACET's PNG viewer) and the host
// (pixel_core's PNG decoder, Aether through gneiss_pal) all link. This file is the re-export that keeps
// every kernel caller's path — `selfhost::inflate::{gunzip, zlib_inflate, ByteSource, Sink, …}` — exactly
// where it was. The only edit the move made inside the decoder: its gzip CRC-32 comes from
// `pixel_core::crc::Crc32` (the same table and API as `crate::hash::Crc32`) instead of reaching back into
// the kernel.
pub use pixel_core::inflate::*;
