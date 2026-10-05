// SPDX-License-Identifier: LGPL-3.0-or-later
//! The ONE inflater (moved from the kernel's selfhost/inflate.rs): gzip and zlib streams written by an
//! independent encoder (CPython's zlib, level 9: dynamic Huffman + LZ77) must inflate to the exact text,
//! and a flipped trailer byte must be refused by name.

use pixel_core::inflate::{self, ByteSource, InflateError, Sink};

struct Src<'a>(&'a [u8], usize);
impl ByteSource for Src<'_> {
    fn next(&mut self) -> Option<u8> {
        let b = self.0.get(self.1).copied();
        self.1 += 1;
        b
    }
}
struct Out(Vec<u8>);
impl Sink for Out {
    fn push(&mut self, b: u8) -> Result<(), ()> {
        self.0.push(b);
        Ok(())
    }
}

fn text() -> Vec<u8> {
    (0..200).flat_map(|i| format!("pixel core line {i} of the one inflater\n").into_bytes()).collect()
}

#[test]
fn gunzip_kat() {
    let gz = include_bytes!("fixtures/inflate/kat.gz");
    let mut out = Out(Vec::new());
    let r = inflate::gunzip(&mut Src(gz, 0), &mut out).unwrap();
    assert_eq!(out.0, text());
    assert_eq!((r.uncompressed, r.crc), (7890, 3934169410));
    let mut bad = gz.to_vec();
    let n = bad.len();
    bad[n - 6] ^= 1; // inside the CRC-32
    assert_eq!(inflate::gunzip(&mut Src(&bad, 0), &mut Out(Vec::new())).err(), Some(InflateError::TrailerMismatch));
}

#[test]
fn zlib_kat() {
    let z = include_bytes!("fixtures/inflate/kat.z");
    let mut out = Out(Vec::new());
    let r = inflate::zlib_inflate(&mut Src(z, 0), &mut out).unwrap();
    assert_eq!(out.0, text());
    assert_eq!(r.adler, 2910622643);
    let mut bad = z.to_vec();
    let n = bad.len();
    bad[n - 1] ^= 1;
    assert_eq!(inflate::zlib_inflate(&mut Src(&bad, 0), &mut Out(Vec::new())).err(), Some(InflateError::AdlerMismatch));
    assert_eq!(pixel_core::zlib_decompress(z, 1 << 20).unwrap(), text());
}
