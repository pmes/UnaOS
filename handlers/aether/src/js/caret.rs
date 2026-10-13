//! AETHERWPT3 (LEDGER SR138): `Selection.modify(alter, direction, granularity)` (Selection API §3 "modify()": the
//! WebKit-origin method the Selection API records — `move` / `extend`, `forward` / `backward` / `left` / `right`,
//! and the granularities `character`, `word`, `sentence`, `line`, `paragraph`, `lineboundary`,
//! `sentenceboundary`, `paragraphboundary`, `documentboundary`).
//!
//! The caret moves over the document's RENDERED text, not its DOM text: the document is read as a run of
//! visible units — characters after CSS Text §4.1.1 white-space processing (a collapsible run is one space;
//! a space at a line's start or end is gone; `pre` / `pre-wrap` / `break-spaces` keep every space and newline,
//! `pre-line` keeps newlines), a `<br>` and a preserved newline as hard breaks, the edge between block-level
//! boxes as one break, `display: none` subtrees and replaced / unrendered elements skipped (the computed
//! `display` and `white-space` are AETHERSTYLE's report, js/cssom.rs `computed_value`). A boundary point maps to
//! the gap between two units (the DOM's boundary-point order, `crate::selection::Order`); a move lands in a gap,
//! which maps back to a boundary point next to the unit it crossed (after the previous unit going forward,
//! before the next going backward, never inside a break). Words are runs of non-whitespace (symbols included,
//! as Chromium moves); `line` and `lineboundary` follow the hard lines (see WPT.md: soft wraps are not read).

use super::dom::{main_doc, node_ref, with_doc};
use crate::selection::{self as model, Key, Point, Selection};
use html_core::{Document, NodeData, NodeId};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ws {
    Collapse,
    PreLine,
    Preserve,
}

#[derive(Clone, Copy)]
enum Span {
    /// Code units [start, end) of a Text node.
    Text(usize, u32, u32),
    /// A `<br>`: its parent and index.
    Br(usize, u32),
    /// The edge between block-level boxes: no DOM span of its own.
    Edge,
}

#[derive(Clone, Copy)]
struct Unit {
    ch: char,
    brk: bool,
    collapsible: bool,
    span: Span,
    /// The key of the boundary point right after this unit.
    after: Key,
}

impl Unit {
    fn edge(&self) -> bool {
        matches!(self.span, Span::Edge)
    }
    fn space(&self) -> bool {
        self.brk || self.ch.is_whitespace()
    }
    fn before_point(&self) -> Option<Point> {
        match self.span {
            Span::Text(n, s, _) => Some(Point { node: n, offset: s }),
            Span::Br(p, i) => Some(Point { node: p, offset: i }),
            Span::Edge => None,
        }
    }
    fn after_point(&self) -> Option<Point> {
        match self.span {
            Span::Text(n, _, e) => Some(Point { node: n, offset: e }),
            Span::Br(p, i) => Some(Point { node: p, offset: i + 1 }),
            Span::Edge => None,
        }
    }
}

/// (display, white-space) of every element of the page's document.
fn styles() -> HashMap<usize, (String, String)> {
    let doc = main_doc();
    let els: Vec<usize> = with_doc(|d| d.descendants(NodeId(doc)).filter(|i| d.element(*i).is_some()).map(|i| i.0).collect());
    els.into_iter()
        .map(|el| (el, (super::cssom::computed_value(el, "display"), super::cssom::computed_value(el, "white-space"))))
        .collect()
}

const SKIP: &[&str] = &[
    "head", "script", "style", "template", "title", "noscript", "textarea", "select", "input", "iframe", "object", "embed",
    "video", "audio", "canvas", "svg", "math", "datalist",
];

struct Walk<'a> {
    d: &'a Document,
    o: &'a model::Order,
    styles: &'a HashMap<usize, (String, String)>,
    units: Vec<Unit>,
    line_start: bool,
    /// A block edge passed since the last unit (the key where it was first passed).
    pending: Option<Key>,
}

impl Walk<'_> {
    fn key(&self, node: usize, offset: u32) -> Key {
        self.o.key(self.d, Point { node, offset }).unwrap_or((u32::MAX, 0))
    }

    fn last_space(&self) -> bool {
        self.units.last().is_some_and(|u| u.collapsible)
    }

    fn trim_trailing(&mut self) {
        if self.last_space() {
            self.units.pop();
        }
    }

    /// A pending block edge becomes a break before the unit about to be pushed.
    fn flush_edge(&mut self) {
        let Some(k) = self.pending.take() else { return };
        if self.units.is_empty() || self.units.last().is_some_and(|u| u.brk) {
            self.line_start = true;
            return;
        }
        self.trim_trailing();
        self.units.push(Unit { ch: '\n', brk: true, collapsible: false, span: Span::Edge, after: k });
        self.line_start = true;
    }

    fn push_break(&mut self, span: Span, after: Key) {
        self.flush_edge();
        self.trim_trailing();
        self.units.push(Unit { ch: '\n', brk: true, collapsible: false, span, after });
        self.line_start = true;
    }

    fn push_char(&mut self, ch: char, span: Span, after: Key, collapsible: bool) {
        if collapsible && (self.pending.is_some() || self.line_start || self.last_space()) {
            return;
        }
        self.flush_edge();
        self.units.push(Unit { ch: if collapsible { ' ' } else { ch }, brk: false, collapsible, span, after });
        self.line_start = false;
    }

    fn text(&mut self, node: usize, t: &str, ws: Ws) {
        let pre = self.o.pre(node).unwrap_or(u32::MAX);
        let mut off = 0u32;
        for c in t.chars() {
            let len = c.len_utf16() as u32;
            let span = Span::Text(node, off, off + len);
            let after = (pre, (off + len) as i64);
            let white = matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c');
            match ws {
                Ws::Preserve if c == '\n' => self.push_break(span, after),
                Ws::Preserve => self.push_char(c, span, after, false),
                Ws::PreLine if c == '\n' => self.push_break(span, after),
                _ if white => self.push_char(' ', span, after, true),
                _ => self.push_char(c, span, after, false),
            }
            off += len;
        }
    }

    fn walk(&mut self, id: usize, ws: Ws) {
        let kids: Vec<NodeId> = self.d.children(NodeId(id)).collect();
        for k in kids {
            match self.d.data(k) {
                NodeData::Text(t) => {
                    let t = t.clone();
                    self.text(k.0, &t, ws);
                }
                NodeData::Element(e) => {
                    if e.is_html("br") {
                        let parent = id;
                        let idx = self.d.children(NodeId(parent)).position(|c| c == k).unwrap_or(0) as u32;
                        let after = self.key(parent, idx + 1);
                        self.push_break(Span::Br(parent, idx), after);
                        continue;
                    }
                    if SKIP.contains(&e.local.as_str()) {
                        continue;
                    }
                    let (display, white) = self.styles.get(&k.0).cloned().unwrap_or_default();
                    if display == "none" {
                        continue;
                    }
                    let block = !display.is_empty() && !display.starts_with("inline") && display != "contents";
                    let mode = match white.as_str() {
                        "pre" | "pre-wrap" | "break-spaces" => Ws::Preserve,
                        "pre-line" => Ws::PreLine,
                        "" => ws,
                        _ => Ws::Collapse,
                    };
                    if block && self.pending.is_none() {
                        self.pending = Some(self.key(k.0, 0));
                    }
                    self.walk(k.0, mode);
                    if block && self.pending.is_none() {
                        let len = self.d.children(k).count() as u32;
                        self.pending = Some(self.key(k.0, len));
                    }
                }
                _ => {}
            }
        }
    }
}

/// The page's visible units, in order.
fn units() -> Vec<Unit> {
    let st = styles();
    let doc = node_ref(main_doc());
    let o = model::order(&doc);
    with_doc(|d| {
        let mut w = Walk { d, o: &o, styles: &st, units: Vec::new(), line_start: true, pending: None };
        w.walk(main_doc(), Ws::Collapse);
        w.trim_trailing();
        w.units
    })
}

/// The gap a boundary point is in: the number of units wholly before it.
fn gap_of(units: &[Unit], p: Point) -> Option<usize> {
    let doc = node_ref(main_doc());
    let o = model::order(&doc);
    let k = with_doc(|d| o.key(d, p))?;
    Some(units.iter().take_while(|u| u.after <= k).count())
}

fn forward_point(u: &[Unit], g: usize) -> Option<Point> {
    if g > 0 && !u[g - 1].brk {
        return u[g - 1].after_point();
    }
    if g < u.len() && !u[g].edge() {
        return u[g].before_point();
    }
    if g > 0 && !u[g - 1].edge() {
        return u[g - 1].after_point();
    }
    None
}

fn backward_point(u: &[Unit], g: usize) -> Option<Point> {
    if g < u.len() && !u[g].brk {
        return u[g].before_point();
    }
    if g > 0 && !u[g - 1].edge() {
        return u[g - 1].after_point();
    }
    if g < u.len() && !u[g].edge() {
        return u[g].before_point();
    }
    None
}

/// A combining mark (it moves with its base character: an extended grapheme cluster, approximated).
fn combining(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F | 0x200D | 0xFE00..=0xFE0F)
}

fn terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '\u{3002}')
}

/// The gap `granularity` reaches from gap `g`.
fn step(u: &[Unit], g: usize, forward: bool, granularity: &str) -> usize {
    let n = u.len();
    let ws = |i: usize| u[i].space();
    let brk = |i: usize| u[i].brk;
    let line_start = |g: usize| (0..g).rev().find(|&i| brk(i)).map_or(0, |i| i + 1);
    let line_end = |g: usize| (g..n).find(|&i| brk(i)).unwrap_or(n);
    let mut i = g;
    match (granularity, forward) {
        ("character", true) => {
            if i < n {
                i += 1;
                while i < n && combining(u[i].ch) {
                    i += 1;
                }
            }
        }
        ("character", false) => {
            while i > 0 {
                i -= 1;
                if !combining(u[i].ch) {
                    break;
                }
            }
        }
        ("word", true) => {
            while i < n && ws(i) {
                i += 1;
            }
            while i < n && !ws(i) {
                i += 1;
            }
        }
        ("word", false) => {
            while i > 0 && ws(i - 1) {
                i -= 1;
            }
            while i > 0 && !ws(i - 1) {
                i -= 1;
            }
        }
        ("sentence" | "sentenceboundary", true) => {
            if granularity == "sentence" {
                while i < n && ws(i) {
                    i += 1;
                }
            }
            while i < n && !brk(i) {
                if terminator(u[i].ch) {
                    while i < n && terminator(u[i].ch) {
                        i += 1;
                    }
                    if i >= n || ws(i) {
                        break;
                    }
                    continue;
                }
                i += 1;
            }
        }
        ("sentence" | "sentenceboundary", false) => {
            if granularity == "sentence" {
                while i > 0 && ws(i - 1) {
                    i -= 1;
                }
            }
            while i > 0 && !brk(i - 1) && !(i >= 2 && terminator(u[i - 2].ch) && ws(i - 1)) {
                i -= 1;
            }
            while i < g && ws(i) && !brk(i) {
                i += 1;
            }
        }
        ("paragraph", true) => {
            if i < n && brk(i) {
                i += 1;
            }
            i = line_end(i);
        }
        ("paragraph", false) => {
            if i > 0 && brk(i - 1) {
                i -= 1;
            }
            i = line_start(i);
        }
        ("paragraphboundary" | "lineboundary", true) => i = line_end(i),
        ("paragraphboundary" | "lineboundary", false) => i = line_start(i),
        ("line", true) => {
            let (s, e) = (line_start(g), line_end(g));
            if e >= n {
                i = n;
            } else {
                let next_end = line_end(e + 1);
                i = (e + 1 + (g - s)).min(next_end);
            }
        }
        ("line", false) => {
            let s = line_start(g);
            if s == 0 {
                i = 0;
            } else {
                let prev_end = s - 1;
                let prev_start = line_start(prev_end);
                i = (prev_start + (g - s)).min(prev_end);
            }
        }
        ("documentboundary", true) => i = n,
        ("documentboundary", false) => i = 0,
        _ => {}
    }
    i
}

const GRANULARITIES: &[&str] =
    &["character", "word", "sentence", "line", "paragraph", "lineboundary", "sentenceboundary", "paragraphboundary", "documentboundary"];

/// `modify(alter, direction, granularity)` on selection `x`: the new selection, or None (an unknown argument, a
/// focus outside the document's rendered text — nothing changes).
pub fn modify(x: Selection, alter: &str, direction: &str, granularity: &str) -> Option<Selection> {
    let alter = alter.to_ascii_lowercase();
    let direction = direction.to_ascii_lowercase();
    let granularity = granularity.to_ascii_lowercase();
    let extend = match alter.as_str() {
        "move" => false,
        "extend" => true,
        _ => return None,
    };
    // `left` / `right` in the document's (left-to-right) inline direction.
    let forward = match direction.as_str() {
        "forward" | "right" => true,
        "backward" | "left" => false,
        _ => return None,
    };
    if !GRANULARITIES.contains(&granularity.as_str()) {
        return None;
    }
    // Moving a range by a character collapses it to the end it moves toward.
    if !extend && !x.is_collapsed() && granularity == "character" {
        let doc = node_ref(main_doc());
        let ((_, s), (_, e)) = model::ordered(&doc, x)?;
        return Some(Selection::collapsed(if forward { e } else { s }));
    }
    let u = units();
    let g = gap_of(&u, x.focus)?;
    let t = step(&u, g, forward, &granularity);
    let p = if forward { forward_point(&u, t) } else { backward_point(&u, t) }?;
    Some(if extend { Selection { anchor: x.anchor, focus: p } } else { Selection::collapsed(p) })
}
