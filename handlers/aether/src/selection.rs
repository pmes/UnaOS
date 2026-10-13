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
//!
//! AETHERMETAL M3 (SR96): the hit test, the fragment alignment, the word segmenter, the offset conversions and
//! [`Point`] are `layout_core::hit` — ONE function both rings call (APPS/AETHER.ELF hit-tests with it on the
//! image); this file keeps the selection state, the document order, the paint plan and the text collector.

use crate::dom::NodeRef;
use crate::layout::LayoutTree;
use html_core::{Document, NodeData, NodeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// A DOM boundary point (AETHERMETAL M3, SR96: the type is `layout_core::hit::Point`, shared with APPS/AETHER.ELF).
pub use layout_core::hit::Point;

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

// AETHERMETAL M3 (SR96): the offset conversions live in `layout_core::hit` (both rings).
pub use layout_core::hit::{byte_to_utf16, utf16_to_byte};

// =================================================================================================
// Fragment text ↔ DOM offsets
// =================================================================================================

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

/// Aligns a Text node's line fragments with its data (AETHERMETAL M3, SR96: `layout_core::hit::align`).
pub use layout_core::hit::align;

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
    layout_core::hit::hit_text(text, x, |b| caret_x(adv, text, b))
}

// =================================================================================================
// Words (UAX #29, simplified to the rules a click needs)
// =================================================================================================

// AETHERMETAL M3 (SR96): the UAX #29 word segmenter lives in `layout_core::hit` (both rings).
pub use layout_core::hit::{word_at, words};

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

/// The line box (index, top, height) a fragment's baseline sits on (AETHERMETAL M3, SR96: `layout_core::hit::line_of`).
pub use layout_core::hit::line_of;

/// Whether text in `tid`'s white-space mode collapses (normal / nowrap / pre-line).
fn collapses_at(layout: &LayoutTree, tid: taffy::NodeId) -> bool {
    layout_core::hit::collapses_at(layout, tid)
}

/// For every text fragment of the IFC rooted at `root`, the selected part (indexed like `il.frags`).
/// Empty when nothing is selected.
pub fn plan_ifc(layout: &LayoutTree, root: taffy::NodeId) -> HashMap<usize, FragSel> {
    let mut out = HashMap::new();
    let Some(sel) = get() else { return out };
    if sel.is_collapsed() {
        return out;
    }
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
    // AETHERMETAL M3 (SR96): the hit is `layout_core::hit::hit`, the function APPS/AETHER.ELF calls; this ring
    // hands it the boxes the last frame painted, each with its recorded scroll, and its shaper's caret.
    let core: Vec<layout_core::hit::TextBox> = boxes
        .iter()
        .map(|b| layout_core::hit::TextBox { ifc: b.ifc, frag: b.frag, node: b.node, x: b.x, w: b.w, top: b.top, bottom: b.bottom })
        .collect();
    // Document coordinates of the pointer, against each box's recorded scroll.
    let pt = |i: usize| -> (f32, f32) {
        let b = &boxes[i];
        ((x + scroll.0 - b.scroll.0 as f64) as f32, (y + scroll.1 - b.scroll.1 as f64) as f32)
    };
    let h = layout_core::hit::hit(layout, &core, pt, |i, text, b| caret_x(&boxes[i].adv, text, b), |n| n.id().0)?;
    Some(Hit { point: h.point, word: h.word, block: h.block })
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
