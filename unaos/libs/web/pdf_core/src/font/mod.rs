// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF fonts and text (PDF3, LEDGER SR314; ISO 32000-1 §9).
//!
//! CHARTER: Aether — shared-core. The part of the Acrobat line (CODEX §2 Web) that turns a font resource and the
//! bytes of a shown string into glyph outlines, advances and Unicode text. Every outline comes from font_core through
//! its `OutlineSink`: TrueType / OpenType (`/FontFile2`, `/FontFile3 /OpenType`) through `font_core::Font`, bare CFF
//! (`/FontFile3 /Type1C`, `/CIDFontType0C`) through `font_core::cff::Cff`, Type 1 (`/FontFile`) through
//! `font_core::type1`. Nothing here rasterizes or paints: PDF2's renderer paints what [`GlyphSink::show`] returns.
//!
//! * [`dict`]: the font dictionary ([`PdfFont::load`]): simple fonts (/Type1 /MMType1 /TrueType), /Type0 with its
//!   CIDFontType0/2 descendant, /Type3; the descriptor's programs; /Widths /FirstChar /MissingWidth, /W /DW;
//!   /CIDToGIDMap; the standard 14 and every unembedded font drawn with a caller-supplied [`Fallback`] face.
//! * [`encoding`]: StandardEncoding (font_core's), WinAnsi, MacRoman, /Differences, and glyph names -> Unicode (AGL).
//! * [`cmap`]: ONE CMap reader for encoding CMaps (codespace, cidrange / cidchar) and /ToUnicode (bfchar / bfrange).
//! * [`type1`]: the `/FontFile` glue to font_core's Type 1 reader.
//! * [`glyphs`]: [`GlyphSink`]: codes -> glyphs -> outlines in text space plus the §9.4.4 advance ([`Shown`]); Type 3
//!   glyphs come back as CharProcs for PDF2's interpreter ([`Type3Glyph`]).
//! * [`text`]: text extraction ([`TextCollector`], `Page::text`) with word and line assembly by position.

pub mod cmap;
pub mod dict;
pub mod encoding;
pub mod glyphs;
pub mod text;
pub mod type1;

pub use dict::{FontKind, PdfFont};
pub use glyphs::{GlyphSink, Shown, TextParams, Type3Glyph};
pub use text::{TextCollector, TextSpan, spans_to_string};

/// The faces unembedded fonts are drawn with (the standard 14 and any font without a program), handed in by the
/// caller as file bytes: pdf_core does no I/O, and a kernel caller is not made to embed fonts it does not want.
/// Helvetica / Arial / sans faces use `sans`; Courier / fixed-pitch use `mono`; Times / serif use `serif`, else `sans`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fallback<'f> {
    pub sans: Option<&'f [u8]>,
    pub mono: Option<&'f [u8]>,
    pub serif: Option<&'f [u8]>,
}

/// How deep Type 3 glyphs may nest (a CharProc showing Type 3 text): PDF2's interpreter checks it.
pub const MAX_TYPE3_DEPTH: u8 = 4;

/// A PDF matrix `[a b c d e f]` (row-vector convention: `p' = p x M`).
pub type Matrix = [f64; 6];

pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` then `b` (`p x a x b`).
pub fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// Apply `m` to a point.
pub fn apply(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (x * m[0] + y * m[2] + m[4], x * m[1] + y * m[3] + m[5])
}
