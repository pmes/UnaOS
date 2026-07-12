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

//! JPEG decode into `RgbBuffer` via the `zune-jpeg` crate.

use crate::color::srgb_lut;
use crate::{RgbBuffer, error::LuxError};
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// Decode JPEG bytes into a linear-RGB [`RgbBuffer`].
///
/// The decoder is forced to emit interleaved 8-bit RGB regardless of the
/// source chroma subsampling or grayscale encoding; samples are then converted
/// from sRGB to linear (see [`crate::color`]).
pub fn decode_jpeg(bytes: &[u8]) -> Result<RgbBuffer, LuxError> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(std::io::Cursor::new(bytes), options);

    let rgb = decoder
        .decode()
        .map_err(|e| LuxError::Decode(format!("jpeg: {e:?}")))?;

    let (width, height) = decoder
        .dimensions()
        .ok_or(LuxError::Decode("jpeg: missing dimensions".into()))?;

    let px = (width as usize)
        .checked_mul(height as usize)
        .ok_or(LuxError::CorruptData)?;
    let expected = px.checked_mul(3).ok_or(LuxError::CorruptData)?;
    if rgb.len() < expected {
        return Err(LuxError::CorruptData);
    }

    let lut = srgb_lut();
    let mut pixels = Vec::with_capacity(expected);
    for p in 0..px {
        let base = p * 3;
        pixels.push(lut[rgb[base] as usize]);
        pixels.push(lut[rgb[base + 1] as usize]);
        pixels.push(lut[rgb[base + 2] as usize]);
    }

    Ok(RgbBuffer {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}
