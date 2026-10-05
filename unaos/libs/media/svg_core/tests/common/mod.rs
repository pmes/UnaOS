//! Shared test helpers: fonts, PNG write (stored deflate), PNG read through pixel_core, scoring.
#![allow(dead_code)]

use std::path::Path;
use svg_core::{DecodedImage, FontSet};

pub fn decoder(b: &[u8]) -> Option<DecodedImage> {
    let img = pixel_core::decode(b).ok()?;
    Some(DecodedImage { width: img.width as usize, height: img.height as usize, rgba: img.rgba })
}

/// Fonts from a directory (every .ttf/.otf/.ttc), with the generic families the oracle's fontconfig uses.
pub fn fonts_from(dir: &Path) -> FontSet {
    let mut fs = FontSet::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut v: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        v.sort();
        for p in v {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if matches!(ext.as_str(), "ttf" | "otf" | "ttc") {
                if let Ok(d) = std::fs::read(&p) {
                    fs.add(d);
                }
            }
        }
    }
    fs.serif = "Noto Serif".into();
    fs.sans_serif = "Noto Sans".into();
    fs.monospace = "Noto Mono".into();
    fs.cursive = "Yellowtail".into();
    fs.fantasy = "Sedgwick Ave Display".into();
    fs.default_family = "Noto Serif".into();
    fs
}

fn crc32(data: &[u8]) -> u32 {
    pixel_core::png::encode::crc32(data)
}

pub fn write_png(path: &Path, w: usize, h: usize, rgba: &[u8]) {
    let mut raw = Vec::with_capacity((w * 4 + 1) * h);
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    let mut z = vec![0x78, 0x01];
    for (i, chunk) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(last as u8);
        let l = chunk.len() as u16;
        z.extend_from_slice(&l.to_le_bytes());
        z.extend_from_slice(&(!l).to_le_bytes());
        z.extend_from_slice(chunk);
    }
    if raw.is_empty() {
        z.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    z.extend_from_slice(&pixel_core::png::encode::adler32(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |ty: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = ty.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    std::fs::write(path, out).unwrap();
}

/// Straight RGBA composited over white → RGB.
pub fn over_white(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4 * 3);
    for p in rgba.chunks_exact(4) {
        let a = p[3] as u32;
        for c in 0..3 {
            out.push(((p[c] as u32 * a + 255 * (255 - a) + 127) / 255) as u8);
        }
    }
    out
}

/// (mean |error| per channel in levels, share of pixels whose max channel error ≤ 8).
pub fn score(a: &[u8], b: &[u8]) -> (f64, f64) {
    let n = a.len() / 3;
    let mut sum = 0u64;
    let mut ok = 0usize;
    for i in 0..n {
        let mut m = 0;
        for c in 0..3 {
            let d = (a[i * 3 + c] as i32 - b[i * 3 + c] as i32).unsigned_abs();
            sum += d as u64;
            m = m.max(d);
        }
        if m <= 8 {
            ok += 1;
        }
    }
    (sum as f64 / (n * 3) as f64, ok as f64 / n as f64)
}

pub fn crc32_of(b: &[u8]) -> u32 {
    crc32(b)
}
