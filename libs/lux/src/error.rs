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

#[derive(Debug)]
pub enum LuxError {
    /// Not enough data to parse or access slice
    BufferTooSmall,
    /// The file does not appear to be a valid TIFF/ARW
    InvalidMagic,
    /// Unrecognized or unsupported endianness indicator
    UnsupportedEndianness,
    /// Could not find the required tags or directories
    MissingData,
    /// Compression type is not supported yet
    UnsupportedCompression(u16),
    /// CFA pattern not supported
    UnsupportedCFA,
    /// Data is corrupt
    CorruptData,
}

impl std::fmt::Display for LuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LuxError::BufferTooSmall => write!(f, "Buffer is too small"),
            LuxError::InvalidMagic => write!(f, "Invalid magic number, expected TIFF"),
            LuxError::UnsupportedEndianness => write!(f, "Unsupported endianness"),
            LuxError::MissingData => write!(f, "Missing required tags or data"),
            LuxError::UnsupportedCompression(c) => write!(f, "Unsupported compression scheme: {}", c),
            LuxError::UnsupportedCFA => write!(f, "Unsupported CFA pattern"),
            LuxError::CorruptData => write!(f, "Data is corrupt"),
        }
    }
}

impl std::error::Error for LuxError {}
