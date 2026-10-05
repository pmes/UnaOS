//! M3: the shaper — widths against Chromium's canvas.measureText (frozen from the oracle run), line layout
//! against Chromium's own line breaking of the same paragraphs, and kerning / ligature behaviour.

mod common;
#[allow(clippy::excessive_precision)]
#[path = "data/chrome_kats.rs"]
mod chrome_kats;

use font_core::{layout_lines, measure, shape, Font, ShapeOptions};
use std::collections::BTreeMap;

fn fonts() -> BTreeMap<&'static str, Vec<u8>> {
    let mut m = BTreeMap::new();
    for (f, _, _, _) in chrome_kats::WIDTHS {
        if !m.contains_key(f) {
            if let Some(d) = common::load(f, None) {
                m.insert(*f, d);
            }
        }
    }
    m
}

#[test]
fn widths_match_canvas_measure_text() {
    let fonts = fonts();
    let (mut n, mut worst) = (0, 0f32);
    for &(fp, size, text, want) in chrome_kats::WIDTHS {
        let Some(d) = fonts.get(fp) else { continue };
        let f = Font::parse(d).unwrap();
        let got = measure(&f, text, size, &ShapeOptions::default());
        let e = (got - want).abs();
        worst = worst.max(e);
        assert!(e <= 0.5, "{fp} {size}px {text:?}: {got} vs Chromium {want}");
        n += 1;
    }
    eprintln!("widths: {n} strings within 0.5 px of canvas.measureText, worst {worst:.4} px");
    assert!(n > 0);
}

#[test]
fn line_layout_matches_chromium() {
    let data = include_str!("data/chrome_lines.tsv");
    let mut cache: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let (mut same, mut total, mut lines_same, mut lines_total) = (0, 0, 0, 0);
    let mut diffs = Vec::new();
    for l in data.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = l.splitn(5, '\t').collect();
        let (fp, size, width, starts, text) = (c[0], c[1].parse::<f32>().unwrap(), c[2].parse::<f32>().unwrap(), c[3], c[4]);
        if !cache.contains_key(fp) {
            match common::load(fp, None) {
                Some(d) => {
                    cache.insert(fp.to_string(), d);
                }
                None => continue,
            }
        }
        let f = Font::parse(&cache[fp]).unwrap();
        let want: Vec<usize> = starts.split(',').map(|s| s.parse().unwrap()).collect();
        let lines = layout_lines(&f, text, size, width, &ShapeOptions::default());
        let got: Vec<usize> = lines.iter().map(|ln| text[..ln.start].chars().count()).collect();
        total += 1;
        lines_total += want.len();
        lines_same += want.iter().filter(|s| got.contains(s)).count();
        if got == want {
            same += 1;
        } else if diffs.len() < 6 {
            diffs.push(format!("{size}px w={width} {fp}\n  chrome {want:?}\n  ours   {got:?}"));
        }
        for ln in &lines {
            assert!(ln.width <= width + 0.01 || text[ln.start..ln.end].trim_end().chars().all(|ch| ch != ' '),
                "line overflows without being a single unbreakable segment");
        }
    }
    eprintln!("line layout: {same}/{total} paragraphs identical to Chromium, {lines_same}/{lines_total} line starts");
    for d in &diffs {
        eprintln!("DIFF {d}");
    }
    assert!(total > 0);
    assert_eq!(same, total);
}

#[test]
fn kerning_and_ligatures() {
    let Some(d) = common::load("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", None) else { return };
    let f = Font::parse(&d).unwrap();
    let on = ShapeOptions::default();
    let off = ShapeOptions { kerning: false, ligatures: false };
    // GPOS kern: "AV" is tighter than the two advances.
    let k = shape(&f, "AV", &on);
    let plain = f.advance(f.glyph_index('A')) as i32 + f.advance(f.glyph_index('V')) as i32;
    assert!(k[0].x_advance + k[1].x_advance < plain);
    assert_eq!(shape(&f, "AV", &off).iter().map(|g| g.x_advance).sum::<i32>(), plain);
    // GSUB liga: "ffi" becomes one glyph whose cluster is the first f; spacing ends after it.
    let g = shape(&f, "office", &on);
    assert_eq!(g.len(), 4, "{g:?}");
    assert_eq!(g[1].cluster, 1);
    assert_eq!(g[2].cluster, 4);
    assert_eq!(shape(&f, "office", &off).len(), 6);
    // Script itemization: Greek and Cyrillic runs shape with their own scripts in one string.
    let mixed = shape(&f, "Aα Жж", &on);
    assert_eq!(mixed.len(), 5);
    assert!(mixed.iter().all(|g| g.glyph != 0));
    // Legacy `kern` fallback is used only without a GPOS kern feature: DejaVu has both, GPOS wins, and the
    // GPOS value for A,V equals the kern-table value in this font family.
    let kt = f.kern.unwrap().pair(f.glyph_index('A'), f.glyph_index('V')) as i32;
    assert_eq!(k[0].x_advance - f.advance(f.glyph_index('A')) as i32, kt);
}
