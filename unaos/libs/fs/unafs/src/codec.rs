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
//! little-endian byte order, fixed-int width — which reproduces the historical
//! bincode 1.3.3 on-disk encoding **byte-for-byte**. Every serialize/deserialize
//! of an on-disk structure in the crate routes through here, and the golden
//! vectors in `tests/kat_vectors.rs` pin the resulting bytes, so the on-disk
//! format stays frozen across library upgrades.
//!
//! ## Decode limits (BEFS-HARDEN, K3-PARSE-3)
//!
//! The volume is untrusted input, and bincode's owned-bytes decode path
//! (`String`/`Vec<u8>`) allocates from the record's *claimed* length **before**
//! reading a single payload byte (`impl_alloc.rs`: `claim_container_read` then
//! `alloc::vec![0u8; len]` — the claim is a no-op under plain `legacy()`'s
//! `NoLimit`). A crafted length prefix therefore drove an unbounded, infallible
//! allocation. Decodes now carry a byte limit
//! ([`with_limit`](bincode::config::Configuration::with_limit)), which turns
//! that claim into a checked budget: a hostile prefix fails with
//! `DecodeError::LimitExceeded` before any allocation. Limits bound decoding
//! only — the encoding (and therefore the KAT-pinned on-disk bytes) is
//! untouched.
//!
//! Two tiers, matching the two record shapes on disk:
//! * [`deserialize_block`] — records that must fit one 4096 B block
//!   (superblock, inode, journal op). Limit: [`BLOCK_RECORD_LIMIT`].
//! * [`deserialize`] — extent-backed records already bounded by the volume
//!   span at the read layer (directory entry lists, the attribute catalog,
//!   spilled attribute values). Limit: [`MAX_RECORD_BYTES`].
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

/// Decode budget for block-sized records (superblock, inode, journal op).
///
/// Such a record's encoding must fit one 4096 B block; the budget is doubled
/// for headroom because bincode's container claims are transient peaks (a
/// `Vec<Extent>` claims `len * size_of::<Extent>()` up front and un-claims per
/// element), not exact encoded sizes.
pub const BLOCK_RECORD_LIMIT: usize = 8192;

/// Decode budget for extent-backed records (directory entry lists, the
/// attribute catalog, spilled attribute values).
///
/// These records span extents, so they are not block-bounded — the read layer
/// already bounds their byte source against the volume span — but their
/// *claimed* lengths still need a hard ceiling so a crafted prefix cannot
/// demand an arbitrary pre-allocation. The ceiling must sit WELL BELOW the
/// kernel's guaranteed-free heap: bincode's `with_limit` only refuses claims
/// ABOVE the budget — a passing claim is followed by an INFALLIBLE internal
/// allocation, so the budget itself is the largest allocation a hostile
/// prefix can force (r12 panel: 64 MiB > the 48 MiB kernel heap → abort).
/// 4 MiB is generous for every record the format produces today (~100k dir
/// entries) and safe under the kernel heap minus the video back buffer;
/// raise it deliberately (with review) if a legit record ever approaches it.
pub const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;

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
///
/// Decoding is budgeted at [`MAX_RECORD_BYTES`]: a hostile length prefix fails
/// with a decode error instead of pre-allocating (see the module docs).
pub fn deserialize<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    bincode::serde::decode_from_slice(
        bytes,
        bincode::config::legacy().with_limit::<MAX_RECORD_BYTES>(),
    )
    .map(|(value, _len)| value)
    .map_err(CodecError::Decode)
}

/// Deserialize a record that must fit a single 4096 B block (superblock,
/// inode, journal op) — same frozen byte layout as [`deserialize`], with the
/// tight [`BLOCK_RECORD_LIMIT`] decode budget: a block-sized record claiming
/// more than a block's worth of payload is corrupt by definition and fails
/// before any allocation (see the module docs).
pub fn deserialize_block<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    deserialize_block_prefix(bytes).map(|(value, _len)| value)
}

/// Like [`deserialize_block`], but also returns the number of bytes the decoded
/// value consumed. The spilled-inode reader needs the consumed length to locate
/// the indirect trailer that follows the inode's own bytes inside the same
/// 4096 B block (an inline inode has only zero padding there — no trailer).
pub fn deserialize_block_prefix<T: DeserializeOwned>(
    bytes: &[u8],
) -> Result<(T, usize), CodecError> {
    bincode::serde::decode_from_slice(
        bytes,
        bincode::config::legacy().with_limit::<BLOCK_RECORD_LIMIT>(),
    )
    .map_err(CodecError::Decode)
}
