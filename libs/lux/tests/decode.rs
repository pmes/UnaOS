// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Round-trip / decode tests for lux's common-format decoders and the ARW
// fail-closed guards. Fixtures live in tests/fixtures (a few bytes each).

use lux::{Format, LuxError, decode, decode_jpeg, decode_png, parse_arw, sniff_format};

const RGB_PNG: &[u8] = include_bytes!("fixtures/tiny_rgb.png");
const GRAY_PNG: &[u8] = include_bytes!("fixtures/tiny_gray.png");
const RED_JPG: &[u8] = include_bytes!("fixtures/tiny_red.jpg");

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn png_rgb_decodes_exact_linear() {
    // 2x2: [red, green ; blue, white]. 0 and 255 map exactly to 0.0 and 1.0.
    let img = decode_png(RGB_PNG).expect("decode tiny_rgb.png");
    assert_eq!((img.width, img.height), (2, 2));
    assert_eq!(img.pixels.len(), 2 * 2 * 3);

    // pixel 0 = red
    assert!(approx(img.pixels[0], 1.0) && approx(img.pixels[1], 0.0) && approx(img.pixels[2], 0.0));
    // pixel 1 = green
    assert!(approx(img.pixels[3], 0.0) && approx(img.pixels[4], 1.0) && approx(img.pixels[5], 0.0));
    // pixel 2 = blue
    assert!(approx(img.pixels[6], 0.0) && approx(img.pixels[7], 0.0) && approx(img.pixels[8], 1.0));
    // pixel 3 = white
    assert!(
        approx(img.pixels[9], 1.0) && approx(img.pixels[10], 1.0) && approx(img.pixels[11], 1.0)
    );
}

#[test]
fn png_grayscale_expands_to_rgb() {
    // 2x1: [black, white] grayscale -> replicated across RGB.
    let img = decode_png(GRAY_PNG).expect("decode tiny_gray.png");
    assert_eq!((img.width, img.height), (2, 1));
    assert_eq!(img.pixels.len(), 2 * 3);
    assert!(approx(img.pixels[0], 0.0) && approx(img.pixels[1], 0.0) && approx(img.pixels[2], 0.0));
    assert!(approx(img.pixels[3], 1.0) && approx(img.pixels[4], 1.0) && approx(img.pixels[5], 1.0));
}

#[test]
fn jpeg_red_block_decodes_near_red() {
    // 8x8 solid red, JPEG is lossy so assert dominance not exactness.
    let img = decode_jpeg(RED_JPG).expect("decode tiny_red.jpg");
    assert_eq!((img.width, img.height), (8, 8));
    assert_eq!(img.pixels.len(), 8 * 8 * 3);
    let (r, g, b) = (img.pixels[0], img.pixels[1], img.pixels[2]);
    assert!(r > 0.7, "red channel weak: {r}");
    assert!(g < 0.2 && b < 0.2, "expected near-pure red, got ({r},{g},{b})");
}

#[test]
fn dispatch_sniffs_and_decodes() {
    assert_eq!(sniff_format(RGB_PNG), Some(Format::Png));
    assert_eq!(sniff_format(RED_JPG), Some(Format::Jpeg));
    assert!(decode(RGB_PNG).is_ok());
    assert!(decode(RED_JPG).is_ok());
    // TIFF/ARW magic recognized even though this stub can't fully decode.
    assert_eq!(sniff_format(b"II\x2a\x00rest"), Some(Format::Arw));
    assert_eq!(sniff_format(b"MM\x00\x2arest"), Some(Format::Arw));
    assert_eq!(sniff_format(b"garbage bytes"), None);
}

#[test]
fn decode_rejects_unknown_format() {
    match decode(b"\x00\x01\x02\x03\x04\x05\x06\x07\x08") {
        Err(LuxError::UnknownFormat) => {}
        Err(e) => panic!("expected UnknownFormat, got {e:?}"),
        Ok(_) => panic!("expected UnknownFormat, got Ok"),
    }
}

#[test]
fn png_fails_closed_on_garbage() {
    // Valid PNG magic, junk body — must Err, not panic.
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&[0u8; 32]);
    assert!(decode_png(&bytes).is_err());
    // Empty / truncated.
    assert!(decode_png(&[]).is_err());
    assert!(decode_png(b"not a png at all").is_err());
}

#[test]
fn jpeg_fails_closed_on_garbage() {
    assert!(decode_jpeg(&[]).is_err());
    assert!(decode_jpeg(b"\xFF\xD8\xFF garbage tail that is not a jpeg").is_err());
    assert!(decode_jpeg(b"totally not a jpeg").is_err());
}

#[test]
fn arw_fails_closed_on_garbage() {
    // Too small.
    assert!(parse_arw(&[]).is_err());
    assert!(parse_arw(&[0u8; 4]).is_err());
    // Valid TIFF magic, bogus IFD offset — bounded walk must Err, not panic/OOB.
    let mut b = b"II\x2a\x00".to_vec();
    b.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    assert!(parse_arw(&b).is_err());
    // Random noise that happens to start with a valid marker.
    let mut noise = b"MM\x00\x2a".to_vec();
    noise.extend_from_slice(&[0x11u8; 64]);
    assert!(parse_arw(&noise).is_err());
}

#[test]
fn arw_fails_closed_on_implausible_dimensions() {
    // A hand-built little-endian TIFF whose IFD names 60000x60000 (3.6 GP)
    // with a 4-byte uncompressed strip. The pixel-count fence must reject it
    // BEFORE allocating, returning an error rather than OOMing.
    let mut b = Vec::new();
    b.extend_from_slice(b"II\x2a\x00"); // header
    b.extend_from_slice(&8u32.to_le_bytes()); // first IFD at offset 8
    let entries: u16 = 5;
    b.extend_from_slice(&entries.to_le_bytes());
    let entry = |tag: u16, ftype: u16, count: u32, val: u32, out: &mut Vec<u8>| {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&ftype.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&val.to_le_bytes());
    };
    // ImageWidth / ImageLength (SHORT would truncate; use LONG type 4)
    entry(256, 4, 1, 60000, &mut b);
    entry(257, 4, 1, 60000, &mut b);
    entry(259, 3, 1, 1, &mut b); // Compression = 1 (uncompressed)
    entry(273, 4, 1, 200, &mut b); // StripOffsets -> some offset
    entry(279, 4, 1, 4, &mut b); // StripByteCounts = 4 bytes
    b.extend_from_slice(&0u32.to_le_bytes()); // next IFD = 0
    b.resize(256, 0); // room for the tiny strip
    match parse_arw(&b) {
        Err(_) => {}
        Ok(img) => panic!(
            "fence failed: allocated {}x{} = {} pixels",
            img.width,
            img.height,
            img.pixels.len()
        ),
    }
}
