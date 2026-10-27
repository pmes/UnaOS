// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. PDF-WRITE (LEDGER SR325): saving a parsed PDF and printing to PDF. The writer reads the
//! object graph only through [`crate::Document`]'s own surface (`trailer`, `resolve`: decryption is the document's, never
//! redone here) and compresses only through pixel_core's one compressor (`pixel_core::deflate::compress`, SHOTZIP's
//! LZ77 + fixed-Huffman engine).
//!
//! * [`write_object`]: §7.3 syntax out (strings literal or hex, names with `#xx`, reals without exponents).
//! * [`Edit`]: changes held over a borrowed document (`set`, `add`, `set_info`); `save` writes every object reachable from
//!   the trailer (numbers and generations kept) with a fresh CLASSIC xref table — chosen for PDF 1.4 compatibility; the
//!   /Encrypt dictionary is dropped, so saving decrypts (re-encryption is owed) — and `save_update` appends only the
//!   changed objects with a new table and a trailer whose /Prev names the original's (the real-world "save", which keeps
//!   the original bytes, and so any signature over them, as a prefix). [`crate::Document::save`] is `Edit::new(doc).save()`.
//! * [`Builder`] / [`PageBuilder`]: new PDF 1.4 files from path operators and images (Flate RGB / Gray, DCT passed
//!   through); text is drawn as paths (embedded fonts are owed: PDF-WRITE-FONTS). [`from_rgba`]: one image per page,
//!   the "print what you see" primitive PDF4's face prints through.

mod builder;
mod serialize;

pub use builder::{Builder, Image, PageBuilder, RgbaPage, from_rgba};
pub use serialize::{Edit, write_object};
