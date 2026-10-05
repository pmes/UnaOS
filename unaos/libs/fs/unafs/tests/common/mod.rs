//! Shared by the UNAFSCODEC suites (`codec_oracle.rs`, `codec_fuzz.rs`): the
//! seeded record generators and the byte mutator. Moving code here must not
//! change the draw: the oracle digests depend on the exact stream.
#![allow(dead_code)]

use unafs::catalog::CatalogEntry;
use unafs::inode::{AttributeValue, Extent, FileKind, IndirectTrailer, Inode};
use unafs::legacy::LegacySuperblock;
use unafs::superblock::Superblock;
use unafs::{DirEntry, ReclaimEntry, SnapshotEntry};

// ---- seeded generator ------------------------------------------------------

pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    pub fn u64(&mut self) -> u64 {
        match self.below(6) {
            0 => 0,
            1 => u64::MAX,
            2 => self.below(256),
            3 => 1u64 << self.below(64),
            _ => self.next(),
        }
    }
    pub fn u32(&mut self) -> u32 {
        self.u64() as u32
    }
    pub fn f64(&mut self) -> f64 {
        match self.below(9) {
            0 => f64::NAN,
            1 => f64::INFINITY,
            2 => f64::NEG_INFINITY,
            3 => -0.0,
            4 => f64::MIN_POSITIVE / 2.0,
            5 => f64::from_bits(0x7FF8_0000_DEAD_BEEF), // NaN with payload
            _ => f64::from_bits(self.next()),
        }
    }
    pub fn f32(&mut self) -> f32 {
        match self.below(7) {
            0 => f32::NAN,
            1 => f32::INFINITY,
            2 => -0.0,
            3 => f32::from_bits(0x7FC0_BEEF),
            _ => f32::from_bits(self.next() as u32),
        }
    }
    pub fn string(&mut self, max: u64) -> String {
        const POOL: [&str; 10] = ["a", "Z", "0", "_", ".", "é", "\u{2603}", "\u{1F600}", "\0", "/"];
        let n = self.below(max + 1);
        (0..n).map(|_| POOL[self.below(POOL.len() as u64) as usize]).collect()
    }
    pub fn bytes(&mut self, max: u64) -> Vec<u8> {
        let n = self.below(max + 1);
        (0..n).map(|_| self.next() as u8).collect()
    }
}

pub fn kind(r: &mut Rng) -> FileKind {
    [FileKind::File, FileKind::Directory, FileKind::Symlink, FileKind::System][r.below(4) as usize]
}
pub fn extent(r: &mut Rng) -> Extent {
    Extent { logical_offset: r.u64(), physical_block: r.u64(), length: r.u64() }
}
pub fn extents(r: &mut Rng, max: u64) -> Vec<Extent> {
    (0..r.below(max + 1)).map(|_| extent(r)).collect()
}
pub fn attr(r: &mut Rng) -> AttributeValue {
    match r.below(5) {
        0 => AttributeValue::Int(r.u64() as i64),
        1 => AttributeValue::Float(r.f64()),
        2 => AttributeValue::String(r.string(40)),
        3 => AttributeValue::Blob(r.bytes(64)),
        _ => AttributeValue::Vector((0..r.below(20)).map(|_| r.f32()).collect()),
    }
}
pub fn inode(r: &mut Rng) -> Inode {
    let mut i = Inode::new(r.u64(), kind(r));
    i.size = r.u64();
    i.chunks = extents(r, 12);
    for _ in 0..r.below(6) {
        i.attributes.insert(r.string(12), attr(r));
    }
    for _ in 0..r.below(3) {
        i.large_attributes.insert(r.string(12), extents(r, 4));
    }
    // In-RAM-only fields: never part of the record.
    i.parent = r.u64();
    i.name = Some(r.string(8));
    i.ctime = r.u64();
    i.mtime = r.u64();
    i.atime = r.u64();
    i
}
pub fn trailer(r: &mut Rng) -> IndirectTrailer {
    IndirectTrailer {
        magic: if r.below(2) == 0 { unafs::inode::INODE_SPILL_MAGIC } else { r.u64() },
        total_extents: r.u64(),
        overflow_len: r.u64(),
        index: extents(r, 8),
    }
}
pub fn superblock(r: &mut Rng) -> Superblock {
    Superblock {
        magic: if r.below(2) == 0 { *b"UNAFS" } else { [r.next() as u8, 1, 2, 3, r.next() as u8] },
        version: r.u32(),
        block_size: r.u32(),
        block_count: r.u64(),
        root_inode: r.u64(),
        catalog_inode: r.u64(),
    }
}
pub fn legacy_superblock(r: &mut Rng) -> LegacySuperblock {
    LegacySuperblock {
        magic: *b"UNAFS",
        version: r.u32(),
        block_size: r.u32(),
        block_count: r.u64(),
        root_inode: r.u64(),
        free_blocks: r.u64(),
        bitmap_start: r.u64(),
        bitmap_blocks: r.u64(),
        journal_start: r.u64(),
        journal_blocks: r.u64(),
        catalog_inode: r.u64(),
    }
}
pub fn dir_entry(r: &mut Rng) -> DirEntry {
    DirEntry { name: r.string(24), inode_id: r.u64(), kind: kind(r) }
}
pub fn catalog_entry(r: &mut Rng) -> CatalogEntry {
    CatalogEntry { key_hash: r.u64(), val_hash: r.u64(), inode_id: r.u64() }
}
pub fn snapshot_entry(r: &mut Rng) -> SnapshotEntry {
    SnapshotEntry {
        generation: r.u64(),
        imap_block: r.u64(),
        imap_leaves: r.u64(),
        name: r.string(16),
        creator: r.string(16),
        timestamp: r.u64(),
    }
}
pub fn reclaim_entry(r: &mut Rng) -> ReclaimEntry {
    ReclaimEntry { generation: r.u64(), blocks: (0..r.below(10)).map(|_| r.u64()).collect() }
}
pub fn list<T>(r: &mut Rng, max: u64, f: fn(&mut Rng) -> T) -> Vec<T> {
    (0..r.below(max + 1)).map(|_| f(r)).collect()
}

/// Flip, truncate, extend, or overwrite a length/tag with a hostile value.
pub fn mutate(r: &mut Rng, b: &mut Vec<u8>) {
    if b.is_empty() {
        b.push(r.next() as u8);
        return;
    }
    match r.below(5) {
        0 => {
            let i = r.below(b.len() as u64) as usize;
            b[i] ^= 1 << r.below(8);
        }
        1 => {
            let n = r.below(b.len() as u64) as usize;
            b.truncate(n);
        }
        2 => {
            let i = r.below(b.len() as u64) as usize;
            b[i] = r.next() as u8;
        }
        3 => {
            let i = r.below(b.len() as u64) as usize;
            let v = [0u64, u64::MAX, u64::MAX / 2, 1 << 40, 5, 0xFFFF_FFFF][r.below(6) as usize];
            for (k, byte) in v.to_le_bytes().iter().enumerate() {
                if i + k < b.len() {
                    b[i + k] = *byte;
                }
            }
        }
        _ => {
            let n = 1 + r.below(16) as usize;
            for _ in 0..n {
                b.push(r.next() as u8);
            }
        }
    }
}

