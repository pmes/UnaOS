//! UNAFSCODEC (SR53): the per-record PROPERTY ORACLE for the hand codec.
//!
//! For every record type that reaches disk, a seeded generator draws a
//! property set (boundary values first, then random values: empty/huge
//! integers, NaN/±inf/−0.0/subnormal floats, empty and multi-byte UTF-8
//! strings, empty and long lists, multi-key maps) and the hand codec's bytes
//! are held to the oracle:
//!
//! * M1 (this file at the M1 commit): byte-for-byte against `bincode 2.0.1`
//!   `legacy()` — the encoder the format was frozen under — and the decoders
//!   against each other on mutated bytes;
//! * always: a digest (FNV-1a 64 over `len ‖ bytes` of every record in the
//!   set) cut from bincode's output at M1 and pinned here, so the same
//!   property set keeps proving byte equality after bincode leaves the crate
//!   (M2), plus decode(encode(v)) re-encodes to the same bytes.

use unafs::catalog::CatalogEntry;
use unafs::codec::{Decode, Encode};
use unafs::inode::{AttributeValue, Extent, FileKind, IndirectTrailer, Inode};
use unafs::legacy::LegacySuperblock;
use unafs::superblock::Superblock;
use unafs::{DirEntry, ReclaimEntry, SnapshotEntry};

/// Records drawn per type.
const N: usize = 2000;

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

// ---- the oracle -------------------------------------------------------------

fn digest_push(h: &mut unafs::hash::FnvHasher, bytes: &[u8]) {
    h.write(&(bytes.len() as u64).to_le_bytes());
    h.write(bytes);
}

/// The reference encoder (bincode 2.0.1 `legacy()`, the format's frozen
/// encoder) — present only while bincode is still in the tree (M1).
fn reference<T: serde::Serialize + ?Sized>(v: &T) -> Vec<u8> {
    bincode::serde::encode_to_vec(v, bincode::config::legacy()).unwrap()
}

/// Draw `N` values of one record type, hold each to the oracle, return the
/// digest of the reference bytes.
fn run<T>(seed: u64, gen_: fn(&mut Rng) -> T) -> u64
where
    T: Encode + Decode + serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
{
    let mut r = Rng::new(seed);
    let mut h = unafs::hash::FnvHasher::new();
    for i in 0..N {
        let v = gen_(&mut r);
        let ours = unafs::codec::serialize(&v).unwrap();
        let theirs = reference(&v);
        assert_eq!(ours, theirs, "{} #{i}: bytes differ from bincode for {v:?}", T::NAME);
        digest_push(&mut h, &theirs);
        // decode(encode(v)) re-encodes to the same bytes (NaN-safe equality).
        let (back, used) = unafs::codec::decode_prefix::<T>(&ours, usize::MAX).unwrap();
        assert_eq!(used, ours.len(), "{} #{i}: decode consumed {used} of {}", T::NAME, ours.len());
        assert_eq!(unafs::codec::serialize(&back).unwrap(), ours, "{} #{i}: round trip", T::NAME);
        // M1 decoder oracle: on mutated bytes, both decoders agree on
        // accept/refuse, and on the re-encoded value when both accept.
        for m in 0..4 {
            let mut bad = ours.clone();
            mutate(&mut r, &mut bad);
            let a = unafs::codec::deserialize::<T>(&bad);
            let b: Result<(T, usize), _> = bincode::serde::decode_from_slice(
                &bad,
                bincode::config::legacy().with_limit::<{ unafs::codec::MAX_RECORD_BYTES }>(),
            );
            match (a, b) {
                (Ok(x), Ok((y, _))) => assert_eq!(
                    unafs::codec::serialize(&x).unwrap(),
                    reference(&y),
                    "{} #{i}/{m}: decoders disagree on value",
                    T::NAME
                ),
                (Err(_), Err(_)) => {}
                (x, y) => panic!("{} #{i}/{m}: accept/refuse differ: ours {x:?} bincode {y:?}", T::NAME),
            }
        }
    }
    h.finish()
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

macro_rules! oracle {
    ($name:ident, $seed:expr, $gen:expr, $ty:ty, $digest:expr) => {
        #[test]
        fn $name() {
            let d = run::<$ty>($seed, $gen);
            println!("{}: {:#018x}", stringify!($name), d);
            assert_eq!(d, $digest, "digest of the bincode-era bytes for {}", stringify!($ty));
        }
    };
}

oracle!(superblock_matches_bincode, 1, superblock, Superblock, 0x2d6185c3506dfb3f);
oracle!(legacy_superblock_matches_bincode, 2, legacy_superblock, LegacySuperblock, 0x7d0325a8c0565830);
oracle!(file_kind_matches_bincode, 3, kind, FileKind, 0x44d1b6e239bc5005);
oracle!(extent_matches_bincode, 4, extent, Extent, 0xd6af5894b2e7fbd5);
oracle!(attribute_value_matches_bincode, 5, attr, AttributeValue, 0xdff0b5f21a0695b1);
oracle!(inode_matches_bincode, 6, inode, Inode, 0xa62e2cb43dd67b3a);
oracle!(indirect_trailer_matches_bincode, 7, trailer, IndirectTrailer, 0x8ceb24750528782a);
oracle!(dir_entry_list_matches_bincode, 8, |r| list(r, 8, dir_entry), Vec<DirEntry>, 0x3a316424737be34f);
oracle!(catalog_list_matches_bincode, 9, |r| list(r, 8, catalog_entry), Vec<CatalogEntry>, 0xc60de683e52fcd6c);
oracle!(snapshot_list_matches_bincode, 10, |r| list(r, 6, snapshot_entry), Vec<SnapshotEntry>, 0x7c375dacba539463);
oracle!(reclaim_list_matches_bincode, 11, |r| list(r, 6, reclaim_entry), Vec<ReclaimEntry>, 0x5058a9fab06808c4);
oracle!(extent_list_matches_bincode, 12, |r| extents(r, 16), Vec<Extent>, 0xb462c01a4a28ede1);

#[test]
fn inode_meta_fields_are_not_part_of_the_record() {
    let mut r = Rng::new(99);
    let a = inode(&mut r);
    let mut b = a.clone();
    b.parent = 0;
    b.name = None;
    b.ctime = 0;
    b.mtime = 0;
    b.atime = 0;
    assert_eq!(unafs::codec::serialize(&a).unwrap(), unafs::codec::serialize(&b).unwrap());
}

