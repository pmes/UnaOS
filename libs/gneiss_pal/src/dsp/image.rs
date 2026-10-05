// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Still images — a thin re-export of `pixel_core` (unaos/libs/media/pixel_core, PIXELCORE / SR25):
//! PNG, JPEG (baseline + progressive), GIF (animated), BMP, QOI and WebP (lossless, and lossy through
//! vp8_core — VP8CORE / SR40), written from the
//! specifications, the same code the kernel's Facet viewer runs. `decode` sniffs the format; anything it
//! refuses is `Error::Unsupported` / `Error::UnknownFormat` and the caller decides what to fall back to.

pub use pixel_core::{
    Error, Format, Frame, Image, MAX_DIM, MAX_PIXELS, decode, decode_bmp, decode_gif, decode_jpeg, decode_png,
    decode_qoi, decode_webp, sniff,
};
