//! UNAFSCODEC (SR53): the per-record PROPERTY ORACLE for the hand codec.
//!
//! For every record type that reaches disk, a seeded generator draws a
//! property set (boundary values first, then random values: empty/huge
//! integers, NaN/±inf/−0.0/subnormal floats, empty and multi-byte UTF-8
//! strings, empty and long lists, multi-key maps) and the hand codec's bytes
//! are held to the oracle:
//!
//! * M1 (this file at the M1 commit 9fe7260f): byte-for-byte against
//!   `bincode 2.0.1` `legacy()` — the encoder the format was frozen under —
//!   and the decoders against each other on mutated bytes (all agreed);
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

mod common;
use common::*;

// ---- the oracle -------------------------------------------------------------

fn digest_push(h: &mut unafs::hash::FnvHasher, bytes: &[u8]) {
    h.write(&(bytes.len() as u64).to_le_bytes());
    h.write(bytes);
}

/// Draw `N` values of one record type, hold each to the oracle, return the
/// digest of the encoded bytes. The pinned digests were cut from bincode
/// 2.0.1 `legacy()`'s bytes for the same draws at the M1 commit (9fe7260f),
/// where this function also asserted byte equality with bincode directly and
/// that both decoders agreed on 8,000 mutations per type.
fn run<T>(seed: u64, gen_: fn(&mut Rng) -> T) -> u64
where
    T: Encode + Decode + std::fmt::Debug,
{
    let mut r = Rng::new(seed);
    let mut h = unafs::hash::FnvHasher::new();
    for i in 0..N {
        let v = gen_(&mut r);
        let ours = unafs::codec::serialize(&v).unwrap();
        digest_push(&mut h, &ours);
        // decode(encode(v)) re-encodes to the same bytes (NaN-safe equality).
        let (back, used) = unafs::codec::decode_prefix::<T>(&ours, usize::MAX).unwrap();
        assert_eq!(used, ours.len(), "{} #{i}: decode consumed {used} of {}", T::NAME, ours.len());
        assert_eq!(unafs::codec::serialize(&back).unwrap(), ours, "{} #{i}: round trip", T::NAME);
        // Keep the generator stream identical to the M1 draw (it mutated
        // four copies per value there).
        for _ in 0..4 {
            let mut bad = ours.clone();
            mutate(&mut r, &mut bad);
            let _ = unafs::codec::deserialize::<T>(&bad);
        }
    }
    h.finish()
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

