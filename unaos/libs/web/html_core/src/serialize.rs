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

fn serialize_children(doc: &Document, node: NodeId, opts: SerializeOpts, s: &mut String) {
    if is_void_element(doc, node) {
        return;
    }
    let node = match doc.element(node) {
        Some(e) if e.ns == Namespace::Html && e.local == "template" => e.template_contents.unwrap_or(node),
        _ => node,
    };
    for child in doc.children(node) {
        serialize_node(doc, child, opts, s);
    }
}

fn serialize_node(doc: &Document, n: NodeId, opts: SerializeOpts, s: &mut String) {
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
            serialize_children(doc, n, opts, s);
            s.push_str("</");
            s.push_str(tagname);
            s.push('>');
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
        NodeData::Document | NodeData::DocumentFragment => serialize_children(doc, n, opts, s),
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
