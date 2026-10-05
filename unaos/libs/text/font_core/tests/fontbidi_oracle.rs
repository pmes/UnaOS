//! FONTBIDI M5: the Chromium oracle on Arabic, Urdu (Nastaliq), Hebrew, Devanagari, Thai and mixed-direction
//! paragraphs, each set in a font stack (the script's Noto font, then DejaVu Sans) exactly as the page does.
//!
//! * `frozen_widths_and_visual_order` (the gate): tests/data/fontbidi_chrome.tsv holds Chromium's
//!   `canvas.measureText` width and the visual order of grapheme clusters (from `Range.getClientRects`) for all 96
//!   jobs; font_core must agree within 0.5 px and byte for byte. Fonts are fetched (sha-pinned, SKIP offline).
//! * `chromium_oracle_rasters` (opt-in, `FONTBIDI_ORACLE_DIR` from oracle/run_fontbidi.sh): the same jobs drawn by
//!   font_core vs Chromium's screenshots, per-glyph mean |coverage error| and the share within 8 levels.

mod common;
use common::noto_font;
use font_core::bidi::{BidiInfo, Direction};
use font_core::{grapheme, shape_fallback, Canvas, Font, GlyphCache, RenderMode, ShapeOptions};
use std::collections::BTreeMap;

const NOTO: &[(&str, &str)] = &[
    ("NotoSansArabic-Regular.ttf", "bd86ca02f087d7f3c3788ba458fb6b73744c7639ed276b8d870dba6def6c40d0"),
    ("NotoSansHebrew-Regular.ttf", "04272f5600d0ec816d31d0df73b23aa8d3501ea359ebe820da31c11ffcf00853"),
    ("NotoSansDevanagari-Regular.ttf", "9c7d935139ea6a1e6ad9dbac4f6d27ece1e04bca8123c8888d00a0f9df4724cd"),
    ("NotoSansThai-Regular.ttf", "d4303fe9c63ebb72759ca8b6d2040c8ae81689f7d08d7b91c656154382b49313"),
    ("NotoNastaliqUrdu-Regular.ttf", "6decda3b03f6366dbd75e3a22544d248ca686b582f897fae8a7dc027302812e3"),
];
const DEJAVU: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf";
const DEJAVU_SHA: &str = "ae7b7855e115a5966d8b1b3f80f254ccc117ec86f9965e202ee2940453837280";

/// Font bytes by file name (Noto fetched, DejaVu from the container), sha-checked.
fn font_bytes(name: &str) -> Option<Vec<u8>> {
    if name.starts_with("DejaVu") {
        return std::fs::read(DEJAVU).ok().filter(|d| common::sha256_hex(d) == DEJAVU_SHA);
    }
    let sha = NOTO.iter().find(|n| n.0 == name)?.1;
    noto_font(name, sha)
}

fn base(p: &str) -> &str {
    p.rsplit('/').next().unwrap()
}

/// Width in px and grapheme visual order (UTF-8 byte offsets) as font_core computes them.
fn ours(fonts: &[Font], text: &str, size: f32) -> (f32, Vec<usize>) {
    let refs: Vec<&Font> = fonts.iter().collect();
    let glyphs = shape_fallback(&refs, text, &ShapeOptions::default(), Direction::Auto);
    let width: f32 = glyphs.iter().map(|(f, g)| g.x_advance as f32 * size / fonts[*f].units_per_em as f32).sum();
    let bidi = BidiInfo::new(text, Direction::Auto);
    let mut char_to_g = vec![0usize; bidi.chars.len()];
    let clusters = grapheme::clusters(text);
    for (s, e) in &clusters {
        for (ci, &off) in bidi.offsets[..bidi.chars.len()].iter().enumerate() {
            if off >= *s && off < *e {
                char_to_g[ci] = *s;
            }
        }
    }
    let mut order: Vec<usize> = Vec::new();
    for ci in bidi.visual_order(0, bidi.chars.len()) {
        let g = char_to_g[ci];
        if !order.contains(&g) {
            order.push(g);
        }
    }
    (width, order)
}

#[test]
fn frozen_widths_and_visual_order() {
    let data = include_str!("data/fontbidi_chrome.tsv");
    let mut cache: BTreeMap<String, Option<Vec<u8>>> = BTreeMap::new();
    let (mut n, mut w_ok, mut o_ok) = (0, 0, 0);
    let mut worst = 0f32;
    let mut fails = Vec::new();
    for line in data.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split('\t').collect();
        let (id, fonts, size, measure, order, text) = (f[0], f[1], f[2], f[3], f[4], f[5]);
        let bytes: Vec<Option<Vec<u8>>> =
            fonts.split(',').map(|p| cache.entry(p.to_string()).or_insert_with(|| font_bytes(p)).clone()).collect();
        if bytes.iter().any(|b| b.is_none()) {
            continue;
        }
        let parsed: Vec<Font> = bytes.iter().map(|b| Font::parse(b.as_ref().unwrap()).unwrap()).collect();
        let size: f32 = size.parse().unwrap();
        let measure: f32 = measure.parse().unwrap();
        let want: Vec<usize> = order.split(',').map(|x| x.parse().unwrap()).collect();
        let (w, o) = ours(&parsed, text, size);
        n += 1;
        let dw = (w - measure).abs();
        worst = worst.max(dw);
        if dw <= 0.5 {
            w_ok += 1;
        } else {
            fails.push(format!("WIDTH {id}: ours {w:.3} chrome {measure:.3}"));
        }
        if o == want {
            o_ok += 1;
        } else {
            fails.push(format!("ORDER {id}: ours {o:?}\n   chrome {want:?}"));
        }
    }
    eprintln!("FONTBIDI vs Chromium: {w_ok}/{n} widths within 0.5 px (worst {worst:.4} px); {o_ok}/{n} visual orders equal");
    for f in &fails {
        eprintln!("{f}");
    }
    if n == 0 {
        eprintln!("SKIP: fonts unavailable");
        return;
    }
    assert_eq!(w_ok, n);
    assert_eq!(o_ok, n);
}

fn read_pgm(p: &std::path::Path) -> Option<(usize, usize, Vec<u8>)> {
    let d = std::fs::read(p).ok()?;
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while d[i].is_ascii_whitespace() {
            i += 1;
        }
        let s = i;
        while !d[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(String::from_utf8_lossy(&d[s..i]).to_string());
    }
    i += 1;
    let w: usize = fields[1].parse().ok()?;
    let h: usize = fields[2].parse().ok()?;
    Some((w, h, d[i..i + w * h].to_vec()))
}

fn skia_lut(contrast: f64, gamma: f64) -> [u8; 256] {
    let mut t = [0u8; 256];
    for (i, v) in t.iter_mut().enumerate() {
        let a = i as f64 / 255.0;
        let a2 = a + (1.0 - a) * contrast * a;
        *v = (255.0 * (1.0 - (1.0 - a2).powf(1.0 / gamma))).round().clamp(0.0, 255.0) as u8;
    }
    t
}

#[test]
fn chromium_oracle_rasters() {
    let Ok(dir) = std::env::var("FONTBIDI_ORACLE_DIR") else {
        eprintln!("SKIP chromium_oracle_rasters: set FONTBIDI_ORACLE_DIR (see oracle/run_fontbidi.sh)");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    names.sort();
    let identity: [u8; 256] = core::array::from_fn(|i| i as u8);
    let skia = skia_lut(0.2, 1.2);
    let mut store: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    // (group, size, model) -> (glyphs, within8, err sum)
    let mut acc: BTreeMap<(String, u32, &str), (usize, usize, f64)> = BTreeMap::new();
    let mut frozen = String::from("# id\tfonts\tsize\tmeasureText\tgrapheme visual order (UTF-8 offsets)\ttext\n");
    for p in names {
        if p.extension().and_then(|e| e.to_str()) != Some("meta") {
            continue;
        }
        let id = p.file_stem().unwrap().to_string_lossy().to_string();
        let meta: BTreeMap<String, String> = std::fs::read_to_string(&p)
            .unwrap()
            .lines()
            .filter_map(|l| l.split_once('=').map(|(a, b)| (a.to_string(), b.to_string())))
            .collect();
        let fonts: Vec<String> = meta["fonts"].split(',').map(String::from).collect();
        frozen += &format!(
            "{id}\t{}\t{}\t{}\t{}\t{}\n",
            fonts.iter().map(|f| base(f)).collect::<Vec<_>>().join(","),
            meta["size"],
            meta["measure"],
            meta["order"],
            meta["text"]
        );
        for f in &fonts {
            store.entry(f.clone()).or_insert_with(|| std::fs::read(f).unwrap());
        }
        let parsed: Vec<Font> = fonts.iter().map(|f| Font::parse(&store[f]).unwrap()).collect();
        let refs: Vec<&Font> = parsed.iter().collect();
        let size: f32 = meta["size"].parse().unwrap();
        let left: f32 = meta["left"].parse().unwrap();
        let baseline: f32 = meta["baseline"].parse().unwrap();
        let (w, h, g) = read_pgm(&dir.join(format!("{id}.chrome.pgm"))).unwrap();
        let chrome: Vec<u8> = g.iter().map(|&v| 255 - v).collect();
        let text = &meta["text"];
        let glyphs = shape_fallback(&refs, text, &ShapeOptions::default(), Direction::Auto);
        for (mode, models) in [
            (RenderMode::Exact, &[("exact", &identity), ("exact+pre", &skia)][..]),
            (RenderMode::SkiaAaa, &[("aaa+pre", &skia)][..]),
        ] {
            let mut cache = GlyphCache::with_mode(8192, mode);
            let mut cv = Canvas::new(w, h);
            let mut boxes = Vec::new();
            let mut pen = left;
            for &(fi, gp) in &glyphs {
                let font = &parsed[fi];
                let scale = size / font.units_per_em as f32;
                let (ix, sx) = font_core::cache::split_position(pen + gp.x_offset as f32 * scale);
                let iy = (baseline - gp.y_offset as f32 * scale).round() as i32;
                if let Some(bm) = cache.get(fi as u32, font, gp.glyph, size, sx, 0) {
                    for row in 0..bm.height as i32 {
                        for col in 0..bm.width as i32 {
                            let (cx, cy) = (ix + bm.left + col, iy + bm.top + row);
                            if cx < 0 || cy < 0 || cx >= w as i32 || cy >= h as i32 {
                                continue;
                            }
                            let a = bm.data[(row * bm.width as i32 + col) as usize] as u32;
                            let d = &mut cv.data[cy as usize * w + cx as usize];
                            let b = *d as u32;
                            *d = (a + b - (a * b + 127) / 255).min(255) as u8;
                        }
                    }
                    if bm.width > 0 {
                        boxes.push((ix + bm.left, iy + bm.top, bm.width as i32, bm.height as i32));
                    }
                }
                pen += gp.x_advance as f32 * scale;
            }
            // Register by the best integer shift in [-2, 2]^2.
            let mut best = (0i32, 0i32, f64::MAX);
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let mut e = 0f64;
                    for y in 0..h as i32 {
                        for x in 0..w as i32 {
                            let (sx, sy) = (x - dx, y - dy);
                            let o = if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h { cv.data[sy as usize * w + sx as usize] } else { 0 };
                            e += (o as f64 - chrome[y as usize * w + x as usize] as f64).abs();
                        }
                    }
                    if e < best.2 {
                        best = (dx, dy, e);
                    }
                }
            }
            let (dx, dy, _) = best;
            let group = id.split('_').next().unwrap().trim_end_matches(char::is_numeric).to_string();
            for &(name, lut) in models {
                let a = acc.entry((group.clone(), size as u32, name)).or_default();
                let a_all = (0usize, 0usize, 0f64);
                let _ = a_all;
                for &(bx, by, bw, bh) in &boxes {
                    let (mut e, mut n) = (0f64, 0u64);
                    for y in (by + dy).max(0)..(by + dy + bh).min(h as i32) {
                        for x in (bx + dx).max(0)..(bx + dx + bw).min(w as i32) {
                            let (sx, sy) = ((x - dx) as usize, (y - dy) as usize);
                            e += (lut[cv.data[sy * w + sx] as usize] as f64 - chrome[y as usize * w + x as usize] as f64).abs();
                            n += 1;
                        }
                    }
                    if n == 0 {
                        continue;
                    }
                    let m = e / n as f64;
                    a.0 += 1;
                    a.2 += m;
                    if m <= 8.0 {
                        a.1 += 1;
                    }
                }
            }
        }
    }
    if let Ok(out) = std::env::var("FONTBIDI_FROZEN_OUT") {
        std::fs::write(out, frozen).unwrap();
    }
    let mut totals: BTreeMap<(u32, &str), (usize, usize, f64)> = BTreeMap::new();
    for ((group, size, model), (n, w8, e)) in &acc {
        eprintln!("{group:>3} {size:>2}px {model:<9}: {n:>4} glyphs, mean |err| {:.2}, within 8: {:.1} %", e / *n as f64, 100.0 * *w8 as f64 / *n as f64);
        let t = totals.entry((*size, model)).or_default();
        t.0 += n;
        t.1 += w8;
        t.2 += e;
    }
    for ((size, model), (n, w8, e)) in &totals {
        eprintln!("ALL {size:>2}px {model:<9}: {n:>4} glyphs, mean |err| {:.2}, within 8: {:.1} %", e / *n as f64, 100.0 * *w8 as f64 / *n as f64);
        if *size >= 16 && *model == "exact" {
            assert!(*w8 as f64 / *n as f64 >= 0.90, "{size}px within-8 share below 90 %");
        }
    }
}
