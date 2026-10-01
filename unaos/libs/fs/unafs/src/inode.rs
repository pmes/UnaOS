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

use crate::storage::BLOCK_SIZE;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Error types related to Inode operations.
#[derive(Error, Debug)]
pub enum InodeError {
    #[error("Inode too large: {0} bytes (max {1})")]
    InodeTooLarge(usize, u64),
    #[error("Serialization error: {0}")]
    Serialization(#[from] crate::codec::CodecError),
    #[error("Malformed inode meta trailer: {0}")]
    BadMeta(&'static str),
}

/// The type of file represented by an Inode.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, PartialOrd, Copy)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    /// A system-internal file (e.g., Attribute Catalog).
    System,
}

/// Represents a contiguous chunk of data on the disk.
///
/// Extents allow for efficient storage of large files by mapping logical offsets
/// to physical blocks and lengths.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Extent {
    /// The logical offset within the file where this extent begins.
    pub logical_offset: u64,
    /// The starting physical block ID on the device.
    pub physical_block: u64,
    /// The length of the extent in bytes.
    pub length: u64,
}

/// A list of extents defining the data layout of a file.
pub type ExtentList = Vec<Extent>;

/// Serialized byte cost of one [`Extent`] under the frozen codec (three
/// fixed-width little-endian `u64`s). Pinned by the KAT vectors; used to size
/// the inline extent budget when an inode spills.
pub const EXTENT_ENC_LEN: usize = 24;

/// Magic tag opening a spilled inode's indirect trailer. Non-zero and
/// distinctive, so it can never collide with the zero padding that follows a
/// plain (inline) inode inside its 4096 B block — that is what lets the reader
/// tell "there is a trailer here" from "this is just padding".
pub const INODE_SPILL_MAGIC: u64 = 0x554E_4146_5358_5054; // "UNAFSXPT"

/// Bytes reserved at the tail of a spilled inode's block for the indirect
/// trailer (magic + counts + the small index extent list). When an inode's full
/// extent list will not fit inline, the leading extents are kept inline up to
/// `BLOCK_SIZE - SPILL_TRAILER_RESERVE`, and the overflow spills to indirect
/// blocks described by the trailer. The index is itself extent-coalesced, so in
/// the common case (indirect blocks allocated contiguously) it is a handful of
/// extents that fits this reserve with room to spare.
pub const SPILL_TRAILER_RESERVE: usize = 1024;

/// The indirect trailer of a SPILLED inode, serialized immediately after the
/// inode's own bytes inside its 4096 B block.
///
/// A file whose extent list fits inline has NO trailer — its inode block is
/// byte-identical to the pre-indirection format. When the list overflows, the
/// leading extents stay in [`Inode::chunks`] and the remainder is serialized as
/// an [`ExtentList`] into freshly allocated INDIRECT blocks; `index` maps those
/// blocks (coalesced) and `overflow_len` is the exact serialized length to read
/// back. On read the overflow is decoded and appended to `chunks`, so every
/// consumer above the inode layer sees one complete extent list — the split is
/// invisible.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct IndirectTrailer {
    /// Always [`INODE_SPILL_MAGIC`] — the presence discriminator.
    pub magic: u64,
    /// Total extents in the FULL list (inline + overflow); a corruption check.
    pub total_extents: u64,
    /// Byte length of the serialized overflow [`ExtentList`] stored in `index`.
    pub overflow_len: u64,
    /// Extents mapping the serialized overflow bytes (the indirect blocks).
    pub index: ExtentList,
}

/// The value of a metadata attribute attached to an Inode.
///
/// Supports various primitives including Vectors for AI embeddings.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum AttributeValue {
    /// A 64-bit signed integer.
    Int(i64),
    /// A 64-bit floating point number.
    Float(f64),
    /// A UTF-8 string.
    String(String),
    /// A binary blob (e.g., thumbnail).
    Blob(Vec<u8>),
    /// A vector of 32-bit floats (e.g., AI embedding).
    /// Used for small vectors that fit in the Inode.
    /// Larger vectors are stored in `large_attributes`.
    Vector(Vec<f32>),
}

/// The atomic unit of metadata in UnaFS.
///
/// An Inode represents a file or directory and contains its metadata and data mapping.
/// It is designed to fit within a single block when serialized.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Inode {
    /// Unique identifier for the Inode.
    pub id: u64,
    /// The type of file (File, Directory, Symlink, System).
    pub kind: FileKind,
    /// The logical size of the file data in bytes.
    pub size: u64,
    /// List of data extents.
    pub chunks: ExtentList,
    /// Key-value map of small semantic attributes.
    pub attributes: BTreeMap<String, AttributeValue>,
    /// Key-value map of large attributes stored in external blocks.
    /// Used for large vectors or blobs (> 256 bytes).
    pub large_attributes: BTreeMap<String, ExtentList>,
    /// v6 META TRAILER fields (B302 M3/M4). `serde(skip)`: the bincode
    /// encoding above is unchanged (every inode golden holds); on a v6 volume
    /// these ride the hand-packed trailer after the inode's bytes
    /// ([`INODE_META_MAGIC`]), on a v3–v5 volume they are never written and
    /// read back as zero/`None`.
    ///
    /// The parent directory's logical id (0 = no parent: the root, the system
    /// inodes, a bare `create_inode`). UnaFS has no hard links, so one parent
    /// is the whole truth.
    #[serde(skip)]
    pub parent: u64,
    /// This inode's name in `parent` (`None` when unnamed or longer than
    /// [`INODE_META_NAME_MAX`] — the path walk then asks the parent's listing).
    #[serde(skip)]
    pub name: Option<String>,
    /// Unix seconds of the last metadata change (create, attribute, rename).
    #[serde(skip)]
    pub ctime: u64,
    /// Unix seconds of the last data change (create, write, directory edit).
    #[serde(skip)]
    pub mtime: u64,
    /// Unix seconds of the last access WRITE (noatime: stamped at create and
    /// write only — under CoW a read that wrote would cost a commit).
    #[serde(skip)]
    pub atime: u64,
}

/// Magic tag opening a v6 inode's META TRAILER ("UNAFSMT1", little-endian
/// u64). Like [`INODE_SPILL_MAGIC`] it can never collide with the zero
/// padding after a plain inode, so presence is self-describing.
pub const INODE_META_MAGIC: u64 = u64::from_le_bytes(*b"UNAFSMT1");
/// Fixed bytes of the meta trailer before the name: magic, parent, ctime,
/// mtime, atime (u64 each) + name length (u16).
pub const INODE_META_FIXED: usize = 42;
/// Longest name the trailer stores inline; longer names store `0xFFFF`.
pub const INODE_META_NAME_MAX: usize = 255;
/// The name-length sentinel for "name not stored".
pub const INODE_META_NO_NAME: u16 = 0xFFFF;

impl Inode {
    /// Create a new Inode with the given ID and default File kind.
    pub fn new(id: u64, kind: FileKind) -> Self {
        Self {
            id,
            kind,
            size: 0,
            chunks: Vec::new(),
            attributes: BTreeMap::new(),
            large_attributes: BTreeMap::new(),
            parent: 0,
            name: None,
            ctime: 0,
            mtime: 0,
            atime: 0,
        }
    }

    /// The hand-packed v6 META TRAILER (little-endian):
    ///
    /// ```text
    ///   off len  field
    ///     0   8  magic "UNAFSMT1"
    ///     8   8  parent (logical id)
    ///    16   8  ctime   24 8 mtime   32 8 atime   (unix seconds)
    ///    40   2  name_len (0xFFFF = not stored)
    ///    42   n  name bytes (UTF-8, n ≤ 255)
    /// ```
    pub fn meta_bytes(&self) -> Vec<u8> {
        let name = self
            .name
            .as_deref()
            .filter(|n| n.len() <= INODE_META_NAME_MAX);
        let mut out = Vec::with_capacity(INODE_META_FIXED + name.map_or(0, |n| n.len()));
        out.extend_from_slice(&INODE_META_MAGIC.to_le_bytes());
        out.extend_from_slice(&self.parent.to_le_bytes());
        out.extend_from_slice(&self.ctime.to_le_bytes());
        out.extend_from_slice(&self.mtime.to_le_bytes());
        out.extend_from_slice(&self.atime.to_le_bytes());
        match name {
            Some(n) => {
                out.extend_from_slice(&(n.len() as u16).to_le_bytes());
                out.extend_from_slice(n.as_bytes());
            }
            None => out.extend_from_slice(&INODE_META_NO_NAME.to_le_bytes()),
        }
        out
    }

    /// Parse a meta trailer at the head of `bytes` into this inode's meta
    /// fields. Returns the trailer's length, `Ok(None)` when no trailer is
    /// present (the magic is absent), or an error when the magic is present
    /// but the trailer is malformed (bounded: the name length is checked
    /// against both the cap and the bytes available).
    pub fn apply_meta(&mut self, bytes: &[u8]) -> Result<Option<usize>, InodeError> {
        if bytes.len() < 8 || u64::from_le_bytes(bytes[0..8].try_into().unwrap()) != INODE_META_MAGIC {
            return Ok(None);
        }
        if bytes.len() < INODE_META_FIXED {
            return Err(InodeError::BadMeta("truncated meta trailer"));
        }
        let rd = |o: usize| u64::from_le_bytes(bytes[o..o + 8].try_into().unwrap());
        let parent = rd(8);
        let (ctime, mtime, atime) = (rd(16), rd(24), rd(32));
        let name_len = u16::from_le_bytes([bytes[40], bytes[41]]);
        let (name, len) = if name_len == INODE_META_NO_NAME {
            (None, INODE_META_FIXED)
        } else {
            let n = name_len as usize;
            if n > INODE_META_NAME_MAX || bytes.len() < INODE_META_FIXED + n {
                return Err(InodeError::BadMeta("meta name length out of range"));
            }
            let s = core::str::from_utf8(&bytes[INODE_META_FIXED..INODE_META_FIXED + n])
                .map_err(|_| InodeError::BadMeta("meta name not UTF-8"))?;
            (Some(String::from(s)), INODE_META_FIXED + n)
        };
        self.parent = parent;
        self.name = name;
        self.ctime = ctime;
        self.mtime = mtime;
        self.atime = atime;
        Ok(Some(len))
    }

    /// Serializes the Inode to bytes, ensuring it fits within a block.
    pub fn to_bytes(&self) -> Result<Vec<u8>, InodeError> {
        let bytes = crate::codec::serialize(self)?;
        if bytes.len() as u64 > BLOCK_SIZE {
            return Err(InodeError::InodeTooLarge(bytes.len(), BLOCK_SIZE));
        }
        Ok(bytes)
    }

    /// Deserializes an Inode from bytes.
    ///
    /// An inode is a block-sized record, so this uses the block-budgeted
    /// decode: a crafted length prefix inside the block fails instead of
    /// pre-allocating (BEFS-HARDEN, K3-PARSE-3).
    ///
    /// This decodes only the INLINE part of the record; a spilled inode's
    /// [`chunks`](Self::chunks) here holds just the leading extents. Callers
    /// that need the full extent list use [`decode_block`](Self::decode_block)
    /// and reconstruct the overflow. Pre-indirection callers (the v1 migration
    /// reader) only ever meet inline inodes, so this stays correct for them.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, InodeError> {
        let (mut inode, len): (Self, usize) = crate::codec::deserialize_block_prefix(bytes)?;
        if bytes.len() > len {
            inode.apply_meta(&bytes[len..])?;
        }
        Ok(inode)
    }

    /// Decode an inode block into its inline inode and, if present, the
    /// indirect trailer that follows it.
    ///
    /// The inline inode is decoded first; its consumed length locates the
    /// trailer. A trailer is present iff the eight bytes at that offset spell
    /// [`INODE_SPILL_MAGIC`] — an inline inode leaves zero padding there, which
    /// can never match. The overflow extents themselves are NOT read here (that
    /// needs the device); the caller uses `trailer.index`/`overflow_len` to read
    /// them back and append them to `chunks`.
    ///
    /// A v6 block carries the META TRAILER first (at the inline inode's end);
    /// the spill trailer, if any, follows it.
    pub fn decode_block(bytes: &[u8]) -> Result<(Self, Option<IndirectTrailer>), InodeError> {
        let (mut inode, mut len): (Self, usize) = crate::codec::deserialize_block_prefix(bytes)?;
        if bytes.len() > len
            && let Some(m) = inode.apply_meta(&bytes[len..])?
        {
            len += m;
        }
        if bytes.len() >= len + 8 {
            let tag = u64::from_le_bytes(bytes[len..len + 8].try_into().unwrap());
            if tag == INODE_SPILL_MAGIC {
                let trailer: IndirectTrailer = crate::codec::deserialize_block(&bytes[len..])?;
                return Ok((inode, Some(trailer)));
            }
        }
        Ok((inode, None))
    }

    /// The number of leading extents that stay inline when this inode spills:
    /// the largest `k` such that the inode with `chunks[0..k]` plus the reserved
    /// trailer budget fits one block. Each [`Extent`] costs a fixed
    /// [`EXTENT_ENC_LEN`] bytes, so this is exact arithmetic, not a search.
    pub fn inline_extent_count(&self) -> Result<usize, InodeError> {
        self.inline_extent_count_with(0)
    }

    /// [`inline_extent_count`](Self::inline_extent_count) with `extra` more
    /// bytes reserved inline (the v6 meta trailer rides between the inode and
    /// the spill trailer).
    pub fn inline_extent_count_with(&self, extra: usize) -> Result<usize, InodeError> {
        let mut stub = self.clone();
        stub.chunks.clear();
        let fixed = crate::codec::serialize(&stub)?.len().saturating_add(extra);
        let budget = (BLOCK_SIZE as usize)
            .saturating_sub(SPILL_TRAILER_RESERVE)
            .saturating_sub(fixed);
        Ok(budget / EXTENT_ENC_LEN)
    }
}
