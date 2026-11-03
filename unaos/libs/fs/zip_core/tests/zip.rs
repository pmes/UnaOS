//! zip_core's witnesses (VUG3MF, LEDGER SR486): an archive written by an independent writer reads; a stored +
//! deflated archive round-trips byte-deterministically; every refusal is reached by name.

use zip_core::{Method, Refusal, Writer, read};

/// Written by CPython 3.11.17's `zipfile` (an independent writer; the generator, run once and inlined):
/// `ZipInfo('hello.txt', (1980,1,1,0,0,0))` stored `b"hello, zip\n"` and `ZipInfo('dir/text.txt', ...)` deflated
/// `b"the quick brown fox jumps over the lazy dog. " * 8`, 278 bytes.
const CPYTHON: &[u8] = &[
    0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x1b, 0x38,
    0x72, 0xcb, 0x0b, 0x00, 0x00, 0x00, 0x0b, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x68, 0x65,
    0x6c, 0x6c, 0x6f, 0x2e, 0x74, 0x78, 0x74, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x2c, 0x20, 0x7a, 0x69,
    0x70, 0x0a, 0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x21, 0x00,
    0x0f, 0x86, 0xd9, 0xb7, 0x33, 0x00, 0x00, 0x00, 0x68, 0x01, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00,
    0x64, 0x69, 0x72, 0x2f, 0x74, 0x65, 0x78, 0x74, 0x2e, 0x74, 0x78, 0x74, 0x2b, 0xc9, 0x48, 0x55,
    0x28, 0x2c, 0xcd, 0x4c, 0xce, 0x56, 0x48, 0x2a, 0xca, 0x2f, 0xcf, 0x53, 0x48, 0xcb, 0xaf, 0x50,
    0xc8, 0x2a, 0xcd, 0x2d, 0x28, 0x56, 0xc8, 0x2f, 0x4b, 0x2d, 0x52, 0x28, 0x01, 0x4a, 0xe7, 0x24,
    0x56, 0x55, 0x2a, 0xa4, 0xe4, 0xa7, 0xeb, 0x81, 0x79, 0xa3, 0x8a, 0xc9, 0x52, 0x0c, 0x00, 0x50,
    0x4b, 0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x1b,
    0x38, 0x72, 0xcb, 0x0b, 0x00, 0x00, 0x00, 0x0b, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00, 0x00, 0x00, 0x00, 0x68, 0x65, 0x6c,
    0x6c, 0x6f, 0x2e, 0x74, 0x78, 0x74, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x00,
    0x08, 0x00, 0x00, 0x00, 0x21, 0x00, 0x0f, 0x86, 0xd9, 0xb7, 0x33, 0x00, 0x00, 0x00, 0x68, 0x01,
    0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01,
    0x32, 0x00, 0x00, 0x00, 0x64, 0x69, 0x72, 0x2f, 0x74, 0x65, 0x78, 0x74, 0x2e, 0x74, 0x78, 0x74,
    0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x02, 0x00, 0x71, 0x00, 0x00, 0x00,
    0x8f, 0x00, 0x00, 0x00, 0x00, 0x00,
];

const FOX: &[u8] = b"the quick brown fox jumps over the lazy dog. ";

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// The end of central directory record's offset (no archive comment in these tests).
fn eocd(b: &[u8]) -> usize {
    b.len() - 22
}

/// The offset of central directory header `n`.
fn central(b: &[u8], n: usize) -> usize {
    let mut p = u32_at(b, eocd(b) + 16) as usize;
    for _ in 0..n {
        p += 46 + u16_at(b, p + 28) as usize + u16_at(b, p + 30) as usize + u16_at(b, p + 32) as usize;
    }
    p
}

fn two_entry() -> Vec<u8> {
    let mut w = Writer::new();
    w.add("a/stored.txt", b"stored bytes\n", Method::Stored).unwrap();
    w.add("b/deflated.txt", &FOX.repeat(40), Method::Deflated).unwrap();
    w.finish()
}

#[test]
fn cpython_archive_reads() {
    let a = read(CPYTHON).unwrap();
    assert_eq!(a.entries().len(), 2);
    let h = a.by_name("hello.txt").unwrap();
    assert_eq!(h.method, Method::Stored);
    assert_eq!(h.data().unwrap(), b"hello, zip\n");
    let t = a.by_name("dir/text.txt").unwrap();
    assert_eq!(t.method, Method::Deflated);
    assert_eq!(t.size, 360);
    assert_eq!(t.data().unwrap(), FOX.repeat(8));
    assert!(a.by_name("missing").is_none());
}

#[test]
fn stored_and_deflated_round_trip() {
    let z = two_entry();
    let a = read(&z).unwrap();
    let names: Vec<&str> = a.entries().iter().map(|e| e.name).collect();
    assert_eq!(names, ["a/stored.txt", "b/deflated.txt"]);
    assert_eq!(a.entries()[0].method, Method::Stored);
    assert_eq!(a.entries()[0].data().unwrap(), b"stored bytes\n");
    let d = &a.entries()[1];
    assert_eq!(d.method, Method::Deflated);
    assert!(d.compressed.len() < FOX.len() * 40, "deflate shrank a repetitive text");
    assert_eq!(d.data().unwrap(), FOX.repeat(40));
}

#[test]
fn writer_is_deterministic() {
    assert_eq!(two_entry(), two_entry());
    assert_eq!(read(&Writer::new().finish()).unwrap().entries().len(), 0);
}

#[test]
fn truncated_central_directory_refuses() {
    let z = two_entry();
    // Drop the last 10 bytes of the central directory, keep the end record: its size now reaches past it.
    let e = eocd(&z);
    let mut cut = z[..e - 10].to_vec();
    cut.extend_from_slice(&z[e..]);
    assert_eq!(read(&cut), Err(Refusal::Truncated { what: "central directory" }));
}

#[test]
fn not_zip_refuses() {
    assert_eq!(read(b""), Err(Refusal::NotZip));
    assert_eq!(read(b"solid cube\nendsolid cube\n"), Err(Refusal::NotZip));
    let z = two_entry();
    assert_eq!(read(&z[..z.len() - 1]), Err(Refusal::NotZip));
}

#[test]
fn crc_mismatch_refuses() {
    let mut z = CPYTHON.to_vec();
    z[30 + 9] ^= 0x20; // the first byte of hello.txt's stored data
    let a = read(&z).unwrap();
    assert_eq!(a.by_name("hello.txt").unwrap().data(), Err(Refusal::CrcMismatch { name: "hello.txt".into() }));
}

#[test]
fn declared_size_bounds_the_inflate() {
    let mut z = two_entry();
    let c = central(&z, 1);
    let smaller = u32_at(&z, c + 24) - 1;
    z[c + 24..c + 28].copy_from_slice(&smaller.to_le_bytes());
    let a = read(&z).unwrap();
    assert_eq!(a.entries()[1].data(), Err(Refusal::SizeMismatch { name: "b/deflated.txt".into() }));
}

#[test]
fn unknown_method_refuses_by_number() {
    let mut z = two_entry();
    let c = central(&z, 0);
    z[c + 10..c + 12].copy_from_slice(&12u16.to_le_bytes()); // bzip2
    assert_eq!(read(&z), Err(Refusal::Method { name: "a/stored.txt".into(), method: 12 }));
}

#[test]
fn local_header_must_agree() {
    let mut z = two_entry();
    let c = central(&z, 1);
    z[c + 10..c + 12].copy_from_slice(&0u16.to_le_bytes()); // central says stored, local says deflated
    assert_eq!(read(&z), Err(Refusal::LocalMismatch { name: "b/deflated.txt".into() }));
}

#[test]
fn encrypted_refuses() {
    let mut z = two_entry();
    let c = central(&z, 0);
    z[c + 8] |= 1;
    assert_eq!(read(&z), Err(Refusal::Encrypted { name: "a/stored.txt".into() }));
}

#[test]
fn zip64_and_multidisk_refuse() {
    let z = two_entry();
    let e = eocd(&z);
    let mut z64 = z.clone();
    z64[e + 10..e + 12].copy_from_slice(&0xFFFFu16.to_le_bytes());
    assert_eq!(read(&z64), Err(Refusal::Zip64));
    let mut multi = z.clone();
    multi[e + 4] = 1;
    assert_eq!(read(&multi), Err(Refusal::MultiDisk));
}
