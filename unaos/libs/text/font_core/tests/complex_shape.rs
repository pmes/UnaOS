//! FONTBIDI M3: complex-script shaping against HarfBuzz — the engine Chromium shapes with — glyph for glyph.
//! tests/data/hb_shape_kat.tsv (oracle/gen_shape_kat.py) holds HarfBuzz's glyph ids, clusters, advances and offsets
//! for Arabic, Hebrew, Devanagari and Thai strings in the Noto fonts; the fonts are fetched at test time
//! (sha-pinned in tests/common, SKIP offline).

mod common;
use common::noto_font;
use font_core::{shape_run, Font, ShapeOptions};

#[test]
fn complex_scripts_match_harfbuzz() {
    // FONTBIDI_HB_KAT points at a larger sweep from oracle/gen_shape_kat.py (same format) when hunting bugs.
    let data = match std::env::var("FONTBIDI_HB_KAT") {
        Ok(p) => std::fs::read_to_string(p).unwrap(),
        Err(_) => include_str!("data/hb_shape_kat.tsv").to_string(),
    };
    let mut per: std::collections::BTreeMap<String, (usize, usize, usize)> = Default::default();
    let mut fails = Vec::new();
    let mut fonts: std::collections::HashMap<String, Option<Vec<u8>>> = Default::default();
    for line in data.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split('\t').collect();
        let bytes = fonts
            .entry(f[0].to_string())
            .or_insert_with(|| {
                if f[0].starts_with("fontbidi_") {
                    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/fontbidi_synth.ttf")).ok()
                } else if f[0].starts_with("DejaVu") {
                    std::fs::read(format!("/usr/share/fonts/truetype/dejavu/{}", f[0]))
                        .ok()
                        .filter(|d| common::sha256_hex(d) == f[1])
                } else if let Some(d) = std::env::var_os("FONTBIDI_FONT_DIR").filter(|_| !f[0].starts_with("Noto")) {
                    std::fs::read(std::path::Path::new(&d).join(f[0])).ok()
                } else {
                    noto_font(f[0], f[1])
                }
            })
            .clone();
        let Some(bytes) = bytes else { continue };
        let font = Font::parse(&bytes).unwrap();
        let text: String = f[4].split(' ').map(|h| char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap()).collect();
        let script: [u8; 4] = f[2].as_bytes().try_into().unwrap();
        let rtl = f[3] == "rtl";
        let want: Vec<[i64; 5]> = f[5]
            .split(';')
            .map(|g| {
                let v: Vec<i64> = g.split(',').map(|x| x.parse().unwrap()).collect();
                [v[0], v[1], v[2], v[3], v[4]]
            })
            .collect();
        let got: Vec<[i64; 5]> = shape_run(&font, &text, 0, text.len(), script, rtl, &ShapeOptions::default())
            .iter()
            .map(|g| [g.glyph as i64, g.cluster as i64, g.x_advance as i64, g.x_offset as i64, g.y_offset as i64])
            .collect();
        let e = per.entry(format!("{} ({})", f[2], f[0].trim_end_matches(".ttf"))).or_default();
        e.0 += 1;
        let strip = |v: &[[i64; 5]]| v.iter().map(|g| [g[0], g[2], g[3], g[4]]).collect::<Vec<_>>();
        if strip(&got) == strip(&want) {
            e.1 += 1;
            if got == want {
                e.2 += 1;
            } else if std::env::var("FONTBIDI_SHOW_CLUSTERS").is_ok() {
                eprintln!("CLUSTER {} {:?}\n  want {:?}\n  got  {:?}", f[2], text, want.iter().map(|g| g[1]).collect::<Vec<_>>(), got.iter().map(|g| g[1]).collect::<Vec<_>>());
            }
        } else if fails.len() < 12 {
            fails.push(format!("{} {:?}\n  want {:?}\n  got  {:?}", f[2], text, want, got));
        }
    }
    for (s, (n, ok, cl)) in &per {
        eprintln!("{s}: {ok}/{n} strings glyph-identical to HarfBuzz (ids, advances, offsets); {cl}/{n} also cluster-identical");
    }
    for f in &fails {
        eprintln!("DIFF {f}");
    }
    let (n, ok) = per.values().fold((0, 0), |a, v| (a.0 + v.0, a.1 + v.1));
    if n == 0 {
        eprintln!("SKIP: no Noto fonts");
        return;
    }
    let cl: usize = per.values().map(|v| v.2).sum();
    assert_eq!(ok, n, "glyph-level mismatches against HarfBuzz");
    assert_eq!(cl, n, "cluster mismatches against HarfBuzz");
}
