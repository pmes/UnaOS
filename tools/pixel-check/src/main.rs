// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `pixel-check` — PIXELCORE's eyes and its oracle scorer.
//!
//! ```text
//! pixel-check <in> [out.png]                 decode with pixel_core, print a summary, write RGBA PNG
//! pixel-check --digest [--orient] <in>...   CRC-32 of the decoded RGBA (the tests' pinned KAT digest)
//! pixel-check --frames <in> <dir>            write every composited animation frame as <dir>/fNNN.png
//! pixel-check --compare <in> <shot.png> <r,g,b> [--orient] [--frame N]
//!                                            composite pixel_core's RGBA over the background r,g,b and
//!                                            score it against a 1:1 Chromium screenshot of the same
//!                                            file (top-left WxH of the shot): max abs diff, % exact
//!                                            channel bytes, PSNR (dB) over RGB
//! ```
//!
//! The PNG writer here is a minimal RGBA (colour type 6) writer over STORED deflate blocks — it
//! exists so the tool needs nothing but pixel_core; its output is checked by pixel_core's own decoder.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pixel-check: {e}");
            ExitCode::from(1)
        }
    }
}

fn read(p: &str) -> Result<Vec<u8>, String> {
    std::fs::read(p).map_err(|e| format!("{p}: {e}"))
}

fn decode(p: &str) -> Result<pixel_core::Image, String> {
    let bytes = read(p)?;
    pixel_core::decode(&bytes).map_err(|e| format!("{p}: {e}"))
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("--compare") => {
            let input = args.get(1).ok_or("--compare <in> <shot.png> <r,g,b>")?;
            let shot = args.get(2).ok_or("missing screenshot")?;
            let bg: Vec<u8> = args
                .get(3)
                .ok_or("missing background r,g,b")?
                .split(',')
                .map(|s| s.trim().parse::<u8>().map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            if bg.len() != 3 {
                return Err("background must be r,g,b".into());
            }
            let mut img = decode(input)?;
            if args.iter().any(|a| a == "--orient") {
                img.apply_orientation();
            }
            let frame = args
                .iter()
                .position(|a| a == "--frame")
                .and_then(|i| args.get(i + 1))
                .and_then(|s| s.parse::<usize>().ok());
            let rgba = match (frame, img.frames.as_ref()) {
                (Some(n), Some(f)) => f.get(n).ok_or("no such frame")?.rgba.clone(),
                _ => img.rgba.clone(),
            };
            let s = pixel_core::decode(&read(shot)?).map_err(|e| format!("{shot}: {e}"))?;
            if s.width < img.width || s.height < img.height {
                return Err(format!("screenshot {}x{} smaller than image {}x{}", s.width, s.height, img.width, img.height));
            }
            let (w, h) = (img.width as usize, img.height as usize);
            let (mut maxd, mut exact, mut n, mut se) = (0u8, 0usize, 0usize, 0f64);
            for y in 0..h {
                for x in 0..w {
                    let p = &rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
                    let q = &s.rgba[(y * s.width as usize + x) * 4..][..4];
                    let a = p[3] as u32;
                    for c in 0..3 {
                        let ours = blend(p[c], bg[c], a);
                        let d = ours.abs_diff(q[c]);
                        maxd = maxd.max(d);
                        exact += (d == 0) as usize;
                        n += 1;
                        se += (d as f64) * (d as f64);
                    }
                }
            }
            let psnr = if se == 0.0 { f64::INFINITY } else { 10.0 * (65025.0 / (se / n as f64)).log10() };
            println!(
                "{input} {}x{} max_abs_diff={maxd} exact={:.3}% psnr={:.2}dB",
                w,
                h,
                100.0 * exact as f64 / n as f64,
                psnr
            );
            Ok(())
        }
        Some("--digest") => {
            // CRC-32 of the decoded RGBA (orientation applied with --orient; every frame of an
            // animation, concatenated) — the KAT digest the tests pin.
            let orient = args.iter().any(|a| a == "--orient");
            for input in args[1..].iter().filter(|a| !a.starts_with("--")) {
                match decode(input) {
                    Ok(mut img) => {
                        if orient {
                            img.apply_orientation();
                        }
                        let mut c = pixel_core::crc::Crc32::new();
                        match img.frames.as_ref() {
                            Some(fr) => fr.iter().for_each(|f| c.update(&f.rgba)),
                            None => c.update(&img.rgba),
                        }
                        println!("{input} {}x{} {:08x}", img.width, img.height, c.finish());
                    }
                    Err(e) => println!("{e} ERR"),
                }
            }
            Ok(())
        }
        Some("--frames") => {
            let input = args.get(1).ok_or("--frames <in> <dir>")?;
            let dir = args.get(2).ok_or("missing output dir")?;
            let img = decode(input)?;
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let frames = img.frames.clone().unwrap_or_else(|| vec![pixel_core::Frame { delay_ms: 0, rgba: img.rgba.clone() }]);
            for (i, f) in frames.iter().enumerate() {
                let p = format!("{dir}/f{i:03}.png");
                std::fs::write(&p, write_png(img.width, img.height, &f.rgba)).map_err(|e| e.to_string())?;
                println!("{p} delay_ms={}", f.delay_ms);
            }
            Ok(())
        }
        Some(input) if !input.starts_with("--") => {
            let bytes = read(input)?;
            let fmt = pixel_core::sniff(&bytes);
            let img = pixel_core::decode(&bytes).map_err(|e| format!("{input}: {e}"))?;
            println!(
                "{input}: {:?} {}x{} frames={} loop={:?} orientation={}",
                fmt.unwrap(),
                img.width,
                img.height,
                img.frames.as_ref().map_or(1, Vec::len),
                img.loop_count,
                img.orientation
            );
            if let Some(out) = args.get(1) {
                std::fs::write(out, write_png(img.width, img.height, &img.rgba)).map_err(|e| e.to_string())?;
                println!("wrote {out}");
            }
            Ok(())
        }
        _ => Err("usage: pixel-check <in> [out.png] | --frames <in> <dir> | --compare <in> <shot.png> <r,g,b> [--orient] [--frame N]".into()),
    }
}

/// Source-over of straight `c` (alpha `a`) onto opaque `bg`, rounded the way Skia does it: the
/// decoder output is premultiplied (`c*a/255`, rounded), then `bg*(255-a)/255` (rounded) is added.
fn blend(c: u8, bg: u8, a: u32) -> u8 {
    fn mul255(x: u32, y: u32) -> u32 {
        let p = x * y + 128;
        (p + (p >> 8)) >> 8
    }
    if std::env::var_os("PIXEL_BLEND_EXACT").is_some() {
        return ((c as u32 * a + bg as u32 * (255 - a) + 127) / 255) as u8;
    }
    (mul255(c as u32, a) + mul255(bg as u32, 255 - a)).min(255) as u8
}

/// RGBA8 → PNG (colour type 6, filter 0, stored deflate).
fn write_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    use pixel_core::png::encode::{adler32, crc32};
    let mut raw = Vec::with_capacity(rgba.len() + h as usize);
    for row in rgba.chunks(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    if raw.is_empty() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(c) = chunks.next() {
        z.push(chunks.peek().is_none() as u8);
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}
