//! One line of text in a named family list (QUARTZFONT, SR64): what a toolkit's label, button or entry
//! needs, on the same path Aether paints page text with.
//!
//! - Faces: each family of the list resolved as an installed family ([`crate::installed_face`]: Chromium's
//!   Linux resolution, css-fonts-4 §5.2 selection, Blink's synthesis), then fontconfig's `sans`, then a
//!   per-character platform face ([`crate::platform_fallback`]) for whatever the list does not cover.
//! - Shaping: `font_core::shape_fallback` — UAX #9 bidi (paragraph direction from the first strong
//!   character), per-cluster fallback, GSUB/GPOS, kerning, ligatures — glyphs in visual order.
//! - Painting: [`crate::raster::draw_glyph`] (Skia analytic AA, quarter-pixel x phases, whole-pixel
//!   baselines, Skia's A8 pre-blend for the ink) composited source-over into a premultiplied buffer.
//! - Caret geometry over grapheme-cluster boundaries (UAX #29) for editing.

use crate::db::{Slant, Style};
use crate::Face;
use font_core::bidi::Direction;
use font_core::shape::{shape_fallback, ShapeOptions};

/// What a line of text asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// Installed family names, in preference order (`sans`, `serif`, `monospace` go through fontconfig).
    pub families: Vec<String>,
    /// Font size in px (the em square).
    pub size: f32,
    pub weight: u16,
    pub italic: bool,
}

impl TextStyle {
    pub fn new(families: &[&str], size: f32) -> Self {
        TextStyle { families: families.iter().map(|s| s.to_string()).collect(), size, weight: 400, italic: false }
    }

    pub fn want(&self) -> Style {
        Style { weight: self.weight as f32, slant: if self.italic { Slant::Italic } else { Slant::Normal }, stretch: 100.0 }
    }

    /// The faces of the family list, each once, in list order; `sans` (then the first face of the fallback
    /// order) when none of them is installed.
    pub fn stack(&self) -> Vec<&'static Face> {
        let want = self.want();
        let mut faces: Vec<&'static Face> = Vec::new();
        let push = |f: Option<&'static Face>, faces: &mut Vec<&'static Face>| {
            if let Some(f) = f {
                if !faces.iter().any(|x| x.id == f.id) {
                    faces.push(f);
                }
            }
        };
        for fam in &self.families {
            push(crate::installed_face(fam, want), &mut faces);
        }
        if faces.is_empty() {
            push(crate::installed_face("sans", want), &mut faces);
        }
        if faces.is_empty() {
            let d = crate::db::db();
            push(d.fallback_order().first().and_then(|&i| crate::load_face(&d.faces[i])), &mut faces);
        }
        faces
    }

    /// The primary face (the first of [`Self::stack`]).
    pub fn primary(&self) -> Option<&'static Face> {
        self.stack().first().copied()
    }

    /// (ascent, descent, line gap) of the primary face in whole px; a size-proportional guess without faces.
    pub fn metrics(&self) -> (f32, f32, f32) {
        match self.primary() {
            Some(f) => crate::line_metrics(f, self.size),
            None => ((self.size * 0.8).round(), (self.size * 0.2).round(), 0.0),
        }
    }

    /// The faces `text` is shaped over: the stack, then a platform face per character it does not cover.
    pub fn run_faces(&self, text: &str) -> Vec<&'static Face> {
        let mut faces = self.stack();
        let n = faces.len();
        let want = self.want();
        for c in text.chars() {
            if !crate::needs_glyph(c) || faces.iter().any(|f| f.font.glyph_index(c) != 0) {
                continue;
            }
            if let Some(f) = crate::platform_fallback(c, want) {
                if !faces[n..].iter().any(|x| x.id == f.id) {
                    faces.push(f);
                }
            }
        }
        faces
    }

    /// Shapes `text` as one line.
    pub fn shape(&self, text: &str) -> Line {
        let (ascent, descent, gap) = self.metrics();
        let faces = self.run_faces(text);
        let fonts: Vec<&font_core::Font> = faces.iter().map(|f| &f.font).collect();
        let mut glyphs = Vec::new();
        let mut pen = 0.0f32;
        if !fonts.is_empty() && !text.is_empty() {
            let opts = ShapeOptions { kerning: true, ligatures: true };
            for (fi, g) in shape_fallback(&fonts, text, &opts, Direction::Auto) {
                let face = faces[fi];
                let k = self.size / face.font.units_per_em.max(1) as f32;
                let adv = g.x_advance as f32 * k;
                glyphs.push(Glyph {
                    face,
                    gid: g.glyph,
                    cluster: g.cluster,
                    pen,
                    x: pen + g.x_offset as f32 * k,
                    dy: -(g.y_offset as f32) * k,
                    adv,
                });
                pen += adv;
            }
        }
        Line { glyphs, width: pen, size: self.size, ascent, descent, gap, len: text.len() }
    }

    /// The advance of `text` in px.
    pub fn width(&self, text: &str) -> f32 {
        self.shape(text).width
    }
}

/// One positioned glyph, px from the line's pen origin.
#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub face: &'static Face,
    pub gid: u16,
    /// Byte offset of its cluster's first character.
    pub cluster: usize,
    /// The pen position before the glyph.
    pub pen: f32,
    /// The glyph origin (pen plus GPOS x offset).
    pub x: f32,
    /// GPOS y offset, down positive.
    pub dy: f32,
    pub adv: f32,
}

/// A shaped line: glyphs in visual order.
#[derive(Clone, Debug)]
pub struct Line {
    pub glyphs: Vec<Glyph>,
    pub width: f32,
    pub size: f32,
    /// Whole-px metrics of the primary face.
    pub ascent: f32,
    pub descent: f32,
    pub gap: f32,
    len: usize,
}

/// Byte order of a premultiplied 32-bit buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelOrder {
    Rgba,
    /// Cairo ARGB32 / GDK B8G8R8A8 on little-endian.
    Bgra,
}

impl Line {
    /// The line box height (ascent + descent + gap, whole px).
    pub fn height(&self) -> f32 {
        self.ascent + self.descent + self.gap
    }

    /// The x of the caret before byte `idx` (a grapheme boundary): the pen before the first glyph of that
    /// cluster; inside a ligature, its advance split evenly by grapheme; the line's end for `idx` at the end.
    pub fn caret_x(&self, text: &str, idx: usize) -> f32 {
        if idx >= self.len || self.glyphs.is_empty() {
            return if idx == 0 { 0.0 } else { self.width };
        }
        if let Some(g) = self.glyphs.iter().find(|g| g.cluster == idx) {
            return if self.rtl_at(g) { g.pen + g.adv } else { g.pen };
        }
        // inside a cluster (a ligature covering several graphemes): interpolate over its graphemes
        let Some(g) = self.glyphs.iter().filter(|g| g.cluster <= idx).max_by_key(|g| g.cluster) else {
            return 0.0;
        };
        let end = self.glyphs.iter().map(|h| h.cluster).filter(|&c| c > g.cluster).min().unwrap_or(self.len);
        let bounds: Vec<usize> = font_core::grapheme::clusters(&text[g.cluster..end.min(text.len())])
            .into_iter()
            .map(|(a, _)| g.cluster + a)
            .collect();
        let n = bounds.len().max(1) as f32;
        let k = bounds.iter().filter(|&&b| b < idx).count() as f32;
        let w: f32 = self.glyphs.iter().filter(|h| h.cluster == g.cluster).map(|h| h.adv).sum();
        if self.rtl_at(g) { g.pen + w - w * k / n } else { g.pen + w * k / n }
    }

    /// A glyph sits in a right-to-left run when the next glyph in visual order starts an earlier cluster.
    fn rtl_at(&self, g: &Glyph) -> bool {
        let i = self.glyphs.iter().position(|h| std::ptr::eq(h, g)).unwrap_or(0);
        match (self.glyphs.get(i + 1), i.checked_sub(1).and_then(|p| self.glyphs.get(p))) {
            (Some(n), _) if n.cluster != g.cluster => n.cluster < g.cluster,
            (_, Some(p)) if p.cluster != g.cluster => p.cluster > g.cluster,
            _ => false,
        }
    }

    /// The grapheme boundary nearest to `x` (for a click).
    pub fn hit(&self, text: &str, x: f32) -> usize {
        let mut best = (f32::INFINITY, 0usize);
        let bounds = font_core::grapheme::clusters(text).into_iter().map(|(a, _)| a).chain(std::iter::once(text.len()));
        for b in bounds {
            let d = (self.caret_x(text, b) - x).abs();
            if d < best.0 {
                best = (d, b);
            }
        }
        best.1
    }

    /// Paints the line with its pen origin at (`x`, `baseline`) into a premultiplied `w`×`h` buffer of
    /// `stride` bytes per row, ink `rgba` (straight alpha), clipped to columns `clip.0..clip.1`.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &self,
        buf: &mut [u8],
        w: u32,
        h: u32,
        stride: usize,
        order: PixelOrder,
        x: f32,
        baseline: f32,
        rgba: [u8; 4],
        clip: (i32, i32),
    ) {
        let lut = crate::raster::preblend((rgba[0], rgba[1], rgba[2]));
        let (c0, c1) = (clip.0.max(0), clip.1.min(w as i32));
        let ink = match order {
            PixelOrder::Rgba => [rgba[0], rgba[1], rgba[2]],
            PixelOrder::Bgra => [rgba[2], rgba[1], rgba[0]],
        };
        for g in &self.glyphs {
            crate::raster::draw_glyph(g.face, g.gid, self.size, x + g.x, baseline + g.dy, &lut, &mut |px, py, a| {
                if px < c0 || px >= c1 || py < 0 || py >= h as i32 {
                    return;
                }
                let i = py as usize * stride + px as usize * 4;
                let Some(p) = buf.get_mut(i..i + 4) else { return };
                let a = a as u32 * rgba[3] as u32 / 255;
                let inv = 255 - a;
                for k in 0..3 {
                    p[k] = ((ink[k] as u32 * a + p[k] as u32 * inv + 127) / 255) as u8;
                }
                p[3] = ((a * 255 + p[3] as u32 * inv + 127) / 255) as u8;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dejavu() -> Option<TextStyle> {
        let st = TextStyle::new(&["DejaVu Sans"], 13.0);
        (st.primary()?.family == "DejaVu Sans").then_some(st)
    }

    /// Widths are font_core's shaped advances (kerning applied): `AV` is narrower than `A` + `V`.
    #[test]
    fn shaped_widths_and_kerning() {
        let Some(st) = dejavu() else { return };
        let (a, v, av) = (st.width("A"), st.width("V"), st.width("AV"));
        assert!(av < a + v - 0.1, "kerned {av} vs {a}+{v}");
        // DejaVu Sans 'A' advance is 1401/2048 em
        assert!((a - 13.0 * 1401.0 / 2048.0).abs() < 1e-3, "{a}");
        let (asc, desc, gap) = st.metrics();
        assert_eq!((asc, desc, gap), (12.0, 3.0, 0.0), "DejaVu Sans hhea 1901/-483/0 at 13px, rounded");
    }

    #[test]
    fn caret_and_hit() {
        let Some(st) = dejavu() else { return };
        let t = "Enter URL...";
        let l = st.shape(t);
        assert_eq!(l.caret_x(t, 0), 0.0);
        assert!((l.caret_x(t, t.len()) - l.width).abs() < 1e-4);
        let x5 = l.caret_x(t, 5);
        assert!((x5 - st.width("Enter")).abs() < 0.05, "{x5}");
        assert_eq!(l.hit(t, x5 + 0.4), 5);
        assert_eq!(l.hit(t, -10.0), 0);
        assert_eq!(l.hit(t, 1e4), t.len());
        // a fallback character (Hebrew) still gets glyphs and a positive width
        let h = st.shape("a\u{05D0}b");
        assert_eq!(h.glyphs.len(), 3);
        assert!(h.glyphs.iter().all(|g| g.gid != 0));
    }

    #[test]
    fn paint_inks_premultiplied() {
        let Some(st) = dejavu() else { return };
        let l = st.shape("C");
        let (w, h) = (16u32, 16u32);
        let mut buf = vec![255u8; (w * h * 4) as usize];
        l.paint(&mut buf, w, h, (w * 4) as usize, PixelOrder::Bgra, 2.0, 12.0, [0, 0, 0, 255], (0, w as i32));
        let dark = buf.chunks_exact(4).filter(|p| p[0] < 128).count();
        assert!(dark > 10, "{dark}");
        assert!(buf.chunks_exact(4).all(|p| p[3] == 255), "opaque stays opaque");
        // transparent destination: premultiplied, colour never exceeds alpha
        let mut t = vec![0u8; (w * h * 4) as usize];
        l.paint(&mut t, w, h, (w * 4) as usize, PixelOrder::Rgba, 2.0, 12.0, [200, 100, 50, 255], (0, w as i32));
        assert!(t.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
        assert!(t.chunks_exact(4).any(|p| p[3] == 255));
    }
}
