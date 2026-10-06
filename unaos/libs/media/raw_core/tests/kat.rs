//! RAWCORE (B444) host KATs — on the SYNTHETIC ARW (`raw_core::synth`): no real Sony file is on this machine.
//! A real file's KATs (Peter's card: the first 64 KiB for the container, a whole small ARW for the demosaic)
//! go in `tests/fixtures` when one is dropped.

use raw_core::synth::{self, Coding};
use raw_core::{color, parse, Binner, Error, RowDecoder, Tone};

#[test]
fn srgb_encoder_matches_std_powf() {
    let t = color::thresholds();
    for i in 0..=4096u32 {
        let lin = i as f64 / 4096.0;
        let s = if lin <= 0.0031308 { lin * 12.92 } else { 1.055 * lin.powf(1.0 / 2.4) - 0.055 };
        let want = (s * 255.0).round() as i32;
        let got = color::encode(lin, &t) as i32;
        assert!((want - got).abs() <= 1, "lin={lin} want={want} got={got}");
    }
    assert_eq!(color::encode(0.0, &t), 0);
    assert_eq!(color::encode(1.0, &t), 255);
    assert_eq!(color::encode(0.5, &t), 188); // sRGB(0.5) = 0.7354 -> 187.5 -> 188
}

#[test]
fn container_ifds_preview_and_facts() {
    let (w, h) = (16, 16);
    let f = synth::arw(w, h, &synth::ramp(w, h), Coding::Plain14);
    let info = parse(&f).expect("parse");
    assert!(info.le);
    assert_eq!(info.ifds, 4, "IFD0, IFD1, the raw SubIFD, the EXIF IFD");
    let s = info.strip.as_ref().expect("strip");
    assert_eq!((s.width, s.height, s.bits, s.compression), (16, 16, 14, 1));
    assert_eq!(s.cfa, [0, 1, 1, 2]);
    assert_eq!((s.black, s.white), (synth::BLACK, synth::WHITE));
    let (po, pl) = info.preview_in(f.len() as u64).expect("preview");
    assert_eq!(&f[po as usize..po as usize + pl as usize], synth::PREVIEW_JPEG);
    let x = &info.facts;
    assert_eq!(x.camera().as_deref(), Some("SONY ILCE-7M3"));
    assert_eq!(x.lens.as_deref(), Some(synth::LENS));
    assert_eq!(x.exposure_text().as_deref(), Some("1/250"));
    assert_eq!(x.iso, Some(400));
    assert_eq!(x.focal_mm(), Some(35));
    assert_eq!(x.taken(), Some(synth::TAKEN_UNIX));
    assert_eq!((x.width, x.height), (Some(16), Some(16)));
    assert_eq!(x.count(), 8);
    assert_eq!(raw_core::mime_of(&f), Some(raw_core::MIME_ARW));
    // The facts read from the head alone (a 64 KiB read of a 50 MB file): the strip is named, not needed.
    let head = &f[..f.len() - 64];
    assert_eq!(parse(head).unwrap().facts, info.facts);
}

#[test]
fn plain14_rows_and_bilinear() {
    let (w, h) = (16u32, 16u32);
    let m = synth::ramp(w, h);
    let f = synth::arw(w, h, &m, Coding::Plain14);
    let info = parse(&f).unwrap();
    let s = info.strip.unwrap();
    let d = RowDecoder::new(&s).unwrap();
    let strip = &f[s.offset as usize..(s.offset + s.len) as usize];
    let got = d.all(strip).unwrap();
    assert_eq!(got, m, "uncompressed samples round-trip");
    let tone = Tone::new(s.black, s.white, d.max_code).unwrap();
    let rgba = raw_core::bilinear_rgba(&got, 16, 16, &s.cfa, &tone).unwrap();
    // Pixel (2,2) is red: R = its own sample; G = mean of its 4 green neighbours; B = mean of 4 diagonal blues.
    let at = |x: usize, y: usize| m[y * 16 + x] as u32;
    let o = (2 * 16 + 2) * 4;
    assert_eq!(rgba[o], tone.map(at(2, 2)));
    assert_eq!(rgba[o + 1], tone.map((at(1, 2) + at(3, 2) + at(2, 1) + at(2, 3) + 2) / 4));
    assert_eq!(rgba[o + 2], tone.map((at(1, 1) + at(3, 1) + at(1, 3) + at(3, 3) + 2) / 4));
    assert_eq!(rgba[o + 3], 255);
    // Black maps to 0; a ramp is monotone along x in every channel.
    assert_eq!(tone.map(synth::BLACK as u32), 0);
    assert_eq!(tone.map(synth::WHITE as u32), 255);
    for x in 1..15 {
        let a = (8 * 16 + x) * 4;
        assert!(rgba[a + 4] >= rgba[a], "R monotone at x={x}");
    }
}

#[test]
fn craw_block_round_trip_and_binner() {
    let (w, h) = (64u32, 4u32);
    // 11-bit codes with every block's span below 128 (exact cRAW).
    let codes: Vec<u16> = (0..w * h).map(|i| 300 + (i % w) as u16 * 3 + ((i / w) % 2) as u16 * 50).collect();
    let f = synth::arw(w, h, &codes, Coding::Craw);
    let s = parse(&f).unwrap().strip.unwrap();
    assert_eq!((s.compression, s.bits), (32767, 8));
    let d = RowDecoder::new(&s).unwrap();
    assert_eq!(d.row_bytes(), 64);
    let got = d.all(&f[s.offset as usize..(s.offset + s.len) as usize]).unwrap();
    // No curve tag: dcraw's default curve is 16 * code, read as `curve[code << 1] >> 2` = 8 * code.
    let want: Vec<u16> = codes.iter().map(|&c| c * 8).collect();
    assert_eq!(got, want, "cRAW decode = dcraw's sony_arw2_load_raw on the encoded rows");
    assert_eq!(d.max_code, (0x7ff * 2 * 16) >> 2);
    // Binner 2x: each output pixel is the per-colour mean of its 2x2 quad.
    let tone = Tone::new(0, 0, d.max_code).unwrap();
    let mut b = Binner::new(2, 32, 2, s.cfa).unwrap();
    for y in 0..4 {
        b.row(y, &got[y * 64..(y + 1) * 64], &tone);
    }
    let q = |x: usize, y: usize| got[y * 64 + x] as u32;
    assert_eq!(b.rgba[0], tone.map(q(0, 0)));
    assert_eq!(b.rgba[1], tone.map((q(1, 0) + q(0, 1) + 1) / 2));
    assert_eq!(b.rgba[2], tone.map(q(1, 1)));
}

#[test]
fn curve_knees() {
    let c = raw_core::decode::sony_curve(Some([1000, 2000, 3000, 3500]));
    assert_eq!(c[1000], 1000);
    assert_eq!(c[1001], 1002);
    assert_eq!(c[2001], 1000 + 2000 + 4);
    assert_eq!(c[4095], 1000 + 2000 + 4000 + 4000 + 595 * 16);
}

#[test]
fn hostile_inputs_fail_closed() {
    assert_eq!(parse(&[]), Err(Error::Truncated));
    assert_eq!(parse(b"GIF89a\0\0\0\0"), Err(Error::NotTiff));
    let mut b = b"II\x2a\x00".to_vec();
    b.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    assert!(parse(&b).is_err());
    // An IFD whose SubIFD points back at itself: the visited set ends the walk.
    let mut l = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    l.extend_from_slice(&1u16.to_le_bytes());
    l.extend_from_slice(&330u16.to_le_bytes());
    l.extend_from_slice(&4u16.to_le_bytes());
    l.extend_from_slice(&1u32.to_le_bytes());
    l.extend_from_slice(&8u32.to_le_bytes());
    l.extend_from_slice(&8u32.to_le_bytes()); // next = itself
    let i = parse(&l).unwrap();
    assert_eq!(i.ifds, 1);
    assert!(i.strip.is_none());
    // A CFA IFD naming 60000x60000: fenced before anything is reserved.
    let mut f = synth::arw(16, 16, &synth::ramp(16, 16), Coding::Plain14);
    let p = f.windows(12).position(|e| e[..2] == 256u16.to_le_bytes() && e[2..4] == 4u16.to_le_bytes()).unwrap();
    f[p + 8..p + 12].copy_from_slice(&60000u32.to_le_bytes());
    f[p + 20..p + 24].copy_from_slice(&60000u32.to_le_bytes());
    assert_eq!(parse(&f), Err(Error::TooLarge));
    // A strip shorter than its rows claim.
    let f = synth::arw(16, 16, &synth::ramp(16, 16), Coding::Plain14);
    let mut s = parse(&f).unwrap().strip.unwrap();
    s.len -= 2;
    assert_eq!(RowDecoder::new(&s).err(), Some(Error::Truncated));
    // Every truncation of a real-shaped file parses or refuses — never panics.
    for n in 0..f.len().min(600) {
        let _ = parse(&f[..n]);
    }
}
