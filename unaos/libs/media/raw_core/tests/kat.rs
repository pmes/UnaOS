//! RAWCORE (B444) host KATs — on the SYNTHETIC ARW (`raw_core::synth`): no real Sony file is on this machine.
//! A real file's KATs (Peter's card: the first 64 KiB for the container, a whole small ARW for the demosaic)
//! go in `tests/fixtures` when one is dropped.

use raw_core::synth::{self, Coding};
use raw_core::synth::Opts;
use raw_core::{color, half_rgba, parse, Binner, Error, RowDecoder, Tone, Wb, WbSource, WbTone};

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

// ── RAWVIEW (B711): the delta block by its documented layout, the half-size develop, the white balance ────────

#[test]
fn craw_delta_block_by_layout() {
    // One 32-column row = two 16-byte blocks: the even columns, then the odd ones. Each block: bits 0..11 max,
    // 11..22 min, 22..26 imax, 26..30 imin, then fourteen 7-bit deltas above min (sh = 0: the span is < 128).
    let block = |px: &[u32; 16]| -> [u8; 16] {
        let imax = (0..16).max_by_key(|&i| (px[i], 16 - i)).unwrap();
        let imin = (0..16).filter(|&i| i != imax).min_by_key(|&i| (px[i], i)).unwrap();
        let (max, min) = (px[imax], px[imin]);
        assert!(max - min < 128);
        let mut v: u128 = (max | min << 11 | (imax as u32) << 22 | (imin as u32) << 26) as u128;
        let mut bit = 30;
        for i in (0..16).filter(|&i| i != imax && i != imin) {
            v |= ((px[i] - min) as u128) << bit;
            bit += 7;
        }
        v.to_le_bytes()
    };
    let mut even = [0u32; 16];
    for (i, p) in even.iter_mut().enumerate() {
        *p = 900 + 5 * i as u32;
    }
    even[3] = 1000;
    even[7] = 890;
    let odd = [500u32; 16];
    let mut row = block(&even).to_vec();
    row.extend_from_slice(&block(&odd));
    let s = raw_core::Strip { width: 32, height: 1, bits: 8, compression: 32767, offset: 0, len: 32, cfa: [0, 1, 1, 2], black: 0, white: 0, curve: None, le: true, ..Default::default() };
    let d = RowDecoder::new(&s).unwrap();
    assert_eq!(d.coding(), "craw");
    let mut out = [0u16; 32];
    d.row(&row, &mut out).unwrap();
    let curve = raw_core::decode::sony_curve(None);
    for i in 0..16 {
        assert_eq!(out[2 * i], curve[(even[i] as usize) << 1] >> 2, "even column {}", 2 * i);
        assert_eq!(out[2 * i + 1], curve[500 << 1] >> 2, "odd column {}", 2 * i + 1);
    }
}

#[test]
fn gamma22_encoder_matches_std_powf() {
    let t = color::gamma22_thresholds();
    for i in 0..=4096u32 {
        let lin = i as f64 / 4096.0;
        let want = (lin.powf(1.0 / 2.2) * 255.0).round() as i32;
        let got = color::encode(lin, &t) as i32;
        assert!((want - got).abs() <= 1, "lin={lin} want={want} got={got}");
    }
}

#[test]
fn white_balance_sources() {
    // AsShotNeutral (1/2, 1, 2/3): gains 2, 1, 1.5.
    assert_eq!(Wb::from_neutral([(1, 2), (1, 1), (2, 3)]), Some(Wb { mul: [2048, 1024, 1536], src: WbSource::AsShotNeutral }));
    assert_eq!(Wb::from_neutral([(0, 1), (1, 1), (1, 1)]), None);
    assert_eq!(Wb::from_levels(1, 1024, 1024, WbSource::Sony), None, "a gain below 1/16 is refused");
    // The SR2 cipher is an XOR stream: twice is the identity, once is not.
    let plain: Vec<u8> = (0..64u8).collect();
    let mut b = plain.clone();
    raw_core::wb::sony_decrypt(&mut b, 0x1234_5678);
    assert_ne!(b, plain);
    raw_core::wb::sony_decrypt(&mut b, 0x1234_5678);
    assert_eq!(b, plain);
    // A preview-carrying file with no SR2 names no white balance.
    let f = synth::arw(16, 16, &synth::ramp(16, 16), Coding::Plain14);
    assert_eq!(parse(&f).unwrap().wb, None);
}

#[test]
fn half_size_develop_no_preview() {
    let lv = [2048u16, 1024, 1024, 1536];
    let (w, h) = (64u32, 32u32);
    let f = synth::arw_with(w, h, &synth::grey(w, h, lv), Coding::Plain14, Opts { preview: false, wb_rggb: Some(lv), ..Opts::default() });
    let info = parse(&f).unwrap();
    assert!(info.preview.is_none(), "no embedded JPEG: Quick Look must develop");
    assert_eq!(info.ifds, 4, "the SR2Private IFD is read for the white balance, not walked");
    assert_eq!(info.wb, Some(Wb { mul: [2048, 1024, 1536], src: WbSource::Sony }), "decrypted SR2SubIFD WB_RGGBLevels");
    let s = info.strip.clone().unwrap();
    let d = RowDecoder::new(&s).unwrap();
    assert_eq!(d.coding(), "plain");
    let m = d.all(&f[s.offset as usize..(s.offset + s.len) as usize]).unwrap();
    assert_eq!(m, synth::grey(w, h, lv), "uncompressed 14-bit mosaic");
    let tone = WbTone::new(s.black, s.white, d.max_code, info.wb.as_ref()).unwrap();
    assert_eq!(tone.gain, [2048, 1024, 1536]);
    let (rgba, ow, oh) = half_rgba(&m, w as usize, h as usize, &s.cfa, &tone).unwrap();
    assert_eq!((ow, oh, rgba.len()), (32, 16, 32 * 16 * 4));
    let lin = synth::GREY_G as f64 / (synth::WHITE - synth::BLACK) as f64;
    let want = (lin.powf(1.0 / 2.2) * 255.0).round() as i32;
    for p in rgba.chunks_exact(4) {
        assert_eq!((p[0], p[1], p[3]), (p[2], p[2], 255), "white-balanced grey renders R = G = B");
        assert!((p[1] as i32 - want).abs() <= 1, "gamma 2.2: got {} want {want}", p[1]);
    }
    // Unity gains on the same scene: not grey (the camera's native RGB).
    let flat = WbTone::new(s.black, s.white, d.max_code, None).unwrap();
    let (rgba, ..) = half_rgba(&m, w as usize, h as usize, &s.cfa, &flat).unwrap();
    assert!(rgba[0] < rgba[1] && rgba[2] < rgba[1]);
    // Too small to halve, and a short mosaic: refused, never a panic.
    assert!(half_rgba(&m[..1], 1, 1, &s.cfa, &tone).is_err());
    assert_eq!(half_rgba(&m[..10], 64, 32, &s.cfa, &tone).err(), Some(Error::Truncated));
}

// ── RAWCOLOR (B724): the colour matrix, Canon CR2, Nikon NEF ──────────────────────────────────────────────────

use raw_core::synth::{Cr2Opts, NefCoding};
use raw_core::{matrix, Fmt, MatrixSource, SliceSource};

#[test]
fn colour_matrix_sources_and_inverse() {
    // The table: the synthetic ARW's ILCE-7M3 takes dcraw's first prefix match, the ILCE-7 row.
    let f = synth::arw(16, 16, &synth::ramp(16, 16), Coding::Plain14);
    let i = parse(&f).unwrap();
    assert_eq!(i.fmt, Fmt::Arw);
    assert_eq!(i.matrix_source, Some(MatrixSource::Table));
    let row7 = matrix::table(Some("SONY"), Some("ILCE-7")).unwrap();
    assert_eq!(row7, [5271, -712, -347, -6153, 13653, 2763, -1601, 2366, 7242]);
    assert_eq!(i.matrix, matrix::from_table(&row7));
    assert_eq!(matrix::table(Some("SONY"), Some("ILCE-7RM3")), matrix::table(Some("SONY"), Some("ILCE-7R")), "prefix-first, as dcraw");
    assert_eq!(matrix::table(Some("Canon"), Some("ILCE-7")), None, "Sony rows only");
    // Every carried row: rows sum to exactly Q, and rgb_cam inverts the row-normalised cam_rgb.
    for (name, row) in matrix::SONY {
        let m = matrix::from_table(row).unwrap_or_else(|| panic!("{name}"));
        let mut cam_rgb = [[0.0f64; 3]; 3];
        for r in 0..3 {
            for c in 0..3 {
                cam_rgb[r][c] = (0..3).map(|k| row[r * 3 + k] as f64 / 10000.0 * matrix::XYZ_RGB[k][c]).sum();
            }
            let n: f64 = cam_rgb[r].iter().sum();
            cam_rgb[r].iter_mut().for_each(|v| *v /= n);
        }
        for r in 0..3 {
            assert_eq!(m[r].iter().sum::<i32>(), matrix::Q, "{name} row {r}");
            for c in 0..3 {
                let p: f64 = (0..3).map(|k| m[r][k] as f64 / matrix::Q as f64 * cam_rgb[k][c]).sum();
                let want = if r == c { 1.0 } else { 0.0 };
                assert!((p - want).abs() < 4e-3, "{name} ({r},{c}) = {p}");
            }
        }
    }
    // The DNG tag outranks the table: ColorMatrix2 = the ILCE-7R row -> matrix=dng, the 7R matrix.
    let row7r = matrix::table(Some("SONY"), Some("ILCE-7R")).unwrap();
    let f = synth::arw_with(16, 16, &synth::ramp(16, 16), Coding::Plain14, Opts { color_matrix2: Some(row7r.map(|v| v as i32)), ..Opts::default() });
    let i = parse(&f).unwrap();
    assert_eq!((i.matrix_source, i.matrix), (Some(MatrixSource::Dng), matrix::from_table(&row7r)));
    assert_eq!(MatrixSource::name(i.matrix_source), "dng");
    assert_eq!(MatrixSource::name(None), "none");
    // A singular matrix is refused.
    assert_eq!(matrix::from_table(&[0; 9]), None);
}

#[test]
fn colour_matrix_develop_keeps_grey_and_saturates() {
    let lv = [2048u16, 1024, 1024, 1536];
    let (w, h) = (64u32, 32u32);
    let f = synth::arw_with(w, h, &synth::grey(w, h, lv), Coding::Plain14, Opts { preview: false, wb_rggb: Some(lv), ..Opts::default() });
    let info = parse(&f).unwrap();
    let s = info.strip.clone().unwrap();
    let d = RowDecoder::new(&s).unwrap();
    let m = d.all(&f[s.offset as usize..(s.offset + s.len) as usize]).unwrap();
    let mat = info.matrix.expect("table matrix");
    let tone = WbTone::with_matrix(s.black, s.white, d.max_code, info.wb.as_ref(), Some(&mat)).unwrap();
    let plain = WbTone::new(s.black, s.white, d.max_code, info.wb.as_ref()).unwrap();
    let (a, ..) = half_rgba(&m, w as usize, h as usize, &s.cfa, &tone).unwrap();
    let (b, ..) = half_rgba(&m, w as usize, h as usize, &s.cfa, &plain).unwrap();
    for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        assert_eq!((p[0], p[1]), (p[2], p[2]), "a white-balanced grey stays grey through the matrix (rows sum to Q)");
        assert!((p[1] as i32 - q[1] as i32).abs() <= 1);
    }
    // Camera red alone: the matrix pushes R up (its diagonal is above one) and a negative off-diagonal clips to zero.
    let red: Vec<u16> = (0..w * h).map(|i| if (i / w) % 2 == 0 && i % 2 == 0 { synth::BLACK + 4000 } else { synth::BLACK }).collect();
    let (a, ..) = half_rgba(&red, w as usize, h as usize, &s.cfa, &tone).unwrap();
    let (b, ..) = half_rgba(&red, w as usize, h as usize, &s.cfa, &WbTone::new(s.black, s.white, d.max_code, None).unwrap()).unwrap();
    assert!(mat[0][0] > matrix::Q);
    assert!(a[0] > b[0], "matrix {:?} vs plain {:?}", &a[..3], &b[..3]);
    for k in 1..3 {
        if mat[k][0] < 0 {
            assert_eq!(a[k], 0, "channel {k}");
        }
    }
}

#[test]
fn cr2_lossless_jpeg_sliced_and_plain() {
    let (w, h) = (64u32, 32u32);
    let m = synth::ramp_even(w, h);
    for slices in [None, Some([1u16, 40, 24]), Some([2, 24, 16])] {
        let f = synth::cr2(w, h, &m, Cr2Opts { preview: true, slices });
        let info = parse(&f).unwrap();
        assert_eq!(info.fmt, Fmt::Cr2);
        assert_eq!(raw_core::mime_of(&f), Some(raw_core::MIME_CR2));
        assert_eq!(info.facts.camera().as_deref(), Some(synth::MODEL_CR2));
        assert_eq!(info.matrix_source, None, "no Canon table rows: matrix=none");
        let s = info.strip.clone().unwrap();
        assert_eq!((s.width, s.height, s.bits, s.compression, s.slices), (64, 32, 14, 6, slices));
        let (po, pl) = info.preview_in(f.len() as u64).expect("IFD0's JPEG strip");
        assert_eq!(&f[po as usize..(po + pl) as usize], synth::PREVIEW_JPEG);
        let d = RowDecoder::new(&s).unwrap();
        assert_eq!((d.coding(), d.sequential(), d.row_bytes()), ("ljpeg", true, 0));
        let strip = &f[s.offset as usize..(s.offset + s.len) as usize];
        assert_eq!(d.all(strip).unwrap(), m, "slices {slices:?}");
        // A parse that never reached the strip: `open` reads the geometry from the strip's head.
        let s0 = raw_core::Strip { width: 0, height: 0, ..s.clone() };
        let d0 = RowDecoder::open(&s0, &mut SliceSource(strip)).unwrap();
        assert_eq!((d0.dims(), d0.max_code), ((64, 32), 16383));
        let mut rows = 0;
        d0.stream(&mut SliceSource(strip), 32, |y, r| {
            assert_eq!(r, &m[y * 64..(y + 1) * 64], "row {y} in order");
            rows += 1;
        })
        .unwrap();
        assert_eq!(rows, 32);
        assert!(d0.row(&[0; 8], &mut [0; 64]).is_err(), "a sequential coding has no row by itself");
    }
    // The head alone (the strip past it): still typed, geometry left to the decoder.
    let f = synth::cr2(w, h, &m, Cr2Opts { preview: false, slices: Some([1, 40, 24]) });
    let s = parse(&f).unwrap().strip.unwrap();
    let i = parse(&f[..s.offset as usize]).unwrap();
    assert_eq!((i.fmt, i.preview), (Fmt::Cr2, None));
    let st = i.strip.unwrap();
    assert_eq!((st.width, st.height), (0, 0));
    // A slice table that does not add up, and a damaged scan: refused, never a panic.
    let strip = &f[s.offset as usize..(s.offset + s.len) as usize];
    let bad = raw_core::Strip { slices: Some([1, 40, 20]), ..s.clone() };
    assert_eq!(RowDecoder::open(&bad, &mut SliceSource(strip)).err(), Some(Error::Malformed("CR2 slice table")));
    for n in [0usize, 2, 10, 40, strip.len() / 2] {
        let _ = RowDecoder::new(&s).and_then(|d| d.all(&strip[..n]));
    }
}

#[test]
fn nef_huffman_curve_and_packed() {
    let (w, h) = (64u32, 32u32);
    let m = synth::ramp_even(w, h);
    let small: Vec<u16> = m.iter().map(|v| v / 2).collect();
    for (c, want, coding, bits, tree, white) in [
        (NefCoding::Lossless14, &m, "nikon", 14u16, Some(5u8), 16383u16),
        (NefCoding::Curve12, &m, "nikon", 12, Some(0), 8190),
        (NefCoding::Packed12, &small, "packed", 12, None, 4095),
    ] {
        let f = synth::nef(w, h, want, c);
        let info = parse(&f).unwrap();
        assert_eq!(info.fmt, Fmt::Nef);
        assert_eq!(raw_core::mime_of(&f), Some(raw_core::MIME_NEF));
        let s = info.strip.clone().unwrap();
        assert_eq!((s.width, s.height, s.bits), (64, 32, bits));
        assert_eq!(s.nikon.as_ref().map(|n| n.tree), tree, "{c:?}");
        let d = RowDecoder::new(&s).unwrap();
        assert_eq!((d.coding(), d.max_code), (coding, white), "{c:?}");
        let strip = &f[s.offset as usize..(s.offset + s.len) as usize];
        assert_eq!(&d.all(strip).unwrap(), want, "{c:?}: the mosaic through the linearisation curve");
    }
    // The curve variant's table: 4096 entries, curve[i] = 2 i.
    let f = synth::nef(w, h, &m, NefCoding::Curve12);
    let n = parse(&f).unwrap().strip.unwrap().nikon.unwrap();
    assert_eq!((n.curve[1], n.curve[4095], n.vpred, n.split), (2, 8190, synth::NEF_VPRED, 0));
    // A Huffman strip whose maker note lost tag 0x96: refused by name.
    let mut s = parse(&f).unwrap().strip.unwrap();
    s.nikon = None;
    assert_eq!(RowDecoder::new(&s).err(), Some(Error::Unsupported("NEF without its linearisation table")));
    // dcraw's six trees are all canonical.
    for t in 0..6 {
        raw_core::nef::tree(t).unwrap();
    }
}
