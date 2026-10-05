// SPDX-License-Identifier: LGPL-3.0-or-later
//! OPENERS (rmbp-ledger B379): `mime_of` over the builder's test-f samples (builder/testf.list; fetched into
//! `unaos/target/testf/` or `$UNAOS_TESTF_DIR`, never committed), and every image sample DECODES (BMP, all three
//! WebPs, JPEG, GIF, PNG) through the same `decode` facet calls. Absent samples are SKIPPED out loud.
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::var_os("UNAOS_TESTF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/testf"))
}

#[test]
fn testf_images_typed_and_decoded() {
    let cases = [
        ("TEST.PNG", "image/png"),
        ("APNG.PNG", "image/png"),
        ("TEST.JPG", "image/jpeg"),
        ("TEST.GIF", "image/gif"),
        ("TEST.BMP", "image/bmp"),
        ("LOSSLESS.WEBP", "image/webp"),
        ("LOSSY.WEBP", "image/webp"),
        ("ANIM.WEBP", "image/webp"),
    ];
    let mut seen = 0;
    for (name, want) in cases {
        let Ok(b) = std::fs::read(dir().join(name)) else {
            eprintln!("SKIP {name}: not fetched (unaos/target/testf)");
            continue;
        };
        seen += 1;
        assert_eq!(pixel_core::mime_of(&b), Some(want), "{name}");
        let img = pixel_core::decode(&b).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(img.width > 0 && img.height > 0, "{name}");
        if name == "TEST.BMP" {
            assert_eq!((img.width, img.height), (127, 64), "rgb24.bmp is 127x64");
        }
        if name == "ANIM.WEBP" {
            assert!(img.frames.as_ref().map_or(0, |f| f.len()) > 1, "ANIM.WEBP decodes as an animation");
        }
    }
    eprintln!("mime_testf: {seen}/8 samples present");
}

#[test]
fn bmp_needs_more_than_bm() {
    // A text file that starts "BM": no reserved-zero words, no DIB size.
    assert_eq!(pixel_core::mime_of(b"BMW and Mini sales, quarterly, 2026, as a plain text note\n"), None);
    let mut h = vec![0u8; 54];
    h[..2].copy_from_slice(b"BM");
    h[14] = 40;
    assert_eq!(pixel_core::mime_of(&h), Some("image/bmp"));
    h[14] = 41;
    assert_eq!(pixel_core::mime_of(&h), None);
}
