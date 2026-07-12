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

pub mod color;
pub mod error;
pub mod jpeg;
pub mod parser;
pub mod png;

pub use error::LuxError;
pub use jpeg::decode_jpeg;
pub use parser::parse_arw;
pub use png::decode_png;

/// The final uncompressed, demosaiced image buffer.
/// Linear RGB (f32) structure, tightly packed.
pub struct RgbBuffer {
    /// Width of the image in pixels
    pub width: u32,
    /// Height of the image in pixels
    pub height: u32,
    /// Tightly packed linear RGB buffer (R, G, B, R, G, B...)
    pub pixels: Vec<f32>,
}

/// The image formats lux can recognize by magic bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// PNG (`\x89PNG\r\n\x1a\n`)
    Png,
    /// JPEG (`FF D8 FF`)
    Jpeg,
    /// TIFF / Sony ARW (`II*\0` or `MM\0*`)
    Arw,
}

/// Sniff the container format from the leading magic bytes. Returns `None` when
/// the bytes match no format lux recognizes.
pub fn sniff_format(bytes: &[u8]) -> Option<Format> {
    const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() >= 8 && &bytes[0..8] == PNG_MAGIC {
        return Some(Format::Png);
    }
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Some(Format::Jpeg);
    }
    if bytes.len() >= 4
        && ((&bytes[0..2] == b"II" && bytes[2] == 42 && bytes[3] == 0)
            || (&bytes[0..2] == b"MM" && bytes[2] == 0 && bytes[3] == 42))
    {
        return Some(Format::Arw);
    }
    None
}

/// Decode any lux-supported image format into an [`RgbBuffer`], dispatching on
/// the sniffed magic bytes.
pub fn decode(bytes: &[u8]) -> Result<RgbBuffer, LuxError> {
    match sniff_format(bytes) {
        Some(Format::Png) => decode_png(bytes),
        Some(Format::Jpeg) => decode_jpeg(bytes),
        Some(Format::Arw) => parse_arw(bytes),
        None => Err(LuxError::UnknownFormat),
    }
}
