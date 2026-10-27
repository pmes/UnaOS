// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF4 (LEDGER SR324): the selection in a PDF document, parallel to the DOM selection (selection.rs keeps DOM
//! boundary points; a PDF page has no DOM). A position is (page, span, char) over PDF3's `TextSpan`s in content
//! order; a span carries ONE box (the renderer's 72-dpi device space), so a character's x is proportional across its
//! span (per-glyph boxes are owed: PDF4-GLYPHPOS). The copied text is the selected spans' slices joined by PDF3's
//! `spans_to_string` (one line assembler), the pages by a newline.
//!
//! CHARTER: Aether owns the browser and the PDF line (CODEX §2 Web); this is inside the live handler.

use std::rc::Rc;

use pdf_core::font::{TextSpan, spans_to_string};

/// A text position: a page, a span of its `Page::text`, a char boundary inside the span's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub page: usize,
    pub span: usize,
    pub ch: usize,
}

/// The selection: where it started and where it was extended to, and the drag in progress (its granularity —
/// 1 char, 2 word, 3 span — and the range the press selected).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PdfSelection {
    pub anchor: Option<Pos>,
    pub focus: Option<Pos>,
    pub drag: Option<(u8, Pos, Pos)>,
}

impl PdfSelection {
    /// (start, end) in document order.
    pub fn ordered(&self) -> Option<(Pos, Pos)> {
        let (a, f) = (self.anchor?, self.focus?);
        Some(if a <= f { (a, f) } else { (f, a) })
    }
}

/// A find hit: chars `lo..hi` of one span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub page: usize,
    pub span: usize,
    pub lo: usize,
    pub hi: usize,
}

impl Range {
    pub fn bounds(&self) -> (Pos, Pos) {
        (Pos { page: self.page, span: self.span, ch: self.lo }, Pos { page: self.page, span: self.span, ch: self.hi })
    }
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// The x of the boundary before char `ch` of `span` (proportional across its box).
pub fn char_x(span: &TextSpan, ch: usize) -> f64 {
    let n = char_len(&span.text).max(1);
    span.bbox[0] + (span.bbox[2] - span.bbox[0]) * ch.min(n) as f64 / n as f64
}

/// The box of chars `lo..hi` of `span`.
pub fn sub_box(span: &TextSpan, lo: usize, hi: usize) -> [f64; 4] {
    [char_x(span, lo), span.bbox[1], char_x(span, hi), span.bbox[3]]
}

/// The span and char boundary at (`x`, `y`) (72-dpi page space): the span whose box holds the point, else the
/// nearest one on that line (within half an em above or below its box); None off every line.
pub fn hit(spans: &[TextSpan], x: f64, y: f64) -> Option<(usize, usize)> {
    let mut best: Option<(usize, f64)> = None;
    for (i, s) in spans.iter().enumerate() {
        let [x0, y0, x1, y1] = s.bbox;
        let em = s.size.abs().max(1.0);
        if y < y0 - 0.5 * em || y > y1 + 0.5 * em {
            continue;
        }
        let dx = if x < x0 { x0 - x } else if x > x1 { x - x1 } else { 0.0 };
        let dy = if y < y0 { y0 - y } else if y > y1 { y - y1 } else { 0.0 };
        let d = dx + 4.0 * dy;
        if best.is_none_or(|(_, b)| d < b) {
            best = Some((i, d));
        }
    }
    let (i, _) = best?;
    let s = &spans[i];
    let n = char_len(&s.text);
    let w = s.bbox[2] - s.bbox[0];
    let ch = if w > 0.0 { ((x - s.bbox[0]) / w * n as f64).round().clamp(0.0, n as f64) as usize } else { 0 };
    Some((i, ch))
}

/// The word around char boundary `ch` of `text`: the run of non-white-space chars it touches (chars `lo..hi`).
pub fn word(text: &str, ch: usize) -> (usize, usize) {
    let cs: Vec<char> = text.chars().collect();
    let n = cs.len();
    let mut at = ch.min(n);
    if at == n || cs[at].is_whitespace() {
        if at > 0 && !cs[at - 1].is_whitespace() {
            at -= 1;
        } else {
            return (ch.min(n), ch.min(n));
        }
    }
    let (mut lo, mut hi) = (at, at + 1);
    while lo > 0 && !cs[lo - 1].is_whitespace() {
        lo -= 1;
    }
    while hi < n && !cs[hi].is_whitespace() {
        hi += 1;
    }
    (lo, hi)
}

/// The selected part of each span of page `page`: (span index, lo, hi), non-empty only.
fn selected_spans(sel: &PdfSelection, page: usize, spans: &[TextSpan]) -> Vec<(usize, usize, usize)> {
    let Some((a, b)) = sel.ordered() else { return Vec::new() };
    if page < a.page || page > b.page || spans.is_empty() {
        return Vec::new();
    }
    let first = if page == a.page { a.span } else { 0 };
    let last = if page == b.page { b.span.min(spans.len() - 1) } else { spans.len() - 1 };
    let mut out = Vec::new();
    for k in first..=last {
        let Some(s) = spans.get(k) else { continue };
        let n = char_len(&s.text);
        let lo = if (page, k) == (a.page, a.span) { a.ch.min(n) } else { 0 };
        let hi = if (page, k) == (b.page, b.span) { b.ch.min(n) } else { n };
        if lo < hi {
            out.push((k, lo, hi));
        }
    }
    out
}

/// The selection's boxes on page `page`.
pub fn rects(sel: &PdfSelection, page: usize, spans: &[TextSpan]) -> Vec<[f64; 4]> {
    selected_spans(sel, page, spans).into_iter().map(|(k, lo, hi)| sub_box(&spans[k], lo, hi)).collect()
}

/// The selection's text: each page's selected slices joined by `spans_to_string`, the pages by "\n".
pub fn text(sel: &PdfSelection, spans_of: impl Fn(usize) -> Rc<Vec<TextSpan>>) -> String {
    let Some((a, b)) = sel.ordered() else { return String::new() };
    let mut pages = Vec::new();
    for page in a.page..=b.page {
        let spans = spans_of(page);
        let cut: Vec<TextSpan> = selected_spans(sel, page, &spans)
            .into_iter()
            .map(|(k, lo, hi)| {
                let s = &spans[k];
                TextSpan { text: s.text.chars().skip(lo).take(hi - lo).collect(), bbox: sub_box(s, lo, hi), ..s.clone() }
            })
            .collect();
        if !cut.is_empty() {
            pages.push(spans_to_string(&cut));
        }
    }
    pages.join("\n")
}
