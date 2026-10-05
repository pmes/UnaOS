//! Aether's document model (AETHERDOM, LEDGER SR49): a thin handle layer over UnaOS's own
//! `html_core` arena DOM (HTMLCORE, SR46).
//!
//! Every document is ONE `html_core::Document` arena behind an `Rc<RefCell<…>>`; a [`NodeRef`] is
//! that arena plus a `NodeId`. The method names and shapes are the ones Aether's layout, CSS
//! cascade, scripting bindings (boa) and media code already call (`as_element`, `attributes.borrow()`,
//! `children`, `parent`, `select`, `text_contents`, `append`, `detach`, …), so moving a caller off the
//! old tree is a path change, not a rewrite.
//!
//! Borrowing: a guard handed out here (`attributes.borrow()`, `as_text().borrow()`) holds a shared
//! borrow of the WHOLE arena, so it must be dropped before any mutation of the same document
//! (`append`, `detach`, `attributes.borrow_mut()`). Navigation (`children`, `descendants`, …)
//! snapshots ids and holds no borrow, so a caller may mutate while iterating.
//!
//! Parsing is `html_core::parse_document` / `parse_fragment` with the scripting flag ON (Aether runs
//! script, so `<noscript>` is raw text, as in a JS-enabled browser). Serialization is `html_core`'s
//! §13.3 serializer.

mod select;

pub use select::{El, Selectors};

use html_core::serialize::{self, SerializeOpts};
use html_core::{Document, Namespace, NodeId, ParseOpts};
use std::cell::{Ref, RefCell, RefMut};
use std::fmt;
use std::rc::Rc;

pub type Arena = Rc<RefCell<Document>>;

/// The options every Aether parse uses: scripting on (Aether executes script).
pub fn parse_opts() -> ParseOpts {
    ParseOpts { scripting: true, ..ParseOpts::default() }
}

fn ser_opts() -> SerializeOpts {
    SerializeOpts { scripting: true }
}

/// Parses a whole HTML document (WHATWG §13.2, `html_core`) and returns its Document node.
pub fn parse_html(html: &str) -> NodeRef {
    let doc = html_core::parse_document(html, parse_opts());
    NodeRef { doc: Rc::new(RefCell::new(doc)), id: Document::ROOT }
}

// ---------------------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------------------

/// An element's local name (owned snapshot; derefs to `str`).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct LocalName(pub String);

impl std::ops::Deref for LocalName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}
impl AsRef<str> for LocalName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for LocalName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl PartialEq<str> for LocalName {
    fn eq(&self, o: &str) -> bool {
        self.0 == o
    }
}
impl PartialEq<&str> for LocalName {
    fn eq(&self, o: &&str) -> bool {
        self.0 == *o
    }
}

/// A namespace URL (derefs to `str`; empty for "no namespace").
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Ns(pub &'static str);

impl std::ops::Deref for Ns {
    type Target = str;
    fn deref(&self) -> &str {
        self.0
    }
}
impl AsRef<str> for Ns {
    fn as_ref(&self) -> &str {
        self.0
    }
}

/// An element's qualified name.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct QualName {
    pub ns: Ns,
    pub local: LocalName,
}

/// An attribute's expanded name, borrowed from the arena while a guard is held.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExpandedName<'a> {
    pub ns: &'a str,
    pub local: &'a str,
}

pub use html_core::Attribute;

// ---------------------------------------------------------------------------------------------------
// Node handle
// ---------------------------------------------------------------------------------------------------

/// A node: its document arena plus its id. Cheap to clone; equality is identity.
#[derive(Clone)]
pub struct NodeRef {
    pub(crate) doc: Arena,
    pub(crate) id: NodeId,
}

impl PartialEq for NodeRef {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id && Rc::ptr_eq(&self.doc, &o.doc)
    }
}
impl Eq for NodeRef {}
impl std::hash::Hash for NodeRef {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        (Rc::as_ptr(&self.doc) as usize).hash(h);
        self.id.hash(h);
    }
}
impl fmt::Debug for NodeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.doc.borrow();
        match d.data(self.id) {
            html_core::NodeData::Element(e) => write!(f, "<{}>#{}", e.local, self.id.0),
            other => write!(f, "{:?}#{}", std::mem::discriminant(other), self.id.0),
        }
    }
}

/// `to_string()` is the node's HTML serialization: outer HTML for an element, the children for a
/// document or fragment, escaped data for text (§13.3, `html_core::serialize`).
impl fmt::Display for NodeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.doc.borrow();
        let s = match d.data(self.id) {
            html_core::NodeData::Document | html_core::NodeData::DocumentFragment => {
                serialize::inner_html(&d, self.id, ser_opts())
            }
            _ => serialize::outer_html(&d, self.id, ser_opts()),
        };
        f.write_str(&s)
    }
}

/// What a node is — an owned view, matched like the old tree's `NodeData`.
pub enum NodeData {
    Element(ElementData),
    Text(TextCell),
    Comment(TextCell),
    ProcessingInstruction(TextCell),
    Doctype(Doctype),
    Document(DocumentData),
    DocumentFragment,
}

/// Element payload: the qualified name (snapshot) and a live handle on its attributes.
#[derive(Clone, Debug)]
pub struct ElementData {
    pub name: QualName,
    pub attributes: AttrCell,
    /// `<template>` contents (a DocumentFragment in the same arena).
    pub template_contents: Option<NodeRef>,
}

/// A doctype (owned snapshot).
#[derive(Clone, Debug)]
pub struct Doctype {
    pub name: String,
    pub public_id: String,
    pub system_id: String,
}

/// The Document node's payload.
#[derive(Clone, Copy, Debug)]
pub struct DocumentData {
    quirks: html_core::QuirksMode,
}

impl DocumentData {
    pub fn quirks_mode(&self) -> html_core::QuirksMode {
        self.quirks
    }
}

/// A live handle on a Text/Comment node's data.
#[derive(Clone, Debug)]
pub struct TextCell {
    node: NodeRef,
}

impl TextCell {
    pub fn borrow(&self) -> Ref<'_, String> {
        Ref::map(self.node.doc.borrow(), |d| text_slot(d, self.node.id))
    }
    pub fn borrow_mut(&self) -> RefMut<'_, String> {
        RefMut::map(self.node.doc.borrow_mut(), |d| text_slot_mut(d, self.node.id))
    }
}

fn text_slot(d: &Document, id: NodeId) -> &String {
    match &d.nodes[id.0].data {
        html_core::NodeData::Text(s) | html_core::NodeData::Comment(s) => s,
        html_core::NodeData::ProcessingInstruction { data, .. } => data,
        _ => unreachable!("TextCell on a non-character-data node"),
    }
}
fn text_slot_mut(d: &mut Document, id: NodeId) -> &mut String {
    match &mut d.nodes[id.0].data {
        html_core::NodeData::Text(s) | html_core::NodeData::Comment(s) => s,
        html_core::NodeData::ProcessingInstruction { data, .. } => data,
        _ => unreachable!("TextCell on a non-character-data node"),
    }
}

fn ns_of(n: Namespace) -> Ns {
    Ns(n.url())
}

impl NodeRef {
    /// This node's arena id (stable for the document's life; ids are never reused).
    pub fn id(&self) -> NodeId {
        self.id
    }

    fn at(&self, id: NodeId) -> NodeRef {
        NodeRef { doc: self.doc.clone(), id }
    }

    fn nav(&self, f: impl FnOnce(&Document) -> Option<NodeId>) -> Option<NodeRef> {
        let id = f(&self.doc.borrow());
        id.map(|i| self.at(i))
    }

    /// Runs `f` over the arena (shared borrow) — the query hooks (`element_children`, `has_class`, …).
    pub fn with_doc<R>(&self, f: impl FnOnce(&Document, NodeId) -> R) -> R {
        f(&self.doc.borrow(), self.id)
    }

    /// The Document node of this node's arena.
    pub fn document_node(&self) -> NodeRef {
        self.at(Document::ROOT)
    }

    /// Same arena?
    pub fn same_document(&self, o: &NodeRef) -> bool {
        Rc::ptr_eq(&self.doc, &o.doc)
    }

    // ---- kind ---------------------------------------------------------------------------------

    pub fn data(&self) -> NodeData {
        let d = self.doc.borrow();
        match d.data(self.id) {
            html_core::NodeData::Element(_) => {
                drop(d);
                NodeData::Element(self.as_element().expect("element"))
            }
            html_core::NodeData::Text(_) => NodeData::Text(TextCell { node: self.clone() }),
            html_core::NodeData::Comment(_) => NodeData::Comment(TextCell { node: self.clone() }),
            html_core::NodeData::ProcessingInstruction { .. } => {
                NodeData::ProcessingInstruction(TextCell { node: self.clone() })
            }
            html_core::NodeData::Doctype { name, public_id, system_id } => NodeData::Doctype(Doctype {
                name: name.clone(),
                public_id: public_id.clone(),
                system_id: system_id.clone(),
            }),
            html_core::NodeData::Document => NodeData::Document(DocumentData { quirks: d.quirks_mode }),
            html_core::NodeData::DocumentFragment => NodeData::DocumentFragment,
        }
    }

    pub fn as_element(&self) -> Option<ElementData> {
        let d = self.doc.borrow();
        let e = d.element(self.id)?;
        Some(ElementData {
            name: QualName { ns: ns_of(e.ns), local: LocalName(e.local.clone()) },
            attributes: AttrCell { node: self.clone() },
            template_contents: e.template_contents.map(|t| self.at(t)),
        })
    }

    /// Element? (no snapshot)
    pub fn is_element(&self) -> bool {
        self.doc.borrow().element(self.id).is_some()
    }

    /// The element's local name, if an element.
    pub fn local_name(&self) -> Option<String> {
        self.doc.borrow().element(self.id).map(|e| e.local.clone())
    }

    pub fn as_text(&self) -> Option<TextCell> {
        matches!(self.doc.borrow().data(self.id), html_core::NodeData::Text(_))
            .then(|| TextCell { node: self.clone() })
    }

    pub fn as_comment(&self) -> Option<TextCell> {
        matches!(self.doc.borrow().data(self.id), html_core::NodeData::Comment(_))
            .then(|| TextCell { node: self.clone() })
    }

    pub fn as_doctype(&self) -> Option<Doctype> {
        match self.doc.borrow().data(self.id) {
            html_core::NodeData::Doctype { name, public_id, system_id } => Some(Doctype {
                name: name.clone(),
                public_id: public_id.clone(),
                system_id: system_id.clone(),
            }),
            _ => None,
        }
    }

    pub fn as_document(&self) -> Option<DocumentData> {
        let d = self.doc.borrow();
        matches!(d.data(self.id), html_core::NodeData::Document)
            .then_some(DocumentData { quirks: d.quirks_mode })
    }

    pub fn into_element_ref(self) -> Option<NodeDataRef<ElementData>> {
        let data = self.as_element()?;
        Some(NodeDataRef { node: self, data })
    }

    // ---- navigation ---------------------------------------------------------------------------

    pub fn parent(&self) -> Option<NodeRef> {
        self.nav(|d| d.parent(self.id))
    }
    pub fn first_child(&self) -> Option<NodeRef> {
        self.nav(|d| d.first_child(self.id))
    }
    pub fn last_child(&self) -> Option<NodeRef> {
        self.nav(|d| d.last_child(self.id))
    }
    pub fn next_sibling(&self) -> Option<NodeRef> {
        self.nav(|d| d.next_sibling(self.id))
    }
    pub fn previous_sibling(&self) -> Option<NodeRef> {
        self.nav(|d| d.prev_sibling(self.id))
    }

    fn snapshot(&self, ids: Vec<NodeId>) -> std::vec::IntoIter<NodeRef> {
        ids.into_iter().map(|i| self.at(i)).collect::<Vec<_>>().into_iter()
    }

    /// Children, in order (a snapshot: safe to mutate while iterating).
    pub fn children(&self) -> std::vec::IntoIter<NodeRef> {
        let ids: Vec<NodeId> = self.doc.borrow().children(self.id).collect();
        self.snapshot(ids)
    }

    /// Descendants in tree order, excluding `self`.
    pub fn descendants(&self) -> std::vec::IntoIter<NodeRef> {
        let ids: Vec<NodeId> = self.doc.borrow().descendants(self.id).collect();
        self.snapshot(ids)
    }

    /// `self` then its descendants in tree order.
    pub fn inclusive_descendants(&self) -> std::vec::IntoIter<NodeRef> {
        let d = self.doc.borrow();
        let ids: Vec<NodeId> = std::iter::once(self.id).chain(d.descendants(self.id)).collect();
        drop(d);
        self.snapshot(ids)
    }

    /// Ancestors, nearest first, excluding `self`.
    pub fn ancestors(&self) -> std::vec::IntoIter<NodeRef> {
        let d = self.doc.borrow();
        let mut ids = Vec::new();
        let mut cur = d.parent(self.id);
        while let Some(c) = cur {
            ids.push(c);
            cur = d.parent(c);
        }
        drop(d);
        self.snapshot(ids)
    }

    /// Concatenated text of every Text node in the inclusive subtree.
    pub fn text_contents(&self) -> String {
        let d = self.doc.borrow();
        match d.data(self.id) {
            html_core::NodeData::Text(t) => t.clone(),
            _ => d.text_content(self.id),
        }
    }

    // ---- query (selectors over the arena) -----------------------------------------------------

    /// Elements in the inclusive subtree matching a CSS selector list, in tree order. `Err` when the
    /// list does not compile.
    pub fn select(&self, selectors: &str) -> Result<std::vec::IntoIter<NodeDataRef<ElementData>>, ()> {
        let sels = Selectors::compile(selectors)?;
        let ids: Vec<NodeId> = {
            let d = self.doc.borrow();
            let scope = d.element(self.id).map(|_| self.id);
            std::iter::once(self.id)
                .chain(d.descendants(self.id))
                .filter(|&id| sels.matches_in(&d, id, scope))
                .collect()
        };
        let out: Vec<NodeDataRef<ElementData>> = ids
            .into_iter()
            .filter_map(|id| NodeRef { doc: self.doc.clone(), id }.into_element_ref())
            .collect();
        Ok(out.into_iter())
    }

    pub fn select_first(&self, selectors: &str) -> Result<NodeDataRef<ElementData>, ()> {
        self.select(selectors)?.next().ok_or(())
    }

    // ---- construction -------------------------------------------------------------------------

    /// A detached Text node in this node's document.
    pub fn new_text<T: Into<String>>(&self, text: T) -> NodeRef {
        let id = self.doc.borrow_mut().create(html_core::NodeData::Text(text.into()));
        self.at(id)
    }

    /// A detached Comment node in this node's document.
    pub fn new_comment<T: Into<String>>(&self, text: T) -> NodeRef {
        let id = self.doc.borrow_mut().create(html_core::NodeData::Comment(text.into()));
        self.at(id)
    }

    /// A detached element in this node's document (`createElement`/`createElementNS`).
    pub fn new_element(&self, ns: Namespace, local: &str) -> NodeRef {
        let id = self.doc.borrow_mut().create_element(ns, local, Vec::new());
        self.at(id)
    }

    // ---- mutation -----------------------------------------------------------------------------

    /// Brings `node` into this arena: itself when it already lives here, else a deep copy (the
    /// original is detached from its own tree, as a DOM adopt would move it).
    fn adopt(&self, node: NodeRef) -> NodeId {
        if Rc::ptr_eq(&self.doc, &node.doc) {
            return node.id;
        }
        let id = {
            let src = node.doc.borrow();
            let mut dst = self.doc.borrow_mut();
            import_subtree(&mut dst, &src, node.id)
        };
        node.detach();
        id
    }

    /// Would inserting `child` under `self` create a cycle (or insert the document)?
    fn bad_child(&self, child: &NodeRef) -> bool {
        if !Rc::ptr_eq(&self.doc, &child.doc) {
            return false;
        }
        let d = self.doc.borrow();
        child.id == Document::ROOT || d.is_inclusive_ancestor(child.id, self.id)
    }

    /// Append `child` as the last child (moving it from wherever it was). A DocumentFragment
    /// contributes its children. Inserting an ancestor of `self` is refused (HierarchyRequestError).
    pub fn append(&self, child: NodeRef) {
        if self.bad_child(&child) {
            return;
        }
        let c = self.adopt(child);
        let mut d = self.doc.borrow_mut();
        if matches!(d.data(c), html_core::NodeData::DocumentFragment) {
            d.reparent_children(c, self.id);
        } else {
            d.append(self.id, c);
        }
    }

    /// Insert `child` as the first child.
    pub fn prepend(&self, child: NodeRef) {
        match self.first_child() {
            Some(f) => f.insert_before(child),
            None => self.append(child),
        }
    }

    /// Insert `new_sibling` immediately before `self` (in `self`'s parent).
    pub fn insert_before(&self, new_sibling: NodeRef) {
        let Some(parent) = self.parent() else { return };
        if parent.bad_child(&new_sibling) || new_sibling == *self {
            return;
        }
        let c = self.adopt(new_sibling);
        let mut d = self.doc.borrow_mut();
        if matches!(d.data(c), html_core::NodeData::DocumentFragment) {
            let kids: Vec<NodeId> = d.children(c).collect();
            for k in kids {
                d.insert_before(parent.id, k, Some(self.id));
            }
        } else {
            d.insert_before(parent.id, c, Some(self.id));
        }
    }

    /// Insert `new_sibling` immediately after `self`.
    pub fn insert_after(&self, new_sibling: NodeRef) {
        match self.next_sibling() {
            Some(n) => n.insert_before(new_sibling),
            None => {
                if let Some(p) = self.parent() {
                    p.append(new_sibling)
                }
            }
        }
    }

    /// DOM "clone a node": a detached copy in the same document; `deep` copies the subtree (and
    /// template contents), shallow copies the node alone.
    pub fn clone_node(&self, deep: bool) -> NodeRef {
        let mut d = self.doc.borrow_mut();
        let id = if deep {
            d.clone_subtree(self.id)
        } else {
            match d.data(self.id).clone() {
                html_core::NodeData::Element(e) => d.create_element(e.ns, &e.local, e.attrs),
                html_core::NodeData::Document => d.create(html_core::NodeData::DocumentFragment),
                other => d.create(other),
            }
        };
        drop(d);
        self.at(id)
    }

    /// Remove from the parent (no-op when detached). The node stays valid and re-insertable.
    pub fn detach(&self) {
        self.doc.borrow_mut().detach(self.id);
    }

    /// Replace every child with the result of parsing `html` as a fragment in this element's
    /// context (WHATWG §13.4, the `innerHTML` setter). Non-elements parse in a `body` context.
    pub fn set_inner_html(&self, html: &str) {
        for c in self.children() {
            c.detach();
        }
        let frag = self.parse_fragment_here(html);
        self.append(frag);
    }

    /// Parses `html` as a fragment in this element's context; returns a detached DocumentFragment
    /// (in this document's arena) holding the result.
    pub fn parse_fragment_here(&self, html: &str) -> NodeRef {
        let (ns, local, attrs, quirks) = {
            let d = self.doc.borrow();
            match d.element(self.id) {
                Some(e) => (e.ns, e.local.clone(), e.attrs.clone(), d.quirks_mode),
                None => (Namespace::Html, "body".to_string(), Vec::new(), d.quirks_mode),
            }
        };
        let (fdoc, froot) = html_core::parse_fragment(ns, &local, attrs, html, quirks, parse_opts());
        let id = import_subtree(&mut self.doc.borrow_mut(), &fdoc, froot);
        self.at(id)
    }

    /// `innerHTML` getter (§13.3 fragment serialization).
    pub fn inner_html(&self) -> String {
        serialize::inner_html(&self.doc.borrow(), self.id, ser_opts())
    }

    /// `outerHTML` getter.
    pub fn outer_html(&self) -> String {
        serialize::outer_html(&self.doc.borrow(), self.id, ser_opts())
    }
}

/// Deep-copies `src`'s subtree at `id` into `dst` (template contents included); returns the copy.
fn import_subtree(dst: &mut Document, src: &Document, id: NodeId) -> NodeId {
    let copy = match src.data(id) {
        html_core::NodeData::Element(e) => {
            let c = dst.create_element(e.ns, &e.local, e.attrs.clone());
            if let Some(el) = dst.element_mut(c) {
                el.html_integration_point = e.html_integration_point;
            }
            if let (Some(s), Some(t)) = (e.template_contents, dst.element(c).and_then(|x| x.template_contents)) {
                let kids: Vec<NodeId> = src.children(s).collect();
                for k in kids {
                    let kc = import_subtree(dst, src, k);
                    dst.append(t, kc);
                }
            }
            c
        }
        html_core::NodeData::Document => dst.create(html_core::NodeData::DocumentFragment),
        other => dst.create(other.clone()),
    };
    let kids: Vec<NodeId> = src.children(id).collect();
    for k in kids {
        let kc = import_subtree(dst, src, k);
        dst.append(copy, kc);
    }
    copy
}

// ---------------------------------------------------------------------------------------------------
// Element refs and attributes
// ---------------------------------------------------------------------------------------------------

/// A node known to carry `T` (only `ElementData` is used): derefs to the payload, `as_node()` gives
/// the node back.
#[derive(Clone, Debug)]
pub struct NodeDataRef<T> {
    node: NodeRef,
    data: T,
}

impl<T> NodeDataRef<T> {
    pub fn as_node(&self) -> &NodeRef {
        &self.node
    }
}

impl<T> std::ops::Deref for NodeDataRef<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.data
    }
}

/// Live handle on an element's attribute list.
#[derive(Clone, Debug)]
pub struct AttrCell {
    node: NodeRef,
}

impl AttrCell {
    pub fn borrow(&self) -> Attributes<'_> {
        let id = self.node.id;
        Attributes {
            map: AttrMap(Ref::map(self.node.doc.borrow(), |d| &d.element(id).expect("element").attrs)),
        }
    }
    pub fn borrow_mut(&self) -> AttributesMut<'_> {
        let id = self.node.id;
        AttributesMut(RefMut::map(self.node.doc.borrow_mut(), |d| {
            &mut d.element_mut(id).expect("element").attrs
        }))
    }
}

/// The attribute list, read-only. `map.iter()` yields `(ExpandedName, &Attribute)` in source order.
pub struct Attributes<'a> {
    pub map: AttrMap<'a>,
}

pub struct AttrMap<'a>(Ref<'a, Vec<Attribute>>);

impl AttrMap<'_> {
    pub fn iter(&self) -> impl Iterator<Item = (ExpandedName<'_>, &Attribute)> {
        self.0.iter().map(|a| (ExpandedName { ns: a.ns.url(), local: a.local.as_str() }, a))
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn find<'v>(v: &'v [Attribute], name: &str) -> Option<&'v Attribute> {
    v.iter().find(|a| a.ns == Namespace::None && a.local == name)
}

impl Attributes<'_> {
    /// Value of the no-namespace attribute `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        find(&self.map.0, name).map(|a| a.value.as_str())
    }
    pub fn contains(&self, name: &str) -> bool {
        find(&self.map.0, name).is_some()
    }
}

/// The attribute list, mutable (holds the arena's exclusive borrow).
pub struct AttributesMut<'a>(RefMut<'a, Vec<Attribute>>);

impl AttributesMut<'_> {
    pub fn get(&self, name: &str) -> Option<&str> {
        find(&self.0, name).map(|a| a.value.as_str())
    }
    pub fn contains(&self, name: &str) -> bool {
        find(&self.0, name).is_some()
    }
    /// Sets the no-namespace attribute `name` (appended when new, in place when present); returns the
    /// previous attribute.
    pub fn insert<V: Into<String>>(&mut self, name: &str, value: V) -> Option<Attribute> {
        let value = value.into();
        if let Some(a) = self.0.iter_mut().find(|a| a.ns == Namespace::None && a.local == name) {
            let old = a.clone();
            a.value = value;
            return Some(old);
        }
        self.0.push(Attribute { prefix: None, ns: Namespace::None, local: name.to_string(), value });
        None
    }
    pub fn remove(&mut self, name: &str) -> Option<Attribute> {
        let i = self.0.iter().position(|a| a.ns == Namespace::None && a.local == name)?;
        Some(self.0.remove(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_navigate_mutate_serialize() {
        let doc = parse_html("<!DOCTYPE html><p id=a class='x y'>one<b>two</b></p><p>three");
        let p = doc.select_first("p.x").unwrap();
        assert_eq!(&*p.name.local, "p");
        assert_eq!(p.attributes.borrow().get("id"), Some("a"));
        assert_eq!(p.as_node().text_contents(), "onetwo");
        assert_eq!(doc.select("p").unwrap().count(), 2);
        p.attributes.borrow_mut().insert("data-k", "v");
        let t = p.as_node().new_text("!");
        p.as_node().append(t);
        assert_eq!(p.as_node().to_string(), r#"<p id="a" class="x y" data-k="v">one<b>two</b>!</p>"#);
        let b = doc.select_first("b").unwrap().as_node().clone();
        b.detach();
        assert_eq!(p.as_node().inner_html(), "one!");
        // an ancestor cannot be appended under its descendant
        let body = doc.select_first("body").unwrap().as_node().clone();
        p.as_node().append(body.clone());
        assert_eq!(body.parent().and_then(|h| h.local_name()), Some("html".into()));
    }

    #[test]
    fn inner_html_is_a_context_fragment_parse() {
        let doc = parse_html("<table><tbody><tr><td>x</td></tr></tbody></table>");
        let tbody = doc.select_first("tbody").unwrap().as_node().clone();
        tbody.set_inner_html("<tr><td>a</td><td>b</td></tr>");
        assert_eq!(tbody.inner_html(), "<tr><td>a</td><td>b</td></tr>");
        let body = doc.select_first("body").unwrap().as_node().clone();
        body.set_inner_html("<div><noscript><i>raw</i></noscript></div>");
        // scripting on: noscript content is raw text
        assert_eq!(doc.select("i").unwrap().count(), 0);
    }

    #[test]
    fn cross_document_append_imports() {
        let a = parse_html("<div id=t></div>");
        let b = parse_html("<span>s</span>");
        let t = a.select_first("#t").unwrap().as_node().clone();
        let s = b.select_first("span").unwrap().as_node().clone();
        t.append(s);
        assert_eq!(t.inner_html(), "<span>s</span>");
        assert_eq!(b.select("span").unwrap().count(), 0);
    }

    #[test]
    fn selectors_match_like_the_engine() {
        let doc = parse_html(
            "<ul><li class=a>1</li><li>2</li><li class='b c'>3</li></ul><a href=x>l</a><a>n</a>\
             <input disabled><svg><rect/></svg><p></p>",
        );
        let n = |s: &str| doc.select(s).unwrap().count();
        assert_eq!(n("li:nth-child(even)"), 1);
        assert_eq!(n("ul > li.b.c"), 1);
        assert_eq!(n("li + li"), 2);
        assert_eq!(n("li ~ .b"), 1);
        assert_eq!(n("a:link"), 1);
        assert_eq!(n("a:any-link"), 1);
        assert_eq!(n("[href]"), 1);
        assert_eq!(n("[class~=c]"), 1);
        assert_eq!(n("li:not(.a)"), 2);
        assert_eq!(n("p:empty"), 1);
        assert_eq!(n(":root"), 1);
        assert_eq!(n("rect"), 1);
        assert_eq!(n("LI"), 3);
        assert_eq!(n("li:first-child, li:last-child"), 2);
        assert_eq!(n("input:hover"), 0);
        // css_core (AETHERSTYLE): Selectors 4 compiles; pseudo-elements parse and never match an element.
        assert_eq!(n("li::before"), 0);
        assert_eq!(n("li:focus-visible"), 0);
        assert_eq!(n("li:is(.a, .b)"), 2);
        assert_eq!(n("li:where(.c)"), 1);
        assert_eq!(n("ul:has(> li.b)"), 1);
        assert_eq!(n("li:nth-child(odd of :not(.a))"), 1);
        assert_eq!(n("input:disabled"), 1);
        assert_eq!(n("input:enabled"), 0);
        assert_eq!(n("li:not(.a, .b)"), 1);
        assert!(doc.select("li:::x").is_err());
        assert!(doc.select("").is_err());
        let s = Selectors::compile("#x, .y, z").unwrap();
        assert!(s.0.0[0].specificity > s.0.0[1].specificity);
        assert!(s.0.0[1].specificity > s.0.0[2].specificity);
    }
}
