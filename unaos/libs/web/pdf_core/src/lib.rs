// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `pdf_core` — the PDF reader core (PDF1, LEDGER SR300).
//!
//! CHARTER: Aether — shared-core. Aether (`handlers/aether`, CODEX §2 Web, "Chrome, Acrobat") owns the PDF line as it
//! owns the web page; this crate is the part both rings link (BP-handlers-4): it turns the bytes of a PDF file
//! (ISO 32000-1) into objects and pages. It draws nothing and does no I/O — PDF2 interprets the content streams, PDF3
//! reads the fonts, PDF4 is the Aether face.
//!
//! * [`lexer`]: §7.2 tokens — whitespace, `%` comments, names with `#xx`, literal strings (escapes, octal, balanced
//!   parentheses, EOL normalisation), hex strings, numbers.
//! * [`object`]: §7.3 objects ([`Object`]: Null Bool Int Real String Name Array Dict Stream Ref), the indirect-object
//!   parser, and the text-string decoder (PDFDocEncoding, UTF-16BE with a BOM).
//! * [`xref`]: §7.5 — the classic table, xref streams (§7.5.8: /W, /Index), the /Prev chain (a visited set ends a loop),
//!   /XRefStm hybrid files and incremental updates (the newest section wins), and RECONSTRUCTION: when `startxref`,
//!   a table or a stream is broken, the file is scanned for `N G obj` and `trailer`. Every real-world reader has such a
//!   fallback (broken offsets are common in the wild); so does this one, and [`XrefKind::Rebuilt`] says when it ran.
//! * [`filters`]: §7.4 — FlateDecode (through pixel_core's inflater, with the PNG and TIFF predictors), LZWDecode
//!   (/EarlyChange), ASCIIHexDecode, ASCII85Decode, RunLengthDecode; the image codecs (DCT, JPX, CCITTFax, JBIG2)
//!   pass through as raw bytes with their filter name kept.
//! * [`doc`]: [`Document::parse`], `resolve` (object cache, object streams), the page tree with inherited attributes
//!   ([`Page`]), the /Info strings. /Encrypt is refused by name ([`Error::Encrypted`]; PDF-CRYPT is owed).
//! * [`content`]: the content-stream TOKENIZER (operands + operator, inline images) — PDF2 interprets it.
//!
//! Everything is bounded: nesting and reference chains by [`MAX_DEPTH`], object counts by [`MAX_OBJECTS`], every
//! decoded stream by [`MAX_DECODED`], the page tree by [`MAX_PAGES`] and a visited set.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod content;
pub mod doc;
pub mod filters;
pub mod lexer;
pub mod object;
pub mod xref;

pub use doc::{Document, Info, Page, XrefKind};
pub use object::{Dict, Object, Ref, Stream};

/// The deepest nesting (arrays, dictionaries, page-tree levels) and the longest reference chain followed.
pub const MAX_DEPTH: usize = 64;
/// The most objects one xref may name.
pub const MAX_OBJECTS: usize = 1 << 22;
/// The most bytes one decoded stream may hold (each filter stage is capped).
pub const MAX_DECODED: usize = 256 * 1024 * 1024;
/// The most pages one document may list.
pub const MAX_PAGES: usize = 1 << 20;

/// Why a PDF (or a part of one) was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The bytes are not the syntax the spec names; the reason and the byte offset.
    Syntax(&'static str, usize),
    /// The trailer names /Encrypt: the standard security handler is PDF-CRYPT (owed).
    Encrypted,
    /// A feature this core does not read yet, by name.
    Unsupported(&'static str),
    /// Nesting or a reference chain deeper than [`MAX_DEPTH`].
    Depth,
    /// A count or a decoded size over its cap.
    TooLarge,
    /// A reference cycle (an object that needs itself, a page tree that loops).
    Loop,
    /// A required object is absent (no /Root, no /Pages).
    Missing,
    /// FlateDecode refused the stream; pixel_core's reason.
    Inflate(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Syntax(why, at) => write!(f, "syntax: {why} at byte {at}"),
            Error::Encrypted => f.write_str("encrypted (PDF-CRYPT owed)"),
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::Depth => f.write_str("nesting too deep"),
            Error::TooLarge => f.write_str("over a size cap"),
            Error::Loop => f.write_str("reference loop"),
            Error::Missing => f.write_str("required object missing"),
            Error::Inflate(why) => write!(f, "inflate: {why}"),
        }
    }
}
