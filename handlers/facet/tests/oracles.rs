// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Facet's pieces against INDEPENDENT implementations (the oracles), and the eyes suite's goldens
//! (themselves proven against Chromium, see tools/eyes/suites/facet/suite.toml) against the CLI.

#![cfg(feature = "chicken-wire-image")]

use facet::raster::Raster;
use facet::source::{ImageCrateSource, ImageSource};

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
        let d = ImageCrateSource.decode(&png).expect("independent decoder accepts Facet's PNG");
        assert_eq!((d.width, d.height), (w, h));
        assert_eq!(d.rgba, r.rgba, "{w}x{h}: pixels survive Facet's writer bit-exact");
    }
    // The LZ77 path earns its keep: a flat 256x256 frame compresses far below raw.
    let flat = Raster::filled(256, 256, [9, 9, 9, 255]);
    let png = facet::png::encode(256, 256, &flat.rgba);
    assert!(png.len() < 4096, "flat 256x256 encoded to {} bytes", png.len());
    assert_eq!(ImageCrateSource.decode(&png).unwrap().rgba, flat.rgba);
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
    let raw = ImageCrateSource.decode(&std::fs::read(fx.join("photo-o6.jpg")).unwrap()).unwrap();
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
    let back = ImageCrateSource.decode(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(Raster::new(back.width, back.height, back.rgba), f.baked(h).unwrap());
    // Reset: back to the original, and the reset itself undoes.
    f.edit(h, &Reset).unwrap();
    assert_eq!(f.baked(h).unwrap(), original);
    assert_eq!(f.edit(h, &Undo).unwrap().edits, 4);
}

#[test]
fn eyes_suite_goldens() {
    let sd = suite_dir();
    let suite: toml::Table = std::fs::read_to_string(sd.join("suite.toml")).unwrap().parse().unwrap();
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
        let decode = |p: &std::path::Path| {
            let d = ImageCrateSource.decode(&std::fs::read(p).unwrap()).unwrap();
            Raster::new(d.width, d.height, d.rgba)
        };
        let (ours, golden) = (decode(&out), decode(&sd.join(format!("goldens/{name}.png"))));
        assert_eq!((ours.width, ours.height), (golden.width, golden.height), "{name}: size");
        let d = facet::compare(&ours, &golden);
        let max = c.get("max_delta").and_then(|v| v.as_integer()).unwrap_or(0) as u8;
        assert!(d.max_delta <= max, "{name}: {d:?} (allowed {max})");
    }
}
