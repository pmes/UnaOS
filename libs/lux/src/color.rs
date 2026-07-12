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

//! Shared color helpers for the common-format decoders (PNG/JPEG).
//!
//! `RgbBuffer` is defined as *linear* RGB. Consumer image formats such as PNG
//! and JPEG store 8-bit samples that are sRGB-encoded, so the decoders convert
//! each sample through the sRGB electro-optical transfer function (EOTF) to keep
//! the buffer contract honest — the same linear space the ARW demosaic path
//! already produces.

/// sRGB → linear for a single 8-bit sample. Maps 0 → 0.0 and 255 → 1.0 exactly.
#[inline]
pub fn srgb_to_linear(sample: u8) -> f32 {
    let s = sample as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// Precompute the 256-entry sRGB → linear lookup table once per decode.
pub fn srgb_lut() -> [f32; 256] {
    let mut lut = [0.0f32; 256];
    for (i, e) in lut.iter_mut().enumerate() {
        *e = srgb_to_linear(i as u8);
    }
    lut
}
