// SPDX-License-Identifier: LGPL-3.0-or-later
//! M4 KATs: WebP lossless (VP8L) must be byte-identical to the `image` crate (image-webp) — lossless
//! means there is exactly one right answer — and lossy VP8 must be refused by name.

mod common;

#[test]
fn vp8l_identical_to_second_opinion() {
    let mut n = 0;
    for name in common::list("webp", ".webp") {
        let b = common::load(&format!("webp/{name}")).unwrap();
        let ours = pixel_core::decode(&b);
        if name.starts_with("lossy") {
            assert_eq!(ours, Err(pixel_core::Error::Unsupported("webp lossy (VP8)")), "{name}");
            continue;
        }
        let img = ours.unwrap_or_else(|e| panic!("{name}: {e}"));
        let (w, h, want) = common::reference(&b).unwrap();
        assert_eq!((img.width, img.height), (w, h), "{name}");
        let (m, cnt) = common::diff(&img.rgba, &want);
        assert_eq!(cnt, 0, "{name}: {cnt} bytes differ (max {m})");
        n += 1;
    }
    eprintln!("vp8l: {n} files identical");
}
