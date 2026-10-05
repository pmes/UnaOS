// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `facet` — the Images handler's command line, and the EYES subject (M4): everything the bus
//! verbs do, from a shell, writing PNGs a harness can score.
//!
//! ```text
//! facet info   <file>
//! facet render <file> --out shot.png [--width W --height H] [--zoom fit|actual|<percent>]
//!              [--rotate 0|90|180|270] [--flip-h] [--flip-v] [--pan X,Y] [EDITS]
//! facet export <file> --out edited.png [--overwrite] [EDITS]
//! facet diff   <a.png> <b.png> [--max-delta N] [--min-psnr DB]
//!
//! EDITS (applied in the order given, non-destructively, then baked):
//!   --crop X,Y,W,H   --turn N (clockwise quarter turns)   --mirror h|v
//!   --resize WxH (triangle filter)   --adjust B,C (CSS brightness(B) contrast(C))
//! ```
//!
//! `render` without `--width/--height` sizes the viewport to the picture as the view displays it
//! (at `--zoom`, after `--rotate`), so `facet render a.jpg --out a.png --zoom 200` is a 2x shot.

use bandy::signals::{FacetEdit, FacetFormat, FacetView, FacetZoom};
use facet::Facet;

fn usage() -> ! {
    eprintln!(
        "usage: facet info <file>\n       facet render <file> --out <png> [--width W --height H] [--zoom fit|actual|N] \
         [--rotate DEG] [--flip-h] [--flip-v] [--pan X,Y] [edits]\n       facet export <file> --out <png> [--overwrite] [edits]\n       \
         facet diff <a.png> <b.png> [--max-delta N] [--min-psnr DB]\nedits: --crop X,Y,W,H --turn N --mirror h|v --resize WxH --adjust B,C"
    );
    std::process::exit(2)
}

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("facet: {msg}");
    std::process::exit(1)
}

fn nums<T: std::str::FromStr>(s: &str, sep: char, n: usize, what: &str) -> Vec<T> {
    let v: Vec<T> = s.split(sep).filter_map(|p| p.trim().parse().ok()).collect();
    if v.len() != n {
        die(format!("{what}: expected {n} numbers separated by '{sep}', got {s:?}"));
    }
    v
}

struct Args {
    file: String,
    second: Option<String>,
    out: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    view: FacetView,
    edits: Vec<FacetEdit>,
    overwrite: bool,
    max_delta: Option<u8>,
    min_psnr: Option<f64>,
}

fn parse(mut it: impl Iterator<Item = String>) -> Args {
    let mut a = Args {
        file: String::new(),
        second: None,
        out: None,
        width: None,
        height: None,
        view: FacetView::default(),
        edits: Vec::new(),
        overwrite: false,
        max_delta: None,
        min_psnr: None,
    };
    let mut positional = Vec::new();
    while let Some(arg) = it.next() {
        let mut val = |name: &str| it.next().unwrap_or_else(|| die(format!("{name} needs a value")));
        match arg.as_str() {
            "--out" => a.out = Some(val("--out")),
            "--width" => a.width = Some(val("--width").parse().unwrap_or_else(|_| die("--width: not a number"))),
            "--height" => a.height = Some(val("--height").parse().unwrap_or_else(|_| die("--height: not a number"))),
            "--zoom" => {
                let z = val("--zoom");
                a.view.zoom = match z.trim_end_matches('%') {
                    "fit" => FacetZoom::Fit,
                    "actual" => FacetZoom::Actual,
                    p => FacetZoom::Percent(p.parse().unwrap_or_else(|_| die(format!("--zoom: {z:?}")))),
                };
            }
            "--rotate" => {
                let d: i64 = val("--rotate").parse().unwrap_or_else(|_| die("--rotate: degrees"));
                if d % 90 != 0 {
                    die("--rotate: a multiple of 90 degrees");
                }
                a.view.quarter_turns = (d / 90).rem_euclid(4) as u8;
            }
            "--flip-h" => a.view.flip_h = true,
            "--flip-v" => a.view.flip_v = true,
            "--pan" => {
                let v: Vec<i32> = nums(&val("--pan"), ',', 2, "--pan");
                a.view.pan_x = v[0];
                a.view.pan_y = v[1];
            }
            "--crop" => {
                let v: Vec<u32> = nums(&val("--crop"), ',', 4, "--crop");
                a.edits.push(FacetEdit::Crop { x: v[0], y: v[1], width: v[2], height: v[3] });
            }
            "--turn" => {
                let n: u8 = val("--turn").parse().unwrap_or_else(|_| die("--turn: quarter turns"));
                a.edits.push(FacetEdit::Rotate { quarter_turns: n % 4 });
            }
            "--mirror" => a.edits.push(FacetEdit::Flip {
                horizontal: match val("--mirror").as_str() {
                    "h" => true,
                    "v" => false,
                    m => die(format!("--mirror: h or v, got {m:?}")),
                },
            }),
            "--resize" => {
                let v: Vec<u32> = nums(&val("--resize"), 'x', 2, "--resize");
                a.edits.push(FacetEdit::Resize { width: v[0], height: v[1] });
            }
            "--adjust" => {
                let v: Vec<f32> = nums(&val("--adjust"), ',', 2, "--adjust");
                a.edits.push(FacetEdit::Adjust { brightness: v[0], contrast: v[1] });
            }
            "--overwrite" => a.overwrite = true,
            "--max-delta" => a.max_delta = Some(val("--max-delta").parse().unwrap_or_else(|_| die("--max-delta"))),
            "--min-psnr" => a.min_psnr = Some(val("--min-psnr").parse().unwrap_or_else(|_| die("--min-psnr"))),
            s if s.starts_with("--") => die(format!("unknown flag {s}")),
            _ => positional.push(arg),
        }
    }
    let mut p = positional.into_iter();
    a.file = p.next().unwrap_or_else(|| usage());
    a.second = p.next();
    a
}

fn open_edited(a: &Args) -> (Facet, u64) {
    let mut f = Facet::new();
    let (h, _) = f.open(&a.file).unwrap_or_else(|e| die(e));
    for e in &a.edits {
        f.edit(h, e).unwrap_or_else(|e| die(e));
    }
    (f, h)
}

fn write_png(path: &str, r: &facet::raster::Raster) {
    std::fs::write(path, facet::png::encode(r.width, r.height, &r.rgba)).unwrap_or_else(|e| die(format!("{path}: {e}")));
}

fn main() {
    let mut argv = std::env::args().skip(1);
    let verb = argv.next().unwrap_or_else(|| usage());
    let a = parse(argv);
    match verb.as_str() {
        "info" => {
            let (mut f, h) = open_edited(&a);
            let i = f.info(h).unwrap_or_else(|e| die(e));
            println!("path: {}", i.path);
            println!("format: {}", i.format);
            println!("stored: {}x{}", i.source_width, i.source_height);
            println!("displayed: {}x{}", i.width, i.height);
            println!("orientation: {}", i.orientation);
            println!("colour: {}", i.colour);
            println!("bit_depth: {}", i.bit_depth);
            println!("alpha: {}", i.has_alpha);
            println!("frames: {}", i.frames);
            println!("bytes: {}", i.bytes);
            println!("edits: {}", i.edits);
            println!("decoder: {}", f.source_name());
        }
        "render" => {
            let out = a.out.clone().unwrap_or_else(|| die("render needs --out"));
            let (mut f, h) = open_edited(&a);
            let baked = f.baked(h).unwrap_or_else(|e| die(e));
            let (iw, ih) = if a.view.quarter_turns % 2 == 1 { (baked.height, baked.width) } else { (baked.width, baked.height) };
            let s = match a.view.zoom {
                FacetZoom::Percent(p) => p.clamp(1, 6400) as f64 / 100.0,
                _ => 1.0,
            };
            let vw = a.width.unwrap_or(((iw as f64 * s).round() as u32).max(1));
            let vh = a.height.unwrap_or(((ih as f64 * s).round() as u32).max(1));
            let r = f.render(h, vw, vh, &a.view).unwrap_or_else(|e| die(e));
            write_png(&out, &r);
        }
        "export" => {
            let out = a.out.clone().unwrap_or_else(|| die("export needs --out"));
            let (mut f, h) = open_edited(&a);
            let n = f.export(h, &out, FacetFormat::Png, a.overwrite).unwrap_or_else(|e| die(e));
            println!("{out}: {n} bytes");
        }
        "diff" => {
            let b = a.second.clone().unwrap_or_else(|| usage());
            let mut f = Facet::new();
            let (ha, _) = f.open(&a.file).unwrap_or_else(|e| die(e));
            let (hb, _) = f.open(&b).unwrap_or_else(|e| die(e));
            let (ra, rb) = (f.baked(ha).unwrap_or_else(|e| die(e)), f.baked(hb).unwrap_or_else(|e| die(e)));
            if (ra.width, ra.height) != (rb.width, rb.height) {
                die(format!("size differs: {}x{} vs {}x{}", ra.width, ra.height, rb.width, rb.height));
            }
            let d = facet::compare(&ra, &rb);
            println!("max_delta: {}\npsnr_db: {:.2}\ndiffering_px: {}", d.max_delta, d.psnr_db, d.differing_px);
            let bad = a.max_delta.is_some_and(|m| d.max_delta > m) || a.min_psnr.is_some_and(|m| d.psnr_db < m);
            std::process::exit(if bad { 1 } else { 0 });
        }
        _ => usage(),
    }
}
