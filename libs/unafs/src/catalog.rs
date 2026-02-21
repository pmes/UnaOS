use serde::{Deserialize, Serialize};
use crate::inode::AttributeValue;
use crate::hash::{FnvHasher, hash_bytes};

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
        // Use Stable Hashing
        let key_hash = hash_bytes(key.as_bytes());

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
    let mut hasher = FnvHasher::new();
    match value {
        AttributeValue::Int(i) => {
            hasher.write(&i.to_be_bytes());
        }
        AttributeValue::Float(f) => {
            hasher.write(&f.to_be_bytes()); // Use bits? f.to_bits() is unstable for NaN?
            // f64::to_be_bytes() is just bits.
        }
        AttributeValue::String(s) => {
            hasher.write(s.as_bytes());
        }
        AttributeValue::Blob(b) => {
            hasher.write(b);
        }
        AttributeValue::Vector(v) => {
            for f in v {
                hasher.write(&f.to_be_bytes());
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
