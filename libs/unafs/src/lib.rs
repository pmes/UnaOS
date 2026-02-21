//! UnaFS: The Holy Grail of storage systems.
//!
//! This library implements a database disguised as a file system, capable of handling
//! massive streams and semantic queries.

pub mod storage;
pub mod inode;
pub mod superblock;
pub mod bitmap;
pub mod fs;
pub mod wal;
pub mod catalog;
pub mod query;
pub mod hash;

pub use storage::{BlockDevice, FileDevice, MemDevice, BLOCK_SIZE};
pub use inode::{Inode, Extent, ExtentList, AttributeValue, InodeError, FileKind};
pub use superblock::Superblock;
pub use fs::{UnaFS, DirEntry};
pub use wal::{Journal, JournalOp};
pub use catalog::{CatalogEntry, serialize_catalog, deserialize_catalog};
pub use query::{Query, QueryOp, parse_value};

/// The default FileSystem type backed by a host file.
pub type FileSystem = UnaFS<FileDevice>;
