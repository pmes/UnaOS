//! UnaFS: The Holy Grail of storage systems.
//!
//! This library implements a database disguised as a file system, capable of handling
//! massive streams and semantic queries.

pub mod storage;
pub mod inode;

pub use storage::{BlockDevice, MemDevice, BLOCK_SIZE};
pub use inode::{Inode, Extent, ExtentList, AttributeValue, InodeError};
