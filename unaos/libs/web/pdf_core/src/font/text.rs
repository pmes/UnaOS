// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Text extraction (ISO 32000-1 §9.10): [`TextCollector`] takes every shown string with its text matrix and CTM,
//! maps each code to Unicode (/ToUnicode, else the encoding's glyph name through the AGL, else U+FFFD) and assembles
//! [`TextSpan`]s by position: a space where the gap between two glyphs on one baseline exceeds 0.25 em, a new span
//! when the font or size changes or the baseline moves by more than 0.5 em. [`spans_to_string`] joins spans into
//! lines. Assembly assumes horizontal, unrotated lines (rotated / vertical text order is owed).

use alloc::string::String;
use alloc::vec::Vec;

use super::glyphs::{GlyphSink, TextParams};
use super::{Fallback, IDENTITY, Matrix, apply, mul};
use crate::content::ops;
use crate::doc::Page;
use crate::object::{Dict, Object};
use crate::Document;

/// A run of extracted text: its string, its box in user space `[x0 y0 x1 y1]` (y up; the em box from 0.2 em below
/// the baseline to 0.8 em above), the font's /BaseFont and the size in user space units.
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
    /// Per span: its baseline y and the x where its last glyph ended.
    ends: Vec<(f64, f64)>,
}

impl<'d, 'f> TextCollector<'d, 'f> {
    pub fn new(doc: &'d Document, fallback: Fallback<'f>) -> Self {
        TextCollector { sink: GlyphSink::new(doc, fallback), spans: Vec::new(), ends: Vec::new() }
    }

    /// One shown string; returns its tx (the renderer's advance).
    pub fn text(&mut self, font: &Dict, bytes: &[u8], p: &TextParams, tm: &Matrix, ctm: &Matrix) -> f64 {
        let (placed, advance) = self.sink.layout(font, bytes, p);
        let f = self.sink.font(font);
        let name = f.base_font.clone();
        let chars: Vec<String> = placed.iter().map(|g| f.unicode(g.code).unwrap_or_else(|| String::from("\u{FFFD}"))).collect();
        let m = mul(tm, ctm);
        let size = p.size.abs() * font_core::fmath::sqrt((m[2] * m[2] + m[3] * m[3]) as f32) as f64;
        for (g, ch) in placed.iter().zip(chars) {
            let (x0, y) = apply(&m, g.pen, p.rise);
            let (x1, _) = apply(&m, g.pen + g.advance, p.rise);
            self.push(&ch, x0, x1.max(x0), y, size, &name);
        }
        advance
    }

    fn push(&mut self, ch: &str, x0: f64, x1: f64, y: f64, size: f64, font: &str) {
        let em = if size > 1e-9 { size } else { 1.0 };
        let glyph_box = [x0, y - 0.2 * em, x1, y + 0.8 * em];
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

/// Spans as plain text: a newline where the baseline moves by more than 0.5 em, a space between two spans of one
/// line whose gap exceeds 0.25 em.
pub fn spans_to_string(spans: &[TextSpan]) -> String {
    let mut out = String::new();
    let mut prev: Option<&TextSpan> = None;
    for s in spans {
        if let Some(p) = prev {
            let em = p.size.max(s.size).max(1e-9);
            let (pb, sb) = (p.bbox[1] + 0.2 * p.size, s.bbox[1] + 0.2 * s.size);
            if (pb - sb).abs() > 0.5 * em {
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
    /// The page's text spans, in content order.
    pub fn text(&self, doc: &Document, fallback: &Fallback<'_>) -> Vec<TextSpan> {
        let mut c = TextCollector::new(doc, *fallback);
        walk(doc, &self.contents, &self.resources, IDENTITY, &mut |font, bytes, p, tm, ctm| c.text(font, bytes, p, tm, ctm));
        c.finish()
    }
}

/// The text state the walk carries (part of the graphics state, saved by `q`).
#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    font: Option<Dict>,
    p: TextParams,
    leading: f64,
}

/// The deepest form XObject nesting the walk follows.
const MAX_FORM_DEPTH: u8 = 8;

/// FOLD SEAM: retires onto render's interpreter (PDF2). A text-only walk of a content stream (BT ET Tf Tc Tw Tz TL
/// Ts Tr Td TD Tm T* Tj TJ ' " q Q cm, and Do of form XObjects to depth 8) so `Page::text` and `pdfprobe --text` run
/// before PDF2's interpreter is in the tree. `f` gets each shown string (font dictionary, bytes, text state, Tm, CTM)
/// and returns its tx. When PDF2 lands, `Page::text` drives PDF2's interpreter with [`TextCollector`] as its TextSink
/// and this function is deleted. Public (hidden) only so the host KATs can drive rendering through it.
#[doc(hidden)]
pub fn walk(doc: &Document, content: &[u8], resources: &Dict, ctm: Matrix, f: &mut dyn FnMut(&Dict, &[u8], &TextParams, &Matrix, &Matrix) -> f64) {
    let gs = Gs { ctm, font: None, p: TextParams::default(), leading: 0.0 };
    walk_in(doc, content, resources, gs, 0, f);
}

fn res_dict(doc: &Document, resources: &Dict, cat: &[u8], name: &[u8]) -> Option<Object> {
    let d = match doc.get(resources.get(cat)?) {
        Object::Dict(d) => d,
        _ => return None,
    };
    Some(doc.get(d.get(name)?))
}

fn walk_in(doc: &Document, content: &[u8], resources: &Dict, mut gs: Gs, depth: u8, f: &mut dyn FnMut(&Dict, &[u8], &TextParams, &Matrix, &Matrix) -> f64) {
    let mut stack: Vec<Gs> = Vec::new();
    let mut tm = IDENTITY;
    let mut tlm = IDENTITY;
    let translate = |x: f64, y: f64| -> Matrix { [1.0, 0.0, 0.0, 1.0, x, y] };
    for op in ops(content) {
        let n: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
        let mut show = |gs: &Gs, tm: &mut Matrix, s: &[u8]| {
            if let Some(font) = &gs.font {
                let adv = f(font, s, &gs.p, &*tm, &gs.ctm);
                *tm = mul(&translate(adv, 0.0), tm);
            }
        };
        match op.operator.as_slice() {
            b"q" => stack.push(gs.clone()),
            b"Q" => {
                if let Some(g) = stack.pop() {
                    gs = g;
                }
            }
            b"cm" if n.len() == 6 => gs.ctm = mul(&[n[0], n[1], n[2], n[3], n[4], n[5]], &gs.ctm),
            b"BT" => {
                tm = IDENTITY;
                tlm = IDENTITY;
            }
            b"Tf" => {
                if let (Some(Object::Name(name)), Some(size)) = (op.operands.first(), op.operands.get(1).and_then(Object::as_f64)) {
                    gs.font = match res_dict(doc, resources, b"Font", name) {
                        Some(Object::Dict(d)) => Some(d),
                        _ => None,
                    };
                    gs.p.size = size;
                }
            }
            b"Tc" if n.len() == 1 => gs.p.char_spacing = n[0],
            b"Tw" if n.len() == 1 => gs.p.word_spacing = n[0],
            b"Tz" if n.len() == 1 => gs.p.h_scale = n[0] / 100.0,
            b"TL" if n.len() == 1 => gs.leading = n[0],
            b"Ts" if n.len() == 1 => gs.p.rise = n[0],
            b"Td" | b"TD" if n.len() == 2 => {
                if op.operator == b"TD" {
                    gs.leading = -n[1];
                }
                tlm = mul(&translate(n[0], n[1]), &tlm);
                tm = tlm;
            }
            b"Tm" if n.len() == 6 => {
                tlm = [n[0], n[1], n[2], n[3], n[4], n[5]];
                tm = tlm;
            }
            b"T*" => {
                tlm = mul(&translate(0.0, -gs.leading), &tlm);
                tm = tlm;
            }
            b"Tj" => {
                if let Some(Object::String(s)) = op.operands.first() {
                    show(&gs, &mut tm, s);
                }
            }
            b"'" | b"\"" => {
                let s = op.operands.last();
                if op.operator == b"\"" && n.len() >= 2 {
                    gs.p.word_spacing = n[0];
                    gs.p.char_spacing = n[1];
                }
                tlm = mul(&translate(0.0, -gs.leading), &tlm);
                tm = tlm;
                if let Some(Object::String(s)) = s {
                    show(&gs, &mut tm, s);
                }
            }
            b"TJ" => {
                if let Some(Object::Array(a)) = op.operands.first() {
                    for e in a {
                        match e {
                            Object::String(s) => show(&gs, &mut tm, s),
                            o => {
                                if let Some(v) = o.as_f64() {
                                    let tx = -v / 1000.0 * gs.p.size * gs.p.h_scale;
                                    tm = mul(&translate(tx, 0.0), &tm);
                                }
                            }
                        }
                    }
                }
            }
            b"Do" if depth < MAX_FORM_DEPTH => {
                let Some(Object::Name(name)) = op.operands.first() else { continue };
                let Some(Object::Stream(s)) = res_dict(doc, resources, b"XObject", name) else { continue };
                if s.dict.name(b"Subtype") != Some(&b"Form"[..]) {
                    continue;
                }
                let Ok(data) = doc.decode(&s) else { continue };
                let m = match doc.get(s.dict.get(b"Matrix").unwrap_or(&Object::Null)) {
                    Object::Array(a) if a.len() == 6 => {
                        let v: Vec<f64> = a.iter().filter_map(|o| doc.get(o).as_f64()).collect();
                        if v.len() == 6 { [v[0], v[1], v[2], v[3], v[4], v[5]] } else { IDENTITY }
                    }
                    _ => IDENTITY,
                };
                let res = match doc.get(s.dict.get(b"Resources").unwrap_or(&Object::Null)) {
                    Object::Dict(d) => d,
                    _ => resources.clone(),
                };
                let mut inner = gs.clone();
                inner.ctm = mul(&m, &gs.ctm);
                walk_in(doc, &data.data, &res, inner, depth + 1, f);
            }
            _ => {}
        }
    }
}
