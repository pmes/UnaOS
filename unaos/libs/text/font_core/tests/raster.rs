//! M2: outlines → coverage. The rasterizer's exact-area property against fontTools' glyph areas, the glyph
//! cache, and a frozen slice of the Chromium oracle (tests/data/chrome_kats.rs) so the gate checks real
//! Chromium pixels even on a host without Chromium.

mod common;
#[allow(clippy::excessive_precision)]
#[path = "data/chrome_kats.rs"]
mod chrome_kats;
#[allow(clippy::excessive_precision)]
#[path = "data/kat_data.rs"]
mod kat_data;

use font_core::cache::split_position;
use font_core::{draw_text, rasterize_glyph, Canvas, Font, GlyphCache, RenderMode, ShapeOptions};

#[test]
fn coverage_integrates_to_the_outline_area() {
    // Exact-area accumulation: the per-pixel signed areas sum to the outline's signed area (fontTools
    // AreaPen) × scale² — exactly, up to f32 and curve flattening — for every KAT outline at 64 px. The 8-bit
    // nonzero coverage then matches |area| wherever contours do not overlap (overlaps fill once, as nonzero
    // must: e.g. Liberation's composite Å, whose ring overlaps the A).
    use font_core::raster::{Rasterizer, Scaled};
    let (mut n, mut overl) = (0, 0);
    let mut worst = 0f64;
    for k in kat_data::KATS {
        let Some(data) = common::load(k.path, Some(k.sha256)) else { continue };
        let f = Font::parse(&data).unwrap();
        let size = 64.0f32;
        let s = size as f64 / f.units_per_em as f64;
        for ol in k.outlines {
            let path = f.glyph_path(ol.glyph).unwrap();
            let mut r = Rasterizer::new(160, 160);
            r.snap_26_6 = false;
            path.replay(&mut Scaled { r: &mut r, scale: size / f.units_per_em as f32, ox: 40.0, oy: 110.0 });
            let signed: f64 = r.signed_area().iter().map(|&v| v as f64).sum();
            // y flips (font y up, pixels y down) and the accumulator counts downward edges positive: the two
            // sign changes cancel, so the totals compare directly. Flattening chords cost < 0.2 %.
            let want = ol.area * s * s;
            let rel = (signed - want).abs() / want.abs();
            worst = worst.max(rel);
            assert!(rel < 2e-3, "{} '{}': signed {signed:.3} vs {want:.3}", k.path, ol.ch);
            let ink: f64 = rasterize_glyph(&f, ol.glyph, size, 0.0, 0.0).unwrap().data.iter().map(|&v| v as f64 / 255.0).sum();
            assert!(ink <= want.abs() * 1.01 + 2.0, "{} '{}': nonzero ink {ink:.1} exceeds |area| {:.1}", k.path, ol.ch, want.abs());
            if ink < want.abs() * 0.99 - 2.0 {
                overl += 1;
                eprintln!("  overlap: {} '{}' ink {ink:.1} < |area| {:.1}", k.path, ol.ch, want.abs());
            }
            n += 1;
        }
    }
    eprintln!("area: {n} glyphs, worst signed-area error {:.4} %, {overl} with overlapping contours", worst * 100.0);
    assert!(n > 0);
    assert!(overl * 20 < n, "too many glyphs fill less than their area: {overl}");
}

#[test]
fn every_glyph_rasterizes() {
    for p in ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/opentype/tlwg/Loma.otf"] {
        let Some(data) = common::load(p, None) else { continue };
        let f = Font::parse(&data).unwrap();
        let mut inked = 0;
        for gid in 0..f.num_glyphs {
            if let Some(b) = rasterize_glyph(&f, gid, 16.0, 0.5, 0.0) {
                assert_eq!(b.data.len(), (b.width * b.height) as usize);
                inked += (b.width > 0) as u32;
            }
        }
        assert!(inked as f32 > f.num_glyphs as f32 * 0.8, "{p}: only {inked} glyphs inked");
    }
}

#[test]
fn subpixel_positions_and_cache() {
    assert_eq!(split_position(10.0), (10, 0));
    assert_eq!(split_position(10.24), (10, 1));
    assert_eq!(split_position(10.4), (10, 2));
    assert_eq!(split_position(10.9), (11, 0));
    assert_eq!(split_position(-0.25), (-1, 3));
    let Some(data) = common::load("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", None) else { return };
    let f = Font::parse(&data).unwrap();
    let g = f.glyph_index('l');
    let mut c = GlyphCache::new(16);
    let a = c.get(1, &f, g, 16.0, 0, 0).cloned().unwrap();
    let b = c.get(1, &f, g, 16.0, 2, 0).cloned().unwrap();
    assert_ne!(a.data, b.data, "half-pixel shift must change the stem coverage");
    let a2 = c.get(1, &f, g, 16.0, 0, 0).cloned().unwrap();
    assert_eq!(a, a2);
    assert_eq!((c.hits, c.misses), (1, 2));
    // 'l' is a box from x = 193 to 377 units = 1.508..2.945 px at 16 px: exact areas 0.492 / 0.945 per row,
    // and shifted by half a pixel 0.992 / 0.445 (8-bit floor of the 26.6-snapped edges).
    assert_eq!((a.left, a.width, &a.data[2..4]), (1, 2, &[124u8, 244][..]));
    assert_eq!((b.left, b.width, &b.data[2..4]), (2, 2, &[252u8, 116][..]));
    let ink = |bm: &font_core::GlyphBitmap| bm.data.iter().map(|&v| v as u32).sum::<u32>();
    assert!((ink(&a) as f64 - ink(&b) as f64).abs() / (ink(&a) as f64) < 0.01);
    // Different sizes / fonts are different keys; the cap clears instead of growing.
    for i in 0..40 {
        c.get(1, &f, g, 10.0 + i as f32, 0, 0);
    }
    assert!(c.len() <= 16);
}

/// Skia's black-text A8 pre-blend (contrast 0.2, gamma 1.2 — Chromium Linux).
fn skia_pre(a: u8) -> u8 {
    let a = a as f64 / 255.0;
    let a2 = a + (1.0 - a) * 0.2 * a;
    (255.0 * (1.0 - (1.0 - a2).powf(1.0 / 1.2))).round() as u8
}

#[test]
fn frozen_chromium_crops() {
    let mut checked = 0;
    for c in chrome_kats::CROPS {
        let Some(data) = common::load(c.font, None) else { continue };
        let f = Font::parse(&data).unwrap();
        // Mean |error| over pixels inked in either image (stricter than the per-glyph-box metric).
        for (mode, limit) in [(RenderMode::Exact, 10.0), (RenderMode::SkiaAaa, 6.0)] {
            let mut cache = GlyphCache::with_mode(512, mode);
            let mut cv = Canvas::new(c.x0 + c.w, c.y0 + c.h);
            draw_text(&mut cache, 0, &f, c.text, c.size, c.left, c.baseline, &mut cv, &ShapeOptions::default());
            let (mut e, mut n) = (0f64, 0u32);
            for y in 0..c.h {
                for x in 0..c.w {
                    let o = cv.data[(c.y0 + y) * cv.width + c.x0 + x];
                    let o = if mode == RenderMode::SkiaAaa { skia_pre(o) } else { o };
                    let ch = c.cov[y * c.w + x];
                    if o > 0 || ch > 0 {
                        e += (o as f64 - ch as f64).abs();
                        n += 1;
                    }
                }
            }
            let mean = e / n as f64;
            eprintln!("crop {} {mode:?}: mean |err| over inked pixels {mean:.2}", c.id);
            assert!(mean <= limit, "{} {mode:?} mean error {mean:.2} > {limit}", c.id);
        }
        checked += 1;
    }
    assert!(checked > 0);
}
