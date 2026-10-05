//! The arena DOM: every node lives in one `Vec`, addressed by [`NodeId`]; parent/child/sibling links are ids.
//!
//! This is the minimal DOM the tree builder (§13.2.6) needs plus navigation and `querySelector`-shaped traversal
//! hooks for a later CSS core: element name + namespace, attributes (with the namespaced forms the parser's
//! "adjust foreign attributes" produces), text, comments, processing instructions, doctype, document fragments
//! (template contents). Detached nodes stay in the arena (ids are never reused).

use alloc::string::String;
use alloc::vec::Vec;

/// A node's index in [`Document::nodes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub usize);

/// Namespaces the HTML parser can produce (§2.1.x "namespaces").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Namespace {
    Html,
    Svg,
    MathMl,
    XLink,
    Xml,
    Xmlns,
    /// No namespace (ordinary attributes).
    None,
}

impl Namespace {
    pub fn url(self) -> &'static str {
        match self {
            Namespace::Html => "http://www.w3.org/1999/xhtml",
            Namespace::Svg => "http://www.w3.org/2000/svg",
            Namespace::MathMl => "http://www.w3.org/1998/Math/MathML",
            Namespace::XLink => "http://www.w3.org/1999/xlink",
            Namespace::Xml => "http://www.w3.org/XML/1998/namespace",
            Namespace::Xmlns => "http://www.w3.org/2000/xmlns/",
            Namespace::None => "",
        }
    }
}

/// An attribute on an element. `prefix`/`ns` are set only by "adjust foreign attributes" (§13.2.6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub prefix: Option<&'static str>,
    pub ns: Namespace,
    pub local: String,
    pub value: String,
}

/// Element payload.
#[derive(Clone, Debug)]
pub struct Element {
    pub ns: Namespace,
    pub local: String,
    pub attrs: Vec<Attribute>,
    /// For `<template>` in the HTML namespace: the DocumentFragment holding its contents.
    pub template_contents: Option<NodeId>,
    /// MathML `annotation-xml` whose start tag had `encoding` = text/html or application/xhtml+xml.
    pub html_integration_point: bool,
}

impl Element {
    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attrs.iter().find(|a| a.ns == Namespace::None && a.local == local).map(|a| a.value.as_str())
    }
    pub fn is(&self, ns: Namespace, local: &str) -> bool {
        self.ns == ns && self.local == local
    }
    pub fn is_html(&self, local: &str) -> bool {
        self.ns == Namespace::Html && self.local == local
    }
}

/// What a node is.
#[derive(Clone, Debug)]
pub enum NodeData {
    Document,
    DocumentFragment,
    Doctype { name: String, public_id: String, system_id: String },
    Element(Element),
    Text(String),
    Comment(String),
    ProcessingInstruction { target: String, data: String },
}

/// One arena slot.
#[derive(Clone, Debug)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub first_child: Option<NodeId>,
    pub last_child: Option<NodeId>,
    pub prev_sibling: Option<NodeId>,
    pub next_sibling: Option<NodeId>,
    pub data: NodeData,
}

/// Document mode (§13.2.6.4.1 sets it from the DOCTYPE).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuirksMode {
    NoQuirks,
    LimitedQuirks,
    Quirks,
}

/// A document: the arena plus the root `Document` node (always `NodeId(0)`).
#[derive(Clone, Debug)]
pub struct Document {
    pub nodes: Vec<Node>,
    pub quirks_mode: QuirksMode,
}

impl Default for Document {
    fn default() -> Self {
        Document::new()
    }
}

impl Document {
    pub const ROOT: NodeId = NodeId(0);

    pub fn new() -> Document {
        let mut d = Document { nodes: Vec::new(), quirks_mode: QuirksMode::NoQuirks };
        d.create(NodeData::Document);
        d
    }

    /// Create a detached node.
    pub fn create(&mut self, data: NodeData) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            parent: None,
            first_child: None,
            last_child: None,
            prev_sibling: None,
            next_sibling: None,
            data,
        });
        id
    }

    /// Create a detached element (and, for an HTML `template`, its contents fragment).
    pub fn create_element(&mut self, ns: Namespace, local: &str, attrs: Vec<Attribute>) -> NodeId {
        let template_contents =
            if ns == Namespace::Html && local == "template" { Some(self.create(NodeData::DocumentFragment)) } else { None };
        self.create(NodeData::Element(Element {
            ns,
            local: String::from(local),
            attrs,
            template_contents,
            html_integration_point: false,
        }))
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0]
    }
    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.0]
    }
    pub fn data(&self, id: NodeId) -> &NodeData {
        &self.nodes[id.0].data
    }
    pub fn element(&self, id: NodeId) -> Option<&Element> {
        match &self.nodes[id.0].data {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }
    pub fn element_mut(&mut self, id: NodeId) -> Option<&mut Element> {
        match &mut self.nodes[id.0].data {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0].parent
    }
    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0].first_child
    }
    pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0].last_child
    }
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0].next_sibling
    }
    pub fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0].prev_sibling
    }

    /// Iterate a node's children in order.
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children { doc: self, next: self.first_child(id) }
    }

    /// Iterate a node's descendants in tree order (not including `id`; not entering template contents).
    pub fn descendants(&self, id: NodeId) -> Descendants<'_> {
        Descendants { doc: self, root: id, next: self.first_child(id) }
    }

    /// The document element (the `Document`'s element child).
    pub fn document_element(&self) -> Option<NodeId> {
        self.children(Self::ROOT).find(|&c| self.element(c).is_some())
    }

    /// Remove `id` from its parent (no-op if detached).
    pub fn detach(&mut self, id: NodeId) {
        let (parent, prev, next) = {
            let n = &self.nodes[id.0];
            (n.parent, n.prev_sibling, n.next_sibling)
        };
        let Some(parent) = parent else { return };
        match prev {
            Some(p) => self.nodes[p.0].next_sibling = next,
            None => self.nodes[parent.0].first_child = next,
        }
        match next {
            Some(n) => self.nodes[n.0].prev_sibling = prev,
            None => self.nodes[parent.0].last_child = prev,
        }
        let n = &mut self.nodes[id.0];
        n.parent = None;
        n.prev_sibling = None;
        n.next_sibling = None;
    }

    /// Append `child` (detaching it first) as `parent`'s last child.
    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        self.detach(child);
        let last = self.nodes[parent.0].last_child;
        {
            let c = &mut self.nodes[child.0];
            c.parent = Some(parent);
            c.prev_sibling = last;
            c.next_sibling = None;
        }
        match last {
            Some(l) => self.nodes[l.0].next_sibling = Some(child),
            None => self.nodes[parent.0].first_child = Some(child),
        }
        self.nodes[parent.0].last_child = Some(child);
    }

    /// Insert `child` (detaching it first) into `parent` before `reference` (append when `None`).
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
        let Some(r) = reference else {
            self.append(parent, child);
            return;
        };
        self.detach(child);
        let prev = self.nodes[r.0].prev_sibling;
        {
            let c = &mut self.nodes[child.0];
            c.parent = Some(parent);
            c.prev_sibling = prev;
            c.next_sibling = Some(r);
        }
        self.nodes[r.0].prev_sibling = Some(child);
        match prev {
            Some(p) => self.nodes[p.0].next_sibling = Some(child),
            None => self.nodes[parent.0].first_child = Some(child),
        }
    }

    /// Move every child of `from` to the end of `to`, in order.
    pub fn reparent_children(&mut self, from: NodeId, to: NodeId) {
        while let Some(c) = self.first_child(from) {
            self.append(to, c);
        }
    }

    /// DOM "clone" with subtree: a detached deep copy of `n` (template contents included).
    pub fn clone_subtree(&mut self, n: NodeId) -> NodeId {
        let data = self.nodes[n.0].data.clone();
        let copy = match data {
            NodeData::Element(e) => {
                let c = self.create_element(e.ns, &e.local, e.attrs.clone());
                if let Some(el) = self.element_mut(c) {
                    el.html_integration_point = e.html_integration_point;
                }
                if let (Some(src), Some(dst)) = (e.template_contents, self.element(c).and_then(|x| x.template_contents)) {
                    let kids: Vec<NodeId> = self.children(src).collect();
                    for k in kids {
                        let kc = self.clone_subtree(k);
                        self.append(dst, kc);
                    }
                }
                c
            }
            other => self.create(other),
        };
        let kids: Vec<NodeId> = self.children(n).collect();
        for k in kids {
            let kc = self.clone_subtree(k);
            self.append(copy, kc);
        }
        copy
    }

    /// Is `a` an inclusive ancestor of `b`?
    pub fn is_inclusive_ancestor(&self, a: NodeId, b: NodeId) -> bool {
        let mut cur = Some(b);
        while let Some(c) = cur {
            if c == a {
                return true;
            }
            cur = self.parent(c);
        }
        false
    }

    // ---- traversal hooks for a selector engine (CSSCORE) ---------------------------------------------------

    /// The parent if it is an element.
    pub fn parent_element(&self, id: NodeId) -> Option<NodeId> {
        self.parent(id).filter(|&p| self.element(p).is_some())
    }
    /// The nearest preceding sibling that is an element.
    pub fn prev_sibling_element(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = self.prev_sibling(id);
        while let Some(c) = cur {
            if self.element(c).is_some() {
                return Some(c);
            }
            cur = self.prev_sibling(c);
        }
        None
    }
    /// The nearest following sibling that is an element.
    pub fn next_sibling_element(&self, id: NodeId) -> Option<NodeId> {
        let mut cur = self.next_sibling(id);
        while let Some(c) = cur {
            if self.element(c).is_some() {
                return Some(c);
            }
            cur = self.next_sibling(c);
        }
        None
    }
    /// Element children only.
    pub fn element_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.children(id).filter(move |&c| self.element(c).is_some())
    }
    /// `id` attribute.
    pub fn element_id(&self, id: NodeId) -> Option<&str> {
        self.element(id).and_then(|e| e.attr("id"))
    }
    /// Does the element's `class` attribute contain `class` (ASCII-whitespace separated)?
    pub fn has_class(&self, id: NodeId, class: &str) -> bool {
        self.element(id)
            .and_then(|e| e.attr("class"))
            .is_some_and(|v| v.split([' ', '\t', '\n', '\x0C', '\r']).any(|t| t == class))
    }
    /// `querySelector`-shaped: the first descendant element of `root` (tree order) matching `pred`.
    pub fn query_first(&self, root: NodeId, mut pred: impl FnMut(&Document, NodeId) -> bool) -> Option<NodeId> {
        self.descendants(root).find(|&n| self.element(n).is_some() && pred(self, n))
    }
    /// `querySelectorAll`-shaped: every descendant element of `root` matching `pred`, in tree order.
    pub fn query_all(&self, root: NodeId, mut pred: impl FnMut(&Document, NodeId) -> bool) -> Vec<NodeId> {
        self.descendants(root).filter(|&n| self.element(n).is_some() && pred(self, n)).collect()
    }
    /// Concatenated data of every Text descendant (DOM `textContent` for an element).
    pub fn text_content(&self, id: NodeId) -> String {
        let mut s = String::new();
        for n in self.descendants(id) {
            if let NodeData::Text(t) = self.data(n) {
                s.push_str(t);
            }
        }
        s
    }
}

/// Iterator over a node's children.
pub struct Children<'a> {
    doc: &'a Document,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let cur = self.next?;
        self.next = self.doc.next_sibling(cur);
        Some(cur)
    }
}

/// Iterator over a node's descendants in tree order.
pub struct Descendants<'a> {
    doc: &'a Document,
    root: NodeId,
    next: Option<NodeId>,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let cur = self.next?;
        self.next = if let Some(c) = self.doc.first_child(cur) {
            Some(c)
        } else {
            let mut n = cur;
            loop {
                if n == self.root {
                    break None;
                }
                if let Some(s) = self.doc.next_sibling(n) {
                    break Some(s);
                }
                match self.doc.parent(n) {
                    Some(p) if p != self.root => n = p,
                    _ => break None,
                }
            }
        };
        Some(cur)
    }
}
