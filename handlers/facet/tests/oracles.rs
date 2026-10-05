// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Facet's pieces against INDEPENDENT implementations (the oracles), and the eyes suite's goldens
//! (themselves proven against Chromium, see tools/eyes/suites/facet/cases/facet.toml) against the CLI.
//!
//! The `image` crate appears here ONLY as a second opinion (a dev-dependency, never linked into Facet):
//! Facet decodes through `PixelCoreSource`; [`oracle_decode`] is someone else's decoder to check it by.

use facet::raster::Raster;
use facet::source::{ImageSource, PixelCoreSource};

/// The independent decoder (the `image` crate): straight RGBA8, no orientation applied.
fn oracle_decode(bytes: &[u8]) -> Raster {
    let img = image::load_from_memory(bytes).expect("the oracle decodes it").to_rgba8();
    let (w, h) = img.dimensions();
    Raster::new(w, h, img.into_raw())
}

/// A deterministic picture with flat areas, ramps, noise and partial alpha.
fn busy(w: u32, h: u32) -> Raster {
    let mut s = 0x1234_5678u32;
    let mut v = Vec::new();
    for y in 0..h {
        for x in 0..w {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let p = if y < h / 3 {
                [(x * 255 / w) as u8, (y * 255 / h) as u8, 128, 255]
            } else if y < 2 * h / 3 {
                [s as u8, (s >> 8) as u8, (s >> 16) as u8, 255]
            } else {
                [200, 30, 90, (x * 255 / w) as u8]
            };
            v.extend(p);
        }
    }
    Raster::new(w, h, v)
}

fn suite_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/eyes/suites/facet")
}

#[test]
fn png_writer_round_trips_through_an_independent_decoder() {
    for (w, h) in [(1, 1), (2, 3), (17, 5), (300, 200), (1024, 3)] {
        let r = busy(w, h);
        let png = facet::png::encode(w, h, &r.rgba);
        let d = oracle_decode(&png);
        assert_eq!((d.width, d.height), (w, h));
        assert_eq!(d.rgba, r.rgba, "{w}x{h}: pixels survive Facet's writer bit-exact");
        assert_eq!(PixelCoreSource.decode(&png).unwrap().rgba, r.rgba, "{w}x{h}: and through pixel_core");
    }
    // The LZ77 path earns its keep: a flat 256x256 frame compresses far below raw.
    let flat = Raster::filled(256, 256, [9, 9, 9, 255]);
    let png = facet::png::encode(256, 256, &flat.rgba);
    assert!(png.len() < 4096, "flat 256x256 encoded to {} bytes", png.len());
    assert_eq!(oracle_decode(&png).rgba, flat.rgba);
}

#[test]
fn triangle_resize_matches_an_independent_triangle_filter() {
    // Opaque input (the image crate filters straight RGBA; premultiplication is the identity here).
    let mut r = busy(120, 90);
    for p in r.rgba.chunks_exact_mut(4) {
        p[3] = 255;
    }
    let img = image::RgbaImage::from_raw(r.width, r.height, r.rgba.clone()).unwrap();
    for (dw, dh) in [(60, 45), (37, 23), (240, 180), (121, 89), (7, 300)] {
        let ours = r.resized(dw, dh);
        let theirs = image::imageops::resize(&img, dw, dh, image::imageops::FilterType::Triangle);
        let d = facet::compare(&ours, &Raster::new(dw, dh, theirs.into_raw()));
        assert!(d.psnr_db > 45.0 && d.max_delta <= 3, "{dw}x{dh}: {d:?}");
    }
}

#[test]
fn exif_orientation_is_applied_on_open() {
    let fx = suite_dir().join("fixtures");
    let mut f = facet::Facet::new();
    let (h6, i6) = f.open(fx.join("photo-o6.jpg").to_str().unwrap()).unwrap();
    assert_eq!((i6.source_width, i6.source_height, i6.width, i6.height, i6.orientation), (48, 32, 32, 48, 6));
    // The stored pixels, decoded without orientation, then turned by Facet's own mapping.
    let raw = PixelCoreSource.decode(&std::fs::read(fx.join("photo-o6.jpg")).unwrap()).unwrap();
    let stored = Raster::new(raw.width, raw.height, raw.rgba);
    assert_eq!(f.baked(h6).unwrap(), stored.oriented(6));
    assert_eq!(stored.oriented(6), stored.rotated(1), "orientation 6 = one clockwise quarter turn");
    let (h5, i5) = f.open(fx.join("photo-o5.jpg").to_str().unwrap()).unwrap();
    assert_eq!((i5.width, i5.height, i5.orientation), (32, 48, 5));
    // Orientation 5 is the transpose: stored (x, y) shows at (y, x).
    let b5 = f.baked(h5).unwrap();
    assert_eq!(b5.px(3, 40), stored.px(40, 3));
}

#[test]
fn edits_are_non_destructive_and_export_png() {
    let fx = suite_dir().join("fixtures");
    let mut f = facet::Facet::new();
    let (h, info) = f.open(fx.join("card.png").to_str().unwrap()).unwrap();
    assert_eq!((info.width, info.height, info.has_alpha, info.colour.as_str()), (48, 32, true, "sRGB chunk"));
    let original = f.baked(h).unwrap();
    use bandy::signals::FacetEdit::*;
    f.edit(h, &Crop { x: 8, y: 4, width: 24, height: 20 }).unwrap();
    f.edit(h, &Rotate { quarter_turns: 1 }).unwrap();
    f.edit(h, &Resize { width: 40, height: 48 }).unwrap();
    let i = f.edit(h, &Adjust { brightness: 1.1, contrast: 0.9 }).unwrap();
    assert_eq!((i.width, i.height, i.edits), (40, 48, 4));
    assert!(f.edit(h, &Crop { x: 0, y: 0, width: 41, height: 1 }).is_err(), "crop checked against the edited size");
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("e.png");
    let n = f.export(h, out.to_str().unwrap(), bandy::signals::FacetFormat::Png, false).unwrap();
    assert_eq!(n, std::fs::metadata(&out).unwrap().len());
    assert!(f.export(h, out.to_str().unwrap(), bandy::signals::FacetFormat::Png, false).is_err(), "no silent overwrite");
    assert!(f.export(h, out.to_str().unwrap(), bandy::signals::FacetFormat::Jpeg, true).is_err(), "JPEG export owed");
    assert_eq!(oracle_decode(&std::fs::read(&out).unwrap()), f.baked(h).unwrap());
    // Reset: back to the original, and the reset itself undoes.
    f.edit(h, &Reset).unwrap();
    assert_eq!(f.baked(h).unwrap(), original);
    assert_eq!(f.edit(h, &Undo).unwrap().edits, 4);
}

#[test]
fn eyes_suite_goldens() {
    let sd = suite_dir();
    let suite: toml::Table = std::fs::read_to_string(sd.join("cases/facet.toml")).unwrap().parse().unwrap();
    let cases = suite["case"].as_array().unwrap();
    assert!(cases.len() >= 10);
    let dir = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_facet");
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let out = dir.path().join(format!("{name}.png"));
        let run = c["subject"]["run"]
            .as_str()
            .unwrap()
            .replace("{repo}/target/release/facet", bin)
            .replace("{suite}", sd.to_str().unwrap())
            .replace("{out}", out.to_str().unwrap());
        let st = std::process::Command::new("sh").arg("-c").arg(&run).status().unwrap();
        assert!(st.success(), "{name}: {run}");
        let decode = |p: &std::path::Path| oracle_decode(&std::fs::read(p).unwrap());
        let (ours, golden) = (decode(&out), decode(&sd.join(format!("goldens/{name}.png"))));
        assert_eq!((ours.width, ours.height), (golden.width, golden.height), "{name}: size");
        let d = facet::compare(&ours, &golden);
        let max = c.get("max_delta").and_then(|v| v.as_integer()).unwrap_or(0) as u8;
        assert!(d.max_delta <= max, "{name}: {d:?} (allowed {max})");
    }
}

#[test]
fn pixel_core_agrees_with_the_second_opinion() {
    // Lossless formats: bit-exact against the independent decoder. JPEG: IDCTs legitimately differ
    // (pixel_core matches Chromium's; the eyes suite proves that), so within a small delta.
    let fx = suite_dir().join("fixtures");
    let card = std::fs::read(fx.join("card.png")).unwrap();
    let ours = PixelCoreSource.decode(&card).unwrap();
    assert_eq!(Raster::new(ours.width, ours.height, ours.rgba), oracle_decode(&card));
    for name in ["photo-o5.jpg", "photo-o6.jpg"] {
        let b = std::fs::read(fx.join(name)).unwrap();
        let ours = PixelCoreSource.decode(&b).unwrap();
        let d = facet::compare(&Raster::new(ours.width, ours.height, ours.rgba), &oracle_decode(&b));
        assert!(d.max_delta <= 3 && d.psnr_db > 50.0, "{name}: {d:?}");
    }
}

/// A baseline JPEG built from Facet's own fixture with its APP1 Exif segment replaced by one carrying
/// `orientation` in the given byte order (EXIF 2.3 §4.5.4: APP1 = "Exif\0\0" + a TIFF stream).
fn with_exif(jpeg: &[u8], orientation: u16, big_endian: bool) -> Vec<u8> {
    let mut tiff = Vec::new();
    let (w16, w32): (fn(u16) -> [u8; 2], fn(u32) -> [u8; 4]) =
        if big_endian { (u16::to_be_bytes, u32::to_be_bytes) } else { (u16::to_le_bytes, u32::to_le_bytes) };
    tiff.extend(if big_endian { *b"MM" } else { *b"II" });
    tiff.extend(w16(42));
    tiff.extend(w32(8));
    tiff.extend(w16(1)); // one IFD0 entry
    tiff.extend(w16(0x0112));
    tiff.extend(w16(3)); // SHORT
    tiff.extend(w32(1));
    tiff.extend(w16(orientation));
    tiff.extend([0, 0]);
    tiff.extend(w32(0));
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend(tiff);
    // Walk the marker segments up to SOS, dropping every APP1 and inserting ours after SOI.
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend(((app1.len() + 2) as u16).to_be_bytes());
    out.extend(&app1);
    let mut i = 2;
    while i + 4 <= jpeg.len() && jpeg[i] == 0xFF && jpeg[i + 1] != 0xDA {
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        if jpeg[i + 1] != 0xE1 {
            out.extend(&jpeg[i..i + 2 + len]);
        }
        i += 2 + len;
    }
    out.extend(&jpeg[i..]);
    out
}

#[test]
fn exif_orientation_facet_reader_agrees_with_pixel_core() {
    // Facet's own reader (meta.rs) is the authority it applies; pixel_core reports what IT found.
    // Two from-spec readers, written separately, must agree on every JPEG.
    let fx = suite_dir().join("fixtures");
    for (name, want) in [("photo-o5.jpg", 5), ("photo-o6.jpg", 6)] {
        let b = std::fs::read(fx.join(name)).unwrap();
        let meta = facet::meta::read(&b, facet::source::Format::Jpeg);
        assert_eq!((meta.orientation, PixelCoreSource.decode(&b).unwrap().orientation), (want, want), "{name}");
    }
    let base = std::fs::read(fx.join("photo-o6.jpg")).unwrap();
    let pixels = PixelCoreSource.decode(&base).unwrap().rgba;
    for big in [false, true] {
        for o in 0..=10u16 {
            let b = with_exif(&base, o, big);
            let meta = facet::meta::read(&b, facet::source::Format::Jpeg).orientation;
            let d = PixelCoreSource.decode(&b).unwrap();
            let want = if (1..=8).contains(&o) { o as u8 } else { 1 };
            assert_eq!(meta, want, "Facet's reader, orientation {o}, big-endian {big}");
            assert_eq!(d.orientation, meta, "pixel_core vs Facet, orientation {o}, big-endian {big}");
            assert_eq!(d.rgba, pixels, "the Exif segment changes no pixel");
        }
    }
    // No Exif at all: both say 1.
    let card = std::fs::read(fx.join("card.png")).unwrap();
    assert_eq!(PixelCoreSource.decode(&card).unwrap().orientation, 1);
}

#[test]
fn refusals_name_the_format_and_never_fall_back() {
    let mut f = facet::Facet::new();
    assert_eq!(f.source_name(), "pixel_core");
    let mut lossy = b"RIFF\x1A\0\0\0WEBPVP8 \x0E\0\0\0".to_vec();
    lossy.extend([0x30, 0x01, 0x00, 0x9D, 0x01, 0x2A, 0x01, 0x00, 0x01, 0x00, 0, 0, 0, 0]);
    let mut avif = vec![0, 0, 0, 0x1C];
    avif.extend(b"ftypavif\0\0\0\0avifmif1miaf");
    for (bytes, name) in [
        (b"II*\0\x08\0\0\0\0\0".to_vec(), "tiff"),
        (b"\0\0\x01\0\x01\0\x10\x10\0\0".to_vec(), "ico"),
        (avif, "avif"),
        (lossy, "webp"),
    ] {
        let e = f.open_bytes("x", &bytes).unwrap_err();
        assert!(e.starts_with(&format!("x: {name}: ")), "{e}");
    }
    assert_eq!(f.open_bytes("x", b"hello").unwrap_err(), "x: unknown format");
}
