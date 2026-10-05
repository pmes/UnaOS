//! UNAFSCODEC (SR53) M3: the record decoders under mutation.
//!
//! 20,000 mutated records (bit flips, truncations, byte overwrites, hostile
//! u64 splices over lengths and tags, appended garbage) spread over every
//! §R1 record type, plus whole mutated INODE BLOCKS through the block reader
//! (record + meta trailer + spill trailer) and the hand-packed superblock /
//! root / catalog-record parsers. The contract (spec §R1 "Decoding"):
//!
//! * no input panics (each decode runs under `catch_unwind`);
//! * every refusal is a typed `DecodeError` naming the record and the
//!   `Type.field` it stopped at, with an offset inside the input;
//! * no decode allocates from a claimed length: a counting allocator bounds
//!   the peak heap of every decode by a small multiple of the INPUT size.

mod common;
use common::*;

use std::alloc::{GlobalAlloc, Layout, System};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};

use unafs::codec::{CodecError, Decode, DecodeErrorKind, Encode};
use unafs::{BLOCK_SIZE, DirEntry, ReclaimEntry, SnapshotEntry};
use unafs::catalog::CatalogEntry;
use unafs::inode::{AttributeValue, Extent, FileKind, IndirectTrailer, Inode};
use unafs::legacy::LegacySuperblock;
use unafs::superblock::Superblock;

// ---- a counting allocator (per-thread peak would need TLS; the suite runs
// its decodes on one thread inside one test, so a global high-water mark is
// exact for the measured section) ----------------------------------------

struct Counting;
static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = CUR.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        CUR.fetch_sub(l.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Peak bytes allocated while running `f`, above the level at entry.
fn peak_during<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let base = CUR.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let r = f();
    (r, PEAK.load(Ordering::Relaxed).saturating_sub(base))
}

/// The heap a decode of `n` input bytes may use: in-RAM records are at most
/// a few times their minimum encoding (an `Inode` is ~4x its 44 B minimum,
/// a map pair a few BTreeMap node slots), so 64x the input plus a fixed
/// slack is generous for honest records and far below what a hostile length
/// prefix (up to 2^64) would force if it were trusted.
fn heap_budget(n: usize) -> usize {
    64 * n + 64 * 1024
}

#[derive(Default)]
struct Tally {
    decoded: usize,
    accepted: usize,
    refused: usize,
    truncated: usize,
    limit: usize,
    variant: usize,
    utf8: usize,
    max_heap_ratio: f64,
}

fn check_err(e: &CodecError, input_len: usize, record: &'static str, t: &mut Tally) {
    let d = e.decode_error();
    assert_eq!(d.record, record, "refusal names the record");
    assert!(!d.field.is_empty(), "refusal names a field");
    assert!(d.offset <= input_len, "refusal offset {} past input {input_len}", d.offset);
    let shown = format!("{e}");
    assert!(shown.contains(d.field), "Display carries the field: {shown}");
    match d.kind {
        DecodeErrorKind::Truncated { .. } => t.truncated += 1,
        DecodeErrorKind::LimitExceeded { .. } => t.limit += 1,
        DecodeErrorKind::BadVariant(_) => t.variant += 1,
        DecodeErrorKind::BadUtf8 => t.utf8 += 1,
    }
    t.refused += 1;
}

/// Mutate `count` encodings of `T` and decode each under both budgets.
fn fuzz<T: Encode + Decode>(seed: u64, count: usize, gen_: fn(&mut Rng) -> T, t: &mut Tally) {
    let mut r = Rng::new(seed);
    for _ in 0..count {
        let v = gen_(&mut r);
        let mut bytes = unafs::codec::serialize(&v).unwrap();
        // One to three stacked mutations.
        for _ in 0..1 + r.below(3) {
            mutate(&mut r, &mut bytes);
        }
        for block_budget in [false, true] {
            let res = catch_unwind(AssertUnwindSafe(|| {
                peak_during(|| {
                    if block_budget {
                        unafs::codec::deserialize_block::<T>(&bytes).map(|_| ())
                    } else {
                        unafs::codec::deserialize::<T>(&bytes).map(|_| ())
                    }
                })
            }));
            let (res, heap) = res.unwrap_or_else(|_| panic!("{} decode panicked on {bytes:02x?}", T::NAME));
            assert!(
                heap <= heap_budget(bytes.len()),
                "{} decode of {} B allocated {heap} B",
                T::NAME,
                bytes.len()
            );
            t.max_heap_ratio = t.max_heap_ratio.max(heap as f64 / (bytes.len().max(1)) as f64);
            t.decoded += 1;
            match res {
                Ok(()) => t.accepted += 1,
                Err(e) => check_err(&e, bytes.len(), T::NAME, t),
            }
        }
    }
}

#[test]
fn twenty_thousand_mutated_records_never_panic_and_refuse_by_field() {
    let mut t = Tally::default();
    // 12 record types; 20,004 mutated records in all (1,667 each).
    let per = 1667;
    fuzz::<Superblock>(101, per, superblock, &mut t);
    fuzz::<LegacySuperblock>(102, per, legacy_superblock, &mut t);
    fuzz::<FileKind>(103, per, kind, &mut t);
    fuzz::<Extent>(104, per, extent, &mut t);
    fuzz::<AttributeValue>(105, per, attr, &mut t);
    fuzz::<Inode>(106, per, inode, &mut t);
    fuzz::<IndirectTrailer>(107, per, trailer, &mut t);
    fuzz::<Vec<DirEntry>>(108, per, |r| list(r, 8, dir_entry), &mut t);
    fuzz::<Vec<CatalogEntry>>(109, per, |r| list(r, 8, catalog_entry), &mut t);
    fuzz::<Vec<SnapshotEntry>>(110, per, |r| list(r, 6, snapshot_entry), &mut t);
    fuzz::<Vec<ReclaimEntry>>(111, per, |r| list(r, 6, reclaim_entry), &mut t);
    fuzz::<Vec<Extent>>(112, per, |r| extents(r, 16), &mut t);
    println!(
        "records {} decodes {} accepted {} refused {} (truncated {} limit {} variant {} utf8 {}) max heap/input {:.1}x",
        per * 12,
        t.decoded,
        t.accepted,
        t.refused,
        t.truncated,
        t.limit,
        t.variant,
        t.utf8,
        t.max_heap_ratio
    );
    assert_eq!(t.decoded, per * 12 * 2);
    assert_eq!(t.accepted + t.refused, t.decoded);
    // Every refusal class is exercised.
    assert!(t.truncated > 0 && t.limit > 0 && t.variant > 0 && t.utf8 > 0);
}

/// Whole inode blocks (record + v6 meta trailer + spill trailer + padding)
/// mutated and read through the block reader the mount uses.
#[test]
fn mutated_inode_blocks_never_panic() {
    let mut r = Rng::new(7);
    let (mut ok, mut refused) = (0usize, 0usize);
    for i in 0..4000 {
        let mut ino = inode(&mut r);
        ino.name = if r.below(3) == 0 { None } else { Some(r.string(30)) };
        let mut block = vec![0u8; BLOCK_SIZE as usize];
        let rec = unafs::codec::serialize(&ino).unwrap();
        let meta = ino.meta_bytes();
        let tr = unafs::codec::serialize(&trailer(&mut r)).unwrap();
        let mut off = 0;
        for part in [&rec[..], &meta[..], &tr[..]] {
            if off + part.len() <= block.len() {
                block[off..off + part.len()].copy_from_slice(part);
                off += part.len();
            }
        }
        for _ in 0..1 + r.below(4) {
            let at = r.below(off.max(1) as u64) as usize;
            match r.below(3) {
                0 => block[at] ^= 1 << r.below(8),
                1 => block[at] = r.next() as u8,
                _ => {
                    let v = [u64::MAX, u64::MAX / 2, 0xFFFF, 4097][r.below(4) as usize];
                    for (k, b) in v.to_le_bytes().iter().enumerate() {
                        if at + k < block.len() {
                            block[at + k] = *b;
                        }
                    }
                }
            }
        }
        let res = catch_unwind(AssertUnwindSafe(|| peak_during(|| Inode::decode_block(&block).map(|_| ()))));
        let (res, heap) = res.unwrap_or_else(|_| panic!("inode block #{i} panicked"));
        assert!(heap <= heap_budget(block.len()), "inode block #{i} allocated {heap} B");
        match res {
            Ok(()) => ok += 1,
            Err(_) => refused += 1,
        }
    }
    println!("inode blocks: 4000 mutated, {ok} accepted, {refused} refused");
}

/// The hand-packed parsers on random and mutated sectors: never a panic.
#[test]
fn hand_packed_parsers_never_panic() {
    let mut r = Rng::new(9);
    for _ in 0..4000 {
        let mut sector = vec![0u8; 512];
        let good = unafs::RootRecord {
            generation: r.u64() | 1,
            imap_block: r.u64(),
            imap_leaves: r.u64(),
            next_inode: r.u64(),
            refmap_block: r.u64(),
            refmap_leaves: r.u64(),
            free_blocks: r.u64(),
            flags: r.u64(),
        }
        .to_bytes();
        sector[..good.len()].copy_from_slice(&good);
        let mut v = sector.clone();
        mutate(&mut r, &mut v);
        let _ = catch_unwind(|| unafs::RootRecord::from_sector(&v)).expect("root parser panicked");
        let bc = r.u64();
        let _ = catch_unwind(|| unafs::CatalogRecord::from_bytes(&v, bc)).expect("catalog parser panicked");
        let mut sb = unafs::codec::serialize(&superblock(&mut r)).unwrap();
        mutate(&mut r, &mut sb);
        let _ = catch_unwind(|| Superblock::from_bytes(&sb)).expect("superblock parser panicked");
    }
}

/// The adversarial lengths, named: each refusal is the field it should be.
#[test]
fn hostile_lengths_name_their_field() {
    let e = |b: &[u8]| unafs::codec::deserialize::<Inode>(b).unwrap_err().decode_error().field;
    let mut b = Vec::new();
    b.extend_from_slice(&1u64.to_le_bytes()); // id
    b.extend_from_slice(&0u32.to_le_bytes()); // kind
    b.extend_from_slice(&0u64.to_le_bytes()); // size
    let mut chunks = b.clone();
    chunks.extend_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(e(&chunks), "Inode.chunks");
    let mut attrs = b.clone();
    attrs.extend_from_slice(&0u64.to_le_bytes());
    attrs.extend_from_slice(&(1u64 << 40).to_le_bytes());
    assert_eq!(e(&attrs), "Inode.attributes");
    let mut kind = 1u64.to_le_bytes().to_vec();
    kind.extend_from_slice(&9u32.to_le_bytes());
    let err = unafs::codec::deserialize::<Inode>(&kind).unwrap_err();
    assert_eq!(err.decode_error().field, "Inode.kind");
    assert_eq!(err.decode_error().kind, DecodeErrorKind::BadVariant(9));
    assert_eq!(err.decode_error().offset, 8);

    let mut s = 2u32.to_le_bytes().to_vec();
    s.extend_from_slice(&2u64.to_le_bytes());
    s.extend_from_slice(&[0xC3, 0x28]); // invalid UTF-8
    let err = unafs::codec::deserialize::<AttributeValue>(&s).unwrap_err();
    assert_eq!(err.decode_error().field, "AttributeValue.String");
    assert_eq!(err.decode_error().kind, DecodeErrorKind::BadUtf8);

    let mut big = 2u32.to_le_bytes().to_vec();
    big.extend_from_slice(&((unafs::codec::BLOCK_RECORD_LIMIT as u64) * 2).to_le_bytes());
    big.resize(4 + 8 + unafs::codec::BLOCK_RECORD_LIMIT * 2, b'a');
    let err = unafs::codec::deserialize_block::<AttributeValue>(&big).unwrap_err();
    assert!(matches!(err.decode_error().kind, DecodeErrorKind::LimitExceeded { .. }));
    // The same bytes fit the extent-backed budget.
    assert!(unafs::codec::deserialize::<AttributeValue>(&big).is_ok());
}
