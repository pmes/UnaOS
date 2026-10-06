// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! RAWCORE (rmbp-ledger B444): the ARW path is `raw_core` — the no_std core the kernel links through pixel_core —
//! and this module adapts it to lux's API (`parse_arw` -> linear f32 [`RgbBuffer`]). The earlier host-only
//! parser (its "4-bit/11-bit/7-bit" Compression-32769 reader was not Sony's format) is gone: Sony's compressed
//! raw is Compression 32767 with BitsPerSample 8 (cRAW), which raw_core decodes.

use crate::{RgbBuffer, error::LuxError};

pub use raw_core;

fn lux_err(e: raw_core::Error, compression: u16) -> LuxError {
    match e {
        raw_core::Error::Truncated => LuxError::BufferTooSmall,
        raw_core::Error::NotTiff => LuxError::InvalidMagic,
        raw_core::Error::NoRaw => LuxError::MissingData,
        raw_core::Error::Unsupported(_) => LuxError::UnsupportedCompression(compression),
        raw_core::Error::Malformed(_) | raw_core::Error::TooLarge | raw_core::Error::OutOfMemory | raw_core::Error::NoPreview => {
            LuxError::CorruptData
        }
    }
}

/// Decode a Sony ARW (or any TIFF-EP raw raw_core reads) to linear RGB, bilinear-demosaiced.
pub fn parse_arw(bytes: &[u8]) -> Result<RgbBuffer, LuxError> {
    let info = raw_core::parse(bytes).map_err(|e| lux_err(e, 0))?;
    let s = info.strip.ok_or(LuxError::MissingData)?;
    let d = raw_core::RowDecoder::new(&s).map_err(|e| lux_err(e, s.compression))?;
    let end = (s.offset + s.len) as usize;
    let strip = bytes.get(s.offset as usize..end).ok_or(LuxError::CorruptData)?;
    let m = d.all(strip).map_err(|e| lux_err(e, s.compression))?;
    let white = if s.white == 0 { d.max_code } else { s.white };
    let pixels = raw_core::bilinear_linear(&m, d.width, d.height, &s.cfa, s.black, white).map_err(|e| lux_err(e, s.compression))?;
    Ok(RgbBuffer { width: s.width, height: s.height, pixels })
}
