//! The Elements panel: the live DOM as an indented tree, and for the selected element its attributes,
//! its border box and its computed values.
//!
//! Seam: `AetherEngine::document` (the page's html_core arena, read through `dom::NodeRef`), the
//! computed values `css::computed_report` derives from `AetherEngine::layout_tree` (what layout and paint
//! consume, serialized as `getComputedStyle` does, the 20 properties of `css::REPORT_PROPS`), the box
//! from `AetherEngine::box_of`, and the picker from `AetherEngine::hit_test_at`. Nothing is cached: each
//! call reads the tree as it stands, so a script's mutation shows on the next read.
//!
//! AETHERDEVTOOLS3 (SR128): the selected element's box model ([`box_model`]) — margin, border, padding and
//! content as layout resolved them (taffy's `Layout.margin` / `border` / `padding` and the border-box
//! `size` of the element's box in `AetherEngine::layout_tree`, the box `box_of` measures) — drawn as four
//! nested rectangles with the numbers on their edges ([`box_model_lines`]), the way every devtools draws it.

use crate::dom::{NodeData, NodeRef};
use crate::AetherEngine;

/// Rows [`tree`] lists before it stops (a face pages past it by collapsing).
pub const MAX_ROWS: usize = 5000;

/// Longest text a row shows of a text or comment node.
const TEXT_CLIP: usize = 80;

/// What a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Document,
    Doctype,
    Element,
    Text,
    Comment,
    Other,
}

/// One row of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeRow {
    /// The node's arena id (what [`inspect`] takes).
    pub id: usize,
    pub depth: usize,
    pub kind: RowKind,
    /// The row's text: `<div id="a" class="b">`, `"text"`, `<!-- c -->`, `<!DOCTYPE html>`, `#document`.
    pub label: String,
    /// Child nodes the row has (whitespace-only text included).
    pub children: usize,
}

/// The selected element, inspected.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    pub id: usize,
    pub tag: String,
    /// The attributes in source order.
    pub attributes: Vec<(String, String)>,
    /// The border box in document coordinates (x, y, w, h); None when the element has no box.
    pub border_box: Option<(f32, f32, f32, f32)>,
    /// `(property, computed value)` for each of `css::REPORT_PROPS`; an element without a box reports
    /// `display: none` and nothing else (the report's own rule).
    pub computed: Vec<(&'static str, String)>,
}

fn clip(s: &str) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > TEXT_CLIP {
        let mut t: String = flat.chars().take(TEXT_CLIP).collect();
        t.push('…');
        t
    } else {
        flat
    }
}

/// The row label of one node, and its kind.
pub fn label(node: &NodeRef) -> (RowKind, String) {
    match node.data() {
        NodeData::Document(_) => (RowKind::Document, "#document".to_string()),
        NodeData::DocumentFragment => (RowKind::Other, "#document-fragment".to_string()),
        NodeData::Doctype(d) => (RowKind::Doctype, format!("<!DOCTYPE {}>", d.name)),
        NodeData::Element(e) => {
            let mut s = format!("<{}", e.name.local.0);
            for (_, a) in e.attributes.borrow().map.iter() {
                s.push_str(&format!(" {}=\"{}\"", a.local, clip(&a.value)));
            }
            s.push('>');
            (RowKind::Element, s)
        }
        NodeData::Text(t) => (RowKind::Text, format!("\"{}\"", clip(&t.borrow()))),
        NodeData::Comment(t) => (RowKind::Comment, format!("<!-- {} -->", clip(&t.borrow()))),
        NodeData::ProcessingInstruction(t) => (RowKind::Other, format!("<?{}?>", clip(&t.borrow()))),
    }
}

/// The document as rows in tree order (depth-first), whitespace-only text nodes left out, at most
/// `max` rows. Empty before the first load.
pub fn tree(engine: &AetherEngine, max: usize) -> Vec<NodeRow> {
    let mut out = Vec::new();
    let Some(doc) = engine.document.clone() else { return out };
    fn walk(node: &NodeRef, depth: usize, max: usize, out: &mut Vec<NodeRow>) {
        if out.len() >= max {
            return;
        }
        let (kind, label) = label(node);
        if kind == RowKind::Text && label == "\"\"" {
            return;
        }
        let kids: Vec<NodeRef> = node.children().collect();
        out.push(NodeRow { id: node.id().0, depth, kind, label, children: kids.len() });
        for k in &kids {
            walk(k, depth + 1, max, out);
        }
        // `<template>` contents are a fragment of the same arena, shown under the element.
        if let Some(t) = node.as_element().and_then(|e| e.template_contents) {
            walk(&t, depth + 1, max, out);
        }
    }
    walk(&doc.document_node(), 0, max, &mut out);
    out
}

/// The node with arena id `id` in the current document, when it is one of its nodes.
pub fn node(engine: &AetherEngine, id: usize) -> Option<NodeRef> {
    let doc = engine.document.as_ref()?.document_node();
    doc.inclusive_descendants().find(|n| n.id().0 == id)
}

/// The element with arena id `id`, inspected (None for a non-element or an id not in the document).
pub fn inspect(engine: &AetherEngine, id: usize) -> Option<Inspection> {
    let n = node(engine, id)?;
    let e = n.as_element()?;
    let attributes = e.attributes.borrow().map.iter().map(|(_, a)| (a.local.clone(), a.value.clone())).collect();
    let report = engine.layout_tree.as_ref().map(crate::css::computed_report).unwrap_or_default();
    let computed = match report.get(&html_core::NodeId(id)) {
        Some(values) => crate::css::REPORT_PROPS.iter().copied().zip(values.iter().cloned()).collect(),
        None => vec![("display", "none".to_string())],
    };
    Some(Inspection { id, tag: e.name.local.0.clone(), attributes, border_box: engine.box_of(&n), computed })
}

/// The element under viewport (`x`, `y`): the paint-order hit test the pointer uses.
pub fn pick(engine: &AetherEngine, x: f64, y: f64) -> Option<usize> {
    engine.hit_test_at(x, y).map(|(n, _)| n.id().0)
}

/// The path from the root element to `id` as `html > body > div#a.b` (the breadcrumb under the tree).
pub fn breadcrumb(engine: &AetherEngine, id: usize) -> String {
    let Some(n) = node(engine, id) else { return String::new() };
    let mut parts: Vec<String> = std::iter::once(n.clone())
        .chain(n.ancestors())
        .filter_map(|a| {
            let e = a.as_element()?;
            let attrs = e.attributes.borrow();
            let mut s = e.name.local.0.clone();
            if let Some(i) = attrs.get("id").filter(|v| !v.is_empty()) {
                s.push('#');
                s.push_str(i);
            }
            for c in attrs.get("class").unwrap_or("").split_whitespace() {
                s.push('.');
                s.push_str(c);
            }
            Some(s)
        })
        .collect();
    parts.reverse();
    parts.join(" > ")
}

/// Four edge widths in CSS px.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    fn of(r: taffy::geometry::Rect<f32>) -> Edges {
        Edges { top: r.top, right: r.right, bottom: r.bottom, left: r.left }
    }
}

/// An element's box model as layout resolved it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxModel {
    pub margin: Edges,
    pub border: Edges,
    pub padding: Edges,
    /// The content box's (width, height): the border box less border and padding.
    pub content: (f32, f32),
}

/// The box model of the element with arena id `id`: its first box in tree order (the one `box_of`
/// measures). None for a non-element, an element without a box of its own (`display: none`, or a run
/// inside an inline formatting context), or before the first layout.
pub fn box_model(engine: &AetherEngine, id: usize) -> Option<BoxModel> {
    let n = node(engine, id)?;
    n.as_element()?;
    let layout = engine.layout_tree.as_ref()?;
    fn find(id: taffy::prelude::NodeId, layout: &crate::layout::LayoutTree, want: &NodeRef) -> Option<taffy::prelude::NodeId> {
        if layout.node_map.get(&id) == Some(want) {
            return Some(id);
        }
        layout.taffy.children(id).ok()?.into_iter().find_map(|c| find(c, layout, want))
    }
    let tid = find(layout.root_node, layout, &n)?;
    let l = layout.taffy.layout(tid).ok()?;
    let (border, padding) = (Edges::of(l.border), Edges::of(l.padding));
    let content = (
        (l.size.width - border.left - border.right - padding.left - padding.right).max(0.0),
        (l.size.height - border.top - border.bottom - padding.top - padding.bottom).max(0.0),
    );
    Some(BoxModel { margin: Edges::of(l.margin), border, padding, content })
}

/// A box-model number: up to two decimals, trailing zeros dropped; an edge of zero reads `-` (as the
/// Chromium and Firefox drawings print it).
fn edge(v: f32) -> String {
    if v.abs() < 0.005 {
        return "-".to_string();
    }
    let s = format!("{:.2}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The box model drawn as text, four nested rectangles: margin outermost, then border, padding and the
/// content box holding its `w x h`; each outer rectangle is labelled at its top-left with its top edge's
/// number centred beside the label, its bottom edge's centred below, and its left and right edges' numbers
/// on the content's row. A zero edge reads `-`.
///
/// ```text
/// +----------------------------------------+
/// | margin            10                   |
/// |    +------------------------------+    |
/// |    | border       2               |    |
/// |    |   +----------------------+   |    |
/// |    |   | padding  5           |   |    |
/// |    |   |     +----------+     |   |    |
/// | 10 | 2 |   6 | 100 x 40 | 6   | 2 | 10 |
/// ...
/// ```
pub fn box_model_lines(bm: &BoxModel) -> Vec<String> {
    let size = format!(" {} x {} ", edge(bm.content.0).replace('-', "0"), edge(bm.content.1).replace('-', "0"));
    let content_rule = format!("+{}+", "-".repeat(size.chars().count()));
    let mut inner = vec![content_rule.clone(), format!("|{size}|"), content_rule];
    for (label, e) in [("padding", bm.padding), ("border", bm.border), ("margin", bm.margin)] {
        let iw = inner[0].chars().count();
        let (t, r, b, l) = (edge(e.top), edge(e.right), edge(e.bottom), edge(e.left));
        let vmax = t.len().max(b.len());
        let mut sw = l.len().max(r.len());
        while iw + 2 * (sw + 2) < 2 * (label.len() + 3) + vmax {
            sw += 1;
        }
        let w = iw + 2 * (sw + 2);
        let centred = |v: &str, label: Option<&str>| {
            let mut row: Vec<char> = vec![' '; w];
            let at = (w - v.len()) / 2;
            for (i, c) in v.chars().enumerate() {
                row[at + i] = c;
            }
            if let Some(lb) = label {
                for (i, c) in lb.chars().enumerate() {
                    row[1 + i] = c;
                }
            }
            format!("|{}|", row.into_iter().collect::<String>())
        };
        let rule = format!("+{}+", "-".repeat(w));
        let mid = inner.len() / 2;
        let mut next = vec![rule.clone(), centred(&t, Some(label))];
        for (k, line) in inner.iter().enumerate() {
            let (sl, sr) = if k == mid { (format!(" {l:>sw$} "), format!(" {r:<sw$} ")) } else { (" ".repeat(sw + 2), " ".repeat(sw + 2)) };
            next.push(format!("|{sl}{line}{sr}|"));
        }
        next.push(centred(&b, None));
        next.push(rule);
        inner = next;
    }
    inner
}
