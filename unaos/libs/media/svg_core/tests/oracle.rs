//! The Chromium oracle (opt-in): renders every vector at its reference size and scores it against Chromium's
//! screenshot of the same file as an `<img>` on white. Set SVGCORE_VECTORS (the resvg `crates/resvg/tests`
//! directory, as `fetch-vectors.sh` lays it out) and SVGCORE_ORACLE_DIR (the screenshots from
//! `oracle/chromium-svg.cjs`). Optional: SVGCORE_ORACLE_OUT (per-file TSV), SVGCORE_DUMP (our PNGs),
//! SVGCORE_FILTER (substring of the path).
mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

#[test]
fn chromium_oracle() {
    let (Ok(vec_dir), Ok(chrome)) = (std::env::var("SVGCORE_VECTORS"), std::env::var("SVGCORE_ORACLE_DIR")) else {
        eprintln!("SKIP: set SVGCORE_VECTORS and SVGCORE_ORACLE_DIR to run the Chromium oracle");
        return;
    };
    let vec_dir = PathBuf::from(vec_dir);
    let chrome = PathBuf::from(chrome);
    let fonts = common::fonts_from(&vec_dir.join("fonts"));
    let opts = svg_core::Options { fonts: Some(&fonts), image_decoder: Some(common::decoder), languages: vec![] };
    let list = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors.txt")).unwrap();
    let filter = std::env::var("SVGCORE_FILTER").ok();
    let dump = std::env::var("SVGCORE_DUMP").ok().map(PathBuf::from);
    let mut per: BTreeMap<String, (usize, f64, f64, usize)> = BTreeMap::new();
    let mut rows = String::new();
    for line in list.lines() {
        if line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f[0] == "font" {
            continue;
        }
        let (cat, rel, w, h) = (f[0], f[1], f[3].parse::<u32>().unwrap(), f[4].parse::<u32>().unwrap());
        if let Some(fl) = &filter {
            if !rel.contains(fl.as_str()) {
                continue;
            }
        }
        let shot = chrome.join(format!("{}.png", rel.replace('/', "__")));
        let (Ok(svg_bytes), Ok(png)) = (std::fs::read(vec_dir.join(rel)), std::fs::read(&shot)) else { continue };
        let Ok(cimg) = pixel_core::decode(&png) else { continue };
        let crgb: Vec<u8> = cimg.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        let ours = match svg_core::Svg::parse(&svg_bytes) {
            Ok(s) => s.render_rgba(w, h, &opts).unwrap_or_else(|_| vec![0; (w * h * 4) as usize]),
            Err(_) => vec![0; (w * h * 4) as usize],
        };
        if let Some(d) = &dump {
            common::write_png(&d.join(format!("{}.png", rel.replace('/', "__"))), w as usize, h as usize, &ours);
        }
        let orgb = common::over_white(&ours);
        let (mean, within) = common::score(&orgb, &crgb);
        let e = per.entry(cat.to_string()).or_insert((0, 0.0, 0.0, 0));
        e.0 += 1;
        e.1 += mean;
        e.2 += within;
        if within >= 0.99 {
            e.3 += 1;
        }
        rows.push_str(&format!("{cat}\t{rel}\t{mean:.3}\t{:.4}\t{:08x}\n", within, common::crc32_of(&ours)));
    }
    let mut tot = (0usize, 0.0, 0.0, 0usize);
    eprintln!("category      files  mean|err|  within8  files>=99%");
    for (k, v) in &per {
        eprintln!("{k:<12} {:>6}  {:>8.3}  {:>6.2}%  {:>6}", v.0, v.1 / v.0 as f64, 100.0 * v.2 / v.0 as f64, v.3);
        tot.0 += v.0;
        tot.1 += v.1;
        tot.2 += v.2;
        tot.3 += v.3;
    }
    if tot.0 > 0 {
        eprintln!("{:<12} {:>6}  {:>8.3}  {:>6.2}%  {:>6}", "ALL", tot.0, tot.1 / tot.0 as f64, 100.0 * tot.2 / tot.0 as f64, tot.3);
    }
    if let Ok(o) = std::env::var("SVGCORE_ORACLE_OUT") {
        std::fs::write(o, rows).unwrap();
    }
}
