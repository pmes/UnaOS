//! Shaping and measuring (AETHERFONT M2): every width Aether lays out with comes from
//! `font_core::shape_fallback` over the run's face stack — bidi (UAX #9) runs in visual order, script runs,
//! per-grapheme-cluster fallback, GSUB (ligatures, contextual forms) and GPOS (kerning, marks), HarfBuzz-
//! identical on FONTBIDI's KATs. Results are cached in em per (selection, text, direction, ligatures).
//!
//! Spacing (css-text-3 §7.1, §8.2) is applied after shaping as Blink does: `letter-spacing` after every
//! grapheme cluster (and common ligatures off when it is non-zero), `word-spacing` on each word-separator
//! character.

use super::{run_faces, Face, FontSel};
use font_core::bidi::Direction;
use font_core::shape::{shape_fallback, ShapeOptions};
use std::collections::HashMap;
use std::rc::Rc;

/// One positioned glyph, in em of its face (multiply by the font size for px).
#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    pub face: &'static Face,
    pub gid: u16,
    /// Byte offset of the cluster's first character in the shaped text.
    pub cluster: usize,
    pub adv: f32,
    pub dx: f32,
    /// Up is positive (font convention).
    pub dy: f32,
}

/// A shaped text: glyphs in visual order and the total advance (em).
#[derive(Debug, Default)]
pub struct Shaped {
    pub glyphs: Vec<ShapedGlyph>,
    pub width: f32,
}

type Key = (FontSel, String, u8, bool);

thread_local! {
    static CACHE: std::cell::RefCell<(u64, HashMap<Key, Rc<Shaped>>)> = std::cell::RefCell::new((0, HashMap::new()));
}

const CACHE_CAP: usize = 65_536;

fn dir_code(d: Direction) -> u8 {
    match d {
        Direction::Auto => 0,
        Direction::Ltr => 1,
        Direction::Rtl => 2,
    }
}

/// Shapes `text` in `sel` with paragraph direction `dir` (cached).
pub fn shape(sel: &FontSel, text: &str, dir: Direction, ligatures: bool) -> Rc<Shaped> {
    let generation = super::webfont::generation();
    let key: Key = (*sel, text.to_string(), dir_code(dir), ligatures);
    if let Some(s) = CACHE.with(|c| {
        let c = c.borrow();
        (c.0 == generation).then(|| c.1.get(&key).cloned()).flatten()
    }) {
        return s;
    }
    let faces = run_faces(sel, text);
    let fonts: Vec<&font_core::Font> = faces.iter().map(|f| &f.font).collect();
    let opts = ShapeOptions { kerning: true, ligatures };
    let mut out = Shaped::default();
    if !fonts.is_empty() {
        for (fi, g) in shape_fallback(&fonts, text, &opts, dir) {
            let face = faces[fi];
            let upem = face.font.units_per_em.max(1) as f32;
            let s = ShapedGlyph {
                face,
                gid: g.glyph,
                cluster: g.cluster,
                adv: g.x_advance as f32 / upem,
                dx: g.x_offset as f32 / upem,
                dy: g.y_offset as f32 / upem,
            };
            out.width += s.adv;
            out.glyphs.push(s);
        }
    }
    let rc = Rc::new(out);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.0 != generation || c.1.len() >= CACHE_CAP {
            *c = (generation, HashMap::new());
        }
        c.1.insert(key, rc.clone());
    });
    rc
}

/// A word-separator character for `word-spacing` (css-text-3 §7.1).
pub fn is_word_separator(c: char) -> bool {
    matches!(c, ' ' | '\u{00A0}' | '\u{1361}' | '\u{10100}' | '\u{10101}' | '\u{1039F}' | '\u{1091F}')
}

/// The measurer the line breaker and every painter use: shaped widths at one size with the run's spacing.
#[derive(Clone, Copy, Debug)]
pub struct Advancer {
    pub sel: FontSel,
    pub size: f32,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub dir: Direction,
}

impl Advancer {
    pub fn new(sel: FontSel, size: f32, letter_spacing: f32) -> Self {
        Advancer { sel, size, letter_spacing, word_spacing: 0.0, dir: Direction::Ltr }
    }
    pub fn with_word_spacing(mut self, ws: f32) -> Self {
        self.word_spacing = ws;
        self
    }
    pub fn with_dir(mut self, rtl: bool) -> Self {
        self.dir = if rtl { Direction::Rtl } else { Direction::Ltr };
        self
    }
    /// Ligatures are on unless letter-spacing is set (Blink's FontDescription rule).
    pub fn ligatures(&self) -> bool {
        self.letter_spacing == 0.0
    }
    pub fn shape(&self, s: &str) -> Rc<Shaped> {
        shape(&self.sel, s, self.dir, self.ligatures())
    }
    /// Advance of one character in px.
    pub fn char(&self, c: char) -> f32 {
        let mut b = [0u8; 4];
        self.str(c.encode_utf8(&mut b))
    }
    /// Advance of `s` in px: the shaped width plus spacing.
    pub fn str(&self, s: &str) -> f32 {
        if s.is_empty() {
            return 0.0;
        }
        let mut w = self.shape(s).width * self.size;
        if self.letter_spacing != 0.0 {
            w += self.letter_spacing * font_core::grapheme::clusters(s).len() as f32;
        }
        if self.word_spacing != 0.0 {
            w += self.word_spacing * s.chars().filter(|&c| is_word_separator(c)).count() as f32;
        }
        w
    }
    /// The glyphs of `s` placed from pen 0 in px: (glyph, x of its origin, y offset down, advance), spacing
    /// applied after each cluster's last glyph in visual order.
    pub fn place(&self, s: &str) -> Vec<(ShapedGlyph, f32, f32)> {
        let sh = self.shape(s);
        let mut out = Vec::with_capacity(sh.glyphs.len());
        let mut pen = 0.0f32;
        let starts: std::collections::HashSet<usize> = if self.letter_spacing != 0.0 {
            font_core::grapheme::clusters(s).into_iter().map(|(a, _)| a).collect()
        } else {
            Default::default()
        };
        for (i, g) in sh.glyphs.iter().enumerate() {
            out.push((*g, pen + g.dx * self.size, -g.dy * self.size));
            pen += g.adv * self.size;
            let last_of_cluster = sh.glyphs.get(i + 1).is_none_or(|n| n.cluster != g.cluster);
            if last_of_cluster {
                if self.letter_spacing != 0.0 && starts.contains(&g.cluster) {
                    pen += self.letter_spacing;
                }
                if self.word_spacing != 0.0 && s[g.cluster..].chars().next().is_some_and(is_word_separator) {
                    pen += self.word_spacing;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::{face, SANS};

    #[test]
    fn kerning_ligatures_and_spacing_kat() {
        let sel = FontSel::new(SANS, 400, false);
        let Some(f) = face(&sel) else { return };
        if f.family != "Liberation Sans" {
            return;
        }
        let a = Advancer::new(sel, 16.0, 0.0);
        // GPOS kerning: "AV" is narrower than A + V
        assert!(a.str("AV") < a.char('A') + a.char('V') - 0.5, "{} vs {}", a.str("AV"), a.char('A') + a.char('V'));
        // letter-spacing after every cluster, word-spacing on spaces
        let ls = Advancer::new(sel, 16.0, 2.0);
        let w0 = Advancer::new(sel, 16.0, 0.0);
        assert!((ls.str("abc") - w0.str("abc") - 6.0).abs() < 0.3);
        let ws = Advancer::new(sel, 16.0, 0.0).with_word_spacing(5.0);
        assert!((ws.str("a b c") - w0.str("a b c") - 10.0).abs() < 1e-3);
        // placed glyphs end where the measure ends
        let p = ls.place("abc");
        let last = p.last().unwrap();
        assert!((last.1 + last.0.adv * 16.0 + 2.0 - ls.str("abc")).abs() < 1e-3);
    }

    #[test]
    fn rtl_runs_come_out_in_visual_order() {
        let sel = FontSel::new(SANS, 400, false);
        if face(&sel).is_none() {
            return;
        }
        // "ab אב": the Hebrew run is reversed in visual order (alef U+05D0 drawn right of bet U+05D1)
        let s = "ab \u{05D0}\u{05D1}";
        let sh = shape(&sel, s, Direction::Ltr, true);
        let clusters: Vec<usize> = sh.glyphs.iter().map(|g| g.cluster).collect();
        let alef = s.find('\u{05D0}').unwrap();
        let bet = s.find('\u{05D1}').unwrap();
        let pa = clusters.iter().position(|&c| c == alef).unwrap();
        let pb = clusters.iter().position(|&c| c == bet).unwrap();
        assert!(pb < pa, "bet before alef visually: {clusters:?}");
    }
}
