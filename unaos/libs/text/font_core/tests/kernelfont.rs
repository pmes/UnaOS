// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! KERNELFONT (rmbp-ledger B359) M4 — the host proof of the kernel's text paint path (R78: no metal, no QEMU).
//!
//! The kernel's `video::text` is a thin fulfiller over `font_core::ui::Engine`; every test here drives that same
//! engine with the kernel's own sizing rules (`video::text`'s `restyle`: Body = DejaVu Sans Mono fitted to the
//! 7x16 console cell; Chrome = DejaVu Sans Bold fitted to the 9x20 bar cell by height and mean advance; Ui =
//! `system.display.font_size` CSS px x ppi / 96 capped by the 16 px cell) into a 0RGB canvas.
//!
//! * Always: the EDID ppi reading, the CSS→device rule, the pre-blend tables against their formula, the byte
//!   bound of the glyph cache and its eviction count, the run cache, the clip rule, and the login-screen strings'
//!   widths against Chromium's `measureText` (frozen in `tests/data/kernelfont_chrome.tsv`).
//! * Opt-in (`KERNELFONT_ORACLE_DIR`, written by `oracle/run_kernelfont.sh`): the login-screen mock drawn by the
//!   engine vs Chromium rendering the same strings in the same DejaVu file at the same size — per-glyph mean
//!   |error| and the share of glyphs within 8 levels, per face.

mod common;

use font_core::ui::{device_px, edid_ppi, Engine, Role, Style, PREBLEND_DARK, PREBLEND_LIGHT};
use font_core::Font;

const DEJAVU: &str = "/usr/share/fonts/truetype/dejavu";

fn file(name: &str) -> Option<&'static [u8]> {
    let d = common::load(&format!("{DEJAVU}/{name}"), None)?;
    Some(Box::leak(d.into_boxed_slice()))
}

/// The kernel's face set (`video::text`'s `FACES`, the DejaVu half), or `None` when the host lacks DejaVu.
fn engine(cap: usize) -> Option<Engine<'static>> {
    let mut e = Engine::new(cap);
    for (f, name, role, bold) in [
        ("DejaVuSans.ttf", "dejavu-sans", Role::Sans, false),
        ("DejaVuSans-Bold.ttf", "dejavu-sans-bold", Role::Sans, true),
        ("DejaVuSansMono.ttf", "dejavu-mono", Role::Mono, false),
        ("DejaVuSansMono-Bold.ttf", "dejavu-mono-bold", Role::Mono, true),
        ("DejaVuSerif.ttf", "dejavu-serif", Role::Serif, false),
    ] {
        e.add_face(name, role, bold, Font::parse(file(f)?).ok()?);
    }
    Some(e)
}

/// The kernel's three styles at `ppi` and `css_px` (`video::text::tt::restyle`, same arithmetic).
fn kernel_styles(e: &Engine, css_px: f32, ppi: u32) -> [(Style, f32); 3] {
    let body = e.fit_size(Role::Mono, Some(7.0), 16.0).unwrap();
    let chrome = e.fit_size_mean(Role::Sans, true, 9.0, 20.0).unwrap();
    let ui = device_px(css_px, ppi).min(e.fit_size(Role::Sans, None, 16.0).unwrap()).max(6.0);
    [
        (Style { role: Role::Mono, bold: false, size: body }, e.baseline_in_cell(Role::Mono, body, 16.0)),
        (Style { role: Role::Sans, bold: true, size: chrome }, e.baseline_in_cell(Role::Sans, chrome, 20.0)),
        (Style { role: Role::Sans, bold: false, size: ui }, e.baseline_in_cell(Role::Sans, ui, 16.0)),
    ]
}

#[test]
fn kernel_sizes() {
    let Some(e) = engine(1 << 20) else { return eprintln!("SKIP: DejaVu not installed") };
    for (ppi, label) in [(96, "no EDID / QEMU"), (227, "rMBP 13in 227 ppi"), (221, "rMBP 15in 221 ppi")] {
        let s = kernel_styles(&e, 13.0, ppi);
        eprintln!(
            "KERNELFONT sizes @{ppi} ppi ({label}): body=mono {:.2}px baseline {} | chrome=sans-bold {:.2}px baseline {} | ui=sans {:.2}px baseline {} (font_size 13 css px wants {:.2})",
            s[0].0.size, s[0].1, s[1].0.size, s[1].1, s[2].0.size, s[2].1, device_px(13.0, ppi)
        );
    }
}

#[test]
fn edid_ppi_and_css_px() {
    // A base block whose first detailed timing is 2560 px across 286 mm (the 13-inch rMBP's panel) -> 227 ppi.
    let mut e = [0u8; 128];
    e[..8].copy_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
    e[21] = 29; // cm, the fallback
    let d = &mut e[54..72];
    d[0] = 0x10;
    d[1] = 0x27; // pixel clock != 0: a timing descriptor
    d[2] = (2560 & 0xFF) as u8;
    d[4] = ((2560 >> 8) << 4) as u8;
    d[12] = (286 & 0xFF) as u8;
    d[14] = ((286 >> 8) << 4) as u8;
    assert_eq!(edid_ppi(&e), Some((2560, 286, 227)));
    // No size in the descriptor: the centimetre field (29 cm -> 290 mm) answers.
    e[54 + 12] = 0;
    e[54 + 14] = 0;
    assert_eq!(edid_ppi(&e), Some((2560, 290, 224)));
    // No timing descriptor at all.
    e[54] = 0;
    e[55] = 0;
    assert_eq!(edid_ppi(&e), None);
    assert_eq!(edid_ppi(&e[..100]), None);
    assert_eq!(device_px(13.0, 96), 13.0);
    assert_eq!(device_px(13.0, 0), 13.0);
    assert!((device_px(13.0, 227) - 30.739).abs() < 0.01);
}

#[test]
fn preblend_tables_are_skias_formula() {
    for i in 0..256 {
        let a = i as f64 / 255.0;
        let a2 = a + (1.0 - a) * 0.2 * a;
        let dark = (255.0 * (1.0 - (1.0 - a2).powf(1.0 / 1.2))).round() as u8;
        let light = (255.0 * a2).round() as u8;
        assert_eq!(PREBLEND_DARK[i], dark, "dark {i}");
        assert_eq!(PREBLEND_LIGHT[i], light, "light {i}");
    }
    assert_eq!((PREBLEND_DARK[0], PREBLEND_DARK[255]), (0, 255));
}

#[test]
fn glyph_cache_is_bounded_and_counts_evictions() {
    let cap = 16 * 1024;
    let Some(mut e) = engine(cap) else { return eprintln!("SKIP: DejaVu not installed") };
    let mut px = vec![0x00FF_FFFFu32; 600 * 40];
    let text = b"The quick brown fox jumps over the lazy dog 0123456789 ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    for size in [12.0f32, 16.0, 20.0, 24.0, 28.0] {
        let st = Style { role: Role::Sans, bold: false, size };
        e.draw_0rgb(&mut px, 600, 600, 40, 2.0, 30.0, text, st, 0);
        assert!(e.stats.cache_bytes <= cap, "cache {} > cap {cap}", e.stats.cache_bytes);
    }
    assert!(e.stats.evictions > 0 && e.stats.flushes > 0, "{:?}", e.stats);
    eprintln!("KERNELFONT cache @{cap} B: {:?}", e.stats);
    // A generous cap never evicts on the same workload.
    let mut big = engine(4 << 20).unwrap();
    for size in [12.0f32, 16.0, 20.0, 24.0, 28.0] {
        big.draw_0rgb(&mut px, 600, 600, 40, 2.0, 30.0, text, Style { role: Role::Sans, bold: false, size }, 0);
    }
    assert_eq!(big.stats.evictions, 0);
    eprintln!("KERNELFONT cache @4 MiB: {:?}", big.stats);
}

#[test]
fn run_cache_shapes_once_and_rows_match_the_whole_draw() {
    let Some(mut e) = engine(1 << 20) else { return eprintln!("SKIP: DejaVu not installed") };
    let st = kernel_styles(&e, 13.0, 96)[1];
    let s = b"Quarry  File  Edit  View  Window  Help";
    let w = 420usize;
    // Whole draw (draw_text's path) vs one scanline at a time (draw_row's path, the strip painters').
    let mut whole = vec![0x00E0_E0E0u32; w * 20];
    e.draw_0rgb(&mut whole, w, w, 20, 3.0, st.1, s, st.0, 0x0010_1010);
    let shaped = e.stats.runs_shaped;
    let mut rows = vec![0x00E0_E0E0u32; w * 20];
    for sy in 0..20 {
        let row = &mut rows[sy * w..(sy + 1) * w];
        e.draw_with(s, st.0, 3.0, st.1, None, Some(sy as i32), 0x0010_1010, &mut |x, y, a| {
            if y == sy as i32 && x >= 0 && (x as usize) < w {
                row[x as usize] = font_core::ui::blend(row[x as usize], 0x0010_1010, a);
            }
        });
    }
    assert_eq!(e.stats.runs_shaped, shaped, "a scanline draw re-shaped the run");
    assert_eq!(whole, rows, "row-at-a-time differs from the whole draw");
    assert!(whole.iter().any(|&p| p != 0x00E0_E0E0 && p != 0x0010_1010), "no anti-aliased pixel");
}

#[test]
fn clip_is_all_or_nothing() {
    let Some(mut e) = engine(1 << 20) else { return eprintln!("SKIP: DejaVu not installed") };
    let st = Style { role: Role::Sans, bold: false, size: 13.0 };
    let s = b"Password";
    let full = e.measure(s, st);
    let w = 200usize;
    let mut px = vec![0x00FF_FFFFu32; w * 16];
    let clip = (full * 0.6) as usize;
    let pen = e.draw_0rgb(&mut px, w, clip, 16, 0.0, 13.0, s, st, 0);
    assert!(pen <= clip as f32 + 0.01 && pen > 0.0, "pen {pen} clip {clip}");
    for y in 0..16 {
        for x in clip..w {
            assert_eq!(px[y * w + x], 0x00FF_FFFF, "ink past the clip at {x},{y}");
        }
    }
    // Unclipped, the pen is the shaped width.
    let mut px2 = vec![0x00FF_FFFFu32; w * 16];
    assert!((e.draw_0rgb(&mut px2, w, w, 16, 0.0, 13.0, s, st, 0) - full).abs() < 0.01);
}

#[test]
fn script_fallback_draws_through_the_stack() {
    let Some(mut e) = engine(1 << 20) else { return eprintln!("SKIP: DejaVu not installed") };
    // DejaVu Sans has Hebrew and Arabic; the stack must give the mono face's missing glyphs from the sans face.
    let st = Style { role: Role::Mono, bold: false, size: 12.0 };
    let p = e.placed("A שלום".as_bytes(), st);
    let sans = e.slots().iter().position(|s| s.name == "dejavu-sans").unwrap() as u8;
    assert!(p.iter().any(|g| g.face == sans && g.glyph != 0), "no glyph from the sans fallback: {p:?}");
}

/// `tests/data/kernelfont_chrome.tsv`: `id  font  size  measureText  text` from the Chromium oracle.
#[test]
fn login_strings_match_chromium_widths() {
    let Some(mut e) = engine(1 << 20) else { return eprintln!("SKIP: DejaVu not installed") };
    let data = include_str!("data/kernelfont_chrome.tsv");
    let (mut n, mut worst) = (0, 0f32);
    for l in data.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let c: Vec<&str> = l.splitn(5, '\t').collect();
        let (font, size, chrome, text) = (c[1], c[2].parse::<f32>().unwrap(), c[3].parse::<f32>().unwrap(), c[4]);
        let st = match font {
            "DejaVuSans-Bold.ttf" => Style { role: Role::Sans, bold: true, size },
            "DejaVuSansMono.ttf" => Style { role: Role::Mono, bold: false, size },
            _ => Style { role: Role::Sans, bold: false, size },
        };
        let ours = e.measure(text.as_bytes(), st);
        let d = (ours - chrome).abs();
        worst = worst.max(d);
        assert!(d <= 0.5, "{}: engine {ours:.3} px, Chromium {chrome:.3} px", c[0]);
        n += 1;
    }
    assert!(n >= 40, "only {n} frozen widths");
    eprintln!("KERNELFONT widths: {n}/{n} within 0.5 px of Chromium measureText, worst {worst:.4} px");
}

fn read_pgm(p: &std::path::Path) -> Option<(usize, usize, Vec<u8>)> {
    let b = std::fs::read(p).ok()?;
    let mut parts = Vec::new();
    let mut i = 0;
    while parts.len() < 4 {
        while b[i].is_ascii_whitespace() {
            i += 1;
        }
        let s = i;
        while !b[i].is_ascii_whitespace() {
            i += 1;
        }
        parts.push(String::from_utf8_lossy(&b[s..i]).to_string());
    }
    i += 1;
    let (w, h): (usize, usize) = (parts[1].parse().ok()?, parts[2].parse().ok()?);
    Some((w, h, b[i..i + w * h].to_vec()))
}

/// The opt-in raster comparison (`oracle/run_kernelfont.sh`).
#[test]
fn login_mock_vs_chromium_rasters() {
    let Ok(dir) = std::env::var("KERNELFONT_ORACLE_DIR") else {
        return eprintln!("SKIP login_mock_vs_chromium_rasters: set KERNELFONT_ORACLE_DIR (oracle/run_kernelfont.sh)");
    };
    let dir = std::path::PathBuf::from(dir);
    let mut e = engine(4 << 20).expect("DejaVu");
    let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|x| x.ok()).map(|x| x.path()).filter(|p| p.extension().is_some_and(|x| x == "meta")).collect();
    names.sort();
    let mut acc: std::collections::BTreeMap<String, (usize, usize, f64)> = Default::default();
    let mut tsv = String::from("# KERNELFONT (B359): Chromium 1194 canvas.measureText of the login-screen strings (oracle/run_kernelfont.sh)\n# id\tfont\tsize\tmeasure\ttext\n");
    for p in names {
        let id = p.file_stem().unwrap().to_string_lossy().to_string();
        let meta: std::collections::BTreeMap<String, String> =
            std::fs::read_to_string(&p).unwrap().lines().filter_map(|l| l.split_once('=').map(|(a, b)| (a.to_string(), b.to_string()))).collect();
        let (w, h, g) = read_pgm(&dir.join(format!("{id}.chrome.pgm"))).unwrap();
        let chrome: Vec<u8> = g.iter().map(|&v| 255 - v).collect();
        let font = meta["font"].rsplit('/').next().unwrap().to_string();
        let size: f32 = meta["size"].parse().unwrap();
        let text = meta["text"].clone();
        tsv += &format!("{id}\t{font}\t{size}\t{}\t{text}\n", meta["measure"]);
        let st = match font.as_str() {
            "DejaVuSans-Bold.ttf" => Style { role: Role::Sans, bold: true, size },
            "DejaVuSansMono.ttf" => Style { role: Role::Mono, bold: false, size },
            _ => Style { role: Role::Sans, bold: false, size },
        };
        let (left, base): (f32, f32) = (meta["left"].parse().unwrap(), meta["baseline"].parse().unwrap());
        let mut cv = vec![0x00FF_FFFFu32; w * h];
        e.draw_0rgb(&mut cv, w, w, h, left, base, text.as_bytes(), st, 0);
        let ours: Vec<u8> = cv.iter().map(|&p| 255 - (p & 0xFF) as u8).collect();
        let err_at = |dx: i32, dy: i32| -> f64 {
            let mut s = 0f64;
            for y in 0..h as i32 {
                for x in 0..w as i32 {
                    let (sx, sy) = (x - dx, y - dy);
                    let o = if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h { ours[sy as usize * w + sx as usize] } else { 0 };
                    s += (o as f64 - chrome[y as usize * w + x as usize] as f64).abs();
                }
            }
            s
        };
        let mut best = (0, 0, f64::MAX);
        for dy in -2..=2 {
            for dx in -2..=2 {
                let s = err_at(dx, dy);
                if s < best.2 {
                    best = (dx, dy, s);
                }
            }
        }
        let (dx, dy, _) = best;
        // Per-glyph boxes from the engine's own placement and rasterizer (same sub-pixel step as the draw).
        let face_font = e.slots().iter().find(|s| s.role == st.role && s.bold == st.bold).unwrap().font;
        let iy = base.round() as i32;
        let key = id.split('_').next().unwrap().to_string();
        let a = acc.entry(key).or_default();
        for g in e.placed(text.as_bytes(), st) {
            let (ix, sx) = font_core::cache::split_position(left + g.x);
            let Some(bm) = font_core::rasterize_glyph_mode(&face_font, g.glyph, size, sx as f32 / 4.0, 0.0, font_core::RenderMode::SkiaAaa) else { continue };
            let (bx, by) = (ix + bm.left + dx, iy + g.y.round() as i32 + bm.top + dy);
            let (mut s, mut n) = (0f64, 0u32);
            for y in by.max(0)..(by + bm.height as i32).min(h as i32) {
                for x in bx.max(0)..(bx + bm.width as i32).min(w as i32) {
                    let (sx2, sy2) = (x - dx, y - dy);
                    let o = if sx2 >= 0 && sy2 >= 0 && (sx2 as usize) < w && (sy2 as usize) < h { ours[sy2 as usize * w + sx2 as usize] } else { 0 };
                    s += (o as f64 - chrome[y as usize * w + x as usize] as f64).abs();
                    n += 1;
                }
            }
            if n > 0 {
                let m = s / n as f64;
                a.0 += 1;
                a.1 += (m <= 8.0) as usize;
                a.2 += m;
            }
        }
        // Evidence: Chromium | engine | |diff| x 4, stacked, as a PGM beside the job.
        let mut out = format!("P5\n{} {}\n255\n", w, h * 3).into_bytes();
        out.extend(g.iter().copied());
        out.extend(ours.iter().map(|&v| 255 - v));
        for y in 0..h {
            for x in 0..w {
                let (sx2, sy2) = (x as i32 - dx, y as i32 - dy);
                let o = if sx2 >= 0 && sy2 >= 0 && (sx2 as usize) < w && (sy2 as usize) < h { ours[sy2 as usize * w + sx2 as usize] } else { 0 };
                let d = ((o as i32 - chrome[y * w + x] as i32).unsigned_abs() * 4).min(255) as u8;
                out.push(255 - d);
            }
        }
        std::fs::write(dir.join(format!("{id}.kernelfont.pgm")), out).unwrap();
    }
    std::fs::write(dir.join("kernelfont_chrome.tsv"), tsv).unwrap();
    let (mut tg, mut tw) = (0, 0);
    for (k, (n, w8, s)) in &acc {
        eprintln!("KERNELFONT raster {k}: {n} glyphs, within-8 {:.1} %, mean |err| {:.2}", 100.0 * *w8 as f64 / *n as f64, s / *n as f64);
        tg += n;
        tw += w8;
    }
    let share = 100.0 * tw as f64 / tg.max(1) as f64;
    eprintln!("KERNELFONT raster all: {tg} glyphs, within-8 {share:.1} %");
    assert!(share >= 90.0, "within-8 share {share:.1} % < 90 %");
}
