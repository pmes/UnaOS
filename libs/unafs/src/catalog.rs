use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use crate::inode::AttributeValue;

/// An entry in the Attribute Catalog.
/// Maps a (Key, Value) pair to an Inode ID.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Copy)]
pub struct CatalogEntry {
    pub key_hash: u64,
    pub val_hash: u64,
    pub inode_id: u64,
}

impl CatalogEntry {
    pub fn new(key: &str, value: &AttributeValue, inode_id: u64) -> Self {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let key_hash = hasher.finish();

        let val_hash = hash_value(value);

        Self {
            key_hash,
            val_hash,
            inode_id,
        }
    }
}

/// Helper to hash an AttributeValue.
pub fn hash_value(value: &AttributeValue) -> u64 {
    let mut hasher = DefaultHasher::new();
    match value {
        AttributeValue::Int(i) => i.hash(&mut hasher),
        AttributeValue::Float(f) => f.to_bits().hash(&mut hasher),
        AttributeValue::String(s) => s.hash(&mut hasher),
        AttributeValue::Blob(b) => b.hash(&mut hasher),
        AttributeValue::Vector(v) => {
            for f in v {
                f.to_bits().hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

/// Helper to serialize a list of catalog entries.
pub fn serialize_catalog(entries: &[CatalogEntry]) -> Result<Vec<u8>, bincode::Error> {
    bincode::serialize(entries)
}

/// Helper to deserialize a list of catalog entries.
pub fn deserialize_catalog(data: &[u8]) -> Result<Vec<CatalogEntry>, bincode::Error> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    bincode::deserialize(data)
}
