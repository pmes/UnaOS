// SPDX-License-Identifier: LGPL-3.0-or-later
//! ANIMWEBP (SR44) KATs: animated WebP (RFC 9649 §2.7.1.1, ANIM + ANMF) and APNG composited into
//! `Image::frames`.
//!
//! * the shared blend (`blend_nonpremult`) against values read out of Chromium's own decode;
//! * hand-built animations whose every frame is computed by hand (solid-colour VP8L frames from a
//!   ten-line encoder below, so each pixel's answer is known): offsets, key frames, blend / no-blend,
//!   dispose-to-background, the no-blend-inside-the-disposed-rectangle rule, the structural refusals;
//! * `tests/anim_digests.txt`: every public animation whose pixel_core decode matched Chromium's
//!   WebCodecs frames exactly (oracle/chromium-anim-raw.cjs + pixel-check --compare-raw), pinned as
//!   geometry + frame count + loop count + delays + CRC-32 of every frame.

mod common;

use pixel_core::{Error, blend_nonpremult, decode};

#[test]
fn blend_matches_chromium_readout() {
    // Read back from Chromium 141 on webp-animated-semitransparent{2,4}.webp (frame 1).
    assert_eq!(blend_nonpremult([0, 255, 255, 3], [255, 255, 0, 0]), [0, 254, 254, 3]);
    assert_eq!(blend_nonpremult([255, 255, 0, 179], [50, 50, 50, 0]), [254, 254, 0, 179]);
    // Worked by hand: dst_factor = 255*128>>8 = 127, out_a = 255, scale = 2^24/255 = 65793.
    assert_eq!(blend_nonpremult([0, 0, 255, 128], [255, 0, 0, 255]), [126, 0, 127, 255]);
    assert_eq!(blend_nonpremult([10, 20, 30, 100], [200, 100, 50, 60]), [60, 41, 35, 136]);
    // A transparent source leaves the destination.
    assert_eq!(blend_nonpremult([1, 2, 3, 0], [9, 8, 7, 6]), [9, 8, 7, 6]);
}

/// LSB-first bit writer (RFC 9649 §3.1).
struct W {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}
impl W {
    fn put(&mut self, v: u32, k: u32) {
        self.acc |= (v as u64) << self.n;
        self.n += k;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    fn done(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

/// A `w x h` VP8L image of one colour: no transforms, no colour cache, no meta codes, five
/// one-symbol "simple" prefix codes (§6.2.1) — so every pixel costs zero bits.
fn vp8l_solid(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut b = W { out: vec![0x2F], acc: 0, n: 0 };
    b.put(w - 1, 14);
    b.put(h - 1, 14);
    b.put((rgba[3] != 255) as u32, 1); // alpha_is_used
    b.put(0, 3); // version
    b.put(0, 1); // no transform
    b.put(0, 1); // no colour cache
    b.put(0, 1); // no meta prefix codes
    for sym in [rgba[1], rgba[0], rgba[2], rgba[3], 0] {
        b.put(1, 1); // simple code
        b.put(0, 1); // one symbol
        b.put(1, 1); // 8-bit symbol
        b.put(sym as u32, 8);
    }
    b.done()
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

fn u24(v: u32) -> [u8; 3] {
    [v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

/// One ANMF frame: offset (even), size, duration, blend, dispose-to-background, a solid colour.
struct F {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    ms: u32,
    blend: bool,
    dispose: bool,
    c: [u8; 4],
}

fn anim(cw: u32, ch: u32, loops: u16, frames: &[F]) -> Vec<u8> {
    let mut x = vec![0x02 | 0x10, 0, 0, 0]; // animation + alpha flags
    x.extend_from_slice(&u24(cw - 1));
    x.extend_from_slice(&u24(ch - 1));
    let mut body = b"WEBP".to_vec();
    body.extend(chunk(b"VP8X", &x));
    let mut a = vec![0xFF, 0xFF, 0xFF, 0xFF]; // background colour (ignored)
    a.extend_from_slice(&loops.to_le_bytes());
    body.extend(chunk(b"ANIM", &a));
    for f in frames {
        let mut d = Vec::new();
        d.extend_from_slice(&u24(f.x / 2));
        d.extend_from_slice(&u24(f.y / 2));
        d.extend_from_slice(&u24(f.w - 1));
        d.extend_from_slice(&u24(f.h - 1));
        d.extend_from_slice(&u24(f.ms));
        d.push(((!f.blend) as u8) << 1 | f.dispose as u8);
        d.extend(chunk(b"VP8L", &vp8l_solid(f.w, f.h, f.c)));
        body.extend(chunk(b"ANMF", &d));
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

fn px(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    rgba[i..i + 4].try_into().unwrap()
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const HALF_BLUE: [u8; 4] = [0, 0, 255, 128];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

#[test]
fn webp_offsets_blend_dispose() {
    let f = |x, y, w, h, blend, dispose, c| F { x, y, w, h, ms: 70, blend, dispose, c };
    let file = anim(
        6,
        4,
        0,
        &[
            f(0, 0, 6, 4, true, false, RED),          // key frame
            f(2, 2, 2, 2, true, true, HALF_BLUE),     // blended over red, then disposed
            f(0, 0, 2, 2, true, false, GREEN),        // canvas: red, a cleared 2x2 hole
            f(2, 0, 4, 2, false, false, HALF_BLUE),   // no-blend: copied as is
        ],
    );
    let img = decode(&file).unwrap();
    assert_eq!((img.width, img.height, img.loop_count), (6, 4, Some(0)));
    let fr = img.frames.as_ref().unwrap();
    assert_eq!(fr.len(), 4);
    assert!(fr.iter().all(|f| f.delay_ms == 70));
    assert_eq!(img.rgba, fr[0].rgba);
    assert!(fr[0].rgba.chunks(4).all(|p| p == RED));
    // Frame 1: the 2x2 at (2,2) is half blue over red, the rest red.
    assert_eq!(px(&fr[1].rgba, 6, 2, 2), [126, 0, 127, 255]);
    assert_eq!(px(&fr[1].rgba, 6, 3, 3), [126, 0, 127, 255]);
    assert_eq!(px(&fr[1].rgba, 6, 1, 2), RED);
    assert_eq!(px(&fr[1].rgba, 6, 4, 3), RED);
    // Frame 2: frame 1's rectangle went back to transparent; green at the top left.
    assert_eq!(px(&fr[2].rgba, 6, 2, 2), CLEAR);
    assert_eq!(px(&fr[2].rgba, 6, 3, 3), CLEAR);
    assert_eq!(px(&fr[2].rgba, 6, 0, 0), GREEN);
    assert_eq!(px(&fr[2].rgba, 6, 1, 1), GREEN);
    assert_eq!(px(&fr[2].rgba, 6, 5, 3), RED);
    // Frame 3: the no-blend rectangle holds the raw half-blue pixels.
    assert_eq!(px(&fr[3].rgba, 6, 2, 0), HALF_BLUE);
    assert_eq!(px(&fr[3].rgba, 6, 5, 1), HALF_BLUE);
    assert_eq!(px(&fr[3].rgba, 6, 1, 1), GREEN);
    assert_eq!(px(&fr[3].rgba, 6, 2, 2), CLEAR);
}

#[test]
fn webp_key_frames_and_disposed_rect_copy() {
    let f = |x, y, w, h, blend, dispose, c| F { x, y, w, h, ms: 0, blend, dispose, c };
    // A translucent first frame is a key frame: copied, not blended over transparent (blending
    // would round 255 down to 254 at alpha 3).
    let faint = [0, 255, 255, 3];
    let img = decode(&anim(2, 2, 1, &[f(0, 0, 2, 2, true, false, faint), f(0, 0, 2, 2, true, false, faint)])).unwrap();
    let fr = img.frames.as_ref().unwrap();
    assert_eq!(img.loop_count, None, "loop count 1 = play once");
    assert_eq!(px(&fr[0].rgba, 2, 0, 0), faint);
    // Frame 1 blends faint over faint (dst alpha 3).
    assert_eq!(px(&fr[1].rgba, 2, 1, 1), blend_nonpremult(faint, faint));
    // Inside the rectangle the previous frame disposed, the new frame is COPIED (libwebp/Blink
    // `FindBlendRangeAtRow`); outside it, blended over what is there.
    let img = decode(&anim(
        4,
        2,
        3,
        &[
            f(0, 0, 4, 2, true, false, RED),
            f(0, 0, 2, 2, true, true, GREEN),
            f(0, 0, 4, 2, true, false, faint),
        ],
    ))
    .unwrap();
    let fr = img.frames.as_ref().unwrap();
    assert_eq!(img.loop_count, Some(2), "loop count 3 = two extra plays");
    assert_eq!(px(&fr[2].rgba, 4, 0, 0), faint, "inside the disposed rect: copied");
    assert_eq!(px(&fr[2].rgba, 4, 3, 1), blend_nonpremult(faint, RED), "outside: blended over red");
    // A previous frame that was full-canvas AND disposed makes the next one a key frame.
    let img = decode(&anim(2, 2, 0, &[f(0, 0, 2, 2, true, true, RED), f(0, 0, 2, 2, true, false, faint)])).unwrap();
    assert_eq!(px(&img.frames.as_ref().unwrap()[1].rgba, 2, 1, 0), faint);
}

#[test]
fn webp_structural_refusals() {
    let f = |x, y, w, h, c| F { x, y, w, h, ms: 0, blend: true, dispose: false, c };
    // A frame leaving the canvas refuses the file (libwebp IsValidExtendedFormat).
    let bad = anim(4, 4, 0, &[f(0, 0, 4, 4, RED), f(2, 2, 4, 4, RED)]);
    assert_eq!(decode(&bad).unwrap_err(), Error::Malformed("webp ANMF frame outside the canvas"));
    // A single-frame animation is a still (frames = None), as GIF's is.
    let one = decode(&anim(4, 4, 0, &[f(0, 0, 4, 4, GREEN)])).unwrap();
    assert!(one.frames.is_none());
    assert!(one.rgba.chunks(4).all(|p| p == GREEN));
    // A file cut inside the last frame keeps the complete frames before it.
    let full = anim(4, 4, 0, &[f(0, 0, 4, 4, RED), f(0, 0, 2, 2, GREEN), f(0, 0, 4, 4, GREEN)]);
    let cut = &full[..full.len() - 3];
    let img = decode(cut).unwrap();
    assert_eq!(img.frames.as_ref().unwrap().len(), 2);
    // ANMF in a file without the VP8X animation flag.
    let mut noflag = full.clone();
    noflag[20] = 0x10;
    assert!(matches!(decode(&noflag), Err(Error::Malformed(_))));
}

#[test]
fn chromium_pinned_animations() {
    let list = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/anim_digests.txt")).unwrap();
    let (mut checked, mut skipped) = (0, 0);
    for line in list.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let Some(bytes) = common::load(f[0]) else {
            skipped += 1;
            continue;
        };
        if f[1] == "REFUSED" {
            assert!(decode(&bytes).is_err(), "{}: Chromium refuses it; so must pixel_core", f[0]);
            checked += 1;
            continue;
        }
        let img = decode(&bytes).unwrap_or_else(|e| panic!("{}: {e}", f[0]));
        let frames = img.frames.clone().unwrap_or_else(|| vec![pixel_core::Frame { delay_ms: 0, rgba: img.rgba.clone() }]);
        let mut c = pixel_core::crc::Crc32::new();
        frames.iter().for_each(|fr| c.update(&fr.rgba));
        let lp = match img.loop_count {
            Some(0) => "inf".to_string(),
            Some(n) => n.to_string(),
            None => "0".to_string(),
        };
        let delays: Vec<String> = frames.iter().map(|fr| fr.delay_ms.to_string()).collect();
        let got = format!("{}x{} {} {lp} {} {:08x}", img.width, img.height, frames.len(), delays.join(","), c.finish());
        assert_eq!(got, f[1..].join(" "), "{}: moved off Chromium's answer", f[0]);
        checked += 1;
    }
    eprintln!("anim digests: {checked} checked, {skipped} skipped (not fetched)");
}
