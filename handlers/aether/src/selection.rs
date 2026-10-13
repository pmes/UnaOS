//! AETHERSELECT (LEDGER SR69): the page's selection — one (anchor, focus) pair of DOM boundary points
//! (a node + an offset: a UTF-16 code-unit offset into a Text node, a child index into an element, per
//! the DOM Standard §5 "boundary point"), shared by the user's pointer (lib.rs MouseDown and the drag
//! the engine extends while its button is held), the painter (render: inverse video behind the glyphs of
//! AETHERINLINE's line fragments) and script (`js/selection.rs`: `getSelection()`, `Selection`, `Range`).
//!
//! CHARTER: Aether owns the browser (CODEX §2); this is inside the live handler.
//!
//! - Positions from a pointer: a paint-order hit into the text fragments the last frame painted
//!   (`render::text_boxes`), the nearest grapheme boundary (UAX #29 clusters, `font_core::grapheme`) by
//!   shaped caret x, mapped back to the DOM offset by aligning the fragment's collapsed text with the Text
//!   node's data ([`align`]).
//! - Words (double click): UAX #29 word boundaries over grapheme clusters ([`word_at`]; font_core has no
//!   word segmenter — the WB rules for letters, digits, mid-letter/mid-number punctuation, ExtendNumLet,
//!   whitespace runs (WB3d) and one ideograph per word are applied here).
//! - Text (copy, `Selection.toString()`): the HTML "rendered text collection steps" (`innerText`) over the
//!   laid-out tree, clipped to the selected range ([`range_text`]).

use crate::dom::NodeRef;
use crate::layout::LayoutTree;
use html_core::{Document, NodeData, NodeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// A DOM boundary point: `node` is an arena id, `offset` a UTF-16 offset (character data) or child index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Point {
    pub node: usize,
    pub offset: u32,
}

/// The selection: where it started (anchor) and where it was extended to (focus).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Point,
    pub focus: Point,
}

impl Selection {
    pub fn collapsed(p: Point) -> Selection {
        Selection { anchor: p, focus: p }
    }
    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.focus
    }
}

thread_local! {
    static SEL: Cell<Option<Selection>> = const { Cell::new(None) };
    /// Bumped on every change: the engine repaints when it moves.
    static GEN: Cell<u64> = const { Cell::new(0) };
    static ORDER: RefCell<Option<((u64, usize, usize), Rc<Order>)>> = const { RefCell::new(None) };
    /// Find-in-page marks (AETHERDEBT SR93): every match of the find bar's query, painted behind the
    /// text in [`FIND_MARK`] (the active one is also the selection).
    static MARKS: RefCell<Vec<(Point, Point)>> = const { RefCell::new(Vec::new()) };
}

/// Chromium's find-in-page match highlight (inactive matches): yellow.
pub const FIND_MARK: (u8, u8, u8) = (255, 255, 0);

/// Most find marks painted (a page full of one-letter matches stays cheap to paint).
const MARKS_CAP: usize = 1000;

/// Sets the find marks (empty clears them); bumps the generation so the engine repaints.
pub fn set_find_marks(marks: Vec<(Point, Point)>) {
    let mut marks = marks;
    marks.truncate(MARKS_CAP);
    let changed = MARKS.with(|m| {
        let mut m = m.borrow_mut();
        if *m == marks {
            false
        } else {
            *m = marks;
            true
        }
    });
    if changed {
        GEN.with(|g| g.set(g.get().wrapping_add(1)));
    }
}

/// The current selection, if any.
pub fn get() -> Option<Selection> {
    SEL.with(Cell::get)
}

/// Changes the selection; a change bumps the generation and queues `selectionchange` on the document
/// (Selection API §3: a task, coalesced). Returns whether it changed.
pub fn set(sel: Option<Selection>) -> bool {
    if get() == sel {
        return false;
    }
    SEL.with(|s| s.set(sel));
    GEN.with(|g| g.set(g.get().wrapping_add(1)));
    crate::js::dom::selection::queue_selectionchange();
    true
}

/// Clears the selection without an event (a new document).
pub fn reset() {
    SEL.with(|s| s.set(None));
    MARKS.with(|m| m.borrow_mut().clear());
    GEN.with(|g| g.set(g.get().wrapping_add(1)));
}

/// The change counter.
pub fn generation() -> u64 {
    GEN.with(Cell::get)
}

/// The colour Chromium paints a selection with on Linux, measured from its own pixels
/// (docs/dev/evidence/aetherselect/chromium-selection-colour.txt): over white (51,103,209), over black
/// (0,52,158) — source-over of rgb(0,65,198) at alpha 204/255. Selected glyphs and their decorations
/// paint white.
pub const HIGHLIGHT: (u8, u8, u8) = (0, 65, 198);
pub const HIGHLIGHT_ALPHA: u8 = 204;
pub const HIGHLIGHT_TEXT: (u8, u8, u8) = (255, 255, 255);

// =================================================================================================
// Tree order
// =================================================================================================

/// Pre-order indices of a document's nodes: every boundary point maps to a key `(index, sub)` whose
/// order is the DOM's boundary-point order (§5.2) — `(text, k)` → `(pre(text), k)`, `(element, i)` →
/// just before its i-th child, `(element, len)` → just after its subtree.
pub struct Order {
    pre: HashMap<usize, u32>,
    end: HashMap<usize, u32>,
}

pub type Key = (u32, i64);

impl Order {
    pub fn build(d: &Document) -> Order {
        let mut pre = HashMap::with_capacity(d.nodes.len());
        let mut end = HashMap::with_capacity(d.nodes.len());
        let mut n = 0u32;
        // (node, children visited)
        let mut stack: Vec<(NodeId, bool)> = vec![(Document::ROOT, false)];
        while let Some((id, done)) = stack.pop() {
            if done {
                end.insert(id.0, n);
                continue;
            }
            pre.insert(id.0, n);
            n += 1;
            stack.push((id, true));
            let kids: Vec<NodeId> = d.children(id).collect();
            for k in kids.into_iter().rev() {
                stack.push((k, false));
            }
        }
        Order { pre, end }
    }

    pub fn pre(&self, id: usize) -> Option<u32> {
        self.pre.get(&id).copied()
    }

    pub fn end(&self, id: usize) -> Option<u32> {
        self.end.get(&id).copied()
    }

    pub fn key(&self, d: &Document, p: Point) -> Option<Key> {
        if p.node >= d.nodes.len() {
            return None;
        }
        match d.data(NodeId(p.node)) {
            NodeData::Text(_) | NodeData::Comment(_) | NodeData::ProcessingInstruction { .. } => {
                Some((self.pre(p.node)?, p.offset as i64))
            }
            _ => match d.children(NodeId(p.node)).nth(p.offset as usize) {
                Some(c) => Some((self.pre(c.0)?, -1)),
                None => Some((self.end(p.node)?, -1)),
            },
        }
    }
}

/// The document order of `doc`'s arena, cached until the DOM changes.
pub fn order(doc: &NodeRef) -> Rc<Order> {
    let version = crate::js::page(|p| p.version);
    let ptr = Rc::as_ptr(doc.arena()) as usize;
    let len = doc.with_doc(|d, _| d.nodes.len());
    let k = (version, ptr, len);
    if let Some(o) = ORDER.with(|c| c.borrow().as_ref().filter(|(ck, _)| *ck == k).map(|(_, o)| o.clone())) {
        return o;
    }
    let o = Rc::new(doc.with_doc(|d, _| Order::build(d)));
    ORDER.with(|c| *c.borrow_mut() = Some((k, o.clone())));
    o
}

/// The DOM length of a node (§4.2): code units of character data, children of anything else.
pub fn node_length(d: &Document, id: usize) -> u32 {
    match d.data(NodeId(id)) {
        NodeData::Doctype { .. } => 0,
        NodeData::Text(t) | NodeData::Comment(t) => t.encode_utf16().count() as u32,
        NodeData::ProcessingInstruction { data, .. } => data.encode_utf16().count() as u32,
        _ => d.children(NodeId(id)).count() as u32,
    }
}

/// (start, end) of the selection in document order, as keys and points.
pub fn ordered(doc: &NodeRef, sel: Selection) -> Option<((Key, Point), (Key, Point))> {
    let o = order(doc);
    doc.with_doc(|d, _| {
        let a = o.key(d, sel.anchor)?;
        let f = o.key(d, sel.focus)?;
        Some(if a <= f { ((a, sel.anchor), (f, sel.focus)) } else { ((f, sel.focus), (a, sel.anchor)) })
    })
}

/// The part of Text node `id` (pre-order `pre`, `len` code units) inside [ks, ke): node-local (s, e) where
/// s = -1 when the range starts before the node and e = len + 1 when it ends after it.
fn local_range(pre: u32, len: u32, ks: Key, ke: Key) -> Option<(i64, i64)> {
    let s = if ks.0 < pre { -1 } else if ks.0 == pre { ks.1.max(0) } else { return None };
    let e = if ke.0 > pre { len as i64 + 1 } else if ke.0 == pre { ke.1 } else { return None };
    (e > s).then_some((s, e))
}

// =================================================================================================
// UTF-16 / byte offsets
// =================================================================================================

pub fn utf16_to_byte(s: &str, u: u32) -> usize {
    let mut n = 0u32;
    for (b, c) in s.char_indices() {
        if n >= u {
            return b;
        }
        n += c.len_utf16() as u32;
    }
    s.len()
}

pub fn byte_to_utf16(s: &str, b: usize) -> u32 {
    s[..b.min(s.len())].encode_utf16().count() as u32
}

// =================================================================================================
// Fragment text ↔ DOM offsets
// =================================================================================================

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

/// Aligns the texts a Text node's line fragments draw (whitespace collapsed, line-end spaces trimmed,
/// tabs expanded — `fonts::lines`) with the node's `data`: for each fragment, the UTF-16 offset in
/// `data` of each of its char boundaries (`chars + 1` entries). A space the layout carried in from a
/// neighbouring node (`lead`) has zero width here; a character text-transform changed maps one to one.
pub fn align(data: &str, frags: &[&str], collapse: bool) -> Vec<Vec<u32>> {
    let src: Vec<(u32, char)> = {
        let mut v = Vec::with_capacity(data.len());
        let mut u = 0u32;
        for c in data.chars() {
            v.push((u, c));
            u += c.len_utf16() as u32;
        }
        v
    };
    let total = data.encode_utf16().count() as u32;
    let off = |i: usize| if i < src.len() { src[i].0 } else { total };
    let mut i = 0usize;
    let mut out = Vec::with_capacity(frags.len());
    for f in frags {
        let mut v = Vec::with_capacity(f.len() + 1);
        let mut in_tab = false;
        for c in f.chars() {
            if collapse {
                if c == ' ' {
                    v.push(off(i));
                    while i < src.len() && is_ws(src[i].1) {
                        i += 1;
                    }
                    continue;
                }
                while i < src.len() && is_ws(src[i].1) {
                    i += 1;
                }
            } else {
                while i < src.len() && matches!(src[i].1, '\n' | '\r') && c != src[i].1 {
                    i += 1;
                }
                if c == ' ' && in_tab && (i >= src.len() || src[i].1 != ' ') {
                    v.push(off(i));
                    continue;
                }
                in_tab = false;
                if c == ' ' && i < src.len() && src[i].1 == '\t' {
                    v.push(off(i));
                    i += 1;
                    in_tab = true;
                    continue;
                }
            }
            v.push(off(i));
            if i < src.len() {
                i += 1;
            }
        }
        v.push(off(i));
        out.push(v);
    }
    out
}

/// The x of the caret before byte `b` of `text` laid out by `adv` (the pen before the first glyph of the
/// cluster at or after `b`; the run's advance at the end).
pub fn caret_x(adv: &crate::fonts::lines::Advancer, text: &str, b: usize) -> f32 {
    if b == 0 {
        return 0.0;
    }
    if b >= text.len() {
        return adv.str(text);
    }
    let placed = adv.place(text);
    let mut best: Option<(usize, f32)> = None;
    for (g, x, _) in &placed {
        if g.cluster >= b && best.is_none_or(|(c, _)| g.cluster < c) {
            best = Some((g.cluster, x - g.dx * adv.size));
        }
    }
    best.map(|(_, x)| x).unwrap_or_else(|| adv.str(text))
}

/// The grapheme boundary of `text` nearest to `x` (byte offset), and the byte offset of the grapheme
/// under `x`.
pub fn hit_text(adv: &crate::fonts::lines::Advancer, text: &str, x: f32) -> (usize, usize) {
    let bounds: Vec<usize> =
        font_core::grapheme::clusters(text).into_iter().map(|(a, _)| a).chain(std::iter::once(text.len())).collect();
    let xs: Vec<f32> = bounds.iter().map(|&b| caret_x(adv, text, b)).collect();
    let mut near = (f32::INFINITY, 0usize);
    for (k, &b) in bounds.iter().enumerate() {
        let d = (xs[k] - x).abs();
        if d < near.0 {
            near = (d, b);
        }
    }
    let mut under = bounds.first().copied().unwrap_or(0);
    for k in 0..bounds.len().saturating_sub(1) {
        if x >= xs[k] {
            under = bounds[k];
        }
    }
    (near.1, under)
}

// =================================================================================================
// Words (UAX #29, simplified to the rules a click needs)
// =================================================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wb {
    Letter,
    Numeric,
    MidLetter,
    MidNum,
    MidNumLet,
    ExtendNumLet,
    Space,
    Ideo,
    Other,
}

fn wb(c: char) -> Wb {
    let u = c as u32;
    if matches!(u, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF) {
        return Wb::Ideo;
    }
    match c {
        ' ' | '\t' | '\u{a0}' | '\u{3000}' | '\u{2000}'..='\u{200a}' => Wb::Space,
        '\n' | '\r' | '\x0c' => Wb::Other,
        ':' | '\u{b7}' | '\u{387}' | '\u{5f4}' | '\u{2027}' | '\u{fe13}' | '\u{fe55}' | '\u{ff1a}' => Wb::MidLetter,
        ',' | ';' | '\u{37e}' | '\u{589}' | '\u{60c}' | '\u{60d}' | '\u{66c}' | '\u{7f8}' | '\u{2044}' | '\u{fe10}'
        | '\u{fe14}' | '\u{fe50}' | '\u{fe54}' | '\u{ff0c}' | '\u{ff1b}' => Wb::MidNum,
        '.' | '\'' | '\u{2018}' | '\u{2019}' | '\u{2024}' | '\u{fe52}' | '\u{ff07}' | '\u{ff0e}' => Wb::MidNumLet,
        '_' | '\u{203f}' | '\u{2040}' | '\u{2054}' | '\u{fe33}' | '\u{fe34}' | '\u{fe4d}'..='\u{fe4f}' | '\u{ff3f}' => {
            Wb::ExtendNumLet
        }
        _ if c.is_numeric() => Wb::Numeric,
        _ if c.is_alphabetic() => Wb::Letter,
        _ => Wb::Other,
    }
}

/// UAX #29 word segments of `text` as byte ranges (over grapheme clusters, so marks stay with their base).
pub fn words(text: &str) -> Vec<(usize, usize)> {
    let gs: Vec<(usize, usize, Wb)> = font_core::grapheme::clusters(text)
        .into_iter()
        .map(|(a, b)| (a, b, text[a..b].chars().next().map(wb).unwrap_or(Wb::Other)))
        .collect();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    let ah = |w: Wb| w == Wb::Letter;
    while i < gs.len() {
        let start = gs[i].0;
        let c = gs[i].2;
        let mut j = i + 1;
        match c {
            Wb::Space => {
                while j < gs.len() && gs[j].2 == Wb::Space {
                    j += 1;
                }
            }
            Wb::Letter | Wb::Numeric | Wb::ExtendNumLet => loop {
                if j >= gs.len() {
                    break;
                }
                let prev = gs[j - 1].2;
                let cur = gs[j].2;
                let next = gs.get(j + 1).map(|g| g.2);
                let joins = match (prev, cur) {
                    // WB5, WB8, WB9, WB10, WB13a, WB13b
                    (Wb::Letter | Wb::Numeric | Wb::ExtendNumLet, Wb::Letter | Wb::Numeric | Wb::ExtendNumLet) => true,
                    // WB6/WB7: letter (MidLetter | MidNumLet) letter
                    (p, Wb::MidLetter | Wb::MidNumLet) if ah(p) && next.is_some_and(ah) => {
                        j += 1;
                        true
                    }
                    // WB11/WB12: digit (MidNum | MidNumLet) digit
                    (Wb::Numeric, Wb::MidNum | Wb::MidNumLet) if next == Some(Wb::Numeric) => {
                        j += 1;
                        true
                    }
                    _ => false,
                };
                if !joins {
                    break;
                }
                j += 1;
            },
            _ => {}
        }
        let end = if j <= gs.len() { gs[j - 1].1 } else { text.len() };
        out.push((start, end));
        i = j;
    }
    out
}

/// The word segment holding the grapheme at byte `b` (the last one when `b` is the end).
pub fn word_at(text: &str, b: usize) -> (usize, usize) {
    let ws = words(text);
    for &(s, e) in &ws {
        if b >= s && b < e {
            return (s, e);
        }
    }
    ws.last().copied().unwrap_or((b, b))
}

// =================================================================================================
// Painting: which glyphs of an inline formatting context are selected
// =================================================================================================

/// One text fragment's selected part: bytes [start, end) of its text, and whether the line break after
/// it is selected too (Chromium paints a space-wide highlight there when the selection runs past the
/// line's end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FragSel {
    pub start: usize,
    pub end: usize,
    pub newline: bool,
}

/// The line box (top, height) of the IFC `il` a fragment with baseline `baseline` sits on.
pub fn line_of(il: &crate::layout::inline::InlineLayout, baseline: f32) -> Option<(usize, f32, f32)> {
    let mut best: Option<(usize, f32, f32, f32)> = None;
    for (i, &(top, h, _)) in il.lines.iter().enumerate() {
        let d = if baseline < top {
            top - baseline
        } else if baseline > top + h {
            baseline - top - h
        } else {
            0.0
        };
        if best.is_none_or(|b| d < b.3) {
            best = Some((i, top, h, d));
        }
    }
    best.map(|(i, t, h, _)| (i, t, h))
}

/// Whether text in `tid`'s white-space mode collapses (normal / nowrap / pre-line).
fn collapses_at(layout: &LayoutTree, tid: taffy::NodeId) -> bool {
    let mut ws = None;
    let mut n = layout.taffy.parent(tid);
    let mut chain = Vec::new();
    while let Some(p) = n {
        chain.push(p);
        n = layout.taffy.parent(p);
    }
    let mut cur = 0u8;
    for p in chain.into_iter().rev() {
        if let Some(dn) = layout.node_map.get(&p) {
            if let Some(el) = dn.as_element() {
                cur = layout
                    .paint_map
                    .get(&p)
                    .and_then(|s| s.white_space)
                    .unwrap_or_else(|| crate::layout::default_white_space(el.name.local.as_ref(), cur));
                ws = Some(cur);
            }
        }
    }
    matches!(ws.unwrap_or(0), 0 | 1 | 4)
}

/// For every text fragment of the IFC rooted at `root`, the selected part (indexed like `il.frags`).
/// Empty when nothing is selected.
pub fn plan_ifc(layout: &LayoutTree, root: taffy::NodeId) -> HashMap<usize, FragSel> {
    let Some(sel) = get() else { return HashMap::new() };
    if sel.is_collapsed() {
        return HashMap::new();
    }
    plan_range(layout, root, sel)
}

/// For every text fragment of the IFC rooted at `root`, the byte ranges the find marks cover
/// (AETHERDEBT SR93). Empty when there are no marks.
pub fn plan_marks(layout: &LayoutTree, root: taffy::NodeId) -> HashMap<usize, Vec<(usize, usize)>> {
    let marks = MARKS.with(|m| m.borrow().clone());
    let mut out: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    if marks.is_empty() || !layout.inline.contains_key(&root) {
        return out;
    }
    for (a, b) in marks {
        for (fi, fs) in plan_range(layout, root, Selection { anchor: a, focus: b }) {
            if fs.end > fs.start {
                out.entry(fi).or_default().push((fs.start, fs.end));
            }
        }
    }
    out
}

/// The part of each text fragment of the IFC at `root` that the range `sel` covers.
fn plan_range(layout: &LayoutTree, root: taffy::NodeId, sel: Selection) -> HashMap<usize, FragSel> {
    let mut out = HashMap::new();
    let Some(il) = layout.inline.get(&root) else { return out };
    let Some(doc) = layout.node_map.values().next().map(|n| n.document_node()) else { return out };
    let Some(((ks, _), (ke, _))) = ordered(&doc, sel) else { return out };
    let ord = order(&doc);
    use crate::layout::inline::Frag;
    // Text fragments per node, in order, and each fragment's line.
    let mut by_node: Vec<(taffy::NodeId, Vec<usize>)> = Vec::new();
    let mut line_idx: HashMap<usize, usize> = HashMap::new();
    for (i, f) in il.frags.iter().enumerate() {
        if let Frag::Text { node, baseline, .. } = f {
            match by_node.iter_mut().find(|(n, _)| n == node) {
                Some((_, v)) => v.push(i),
                None => by_node.push((*node, vec![i])),
            }
            if let Some((li, _, _)) = line_of(il, *baseline) {
                line_idx.insert(i, li);
            }
        }
    }
    let last_on_line = |i: usize| -> bool {
        let li = line_idx.get(&i).copied();
        !il.frags[i + 1..].iter().enumerate().any(|(k, f)| {
            matches!(f, Frag::Text { .. }) && line_idx.get(&(i + 1 + k)).copied() == li
        })
    };
    for (tid, idxs) in by_node {
        let Some(dn) = layout.node_map.get(&tid) else { continue };
        let id = dn.id().0;
        let Some(pre) = ord.pre(id) else { continue };
        let data = dn.text_contents();
        let len = data.encode_utf16().count() as u32;
        let Some((s, e)) = local_range(pre, len, ks, ke) else { continue };
        let texts: Vec<&str> = idxs
            .iter()
            .map(|&i| match &il.frags[i] {
                Frag::Text { text, .. } => text.as_str(),
                _ => "",
            })
            .collect();
        let maps = align(&data, &texts, collapses_at(layout, tid));
        for (k, &fi) in idxs.iter().enumerate() {
            let b = &maps[k];
            let text = texts[k];
            let byte_at: Vec<usize> = text.char_indices().map(|(p, _)| p).chain(std::iter::once(text.len())).collect();
            let n = b.len() - 1;
            let mut first: Option<usize> = None;
            let mut last = 0usize;
            for c in 0..n {
                let (b0, b1) = (b[c] as i64, b[c + 1] as i64);
                let on = if b1 > b0 { s <= b0 && b1 <= e } else { s < b0 && b0 < e };
                if on {
                    first.get_or_insert(c);
                    last = c + 1;
                }
            }
            let end16 = b[n] as i64;
            let newline = last_on_line(fi) && s <= end16 && end16 < e;
            match first {
                Some(f) => {
                    out.insert(fi, FragSel { start: byte_at[f], end: byte_at[last], newline: newline && last == n });
                }
                None if newline => {
                    out.insert(fi, FragSel { start: text.len(), end: text.len(), newline: true });
                }
                None => {}
            }
        }
    }
    out
}

// =================================================================================================
// Hit testing
// =================================================================================================

/// Where a pointer at viewport (`x`, `y`) lands in the text the last frame painted: the boundary point
/// nearest to it, and the word around the grapheme under it (as two points in the same Text node).
pub struct Hit {
    pub point: Point,
    pub word: (Point, Point),
    /// The block that holds the text (the IFC root's element, else its nearest element ancestor).
    pub block: Option<usize>,
}

pub fn hit(layout: &LayoutTree, boxes: &[crate::render::TextBox], scroll: (f64, f64), x: f64, y: f64) -> Option<Hit> {
    if boxes.is_empty() {
        return None;
    }
    // Document coordinates of the pointer, against each box's recorded scroll.
    let pt = |b: &crate::render::TextBox| -> (f32, f32) {
        ((x + scroll.0 - b.scroll.0 as f64) as f32, (y + scroll.1 - b.scroll.1 as f64) as f32)
    };
    let mut pick: Option<(usize, f32, f32)> = None; // (box, dy, dx)
    for (i, b) in boxes.iter().enumerate() {
        let (px, py) = pt(b);
        let dy = if py < b.top {
            b.top - py
        } else if py >= b.bottom {
            py - b.bottom + 0.001
        } else {
            0.0
        };
        let dx = if px < b.x {
            b.x - px
        } else if px > b.x + b.w {
            px - b.x - b.w
        } else {
            0.0
        };
        // Later boxes paint on top: a tie goes to the later one.
        if pick.is_none_or(|(_, pdy, pdx)| dy < pdy || (dy == pdy && dx <= pdx)) {
            pick = Some((i, dy, dx));
        }
    }
    let (i, _, _) = pick?;
    let b = &boxes[i];
    let il = layout.inline.get(&b.ifc)?;
    let crate::layout::inline::Frag::Text { text, .. } = il.frags.get(b.frag)? else { return None };
    let dn = layout.node_map.get(&b.node)?;
    let (px, _) = pt(b);
    let (near, under) = hit_text(&b.adv, text, px - b.x);
    // Map the fragment's bytes to the node's offsets: align every fragment of this node in the IFC.
    let idxs: Vec<usize> = il
        .frags
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f, crate::layout::inline::Frag::Text { node, .. } if *node == b.node))
        .map(|(k, _)| k)
        .collect();
    let texts: Vec<&str> = idxs
        .iter()
        .map(|&k| match &il.frags[k] {
            crate::layout::inline::Frag::Text { text, .. } => text.as_str(),
            _ => "",
        })
        .collect();
    let data = dn.text_contents();
    let maps = align(&data, &texts, collapses_at(layout, b.node));
    let k = idxs.iter().position(|&f| f == b.frag)?;
    let char_ix = |byte: usize| text[..byte.min(text.len())].chars().count();
    let to16 = |byte: usize| maps[k][char_ix(byte).min(maps[k].len() - 1)];
    let id = dn.id().0;
    let point = Point { node: id, offset: to16(near) };
    let u = utf16_to_byte(&data, to16(under));
    let (ws, we) = word_at(&data, u);
    let word = (Point { node: id, offset: byte_to_utf16(&data, ws) }, Point { node: id, offset: byte_to_utf16(&data, we) });
    let mut block = layout.node_map.get(&b.ifc).map(|n| n.id().0);
    if block.is_none() {
        let mut n = dn.parent();
        while let Some(p) = n {
            if let Some(el) = p.as_element() {
                if !crate::layout::is_inline(el.name.local.as_ref()) {
                    block = Some(p.id().0);
                    break;
                }
            }
            n = p.parent();
        }
    }
    Some(Hit { point, word, block })
}

// =================================================================================================
// Text: the HTML innerText algorithm over a range
// =================================================================================================

enum Item {
    Text(String),
    Breaks(u8),
}

struct Collector<'a> {
    layout: &'a LayoutTree,
    boxes: HashMap<usize, taffy::NodeId>,
    order: &'a Order,
    ks: Key,
    ke: Key,
    items: Vec<Item>,
    /// A collapsible space waits for the next content on the line.
    pending: bool,
    /// At the start of a line (block start, after a forced break): collapsible spaces drop.
    line_start: bool,
}

impl Collector<'_> {
    fn text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.pending && !self.line_start {
            self.items.push(Item::Text(" ".into()));
        }
        self.pending = false;
        self.line_start = false;
        self.items.push(Item::Text(s.to_string()));
    }

    fn content(&mut self) {
        if self.pending && !self.line_start {
            self.items.push(Item::Text(" ".into()));
        }
        self.pending = false;
        self.line_start = false;
    }

    fn forced(&mut self, s: &str) {
        self.pending = false;
        self.items.push(Item::Text(s.into()));
        self.line_start = true;
    }

    fn breaks(&mut self, n: u8) {
        self.pending = false;
        self.items.push(Item::Breaks(n));
        self.line_start = true;
    }

    fn intersects(&self, id: usize) -> bool {
        match (self.order.pre(id), self.order.end(id)) {
            (Some(p), Some(e)) => self.ks.0 < e && self.ke.0 > p,
            _ => false,
        }
    }

    fn node(&mut self, d: &Document, id: usize, ws: u8, tt: u8) {
        match d.data(NodeId(id)) {
            NodeData::Text(t) => {
                if !self.boxes.contains_key(&id) {
                    return;
                }
                let Some(pre) = self.order.pre(id) else { return };
                let len = t.encode_utf16().count() as u32;
                let Some((s, e)) = local_range(pre, len, self.ks, self.ke) else { return };
                let (s, e) = (s.max(0) as u32, (e.min(len as i64)) as u32);
                let part = &t[utf16_to_byte(t, s)..utf16_to_byte(t, e)];
                let part = transform(part, tt);
                self.run(&part, ws);
            }
            NodeData::Element(el) => {
                let tag = el.local.to_ascii_lowercase();
                let Some(&tid) = self.boxes.get(&id) else { return };
                if self.layout.taffy.style(tid).map(|s| s.display == taffy::style::Display::None).unwrap_or(false) {
                    return;
                }
                if !self.intersects(id) {
                    return;
                }
                let spec = self.layout.paint_map.get(&tid);
                let ws = spec.and_then(|p| p.white_space).unwrap_or_else(|| crate::layout::default_white_space(&tag, ws));
                let tt = spec.and_then(|p| p.text_transform).unwrap_or(tt);
                if tag == "br" {
                    self.forced("\n");
                    return;
                }
                let kids: Vec<usize> = d.children(NodeId(id)).map(|c| c.0).collect();
                let block = self.block_level(tid, &tag);
                let atomic = !block && crate::layout::inline::is_atomic_pub(self.layout, tid);
                let n = if tag == "p" { 2 } else { 1 };
                if block {
                    self.breaks(n);
                } else if atomic {
                    self.content();
                    self.line_start = true;
                }
                for k in kids {
                    self.node(d, k, ws, tt);
                }
                if block {
                    self.breaks(n);
                } else if atomic {
                    self.pending = false;
                    self.line_start = false;
                }
                // Table cells end with a tab and rows with a newline, unless they are the last of their parent.
                if matches!(tag.as_str(), "td" | "th") && next_element(d, id).is_some() {
                    self.forced("\t");
                } else if tag == "tr" && next_row(d, id) {
                    self.forced("\n");
                }
            }
            NodeData::Document | NodeData::DocumentFragment => {
                let kids: Vec<usize> = d.children(NodeId(id)).map(|c| c.0).collect();
                for k in kids {
                    self.node(d, k, ws, tt);
                }
            }
            _ => {}
        }
    }

    fn run(&mut self, s: &str, ws: u8) {
        match ws {
            0 | 1 | 4 => {
                let mut word = String::new();
                for c in s.chars() {
                    if is_ws(c) {
                        if !word.is_empty() {
                            let w = std::mem::take(&mut word);
                            self.text(&w);
                        }
                        if ws == 4 && c == '\n' {
                            self.forced("\n");
                        } else if !self.line_start {
                            self.pending = true;
                        }
                    } else {
                        word.push(c);
                    }
                }
                if !word.is_empty() {
                    self.text(&word);
                }
            }
            _ => {
                let s: String = s.chars().filter(|&c| c != '\r').collect();
                self.text(&s);
                if s.ends_with('\n') {
                    self.line_start = true;
                }
            }
        }
    }

    fn block_level(&self, tid: taffy::NodeId, tag: &str) -> bool {
        if matches!(tag, "tr" | "td" | "th" | "thead" | "tbody" | "tfoot" | "col" | "colgroup") {
            return false;
        }
        // A flex item's display is blockified (css-flexbox-1 §4).
        if let Some(p) = self.layout.taffy.parent(tid) {
            if self.layout.paint_map.get(&p).and_then(|s| s.flex_container) == Some(true) {
                return true;
            }
        }
        match self.layout.paint_map.get(&tid).and_then(|p| p.display_kind) {
            Some(k) => k == 1,
            None => !crate::layout::is_inline(tag),
        }
    }
}

fn next_element(d: &Document, id: usize) -> Option<usize> {
    let mut n = d.next_sibling(NodeId(id));
    while let Some(s) = n {
        if d.element(s).is_some() {
            return Some(s.0);
        }
        n = d.next_sibling(s);
    }
    None
}

/// Whether a `tr` is followed by another row of the same table (in its section or a later one).
fn next_row(d: &Document, id: usize) -> bool {
    if next_element(d, id).is_some() {
        return true;
    }
    let Some(section) = d.parent(NodeId(id)) else { return false };
    if d.element(section).is_some_and(|e| e.is_html("table")) {
        return false;
    }
    let mut n = d.next_sibling(section);
    while let Some(s) = n {
        if d.children(s).any(|c| d.element(c).is_some_and(|e| e.is_html("tr"))) {
            return true;
        }
        n = d.next_sibling(s);
    }
    false
}

fn transform(s: &str, tt: u8) -> String {
    match tt {
        1 => s.to_uppercase(),
        2 => s.to_lowercase(),
        3 => {
            let mut out = String::with_capacity(s.len());
            let mut at_start = true;
            for c in s.chars() {
                if at_start && c.is_alphanumeric() {
                    out.extend(c.to_uppercase());
                    at_start = false;
                } else {
                    if is_ws(c) {
                        at_start = true;
                    }
                    out.push(c);
                }
            }
            out
        }
        _ => s.to_string(),
    }
}

/// The rendered text (HTML `innerText`, §3.2.7 "rendered text collection steps") of the part of
/// `doc`'s document between boundary-point keys `ks` and `ke`.
pub fn range_text_keys(layout: &LayoutTree, doc: &NodeRef, ks: Key, ke: Key) -> String {
    let ord = order(doc);
    let boxes: HashMap<usize, taffy::NodeId> = layout.node_map.iter().map(|(t, n)| (n.id().0, *t)).collect();
    let mut c = Collector { layout, boxes, order: &ord, ks, ke, items: Vec::new(), pending: false, line_start: true };
    doc.with_doc(|d, _| c.node(d, Document::ROOT.0, 0, 0));
    // Remove empty strings, leading/trailing break runs; collapse each run of breaks to its maximum.
    let items: Vec<Item> = c.items.into_iter().filter(|i| !matches!(i, Item::Text(t) if t.is_empty())).collect();
    let first = items.iter().position(|i| matches!(i, Item::Text(_)));
    let last = items.iter().rposition(|i| matches!(i, Item::Text(_)));
    let (Some(first), Some(last)) = (first, last) else { return String::new() };
    let mut out = String::new();
    let mut run = 0u8;
    for it in &items[first..=last] {
        match it {
            Item::Breaks(n) => run = run.max(*n),
            Item::Text(t) => {
                for _ in 0..run {
                    out.push('\n');
                }
                run = 0;
                out.push_str(t);
            }
        }
    }
    out
}

/// The rendered text of the range [start, end] (points in `doc`'s document).
pub fn range_text(layout: &LayoutTree, doc: &NodeRef, start: Point, end: Point) -> String {
    let ord = order(doc);
    let keys = doc.with_doc(|d, _| Some((ord.key(d, start)?, ord.key(d, end)?)));
    match keys {
        Some((a, b)) if a <= b => range_text_keys(layout, doc, a, b),
        Some((a, b)) => range_text_keys(layout, doc, b, a),
        None => String::new(),
    }
}

/// The text the current selection copies (`Selection.toString()`).
pub fn selection_text(layout: &LayoutTree, doc: &NodeRef) -> String {
    match get() {
        Some(s) => range_text(layout, doc, s.anchor, s.focus),
        None => String::new(),
    }
}

/// `element.innerText` (the whole element's contents).
pub fn inner_text(layout: &LayoutTree, element: &NodeRef) -> String {
    let len = element.with_doc(|d, id| node_length(d, id.0));
    let doc = element.document_node();
    range_text(layout, &doc, Point { node: element.id().0, offset: 0 }, Point { node: element.id().0, offset: len })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_collapsed_fragments_kat() {
        // "  aa   bb\n cc " laid out as two lines "aa bb" / "cc".
        let m = align("  aa   bb\n cc ", &["aa bb", "cc"], true);
        assert_eq!(m[0], vec![2, 3, 4, 7, 8, 9]);
        assert_eq!(m[1], vec![11, 12, 13]);
        // A lead space carried in from the previous node has no width.
        let m = align("x", &[" x"], true);
        assert_eq!(m[0], vec![0, 0, 1]);
        // pre: a tab expanded to spaces maps to the tab.
        let m = align("a\tb", &["a       b"], false);
        assert_eq!(m[0][1], 1);
        assert_eq!(*m[0].last().unwrap(), 3);
        // Non-BMP characters count two code units.
        let m = align("a😀b", &["a😀b"], true);
        assert_eq!(m[0], vec![0, 1, 3, 4]);
    }

    #[test]
    fn uax29_words_kat() {
        let t = "can't stop 3.14, e.g. foo_bar  世界";
        let ws: Vec<&str> = words(t).iter().map(|&(a, b)| &t[a..b]).collect();
        assert_eq!(ws, vec!["can't", " ", "stop", " ", "3.14", ",", " ", "e.g", ".", " ", "foo_bar", "  ", "世", "界"]);
        assert_eq!(word_at("hello world", 7), (6, 11));
        assert_eq!(word_at("hello world", 5), (5, 6));
        assert_eq!(word_at("hello", 5), (0, 5));
    }

    #[test]
    fn utf16_offsets_kat() {
        assert_eq!(utf16_to_byte("a😀b", 3), 5);
        assert_eq!(byte_to_utf16("a😀b", 5), 3);
        assert_eq!(utf16_to_byte("ab", 9), 2);
    }
}
