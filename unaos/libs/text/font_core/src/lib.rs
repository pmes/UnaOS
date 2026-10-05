// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: shared-core (text)
//!
//! FONTCORE (LEDGER SR48): UnaOS reads fonts from the specifications. `no_std` + `alloc`, no `unsafe`,
//! zero dependencies.
//!
//! - [`Font`]: the sfnt table directory (TrueType, OpenType/CFF, TrueType Collections), `head`, `hhea`,
//!   `maxp`, `hmtx`, `OS/2`, `post` (incl. format 2 glyph names), `cmap` 0/4/6/12/13.
//! - Outlines: `glyf`/`loca` simple and composite glyphs ([`glyf`]) and CFF Type 2 charstrings ([`cff`]),
//!   delivered as quadratic/cubic paths ([`path`]).
//! - [`raster`]: exact-area, nonzero, 256-level scanline rasterizer with subpixel origins, no hinting.
//! - [`cache`]: glyph bitmaps keyed by (font, glyph, size, subpixel x/y).
//! - [`layout`]: OpenType Layout common tables, GSUB single/ligature, GPOS pair, GDEF, legacy `kern`.
//! - [`shape`]: bidi-aware shaping for every script the OpenType engine ([`ot`]: GSUB 1–8, GPOS 1–9) and the
//!   complex shapers (Arabic joining, Devanagari, Thai, Hebrew) cover; font fallback stacks; measuring, line
//!   layout, drawing. Glyph- and cluster-identical to HarfBuzz on the FONTBIDI KATs.
//! - [`bidi`] (UAX #9), [`grapheme`] (UAX #29), [`script`] (Scripts + Script_Extensions itemizer),
//!   [`normalize`] (UAX #15 NFC/NFD), [`linebreak`] (UAX #14, every class) — tables generated from UCD 17.0.0
//!   ([`ucd`]) and proven on Unicode's conformance files in full (FONTBIDI, SR56).

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod bidi;
pub mod cache;
pub mod cff;
mod cff_strings;
pub mod cmap;
mod complex;
pub mod fmath;
pub mod font;
pub mod glyf;
pub mod grapheme;
pub mod layout;
pub mod linebreak;
pub mod name;
pub mod normalize;
pub mod ot;
pub mod path;
mod post_names;
pub mod raster;
mod reader;
pub mod script;
pub mod shape;
pub mod ui;
pub mod ucd;

pub use cache::GlyphCache;
pub use font::{Error, Font, Os2, Outlines, Post};
pub use path::{OutlineSink, Path, PathCmd};
pub use raster::{rasterize_glyph, rasterize_glyph_mode, GlyphBitmap, RenderMode};
pub use bidi::Direction;
pub use shape::{draw_text, layout_lines, measure, shape, shape_dir, shape_fallback, shape_run, Canvas, GlyphPos, Line, ShapeOptions};
