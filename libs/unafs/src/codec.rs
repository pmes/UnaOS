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

//! The central (de)serialization seam for everything that reaches disk.
//!
//! Wraps bincode 2.x in its [`legacy`](bincode::config::legacy) configuration —
//! little-endian byte order, fixed-int width, no length limit — which reproduces
//! the historical bincode 1.3.3 on-disk encoding **byte-for-byte**. Every
//! serialize/deserialize of an on-disk structure in the crate routes through
//! here, and the golden vectors in `tests/kat_vectors.rs` pin the resulting
//! bytes, so the on-disk format stays frozen across library upgrades.
//!
//! This module is `no_std`-friendly (needs only `alloc`); the host-only backends
//! live behind the `std` feature.

use serde::Serialize;
use serde::de::DeserializeOwned;

use alloc::vec::Vec;

/// Error raised by the codec seam.
///
/// bincode 2.x splits encoding and decoding failures into two distinct types;
/// this unifies them behind one crate-facing error so downstream `#[from]`
/// conversions stay simple.
#[derive(Debug)]
pub enum CodecError {
    /// A value could not be encoded.
    Encode(bincode::error::EncodeError),
    /// Bytes could not be decoded into the target type.
    Decode(bincode::error::DecodeError),
}

impl core::fmt::Display for CodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CodecError::Encode(e) => write!(f, "encode error: {e}"),
            CodecError::Decode(e) => write!(f, "decode error: {e}"),
        }
    }
}

// `core::error::Error` is re-exported as `std::error::Error` under std, so this
// single impl satisfies thiserror's `#[from]` in both std and no_std builds.
impl core::error::Error for CodecError {}

impl From<bincode::error::EncodeError> for CodecError {
    fn from(e: bincode::error::EncodeError) -> Self {
        CodecError::Encode(e)
    }
}

impl From<bincode::error::DecodeError> for CodecError {
    fn from(e: bincode::error::DecodeError) -> Self {
        CodecError::Decode(e)
    }
}

/// Serialize a value into the frozen on-disk byte layout.
///
/// Accepts unsized `T` (e.g. slices) so `serialize(&[entry])` works as it did
/// under bincode 1.3.
pub fn serialize<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CodecError> {
    bincode::serde::encode_to_vec(value, bincode::config::legacy()).map_err(CodecError::Encode)
}

/// Deserialize a value from the frozen on-disk byte layout.
///
/// Trailing bytes after the decoded value are ignored — on-disk records live in
/// fixed-size blocks padded with zeros, so the encoded prefix is authoritative.
pub fn deserialize<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    bincode::serde::decode_from_slice(bytes, bincode::config::legacy())
        .map(|(value, _len)| value)
        .map_err(CodecError::Decode)
}
