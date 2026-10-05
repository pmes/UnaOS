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
//! - [`shape`]: Latin/Greek/Cyrillic shaping, measuring, line layout, drawing.
//! - [`linebreak`]: UAX #14 for the scripts above, checked against LineBreakTest-17.0.0.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod cache;
pub mod cff;
mod cff_strings;
pub mod cmap;
pub mod fmath;
pub mod font;
pub mod glyf;
pub mod layout;
pub mod linebreak;
pub mod path;
mod post_names;
pub mod raster;
mod reader;
pub mod shape;

pub use cache::GlyphCache;
pub use font::{Error, Font, Os2, Outlines, Post};
pub use path::{OutlineSink, Path, PathCmd};
pub use raster::{rasterize_glyph, rasterize_glyph_mode, GlyphBitmap, RenderMode};
pub use shape::{draw_text, layout_lines, measure, shape, Canvas, GlyphPos, Line, ShapeOptions};
