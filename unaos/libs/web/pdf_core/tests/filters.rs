// SPDX-License-Identifier: LGPL-3.0-or-later
//! PDF1 (SR300): every filter against a known plaintext; the predictors; the decode chain.

mod common;

use common::{lzw_encode, zlib_stored};
use pdf_core::filters::{self, ascii85, ascii_hex, flate, lzw, predict, run_length};
use pdf_core::object::{Dict, Stream};
use pdf_core::lexer::Lexer;

fn dict(s: &str) -> Dict {
    pdf_core::object::parse_object(&mut Lexer::new(s.as_bytes()), 0).unwrap().as_dict().unwrap().clone()
}

/// zlib.compress(b"The quick brown fox jumps over the lazy dog. " * 4, 9), from CPython.
const ZLIB_FOX: &str = "78da0bc94855282ccd4cce56482aca2fcf5348cbaf50c82acd2d2856c82f4b2d5228014ae72456552aa4e4a7eb29840c0ec500fa60409d";

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn flate_zlib_raw_and_lenient_trailer() {
    let fox = b"The quick brown fox jumps over the lazy dog. ".repeat(4);
    assert_eq!(flate(&unhex(ZLIB_FOX)).unwrap(), fox);
    let z = zlib_stored(b"hello world");
    assert_eq!(flate(&z).unwrap(), b"hello world");
    // A bare DEFLATE stream (no zlib header): inflate_raw.
    assert_eq!(flate(&z[2..z.len() - 4]).unwrap(), b"hello world");
    // The Adler-32 missing, or wrong: the data is kept.
    assert_eq!(flate(&z[..z.len() - 4]).unwrap(), b"hello world");
    let mut bad = z.clone();
    *bad.last_mut().unwrap() ^= 0xFF;
    assert_eq!(flate(&bad).unwrap(), b"hello world");
}

#[test]
fn lzw_spec_example_7_4_4_2() {
    // ISO 32000-1 §7.4.4.2 Example 2: 45 45 45 45 45 65 45 45 45 66.
    let enc = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
    assert_eq!(lzw(&enc, true).unwrap(), b"-----A---B");
    assert_eq!(lzw_encode(b"-----A---B", true), enc);
}

#[test]
fn lzw_code_widths_grow_with_and_without_early_change() {
    // Enough distinct strings to pass 512, 1024 and 2048 codes.
    let mut data = Vec::new();
    let mut x: u32 = 12345;
    for _ in 0..6000 {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        data.push(b'a' + ((x >> 16) % 7) as u8);
    }
    for early in [true, false] {
        let enc = lzw_encode(&data, early);
        assert_eq!(lzw(&enc, early).unwrap(), data, "early={early}");
    }
    assert_ne!(lzw_encode(&data, true), lzw_encode(&data, false));
}

#[test]
fn ascii_hex_7_4_2() {
    assert_eq!(ascii_hex(b"48 65 6C\n6c6F>ignored").unwrap(), b"Hello");
    assert_eq!(ascii_hex(b"414>").unwrap(), [0x41, 0x40]);
    assert!(ascii_hex(b"4G>").is_err());
}

#[test]
fn ascii85_7_4_3() {
    // base64.a85encode(b"Hello World", adobe=True) and (b"\0\0\0\0abc"), from CPython.
    assert_eq!(ascii85(b"<~87cURD]i,\"Ebo7~>").unwrap(), b"Hello World");
    assert_eq!(ascii85(b"87cURD]i,\n\"Ebo7~>").unwrap(), b"Hello World");
    assert_eq!(ascii85(b"<~z@:E^~>").unwrap(), b"\0\0\0\0abc");
    assert!(ascii85(b"{{{{{~>").is_err());
}

#[test]
fn run_length_7_4_5() {
    assert_eq!(run_length(&[2, b'a', b'b', b'c', 254, b'x', 128, b'z']).unwrap(), b"abcxxx");
}

fn png_encode(rows: &[Vec<u8>], bpp: usize, kinds: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut prev = vec![0u8; rows[0].len()];
    for (r, &k) in rows.iter().zip(kinds) {
        out.push(k);
        for i in 0..r.len() {
            let a = if i >= bpp { r[i - bpp] } else { 0 };
            let b = prev[i];
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            let pred = match k {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                _ => {
                    let p = a as i16 + b as i16 - c as i16;
                    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
                    if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
                }
            };
            out.push(r[i].wrapping_sub(pred));
        }
        prev = r.clone();
    }
    out
}

#[test]
fn png_predictors_all_five() {
    // 4 columns of RGB, 8 bits: bpp 3, row 12.
    let rows: Vec<Vec<u8>> = (0..5u8).map(|y| (0..12u8).map(|x| x.wrapping_mul(37).wrapping_add(y.wrapping_mul(91))).collect()).collect();
    let enc = png_encode(&rows, 3, &[0, 1, 2, 3, 4]);
    assert_eq!(predict(&enc, 12, 3, 8, 4).unwrap(), rows.concat());
    // The same through the stream decode chain (Flate + /DecodeParms).
    let s = Stream { dict: dict("<< /Filter /FlateDecode /DecodeParms << /Predictor 15 /Colors 3 /Columns 4 >> >>"), data: zlib_stored(&enc) };
    assert_eq!(filters::decode(&s, &|o| o.clone()).unwrap().data, rows.concat());
}

#[test]
fn tiff_predictor_2() {
    assert_eq!(predict(&[1, 1, 1, 1], 2, 1, 8, 4).unwrap(), [1, 2, 3, 4]);
    // 4 bits per sample: samples 1,1,1,1 -> 1,2,3,4.
    assert_eq!(predict(&[0x11, 0x11], 2, 1, 4, 4).unwrap(), [0x12, 0x34]);
    // 16 bits, two colours.
    assert_eq!(predict(&[0, 1, 0, 2, 0, 1, 0, 2], 2, 2, 16, 2).unwrap(), [0, 1, 0, 2, 0, 2, 0, 4]);
}

#[test]
fn filter_chains_and_image_pass_through() {
    let s = Stream { dict: dict("<< /Filter [/AHx /Fl] >>"), data: b"7801010500faff68656c6c6f062c0215>".to_vec() };
    assert_eq!(filters::decode(&s, &|o| o.clone()).unwrap().data, b"hello");
    let s = Stream { dict: dict("<< /Filter [/A85 /DCTDecode] /DecodeParms [null << /ColorTransform 0 >>] >>"), data: b"87cURD]i,\"Ebo7~>".to_vec() };
    let d = filters::decode(&s, &|o| o.clone()).unwrap();
    assert_eq!(d.data, b"Hello World");
    assert_eq!(d.image_filter.as_deref(), Some(&b"DCTDecode"[..]));
    assert_eq!(d.image_parms.unwrap().int(b"ColorTransform"), Some(0));
    let s = Stream { dict: dict("<< /Filter /Crypt >>"), data: Vec::new() };
    assert!(filters::decode(&s, &|o| o.clone()).is_err());
    let s = Stream { dict: dict("<< /Filter /LZWDecode /DecodeParms << /EarlyChange 0 >> >>"), data: lzw_encode(b"abcabcabc", false) };
    assert_eq!(filters::decode(&s, &|o| o.clone()).unwrap().data, b"abcabcabc");
}

#[test]
fn png_predictor_lenient() {
    // PIXEL-UNFILTER (SR316): the PDF rules kept around pixel_core's unfilter. 3 columns of gray, row 3, stride 4:
    // row 0 has filter type 7 (read as None), row 1 is Sub, row 2 is a short Up row (the stream stops mid-row).
    let enc = [7u8, 1, 2, 3, 1, 1, 1, 1, 2, 5];
    assert_eq!(predict(&enc, 10, 1, 8, 3).unwrap(), [1, 2, 3, 1, 2, 3, 6]);
}
