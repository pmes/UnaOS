// SPDX-License-Identifier: LGPL-3.0-or-later
//! M3 KATs: GIF (frames, disposal, transparency, loop count, delays), BMP (every header/depth in the
//! set, top-down rows), QOI (exact, against PNG sources re-encoded by the `image` crate's QOI writer).

mod common;

#[test]
fn gif_animation_metadata() {
    if let Some(b) = common::load("gif/animation-speed.gif") {
        let img = pixel_core::decode(&b).unwrap();
        let f = img.frames.as_ref().expect("animated");
        assert_eq!(f.iter().map(|f| f.delay_ms).collect::<Vec<_>>(), vec![250, 500, 1000, 2000]);
        assert_eq!(img.loop_count, Some(0), "NETSCAPE2.0 loop 0 = forever");
        assert_eq!(img.rgba, f[0].rgba, "Image::rgba is the first composited frame");
    }
    if let Some(b) = common::load("gif/dispose-restore-previous.gif") {
        assert_eq!(pixel_core::decode(&b).unwrap().frames.unwrap().len(), 5);
    }
    if let Some(b) = common::load("gif/images-combine.gif") {
        assert_eq!(pixel_core::decode(&b).unwrap().loop_count, None, "no loop extension");
    }
    for bad in ["gif/no-data.gif", "gif/zero-width.gif"] {
        if let Some(b) = common::load(bad) {
            assert!(pixel_core::decode(&b).is_err(), "{bad} must be refused");
        }
    }
}

#[test]
fn gif_first_frame_second_opinion() {
    let mut n = 0;
    for name in common::list("gif", ".gif") {
        let b = common::load(&format!("gif/{name}")).unwrap();
        let (Ok(img), Some((w, h, want))) = (pixel_core::decode(&b), common::reference(&b)) else { continue };
        if (img.width, img.height) != (w, h) {
            continue;
        }
        // Compare only where both say opaque: decoders differ on what a transparent pixel's colour is.
        let mut bad = 0;
        for (p, q) in img.rgba.chunks(4).zip(want.chunks(4)) {
            if p[3] == 255 && q[3] == 255 && p != q {
                bad += 1;
            }
        }
        assert_eq!(bad, 0, "{name}: {bad} opaque pixels differ from the image crate");
        n += 1;
    }
    eprintln!("gif second opinion: {n} files");
}

#[test]
fn bmp_second_opinion_and_top_down() {
    let mut n = 0;
    for name in common::list("bmp", ".bmp") {
        let b = common::load(&format!("bmp/{name}")).unwrap();
        let img = pixel_core::decode(&b).unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Some((w, h, want)) = common::reference(&b) {
            assert_eq!((img.width, img.height), (w, h), "{name}");
            let (m, cnt) = common::diff(&img.rgba, &want);
            eprintln!("{name}: vs image crate max {m} differing {cnt}");
            n += 1;
        }
        // Flip the rows and negate the height: the decode must be identical.
        let bpp = u16::from_le_bytes([b[28], b[29]]) as usize;
        let comp = u32::from_le_bytes([b[30], b[31], b[32], b[33]]);
        if b[14] >= 40 && comp != 1 && comp != 2 {
            let off = u32::from_le_bytes([b[10], b[11], b[12], b[13]]) as usize;
            let h = i32::from_le_bytes([b[22], b[23], b[24], b[25]]);
            if h > 0 {
                let stride = (img.width as usize * bpp).div_ceil(32) * 4;
                let mut t = b.clone();
                for r in 0..h as usize {
                    let src = off + r * stride;
                    let dst = off + (h as usize - 1 - r) * stride;
                    t[dst..dst + stride].copy_from_slice(&b[src..src + stride]);
                }
                t[22..26].copy_from_slice(&(-h).to_le_bytes());
                assert_eq!(pixel_core::decode(&t).unwrap().rgba, img.rgba, "{name}: top-down variant");
            }
        }
    }
    eprintln!("bmp second opinion: {n} files");
}

#[test]
fn qoi_exact_against_png_sources() {
    let mut n = 0;
    for name in common::list("pngsuite", ".png") {
        if name.starts_with('x') {
            continue;
        }
        let png = common::load(&format!("pngsuite/{name}")).unwrap();
        let src = image::load_from_memory(&png).unwrap().to_rgba8();
        let mut qoi = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(src.clone()).write_to(&mut qoi, image::ImageFormat::Qoi).unwrap();
        let img = pixel_core::decode(&qoi.into_inner()).unwrap_or_else(|e| panic!("{name}.qoi: {e}"));
        assert_eq!(img.rgba, src.into_raw(), "{name}: QOI round trip");
        n += 1;
    }
    eprintln!("qoi: {n} files exact");
}
