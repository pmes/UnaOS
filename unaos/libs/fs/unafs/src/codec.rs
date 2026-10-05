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

//! CHARTER: Kernel — fs-core
//!
//! UNAFSCODEC (SR53): the record codec for everything that reaches disk,
//! written from UnaFS's own byte-level specification
//! (`docs/dev/OS/09_FILESYSTEM/unafs-records.md`, "§R"), with no third-party
//! serializer under it.
//!
//! ## The encoding rules (spec §R1, frozen)
//!
//! | value | bytes |
//! | :--- | :--- |
//! | `u32` / `u64` / `i64` | 4 / 8 / 8 bytes, little-endian, two's complement |
//! | `f32` / `f64` | the IEEE-754 bit pattern as a LE `u32` / `u64` (NaN payloads kept) |
//! | `[u8; N]` | the N bytes, no length |
//! | length | `u64` LE (no varints anywhere in the format) |
//! | `String` | length (byte count) ‖ UTF-8 bytes |
//! | `Vec<T>` / `[T]` | length (element count) ‖ each element |
//! | map | length (pair count) ‖ (key ‖ value) per pair, keys ascending |
//! | enum | variant index as `u32` LE ‖ the variant's payload |
//! | struct | its fields in declaration order, nothing between them |
//!
//! These are exactly the rules the format was frozen under (bincode 1.3.3,
//! then bincode 2.x `legacy()`), so every existing volume reads unchanged —
//! proven by `tests/codec_oracle.rs` (per-record property oracle, digests cut
//! against bincode 2.0.1), `tests/codec_volume.rs` (whole-volume images cut
//! under bincode) and the golden vectors in `tests/kat_vectors.rs`.
//!
//! A record is decoded from a PREFIX of its byte source: trailing bytes are
//! not part of the record (on-disk records live in zero-padded blocks, and a
//! v6 inode is followed by its hand-packed trailers).
//!
//! ## Refusal (spec §R9; BEFS-HARDEN, K3-PARSE-3)
//!
//! The volume is untrusted input. Decoding never panics and never allocates
//! from a claimed length before checking it: a length prefix is refused when
//! it claims more bytes than the record budget allows ([`DecodeErrorKind::
//! LimitExceeded`]) or more than the source still holds
//! ([`DecodeErrorKind::Truncated`]) — before any allocation. Every refusal is
//! a typed [`DecodeError`] naming the record field it stopped at and its byte
//! offset.
//!
//! Two budgets, matching the two record shapes on disk:
//! * [`deserialize_block`] — records that must fit one 4096 B block
//!   (superblock, inode, indirect trailer). Budget: [`BLOCK_RECORD_LIMIT`].
//! * [`deserialize`] — extent-backed records already bounded by the volume
//!   span at the read layer (directory entry lists, the flat attribute
//!   catalog, spilled attribute values, the overflow extent list, the
//!   snapshot index, the reclaim queue). Budget: [`MAX_RECORD_BYTES`].
//!
//! This module is `no_std` (needs only `alloc`).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Decode budget for block-sized records (superblock, inode, indirect
/// trailer): the total bytes one decode may consume. Such a record's encoding
/// must fit one 4096 B block; the budget is doubled for headroom (it is the
/// budget the bincode-era codec carried, kept so no record that decoded then
/// is refused now).
pub const BLOCK_RECORD_LIMIT: usize = 8192;

/// Decode budget for extent-backed records (directory entry lists, the
/// attribute catalog, spilled attribute values, the overflow extent list, the
/// snapshot index, the reclaim queue).
///
/// These records span extents, so they are not block-bounded — the read layer
/// already bounds their byte source against the volume span — but a record
/// still has a hard ceiling so a crafted prefix cannot demand an arbitrary
/// allocation. The ceiling sits WELL BELOW the kernel's guaranteed-free heap
/// (r12 panel: 64 MiB > the 48 MiB kernel heap → abort). 4 MiB is generous
/// for every record the format produces today (~100k dir entries); raise it
/// deliberately (with review) if a legit record ever approaches it. Unlike the
/// bincode-era seam, a passing claim no longer allocates before the bytes are
/// known to be present: a claim must ALSO fit the bytes the source holds.
pub const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;

/// Why a decode stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeErrorKind {
    /// The field needs `need` bytes; the record's source has only `have` left.
    Truncated { need: u64, have: u64 },
    /// Reading the field would take the record past its decode budget.
    LimitExceeded { claimed: u64, limit: u64 },
    /// An enum discriminant names no variant of the type.
    BadVariant(u32),
    /// A string field is not valid UTF-8.
    BadUtf8,
}

/// A typed refusal: the record field the decode stopped at, the byte offset
/// (from the start of the record) where that field begins, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError {
    /// The record type being decoded at the top level (e.g. `"Inode"`,
    /// `"Vec<DirEntry>"`).
    pub record: &'static str,
    /// The field, as `Type.field` (e.g. `"Extent.length"`,
    /// `"Inode.attributes"`, `"AttributeValue.String"`).
    pub field: &'static str,
    /// Byte offset of the field within the record.
    pub offset: usize,
    /// What went wrong.
    pub kind: DecodeErrorKind,
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: field `{}` at byte {}: ", self.record, self.field, self.offset)?;
        match self.kind {
            DecodeErrorKind::Truncated { need, have } => {
                write!(f, "needs {need} bytes, {have} remain")
            }
            DecodeErrorKind::LimitExceeded { claimed, limit } => {
                write!(f, "claims {claimed} bytes past the {limit} B record budget")
            }
            DecodeErrorKind::BadVariant(v) => write!(f, "no variant {v}"),
            DecodeErrorKind::BadUtf8 => write!(f, "not UTF-8"),
        }
    }
}

/// Error raised by the codec seam. Encoding is infallible (a record is
/// always representable); only decoding refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// Bytes could not be decoded into the target record.
    Decode(DecodeError),
}

impl CodecError {
    /// The decode refusal, field and all.
    pub fn decode_error(&self) -> &DecodeError {
        match self {
            CodecError::Decode(e) => e,
        }
    }
}

impl core::fmt::Display for CodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CodecError::Decode(e) => write!(f, "decode error: {e}"),
        }
    }
}

// `core::error::Error` is re-exported as `std::error::Error` under std, so this
// single impl satisfies thiserror's `#[from]` in both std and no_std builds.
impl core::error::Error for CodecError {}

impl From<DecodeError> for CodecError {
    fn from(e: DecodeError) -> Self {
        CodecError::Decode(e)
    }
}

// ---------------------------------------------------------------------------
// Writer / Reader
// ---------------------------------------------------------------------------

/// Appends spec §R1 primitives to a byte buffer.
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    pub fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
    /// A fixed-size byte array: the bytes, no length.
    pub fn raw(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
    /// A length prefix (element / byte / pair count).
    pub fn len(&mut self, n: usize) {
        self.u64(n as u64);
    }
    pub fn str(&mut self, s: &str) {
        self.len(s.len());
        self.raw(s.as_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.len(b.len());
        self.raw(b);
    }
    pub fn seq<T: Encode>(&mut self, items: &[T]) {
        self.len(items.len());
        for it in items {
            it.encode(self);
        }
    }
    pub fn map<V: Encode>(&mut self, m: &BTreeMap<String, V>) {
        self.len(m.len());
        for (k, v) in m {
            self.str(k);
            v.encode(self);
        }
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads spec §R1 primitives from a record's byte source under a budget.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    limit: usize,
    record: &'static str,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8], limit: usize, record: &'static str) -> Self {
        Self { buf, pos: 0, limit, record }
    }

    /// Bytes consumed so far (the record's length once decode returns).
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Bytes the record may still consume: the smaller of the source's
    /// remainder and the budget's remainder.
    pub fn remaining(&self) -> usize {
        (self.buf.len() - self.pos).min(self.limit.saturating_sub(self.pos))
    }

    pub fn error(&self, field: &'static str, kind: DecodeErrorKind) -> DecodeError {
        DecodeError { record: self.record, field, offset: self.pos, kind }
    }

    /// Check a claim of `n` bytes (budget first, then the source) without
    /// consuming. Never allocates.
    fn claim(&self, field: &'static str, n: u64) -> Result<usize, DecodeError> {
        let budget_left = self.limit.saturating_sub(self.pos) as u64;
        if n > budget_left {
            return Err(self.error(
                field,
                DecodeErrorKind::LimitExceeded {
                    claimed: n,
                    limit: self.limit as u64,
                },
            ));
        }
        let have = (self.buf.len() - self.pos) as u64;
        if n > have {
            return Err(self.error(field, DecodeErrorKind::Truncated { need: n, have }));
        }
        Ok(n as usize)
    }

    fn take(&mut self, field: &'static str, n: u64) -> Result<&'a [u8], DecodeError> {
        let n = self.claim(field, n)?;
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn array<const N: usize>(&mut self, field: &'static str) -> Result<[u8; N], DecodeError> {
        let s = self.take(field, N as u64)?;
        let mut out = [0u8; N];
        out.copy_from_slice(s);
        Ok(out)
    }
    pub fn u32(&mut self, field: &'static str) -> Result<u32, DecodeError> {
        self.array::<4>(field).map(u32::from_le_bytes)
    }
    pub fn u64(&mut self, field: &'static str) -> Result<u64, DecodeError> {
        self.array::<8>(field).map(u64::from_le_bytes)
    }
    pub fn i64(&mut self, field: &'static str) -> Result<i64, DecodeError> {
        self.array::<8>(field).map(i64::from_le_bytes)
    }
    pub fn f32(&mut self, field: &'static str) -> Result<f32, DecodeError> {
        self.u32(field).map(f32::from_bits)
    }
    pub fn f64(&mut self, field: &'static str) -> Result<f64, DecodeError> {
        self.u64(field).map(f64::from_bits)
    }

    /// An enum discriminant (`u32`), checked against the variant count.
    pub fn variant(&mut self, field: &'static str, count: u32) -> Result<u32, DecodeError> {
        let at = self.pos;
        let v = self.u32(field)?;
        if v >= count {
            return Err(DecodeError {
                record: self.record,
                field,
                offset: at,
                kind: DecodeErrorKind::BadVariant(v),
            });
        }
        Ok(v)
    }

    /// A length prefix for `n` elements whose encodings are each at least
    /// `min_elem` bytes. The claim (`n * min_elem`) must fit the budget and
    /// the source BEFORE anything is allocated; the prefix itself is
    /// consumed only when it does.
    pub fn len(&mut self, field: &'static str, min_elem: usize) -> Result<usize, DecodeError> {
        let at = self.pos;
        let n = self.u64(field)?;
        let bytes = n.saturating_mul(min_elem as u64);
        if let Err(mut e) = self.claim(field, bytes) {
            e.offset = at;
            self.pos = at;
            return Err(e);
        }
        Ok(n as usize)
    }

    pub fn bytes(&mut self, field: &'static str) -> Result<Vec<u8>, DecodeError> {
        let n = self.len(field, 1)?;
        Ok(self.take(field, n as u64)?.to_vec())
    }

    pub fn string(&mut self, field: &'static str) -> Result<String, DecodeError> {
        let at = self.pos;
        let n = self.len(field, 1)?;
        let s = self.take(field, n as u64)?;
        match core::str::from_utf8(s) {
            Ok(s) => Ok(String::from(s)),
            Err(_) => Err(DecodeError {
                record: self.record,
                field,
                offset: at,
                kind: DecodeErrorKind::BadUtf8,
            }),
        }
    }

    pub fn seq<T: Decode>(&mut self, field: &'static str) -> Result<Vec<T>, DecodeError> {
        let n = self.len(field, T::MIN_LEN)?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(T::decode(self)?);
        }
        Ok(out)
    }

    /// A string-keyed map. Pairs are inserted in stored order; a repeated key
    /// keeps its LAST value (the rule every volume was written and read
    /// under — the writer never repeats a key).
    pub fn map<V: Decode>(&mut self, field: &'static str) -> Result<BTreeMap<String, V>, DecodeError> {
        let n = self.len(field, 8 + V::MIN_LEN)?;
        let mut out = BTreeMap::new();
        for _ in 0..n {
            let k = self.string(field)?;
            let v = V::decode(self)?;
            out.insert(k, v);
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// A value with a spec §R encoding.
pub trait Encode {
    fn encode(&self, w: &mut Writer);
}

/// A value decodable from its spec §R encoding.
pub trait Decode: Sized {
    /// The record name errors carry when this type is decoded at top level.
    const NAME: &'static str;
    /// The smallest possible encoding, in bytes (bounds a length prefix's
    /// claim before allocation).
    const MIN_LEN: usize;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError>;
}

impl<T: Encode> Encode for [T] {
    fn encode(&self, w: &mut Writer) {
        w.seq(self);
    }
}

impl<T: Encode> Encode for Vec<T> {
    fn encode(&self, w: &mut Writer) {
        w.seq(self);
    }
}

impl<T: Encode + ?Sized> Encode for &T {
    fn encode(&self, w: &mut Writer) {
        (**self).encode(w);
    }
}

impl Encode for u64 {
    fn encode(&self, w: &mut Writer) {
        w.u64(*self);
    }
}

impl Decode for u64 {
    const NAME: &'static str = "u64";
    const MIN_LEN: usize = 8;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.u64("u64")
    }
}

/// Top-level lists (directory entry lists, the flat catalog, the overflow
/// extent list, the snapshot index, the reclaim queue) are `Vec<T>`.
impl<T: Decode + ListName> Decode for Vec<T> {
    const NAME: &'static str = T::LIST_NAME;
    const MIN_LEN: usize = 8;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.seq(T::LIST_FIELD)
    }
}

/// The record name and length-field name of a top-level `Vec<T>`.
pub trait ListName {
    const LIST_NAME: &'static str;
    const LIST_FIELD: &'static str;
}

/// Implements [`ListName`] for record types that are stored as lists.
macro_rules! list_name {
    ($($t:ty => $name:literal),* $(,)?) => {
        $(impl $crate::codec::ListName for $t {
            const LIST_NAME: &'static str = concat!("Vec<", $name, ">");
            const LIST_FIELD: &'static str = concat!("Vec<", $name, ">.len");
        })*
    };
}
pub(crate) use list_name;

impl ListName for u64 {
    const LIST_NAME: &'static str = "Vec<u64>";
    const LIST_FIELD: &'static str = "Vec<u64>.len";
}

// ---------------------------------------------------------------------------
// The seam
// ---------------------------------------------------------------------------

/// Encode a record into its spec §R bytes.
///
/// Accepts unsized `T` (e.g. slices) so `serialize(&entries[..])` encodes a list.
/// Infallible in practice; the `Result` keeps the seam's call sites uniform.
pub fn serialize<T: Encode + ?Sized>(value: &T) -> Result<Vec<u8>, CodecError> {
    let mut w = Writer::new();
    value.encode(&mut w);
    Ok(w.into_bytes())
}

/// The encoded length of a record (no allocation beyond the encode itself).
pub fn encoded_len<T: Encode + ?Sized>(value: &T) -> usize {
    let mut w = Writer::new();
    value.encode(&mut w);
    w.buf.len()
}

/// Decode an extent-backed record from the head of `bytes` (trailing bytes
/// ignored), under the [`MAX_RECORD_BYTES`] budget.
pub fn deserialize<T: Decode>(bytes: &[u8]) -> Result<T, CodecError> {
    decode_prefix(bytes, MAX_RECORD_BYTES).map(|(v, _)| v)
}

/// Decode a record that must fit a single 4096 B block (superblock, inode,
/// indirect trailer), under the tight [`BLOCK_RECORD_LIMIT`] budget.
pub fn deserialize_block<T: Decode>(bytes: &[u8]) -> Result<T, CodecError> {
    deserialize_block_prefix(bytes).map(|(value, _len)| value)
}

/// Like [`deserialize_block`], but also returns the number of bytes the record
/// consumed. The inode reader needs it to locate the trailers that follow the
/// inode's own bytes inside the same 4096 B block.
pub fn deserialize_block_prefix<T: Decode>(bytes: &[u8]) -> Result<(T, usize), CodecError> {
    decode_prefix(bytes, BLOCK_RECORD_LIMIT)
}

/// Decode `T` from the head of `bytes` under an explicit budget.
pub fn decode_prefix<T: Decode>(bytes: &[u8], limit: usize) -> Result<(T, usize), CodecError> {
    let mut r = Reader::new(bytes, limit, T::NAME);
    let v = T::decode(&mut r)?;
    Ok((v, r.position()))
}
