//! §13.3 "Serializing HTML fragments", written from the specification, plus the html5lib tree-dump format
//! used by the tree-construction vectors.

use alloc::string::String;
use alloc::vec::Vec;

use crate::dom::{Document, Namespace, NodeData, NodeId};

/// "serializes as void": the void elements plus basefont, bgsound, frame, keygen, param.
pub fn serializes_as_void(local: &str) -> bool {
    matches!(
        local,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "source"
            | "track"
            | "wbr"
            | "basefont"
            | "bgsound"
            | "frame"
            | "keygen"
            | "param"
    )
}

/// Serialization options.
#[derive(Clone, Copy, Debug, Default)]
pub struct SerializeOpts {
    /// "scripting is enabled for the node" — makes `<noscript>` text literal. UnaOS parses with scripting off.
    pub scripting: bool,
}

/// Escaping a string (§13.3): `&` `U+00A0` `<` `>` always, `"` in attribute mode.
pub fn escape(out: &mut String, s: &str, attr_mode: bool) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '\u{A0}' => out.push_str("&nbsp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr_mode => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

fn is_void_element(doc: &Document, n: NodeId) -> bool {
    doc.element(n).is_some_and(|e| e.ns == Namespace::Html && serializes_as_void(&e.local))
}

/// The HTML fragment serialization algorithm: the serialization of `node`'s children (`innerHTML`).
pub fn inner_html(doc: &Document, node: NodeId, opts: SerializeOpts) -> String {
    let mut s = String::new();
    serialize_children(doc, node, opts, &mut s);
    s
}

/// `outerHTML`: the node itself followed by its children (for an element), as browsers expose it.
pub fn outer_html(doc: &Document, node: NodeId, opts: SerializeOpts) -> String {
    let mut s = String::new();
    serialize_node(doc, node, opts, &mut s);
    s
}

/// The children whose serializations make up `node`'s inner HTML (template contents for a template).
fn content_root(doc: &Document, node: NodeId) -> Option<NodeId> {
    if is_void_element(doc, node) {
        return None;
    }
    Some(match doc.element(node) {
        Some(e) if e.ns == Namespace::Html && e.local == "template" => e.template_contents.unwrap_or(node),
        _ => node,
    })
}

fn serialize_children(doc: &Document, node: NodeId, opts: SerializeOpts, s: &mut String) {
    if let Some(root) = content_root(doc, node) {
        walk(doc, doc.first_child(root), opts, s);
    }
}

fn serialize_node(doc: &Document, n: NodeId, opts: SerializeOpts, s: &mut String) {
    match doc.data(n) {
        NodeData::Document | NodeData::DocumentFragment => serialize_children(doc, n, opts, s),
        _ => {
            let mut stack: Vec<NodeId> = Vec::new();
            emit(doc, n, opts, s, &mut stack);
            drain(doc, opts, s, &mut stack);
        }
    }
}

/// Serialize a run of siblings starting at `first`, iteratively (no recursion: arbitrarily deep trees are fine).
fn walk(doc: &Document, first: Option<NodeId>, opts: SerializeOpts, s: &mut String) {
    let mut stack: Vec<NodeId> = Vec::new();
    let mut cur = first;
    while let Some(n) = cur {
        emit(doc, n, opts, s, &mut stack);
        drain(doc, opts, s, &mut stack);
        cur = doc.next_sibling(n);
    }
}

/// Finish every element `emit` left open on `stack` (start tag written): its children, then its end tag.
fn drain(doc: &Document, opts: SerializeOpts, s: &mut String, stack: &mut Vec<NodeId>) {
    // Each frame is an element whose start tag was written; walk its children, then close it.
    let mut cursors: Vec<Option<NodeId>> = Vec::new();
    let mut frames: Vec<NodeId> = Vec::new();
    for f in stack.drain(..) {
        let first = content_root(doc, f).and_then(|r| doc.first_child(r));
        frames.push(f);
        cursors.push(first);
    }
    // `frames`/`cursors` act as one explicit recursion stack.
    while let Some(cursor) = cursors.last_mut() {
        match *cursor {
            Some(child) => {
                *cursor = doc.next_sibling(child);
                let mut pushed: Vec<NodeId> = Vec::new();
                emit(doc, child, opts, s, &mut pushed);
                for f in pushed {
                    let first = content_root(doc, f).and_then(|r| doc.first_child(r));
                    frames.push(f);
                    cursors.push(first);
                }
            }
            None => {
                cursors.pop();
                let el = frames.pop().expect("frame");
                if let Some(e) = doc.element(el) {
                    s.push_str("</");
                    s.push_str(&e.local);
                    s.push('>');
                }
            }
        }
    }
}

/// Write `n`'s own markup; for a non-void element write the start tag and push it so its children follow.
fn emit(doc: &Document, n: NodeId, opts: SerializeOpts, s: &mut String, stack: &mut Vec<NodeId>) {
    match doc.data(n) {
        NodeData::Element(e) => {
            let tagname = e.local.as_str();
            s.push('<');
            s.push_str(tagname);
            for a in &e.attrs {
                s.push(' ');
                match a.ns {
                    Namespace::None => {}
                    Namespace::Xml => s.push_str("xml:"),
                    Namespace::Xmlns if a.local == "xmlns" => {}
                    Namespace::Xmlns => s.push_str("xmlns:"),
                    Namespace::XLink => s.push_str("xlink:"),
                    _ => {
                        if let Some(p) = a.prefix {
                            s.push_str(p);
                            s.push(':');
                        }
                    }
                }
                s.push_str(&a.local);
                s.push_str("=\"");
                escape(s, &a.value, true);
                s.push('"');
            }
            s.push('>');
            if e.ns == Namespace::Html && serializes_as_void(tagname) {
                return;
            }
            stack.push(n);
        }
        NodeData::Text(t) => {
            let raw = doc.parent(n).and_then(|p| doc.element(p)).is_some_and(|pe| {
                pe.ns == Namespace::Html
                    && (matches!(pe.local.as_str(), "style" | "script" | "xmp" | "iframe" | "noembed" | "noframes" | "plaintext")
                        || (pe.local == "noscript" && opts.scripting))
            });
            if raw {
                s.push_str(t);
            } else {
                escape(s, t, false);
            }
        }
        NodeData::Comment(c) => {
            s.push_str("<!--");
            s.push_str(c);
            s.push_str("-->");
        }
        NodeData::ProcessingInstruction { target, data } => {
            s.push_str("<?");
            s.push_str(target);
            s.push(' ');
            s.push_str(data);
            s.push_str("?>");
        }
        NodeData::Doctype { name, .. } => {
            s.push_str("<!DOCTYPE ");
            s.push_str(name);
            s.push('>');
        }
        NodeData::Document | NodeData::DocumentFragment => {
            let mut cur = doc.first_child(n);
            while let Some(c) = cur {
                emit(doc, c, opts, s, stack);
                drain(doc, opts, s, stack);
                cur = doc.next_sibling(c);
            }
        }
    }
}

/// The html5lib tree-construction dump format (`| <html>` lines, two spaces per depth, attributes sorted).
pub fn html5lib_dump(doc: &Document, root: NodeId) -> String {
    let mut out = String::new();
    for c in doc.children(root) {
        dump_node(doc, c, 1, &mut out);
    }
    out
}

fn indent(out: &mut String, depth: usize) {
    out.push('|');
    out.push(' ');
    for _ in 1..depth {
        out.push_str("  ");
    }
}

fn dump_node(doc: &Document, n: NodeId, depth: usize, out: &mut String) {
    indent(out, depth);
    match doc.data(n) {
        NodeData::Doctype { name, public_id, system_id } => {
            out.push_str("<!DOCTYPE ");
            out.push_str(name);
            if !public_id.is_empty() || !system_id.is_empty() {
                out.push_str(" \"");
                out.push_str(public_id);
                out.push_str("\" \"");
                out.push_str(system_id);
                out.push('"');
            }
            out.push_str(">\n");
        }
        NodeData::Comment(c) => {
            out.push_str("<!-- ");
            out.push_str(c);
            out.push_str(" -->\n");
        }
        NodeData::ProcessingInstruction { target, data } => {
            out.push_str("<?");
            out.push_str(target);
            out.push(' ');
            out.push_str(data);
            out.push_str(">\n");
        }
        NodeData::Text(t) => {
            out.push('"');
            out.push_str(t);
            out.push_str("\"\n");
        }
        NodeData::Document | NodeData::DocumentFragment => out.push_str("#fragment\n"),
        NodeData::Element(e) => {
            out.push('<');
            match e.ns {
                Namespace::Svg => out.push_str("svg "),
                Namespace::MathMl => out.push_str("math "),
                _ => {}
            }
            out.push_str(&e.local);
            out.push_str(">\n");
            let mut attrs: Vec<(String, &str)> = e
                .attrs
                .iter()
                .map(|a| {
                    let name = match (a.ns, a.prefix) {
                        (Namespace::None, _) => a.local.clone(),
                        (_, Some(p)) => {
                            let mut s = String::from(p);
                            s.push(' ');
                            s.push_str(&a.local);
                            s
                        }
                        (_, None) => {
                            let mut s = String::from(if a.ns == Namespace::Xmlns { "xmlns" } else { "" });
                            s.push(' ');
                            s.push_str(&a.local);
                            s
                        }
                    };
                    (name, a.value.as_str())
                })
                .collect();
            attrs.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, value) in attrs {
                indent(out, depth + 1);
                out.push_str(&name);
                out.push_str("=\"");
                out.push_str(value);
                out.push_str("\"\n");
            }
            if let Some(contents) = e.template_contents {
                indent(out, depth + 1);
                out.push_str("content\n");
                for c in doc.children(contents) {
                    dump_node(doc, c, depth + 2, out);
                }
            } else {
                for c in doc.children(n) {
                    dump_node(doc, c, depth + 1, out);
                }
            }
        }
    }
}
