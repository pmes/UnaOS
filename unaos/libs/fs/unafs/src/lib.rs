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

//! UnaFS: The Holy Grail of storage systems.
//!
//! This library implements a database disguised as a file system, capable of handling
//! massive streams and semantic queries.
//!
//! # no_std
//!
//! The crate is `#![no_std]` + `alloc` by default-off: the on-disk format types,
//! the [`codec`] serialization seam, the [`storage::BlockDevice`] trait, the
//! in-memory [`storage::MemDevice`], and the semantic query engine
//! ([`Query`] parsing, [`UnaFS::query`], and [`cosine_similarity`] — FP `sqrt`
//! routed through `libm`) compile without `std`. The host-native backends —
//! the `FileDevice` file backend, the [`io`] memory-map reader, and the bandy
//! event bus — live behind the default-on `std` feature.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg_attr(not(feature = "std"), macro_use)]
extern crate alloc;

pub mod adapter;
pub mod catalog;
pub mod codec;
pub mod fs;
pub mod fsck;
pub mod hash;
pub mod inode;
#[cfg(feature = "std")]
pub mod io;
pub mod legacy;
pub mod query;
pub mod refmap;
pub mod root;
pub mod storage;
pub mod superblock;
pub mod warnlog;

pub use adapter::{
    BlockAdapter, MemSectorDevice, Partition, PartitionScheme, PartitionSpan, PartitionTable,
    SECTOR_SIZE, SECTORS_PER_BLOCK, SectorDevice, SectorError, locate_unafs, parse_partitions,
};
pub use catalog::{CatalogEntry, deserialize_catalog, serialize_catalog};
pub use fs::{
    BatchFile, CommitStats, DirEntry, ReclaimEntry, SNAPSHOT_CAP, SnapshotEntry, UnaFS,
    cosine_similarity,
};
pub use fsck::FsckReport;
pub use inode::{AttributeValue, Extent, ExtentList, FileKind, Inode, InodeError};
pub use query::{Query, QueryOp, parse_value};
pub use root::{ROOT_RECORD_SIZE, RootRecord, RootSlot};
#[cfg(feature = "std")]
pub use storage::FileDevice;
pub use storage::{BLOCK_SIZE, BlockDevice, MemDevice};
pub use superblock::Superblock;

/// The default FileSystem type backed by a host file.
#[cfg(feature = "std")]
pub type FileSystem = UnaFS<FileDevice>;
