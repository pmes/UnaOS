//! UnaFS: The Holy Grail of storage systems.
//!
//! This library implements a database disguised as a file system, capable of handling
//! massive streams and semantic queries.

pub mod bitmap;
pub mod catalog;
pub mod fs;
pub mod hash;
pub mod inode;
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
