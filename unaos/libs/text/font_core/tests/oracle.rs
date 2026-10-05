//! M2/M3 Chromium oracle comparison (opt-in: needs the rasters `oracle/run.sh` produces with Chromium).
//! `FONTCORE_ORACLE_DIR=<dir> cargo test --release -p font_core --test oracle -- --nocapture`
//! Without the variable the test prints SKIP and passes (the gate host may not have Chromium).
//!
//! Per job (string × font × size) FONTCORE shapes and draws the text at the same origin Chromium used and
//! compares coverage pixel by pixel. Chromium coverage = 255 - gray (black text on white, grayscale AA).
//! Reported: per-glyph mean absolute coverage error (over each glyph's bitmap box), the share of glyphs whose
//! mean error is within 8 levels, and shaping width vs canvas.measureText. Skia composites A8 glyph masks
//! through its gamma/contrast pre-blend (Chromium Linux: SK_GAMMA_EXPONENT 1.2, SK_GAMMA_CONTRAST 0.2), so
//! three models are reported: `exact` (FONTCORE's default exact-area coverage, compared raw — the SR48 gate),
//! `exact+pre` (same coverage through that documented pre-blend) and `aaa+pre` (RenderMode::SkiaAaa — edge y
//! snapped to 1/4 px as Skia's analytic AA does — through the pre-blend).

mod common;

use font_core::{draw_text, shape, Canvas, Font, GlyphCache, RenderMode, ShapeOptions};
use std::collections::BTreeMap;

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

/// Skia's A8 mask pre-blend for black text (SkMaskGamma, luminance 0): coverage a -> 1 - (1 - a')^(1/g),
/// a' = a + c·a·(1-a).
fn skia_lut(contrast: f64, gamma: f64) -> [u8; 256] {
    let mut t = [0u8; 256];
    for (i, v) in t.iter_mut().enumerate() {
        let a = i as f64 / 255.0;
        let a2 = a + (1.0 - a) * contrast * a;
        let m = 1.0 - (1.0 - a2).powf(1.0 / gamma);
        *v = (255.0 * m).round().clamp(0.0, 255.0) as u8;
    }
    t
}

struct Job {
    font: String,
    size: f32,
    text: String,
    left: f32,
    baseline: f32,
    measure: f32,
    w: usize,
    h: usize,
    chrome: Vec<u8>, // coverage
}

fn load_jobs(dir: &std::path::Path) -> Vec<(String, Job)> {
    let mut out = Vec::new();
    let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    names.sort();
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
        let (w, h, g) = read_pgm(&dir.join(format!("{id}.chrome.pgm"))).unwrap();
        out.push((
            id,
            Job {
                font: meta["font"].clone(),
                size: meta["size"].parse().unwrap(),
                text: meta["text"].clone(),
                left: meta["left"].parse().unwrap(),
                baseline: meta["baseline"].parse().unwrap(),
                measure: meta["measure"].parse().unwrap(),
                w,
                h,
                chrome: g.iter().map(|&v| 255 - v).collect(),
            },
        ));
    }
    out
}

#[derive(Default, Clone)]
struct Acc {
    glyphs: usize,
    within8: usize,
    err_sum: f64,
    pix: u64,
    pix_err: f64,
}

#[test]
fn chromium_oracle() {
    let Ok(dir) = std::env::var("FONTCORE_ORACLE_DIR") else {
        eprintln!("SKIP chromium_oracle: set FONTCORE_ORACLE_DIR (see oracle/run.sh)");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let jobs = load_jobs(&dir);
    assert!(!jobs.is_empty(), "no oracle jobs in {dir:?}");
    let identity: [u8; 256] = core::array::from_fn(|i| i as u8);
    let skia = skia_lut(0.2, 1.2);
    let mut fonts: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    // (size, lut name) -> accumulators
    let mut by_size: BTreeMap<(u32, &str), Acc> = BTreeMap::new();
    let mut width_ok = 0;
    let mut width_n = 0;
    let mut worst_w = 0f32;
    let mut report = String::new();
    let mut fit: BTreeMap<u8, (f64, u64)> = BTreeMap::new(); // our coverage -> mean chrome coverage
    for (id, j) in &jobs {
        let data = fonts.entry(j.font.clone()).or_insert_with(|| std::fs::read(&j.font).unwrap());
        let f = Font::parse(data).unwrap();
        let opts = ShapeOptions::default();
        // Shaping width vs canvas.measureText.
        let ours_w = font_core::measure(&f, &j.text, j.size, &opts);
        let dw = (ours_w - j.measure).abs();
        worst_w = worst_w.max(dw);
        width_n += 1;
        if dw <= 0.5 {
            width_ok += 1;
        } else {
            report += &format!("  WIDTH {id}: ours {ours_w:.3} chrome {:.3}\n", j.measure);
        }
        for (mode, models) in [
            (RenderMode::Exact, &[("exact", &identity), ("exact+pre", &skia)][..]),
            (RenderMode::SkiaAaa, &[("aaa+pre", &skia)][..]),
        ] {
            // Raster: draw at the same origin; register by the best integer shift in [-2, 2]^2.
            let mut cache = GlyphCache::with_mode(4096, mode);
            let mut cv = Canvas::new(j.w, j.h);
            draw_text(&mut cache, 0, &f, &j.text, j.size, j.left, j.baseline, &mut cv, &opts);
            let err_at = |dx: i32, dy: i32, lut: &[u8; 256]| -> f64 {
                let mut e = 0f64;
                for y in 0..j.h as i32 {
                    for x in 0..j.w as i32 {
                        let (sx, sy) = (x - dx, y - dy);
                        let o = if sx >= 0 && sy >= 0 && (sx as usize) < j.w && (sy as usize) < j.h {
                            lut[cv.data[sy as usize * j.w + sx as usize] as usize]
                        } else {
                            0
                        };
                        e += (o as f64 - j.chrome[y as usize * j.w + x as usize] as f64).abs();
                    }
                }
                e
            };
            let mut best = (0, 0, f64::MAX);
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let e = err_at(dx, dy, &identity);
                    if e < best.2 {
                        best = (dx, dy, e);
                    }
                }
            }
            let (dx, dy, _) = best;
            // Per-glyph boxes from the shaper (same placement rule as draw_text).
            let scale = j.size / f.units_per_em as f32;
            let mut pen = j.left;
            let mut boxes = Vec::new();
            for g in shape(&f, &j.text, &opts) {
                let (ix, sx) = font_core::cache::split_position(pen + g.x_offset as f32 * scale);
                let iy = (j.baseline - g.y_offset as f32 * scale).round() as i32;
                if let Some(bm) = cache.get(0, &f, g.glyph, j.size, sx, 0) {
                    if bm.width > 0 {
                        boxes.push((ix + bm.left + dx, iy + bm.top + dy, bm.width as i32, bm.height as i32));
                    }
                }
                pen += g.x_advance as f32 * scale;
            }
            for &(name, lut) in models {
                let acc = by_size.entry((j.size as u32, name)).or_default();
                for &(bx, by, bw, bh) in &boxes {
                    let mut e = 0f64;
                    let mut n = 0u64;
                    for y in by.max(0)..(by + bh).min(j.h as i32) {
                        for x in bx.max(0)..(bx + bw).min(j.w as i32) {
                            let (sx, sy) = ((x - dx) as usize, (y - dy) as usize);
                            let o = lut[cv.data[sy * j.w + sx] as usize] as f64;
                            let c = j.chrome[y as usize * j.w + x as usize] as f64;
                            e += (o - c).abs();
                            n += 1;
                        }
                    }
                    if n == 0 {
                        continue;
                    }
                    let m = e / n as f64;
                    acc.glyphs += 1;
                    acc.err_sum += m;
                    acc.within8 += (m <= 8.0) as usize;
                    acc.pix += n;
                    acc.pix_err += e;
                }
            }
            for y in 0..j.h as i32 {
                for x in 0..j.w as i32 {
                    let (sx, sy) = (x - dx, y - dy);
                    if sx < 0 || sy < 0 || sx as usize >= j.w || sy as usize >= j.h {
                        continue;
                    }
                    let o = cv.data[sy as usize * j.w + sx as usize];
                    let c = j.chrome[y as usize * j.w + x as usize];
                    if mode == RenderMode::Exact && (o > 0 || c > 0) {
                        let e = fit.entry(o / 16).or_default();
                        e.0 += c as f64;
                        e.1 += 1;
                    }
                }
            }
            if dx != 0 || dy != 0 {
                report += &format!("  shift {id} {mode:?}: dx={dx} dy={dy}\n");
            }
            if std::env::var("FONTCORE_ORACLE_DUMP").is_ok() {
                let mut pgm = format!("P5\n{} {}\n255\n", j.w, j.h).into_bytes();
                pgm.extend(cv.data.iter().map(|&v| 255 - v));
                std::fs::write(dir.join(format!("{id}.ours-{mode:?}.pgm")), pgm).unwrap();
            }
        }
    }
    eprintln!("ORACLE {} jobs from {dir:?}", jobs.len());
    eprint!("{report}");
    eprintln!("size  model      glyphs  mean-err/glyph  within-8   mean-err/pixel");
    let mut gate_fail = Vec::new();
    for ((size, name), a) in &by_size {
        let share = a.within8 as f64 / a.glyphs.max(1) as f64;
        eprintln!(
            "{size:>4}  {name:<9}  {:>6}  {:>14.2}  {:>7.1}%  {:>9.2}",
            a.glyphs,
            a.err_sum / a.glyphs.max(1) as f64,
            100.0 * share,
            a.pix_err / a.pix.max(1) as f64
        );
        if *name == "exact" && *size >= 16 && share < 0.90 {
            gate_fail.push(format!("{size}px within-8 {:.1}%", 100.0 * share));
        }
    }
    eprintln!("coverage transfer, exact mode (ours/16 -> mean chrome):");
    for (k, (s, n)) in &fit {
        eprint!(" {}:{:.0}", *k as u32 * 16 + 8, s / *n as f64);
    }
    eprintln!();
    eprintln!("WIDTH: {width_ok}/{width_n} within 0.5 px of canvas.measureText (worst {worst_w:.3} px)");
    assert_eq!(width_ok, width_n, "shaping widths off");
    assert!(gate_fail.is_empty(), "SR48 raster target missed: {gate_fail:?}");
}
