// SPDX-License-Identifier: LGPL-3.0-or-later
//! M2 KATs: JPEG against the `image` crate (zune-jpeg) as a second opinion — a DIFFERENT IDCT and
//! upsampler, so the bar is PSNR, not identity (identity is held against Chromium in
//! `oracle_digests.rs`) — plus the refusals and the EXIF orientation geometry.

mod common;

#[test]
fn jpeg_second_opinion_psnr() {
    let mut n = 0;
    for name in common::list("jpeg", ".jpg") {
        let bytes = common::load(&format!("jpeg/{name}")).unwrap();
        let Ok(img) = pixel_core::decode(&bytes) else { continue };
        if name.starts_with("jpg-cmyk") || name == "ycck.jpg" {
            continue; // CMYK conventions differ between decoders (no ICC either side).
        }
        let Some((w, h, want)) = common::reference(&bytes) else { continue };
        assert_eq!((img.width, img.height), (w, h), "{name}");
        let p = common::psnr_rgb(&img.rgba, &want);
        eprintln!("{name}: psnr vs image crate {p:.2} dB");
        assert!(p >= 30.0, "{name}: PSNR {p:.2} dB vs image crate");
        n += 1;
    }
    eprintln!("jpeg second opinion: {n} files");
}

#[test]
fn jpeg_refusals_and_orientation() {
    if let Some(b) = common::load("jpeg/testimgari.jpg") {
        assert_eq!(pixel_core::decode(&b), Err(pixel_core::Error::Unsupported("jpeg arithmetic coding")));
    }
    if let Some(b) = common::load("jpeg/Landscape_6.jpg") {
        let mut img = pixel_core::decode(&b).unwrap();
        assert_eq!((img.width, img.height, img.orientation), (1200, 1800, 6));
        img.apply_orientation();
        assert_eq!((img.width, img.height, img.orientation), (1800, 1200, 1));
    }
    // A truncated file is refused, not shown partially.
    if let Some(b) = common::load("jpeg/testorig.jpg") {
        assert!(pixel_core::decode(&b[..b.len() / 2]).is_err());
    }
}

#[test]
fn orientation_maps_all_eight() {
    // A 2x3 image with distinct pixels; each orientation's display must put stored (0,0) where EXIF says.
    let w = 2u32;
    let h = 3u32;
    let rgba: Vec<u8> = (0..w * h).flat_map(|i| [i as u8, 0, 0, 255]).collect();
    // Displayed top-left pixel for each orientation (EXIF 2.3 Table: 1 TL, 2 TR, 3 BR, 4 BL, 5 LT, 6 RT, 7 RB, 8 LB)
    let expect_tl = [0u8, 0, 1, 5, 4, 0, 4, 5, 1];
    for o in 1..=8u8 {
        let mut img = pixel_core::decode(&qoi_of(w, h, &rgba)).unwrap();
        img.orientation = o;
        img.apply_orientation();
        let dims = if o >= 5 { (h, w) } else { (w, h) };
        assert_eq!((img.width, img.height), dims, "orientation {o}");
        assert_eq!(img.rgba[0], expect_tl[o as usize], "orientation {o}: displayed top-left");
    }
}

/// A QOI file of raw QOI_OP_RGBA chunks (the simplest container to build by hand).
fn qoi_of(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut v = b"qoif".to_vec();
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&[4, 0]);
    for p in rgba.chunks(4) {
        v.push(0xFF);
        v.extend_from_slice(p);
    }
    v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    v
}
