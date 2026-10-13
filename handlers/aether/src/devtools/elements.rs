//! The Elements panel: the live DOM as an indented tree, and for the selected element its attributes,
//! its border box and its computed values.
//!
//! Seam: `AetherEngine::document` (the page's html_core arena, read through `dom::NodeRef`), the
//! computed values `css::computed_report` derives from `AetherEngine::layout_tree` (what layout and paint
//! consume, serialized as `getComputedStyle` does, the 20 properties of `css::REPORT_PROPS`), the box
//! from `AetherEngine::box_of`, and the picker from `AetherEngine::hit_test_at`. Nothing is cached: each
//! call reads the tree as it stands, so a script's mutation shows on the next read.

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
