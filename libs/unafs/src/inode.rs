use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use crate::storage::BLOCK_SIZE;
use thiserror::Error;

/// Error types related to Inode operations.
#[derive(Error, Debug)]
pub enum InodeError {
    #[error("Inode too large: {0} bytes (max {1})")]
    InodeTooLarge(usize, u64),
    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),
}

/// The type of file represented by an Inode.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, PartialOrd, Copy)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
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
    /// The type of file (File, Directory, Symlink).
    pub kind: FileKind,
    /// The logical size of the file data in bytes.
    pub size: u64,
    /// List of data extents.
    pub chunks: ExtentList,
    /// Key-value map of semantic attributes.
    pub attributes: BTreeMap<String, AttributeValue>,
}

impl Inode {
    /// Create a new Inode with the given ID and default File kind.
    pub fn new(id: u64, kind: FileKind) -> Self {
        Self {
            id,
            kind,
            size: 0,
            chunks: Vec::new(),
            attributes: BTreeMap::new(),
        }
    }

    /// Serializes the Inode to bytes, ensuring it fits within a block.
    pub fn to_bytes(&self) -> Result<Vec<u8>, InodeError> {
        let bytes = bincode::serialize(self)?;
        if bytes.len() as u64 > BLOCK_SIZE {
            return Err(InodeError::InodeTooLarge(bytes.len(), BLOCK_SIZE));
        }
        Ok(bytes)
    }

    /// Deserializes an Inode from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, InodeError> {
        let inode = bincode::deserialize(bytes)?;
        Ok(inode)
    }
}
