// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Regenerate the eyes suite's fixtures (`tools/eyes/suites/facet/fixtures/`), deterministically:
//! `cargo run --release -p facet --example make_fixtures -- <dir>`.
//!
//! - `card.png` — 48x32 RGBA test card (Facet's own PNG writer): four flat quadrants, a 1-px black
//!   border, a horizontal grey ramp, and a half-transparent band, so every turn, flip, crop and
//!   blend is legible.
//! - `photo-o6.jpg`, `photo-o5.jpg` — a 48x32 card in JPEG (encoded by the `image` crate's JPEG
//!   ENCODER — a fixture tool, not a Facet path) with an APP1 EXIF segment carrying Orientation 6
//!   (display rotated 90 degrees clockwise) / 5 (transposed), spliced in by hand.

fn card(alpha_band: bool) -> (u32, u32, Vec<u8>) {
    let (w, h) = (48u32, 32u32);
    let mut v = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let mut p = match (x < w / 2, y < h / 2) {
                (true, true) => [220, 40, 40, 255],
                (false, true) => [40, 180, 60, 255],
                (true, false) => [40, 70, 220, 255],
                (false, false) => [240, 240, 240, 255],
            };
            if (12..16).contains(&y) {
                let g = (x * 255 / (w - 1)) as u8;
                p = [g, g, g, 255];
            }
            if alpha_band && (20..24).contains(&y) {
                p[3] = 128;
            }
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                p = [0, 0, 0, 255];
            }
            v.extend(p);
        }
    }
    (w, h, v)
}

fn exif_app1(orientation: u16) -> Vec<u8> {
    let mut t = b"Exif\0\0MM\0*".to_vec();
    t.extend(8u32.to_be_bytes());
    t.extend(1u16.to_be_bytes());
    t.extend(0x0112u16.to_be_bytes());
    t.extend(3u16.to_be_bytes());
    t.extend(1u32.to_be_bytes());
    t.extend(orientation.to_be_bytes());
    t.extend(0u16.to_be_bytes());
    t.extend(0u32.to_be_bytes());
    let mut seg = vec![0xFF, 0xE1];
    seg.extend(((t.len() + 2) as u16).to_be_bytes());
    seg.extend(t);
    seg
}

fn jpeg_with_orientation(o: u16) -> Vec<u8> {
    let (w, h, rgba) = card(false);
    let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 95)
        .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
        .expect("jpeg encode");
    let mut out = jpg[..2].to_vec(); // SOI
    out.extend(exif_app1(o));
    out.extend(&jpg[2..]);
    out
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "tools/eyes/suites/facet/fixtures".into()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let (w, h, rgba) = card(true);
    std::fs::write(dir.join("card.png"), facet::png::encode(w, h, &rgba)).expect("write");
    std::fs::write(dir.join("photo-o6.jpg"), jpeg_with_orientation(6)).expect("write");
    std::fs::write(dir.join("photo-o5.jpg"), jpeg_with_orientation(5)).expect("write");
    println!("fixtures written to {}", dir.display());
}
