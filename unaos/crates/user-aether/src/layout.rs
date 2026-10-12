// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AETHERMETAL (SR66): the CSS2 visual formatting model, the subset M1 draws, written for this crate (the host
// engine lays out with taffy, which is std; porting it is M2's question — docs/dev/evidence/aethermetal/).
//
// BOX TREE (CSS2 §9.2). Each element whose computed `display` is block or list-item generates a block box; inline
// elements generate no box of their own — their text becomes inline content carrying the element's style. A block
// box whose children mix block-level and inline content wraps every maximal run of inline content in an ANONYMOUS
// block box (§9.2.1.1); a run that is only collapsible white space generates nothing. A block inside an inline
// element breaks the inline (its text before and after lands in two anonymous boxes). `display: none` generates
// nothing for the element or its subtree.
//
// BLOCK FORMATTING (§10.3.3, §8.3.1). Widths: `width` or what the containing block leaves after margins, borders
// and padding, capped by `max-width`; `margin: auto` on both sides of a narrower box centres it. Boxes stack
// vertically. ADJACENT VERTICAL MARGINS COLLAPSE: between siblings (the larger positive plus the most negative),
// between a box and its first/last in-flow child when no border or padding separates them (the child's margin
// escapes through the parent), and through an empty box (its own top and bottom margins meet). The root's margins
// never collapse with its children's.
//
// INLINE FORMATTING (§9.4.2, §10.8). The content is broken into words at white space; `white-space: normal`
// collapses each run of spaces/tabs/newlines to one space and drops it at a line's start and end; `pre` keeps
// every space and breaks only at newlines (a line too long overflows, clipped by the window). Words are measured
// with font_core (shaped), a line takes words while they fit, adjacent words of one style merge into one run.
// Each line box is as tall as its tallest inline box: per run, the half-leading model — A = (line-height −
// (ascent + descent)) / 2 + ascent above the baseline, line-height − A below — with the block's own font as the
// strut; `text-align` shifts the runs. A list item's marker (• ◦ ▪ or "n.") is placed outside its content box,
// right-aligned at the first line's baseline.

use alloc::string::String;
use alloc::vec::Vec;
use html_core::{Document, NodeData, NodeId};

use crate::style::{Align, Display, Len, LineH, ListType, Space, Style, Styled};
use crate::txt::Txt;

/// Inline content: text in a style (its own background is painted when `own`), or a forced break.
pub enum Inline {
    Text { s: String, st: usize, own: bool },
    Br,
}

pub enum Kind {
    Block,
    ListItem(String),
    Anon(Vec<Inline>),
}

/// A box of the box tree (block-level only: inline content lives in `Kind::Anon`).
pub struct BoxNode {
    pub st: usize,
    pub kind: Kind,
    pub kids: Vec<BoxNode>,
}

// ---- box generation ----------------------------------------------------------------------------------------

/// The box tree of the document's root element (`None` if the document has no element or it is display:none).
pub fn build(doc: &Document, sty: &Styled) -> Option<BoxNode> {
    let root = doc.document_element()?;
    let st = sty.of[root.0];
    if sty.styles[st].display == Display::None {
        return None;
    }
    Some(block_of(doc, sty, root, None))
}

fn block_of(doc: &Document, sty: &Styled, n: NodeId, ordinal: Option<i64>) -> BoxNode {
    let st = sty.of[n.0];
    let s = &sty.styles[st];
    let kind = match (s.display, ordinal) {
        (Display::ListItem, Some(k)) => Kind::ListItem(marker(s.list, k)),
        (Display::ListItem, None) => Kind::ListItem(marker(s.list, 1)),
        _ => Kind::Block,
    };
    let mut b = BoxNode { st, kind, kids: Vec::new() };
    let mut pend: Vec<Inline> = Vec::new();
    // list numbering: `<ol start>` and `reversed` are not M1's; items count from `start` or 1
    let mut li = doc.element(n).and_then(|e| e.attr("start")).and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(1);
    walk_children(doc, sty, n, st, false, &mut b.kids, &mut pend, &mut li);
    flush(&mut b.kids, &mut pend, st, sty);
    b
}

fn walk_children(doc: &Document, sty: &Styled, n: NodeId, ist: usize, own: bool, out: &mut Vec<BoxNode>, pend: &mut Vec<Inline>, li: &mut i64) {
    for c in doc.children(n) {
        match doc.data(c) {
            NodeData::Text(t) => pend.push(Inline::Text { s: t.clone(), st: ist, own }),
            NodeData::Element(e) => {
                let cs = sty.of[c.0];
                let s = &sty.styles[cs];
                match s.display {
                    Display::None => {}
                    Display::Block | Display::ListItem => {
                        flush(out, pend, ist, sty);
                        let ord = if s.display == Display::ListItem {
                            let k = *li;
                            *li += 1;
                            Some(k)
                        } else {
                            None
                        };
                        out.push(block_of(doc, sty, c, ord));
                    }
                    Display::Inline => {
                        if e.is_html("br") {
                            pend.push(Inline::Br);
                        } else if e.is_html("img") {
                            // M1 draws no images: the alt text stands in, in brackets
                            if let Some(alt) = e.attr("alt").filter(|a| !a.is_empty()) {
                                let mut s = String::from("[");
                                s.push_str(alt);
                                s.push(']');
                                pend.push(Inline::Text { s, st: cs, own: true });
                            }
                        } else {
                            walk_children(doc, sty, c, cs, true, out, pend, li);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Close a run of inline content into an anonymous block (nothing for collapsible white space only).
fn flush(out: &mut Vec<BoxNode>, pend: &mut Vec<Inline>, parent_st: usize, sty: &Styled) {
    if pend.is_empty() {
        return;
    }
    let keep = pend.iter().any(|i| match i {
        Inline::Br => true,
        Inline::Text { s, st, .. } => sty.styles[*st].space == Space::Pre && !s.is_empty() || s.chars().any(|c| !is_ws(c)),
    });
    let items = core::mem::take(pend);
    if keep {
        out.push(BoxNode { st: parent_st, kind: Kind::Anon(items), kids: Vec::new() });
    }
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C')
}

fn marker(t: ListType, k: i64) -> String {
    match t {
        ListType::None => String::new(),
        ListType::Disc => String::from("\u{2022}"),
        ListType::Circle => String::from("\u{25E6}"),
        ListType::Square => String::from("\u{25AA}"),
        ListType::Decimal => {
            let mut s = alloc::format!("{}", k);
            s.push('.');
            s
        }
    }
}

// ---- the laid-out tree ---------------------------------------------------------------------------------------

/// A run of text on a line: `x` from the content box's left, `w` its advance.
pub struct Frag {
    pub x: f32,
    pub w: f32,
    pub s: String,
    pub st: usize,
    pub own: bool,
}

/// A line box: `y` from the box's border-box top, `h` tall, its baseline `base` below `y`.
pub struct LineBox {
    pub y: f32,
    pub h: f32,
    pub base: f32,
    pub frags: Vec<Frag>,
}

/// One block box placed: `x`/`y` its border box relative to the parent's border box, `w` x `h` its size.
pub struct Laid {
    pub st: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// The content box's left edge from the border box's (border + padding).
    pub cx: f32,
    pub kids: Vec<Laid>,
    pub lines: Vec<LineBox>,
    pub marker: Option<String>,
    /// The first line's baseline from this box's border-box top (a list marker sits on it).
    pub first_base: Option<f32>,
    /// The margins that face the outside after collapsing with escaped child margins.
    pub mt: M,
    pub mb: M,
    /// An empty box its top and bottom margins collapse through.
    pub through: bool,
    pub anon: bool,
}

/// A collapsing margin set: the largest positive and the most negative margin met.
#[derive(Clone, Copy, Default)]
pub struct M {
    pos: f32,
    neg: f32,
}

impl M {
    fn of(v: f32) -> M {
        let mut m = M::default();
        m.add(v);
        m
    }
    fn add(&mut self, v: f32) {
        if v >= 0.0 {
            if v > self.pos {
                self.pos = v;
            }
        } else if v < self.neg {
            self.neg = v;
        }
    }
    fn join(&mut self, o: M) {
        self.add(o.pos);
        self.add(o.neg);
    }
    pub fn v(self) -> f32 {
        self.pos + self.neg
    }
}

/// Totals the start line and the witness report.
#[derive(Default)]
pub struct Stats {
    pub lines: usize,
    pub blocks: usize,
}

pub struct Cx<'a> {
    pub sty: &'a Styled,
    pub txt: &'a mut Txt,
    pub stats: Stats,
}

/// Lay out the root box in a viewport `vw` px wide; the page height is the root's margin box.
pub fn layout_root(b: &BoxNode, vw: f32, cx: &mut Cx) -> (Laid, f32) {
    let mut l = layout_block(b, vw, true, cx);
    // the root's margins never collapse with the canvas: they push its border box in from the page top
    l.y = l.mt.v();
    let doc_h = l.y + l.h + l.mb.v();
    (l, doc_h)
}

fn layout_block(b: &BoxNode, avail: f32, root: bool, cx: &mut Cx) -> Laid {
    let s: Style = cx.sty.styles[b.st].clone();
    cx.stats.blocks += 1;
    if let Kind::Anon(items) = &b.kind {
        let lines = layout_inline(items, avail, &s, cx);
        let h = lines.last().map_or(0.0, |l| l.y + l.h);
        let first_base = lines.first().map(|l| l.y + l.base);
        return Laid {
            st: b.st,
            x: 0.0,
            y: 0.0,
            w: avail,
            h,
            cx: 0.0,
            kids: Vec::new(),
            lines,
            marker: None,
            first_base,
            mt: M::default(),
            mb: M::default(),
            through: h == 0.0,
            anon: true,
        };
    }
    let pad = [s.padding[0].px(avail), s.padding[1].px(avail), s.padding[2].px(avail), s.padding[3].px(avail)];
    let bw = [s.bw(0), s.bw(1), s.bw(2), s.bw(3)];
    let mut ml = s.margin[3].px(avail);
    let mut mr = s.margin[1].px(avail);
    let frame = bw[1] + bw[3] + pad[1] + pad[3];
    let mut cw = match s.width {
        Len::Auto => avail - ml - mr - frame,
        w => w.px(avail),
    };
    match s.max_width {
        Len::Auto => {}
        m => {
            let m = m.px(avail);
            if cw > m {
                cw = m;
            }
        }
    }
    if cw < 0.0 {
        cw = 0.0;
    }
    let narrowed = s.width != Len::Auto || s.max_width != Len::Auto;
    if narrowed {
        let free = avail - cw - frame;
        match (s.margin[3] == Len::Auto, s.margin[1] == Len::Auto) {
            (true, true) => {
                ml = (free / 2.0).max(0.0);
                mr = ml;
            }
            (true, false) => ml = (free - mr).max(0.0),
            _ => {}
        }
    }
    let _ = mr;
    let content_x = bw[3] + pad[3];
    let content_y = bw[0] + pad[0];
    let collapse_top = !root && bw[0] == 0.0 && pad[0] == 0.0;
    let collapse_bot = !root && bw[2] == 0.0 && pad[2] == 0.0 && s.width == Len::Auto;

    let mut kids: Vec<Laid> = Vec::new();
    let mut mt = M::of(s.margin[0].px(avail));
    let mut pending = M::default();
    let mut cursor = 0.0f32; // content-relative: the bottom border edge of the last placed child
    let mut first = true;
    let mut first_base: Option<f32> = None;
    for k in &b.kids {
        let mut c = layout_block(k, cw, false, cx);
        if c.through {
            // an empty box: its margins meet each other and whatever is adjacent
            if first && collapse_top {
                mt.join(c.mt);
                mt.join(c.mb);
            } else {
                pending.join(c.mt);
                pending.join(c.mb);
            }
            continue;
        }
        let y = if first && collapse_top {
            mt.join(c.mt);
            0.0
        } else {
            pending.join(c.mt);
            cursor + pending.v()
        };
        // the child's x is its used margin-left (0 for an anonymous box): place it in the content box
        c.x += content_x;
        c.y = content_y + y;
        if first_base.is_none() {
            first_base = c.first_base.map(|fb| c.y + fb);
        }
        cursor = y + c.h;
        pending = c.mb;
        first = false;
        kids.push(c);
    }
    let mut mb = M::of(s.margin[2].px(avail));
    let content_h = if first {
        // no in-flow content: pending holds the empty children's margins
        if collapse_top && collapse_bot {
            mt.join(pending);
            0.0
        } else {
            pending.v().max(0.0)
        }
    } else if collapse_bot {
        mb.join(pending);
        cursor
    } else {
        cursor + pending.v()
    };
    let h = content_y + content_h + bw[2] + pad[2];
    let through = first && collapse_top && collapse_bot && h == 0.0;
    let marker = match &b.kind {
        Kind::ListItem(m) if !m.is_empty() => Some(m.clone()),
        _ => None,
    };
    Laid {
        st: b.st,
        x: ml,
        y: 0.0,
        w: cw + frame,
        h,
        cx: content_x,
        kids,
        lines: Vec::new(),
        marker,
        first_base: first_base.or(if marker_alone(&b.kind) { Some(content_y + cx.txt.metrics(&s).0) } else { None }),
        mt,
        mb,
        through: through && !matches!(b.kind, Kind::ListItem(_)),
        anon: false,
    }
}

fn marker_alone(k: &Kind) -> bool {
    matches!(k, Kind::ListItem(m) if !m.is_empty())
}

/// The used line height of `st`, px.
fn line_h(st: &Style, txt: &Txt) -> f32 {
    match st.line_h {
        LineH::Normal => txt.normal_lh(st),
        LineH::Mul(m) => st.font_size * m,
        LineH::Px(p) => p,
    }
}

enum Tok {
    Word(String, usize, bool),
    Space(usize),
    Br,
}

/// Break inline content into words, spaces and breaks, collapsing white space where the style says so.
fn tokens(items: &[Inline], sty: &Styled) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    for it in items {
        match it {
            Inline::Br => out.push(Tok::Br),
            Inline::Text { s, st, own } => {
                if sty.styles[*st].space == Space::Pre {
                    let mut first = true;
                    for seg in s.split('\n') {
                        if !first {
                            out.push(Tok::Br);
                        }
                        first = false;
                        let seg = seg.trim_end_matches('\r');
                        if !seg.is_empty() {
                            let mut w = String::new();
                            for ch in seg.chars() {
                                if ch == '\t' {
                                    let n = 4 - (w.chars().count() % 4);
                                    for _ in 0..n {
                                        w.push(' ');
                                    }
                                } else {
                                    w.push(ch);
                                }
                            }
                            out.push(Tok::Word(w, *st, *own));
                        }
                    }
                } else {
                    let mut w = String::new();
                    for ch in s.chars() {
                        if is_ws(ch) {
                            if !w.is_empty() {
                                out.push(Tok::Word(core::mem::take(&mut w), *st, *own));
                            }
                            if !matches!(out.last(), Some(Tok::Space(_))) {
                                out.push(Tok::Space(*st));
                            }
                        } else {
                            w.push(ch);
                        }
                    }
                    if !w.is_empty() {
                        out.push(Tok::Word(w, *st, *own));
                    }
                }
            }
        }
    }
    out
}

fn layout_inline(items: &[Inline], width: f32, block: &Style, cx: &mut Cx) -> Vec<LineBox> {
    let toks = tokens(items, cx.sty);
    let mut lines: Vec<LineBox> = Vec::new();
    let mut frags: Vec<Frag> = Vec::new();
    let mut x = 0.0f32;
    let mut space: Option<usize> = None;
    let mut y = 0.0f32;
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Space(st) => {
                if !frags.is_empty() {
                    space = Some(*st);
                }
            }
            Tok::Br => {
                y = finish(&mut lines, &mut frags, y, width, x, block, cx);
                x = 0.0;
                space = None;
            }
            Tok::Word(w, st, own) => {
                let s = &cx.sty.styles[*st];
                let ww = cx.txt.measure(w, s);
                let sp = match space {
                    Some(sst) if !frags.is_empty() => cx.txt.measure(" ", &cx.sty.styles[sst]),
                    _ => 0.0,
                };
                let pre = s.space == Space::Pre;
                let (x0, sp) = if !frags.is_empty() && !pre && x + sp + ww > width + 0.01 {
                    y = finish(&mut lines, &mut frags, y, width, x, block, cx);
                    (0.0, 0.0)
                } else {
                    (x, sp)
                };
                let merge = matches!(frags.last(), Some(f) if f.st == *st && f.own == *own);
                if merge {
                    let f = frags.last_mut().unwrap();
                    if sp > 0.0 {
                        f.s.push(' ');
                    }
                    f.s.push_str(w);
                    f.w += sp + ww;
                } else {
                    frags.push(Frag { x: x0 + sp, w: ww, s: w.clone(), st: *st, own: *own });
                }
                x = x0 + sp + ww;
                space = None;
            }
        }
        i += 1;
    }
    if !frags.is_empty() {
        finish(&mut lines, &mut frags, y, width, x, block, cx);
    }
    lines
}

/// Close the current line: its height and baseline from the half-leading model, its runs aligned.
fn finish(lines: &mut Vec<LineBox>, frags: &mut Vec<Frag>, y: f32, width: f32, x_end: f32, block: &Style, cx: &mut Cx) -> f32 {
    let strut = |st: &Style, txt: &Txt| -> (f32, f32) {
        let (a, d) = txt.metrics(st);
        let lh = line_h(st, txt);
        let above = (lh - (a + d)) / 2.0 + a;
        (above, lh - above)
    };
    let (mut above, mut below) = strut(block, cx.txt);
    for f in frags.iter() {
        let (a, b) = strut(&cx.sty.styles[f.st], cx.txt);
        if a > above {
            above = a;
        }
        if b > below {
            below = b;
        }
    }
    let shift = match block.align {
        Align::Left => 0.0,
        Align::Center => ((width - x_end) / 2.0).max(0.0),
        Align::Right => (width - x_end).max(0.0),
    };
    let mut fs = core::mem::take(frags);
    for f in fs.iter_mut() {
        f.x += shift;
    }
    let h = font_core::fmath::ceil(above + below);
    lines.push(LineBox { y, h, base: font_core::fmath::round(above), frags: fs });
    cx.stats.lines += 1;
    y + h
}
