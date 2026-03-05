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

pub mod bitmap;
pub mod catalog;
pub mod fs;
pub mod hash;
pub mod inode;
pub mod io;
pub mod query;
pub mod storage;
pub mod superblock;
pub mod wal;

pub use catalog::{CatalogEntry, deserialize_catalog, serialize_catalog};
pub use fs::{DirEntry, UnaFS};
pub use inode::{AttributeValue, Extent, ExtentList, FileKind, Inode, InodeError};
pub use query::{Query, QueryOp, parse_value};
pub use storage::{BLOCK_SIZE, BlockDevice, FileDevice, MemDevice};
pub use superblock::Superblock;
pub use wal::{Journal, JournalOp};

/// The default FileSystem type backed by a host file.
pub type FileSystem = UnaFS<FileDevice>;
