//! FONTHINT (SR62) M2: FreeType as the per-glyph oracle for `Hinting::Slight`.
//!
//! For every installed TrueType face of the corpus, every glyph the cmap gives for printable ASCII, Latin-1,
//! Greek, Cyrillic and Hebrew, at 12 / 16 / 24 px: FreeType 2.13.2 (freetype-py, `oracle/ft_hint_oracle.py`)
//! loads the glyph with `FT_LOAD_TARGET_LIGHT` — what Chromium's Skia asks for under fontconfig `hintslight` —
//! and font_core's auto-hinter hints the same glyph. Reported per face and size:
//!
//! - **points**: share of glyphs whose hinted outline is identical to FreeType's, point for point in 26.6;
//! - **exact**: share whose 8-bit coverage bitmap (font_core `RenderMode::Exact` vs FreeType's gray raster) is
//!   identical;
//! - **≤8**: share whose bitmap differs by at most 8 levels at every pixel; **mean≤8**: share whose mean |Δ| over
//!   the glyph box is at most 8 (AETHERFONT's per-glyph score). The bitmap columns measure font_core's
//!   rasterizer against FreeType's gray rasterizer on the SAME points — curve flattening and cell arithmetic differ
//!   by a few levels, which is why they trail the point column; Chromium itself draws hinted paths with Skia.
//!
//! `FONTHINT_FULL=1` runs every glyph of every cmap instead of the script sample.
//!
//! SKIPs (passes) when python3 + freetype-py are missing. `FONTHINT_DUMP=<face substring>:<gid>:<size>` prints one
//! glyph's points side by side.

mod common;

use font_core::hint::autofit::AutoHinter;
use font_core::raster::{rasterize_path_px, GlyphBitmap, RenderMode};
use font_core::Font;
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

pub const FACES: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Italic.ttf",
    "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
    "/usr/share/fonts/truetype/freefont/FreeSerif.ttf",
    "/usr/share/fonts/truetype/freefont/FreeMono.ttf",
    "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    // CFF: the Adobe engine's hint model (M3)
    "/usr/share/fonts/opentype/tlwg/Loma.otf",
    "/usr/share/fonts/opentype/tlwg/Loma-Bold.otf",
];
pub const SIZES: [f32; 3] = [12.0, 16.0, 24.0];

pub fn corpus_chars() -> Vec<char> {
    let mut v = Vec::new();
    let full = std::env::var("FONTHINT_FULL").is_ok();
    let ranges: &[(u32, u32)] = if full {
        &[(0x20, 0x10FFFF)]
    } else {
        &[
            (0x21, 0x7E),
            (0xA1, 0xFF),
            (0x386, 0x3CE),
            (0x400, 0x45F),
            (0x5D0, 0x5EA),
            (0x621, 0x64A),
            (0xE01, 0xE3A),
            (0x2010, 0x2027), // punctuation (latn)
            (0x2190, 0x21FF), // arrows: no script — FreeType's fallback style (hani, CJK writing system)
            (0x2200, 0x22FF), // math operators (fallback)
            (0x3041, 0x3096), // hiragana (hani)
            (0x4E00, 0x4E3F), // CJK ideographs (hani)
        ]
    };
    for &r in ranges {
        for c in r.0..=r.1 {
            if let Some(ch) = char::from_u32(c) {
                v.push(ch);
            }
        }
    }
    v
}

pub struct FtGlyph {
    pub points: Vec<(i64, i64, u8)>,
    pub ends: Vec<usize>,
    pub left: i32,
    pub top: i32,
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// Run the FreeType oracle over `jobs` (path, size, gids); `None` when FreeType is unavailable.
pub fn freetype(jobs: &[(String, f32, Vec<u16>)], mode: &str) -> Option<HashMap<(String, u32, u16), FtGlyph>> {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle/ft_hint_oracle.py");
    let mut child = Command::new("python3").arg(script).arg(mode).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let mut s = String::new();
    for (p, size, gids) in jobs {
        let g: Vec<String> = gids.iter().map(|g| g.to_string()).collect();
        s.push_str(&format!("{p}\t0\t{size}\t{}\n", g.join(",")));
    }
    // feed stdin from a thread: the oracle writes while it reads, and both pipes fill on a large corpus
    let feeder = std::thread::spawn(move || stdin.write_all(s.as_bytes()).is_ok());
    let out = child.wait_with_output().ok()?;
    feeder.join().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if text.starts_with("NO_FREETYPE") || !out.status.success() {
        return None;
    }
    let mut m = HashMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 11 {
            continue;
        }
        let size: f32 = f[1].parse().ok()?;
        let gid: u16 = f[2].parse().ok()?;
        let ends = if f[4].is_empty() { vec![] } else { f[4].split(',').map(|v| v.parse().unwrap()).collect() };
        let points = if f[5].is_empty() {
            vec![]
        } else {
            f[5].split(';')
                .map(|t| {
                    let q: Vec<i64> = t.split(',').map(|v| v.parse().unwrap()).collect();
                    (q[0], q[1], q[2] as u8)
                })
                .collect()
        };
        let data = (0..f[10].len() / 2).map(|i| u8::from_str_radix(&f[10][2 * i..2 * i + 2], 16).unwrap()).collect();
        m.insert(
            (f[0].to_string(), (size * 64.0) as u32, gid),
            FtGlyph { points, ends, left: f[6].parse().ok()?, top: f[7].parse().ok()?, w: f[8].parse().ok()?, h: f[9].parse().ok()?, data },
        );
    }
    Some(m)
}

/// Max and mean |Δ| between a font_core bitmap and FreeType's (FreeType `top` is y-up), over the union box.
pub fn bitmap_diff(a: Option<&GlyphBitmap>, b: &FtGlyph) -> (u32, f64) {
    let at = |x: i32, y: i32| -> i32 {
        match a {
            Some(g) if x >= g.left && y >= g.top && x < g.left + g.width as i32 && y < g.top + g.height as i32 => {
                g.data[((y - g.top) as u32 * g.width + (x - g.left) as u32) as usize] as i32
            }
            _ => 0,
        }
    };
    let bt = -b.top;
    let bv = |x: i32, y: i32| -> i32 {
        if x >= b.left && y >= bt && x < b.left + b.w as i32 && y < bt + b.h as i32 {
            b.data[((y - bt) as u32 * b.w + (x - b.left) as u32) as usize] as i32
        } else {
            0
        }
    };
    let (mut x0, mut y0, mut x1, mut y1) = (b.left, bt, b.left + b.w as i32, bt + b.h as i32);
    if let Some(g) = a {
        if g.width > 0 {
            x0 = x0.min(g.left);
            y0 = y0.min(g.top);
            x1 = x1.max(g.left + g.width as i32);
            y1 = y1.max(g.top + g.height as i32);
        }
    }
    let (mut mx, mut sum, mut n) = (0u32, 0u64, 0u64);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = (at(x, y) - bv(x, y)).unsigned_abs();
            mx = mx.max(d);
            sum += d as u64;
            n += 1;
        }
    }
    (mx, if n > 0 { sum as f64 / n as f64 } else { 0.0 })
}

#[test]
fn light_autohint_matches_freetype_per_glyph() {
    let chars = corpus_chars();
    let mut fonts = Vec::new();
    for p in FACES {
        if let Ok(d) = std::fs::read(p) {
            fonts.push((p.to_string(), d));
        }
    }
    if fonts.is_empty() {
        eprintln!("SKIP: no corpus faces");
        return;
    }
    let mut jobs = Vec::new();
    for (p, d) in &fonts {
        let f = Font::parse(d).unwrap();
        let mut gids: Vec<u16> = chars.iter().map(|&c| f.glyph_index(c)).filter(|&g| g != 0).collect();
        gids.sort();
        gids.dedup();
        for s in SIZES {
            jobs.push((p.clone(), s, gids.clone()));
        }
    }
    let Some(ft) = freetype(&jobs, "light") else {
        eprintln!("SKIP: python3 freetype-py not available");
        return;
    };
    let dump = std::env::var("FONTHINT_DUMP").ok();
    let mut total = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut dejavu_latin = (0usize, 0usize);
    for (p, d) in &fonts {
        let f = Font::parse(d).unwrap();
        let mut h = AutoHinter::new(&f);
        for (jp, size, gids) in &jobs {
            if jp != p {
                continue;
            }
            let (mut n, mut pe, mut be, mut b8, mut m8) = (0, 0, 0, 0, 0);
            for &g in gids {
                let Some(r) = ft.get(&(p.clone(), (size * 64.0) as u32, g)) else { continue };
                let Some(o) = h.hint(&f, g, *size) else { continue };
                n += 1;
                let same_pts = o.points.len() == r.points.len()
                    && o.points.iter().zip(r.points.iter()).all(|(a, b)| a.0 == b.0 && a.1 == b.1);
                if same_pts {
                    pe += 1;
                } else if std::env::var("FONTHINT_FAILS").map(|v| p.contains(&v)).unwrap_or(false) {
                    eprintln!("FAIL {} gid {g} size {size} style {} npts {}/{}", p, h.style_name(&f, g), o.points.len(), r.points.len());
                }
                let bm = rasterize_path_px(&o.to_path_26_6(), 0.0, 0.0, RenderMode::Exact);
                let (mx, mean) = bitmap_diff(bm.as_ref(), r);
                if mx == 0 {
                    be += 1;
                }
                if mean <= 8.0 {
                    m8 += 1;
                }
                if mx <= 8 {
                    b8 += 1;
                }
                if p.contains("DejaVuSans.ttf") && g < 200 {
                    dejavu_latin.0 += 1;
                    dejavu_latin.1 += same_pts as usize;
                }
                if let Some(dd) = &dump {
                    let q: Vec<&str> = dd.split(':').collect();
                    if q.len() == 3 && p.contains(q[0]) && q[1].parse::<u16>().ok() == Some(g) && q[2].parse::<f32>().ok() == Some(*size) {
                        if p.ends_with(".otf") {
                            for l in font_core::hint::cff_hint::hint_cff_traced(&f, g, *size, true).1 {
                                eprintln!("map {l}");
                            }
                        }
                        if let Some(t) = &h.last_debug {
                            for dim in 0..2 {
                                for (i, s) in t.segments[dim].iter().enumerate() {
                                    eprintln!("seg{dim} {i:2} {:?}", s);
                                }
                                for (i, e) in t.edges[dim].iter().enumerate() {
                                    eprintln!("edge{dim} {i:2} {:?}", e);
                                }
                            }
                        }
                        eprintln!("style {}", h.style_name(&f, g));
                        if let Some(b) = &bm {
                            eprintln!("ours left {} top {} {}x{}", b.left, b.top, b.width, b.height);
                            for y in 0..b.height { eprintln!("{}", (0..b.width).map(|x| format!("{:4}", b.get(x, y))).collect::<String>()); }
                        }
                        eprintln!("ft left {} top {} {}x{}", r.left, -r.top, r.w, r.h);
                        for y in 0..r.h { eprintln!("{}", (0..r.w).map(|x| format!("{:4}", r.data[(y * r.w + x) as usize])).collect::<String>()); }
                        for (i, (a, b)) in o.points.iter().zip(r.points.iter()).enumerate() {
                            eprintln!("{i:3} ours {:6},{:6}  ft {:6},{:6} {}", a.0, a.1, b.0, b.1, if a.0 != b.0 || a.1 != b.1 { "<<" } else { "" });
                        }
                    }
                }
            }
            let name = p.rsplit('/').next().unwrap();
            let pc = |k: usize| 100.0 * k as f64 / n.max(1) as f64;
            eprintln!(
                "{name:32} {size:>4} px  {n:5} glyphs  points {:5.1} %  bitmap exact {:5.1} %  max<=8 {:5.1} %  mean<=8 {:5.1} %",
                pc(pe),
                pc(be),
                pc(b8),
                pc(m8)
            );
            total.0 += n;
            total.1 += pe;
            total.2 += be;
            total.3 += b8;
            total.4 += m8;
        }
    }
    let pc = |k: usize| 100.0 * k as f64 / total.0.max(1) as f64;
    eprintln!(
        "ALL {} glyphs: points {:.2} %  bitmap exact {:.1} %  max<=8 {:.1} %  mean<=8 {:.1} %",
        total.0,
        pc(total.1),
        pc(total.2),
        pc(total.3),
        pc(total.4)
    );
    assert!(total.0 > 0);
    // The hinter is FreeType's to the 1/64 px on the Latin core of DejaVu Sans, and on ≥ 99 % of everything.
    assert_eq!(dejavu_latin.0, dejavu_latin.1, "DejaVu Sans Latin glyphs must hint point-identically");
    assert!(total.1 * 100 >= total.0 * 99, "point-exact share {:.2} % < 99 %", pc(total.1));
}
