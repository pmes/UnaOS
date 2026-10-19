// SPDX-License-Identifier: LGPL-3.0-or-later
// GEODEMOD (LEDGER SR157): the one `.geode` v1 codec — streamed and whole forms agree, every value kind round-trips,
// and every structural refusal the host crate and the kernel make comes from here.

use std::collections::BTreeMap;

use geode_core::{Bytes, Encoder, Entry, check, entry_head, entry_len, attrs_len, read_archive, write_archive, HEADER_LEN, TRAILER_LEN};
use unafs::codec::Writer;
use unafs::{AttributeValue as A, FileKind};

fn entry(path: &str, kind: FileKind, data: &[u8]) -> Entry {
    let mut attributes = BTreeMap::new();
    if kind == FileKind::File {
        attributes.insert("geode.note".to_string(), A::String(format!("note {path}")));
        attributes.insert("geode.rank".to_string(), A::Int(-7));
        attributes.insert("geode.weight".to_string(), A::Float(2.5));
        attributes.insert("geode.thumb".to_string(), A::Blob((0..6000usize).map(|k| k as u8).collect()));
        attributes.insert("geode.vec".to_string(), A::Vector((0..16).map(|k| k as f32 * 0.25).collect()));
    }
    Entry { path: path.into(), kind, ctime: 1_600_000_000, mtime: 1_600_000_001, atime: 1_600_000_002, attributes, data: data.to_vec() }
}

fn sample() -> Vec<Entry> {
    vec![
        entry("docs", FileKind::Directory, b""),
        entry("docs/a.txt", FileKind::File, b"alpha"),
        entry("docs/deep", FileKind::Directory, b""),
        entry("docs/deep/big.bin", FileKind::File, &(0..200_000u32).map(|k| (k * 7) as u8).collect::<Vec<u8>>()),
        entry("top.txt", FileKind::File, b""),
    ]
}

/// The pre-GEODEMOD whole-record encoding (handlers/geode's `write_archive` as it was), independent of the core.
fn reference(entries: &[Entry]) -> Vec<u8> {
    let mut w = Writer::new();
    w.raw(b"GEODEAR1");
    w.u32(1);
    w.u64(entries.len() as u64);
    for e in entries {
        w.str(&e.path);
        unafs::codec::Encode::encode(&e.kind, &mut w);
        w.u64(e.ctime);
        w.u64(e.mtime);
        w.u64(e.atime);
        w.map(&e.attributes);
        w.bytes(&e.data);
    }
    let mut out = w.into_bytes();
    let sum = unafs::hash::hash_bytes(&out);
    out.extend_from_slice(b"GEODEEND");
    out.extend_from_slice(&sum.to_le_bytes());
    out
}

#[test]
fn whole_and_streamed_encodings_are_the_v1_bytes() {
    let es = sample();
    let whole = write_archive(&es);
    assert_eq!(whole, reference(&es));
    // Streamed: heads then data in small pieces, as the kernel's writer passes them.
    let mut out = Vec::new();
    let mut enc = Encoder::new(&mut out, es.len() as u64).unwrap();
    for e in &es {
        enc.put(&entry_head(&e.path, e.kind, e.ctime, e.mtime, e.atime, &e.attributes, e.data.len() as u64)).unwrap();
        for c in e.data.chunks(4096) {
            enc.put(c).unwrap();
        }
    }
    let n = enc.finish().unwrap();
    assert_eq!(out, whole);
    assert_eq!(n, whole.len() as u64);
    // The measure the kernel's plan makes.
    let measured: u64 = HEADER_LEN + TRAILER_LEN + es.iter().map(|e| entry_len(e.path.len(), attrs_len(&e.attributes), e.data.len() as u64)).sum::<u64>();
    assert_eq!(measured, whole.len() as u64);
}

#[test]
fn decode_round_trips_and_check_agrees() {
    let es = sample();
    let bytes = write_archive(&es);
    assert_eq!(read_archive(&bytes).unwrap(), es);
    let s = check(&mut Bytes(&bytes)).unwrap();
    assert_eq!(s.entries, 5);
    assert_eq!(s.files, 3);
    assert_eq!(s.data_bytes, 5 + 200_000);
    assert_eq!(s.tops, vec!["docs".to_string(), "top.txt".to_string()]);
    assert_eq!(read_archive(&write_archive(&[])).unwrap(), Vec::<Entry>::new());
}

#[test]
fn damage_and_bad_structure_are_refused() {
    let bytes = write_archive(&sample());
    for at in [12, 40, bytes.len() / 2, bytes.len() - 17] {
        let mut b = bytes.clone();
        b[at] ^= 0x40;
        assert!(read_archive(&b).is_err(), "flip at {at}");
        assert!(check(&mut Bytes(&b)).is_err(), "flip at {at}");
    }
    assert_eq!(read_archive(&bytes[..bytes.len() - 1]), Err("trailer"));
    assert_eq!(read_archive(&bytes[..10]), Err("short"));
    let mut tail = bytes.clone();
    tail.truncate(tail.len() - 16);
    tail.push(0);
    let sum = unafs::hash::hash_bytes(&tail);
    tail.extend_from_slice(b"GEODEEND");
    tail.extend_from_slice(&sum.to_le_bytes());
    assert_eq!(read_archive(&tail), Err("trailing"));

    let e = |p: &str, k| entry(p, k, b"");
    let cases: [(Vec<Entry>, &str); 5] = [
        (vec![e("../x", FileKind::File)], "path"),
        (vec![e("a//b", FileKind::File)], "path"),
        (vec![e("d/x", FileKind::File), e("d", FileKind::Directory)], "parent"),
        (vec![e("x", FileKind::File), e("x", FileKind::File)], "repeated"),
        (vec![entry("d", FileKind::Directory, b"oops")], "dirdata"),
    ];
    for (bad, why) in cases {
        let b = write_archive(&bad);
        assert_eq!(read_archive(&b), Err(why));
        assert_eq!(check(&mut Bytes(&b)).map(|_| ()), Err(why));
    }
    let b = write_archive(&[e("link", FileKind::Symlink)]);
    assert_eq!(read_archive(&b), Err("kind"));
}
