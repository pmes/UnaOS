//! FNV-1a Hash Implementation (Stable & Deterministic)
//!
//! Used for the Attribute Catalog to ensure consistent indexing across
//! reboots and architectures.

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

pub struct FnvHasher {
    state: u64,
}

impl FnvHasher {
    pub fn new() -> Self {
        Self {
            state: FNV_OFFSET_BASIS,
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state ^= byte as u64;
            self.state = self.state.wrapping_mul(FNV_PRIME);
        }
    }

    pub fn finish(&self) -> u64 {
        self.state
    }
}

/// Helper function to hash a byte slice using FNV-1a.
pub fn hash_bytes(data: &[u8]) -> u64 {
    let mut hasher = FnvHasher::new();
    hasher.write(data);
    hasher.finish()
}
