// SPDX-License-Identifier: LGPL-3.0-or-later
//! ATTRCOLUMNS (rmbp-ledger B402): `facts_of` over the test-f images (`$UNAOS_TESTF_DIR` or `unaos/target/testf`),
//! cross-checked against this core's own decoder: the header's size IS the decoded size, and `animated` IS
//! "decodes to more than one frame". Absent samples are SKIPPED out loud.
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::var_os("UNAOS_TESTF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/testf"))
}

#[test]
fn testf_image_facts_match_the_decoder() {
    let mut seen = 0;
    for name in ["TEST.PNG", "APNG.PNG", "TEST.JPG", "TEST.GIF", "TEST.BMP", "LOSSLESS.WEBP", "LOSSY.WEBP", "ANIM.WEBP"] {
        let Ok(b) = std::fs::read(dir().join(name)) else {
            eprintln!("SKIP {name}: not fetched");
            continue;
        };
        seen += 1;
        let f = pixel_core::facts_of(&b).unwrap_or_else(|| panic!("{name}: no facts"));
        let img = pixel_core::decode(&b).unwrap_or_else(|e| panic!("{name}: {e}"));
        let frames = img.frames.as_ref().map_or(1, |f| f.len());
        eprintln!("{name}: {}x{} animated={} (decoded {}x{} frames={})", f.width, f.height, f.animated, img.width, img.height, frames);
        assert_eq!((f.width, f.height), (img.width, img.height), "{name}");
        assert_eq!(f.animated, frames > 1, "{name}");
    }
    eprintln!("image facts: {seen} samples");
}

#[test]
fn generated_facts() {
    // The builder's generated samples (builder/src/main.rs `testf_generated`): QOI 64x64, SVG 128x128.
    let mut q = b"qoif".to_vec();
    q.extend_from_slice(&64u32.to_be_bytes());
    q.extend_from_slice(&64u32.to_be_bytes());
    q.extend_from_slice(&[4, 0]);
    assert_eq!(pixel_core::facts_of(&q), Some(pixel_core::ImageFacts { width: 64, height: 64, animated: false }));
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"128\" height=\"128\" viewBox=\"0 0 128 128\">\n</svg>\n";
    assert_eq!(pixel_core::facts_of(svg).map(|f| (f.width, f.height)), Some((128, 128)));
    let vb = b"<svg viewBox=\"0 0 300 150\"></svg>";
    assert_eq!(pixel_core::facts_of(vb).map(|f| (f.width, f.height)), Some((300, 150)));
    assert_eq!(pixel_core::facts_of(b"BM not an image at all, just text"), None);
    assert_eq!(pixel_core::facts_of(b"plain text"), None);
}
