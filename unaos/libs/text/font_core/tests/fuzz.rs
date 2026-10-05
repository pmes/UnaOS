//! M1 robustness: truncated and mutated fonts must never panic (or hang) any public entry point.
//! Deterministic xorshift corpus; mutations are aimed inside individual tables so they reach the parsers.

mod common;

use font_core::{layout_lines, measure, rasterize_glyph, shape, Font, GlyphCache, ShapeOptions};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const FONTS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/opentype/tlwg/Loma.otf",
];

/// Drive every API over a (possibly broken) font.
fn exercise(data: &[u8]) -> usize {
    let Ok(f) = Font::parse(data) else { return 0 };
    let mut work = 0;
    for cp in (0x20u32..0x250).step_by(7).chain([0x391, 0x3B1, 0x416, 0x20AC, 0x1F600]) {
        let g = f.glyph_index(char::from_u32(cp).unwrap());
        let _ = f.advance(g);
        let _ = f.lsb(g);
        work += 1;
    }
    let n = f.num_glyphs.min(4000);
    let step = (n / 60).max(1) as usize;
    for gid in (0..n).step_by(step) {
        let _ = f.glyph_path(gid);
        let _ = f.glyph_name(gid);
        if let font_core::Outlines::Glyf(g) = f.outlines {
            let _ = g.components(gid);
            let _ = g.bbox(gid);
        }
        work += 1;
    }
    for gid in [0u16, 1, 3, 36, 68, 100] {
        let _ = rasterize_glyph(&f, gid, 13.0, 0.25, 0.0);
    }
    let o = ShapeOptions::default();
    let text = "AVATAR office Wa fi ffl — Ωμέγα Жёлтый 12.5%";
    let _ = shape(&f, text, &o);
    let _ = measure(&f, text, 16.0, &o);
    let _ = layout_lines(&f, text, 16.0, 60.0, &o);
    // FONTBIDI: the complex-script paths (bidi, joining, Indic reordering, Thai, contextual lookups, marks).
    let text2 = "مرحبا بالعالم لَا שָׁלוֹם עוֹלָם क्षत्रिय र्कि ि ภาษาไทย น้ำ (abc ١٢٣) ـّـ\u{200D}\u{200C}";
    let _ = shape(&f, text2, &o);
    let _ = layout_lines(&f, text2, 16.0, 60.0, &o);
    let mut cache = GlyphCache::new(64);
    let mut canvas = font_core::Canvas::new(200, 40);
    font_core::draw_text(&mut cache, 1, &f, text, 14.0, 2.0, 20.0, &mut canvas, &o);
    if let Some(c) = f.cmap() {
        for i in 0..c.len() {
            if let Some(st) = c.subtable(i) {
                let _ = st.glyph(0x41);
            }
        }
    }
    work
}

/// (offset, length) of each table in the directory.
fn tables(data: &[u8]) -> Vec<(usize, usize)> {
    let n = u16::from_be_bytes([data[4], data[5]]) as usize;
    (0..n)
        .filter_map(|i| {
            let r = 12 + 16 * i;
            let off = u32::from_be_bytes(data.get(r + 8..r + 12)?.try_into().ok()?) as usize;
            let len = u32::from_be_bytes(data.get(r + 12..r + 16)?.try_into().ok()?) as usize;
            Some((off, len))
        })
        .collect()
}

#[test]
fn truncated_fonts_never_panic() {
    let mut runs = 0;
    for p in FONTS {
        let Some(data) = common::load(p, None) else { continue };
        let mut cuts: Vec<usize> = (0..64).collect();
        for (off, len) in tables(&data) {
            cuts.extend([off, off + 1, off + len / 2, off + len.saturating_sub(1)]);
        }
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..40 {
            cuts.push(rng.below(data.len()));
        }
        for c in cuts {
            if c < data.len() {
                exercise(&data[..c]);
                runs += 1;
            }
        }
    }
    eprintln!("fuzz: {runs} truncations");
    assert!(runs > 0);
}

#[test]
fn mutated_fonts_never_panic() {
    let mut runs = 0;
    let mut parsed = 0;
    for (fi, p) in FONTS.iter().enumerate() {
        let Some(orig) = common::load(p, None) else { continue };
        let tabs = tables(&orig);
        let mut rng = Rng(0xD1B5_4A32_D192_ED03 ^ fi as u64);
        let rounds: usize = std::env::var("FONTCORE_FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
        for round in 0..rounds {
            let mut d = orig.clone();
            let (off, len) = tabs[rng.below(tabs.len())];
            // Hit the table's head harder: headers and offsets live there.
            let span = if round % 2 == 0 { len.min(64) } else { len };
            let flips = 1 + rng.below(24);
            for _ in 0..flips {
                let i = off + rng.below(span.max(1));
                if i < d.len() {
                    d[i] = match rng.below(4) {
                        0 => 0,
                        1 => 0xFF,
                        2 => d[i] ^ (1 << rng.below(8)),
                        _ => rng.next() as u8,
                    };
                }
            }
            if exercise(&d) > 0 {
                parsed += 1;
            }
            runs += 1;
        }
    }
    eprintln!("fuzz: {runs} mutations ({parsed} still parsed)");
    assert!(runs > 0);
}

#[test]
fn garbage_never_panics() {
    let mut rng = Rng(42);
    for len in [0usize, 1, 4, 11, 12, 28, 100, 1000, 5000] {
        for _ in 0..30 {
            let mut d: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            if d.len() >= 12 {
                d[..4].copy_from_slice(&[0, 1, 0, 0]);
                d[4] = 0;
                d[5] = (rng.below(20)) as u8;
            }
            exercise(&d);
        }
    }
}
