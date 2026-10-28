// SPDX-License-Identifier: LGPL-3.0-or-later
//! PDFICC (LEDGER SR358) KATs: pixel_core SURFACES an embedded ICC profile as `Image::icc` — PNG iCCP (inflated; a cICP
//! outranks it), JPEG APP2 ICC_PROFILE (chunks joined in sequence order), WebP ICCP (after a VP8X with the ICC flag) —
//! and never applies it: the pixels of a tagged file are the pixels of the same file untagged. The test files are made
//! here (the `image` crate's encoders, test-side only) and the profile is icc_core's `kat::adobe_rgb_v2`.

use std::io::Cursor;

fn pattern() -> image::RgbaImage {
    image::RgbaImage::from_fn(8, 8, |x, y| image::Rgba([(x * 30) as u8, (y * 30) as u8, 200, 255]))
}

fn encoded(fmt: image::ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    match fmt {
        image::ImageFormat::Jpeg => image::DynamicImage::ImageRgba8(pattern()).to_rgb8().write_to(&mut out, fmt).unwrap(),
        _ => pattern().write_to(&mut out, fmt).unwrap(),
    }
    out.into_inner()
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend(kind);
    c.extend(data);
    let crc = pixel_core::png::encode::crc32(&c[4..]);
    c.extend(crc.to_be_bytes());
    c
}

/// A PNG with `chunks` inserted right after IHDR (signature 8 + IHDR 25 bytes).
fn png_with(chunks: &[Vec<u8>]) -> Vec<u8> {
    let png = encoded(image::ImageFormat::Png);
    let mut out = png[..33].to_vec();
    for c in chunks {
        out.extend(c);
    }
    out.extend(&png[33..]);
    out
}

fn iccp(profile: &[u8]) -> Vec<u8> {
    let mut d = b"Adobe RGB (1998)\0\0".to_vec();
    d.extend(pixel_core::deflate::compress(profile).unwrap());
    png_chunk(b"iCCP", &d)
}

#[test]
fn png_iccp_surfaces() {
    let profile = icc_core::kat::adobe_rgb_v2();
    let tagged = pixel_core::decode(&png_with(&[iccp(&profile)])).unwrap();
    let plain = pixel_core::decode(&encoded(image::ImageFormat::Png)).unwrap();
    assert_eq!(tagged.icc.as_deref(), Some(&profile[..]));
    assert_eq!((tagged.width, tagged.height, &tagged.rgba), (plain.width, plain.height, &plain.rgba), "surfaced, not applied");
    // The profile the decoder hands back is one icc_core reads.
    assert!(icc_core::Profile::parse(tagged.icc.as_deref().unwrap()).is_ok());
}

#[test]
fn png_cicp_wins_over_iccp() {
    let profile = icc_core::kat::adobe_rgb_v2();
    let img = pixel_core::decode(&png_with(&[png_chunk(b"cICP", &[1, 13, 0, 1]), iccp(&profile)])).unwrap();
    assert_eq!(img.icc, None, "cICP outranks iCCP (PNG 3rd ed. §11.3.2.6)");
    // A broken iCCP (method byte 1) is dropped as an ancillary chunk, never an error.
    let mut bad = b"x\0\x01".to_vec();
    bad.extend(pixel_core::deflate::compress(&profile).unwrap());
    assert_eq!(pixel_core::decode(&png_with(&[png_chunk(b"iCCP", &bad)])).unwrap().icc, None);
}

#[test]
fn jpeg_app2_chunks_join_in_order() {
    let profile = icc_core::kat::adobe_rgb_v2();
    let (a, b) = profile.split_at(profile.len() / 2);
    let app2 = |seq: u8, part: &[u8]| {
        let mut s = vec![0xFF, 0xE2];
        s.extend(((part.len() + 16) as u16).to_be_bytes());
        s.extend(b"ICC_PROFILE\0");
        s.extend([seq, 2]);
        s.extend(part);
        s
    };
    let jpeg = encoded(image::ImageFormat::Jpeg);
    let mut tagged = jpeg[..2].to_vec();
    tagged.extend(app2(2, b)); // written out of order on purpose
    tagged.extend(app2(1, a));
    tagged.extend(&jpeg[2..]);
    let img = pixel_core::decode(&tagged).unwrap();
    let plain = pixel_core::decode(&jpeg).unwrap();
    assert_eq!(img.icc.as_deref(), Some(&profile[..]));
    assert_eq!(img.rgba, plain.rgba, "surfaced, not applied");
}

#[test]
fn webp_iccp_surfaces() {
    let profile = icc_core::kat::adobe_rgb_v2();
    let webp = encoded(image::ImageFormat::WebP);
    // The encoder's lossless file: RIFF size WEBP, then one VP8L chunk.
    assert_eq!(&webp[12..16], b"VP8L");
    let chunk = |fourcc: &[u8; 4], d: &[u8]| {
        let mut c = fourcc.to_vec();
        c.extend((d.len() as u32).to_le_bytes());
        c.extend(d);
        if d.len() % 2 == 1 {
            c.push(0);
        }
        c
    };
    let mut vp8x = vec![0x20, 0, 0, 0]; // the ICC flag
    vp8x.extend([7, 0, 0, 7, 0, 0]); // canvas 8 x 8 (minus one, 24-bit LE)
    let mut body = b"WEBP".to_vec();
    body.extend(chunk(b"VP8X", &vp8x));
    body.extend(chunk(b"ICCP", &profile));
    body.extend(&webp[12..]);
    let mut file = b"RIFF".to_vec();
    file.extend((body.len() as u32).to_le_bytes());
    file.extend(body);
    let img = pixel_core::decode(&file).unwrap();
    let plain = pixel_core::decode(&webp).unwrap();
    assert_eq!(img.icc.as_deref(), Some(&profile[..]));
    assert_eq!(img.rgba, plain.rgba, "surfaced, not applied");
}

#[test]
fn untagged_images_keep_icc_none_and_pins() {
    for fmt in [image::ImageFormat::Png, image::ImageFormat::Jpeg, image::ImageFormat::WebP, image::ImageFormat::Gif, image::ImageFormat::Bmp] {
        let img = pixel_core::decode(&encoded(fmt)).unwrap();
        assert_eq!(img.icc, None, "{fmt:?}");
    }
    // The in-tree WebP fixtures carry a REAL profile: the 456-byte v4 sRGB (para curves) libwebp's tools embed. It is
    // surfaced, and icc_core's content check calls it sRGB, so every consumer's open of these files stays byte-identical
    // (their digests stay pinned in webp_kat.rs).
    for name in ["cat-ll.webp", "grumpycat-ll.webp", "pal8v5-ll.webp"] {
        let b = std::fs::read(format!("{}/tests/fixtures/webp/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let icc = pixel_core::decode(&b).unwrap().icc.expect(name);
        assert_eq!(icc.len(), 456, "{name}");
        assert!(icc_core::Profile::parse(&icc).unwrap().is_srgb(), "{name}");
    }
}

// KERNEL-PNG-ICC / KERNEL-ANIM-ICC (rmbp-ledger B974): the streaming faces carry the profile `decode` reports — the
// kernel's streaming PNG reads iCCP by `png::iccp_profile` (the decoder's own rule), the kernel's animation player
// builds its one transform from `Animation::icc`, and `decode_first_frame` keeps the profile (a still WebP's ICCP was
// dropped there before).

fn facetanim(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/facetanim/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// `webp` (VP8X first, at 12) with the ICC flag set and an ICCP chunk inserted directly after VP8X.
fn webp_tagged(webp: &[u8], profile: &[u8]) -> Vec<u8> {
    assert_eq!(&webp[12..16], b"VP8X");
    let vend = 20 + u32::from_le_bytes([webp[16], webp[17], webp[18], webp[19]]) as usize;
    let mut c = b"ICCP".to_vec();
    c.extend((profile.len() as u32).to_le_bytes());
    c.extend(profile);
    if profile.len() % 2 == 1 {
        c.push(0);
    }
    let mut out = webp[..vend].to_vec();
    out[20] |= 0x20;
    out.extend(&c);
    out.extend(&webp[vend..]);
    let riff = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff.to_le_bytes());
    out
}

#[test]
fn animation_surfaces_icc() {
    let profile = icc_core::kat::adobe_rgb_v2();
    // APNG: iCCP right after IHDR.
    let apng = facetanim("ANIM3.PNG");
    let mut tagged_png = apng[..33].to_vec();
    tagged_png.extend(iccp(&profile));
    tagged_png.extend(&apng[33..]);
    // Animated WebP: ICCP after VP8X, the flag set.
    let tagged_webp = webp_tagged(&facetanim("ANIM3.WEBP"), &profile);
    for (name, plain, tagged) in [("apng", apng.clone(), tagged_png), ("webp", facetanim("ANIM3.WEBP"), tagged_webp)] {
        let mut a = pixel_core::Animation::new(&tagged[..]).unwrap();
        assert_eq!(a.icc(), Some(&profile[..]), "{name}: Animation::icc");
        assert_eq!(pixel_core::decode(&tagged).unwrap().icc.as_deref(), Some(&profile[..]), "{name}: decode agrees");
        assert_eq!(pixel_core::Animation::new(&plain[..]).unwrap().icc(), None, "{name}: untagged");
        // Surfaced, not applied: every frame's canvas is the untagged file's.
        let mut b = pixel_core::Animation::new(&plain[..]).unwrap();
        while let (Some(fa), Some(fb)) = (a.next_frame(), b.next_frame()) {
            assert_eq!(fa.unwrap(), fb.unwrap(), "{name}");
            assert!(a.canvas() == b.canvas(), "{name}: canvas moved");
        }
        let f0 = pixel_core::decode_first_frame(&tagged).unwrap();
        assert_eq!(f0.icc.as_deref(), Some(&profile[..]), "{name}: decode_first_frame keeps the profile");
        assert!(f0.rgba == pixel_core::decode_first_frame(&plain).unwrap().rgba, "{name}: rgba unchanged");
    }
    assert_eq!(pixel_core::Animation::new(&facetanim("ANIM3.GIF")[..]).unwrap().icc(), None, "GIF carries none");
}

#[test]
fn decode_first_frame_keeps_the_still_profile() {
    let profile = icc_core::kat::adobe_rgb_v2();
    // A still PNG and a still VP8X WebP (the WebP ICCP decode_first_frame used to drop).
    let png = png_with(&[iccp(&profile)]);
    let webp = encoded(image::ImageFormat::WebP);
    assert_eq!(&webp[12..16], b"VP8L");
    let mut vp8x = b"VP8X".to_vec();
    vp8x.extend(10u32.to_le_bytes());
    vp8x.extend([0, 0, 0, 0, 7, 0, 0, 7, 0, 0]);
    let mut still_webp = webp[..12].to_vec();
    still_webp.extend(vp8x);
    still_webp.extend(&webp[12..]);
    let still_webp = webp_tagged(&still_webp, &profile);
    for (name, b) in [("png", png), ("webp", still_webp)] {
        let f0 = pixel_core::decode_first_frame(&b).unwrap();
        let d = pixel_core::decode(&b).unwrap();
        assert_eq!(f0.icc.as_deref(), Some(&profile[..]), "{name}");
        assert_eq!(f0.icc, d.icc, "{name}: decode agrees");
        assert!(f0.rgba == d.rgba, "{name}");
    }
}

#[test]
fn iccp_profile_is_the_decoders_rule() {
    let profile = icc_core::kat::adobe_rgb_v2();
    let chunk = iccp(&profile);
    let data = &chunk[8..chunk.len() - 4];
    assert_eq!(pixel_core::png::iccp_profile(data).as_deref(), Some(&profile[..]));
    assert_eq!(pixel_core::png::iccp_profile(data), pixel_core::decode(&png_with(&[chunk.clone()])).unwrap().icc);
    let mut bad = b"x\0\x01".to_vec();
    bad.extend(pixel_core::deflate::compress(&profile).unwrap());
    assert_eq!(pixel_core::png::iccp_profile(&bad), None, "method 1");
    assert_eq!(pixel_core::png::iccp_profile(b"no nul at all"), None);
}
