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

//! PNG decode into `RgbBuffer` via the `png` crate.

use crate::color::srgb_lut;
use crate::{RgbBuffer, error::LuxError};
use png::{ColorType, Transformations};

/// Decode PNG bytes into a linear-RGB [`RgbBuffer`].
///
/// Palette, grayscale, and low-bit-depth inputs are expanded to 8-bit and
/// 16-bit samples are stripped to 8-bit; any alpha channel is dropped. Samples
/// are converted from sRGB to linear (see [`crate::color`]).
pub fn decode_png(bytes: &[u8]) -> Result<RgbBuffer, LuxError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // EXPAND: palette → RGB, grayscale < 8bpp → 8bpp, tRNS → alpha.
    // STRIP_16: 16-bit channels → 8-bit.
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);

    let mut reader = decoder
        .read_info()
        .map_err(|e| LuxError::Decode(format!("png header: {e}")))?;

    let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| LuxError::Decode(format!("png frame: {e}")))?;

    let width = info.width;
    let height = info.height;
    let px = (width as usize)
        .checked_mul(height as usize)
        .ok_or(LuxError::CorruptData)?;

    // Channels per pixel after the transformations above (always 8-bit).
    let channels = match info.color_type {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
        // EXPAND turns Indexed into Rgb/Rgba, so this should not occur.
        ColorType::Indexed => return Err(LuxError::Decode("png: unexpanded palette".into())),
    };

    let expected = px
        .checked_mul(channels)
        .ok_or(LuxError::CorruptData)?;
    // next_frame only fills the first frame's bytes; validate we have them.
    if (info.buffer_size()) < expected || buf.len() < expected {
        return Err(LuxError::CorruptData);
    }

    let lut = srgb_lut();
    let mut pixels = Vec::with_capacity(px * 3);
    for p in 0..px {
        let base = p * channels;
        let (r, g, b) = match channels {
            1 | 2 => {
                let v = buf[base];
                (v, v, v)
            }
            _ => (buf[base], buf[base + 1], buf[base + 2]),
        };
        pixels.push(lut[r as usize]);
        pixels.push(lut[g as usize]);
        pixels.push(lut[b as usize]);
    }

    Ok(RgbBuffer {
        width,
        height,
        pixels,
    })
}
