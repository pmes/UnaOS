//! UnaFS: The Holy Grail of storage systems.
//!
//! This library implements a database disguised as a file system, capable of handling
//! massive streams and semantic queries.

pub mod storage;
pub mod inode;
pub mod superblock;
pub mod bitmap;
pub mod fs;

pub use storage::{BlockDevice, FileDevice, MemDevice, BLOCK_SIZE};
pub use inode::{Inode, Extent, ExtentList, AttributeValue, InodeError, FileKind};
pub use superblock::Superblock;
pub use fs::{UnaFS, DirEntry};
