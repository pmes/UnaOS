//! FONTHINT (SR62) KATs: FreeType's `FT_LOAD_TARGET_LIGHT` outlines, frozen by `oracle/gen_hint_kat.py` into
//! `tests/data/hint_kat.tsv` (FreeType 2.13.2), must come out of font_core's hinters point for point in 26.6 —
//! the light auto-hinter's latin and CJK writing systems (DejaVu, Liberation, FreeSans with its OpenType-feature
//! styles, WenQuanYi), the CJK fallback for symbols, and the Adobe CFF hint model (Loma). Each face is used only
//! when the container's file has the sha256 the KAT was cut from (else that face SKIPs); the live, wide oracle is
//! `tests/hint_oracle.rs`.

mod common;

use font_core::hint::autofit::AutoHinter;
use font_core::hint::{Hinting, Outline};
use font_core::raster::{rasterize_glyph_hinted, RenderMode};
use font_core::{Font, GlyphCache};
use std::collections::HashMap;

#[test]
fn freetype_light_points_kat() {
    let src = include_str!("data/hint_kat.tsv");
    let mut fonts: HashMap<String, Option<Vec<u8>>> = HashMap::new();
    let mut hinters: HashMap<String, AutoHinter> = HashMap::new();
    let (mut n, mut ok, mut skipped) = (0, 0, 0);
    let mut bad = Vec::new();
    for line in src.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let (path, sha, size, gid) = (f[0], f[1], f[2].parse::<f32>().unwrap(), f[3].parse::<u16>().unwrap());
        let data = fonts.entry(path.to_string()).or_insert_with(|| common::load(path, Some(sha)));
        let Some(data) = data.as_ref() else {
            skipped += 1;
            continue;
        };
        let font = Font::parse(data).unwrap();
        let h = hinters.entry(path.to_string()).or_insert_with(|| AutoHinter::new(&font));
        let want: Vec<(i64, i64)> = if f.len() > 4 && !f[4].is_empty() {
            f[4].split(';')
                .map(|p| {
                    let (x, y) = p.split_once(',').unwrap();
                    (x.parse().unwrap(), y.parse().unwrap())
                })
                .collect()
        } else {
            Vec::new()
        };
        let got = h.hint(&font, gid, size).map(|o: Outline| o.points).unwrap_or_default();
        n += 1;
        if got == want {
            ok += 1;
        } else {
            bad.push(format!("{} gid {gid} @{size}", path.rsplit('/').next().unwrap()));
        }
    }
    eprintln!("hint KAT: {ok}/{n} glyph outlines identical to FreeType ({skipped} skipped: font file differs)");
    // DejaVu Sans Arabic glyphs aside (HarfBuzz `isol` shaping of blue characters is not modelled), every KAT
    // glyph here matches.
    assert_eq!(ok, n, "mismatches: {bad:?}");
}

#[test]
fn hinting_mode_keys_the_glyph_cache_and_snaps_stems() {
    let Some(data) = common::load("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", None) else { return };
    let font = Font::parse(&data).unwrap();
    let gid = font.glyph_index('H');
    let mut cache = GlyphCache::new(64);
    let plain = cache.get(1, &font, gid, 13.0, 0, 0).cloned().unwrap();
    cache.hinting = Hinting::Slight;
    let hinted = cache.get(1, &font, gid, 13.0, 0, 0).cloned().unwrap();
    assert_eq!(cache.len(), 2, "the hinting mode is part of the key");
    assert_ne!(plain, hinted);
    // light hinting is vertical only: the H's top and bottom land on the pixel grid, so its first and last rows
    // carry the same coverage as the stem rows inside (no partially covered edge row); the stems keep their
    // fractional x coverage
    let line = |b: &font_core::GlyphBitmap, y: u32| (0..b.width).map(|x| b.get(x, y)).collect::<Vec<u8>>();
    let h = hinted.height;
    assert_eq!(h, 9);
    assert_eq!(line(&hinted, 0), line(&hinted, 1));
    assert_eq!(line(&hinted, h - 1), line(&hinted, h - 2));
    assert!(line(&hinted, 0).iter().any(|&v| v != 0 && v != 255), "x stays unhinted");
    // and the entry point agrees with the cache
    let mut h = AutoHinter::new(&font);
    let direct = rasterize_glyph_hinted(&font, &mut h, gid, 13.0, 0.0, 0.0, RenderMode::Exact, Hinting::Slight).unwrap();
    assert_eq!(direct, hinted);
}
