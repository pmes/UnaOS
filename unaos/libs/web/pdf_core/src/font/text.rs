// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Text extraction (ISO 32000-1 §9.10): [`TextCollector`] is a `render::TextSink`; PDF2's interpreter hands it every
//! shown string with its text matrix and CTM. It maps each code to Unicode (/ToUnicode, else the encoding's glyph name
//! through the AGL, else U+FFFD) and assembles [`TextSpan`]s by position: a space where the gap between two glyphs on
//! one line exceeds 0.25 em, a new span when the font or size changes or the baseline moves by more than 0.5 em.
//! [`spans_to_string`] joins spans into lines. Positions are the renderer's device space at 72 dpi (y down, the
//! CropBox's corner at the origin, /Rotate applied). Assembly assumes horizontal lines (rotated / vertical text order
//! is owed); `Page::text` runs the whole interpreter and discards the pixels (a text-only pass is owed).

use alloc::string::String;
use alloc::vec::Vec;

use super::glyphs::{GlyphSink, TextParams};
use super::{Fallback, Matrix, apply, mul};
use crate::doc::Page;
use crate::object::Dict;
use crate::render::{Glyphs, TextRun, TextSink};
use crate::Document;

/// A run of extracted text: its string, its box `[x0 y0 x1 y1]` (min / max corners in the renderer's 72 dpi device
/// space; each glyph's em box from 0.2 em below the baseline to 0.8 em above), the font's /BaseFont and the size in
/// device units.
#[derive(Clone, Debug, PartialEq)]
pub struct TextSpan {
    pub text: String,
    pub bbox: [f64; 4],
    pub font: String,
    pub size: f64,
}

/// Collects the spans of a page.
pub struct TextCollector<'d, 'f> {
    sink: GlyphSink<'d, 'f>,
    spans: Vec<TextSpan>,
    /// Per span: its baseline (the coordinate across the line) and where its last glyph ended (along the line).
    ends: Vec<(f64, f64)>,
}

fn matrix(t: &svg_core::geom::Transform) -> Matrix {
    [t.a, t.b, t.c, t.d, t.e, t.f]
}

impl<'d, 'f> TextCollector<'d, 'f> {
    pub fn new(doc: &'d Document, fallback: Fallback<'f>) -> Self {
        TextCollector { sink: GlyphSink::new(doc, fallback), spans: Vec::new(), ends: Vec::new() }
    }

    /// One shown string under `tm` x `ctm`; returns its tx (the renderer's advance).
    pub fn collect(&mut self, font: &Dict, bytes: &[u8], p: &TextParams, tm: &Matrix, ctm: &Matrix) -> f64 {
        let (placed, advance) = self.sink.layout(font, bytes, p);
        let f = self.sink.font(font);
        let name = f.base_font.clone();
        let chars: Vec<String> = placed.iter().map(|g| f.unicode(g.code).unwrap_or_else(|| String::from("\u{FFFD}"))).collect();
        let m = mul(tm, ctm);
        let size = p.size.abs() * font_core::fmath::sqrt((m[2] * m[2] + m[3] * m[3]) as f32) as f64;
        for (g, ch) in placed.iter().zip(chars) {
            let (x0, y) = apply(&m, g.pen, p.rise);
            let (x1, _) = apply(&m, g.pen + g.advance, p.rise);
            let (bx0, by0) = apply(&m, g.pen, p.rise - 0.2 * p.size);
            let (bx1, by1) = apply(&m, g.pen + g.advance, p.rise + 0.8 * p.size);
            let bbox = [bx0.min(bx1), by0.min(by1), bx0.max(bx1), by0.max(by1)];
            self.push(&ch, x0, x1.max(x0), y, size, bbox, &name);
        }
        advance
    }

    #[allow(clippy::too_many_arguments)]
    fn push(&mut self, ch: &str, x0: f64, x1: f64, y: f64, size: f64, glyph_box: [f64; 4], font: &str) {
        let em = if size > 1e-9 { size } else { 1.0 };
        if let (Some(span), Some(end)) = (self.spans.last_mut(), self.ends.last_mut()) {
            let same_line = (y - end.0).abs() <= 0.5 * em;
            if same_line && span.font == font && (span.size - size).abs() <= 0.01 * em {
                if x0 - end.1 > 0.25 * em && !span.text.ends_with(' ') && ch != " " {
                    span.text.push(' ');
                }
                span.text.push_str(ch);
                span.bbox = [span.bbox[0].min(glyph_box[0]), span.bbox[1].min(glyph_box[1]), span.bbox[2].max(glyph_box[2]), span.bbox[3].max(glyph_box[3])];
                end.1 = x1;
                return;
            }
        }
        self.spans.push(TextSpan { text: String::from(ch), bbox: glyph_box, font: String::from(font), size });
        self.ends.push((y, x1));
    }

    pub fn finish(self) -> Vec<TextSpan> {
        self.spans
    }
}

/// PDF2's font seam for extraction: the advance only (nothing painted).
impl TextSink for TextCollector<'_, '_> {
    fn text(&mut self, r: &TextRun) -> Glyphs {
        let p = TextParams { size: r.size, char_spacing: r.char_spacing, word_spacing: r.word_spacing, h_scale: r.h_scale, rise: r.rise };
        let advance = self.collect(r.font, r.bytes, &p, &matrix(&r.tm), &matrix(&r.ctm));
        Glyphs { advance, ..Glyphs::default() }
    }
}

/// Spans as plain text: a newline where the line moves by more than 0.5 em (box centres across the line), a space
/// between two spans of one line whose gap exceeds 0.25 em.
pub fn spans_to_string(spans: &[TextSpan]) -> String {
    let mut out = String::new();
    let mut prev: Option<&TextSpan> = None;
    for s in spans {
        if let Some(p) = prev {
            let em = p.size.max(s.size).max(1e-9);
            let (pc, sc) = ((p.bbox[1] + p.bbox[3]) / 2.0, (s.bbox[1] + s.bbox[3]) / 2.0);
            if (pc - sc).abs() > 0.5 * em {
                out.push('\n');
            } else if s.bbox[0] - p.bbox[2] > 0.25 * em && !out.ends_with(' ') && !s.text.starts_with(' ') {
                out.push(' ');
            }
        }
        out.push_str(&s.text);
        prev = Some(s);
    }
    out
}

impl Page {
    /// The page's text spans, in content order, through PDF2's interpreter (`Page::render` at 72 dpi with a
    /// [`TextCollector`] as its TextSink). A page the renderer refuses yields no spans.
    pub fn text(&self, doc: &Document, fallback: &Fallback<'_>) -> Vec<TextSpan> {
        let mut c = TextCollector::new(doc, *fallback);
        let _ = self.render(doc, 72.0, &mut c);
        c.finish()
    }
}
