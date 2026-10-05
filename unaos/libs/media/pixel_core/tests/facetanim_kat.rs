// SPDX-License-Identifier: LGPL-3.0-or-later
//! FACETANIM (rmbp-ledger B358) KATs: the streaming face — [`pixel_core::Animation`] (one canvas, at
//! most one more buffer) and [`pixel_core::decode_first_frame`] — byte-equal, frame for frame, to the
//! full composite `decode` keeps in `Image::frames`, on the ANIMWEBP oracle corpus (animated WebP +
//! APNG, `tests/anim_digests.txt`, pinned to Chromium) and on every GIF / WebP / PNG vector fetched
//! by `fetch-vectors.sh` (SKIP offline), plus hand-built GIFs (disposal 2 and 3, a frame that breaks).

mod common;

use pixel_core::{Animation, Error, decode, decode_first_frame};

/// Walk `bytes` through `Animation` and compare every frame with `decode`. Returns the frame count.
fn check(name: &str, bytes: &[u8]) -> Option<usize> {
    let full = decode(bytes);
    let anim = Animation::new(bytes);
    let img = match full {
        Ok(img) => img,
        Err(e) => {
            // A refused file: refused up front, or (GIF: a structural error after good frames, which
            // `decode` refuses whole) the stream ends in that error.
            if let Ok(mut a) = anim {
                let mut last = None;
                while let Some(r) = a.next_frame() {
                    last = Some(r);
                }
                assert!(matches!(last, Some(Err(_))), "{name}: decode refused ({e}) but the stream played clean");
            }
            return None;
        }
    };
    let mut a = anim.unwrap_or_else(|e| panic!("{name}: decode ok, Animation::new refused: {e}"));
    let want: Vec<(u32, Vec<u8>)> = match &img.frames {
        Some(f) => f.iter().map(|f| (f.delay_ms, f.rgba.clone())).collect(),
        None => vec![(u32::MAX, img.rgba.clone())],
    };
    assert_eq!((a.width(), a.height()), (img.width, img.height), "{name}: geometry");
    assert_eq!(a.loop_count(), img.loop_count, "{name}: loop count");
    for pass in 0..2 {
        let mut i = 0;
        while let Some(r) = a.next_frame() {
            let fi = r.unwrap_or_else(|e| panic!("{name}: frame {i} failed in the stream: {e}"));
            assert_eq!(fi.index, i, "{name}: index");
            assert!(i < want.len(), "{name}: the stream played more frames than decode kept");
            if want[i].0 != u32::MAX {
                assert_eq!(fi.delay_ms, want[i].0, "{name}: frame {i} delay");
            }
            assert!(a.canvas() == &want[i].1[..], "{name}: frame {i} (pass {pass}) differs from the full composite");
            assert!(a.buffers_held() <= 2, "{name}: frame {i} holds {} buffers", a.buffers_held());
            i += 1;
        }
        assert_eq!(i, want.len(), "{name}: frames played (pass {pass})");
        assert_eq!(a.frame_count(), want.len(), "{name}: frame_count after the end");
        assert_eq!(a.is_animated(), img.frames.is_some(), "{name}: is_animated");
        a.rewind();
    }
    let f0 = decode_first_frame(bytes).unwrap_or_else(|e| panic!("{name}: decode_first_frame: {e}"));
    assert_eq!((f0.width, f0.height), (img.width, img.height), "{name}: first-frame geometry");
    assert!(f0.rgba == img.rgba, "{name}: decode_first_frame differs from decode().rgba");
    assert!(f0.frames.is_none(), "{name}: decode_first_frame kept frames");
    Some(want.len())
}

fn run_dir(sub: &str, ext: &str) -> (usize, usize, usize) {
    let (mut files, mut animated, mut frames) = (0, 0, 0);
    for n in common::list(sub, ext) {
        let rel = format!("{sub}/{n}");
        let Some(bytes) = common::load(&rel) else { continue };
        if let Some(k) = check(&rel, &bytes) {
            files += 1;
            frames += k;
            animated += (k > 1) as usize;
        }
    }
    (files, animated, frames)
}

#[test]
fn stream_equals_full_composite_on_the_animwebp_corpus() {
    let list = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/anim_digests.txt")).unwrap();
    let (mut files, mut frames, mut skipped) = (0, 0, 0);
    for line in list.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let rel = line.split_whitespace().next().unwrap();
        let Some(bytes) = common::load(rel) else {
            skipped += 1;
            continue;
        };
        if let Some(k) = check(rel, &bytes) {
            files += 1;
            frames += k;
        }
    }
    eprintln!("FACETANIM corpus: {files} files, {frames} frames byte-equal, {skipped} SKIP (not fetched)");
}

#[test]
fn stream_equals_full_composite_on_every_gif_webp_png_vector() {
    for (sub, ext) in [("gif", ".gif"), ("webp", ".webp"), ("pngsuite", ".png"), ("apng", ".png"), ("anim", ".webp"), ("anim", ".png")] {
        let (f, a, k) = run_dir(sub, ext);
        eprintln!("FACETANIM {sub}/*{ext}: {f} files ({a} animated), {k} frames byte-equal");
    }
}

// ---------------------------------------------------------------------------------------------
// Hand-built GIFs: 2x1 canvas, a 4-colour global table, one-pixel frames (LZW min code size 2).

const RED: u8 = 1;
const GREEN: u8 = 2;
const BLUE: u8 = 3;

/// The LZW data for one 1x1 frame of colour index `c` (clear, c, EOI at 3 bits), as sub-blocks.
fn one_pixel(x: u16, c: u8, gce: Option<(u8, u16)>, out: &mut Vec<u8>) {
    if let Some((disposal, delay_cs)) = gce {
        out.extend_from_slice(&[0x21, 0xF9, 4, disposal << 2, delay_cs as u8, (delay_cs >> 8) as u8, 0, 0]);
    }
    out.push(0x2C);
    out.extend_from_slice(&x.to_le_bytes());
    out.extend_from_slice(&[0, 0, 1, 0, 1, 0, 0]);
    // codes (3 bits, LSB first): clear=4, c, eoi=5
    let bits = 4u32 | (c as u32) << 3 | 5 << 6;
    out.extend_from_slice(&[2, 2, bits as u8, (bits >> 8) as u8, 0]);
}

fn gif(frames: &[(u16, u8, u8, u16)], loop_ext: Option<u16>) -> Vec<u8> {
    let mut b = b"GIF89a".to_vec();
    b.extend_from_slice(&[2, 0, 1, 0, 0x81, 0, 0]);
    b.extend_from_slice(&[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
    if let Some(n) = loop_ext {
        b.extend_from_slice(&[0x21, 0xFF, 11]);
        b.extend_from_slice(b"NETSCAPE2.0");
        b.extend_from_slice(&[3, 1, n as u8, (n >> 8) as u8, 0]);
    }
    for &(x, c, disposal, delay) in frames {
        one_pixel(x, c, Some((disposal, delay)), &mut b);
    }
    b.push(0x3B);
    b
}

fn px<B: AsRef<[u8]>>(a: &Animation<B>, x: usize) -> [u8; 4] {
    let c = a.canvas();
    [c[x * 4], c[x * 4 + 1], c[x * 4 + 2], c[x * 4 + 3]]
}

#[test]
fn gif_disposal_previous_holds_one_extra_buffer_then_releases_it() {
    // f0 red at x0 (keep); f1 green at x1 (restore to previous); f2 blue at x0 (none).
    let b = gif(&[(0, RED, 1, 10), (1, GREEN, 3, 20), (0, BLUE, 0, 30)], Some(0));
    check("hand/disposal3.gif", &b).unwrap();
    let mut a = Animation::new(&b).unwrap();
    assert_eq!((a.frame_count(), a.loop_count()), (3, Some(0)));
    a.next_frame().unwrap().unwrap();
    assert_eq!((px(&a, 0), px(&a, 1), a.buffers_held()), ([255, 0, 0, 255], [0, 0, 0, 0], 1));
    let f1 = a.next_frame().unwrap().unwrap();
    assert_eq!((f1.index, f1.delay_ms), (1, 200));
    assert_eq!((px(&a, 1), a.buffers_held()), ([0, 255, 0, 255], 2));
    a.next_frame().unwrap().unwrap();
    assert_eq!((px(&a, 0), px(&a, 1), a.buffers_held()), ([0, 0, 255, 255], [0, 0, 0, 0], 1));
    assert!(a.next_frame().is_none());
}

#[test]
fn gif_disposal_background_and_first_frame() {
    let b = gif(&[(0, RED, 2, 10), (1, GREEN, 0, 10)], None);
    check("hand/disposal2.gif", &b).unwrap();
    let f0 = decode_first_frame(&b).unwrap();
    assert_eq!(&f0.rgba[..], &[255, 0, 0, 255, 0, 0, 0, 0]);
    let mut a = Animation::new(&b).unwrap();
    a.next_frame();
    a.next_frame();
    assert_eq!((px(&a, 0), px(&a, 1)), ([0, 0, 0, 0], [0, 255, 0, 255]));
    assert_eq!(a.loop_count(), None);
}

#[test]
fn gif_single_frame_is_a_still_and_a_broken_tail_ends_in_error() {
    let b = gif(&[(0, RED, 0, 10)], Some(0));
    assert_eq!(check("hand/one.gif", &b), Some(1));
    let a = Animation::new(&b).unwrap();
    assert_eq!((a.frame_count(), a.is_animated(), a.loop_count()), (1, false, None));
    // A second Image Descriptor cut off inside its header: decode refuses the file, the stream shows
    // frame 0 and then reports the break; decode_first_frame still answers frame 0.
    let mut cut = gif(&[(0, RED, 0, 10)], None);
    cut.pop();
    cut.extend_from_slice(&[0x2C, 1, 0]);
    assert_eq!(decode(&cut).unwrap_err(), Error::Truncated);
    let mut a = Animation::new(&cut).unwrap();
    assert!(matches!(a.next_frame(), Some(Ok(_))));
    assert_eq!(a.next_frame(), Some(Err(Error::Truncated)));
    assert_eq!(&decode_first_frame(&cut).unwrap().rgba[..4], &[255, 0, 0, 255]);
}

// ---------------------------------------------------------------------------------------------
// The staged fixtures `tests facetanim` reads on metal (builder copies them onto the DATA volume as
// /apps/ANIM3.GIF, /apps/ANIM3.WEBP, /apps/ANIM3.PNG). Each is an 8x8, 3-frame, loop-forever
// animation, 200 ms a frame: frame 0 solid red, frame 1 a 4x4 green square at (2,2) over it, frame 2
// solid blue. The kernel test reads frame 1 back: (3,3) green, (0,0) red. The committed bytes ARE what
// these generators write (`FACETANIM_WRITE=1 cargo test -p pixel_core --test facetanim_kat` rewrites
// them); the test below holds both to that and to the composite.

const FX_RED: [u8; 4] = [255, 0, 0, 255];
const FX_GREEN: [u8; 4] = [0, 255, 0, 255];
const FX_BLUE: [u8; 4] = [0, 0, 255, 255];
/// (x, y, w, h, colour) of the three frames.
const FX_FRAMES: [(u32, u32, u32, u32, [u8; 4]); 3] = [(0, 0, 8, 8, FX_RED), (2, 2, 4, 4, FX_GREEN), (0, 0, 8, 8, FX_BLUE)];

/// GIF89a: palette {black, red, green, blue}; LZW minimum code size 2, every pixel sent as
/// Clear + root (3-bit codes, the table never grows), so no encoder state is needed.
fn fixture_gif() -> Vec<u8> {
    let mut b = b"GIF89a".to_vec();
    b.extend_from_slice(&[8, 0, 8, 0, 0x81, 0, 0]);
    b.extend_from_slice(&[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
    b.extend_from_slice(&[0x21, 0xFF, 11]);
    b.extend_from_slice(b"NETSCAPE2.0");
    b.extend_from_slice(&[3, 1, 0, 0, 0]);
    for (x, y, w, h, c) in FX_FRAMES {
        let ci = match c {
            FX_RED => 1u32,
            FX_GREEN => 2,
            _ => 3,
        };
        b.extend_from_slice(&[0x21, 0xF9, 4, 1 << 2, 20, 0, 0, 0]); // disposal 1 (keep), 20 cs
        b.push(0x2C);
        for v in [x, y, w, h] {
            b.extend_from_slice(&(v as u16).to_le_bytes());
        }
        b.push(0);
        let mut bits: Vec<u8> = Vec::new();
        let (mut acc, mut n) = (0u32, 0u32);
        let mut put = |v: u32, bits: &mut Vec<u8>| {
            acc |= v << n;
            n += 3;
            while n >= 8 {
                bits.push(acc as u8);
                acc >>= 8;
                n -= 8;
            }
        };
        for _ in 0..w * h {
            put(4, &mut bits);
            put(ci, &mut bits);
        }
        put(5, &mut bits);
        if n > 0 {
            bits.push(acc as u8);
        }
        b.push(2);
        for blk in bits.chunks(255) {
            b.push(blk.len() as u8);
            b.extend_from_slice(blk);
        }
        b.push(0);
    }
    b.push(0x3B);
    b
}

/// Animated WebP: VP8X (animation) + ANIM (loop 0) + three ANMF frames, each a solid VP8L image
/// (five one-symbol prefix codes, so every pixel costs zero bits), no blending, no disposal.
fn fixture_webp() -> Vec<u8> {
    fn put(out: &mut Vec<u8>, acc: &mut u64, n: &mut u32, v: u32, k: u32) {
        *acc |= (v as u64) << *n;
        *n += k;
        while *n >= 8 {
            out.push(*acc as u8);
            *acc >>= 8;
            *n -= 8;
        }
    }
    fn vp8l(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        let (mut out, mut acc, mut n) = (vec![0x2F], 0u64, 0u32);
        put(&mut out, &mut acc, &mut n, w - 1, 14);
        put(&mut out, &mut acc, &mut n, h - 1, 14);
        put(&mut out, &mut acc, &mut n, 0, 1 + 3 + 1 + 1 + 1); // no alpha, v0, no transform/cache/meta
        for sym in [c[1], c[0], c[2], c[3], 0] {
            put(&mut out, &mut acc, &mut n, 0b101, 3); // simple, one symbol, 8-bit
            put(&mut out, &mut acc, &mut n, sym as u32, 8);
        }
        if n > 0 {
            out.push(acc as u8);
        }
        out
    }
    fn chunk(cc: &[u8; 4], d: &[u8]) -> Vec<u8> {
        let mut v = cc.to_vec();
        v.extend_from_slice(&(d.len() as u32).to_le_bytes());
        v.extend_from_slice(d);
        if d.len() & 1 == 1 {
            v.push(0);
        }
        v
    }
    let u24 = |v: u32| [v as u8, (v >> 8) as u8, (v >> 16) as u8];
    let mut body = b"WEBP".to_vec();
    let mut x = vec![0x02, 0, 0, 0];
    x.extend_from_slice(&u24(7));
    x.extend_from_slice(&u24(7));
    body.extend(chunk(b"VP8X", &x));
    body.extend(chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]));
    for (fx, fy, w, h, c) in FX_FRAMES {
        let mut d = Vec::new();
        for v in [fx / 2, fy / 2, w - 1, h - 1, 200] {
            d.extend_from_slice(&u24(v));
        }
        d.push(0x02); // do not blend, no dispose
        d.extend(chunk(b"VP8L", &vp8l(w, h, c)));
        body.extend(chunk(b"ANMF", &d));
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

/// APNG: 8-bit RGBA, acTL (3 frames, plays 0), frame 0's fcTL before the IDAT (the default image is
/// frame 0), frames 1-2 as fdAT; stored-deflate data, dispose NONE, blend SOURCE, delay 1/5 s.
fn fixture_apng() -> Vec<u8> {
    fn z(w: u32, h: u32, c: [u8; 4]) -> Vec<u8> {
        let mut raw = Vec::new();
        for _ in 0..h {
            raw.push(0);
            for _ in 0..w {
                raw.extend_from_slice(&c);
            }
        }
        let mut z = vec![0x78, 0x01, 0x01];
        z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
        z.extend_from_slice(&raw);
        let (mut a, mut b) = (1u32, 0u32);
        for &x in &raw {
            a = (a + x as u32) % 65521;
            b = (b + a) % 65521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        z
    }
    fn pchunk(out: &mut Vec<u8>, kind: &[u8; 4], d: &[u8]) {
        out.extend_from_slice(&(d.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(d);
        out.extend_from_slice(&body);
        out.extend_from_slice(&pixel_core::crc::crc32(&body).to_be_bytes());
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    pchunk(&mut out, b"IHDR", &[0, 0, 0, 8, 0, 0, 0, 8, 8, 6, 0, 0, 0]);
    pchunk(&mut out, b"acTL", &[0, 0, 0, 3, 0, 0, 0, 0]);
    let mut seq = 0u32;
    for (i, (x, y, w, h, c)) in FX_FRAMES.into_iter().enumerate() {
        let mut f = seq.to_be_bytes().to_vec();
        for v in [w, h, x, y] {
            f.extend_from_slice(&v.to_be_bytes());
        }
        f.extend_from_slice(&[0, 1, 0, 5, 0, 0]);
        pchunk(&mut out, b"fcTL", &f);
        seq += 1;
        if i == 0 {
            pchunk(&mut out, b"IDAT", &z(w, h, c));
        } else {
            let mut d = seq.to_be_bytes().to_vec();
            d.extend(z(w, h, c));
            pchunk(&mut out, b"fdAT", &d);
            seq += 1;
        }
    }
    pchunk(&mut out, b"IEND", &[]);
    out
}

#[test]
fn staged_fixtures_are_three_frames_with_a_green_frame_one() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/facetanim");
    for (name, bytes) in [("ANIM3.GIF", fixture_gif()), ("ANIM3.WEBP", fixture_webp()), ("ANIM3.PNG", fixture_apng())] {
        if std::env::var_os("FACETANIM_WRITE").is_some() {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(name), &bytes).unwrap();
        }
        let committed = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e} (FACETANIM_WRITE=1 writes it)"));
        assert_eq!(committed, bytes, "{name}: the committed fixture is not the generator's output");
        assert!(bytes.len() < 20 * 1024, "{name}: {} bytes", bytes.len());
        assert_eq!(check(name, &bytes), Some(3), "{name}: frames");
        // Second opinion on frame 0 (the `image` crate decodes the same bytes independently).
        let (rw, rh, rpx) = common::reference(&bytes).unwrap_or_else(|| panic!("{name}: the image crate refused it"));
        assert_eq!((rw, rh), (8, 8), "{name}: second-opinion geometry");
        assert!(rpx.chunks_exact(4).all(|p| p == FX_RED), "{name}: second opinion's frame 0 is not solid red");
        let mut a = Animation::new(&bytes[..]).unwrap();
        assert_eq!((a.width(), a.height(), a.frame_count(), a.loop_count()), (8, 8, 3, Some(0)), "{name}");
        let at = |a: &Animation<&[u8]>, x: usize, y: usize| px(a, y * 8 + x);
        let f0 = a.next_frame().unwrap().unwrap();
        assert_eq!((f0.delay_ms, at(&a, 3, 3)), (200, FX_RED), "{name}: frame 0");
        a.next_frame().unwrap().unwrap();
        assert_eq!((at(&a, 3, 3), at(&a, 0, 0), at(&a, 6, 6)), (FX_GREEN, FX_RED, FX_RED), "{name}: frame 1");
        a.next_frame().unwrap().unwrap();
        assert_eq!(at(&a, 3, 3), FX_BLUE, "{name}: frame 2");
        assert_eq!(a.buffers_held(), 1, "{name}");
        assert!(a.next_frame().is_none());
        eprintln!("FACETANIM fixture {name}: {} bytes, 3 frames", bytes.len());
    }
}
