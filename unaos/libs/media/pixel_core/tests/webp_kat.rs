// SPDX-License-Identifier: LGPL-3.0-or-later
//! M4 KATs: WebP lossless (VP8L) must be byte-identical to the `image` crate (image-webp) — lossless
//! means there is exactly one right answer. Lossy (VP8, VP8CORE SR40) is held to Chromium's answer in
//! `oracle_digests.txt`; here it must decode at the second opinion's dimensions, and the ALPH paths the
//! public files never use (raw alpha, filtering methods 1-3) are synthesised and must round-trip.

mod common;

#[test]
fn vp8l_identical_to_second_opinion() {
    let mut n = 0;
    let mut lossy = 0;
    for name in common::list("webp", ".webp") {
        let b = common::load(&format!("webp/{name}")).unwrap();
        let img = pixel_core::decode(&b).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (w, h, want) = common::reference(&b).unwrap();
        assert_eq!((img.width, img.height), (w, h), "{name}");
        if name.starts_with("lossy") {
            lossy += 1;
            continue;
        }
        let (m, cnt) = common::diff(&img.rgba, &want);
        assert_eq!(cnt, 0, "{name}: {cnt} bytes differ (max {m})");
        n += 1;
    }
    eprintln!("vp8l: {n} files identical; lossy: {lossy} decoded (pixels pinned to Chromium in oracle_digests)");
}

/// Rebuild a lossy+alpha file with its ALPH chunk replaced by `alph`.
fn with_alph(file: &[u8], alph: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut p = 12;
    while p + 8 <= file.len() {
        let n = u32::from_le_bytes(file[p + 4..p + 8].try_into().unwrap()) as usize;
        let cc = &file[p..p + 4];
        let d = if cc == b"ALPH" { alph } else { &file[p + 8..p + 8 + n] };
        body.extend_from_slice(cc);
        body.extend_from_slice(&(d.len() as u32).to_le_bytes());
        body.extend_from_slice(d);
        if d.len() & 1 == 1 {
            body.push(0);
        }
        p += 8 + n + (n & 1);
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(4 + body.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    out
}

#[test]
fn alph_raw_and_every_filter_round_trip() {
    let Some(b) = common::load("webp/lossy_alpha_4.webp") else { return };
    let orig = pixel_core::decode(&b).unwrap();
    let (w, h) = (orig.width as usize, orig.height as usize);
    let a: Vec<u8> = orig.rgba.chunks_exact(4).map(|p| p[3]).collect();
    assert!(a.iter().any(|&v| v > 0 && v < 255), "fixture has partial alpha");
    // RFC 9649 §2.7 predictors: (0,0) from 0, row 0 from the left, column 0 from above.
    let pred = |x: usize, y: usize, m: u8| -> u8 {
        match (x, y) {
            (0, 0) => 0,
            (_, 0) => a[x - 1],
            (0, _) => a[(y - 1) * w],
            _ => {
                let (l, t, tl) = (a[y * w + x - 1] as i32, a[(y - 1) * w + x] as i32, a[(y - 1) * w + x - 1] as i32);
                match m {
                    1 => l as u8,
                    2 => t as u8,
                    _ => (l + t - tl).clamp(0, 255) as u8,
                }
            }
        }
    };
    for m in 0u8..4 {
        let mut alph = vec![m << 2];
        for y in 0..h {
            for x in 0..w {
                let v = a[y * w + x];
                alph.push(if m == 0 { v } else { v.wrapping_sub(pred(x, y, m)) });
            }
        }
        let img = pixel_core::decode(&with_alph(&b, &alph)).unwrap();
        assert_eq!(img.rgba, orig.rgba, "raw alpha, filtering method {m}");
    }
}
