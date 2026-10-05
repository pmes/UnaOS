//! The DOM Standard over html_core's arena: Node and its family, the mutation algorithms (DOM §4.2.3),
//! live collections, DOMTokenList, Attr/NamedNodeMap and DOMImplementation.
//!
//! A node's JavaScript wrapper is a platform object (`idl::T_NODE`) holding the node's arena id; one
//! wrapper per node for the page's life (`a.parentNode === a.parentNode`), created on first touch with
//! the prototype of the interface the node is (HTMLAnchorElement for `<a>`, Text, Comment, …).
//!
//! Every document a page owns lives in the ONE arena of its main document: `DOMParser` results and
//! `createHTMLDocument` documents are extra Document nodes in it, with their own metadata
//! (`PageState::docs`) and a node-document map for nodes they own. Moving a node between documents is
//! therefore DOM adoption (a relink plus a node-document update), and a wrapper keeps its identity.
//!
//! NodeList, HTMLCollection, NamedNodeMap and DOMTokenList are legacy platform objects with indexed
//! (and, for HTMLCollection/NamedNodeMap, named) properties. Each is a Proxy whose target is a branded
//! platform object describing the collection (kind, root, filter) and whose traps compute the live
//! member list from the arena on every access — `childNodes` sees an `appendChild` made a line earlier.

use super::idl::*;
use super::{page, touch};
use crate::dom::{Arena, NodeRef, Selectors};
use html_core::{Attribute, Document, Namespace, NodeData, NodeId};
use js_core::string::JsStr;
use js_core::vm::*;
use std::collections::HashSet;

// =================================================================================================
// Arena access
// =================================================================================================

pub(crate) fn arena() -> Arena {
    page(|p| p.doc.as_ref().map(|d| d.arena().clone())).expect("page has a document")
}

pub(crate) fn with_doc<R>(f: impl FnOnce(&Document) -> R) -> R {
    let a = arena();
    let d = a.borrow();
    f(&d)
}

pub(crate) fn with_doc_mut<R>(f: impl FnOnce(&mut Document) -> R) -> R {
    let a = arena();
    let mut d = a.borrow_mut();
    f(&mut d)
}

pub(crate) fn node_ref(id: usize) -> NodeRef {
    NodeRef::from_arena(arena(), NodeId(id))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NK {
    Document,
    Fragment,
    Doctype,
    Element,
    Text,
    Comment,
    Pi,
}

pub fn kind_in(d: &Document, id: usize) -> NK {
    match d.data(NodeId(id)) {
        NodeData::Document => NK::Document,
        NodeData::DocumentFragment => NK::Fragment,
        NodeData::Doctype { .. } => NK::Doctype,
        NodeData::Element(_) => NK::Element,
        NodeData::Text(_) => NK::Text,
        NodeData::Comment(_) => NK::Comment,
        NodeData::ProcessingInstruction { .. } => NK::Pi,
    }
}

pub fn kind(id: usize) -> NK {
    with_doc(|d| kind_in(d, id))
}

fn is_char_data(k: NK) -> bool {
    matches!(k, NK::Text | NK::Comment | NK::Pi)
}

pub fn parent_of(id: usize) -> Option<usize> {
    with_doc(|d| d.parent(NodeId(id)).map(|p| p.0))
}

pub fn children_of(id: usize) -> Vec<usize> {
    with_doc(|d| d.children(NodeId(id)).map(|c| c.0).collect())
}

/// The root of `id`'s tree (DOM "root").
pub fn root_of(id: usize) -> usize {
    with_doc(|d| {
        let mut cur = NodeId(id);
        while let Some(p) = d.parent(cur) {
            cur = p;
        }
        cur.0
    })
}

/// Connected: in the main document's tree (the only document with a browsing context).
pub fn is_connected(id: usize) -> bool {
    root_of(id) == main_doc()
}

pub fn main_doc() -> usize {
    page(|p| p.doc.as_ref().map(|d| d.id().0)).unwrap_or(0)
}

/// The node document of `id` (DOM §4.4): the document a node belongs to.
pub fn node_document(id: usize) -> usize {
    if kind(id) == NK::Document {
        return id;
    }
    page(|p| p.node_doc.get(&id).copied()).unwrap_or_else(main_doc)
}

/// Is `doc` an HTML document (vs an XML document from `createDocument`)?
pub fn is_html_doc(doc: usize) -> bool {
    page(|p| p.docs.get(&doc).map(|m| m.html)).unwrap_or(true)
}

/// Element namespace and prefix (the side table carries what html_core's closed namespace set cannot).
pub fn element_ns(d: &Document, id: usize) -> (String, Option<String>) {
    if let Some((ns, prefix)) = page(|p| p.ns_override.get(&id).cloned()) {
        return (ns, prefix);
    }
    match d.element(NodeId(id)) {
        Some(e) => (e.ns.url().to_string(), None),
        None => (String::new(), None),
    }
}

pub fn local_name_in(d: &Document, id: usize) -> String {
    d.element(NodeId(id)).map(|e| e.local.clone()).unwrap_or_default()
}

/// The element's qualified name (prefix:local or local).
pub fn qualified_name(d: &Document, id: usize) -> String {
    let (_, prefix) = element_ns(d, id);
    let local = local_name_in(d, id);
    match prefix {
        Some(p) => format!("{p}:{local}"),
        None => local,
    }
}

/// HTML-uppercased qualified name (Element.tagName).
pub fn tag_name(id: usize) -> String {
    with_doc(|d| {
        let q = qualified_name(d, id);
        let html_ns = d.element(NodeId(id)).is_some_and(|e| e.ns == Namespace::Html) && page(|p| !p.ns_override.contains_key(&id));
        if html_ns && is_html_doc(node_document(id)) { q.to_ascii_uppercase() } else { q }
    })
}

pub fn is_html_element(d: &Document, id: usize) -> bool {
    d.element(NodeId(id)).is_some_and(|e| e.ns == Namespace::Html) && page(|p| !p.ns_override.contains_key(&id))
}

pub fn is_html_tag(d: &Document, id: usize, tag: &str) -> bool {
    is_html_element(d, id) && d.element(NodeId(id)).is_some_and(|e| e.local == tag)
}

pub fn attr_value(id: usize, name: &str) -> Option<String> {
    with_doc(|d| d.element(NodeId(id)).and_then(|e| e.attr(name)).map(str::to_string))
}

/// Text of a CharacterData node.
pub fn char_data(d: &Document, id: usize) -> String {
    match d.data(NodeId(id)) {
        NodeData::Text(t) | NodeData::Comment(t) => t.clone(),
        NodeData::ProcessingInstruction { data, .. } => data.clone(),
        _ => String::new(),
    }
}

fn set_char_data(d: &mut Document, id: usize, s: String) {
    match &mut d.node_mut(NodeId(id)).data {
        NodeData::Text(t) | NodeData::Comment(t) => *t = s,
        NodeData::ProcessingInstruction { data, .. } => *data = s,
        _ => {}
    }
}

/// DOM "descendant text content".
pub fn text_content_of(d: &Document, id: usize) -> String {
    d.text_content(NodeId(id))
}

thread_local! {
    static INTERNED: std::cell::RefCell<HashSet<&'static str>> = std::cell::RefCell::new(HashSet::new());
}

/// A `'static` copy of an attribute prefix (html_core stores prefixes as `&'static str`); bounded by the
/// number of distinct prefixes a page uses.
fn intern(s: &str) -> &'static str {
    INTERNED.with(|i| {
        let mut i = i.borrow_mut();
        if let Some(v) = i.get(s) {
            return *v;
        }
        let v: &'static str = Box::leak(s.to_string().into_boxed_str());
        i.insert(v);
        v
    })
}

fn ns_from_url(url: &str) -> Option<Namespace> {
    Some(match url {
        "http://www.w3.org/1999/xhtml" => Namespace::Html,
        "http://www.w3.org/2000/svg" => Namespace::Svg,
        "http://www.w3.org/1998/Math/MathML" => Namespace::MathMl,
        "http://www.w3.org/1999/xlink" => Namespace::XLink,
        "http://www.w3.org/XML/1998/namespace" => Namespace::Xml,
        "http://www.w3.org/2000/xmlns/" => Namespace::Xmlns,
        "" => Namespace::None,
        _ => return None,
    })
}

// =================================================================================================
// Wrappers
// =================================================================================================

/// The interface a node presents.
pub fn interface_name(id: usize) -> &'static str {
    with_doc(|d| match d.data(NodeId(id)) {
        NodeData::Document => {
            if is_html_doc(id) {
                "HTMLDocument"
            } else {
                "XMLDocument"
            }
        }
        NodeData::DocumentFragment => "DocumentFragment",
        NodeData::Doctype { .. } => "DocumentType",
        NodeData::Text(_) => "Text",
        NodeData::Comment(_) => "Comment",
        NodeData::ProcessingInstruction { .. } => "ProcessingInstruction",
        NodeData::Element(e) => {
            if page(|p| p.ns_override.contains_key(&id)) {
                return "Element";
            }
            match e.ns {
                Namespace::Html => super::html::interface_for_tag(&e.local),
                Namespace::Svg => {
                    if e.local == "svg" {
                        "SVGSVGElement"
                    } else {
                        "SVGElement"
                    }
                }
                Namespace::MathMl => "MathMLElement",
                _ => "Element",
            }
        }
    })
}

pub fn iface(name: &str) -> Option<Iface> {
    page(|p| p.ifaces.get(name).copied())
}

pub fn proto(name: &str) -> Obj {
    for n in [name, "HTMLUnknownElement", "HTMLElement", "Element", "Node"] {
        if let Some(i) = iface(n) {
            return i.proto;
        }
    }
    unreachable!("Node interface installed")
}

pub(crate) fn register_iface(name: &'static str, i: Iface) {
    page(|p| p.ifaces.insert(name, i));
}

/// The wrapper of node `id` (created on first touch, then the same object forever).
pub fn wrap(vm: &mut Vm, id: usize) -> Value {
    if let Some(o) = page(|p| p.wrappers.get(&id).map(|w| w.0)) {
        return Value::Object(o);
    }
    let p = proto(interface_name(id));
    let o = host_obj(vm, p, T_NODE, vec![num(id as f64)]);
    let r = root(vm, Value::Object(o));
    page(|p| p.wrappers.insert(id, (o, r)));
    Value::Object(o)
}

pub fn wrap_opt(vm: &mut Vm, id: Option<usize>) -> Value {
    match id {
        Some(i) => wrap(vm, i),
        None => Value::Null,
    }
}

/// The node id behind a wrapper value.
pub fn node_of(vm: &Vm, v: &Value) -> Option<usize> {
    let o = v.as_object()?;
    if tag_of(vm, o) != Some(T_NODE) {
        return None;
    }
    let n = slot_num(vm, o, 0);
    n.is_finite().then_some(n as usize)
}

/// `this` as a node (else "Illegal invocation").
pub fn this_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    match node_of(vm, &ctx.this) {
        Some(n) => Ok(n),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn this_kind(vm: &mut Vm, ctx: &CallCtx, ok: &[NK]) -> JsResult<usize> {
    let n = this_node(vm, ctx)?;
    if ok.contains(&kind(n)) {
        Ok(n)
    } else {
        vm.throw_type("Illegal invocation")
    }
}

pub fn this_element(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    this_kind(vm, ctx, &[NK::Element])
}

/// A `Node` argument (TypeError "parameter n is not of type 'Node'" otherwise).
pub fn node_arg(vm: &mut Vm, ctx: &CallCtx, i: usize, iface: &str, method: &str) -> JsResult<usize> {
    let v = arg(vm, ctx, i);
    match node_of(vm, &v) {
        Some(n) => Ok(n),
        None => vm.throw_type(&format!(
            "Failed to execute '{method}' on '{iface}': parameter {} is not of type 'Node'.",
            i + 1
        )),
    }
}

/// A `Node?` argument.
fn opt_node_arg(vm: &mut Vm, ctx: &CallCtx, i: usize, iface: &str, method: &str) -> JsResult<Option<usize>> {
    let v = arg(vm, ctx, i);
    if v.is_nullish() {
        return Ok(None);
    }
    node_arg(vm, ctx, i, iface, method).map(Some)
}

/// Per-node cached platform objects (childNodes, classList, style, …) for `[SameObject]` identity.
pub fn cached(vm: &mut Vm, id: usize, which: u8, make: impl FnOnce(&mut Vm) -> Obj) -> Value {
    if let Some(o) = page(|p| p.same_object.get(&(id, which)).map(|x| x.0)) {
        return Value::Object(o);
    }
    let o = make(vm);
    let r = root(vm, Value::Object(o));
    page(|p| p.same_object.insert((id, which), (o, r)));
    Value::Object(o)
}

pub const SO_CHILDNODES: u8 = 1;
pub const SO_CHILDREN: u8 = 2;
pub const SO_CLASSLIST: u8 = 3;
pub const SO_ATTRIBUTES: u8 = 4;
pub const SO_STYLE: u8 = 5;
pub const SO_DATASET: u8 = 6;
pub const SO_RELLIST: u8 = 7;
pub const SO_FORMS: u8 = 8;
pub const SO_IMAGES: u8 = 9;
pub const SO_LINKS: u8 = 10;
pub const SO_SCRIPTS: u8 = 11;
pub const SO_IMPL: u8 = 12;
pub const SO_ANCHORS: u8 = 13;
pub const SO_EMBEDS: u8 = 14;

// =================================================================================================
// Creation
// =================================================================================================

/// Records that `id` belongs to document `doc`.
pub fn set_node_doc(id: usize, doc: usize) {
    let main = main_doc();
    page(|p| {
        if doc == main {
            p.node_doc.remove(&id);
        } else {
            p.node_doc.insert(id, doc);
        }
    });
}

pub fn create_element(doc: usize, ns: Namespace, local: &str) -> usize {
    let id = with_doc_mut(|d| d.create_element(ns, local, Vec::new()).0);
    set_node_doc(id, doc);
    if let Some(t) = with_doc(|d| d.element(NodeId(id)).and_then(|e| e.template_contents)) {
        set_node_doc(t.0, doc);
    }
    id
}

pub fn create_text(doc: usize, s: String) -> usize {
    let id = with_doc_mut(|d| d.create(NodeData::Text(s)).0);
    set_node_doc(id, doc);
    id
}

pub fn create_comment(doc: usize, s: String) -> usize {
    let id = with_doc_mut(|d| d.create(NodeData::Comment(s)).0);
    set_node_doc(id, doc);
    id
}

/// DOM "adopt" (§4.5): `node` (removed from its parent) and its shadow-including descendants move to
/// `doc`.
pub fn adopt(node: usize, doc: usize) {
    if parent_of(node).is_some() {
        with_doc_mut(|d| d.detach(NodeId(node)));
    }
    if node_document(node) == doc && kind(node) != NK::Document {
        return;
    }
    let ids: Vec<usize> = with_doc(|d| {
        let mut v = vec![node];
        v.extend(d.descendants(NodeId(node)).map(|n| n.0));
        // template contents ride with their element.
        let mut extra = Vec::new();
        for &i in &v {
            if let Some(t) = d.element(NodeId(i)).and_then(|e| e.template_contents) {
                extra.push(t.0);
                extra.extend(d.descendants(t).map(|n| n.0));
            }
        }
        v.extend(extra);
        v
    });
    for i in ids {
        set_node_doc(i, doc);
    }
}

/// DOM "clone a node" (§4.4) into document `doc`.
pub fn clone_node(id: usize, deep: bool, doc: Option<usize>) -> usize {
    let doc = doc.unwrap_or_else(|| node_document(id));
    let copy = with_doc_mut(|d| {
        if deep {
            d.clone_subtree(NodeId(id)).0
        } else {
            let data = d.data(NodeId(id)).clone();
            match data {
                NodeData::Element(e) => {
                    let c = d.create_element(e.ns, &e.local, e.attrs.clone());
                    c.0
                }
                other => d.create(other).0,
            }
        }
    });
    // Node documents, namespace overrides, script "already started" and form-control state copy over.
    let pairs: Vec<(usize, usize)> = with_doc(|d| {
        let a: Vec<usize> = std::iter::once(id).chain(if deep { d.descendants(NodeId(id)).map(|n| n.0).collect::<Vec<_>>() } else { Vec::new() }).collect();
        let b: Vec<usize> = std::iter::once(copy).chain(if deep { d.descendants(NodeId(copy)).map(|n| n.0).collect::<Vec<_>>() } else { Vec::new() }).collect();
        a.into_iter().zip(b).collect()
    });
    for (src, dst) in pairs {
        set_node_doc(dst, doc);
        page(|p| {
            if let Some(o) = p.ns_override.get(&src).cloned() {
                p.ns_override.insert(dst, o);
            }
            if p.started.contains(&src) {
                p.started.insert(dst);
            }
            if let Some(v) = p.values.get(&src).cloned() {
                p.values.insert(dst, v);
            }
            if let Some(c) = p.checked.get(&src).copied() {
                p.checked.insert(dst, c);
            }
        });
        if let Some(t) = with_doc(|d| d.element(NodeId(dst)).and_then(|e| e.template_contents)) {
            set_node_doc(t.0, doc);
        }
    }
    if kind(copy) == NK::Document {
        let meta = page(|p| p.docs.get(&id).cloned());
        if let Some(m) = meta {
            page(|p| p.docs.insert(copy, m));
        }
    }
    copy
}

// =================================================================================================
// Mutation algorithms (DOM §4.2.3)
// =================================================================================================

fn hier(vm: &mut Vm, msg: &str) -> Value {
    dom_exception(vm, "HierarchyRequestError", msg)
}

fn is_inclusive_ancestor(d: &Document, a: usize, b: usize) -> bool {
    d.is_inclusive_ancestor(NodeId(a), NodeId(b))
}

/// "host-including inclusive ancestor": also through a template's contents to the template.
fn host_including_ancestor(d: &Document, a: usize, b: usize) -> bool {
    let mut cur = b;
    loop {
        if is_inclusive_ancestor(d, a, cur) {
            return true;
        }
        let mut root = NodeId(cur);
        while let Some(p) = d.parent(root) {
            root = p;
        }
        if !matches!(d.data(root), NodeData::DocumentFragment) {
            return false;
        }
        // A template's contents fragment: its host is the template element.
        let host = (0..d.nodes.len()).find(|&i| d.element(NodeId(i)).and_then(|e| e.template_contents) == Some(root));
        match host {
            Some(h) => cur = h,
            None => return false,
        }
    }
}

fn element_children_count(d: &Document, id: usize) -> usize {
    d.children(NodeId(id)).filter(|c| d.element(*c).is_some()).count()
}

fn has_doctype_child(d: &Document, id: usize) -> Option<usize> {
    d.children(NodeId(id)).find(|c| matches!(d.data(*c), NodeData::Doctype { .. })).map(|c| c.0)
}

fn doctype_following(d: &Document, child: usize) -> bool {
    let mut cur = d.next_sibling(NodeId(child));
    while let Some(c) = cur {
        if matches!(d.data(c), NodeData::Doctype { .. }) {
            return true;
        }
        cur = d.next_sibling(c);
    }
    false
}

fn element_preceding(d: &Document, child: usize) -> bool {
    let mut cur = d.prev_sibling(NodeId(child));
    while let Some(c) = cur {
        if d.element(c).is_some() {
            return true;
        }
        cur = d.prev_sibling(c);
    }
    false
}

/// DOM "ensure pre-insertion validity" (`replacing` = the "replace a child" variant of step 6).
fn ensure_validity(vm: &mut Vm, node: usize, parent: usize, child: Option<usize>, replacing: bool) -> JsResult<()> {
    enum Fail {
        Hier(&'static str),
        NotFound,
    }
    let r = with_doc(|d| -> Result<(), Fail> {
        let pk = kind_in(d, parent);
        if !matches!(pk, NK::Document | NK::Fragment | NK::Element) {
            return Err(Fail::Hier("The parent cannot have children."));
        }
        if host_including_ancestor(d, node, parent) {
            return Err(Fail::Hier("The new child element contains the parent."));
        }
        if let Some(c) = child {
            if d.parent(NodeId(c)).map(|p| p.0) != Some(parent) {
                return Err(Fail::NotFound);
            }
        }
        let nk = kind_in(d, node);
        if !matches!(nk, NK::Fragment | NK::Doctype | NK::Element | NK::Text | NK::Comment | NK::Pi) {
            return Err(Fail::Hier("Nodes of this type cannot be inserted."));
        }
        if (nk == NK::Text && pk == NK::Document) || (nk == NK::Doctype && pk != NK::Document) {
            return Err(Fail::Hier("Nodes of this type cannot be inserted here."));
        }
        if pk == NK::Document {
            let parent_elem = |except: Option<usize>| {
                d.children(NodeId(parent)).any(|c| d.element(c).is_some() && Some(c.0) != except)
            };
            match nk {
                NK::Fragment => {
                    let elems = element_children_count(d, node);
                    let has_text = d.children(NodeId(node)).any(|c| matches!(d.data(c), NodeData::Text(_)));
                    if elems > 1 || has_text {
                        return Err(Fail::Hier("Only one element on document allowed."));
                    }
                    if elems == 1 {
                        let bad = if replacing {
                            parent_elem(child) || child.is_some_and(|c| doctype_following(d, c))
                        } else {
                            parent_elem(None)
                                || child.is_some_and(|c| matches!(d.data(NodeId(c)), NodeData::Doctype { .. }))
                                || child.is_some_and(|c| doctype_following(d, c))
                        };
                        if bad {
                            return Err(Fail::Hier("Only one element on document allowed."));
                        }
                    }
                }
                NK::Element => {
                    let bad = if replacing {
                        parent_elem(child) || child.is_some_and(|c| doctype_following(d, c))
                    } else {
                        parent_elem(None)
                            || child.is_some_and(|c| matches!(d.data(NodeId(c)), NodeData::Doctype { .. }))
                            || child.is_some_and(|c| doctype_following(d, c))
                    };
                    if bad {
                        return Err(Fail::Hier("Only one element on document allowed."));
                    }
                }
                NK::Doctype => {
                    let dt = has_doctype_child(d, parent);
                    let bad = if replacing {
                        dt.is_some_and(|x| Some(x) != child) || child.is_some_and(|c| element_preceding(d, c))
                    } else {
                        dt.is_some()
                            || child.is_some_and(|c| element_preceding(d, c))
                            || (child.is_none() && parent_elem(None))
                    };
                    if bad {
                        return Err(Fail::Hier("Only one doctype on document allowed."));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    });
    match r {
        Ok(()) => Ok(()),
        Err(Fail::Hier(m)) => Err(hier(vm, m)),
        Err(Fail::NotFound) => {
            throw_dom(vm, "NotFoundError", "The node before which the new node is to be inserted is not a child of this node.")
        }
    }
}

/// DOM "insert" (§4.2.3): `node` (or a fragment's children) into `parent` before `child`.
pub fn insert(vm: &mut Vm, node: usize, parent: usize, child: Option<usize>) {
    let nodes: Vec<usize> = if kind(node) == NK::Fragment { children_of(node) } else { vec![node] };
    if nodes.is_empty() {
        return;
    }
    let doc = node_document(parent);
    for &n in &nodes {
        adopt(n, doc);
        with_doc_mut(|d| d.insert_before(NodeId(parent), NodeId(n), child.map(NodeId)));
    }
    touch();
    // Post-insertion steps: scripts that became connected are prepared (HTML §4.12.1).
    for &n in &nodes {
        super::loader::node_inserted(vm, n);
    }
    super::loader::children_changed(vm, parent);
}

/// DOM "pre-insert".
pub fn pre_insert(vm: &mut Vm, node: usize, parent: usize, child: Option<usize>) -> JsResult<usize> {
    ensure_validity(vm, node, parent, child, false)?;
    let mut reference = child;
    if reference == Some(node) {
        reference = with_doc(|d| d.next_sibling(NodeId(node)).map(|n| n.0));
    }
    insert(vm, node, parent, reference);
    Ok(node)
}

/// DOM "remove" (§4.2.3).
pub fn remove(vm: &mut Vm, node: usize) {
    let Some(parent) = parent_of(node) else { return };
    let focused = page(|p| p.focused);
    if let Some(f) = focused {
        if with_doc(|d| is_inclusive_ancestor(d, node, f)) {
            page(|p| p.focused = None);
        }
    }
    with_doc_mut(|d| d.detach(NodeId(node)));
    touch();
    super::loader::children_changed(vm, parent);
}

/// DOM "replace a child".
pub fn replace_child(vm: &mut Vm, child: usize, node: usize, parent: usize) -> JsResult<usize> {
    let nodes_ok = with_doc(|d| d.parent(NodeId(child)).map(|p| p.0) == Some(parent));
    // Steps 1–6 with the replace variant; the NotFoundError message differs.
    if !nodes_ok {
        let pk = kind(parent);
        if !matches!(pk, NK::Document | NK::Fragment | NK::Element) {
            return Err(hier(vm, "The parent cannot have children."));
        }
        if with_doc(|d| host_including_ancestor(d, node, parent)) {
            return Err(hier(vm, "The new child element contains the parent."));
        }
        return throw_dom(vm, "NotFoundError", "The node to be replaced is not a child of this node.");
    }
    ensure_validity(vm, node, parent, Some(child), true)?;
    let mut reference = with_doc(|d| d.next_sibling(NodeId(child)).map(|n| n.0));
    if reference == Some(node) {
        reference = with_doc(|d| d.next_sibling(NodeId(node)).map(|n| n.0));
    }
    if child != node {
        with_doc_mut(|d| d.detach(NodeId(child)));
    }
    insert(vm, node, parent, reference);
    touch();
    Ok(child)
}

/// DOM "replace all" (§4.2.3): `parent`'s children become `node` (or nothing).
pub fn replace_all(vm: &mut Vm, node: Option<usize>, parent: usize) {
    if let Some(n) = node {
        adopt(n, node_document(parent));
    }
    let kids = children_of(parent);
    with_doc_mut(|d| {
        for k in &kids {
            d.detach(NodeId(*k));
        }
    });
    touch();
    if let Some(n) = node {
        insert(vm, n, parent, None);
    } else if !kids.is_empty() {
        super::loader::children_changed(vm, parent);
    }
}

/// "convert nodes into a node" (ParentNode/ChildNode variadics): strings become Text nodes; several
/// nodes become a fragment.
fn convert_nodes(vm: &mut Vm, ctx: &CallCtx, doc: usize) -> JsResult<usize> {
    let mut ids = Vec::new();
    for i in 0..ctx.argc {
        let v = arg(vm, ctx, i);
        match node_of(vm, &v) {
            Some(n) => ids.push(n),
            None => {
                let s = string(vm, &v)?;
                ids.push(create_text(doc, s));
            }
        }
    }
    if ids.len() == 1 {
        return Ok(ids[0]);
    }
    let frag = with_doc_mut(|d| d.create(NodeData::DocumentFragment).0);
    set_node_doc(frag, doc);
    for n in ids {
        adopt(n, doc);
        with_doc_mut(|d| d.append(NodeId(frag), NodeId(n)));
    }
    Ok(frag)
}

// =================================================================================================
// Node
// =================================================================================================

fn node_type(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    Ok(num(match kind(n) {
        NK::Element => 1.0,
        NK::Text => 3.0,
        NK::Pi => 7.0,
        NK::Comment => 8.0,
        NK::Document => 9.0,
        NK::Doctype => 10.0,
        NK::Fragment => 11.0,
    }))
}

pub fn node_name_of(n: usize) -> String {
    match kind(n) {
        NK::Element => tag_name(n),
        NK::Text => "#text".into(),
        NK::Comment => "#comment".into(),
        NK::Document => "#document".into(),
        NK::Fragment => "#document-fragment".into(),
        NK::Doctype => with_doc(|d| match d.data(NodeId(n)) {
            NodeData::Doctype { name, .. } => name.clone(),
            _ => String::new(),
        }),
        NK::Pi => with_doc(|d| match d.data(NodeId(n)) {
            NodeData::ProcessingInstruction { target, .. } => target.clone(),
            _ => String::new(),
        }),
    }
}

fn node_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    Ok(s(&node_name_of(n)))
}

/// The document base URL (HTML §2.4.1): the first `<base href>` of the document, else its URL.
pub fn base_url(doc: usize) -> String {
    let url = page(|p| p.docs.get(&doc).map(|m| m.url.clone())).unwrap_or_else(super::page_url);
    let base = with_doc(|d| {
        d.query_first(NodeId(doc), |d, i| d.element(i).is_some_and(|e| e.is_html("base") && e.attr("href").is_some()))
            .and_then(|i| d.element(i).and_then(|e| e.attr("href")).map(str::to_string))
    });
    match base {
        Some(h) => url::Url::parse(&url).and_then(|u| u.join(&h)).map(|u| u.to_string()).unwrap_or(url),
        None => url,
    }
}

/// Parses `s` relative to `doc`'s base URL (the HTML "encoding-parse a URL").
pub fn resolve_url(doc: usize, s: &str) -> Option<String> {
    let base = base_url(doc);
    match url::Url::parse(&base) {
        Ok(b) => b.join(s.trim()).ok().map(|u| u.to_string()),
        Err(_) => url::Url::parse(s.trim()).ok().map(|u| u.to_string()),
    }
}

fn base_uri(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    Ok(s(&base_url(node_document(n))))
}

fn is_connected_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    Ok(Value::Bool(is_connected(n)))
}

fn owner_document(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    if kind(n) == NK::Document {
        return Ok(Value::Null);
    }
    let d = node_document(n);
    Ok(wrap(vm, d))
}

fn get_root_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let r = root_of(n);
    Ok(wrap(vm, r))
}

fn parent_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let p = parent_of(n);
    Ok(wrap_opt(vm, p))
}

fn parent_element(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let p = with_doc(|d| d.parent_element(NodeId(n)).map(|p| p.0));
    Ok(wrap_opt(vm, p))
}

fn has_child_nodes(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    Ok(Value::Bool(with_doc(|d| d.first_child(NodeId(n)).is_some())))
}

fn child_nodes(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    Ok(cached(vm, n, SO_CHILDNODES, |vm| make_collection(vm, CK_CHILDNODES, n, "", "")))
}

fn first_child(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let c = with_doc(|d| d.first_child(NodeId(n)).map(|c| c.0));
    Ok(wrap_opt(vm, c))
}

fn last_child(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let c = with_doc(|d| d.last_child(NodeId(n)).map(|c| c.0));
    Ok(wrap_opt(vm, c))
}

fn previous_sibling(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let c = with_doc(|d| d.prev_sibling(NodeId(n)).map(|c| c.0));
    Ok(wrap_opt(vm, c))
}

fn next_sibling(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let c = with_doc(|d| d.next_sibling(NodeId(n)).map(|c| c.0));
    Ok(wrap_opt(vm, c))
}

fn node_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    if is_char_data(kind(n)) {
        return Ok(s(&with_doc(|d| char_data(d, n))));
    }
    Ok(Value::Null)
}

fn node_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    if is_char_data(kind(n)) {
        let v = arg(vm, ctx, 0);
        let t = string_null_empty(vm, &v)?;
        replace_data(vm, n, 0, u32::MAX, &t)?;
    }
    Ok(Value::Undefined)
}

fn text_content_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    match kind(n) {
        NK::Element | NK::Fragment => Ok(s(&with_doc(|d| text_content_of(d, n)))),
        NK::Text | NK::Comment | NK::Pi => Ok(s(&with_doc(|d| char_data(d, n)))),
        _ => Ok(Value::Null),
    }
}

/// `textContent` setter semantics, shared with `innerText` (which differs for `<br>`, not modelled).
pub fn set_text_content(vm: &mut Vm, n: usize, t: String) -> JsResult<()> {
    match kind(n) {
        NK::Element | NK::Fragment => {
            let node = if t.is_empty() { None } else { Some(create_text(node_document(n), t)) };
            replace_all(vm, node, n);
        }
        NK::Text | NK::Comment | NK::Pi => replace_data(vm, n, 0, u32::MAX, &t)?,
        _ => {}
    }
    Ok(())
}

fn text_content_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    let t = string_null_empty(vm, &v)?;
    set_text_content(vm, n, t)?;
    Ok(Value::Undefined)
}

fn normalize(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    // DOM §4.4 normalize: for each exclusive Text descendant: drop empty ones, merge contiguous runs.
    let texts: Vec<usize> = with_doc(|d| {
        d.descendants(NodeId(n)).filter(|c| matches!(d.data(*c), NodeData::Text(_))).map(|c| c.0).collect()
    });
    let mut changed = false;
    for t in texts {
        // Skip nodes already merged away (detached by an earlier step).
        if parent_of(t).is_none() {
            continue;
        }
        let len = with_doc(|d| char_data(d, t).len());
        if len == 0 {
            with_doc_mut(|d| d.detach(NodeId(t)));
            changed = true;
            continue;
        }
        let mut data = with_doc(|d| char_data(d, t));
        let mut cur = with_doc(|d| d.next_sibling(NodeId(t)).map(|c| c.0));
        while let Some(c) = cur {
            if kind(c) != NK::Text {
                break;
            }
            data.push_str(&with_doc(|d| char_data(d, c)));
            let next = with_doc(|d| d.next_sibling(NodeId(c)).map(|x| x.0));
            with_doc_mut(|d| d.detach(NodeId(c)));
            changed = true;
            cur = next;
        }
        with_doc_mut(|d| set_char_data(d, t, data));
    }
    if changed {
        touch();
    }
    Ok(Value::Undefined)
}

fn clone_node_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let deep = boolean(vm, &arg(vm, ctx, 0));
    let c = clone_node(n, deep, None);
    Ok(wrap(vm, c))
}

/// DOM "equals" (§4.4).
fn node_equals(d: &Document, a: usize, b: usize) -> bool {
    let (da, db) = (d.data(NodeId(a)), d.data(NodeId(b)));
    let same = match (da, db) {
        (NodeData::Document, NodeData::Document) | (NodeData::DocumentFragment, NodeData::DocumentFragment) => true,
        (
            NodeData::Doctype { name: n1, public_id: p1, system_id: s1 },
            NodeData::Doctype { name: n2, public_id: p2, system_id: s2 },
        ) => n1 == n2 && p1 == p2 && s1 == s2,
        (NodeData::Element(e1), NodeData::Element(e2)) => {
            element_ns(d, a) == element_ns(d, b)
                && e1.local == e2.local
                && e1.attrs.len() == e2.attrs.len()
                && e1.attrs.iter().all(|x| e2.attrs.iter().any(|y| y.ns == x.ns && y.local == x.local && y.value == x.value))
        }
        (NodeData::Text(x), NodeData::Text(y)) | (NodeData::Comment(x), NodeData::Comment(y)) => x == y,
        (
            NodeData::ProcessingInstruction { target: t1, data: d1 },
            NodeData::ProcessingInstruction { target: t2, data: d2 },
        ) => t1 == t2 && d1 == d2,
        _ => false,
    };
    if !same {
        return false;
    }
    let ca: Vec<NodeId> = d.children(NodeId(a)).collect();
    let cb: Vec<NodeId> = d.children(NodeId(b)).collect();
    ca.len() == cb.len() && ca.iter().zip(cb.iter()).all(|(x, y)| node_equals(d, x.0, y.0))
}

fn is_equal_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    if v.is_nullish() {
        return Ok(Value::Bool(false));
    }
    let o = node_arg(vm, ctx, 0, "Node", "isEqualNode")?;
    Ok(Value::Bool(with_doc(|d| node_equals(d, n, o))))
}

fn is_same_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    Ok(Value::Bool(node_of(vm, &v) == Some(n)))
}

/// Tree order index path from the root (for compareDocumentPosition).
fn ancestors_inclusive(d: &Document, n: usize) -> Vec<usize> {
    let mut v = vec![n];
    let mut cur = NodeId(n);
    while let Some(p) = d.parent(cur) {
        v.push(p.0);
        cur = p;
    }
    v.reverse();
    v
}

/// Is `a` before `b` in tree order? (Both in the same tree.)
pub fn precedes(d: &Document, a: usize, b: usize) -> bool {
    let pa = ancestors_inclusive(d, a);
    let pb = ancestors_inclusive(d, b);
    let mut i = 0;
    while i < pa.len() && i < pb.len() && pa[i] == pb[i] {
        i += 1;
    }
    if i == pa.len() {
        return true; // a is an ancestor of b
    }
    if i == pb.len() {
        return false;
    }
    // siblings pa[i], pb[i] under pa[i-1]
    let mut cur = d.next_sibling(NodeId(pa[i]));
    while let Some(c) = cur {
        if c.0 == pb[i] {
            return true;
        }
        cur = d.next_sibling(c);
    }
    false
}

fn compare_document_position(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let me = this_node(vm, ctx)?;
    let other = node_arg(vm, ctx, 0, "Node", "compareDocumentPosition")?;
    if me == other {
        return Ok(num(0.0));
    }
    let r = with_doc(|d| {
        let ra = ancestors_inclusive(d, me)[0];
        let rb = ancestors_inclusive(d, other)[0];
        if ra != rb {
            // DISCONNECTED | IMPLEMENTATION_SPECIFIC | consistent PRECEDING/FOLLOWING.
            return 0x01 | 0x20 | if other < me { 0x02 } else { 0x04 };
        }
        if is_inclusive_ancestor(d, other, me) {
            return 0x08 | 0x02; // CONTAINS | PRECEDING
        }
        if is_inclusive_ancestor(d, me, other) {
            return 0x10 | 0x04; // CONTAINED_BY | FOLLOWING
        }
        if precedes(d, other, me) { 0x02 } else { 0x04 }
    });
    Ok(num(r as f64))
}

fn contains(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    if v.is_nullish() {
        return Ok(Value::Bool(false));
    }
    let o = node_arg(vm, ctx, 0, "Node", "contains")?;
    Ok(Value::Bool(with_doc(|d| is_inclusive_ancestor(d, n, o))))
}

/// DOM "locate a namespace" (§4.4).
fn locate_namespace(n: usize, prefix: Option<&str>) -> Option<String> {
    match kind(n) {
        NK::Element => {
            let (ns, pfx) = with_doc(|d| element_ns(d, n));
            if prefix == Some("xml") {
                return Some("http://www.w3.org/XML/1998/namespace".into());
            }
            if prefix == Some("xmlns") {
                return Some("http://www.w3.org/2000/xmlns/".into());
            }
            if !ns.is_empty() && pfx.as_deref() == prefix {
                return Some(ns);
            }
            let found = with_doc(|d| {
                d.element(NodeId(n)).and_then(|e| {
                    e.attrs.iter().find_map(|a| {
                        let hit = match prefix {
                            Some(p) => a.ns == Namespace::Xmlns && a.prefix == Some("xmlns") && a.local == p,
                            None => a.ns == Namespace::Xmlns && a.prefix.is_none() && a.local == "xmlns",
                        };
                        hit.then(|| a.value.clone())
                    })
                })
            });
            if let Some(v) = found {
                return if v.is_empty() { None } else { Some(v) };
            }
            let p = with_doc(|d| d.parent_element(NodeId(n)).map(|p| p.0))?;
            locate_namespace(p, prefix)
        }
        NK::Document => {
            let de = with_doc(|d| d.children(NodeId(n)).find(|c| d.element(*c).is_some()).map(|c| c.0))?;
            locate_namespace(de, prefix)
        }
        NK::Doctype | NK::Fragment => None,
        _ => {
            let p = with_doc(|d| d.parent_element(NodeId(n)).map(|p| p.0))?;
            locate_namespace(p, prefix)
        }
    }
}

fn locate_prefix(n: usize, ns: &str) -> Option<String> {
    let el = match kind(n) {
        NK::Element => Some(n),
        NK::Document => with_doc(|d| d.children(NodeId(n)).find(|c| d.element(*c).is_some()).map(|c| c.0)),
        NK::Doctype | NK::Fragment => None,
        _ => with_doc(|d| d.parent_element(NodeId(n)).map(|p| p.0)),
    }?;
    let (ens, pfx) = with_doc(|d| element_ns(d, el));
    if ens == ns && pfx.is_some() {
        return pfx;
    }
    let found = with_doc(|d| {
        d.element(NodeId(el)).and_then(|e| {
            e.attrs.iter().find_map(|a| (a.ns == Namespace::Xmlns && a.prefix == Some("xmlns") && a.value == ns).then(|| a.local.clone()))
        })
    });
    if found.is_some() {
        return found;
    }
    let p = with_doc(|d| d.parent_element(NodeId(el)).map(|p| p.0))?;
    locate_prefix(p, ns)
}

fn lookup_namespace_uri(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let p = arg(vm, ctx, 0);
    let p = opt_string(vm, &p)?.filter(|s| !s.is_empty());
    Ok(match locate_namespace(n, p.as_deref()) {
        Some(ns) => s(&ns),
        None => Value::Null,
    })
}

fn lookup_prefix(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    let Some(ns) = opt_string(vm, &v)?.filter(|s| !s.is_empty()) else { return Ok(Value::Null) };
    Ok(match locate_prefix(n, &ns) {
        Some(p) => s(&p),
        None => Value::Null,
    })
}

fn is_default_namespace(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    let ns = opt_string(vm, &v)?.filter(|s| !s.is_empty());
    Ok(Value::Bool(locate_namespace(n, None) == ns))
}

fn insert_before_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let parent = this_node(vm, ctx)?;
    need(vm, ctx, 2, "Node", "insertBefore")?;
    let node = node_arg(vm, ctx, 0, "Node", "insertBefore")?;
    let child = opt_node_arg(vm, ctx, 1, "Node", "insertBefore")?;
    let r = pre_insert(vm, node, parent, child)?;
    Ok(wrap(vm, r))
}

fn append_child_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let parent = this_node(vm, ctx)?;
    need(vm, ctx, 1, "Node", "appendChild")?;
    let node = node_arg(vm, ctx, 0, "Node", "appendChild")?;
    let r = pre_insert(vm, node, parent, None)?;
    Ok(wrap(vm, r))
}

fn replace_child_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let parent = this_node(vm, ctx)?;
    need(vm, ctx, 2, "Node", "replaceChild")?;
    let node = node_arg(vm, ctx, 0, "Node", "replaceChild")?;
    let child = node_arg(vm, ctx, 1, "Node", "replaceChild")?;
    let r = replace_child(vm, child, node, parent)?;
    Ok(wrap(vm, r))
}

fn remove_child_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let parent = this_node(vm, ctx)?;
    need(vm, ctx, 1, "Node", "removeChild")?;
    let child = node_arg(vm, ctx, 0, "Node", "removeChild")?;
    if parent_of(child) != Some(parent) {
        return throw_dom(vm, "NotFoundError", "The node to be removed is not a child of this node.");
    }
    remove(vm, child);
    Ok(wrap(vm, child))
}

// =================================================================================================
// ParentNode / ChildNode / NonDocumentTypeChildNode mixins
// =================================================================================================

fn children_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    Ok(cached(vm, n, SO_CHILDREN, |vm| make_collection(vm, CK_CHILDREN, n, "", "")))
}

fn first_element_child(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let c = with_doc(|d| d.element_children(NodeId(n)).next().map(|c| c.0));
    Ok(wrap_opt(vm, c))
}

fn last_element_child(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let c = with_doc(|d| d.element_children(NodeId(n)).last().map(|c| c.0));
    Ok(wrap_opt(vm, c))
}

fn child_element_count(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    Ok(num(with_doc(|d| element_children_count(d, n)) as f64))
}

fn prepend_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let node = convert_nodes(vm, ctx, node_document(n))?;
    let first = with_doc(|d| d.first_child(NodeId(n)).map(|c| c.0));
    pre_insert(vm, node, n, first)?;
    Ok(Value::Undefined)
}

fn append_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let node = convert_nodes(vm, ctx, node_document(n))?;
    pre_insert(vm, node, n, None)?;
    Ok(Value::Undefined)
}

fn replace_children_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let node = convert_nodes(vm, ctx, node_document(n))?;
    ensure_validity(vm, node, n, None, false)?;
    replace_all(vm, Some(node), n);
    Ok(Value::Undefined)
}

/// Compiles a selector list or throws the Selectors API SyntaxError.
fn compile_selectors(vm: &mut Vm, sel: &str) -> JsResult<Selectors> {
    match Selectors::compile(sel) {
        Ok(s) => Ok(s),
        Err(()) => throw_dom(vm, "SyntaxError", &format!("'{sel}' is not a valid selector.")),
    }
}

/// Elements among the descendants of `root` matching `sel`, in tree order (`first` stops at one).
pub fn query(vm: &mut Vm, root: usize, sel: &str, first: bool) -> JsResult<Vec<usize>> {
    let sels = compile_selectors(vm, sel)?;
    Ok(with_doc(|d| {
        let scope = d.element(NodeId(root)).map(|_| NodeId(root));
        let mut out = Vec::new();
        for id in d.descendants(NodeId(root)) {
            if sels.matches_in(d, id, scope) {
                out.push(id.0);
                if first {
                    break;
                }
            }
        }
        out
    }))
}

fn query_selector(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    need(vm, ctx, 1, "Element", "querySelector")?;
    let v = arg(vm, ctx, 0);
    let sel = string(vm, &v)?;
    let r = query(vm, n, &sel, true)?;
    Ok(wrap_opt(vm, r.first().copied()))
}

fn query_selector_all(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    need(vm, ctx, 1, "Element", "querySelectorAll")?;
    let v = arg(vm, ctx, 0);
    let sel = string(vm, &v)?;
    let r = query(vm, n, &sel, false)?;
    Ok(static_node_list(vm, r))
}

fn before_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let Some(parent) = parent_of(n) else { return Ok(Value::Undefined) };
    let args: HashSet<usize> = (0..ctx.argc).filter_map(|i| node_of(vm, &arg(vm, ctx, i))).collect();
    // viable previous sibling: first preceding sibling not in nodes.
    let mut prev = with_doc(|d| d.prev_sibling(NodeId(n)).map(|c| c.0));
    while let Some(p) = prev {
        if !args.contains(&p) {
            break;
        }
        prev = with_doc(|d| d.prev_sibling(NodeId(p)).map(|c| c.0));
    }
    let node = convert_nodes(vm, ctx, node_document(n))?;
    let reference = match prev {
        None => with_doc(|d| d.first_child(NodeId(parent)).map(|c| c.0)),
        Some(p) => with_doc(|d| d.next_sibling(NodeId(p)).map(|c| c.0)),
    };
    pre_insert(vm, node, parent, reference)?;
    Ok(Value::Undefined)
}

fn after_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let Some(parent) = parent_of(n) else { return Ok(Value::Undefined) };
    let args: HashSet<usize> = (0..ctx.argc).filter_map(|i| node_of(vm, &arg(vm, ctx, i))).collect();
    let mut next = with_doc(|d| d.next_sibling(NodeId(n)).map(|c| c.0));
    while let Some(x) = next {
        if !args.contains(&x) {
            break;
        }
        next = with_doc(|d| d.next_sibling(NodeId(x)).map(|c| c.0));
    }
    let node = convert_nodes(vm, ctx, node_document(n))?;
    pre_insert(vm, node, parent, next)?;
    Ok(Value::Undefined)
}

fn replace_with_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let Some(parent) = parent_of(n) else { return Ok(Value::Undefined) };
    let args: HashSet<usize> = (0..ctx.argc).filter_map(|i| node_of(vm, &arg(vm, ctx, i))).collect();
    let mut next = with_doc(|d| d.next_sibling(NodeId(n)).map(|c| c.0));
    while let Some(x) = next {
        if !args.contains(&x) {
            break;
        }
        next = with_doc(|d| d.next_sibling(NodeId(x)).map(|c| c.0));
    }
    let node = convert_nodes(vm, ctx, node_document(n))?;
    if parent_of(n) == Some(parent) {
        replace_child(vm, n, node, parent)?;
    } else {
        pre_insert(vm, node, parent, next)?;
    }
    Ok(Value::Undefined)
}

fn remove_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    remove(vm, n);
    Ok(Value::Undefined)
}

fn previous_element_sibling(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let c = with_doc(|d| d.prev_sibling_element(NodeId(n)).map(|c| c.0));
    Ok(wrap_opt(vm, c))
}

fn next_element_sibling(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    let c = with_doc(|d| d.next_sibling_element(NodeId(n)).map(|c| c.0));
    Ok(wrap_opt(vm, c))
}

// =================================================================================================
// CharacterData / Text / Comment / ProcessingInstruction / DocumentType
// =================================================================================================

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn from_utf16(v: &[u16]) -> String {
    String::from_utf16_lossy(v)
}

/// DOM "replace data" (§4.10), offsets in UTF-16 code units.
pub fn replace_data(vm: &mut Vm, n: usize, offset: u32, count: u32, data: &str) -> JsResult<()> {
    let cur = utf16(&with_doc(|d| char_data(d, n)));
    let len = cur.len() as u32;
    if offset > len {
        return throw_dom(vm, "IndexSizeError", "The offset is greater than the node's length.");
    }
    let count = count.min(len - offset);
    let mut out = cur[..offset as usize].to_vec();
    out.extend(utf16(data));
    out.extend_from_slice(&cur[(offset + count) as usize..]);
    with_doc_mut(|d| set_char_data(d, n, from_utf16(&out)));
    touch();
    if let Some(p) = parent_of(n) {
        super::loader::children_changed(vm, p);
    }
    Ok(())
}

fn this_cdata(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    this_kind(vm, ctx, &[NK::Text, NK::Comment, NK::Pi])
}

fn cdata_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_cdata(vm, ctx)?;
    Ok(s(&with_doc(|d| char_data(d, n))))
}

fn cdata_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_cdata(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    let t = string_null_empty(vm, &v)?;
    replace_data(vm, n, 0, u32::MAX, &t)?;
    Ok(Value::Undefined)
}

fn cdata_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_cdata(vm, ctx)?;
    Ok(num(with_doc(|d| char_data(d, n)).encode_utf16().count() as f64))
}

fn substring_data(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_cdata(vm, ctx)?;
    need(vm, ctx, 2, "CharacterData", "substringData")?;
    let o = unsigned_long(vm, &arg(vm, ctx, 0))?;
    let c = unsigned_long(vm, &arg(vm, ctx, 1))?;
    let cur = utf16(&with_doc(|d| char_data(d, n)));
    let len = cur.len() as u32;
    if o > len {
        return throw_dom(vm, "IndexSizeError", "The offset is greater than the node's length.");
    }
    let end = (o as u64 + c as u64).min(len as u64) as usize;
    Ok(Value::String(JsStr::from_slice(&cur[o as usize..end])))
}

fn append_data(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_cdata(vm, ctx)?;
    need(vm, ctx, 1, "CharacterData", "appendData")?;
    let t = string(vm, &arg(vm, ctx, 0))?;
    let len = with_doc(|d| char_data(d, n)).encode_utf16().count() as u32;
    replace_data(vm, n, len, 0, &t)?;
    Ok(Value::Undefined)
}

fn insert_data(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_cdata(vm, ctx)?;
    need(vm, ctx, 2, "CharacterData", "insertData")?;
    let o = unsigned_long(vm, &arg(vm, ctx, 0))?;
    let t = string(vm, &arg(vm, ctx, 1))?;
    replace_data(vm, n, o, 0, &t)?;
    Ok(Value::Undefined)
}

fn delete_data(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_cdata(vm, ctx)?;
    need(vm, ctx, 2, "CharacterData", "deleteData")?;
    let o = unsigned_long(vm, &arg(vm, ctx, 0))?;
    let c = unsigned_long(vm, &arg(vm, ctx, 1))?;
    replace_data(vm, n, o, c, "")?;
    Ok(Value::Undefined)
}

fn replace_data_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_cdata(vm, ctx)?;
    need(vm, ctx, 3, "CharacterData", "replaceData")?;
    let o = unsigned_long(vm, &arg(vm, ctx, 0))?;
    let c = unsigned_long(vm, &arg(vm, ctx, 1))?;
    let t = string(vm, &arg(vm, ctx, 2))?;
    replace_data(vm, n, o, c, &t)?;
    Ok(Value::Undefined)
}

fn current_doc_for_ctor() -> usize {
    main_doc()
}

fn text_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'Text': Please use the 'new' operator");
    }
    let v = arg(vm, ctx, 0);
    let t = if v.is_undefined() { String::new() } else { string(vm, &v)? };
    let id = create_text(current_doc_for_ctor(), t);
    let w = wrap(vm, id);
    let default = proto("Text");
    let p = proto_from_new_target(vm, &ctx.new_target, default)?;
    if p != default {
        vm.heap.get_mut(w.as_object().unwrap()).proto = Some(p);
    }
    Ok(w)
}

fn comment_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'Comment': Please use the 'new' operator");
    }
    let v = arg(vm, ctx, 0);
    let t = if v.is_undefined() { String::new() } else { string(vm, &v)? };
    let id = create_comment(current_doc_for_ctor(), t);
    Ok(wrap(vm, id))
}

fn split_text(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_kind(vm, ctx, &[NK::Text])?;
    need(vm, ctx, 1, "Text", "splitText")?;
    let o = unsigned_long(vm, &arg(vm, ctx, 0))?;
    let cur = utf16(&with_doc(|d| char_data(d, n)));
    let len = cur.len() as u32;
    if o > len {
        return throw_dom(vm, "IndexSizeError", "The offset is greater than the Text node's length.");
    }
    let new_data = from_utf16(&cur[o as usize..]);
    let new_node = create_text(node_document(n), new_data);
    if let Some(p) = parent_of(n) {
        let next = with_doc(|d| d.next_sibling(NodeId(n)).map(|c| c.0));
        with_doc_mut(|d| d.insert_before(NodeId(p), NodeId(new_node), next.map(NodeId)));
    }
    replace_data(vm, n, o, len - o, "")?;
    Ok(wrap(vm, new_node))
}

fn whole_text(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_kind(vm, ctx, &[NK::Text])?;
    let t = with_doc(|d| {
        let mut start = NodeId(n);
        while let Some(p) = d.prev_sibling(start) {
            if !matches!(d.data(p), NodeData::Text(_)) {
                break;
            }
            start = p;
        }
        let mut out = String::new();
        let mut cur = Some(start);
        while let Some(c) = cur {
            match d.data(c) {
                NodeData::Text(t) => out.push_str(t),
                _ => break,
            }
            cur = d.next_sibling(c);
        }
        out
    });
    Ok(s(&t))
}

fn pi_target(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_kind(vm, ctx, &[NK::Pi])?;
    Ok(s(&node_name_of(n)))
}

fn doctype_field(vm: &mut Vm, ctx: &CallCtx, which: u8) -> JsResult<Value> {
    let n = this_kind(vm, ctx, &[NK::Doctype])?;
    let v = with_doc(|d| match d.data(NodeId(n)) {
        NodeData::Doctype { name, public_id, system_id } => match which {
            0 => name.clone(),
            1 => public_id.clone(),
            _ => system_id.clone(),
        },
        _ => String::new(),
    });
    Ok(s(&v))
}
fn doctype_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    doctype_field(vm, ctx, 0)
}
fn doctype_public(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    doctype_field(vm, ctx, 1)
}
fn doctype_system(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    doctype_field(vm, ctx, 2)
}

// =================================================================================================
// Element
// =================================================================================================

fn namespace_uri(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_element(vm, ctx)?;
    let (ns, _) = with_doc(|d| element_ns(d, n));
    Ok(if ns.is_empty() { Value::Null } else { s(&ns) })
}

fn prefix_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_element(vm, ctx)?;
    let (_, p) = with_doc(|d| element_ns(d, n));
    Ok(p.map(|p| s(&p)).unwrap_or(Value::Null))
}

fn local_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_element(vm, ctx)?;
    Ok(s(&with_doc(|d| local_name_in(d, n))))
}

fn tag_name_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_element(vm, ctx)?;
    Ok(s(&tag_name(n)))
}

/// Should attribute names on `el` be lowercased (HTML element in an HTML document)?
fn lower_attr_names(el: usize) -> bool {
    with_doc(|d| is_html_element(d, el)) && is_html_doc(node_document(el))
}

fn attr_qname(a: &Attribute) -> String {
    match a.prefix {
        Some(p) => format!("{p}:{}", a.local),
        None => match a.ns {
            Namespace::Xmlns if a.local != "xmlns" => format!("xmlns:{}", a.local),
            Namespace::XLink => format!("xlink:{}", a.local),
            Namespace::Xml => format!("xml:{}", a.local),
            _ => a.local.clone(),
        },
    }
}

/// Index of the first attribute whose qualified name is `qname` (lowercased first for HTML elements).
fn find_attr_qname(el: usize, qname: &str) -> Option<usize> {
    let q = if lower_attr_names(el) { qname.to_ascii_lowercase() } else { qname.to_string() };
    with_doc(|d| d.element(NodeId(el)).and_then(|e| e.attrs.iter().position(|a| attr_qname(a) == q)))
}

fn find_attr_ns(el: usize, ns: &str, local: &str) -> Option<usize> {
    with_doc(|d| d.element(NodeId(el)).and_then(|e| e.attrs.iter().position(|a| a.ns.url() == ns && a.local == local)))
}

pub fn attr_at(el: usize, i: usize) -> Option<Attribute> {
    with_doc(|d| d.element(NodeId(el)).and_then(|e| e.attrs.get(i).cloned()))
}

/// HTML "attribute change steps" hook: event handler attributes, form-control default values.
fn attribute_changed(vm: &mut Vm, el: usize, local: &str, ns: Namespace, old: Option<String>, new: Option<String>) {
    touch();
    if ns == Namespace::None && local.starts_with("on") {
        super::events::handler_attribute_changed(vm, el, &local[2..], new.is_some());
    }
    // An Attr object for a removed attribute keeps its last value.
    if new.is_none() {
        let key = (el, format!("{}|{}", ns.url(), local));
        if let Some((o, r)) = page(|p| p.attrs.remove(&key)) {
            set_slot(vm, o, 0, num(-1.0));
            set_slot(vm, o, 4, s(&old.unwrap_or_default()));
            unroot(vm, r);
            // Still referenced by script, maybe: keep it alive through its own root entry.
            let r2 = root(vm, Value::Object(o));
            let _ = r2;
        }
    }
}

/// Sets (or creates) an attribute (DOM "set an attribute value").
pub fn set_attr(vm: &mut Vm, el: usize, local: &str, value: &str) {
    let old = with_doc_mut(|d| {
        let e = d.element_mut(NodeId(el))?;
        match e.attrs.iter_mut().find(|a| a.ns == Namespace::None && a.local == local) {
            Some(a) => Some(std::mem::replace(&mut a.value, value.to_string())),
            None => {
                e.attrs.push(Attribute { prefix: None, ns: Namespace::None, local: local.to_string(), value: value.to_string() });
                None
            }
        }
    });
    attribute_changed(vm, el, local, Namespace::None, old, Some(value.to_string()));
}

pub fn remove_attr(vm: &mut Vm, el: usize, local: &str) {
    let old = with_doc_mut(|d| {
        let e = d.element_mut(NodeId(el))?;
        let i = e.attrs.iter().position(|a| a.ns == Namespace::None && a.local == local)?;
        Some(e.attrs.remove(i).value)
    });
    if old.is_some() {
        attribute_changed(vm, el, local, Namespace::None, old, None);
    }
}

/// XML `Name` production, as the DOM's "valid attribute local name"/"valid element local name" checks
/// use it (InvalidCharacterError).
pub fn is_valid_name(s: &str) -> bool {
    fn start(c: char) -> bool {
        c == ':' || c == '_' || c.is_ascii_alphabetic()
            || matches!(c as u32, 0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x2FF | 0x370..=0x37D | 0x37F..=0x1FFF | 0x200C..=0x200D | 0x2070..=0x218F | 0x2C00..=0x2FEF | 0x3001..=0xD7FF | 0xF900..=0xFDCF | 0xFDF0..=0xFFFD | 0x10000..=0xEFFFF)
    }
    fn rest(c: char) -> bool {
        start(c) || c == '-' || c == '.' || c.is_ascii_digit() || matches!(c as u32, 0xB7 | 0x300..=0x36F | 0x203F..=0x2040)
    }
    let mut it = s.chars();
    match it.next() {
        Some(c) if start(c) => it.all(rest),
        _ => false,
    }
}

/// DOM "validate and extract" (§1.3): (namespace, prefix, local name).
pub fn validate_and_extract(vm: &mut Vm, ns: Option<String>, qname: &str) -> JsResult<(String, Option<String>, String)> {
    let ns = ns.filter(|n| !n.is_empty()).unwrap_or_default();
    if !is_valid_name(qname) || qname.starts_with(':') || qname.ends_with(':') || qname.matches(':').count() > 1 {
        return throw_dom(vm, "InvalidCharacterError", &format!("'{qname}' is not a valid name."));
    }
    let (prefix, local) = match qname.split_once(':') {
        Some((p, l)) => (Some(p.to_string()), l.to_string()),
        None => (None, qname.to_string()),
    };
    if prefix.is_some() && ns.is_empty() {
        return throw_dom(vm, "NamespaceError", "A prefix needs a namespace.");
    }
    if prefix.as_deref() == Some("xml") && ns != "http://www.w3.org/XML/1998/namespace" {
        return throw_dom(vm, "NamespaceError", "The xml prefix is reserved.");
    }
    let xmlns_ns = "http://www.w3.org/2000/xmlns/";
    if (qname == "xmlns" || prefix.as_deref() == Some("xmlns")) != (ns == xmlns_ns) {
        return throw_dom(vm, "NamespaceError", "The xmlns prefix and namespace go together.");
    }
    Ok((ns, prefix, local))
}

fn get_attribute(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "Element", "getAttribute")?;
    let q = string(vm, &arg(vm, ctx, 0))?;
    Ok(match find_attr_qname(el, &q).and_then(|i| attr_at(el, i)) {
        Some(a) => s(&a.value),
        None => Value::Null,
    })
}

fn get_attribute_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 2, "Element", "getAttributeNS")?;
    let ns = opt_string(vm, &arg(vm, ctx, 0))?.unwrap_or_default();
    let l = string(vm, &arg(vm, ctx, 1))?;
    Ok(match find_attr_ns(el, &ns, &l).and_then(|i| attr_at(el, i)) {
        Some(a) => s(&a.value),
        None => Value::Null,
    })
}

fn set_attribute(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 2, "Element", "setAttribute")?;
    let q = string(vm, &arg(vm, ctx, 0))?;
    let v = string(vm, &arg(vm, ctx, 1))?;
    if !is_valid_name(&q) {
        return throw_dom(vm, "InvalidCharacterError", &format!("'{q}' is not a valid attribute name."));
    }
    let q = if lower_attr_names(el) { q.to_ascii_lowercase() } else { q };
    match find_attr_qname(el, &q) {
        Some(i) => {
            let (old, local, ns) = with_doc_mut(|d| {
                let a = &mut d.element_mut(NodeId(el)).unwrap().attrs[i];
                (std::mem::replace(&mut a.value, v.clone()), a.local.clone(), a.ns)
            });
            attribute_changed(vm, el, &local, ns, Some(old), Some(v));
        }
        None => set_attr(vm, el, &q, &v),
    }
    Ok(Value::Undefined)
}

fn set_attribute_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 3, "Element", "setAttributeNS")?;
    let ns = opt_string(vm, &arg(vm, ctx, 0))?;
    let q = string(vm, &arg(vm, ctx, 1))?;
    let v = string(vm, &arg(vm, ctx, 2))?;
    let (ns, prefix, local) = validate_and_extract(vm, ns, &q)?;
    let nse = match ns_from_url(&ns) {
        Some(n) => n,
        None => {
            crate::ledger::record_dom("setAttributeNS-namespace-unrepresentable");
            Namespace::None
        }
    };
    let old = with_doc_mut(|d| {
        let e = d.element_mut(NodeId(el))?;
        match e.attrs.iter_mut().find(|a| a.ns == nse && a.local == local) {
            Some(a) => Some(std::mem::replace(&mut a.value, v.clone())),
            None => {
                e.attrs.push(Attribute { prefix: prefix.as_deref().map(intern), ns: nse, local: local.clone(), value: v.clone() });
                None
            }
        }
    });
    attribute_changed(vm, el, &local, nse, old, Some(v));
    Ok(Value::Undefined)
}

fn remove_attribute(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "Element", "removeAttribute")?;
    let q = string(vm, &arg(vm, ctx, 0))?;
    if let Some(i) = find_attr_qname(el, &q) {
        let a = with_doc_mut(|d| d.element_mut(NodeId(el)).unwrap().attrs.remove(i));
        attribute_changed(vm, el, &a.local, a.ns, Some(a.value), None);
    }
    Ok(Value::Undefined)
}

fn remove_attribute_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 2, "Element", "removeAttributeNS")?;
    let ns = opt_string(vm, &arg(vm, ctx, 0))?.unwrap_or_default();
    let l = string(vm, &arg(vm, ctx, 1))?;
    if let Some(i) = find_attr_ns(el, &ns, &l) {
        let a = with_doc_mut(|d| d.element_mut(NodeId(el)).unwrap().attrs.remove(i));
        attribute_changed(vm, el, &a.local, a.ns, Some(a.value), None);
    }
    Ok(Value::Undefined)
}

fn toggle_attribute(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "Element", "toggleAttribute")?;
    let q = string(vm, &arg(vm, ctx, 0))?;
    if !is_valid_name(&q) {
        return throw_dom(vm, "InvalidCharacterError", &format!("'{q}' is not a valid attribute name."));
    }
    let q = if lower_attr_names(el) { q.to_ascii_lowercase() } else { q };
    let force = arg(vm, ctx, 1);
    let has = find_attr_qname(el, &q);
    match has {
        None => {
            if force.is_undefined() || boolean(vm, &force) {
                set_attr(vm, el, &q, "");
                return Ok(Value::Bool(true));
            }
            Ok(Value::Bool(false))
        }
        Some(i) => {
            if force.is_undefined() || !boolean(vm, &force) {
                let a = with_doc_mut(|d| d.element_mut(NodeId(el)).unwrap().attrs.remove(i));
                attribute_changed(vm, el, &a.local, a.ns, Some(a.value), None);
                return Ok(Value::Bool(false));
            }
            Ok(Value::Bool(true))
        }
    }
}

fn has_attribute(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "Element", "hasAttribute")?;
    let q = string(vm, &arg(vm, ctx, 0))?;
    Ok(Value::Bool(find_attr_qname(el, &q).is_some()))
}

fn has_attribute_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 2, "Element", "hasAttributeNS")?;
    let ns = opt_string(vm, &arg(vm, ctx, 0))?.unwrap_or_default();
    let l = string(vm, &arg(vm, ctx, 1))?;
    Ok(Value::Bool(find_attr_ns(el, &ns, &l).is_some()))
}

fn has_attributes(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(Value::Bool(with_doc(|d| d.element(NodeId(el)).is_some_and(|e| !e.attrs.is_empty()))))
}

fn get_attribute_names(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let names: Vec<Value> = with_doc(|d| d.element(NodeId(el)).map(|e| e.attrs.iter().map(|a| s(&attr_qname(a))).collect()).unwrap_or_default());
    Ok(Value::Object(vm.new_array(names)))
}

fn id_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&attr_value(el, "id").unwrap_or_default()))
}

fn id_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, "id", &v);
    Ok(Value::Undefined)
}

fn class_name_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&attr_value(el, "class").unwrap_or_default()))
}

fn class_name_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, "class", &v);
    Ok(Value::Undefined)
}

fn class_list_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(cached(vm, el, SO_CLASSLIST, |vm| make_collection(vm, CK_TOKENS, el, "class", "")))
}

fn class_list_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    // [PutForwards=value]
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, "class", &v);
    Ok(Value::Undefined)
}

fn slot_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(s(&attr_value(el, "slot").unwrap_or_default()))
}
fn slot_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, "slot", &v);
    Ok(Value::Undefined)
}

fn attributes_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    Ok(cached(vm, el, SO_ATTRIBUTES, |vm| make_collection(vm, CK_ATTRMAP, el, "", "")))
}

fn matches_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "Element", "matches")?;
    let sel = string(vm, &arg(vm, ctx, 0))?;
    let sels = compile_selectors(vm, &sel)?;
    Ok(Value::Bool(with_doc(|d| sels.matches_in(d, NodeId(el), Some(NodeId(el))))))
}

fn closest_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "Element", "closest")?;
    let sel = string(vm, &arg(vm, ctx, 0))?;
    let sels = compile_selectors(vm, &sel)?;
    let hit = with_doc(|d| {
        let mut cur = Some(NodeId(el));
        while let Some(c) = cur {
            if d.element(c).is_some() && sels.matches_in(d, c, Some(NodeId(el))) {
                return Some(c.0);
            }
            cur = d.parent_element(c);
        }
        None
    });
    Ok(wrap_opt(vm, hit))
}

fn get_elements_by_tag_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    need(vm, ctx, 1, "Element", "getElementsByTagName")?;
    let q = string(vm, &arg(vm, ctx, 0))?;
    Ok(Value::Object(make_collection(vm, CK_TAGNAME, n, &q, "")))
}

fn get_elements_by_tag_name_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    need(vm, ctx, 2, "Element", "getElementsByTagNameNS")?;
    let ns = opt_string(vm, &arg(vm, ctx, 0))?.unwrap_or_default();
    let l = string(vm, &arg(vm, ctx, 1))?;
    Ok(Value::Object(make_collection(vm, CK_TAGNAME_NS, n, &ns, &l)))
}

fn get_elements_by_class_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_node(vm, ctx)?;
    need(vm, ctx, 1, "Element", "getElementsByClassName")?;
    let c = string(vm, &arg(vm, ctx, 0))?;
    Ok(Value::Object(make_collection(vm, CK_CLASSNAME, n, &c, "")))
}

/// "insert adjacent" (DOM §4.9).
fn insert_adjacent(vm: &mut Vm, el: usize, where_: &str, node: usize) -> JsResult<Option<usize>> {
    match where_.to_ascii_lowercase().as_str() {
        "beforebegin" => {
            let Some(p) = parent_of(el) else { return Ok(None) };
            pre_insert(vm, node, p, Some(el)).map(Some)
        }
        "afterbegin" => {
            let first = with_doc(|d| d.first_child(NodeId(el)).map(|c| c.0));
            pre_insert(vm, node, el, first).map(Some)
        }
        "beforeend" => pre_insert(vm, node, el, None).map(Some),
        "afterend" => {
            let Some(p) = parent_of(el) else { return Ok(None) };
            let next = with_doc(|d| d.next_sibling(NodeId(el)).map(|c| c.0));
            pre_insert(vm, node, p, next).map(Some)
        }
        _ => throw_dom(vm, "SyntaxError", &format!("'{where_}' is not a valid position.")),
    }
}

fn insert_adjacent_element(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 2, "Element", "insertAdjacentElement")?;
    let w = string(vm, &arg(vm, ctx, 0))?;
    let v = arg(vm, ctx, 1);
    let node = match node_of(vm, &v) {
        Some(n) if kind(n) == NK::Element => n,
        _ => return vm.throw_type("Failed to execute 'insertAdjacentElement' on 'Element': parameter 2 is not of type 'Element'."),
    };
    let r = insert_adjacent(vm, el, &w, node)?;
    Ok(wrap_opt(vm, r))
}

fn insert_adjacent_text(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 2, "Element", "insertAdjacentText")?;
    let w = string(vm, &arg(vm, ctx, 0))?;
    let t = string(vm, &arg(vm, ctx, 1))?;
    let node = create_text(node_document(el), t);
    insert_adjacent(vm, el, &w, node)?;
    Ok(Value::Undefined)
}

// -------------------------------------------------------------------------------------------- markup

/// The HTML fragment parsing algorithm (§13.4) in `context`'s context: the parsed nodes, imported
/// into the page arena under a new DocumentFragment, with every script marked "already started"
/// (scripts inserted through innerHTML never run).
pub fn parse_fragment(context: usize, markup: &str) -> usize {
    let (ns, local, attrs, quirks) = with_doc(|d| match d.element(NodeId(context)) {
        Some(e) if !(e.ns == Namespace::Html && e.local == "html") || true => (e.ns, e.local.clone(), e.attrs.clone(), d.quirks_mode),
        _ => (Namespace::Html, "body".to_string(), Vec::new(), d.quirks_mode),
    });
    let (fdoc, froot) = html_core::parse_fragment(ns, &local, attrs, markup, quirks, crate::dom::parse_opts());
    let frag = with_doc_mut(|d| crate::dom::import_tree(d, &fdoc, froot, false).0);
    let doc = node_document(context);
    let ids: Vec<usize> = with_doc(|d| {
        let mut v = vec![frag];
        v.extend(d.descendants(NodeId(frag)).map(|n| n.0));
        v
    });
    for &i in &ids {
        set_node_doc(i, doc);
        if with_doc(|d| d.element(NodeId(i)).is_some_and(|e| e.is_html("script"))) {
            page(|p| p.started.insert(i));
        }
        if let Some(t) = with_doc(|d| d.element(NodeId(i)).and_then(|e| e.template_contents)) {
            set_node_doc(t.0, doc);
        }
    }
    frag
}

/// Marks `id` "already started" when it is an HTML script (parsed by something other than the page's
/// parser: those scripts never run).
pub fn mark_script_started_if_script(id: usize) {
    if with_doc(|d| d.element(NodeId(id)).is_some_and(|e| e.is_html("script"))) {
        page(|p| p.started.insert(id));
    }
}

/// DOM Parsing §3.2 "XML serialization" (the XMLSerializer path): namespace declarations where an
/// element's namespace differs from its context's, `/>` for empty non-HTML elements and ` />` for HTML
/// void elements, `&amp; &lt; &gt;` (and `&quot;` in attributes) escaping.
pub fn xml_serialize(id: usize) -> String {
    fn esc(s: &str, attr: bool) -> String {
        let mut o = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '&' => o.push_str("&amp;"),
                '<' => o.push_str("&lt;"),
                '>' => o.push_str("&gt;"),
                '"' if attr => o.push_str("&quot;"),
                c => o.push(c),
            }
        }
        o
    }
    fn walk(d: &Document, id: usize, ctx_ns: Option<&str>, out: &mut String) {
        match d.data(NodeId(id)) {
            NodeData::Document | NodeData::DocumentFragment => {
                for c in d.children(NodeId(id)) {
                    walk(d, c.0, ctx_ns, out);
                }
            }
            NodeData::Doctype { name, public_id, system_id } => {
                out.push_str("<!DOCTYPE ");
                out.push_str(name);
                if !public_id.is_empty() {
                    out.push_str(&format!(" PUBLIC \"{public_id}\""));
                }
                if !system_id.is_empty() {
                    if public_id.is_empty() {
                        out.push_str(" SYSTEM");
                    }
                    out.push_str(&format!(" \"{system_id}\""));
                }
                out.push('>');
            }
            NodeData::Text(t) => out.push_str(&esc(t, false)),
            NodeData::Comment(t) => out.push_str(&format!("<!--{t}-->")),
            NodeData::ProcessingInstruction { target, data } => out.push_str(&format!("<?{target} {data}?>")),
            NodeData::Element(e) => {
                let (ns, prefix) = element_ns(d, id);
                let q = match &prefix {
                    Some(p) => format!("{p}:{}", e.local),
                    None => e.local.clone(),
                };
                out.push('<');
                out.push_str(&q);
                let ns_opt = if ns.is_empty() { None } else { Some(ns.as_str()) };
                if ns_opt != ctx_ns && prefix.is_none() && !e.attrs.iter().any(|a| a.ns == Namespace::Xmlns && a.local == "xmlns") {
                    out.push_str(&format!(" xmlns=\"{}\"", esc(ns_opt.unwrap_or(""), true)));
                }
                for a in &e.attrs {
                    out.push(' ');
                    out.push_str(&attr_qname(a));
                    out.push_str("=\"");
                    out.push_str(&esc(&a.value, true));
                    out.push('"');
                }
                let kids: Vec<NodeId> = match e.template_contents {
                    Some(t) => d.children(t).collect(),
                    None => d.children(NodeId(id)).collect(),
                };
                let html = ns == Namespace::Html.url();
                if kids.is_empty() {
                    if html && html_core::serialize::serializes_as_void(&e.local) {
                        out.push_str(" />");
                        return;
                    }
                    if !html {
                        out.push_str("/>");
                        return;
                    }
                }
                out.push('>');
                for k in kids {
                    walk(d, k.0, ns_opt, out);
                }
                out.push_str(&format!("</{q}>"));
            }
        }
    }
    with_doc(|d| {
        let mut out = String::new();
        walk(d, id, None, &mut out);
        out
    })
}

fn xml_serialize_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    match node_of(vm, &v) {
        Some(n) => Ok(s(&xml_serialize(n))),
        None => vm.throw_type("Failed to execute 'serializeToString' on 'XMLSerializer': parameter 1 is not of type 'Node'."),
    }
}

pub fn serialize_inner(id: usize) -> String {
    with_doc(|d| html_core::serialize::inner_html(d, NodeId(id), html_core::serialize::SerializeOpts { scripting: true }))
}

pub fn serialize_outer(id: usize) -> String {
    with_doc(|d| html_core::serialize::outer_html(d, NodeId(id), html_core::serialize::SerializeOpts { scripting: true }))
}

fn inner_html_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_kind(vm, ctx, &[NK::Element, NK::Fragment, NK::Document])?;
    // <template>: its contents.
    let target = with_doc(|d| d.element(NodeId(n)).and_then(|e| e.template_contents).map(|t| t.0)).unwrap_or(n);
    Ok(s(&serialize_inner(target)))
}

fn inner_html_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_kind(vm, ctx, &[NK::Element, NK::Fragment])?;
    let markup = string_null_empty(vm, &arg(vm, ctx, 0))?;
    let context = if kind(n) == NK::Element { n } else { create_element(node_document(n), Namespace::Html, "body") };
    let frag = parse_fragment(context, &markup);
    let target = with_doc(|d| d.element(NodeId(n)).and_then(|e| e.template_contents).map(|t| t.0)).unwrap_or(n);
    replace_all(vm, Some(frag), target);
    Ok(Value::Undefined)
}

fn outer_html_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_element(vm, ctx)?;
    Ok(s(&serialize_outer(n)))
}

fn outer_html_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_element(vm, ctx)?;
    let markup = string_null_empty(vm, &arg(vm, ctx, 0))?;
    let Some(parent) = parent_of(n) else { return Ok(Value::Undefined) };
    if kind(parent) == NK::Document {
        return throw_dom(vm, "NoModificationAllowedError", "This element's parent is of type '#document'.");
    }
    let context = if kind(parent) == NK::Fragment { create_element(node_document(n), Namespace::Html, "body") } else { parent };
    let frag = parse_fragment(context, &markup);
    replace_child(vm, n, frag, parent)?;
    Ok(Value::Undefined)
}

fn insert_adjacent_html(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 2, "Element", "insertAdjacentHTML")?;
    let w = string(vm, &arg(vm, ctx, 0))?.to_ascii_lowercase();
    let markup = string(vm, &arg(vm, ctx, 1))?;
    let context = match w.as_str() {
        "beforebegin" | "afterend" => {
            let p = parent_of(el);
            match p {
                Some(p) if kind(p) == NK::Element => p,
                Some(p) if kind(p) == NK::Document => {
                    return throw_dom(vm, "NoModificationAllowedError", "The element has no parent.");
                }
                _ => return throw_dom(vm, "NoModificationAllowedError", "The element has no parent."),
            }
        }
        "afterbegin" | "beforeend" => el,
        _ => return throw_dom(vm, "SyntaxError", &format!("'{w}' is not a valid position.")),
    };
    let context = if with_doc(|d| is_html_tag(d, context, "html")) && is_html_doc(node_document(context)) {
        create_element(node_document(el), Namespace::Html, "body")
    } else {
        context
    };
    let frag = parse_fragment(context, &markup);
    insert_adjacent(vm, el, &w, frag)?;
    Ok(Value::Undefined)
}

// =================================================================================================
// Document
// =================================================================================================

fn this_doc(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    this_kind(vm, ctx, &[NK::Document])
}

fn document_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'Document': Please use the 'new' operator");
    }
    let id = new_document(false, "application/xml", &super::page_url());
    Ok(wrap(vm, id))
}

/// A new (detached) Document node in the page arena.
pub fn new_document(html: bool, content_type: &str, url: &str) -> usize {
    let id = with_doc_mut(|d| d.create(NodeData::Document).0);
    page(|p| p.docs.insert(id, super::DocMeta { html, content_type: content_type.to_string(), url: url.to_string(), quirks: false }));
    id
}

fn doc_url(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let u = page(|p| p.docs.get(&d).map(|m| m.url.clone())).unwrap_or_else(super::page_url);
    Ok(s(if u.is_empty() { "about:blank" } else { &u }))
}

fn compat_mode(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let quirks = if d == main_doc() {
        with_doc(|doc| doc.quirks_mode == html_core::QuirksMode::Quirks)
    } else {
        page(|p| p.docs.get(&d).is_some_and(|m| m.quirks))
    };
    Ok(s(if quirks { "BackCompat" } else { "CSS1Compat" }))
}

fn character_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_doc(vm, ctx)?;
    Ok(s("UTF-8"))
}

fn content_type(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    Ok(s(&page(|p| p.docs.get(&d).map(|m| m.content_type.clone())).unwrap_or_else(|| "text/html".into())))
}

fn doctype_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let dt = with_doc(|doc| has_doctype_child(doc, d));
    Ok(wrap_opt(vm, dt))
}

pub fn document_element_of(d: usize) -> Option<usize> {
    with_doc(|doc| doc.children(NodeId(d)).find(|c| doc.element(*c).is_some()).map(|c| c.0))
}

fn document_element(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let e = document_element_of(d);
    Ok(wrap_opt(vm, e))
}

/// The body element (HTML §3.1.4): the first `body`/`frameset` child of the html element.
pub fn body_of(d: usize) -> Option<usize> {
    let html = document_element_of(d)?;
    with_doc(|doc| {
        if !is_html_tag(doc, html, "html") {
            return None;
        }
        doc.children(NodeId(html)).find(|c| is_html_tag(doc, c.0, "body") || is_html_tag(doc, c.0, "frameset")).map(|c| c.0)
    })
}

pub fn head_of(d: usize) -> Option<usize> {
    let html = document_element_of(d)?;
    with_doc(|doc| doc.children(NodeId(html)).find(|c| is_html_tag(doc, c.0, "head")).map(|c| c.0))
}

fn body_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let b = body_of(d);
    Ok(wrap_opt(vm, b))
}

fn body_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    let ok = node_of(vm, &v).filter(|&n| with_doc(|doc| is_html_tag(doc, n, "body") || is_html_tag(doc, n, "frameset")));
    let Some(newb) = ok else {
        return throw_dom(vm, "HierarchyRequestError", "The new body element is of type 'BODY' or 'FRAMESET' only.");
    };
    match body_of(d) {
        Some(old) if old == newb => {}
        Some(old) => {
            let p = parent_of(old).unwrap();
            replace_child(vm, old, newb, p)?;
        }
        None => match document_element_of(d) {
            Some(html) => {
                pre_insert(vm, newb, html, None)?;
            }
            None => return throw_dom(vm, "HierarchyRequestError", "No document element."),
        },
    }
    Ok(Value::Undefined)
}

fn head_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let h = head_of(d);
    Ok(wrap_opt(vm, h))
}

fn title_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let t = with_doc(|doc| {
        doc.query_first(NodeId(d), |doc, i| doc.element(i).is_some_and(|e| e.local == "title"))
            .map(|i| doc.text_content(i))
            .unwrap_or_default()
    });
    // strip and collapse ASCII whitespace
    let t = t.split(|c: char| matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    Ok(s(&t))
}

fn title_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    let existing = with_doc(|doc| doc.query_first(NodeId(d), |doc, i| doc.element(i).is_some_and(|e| e.is_html("title"))).map(|i| i.0));
    let el = match existing {
        Some(e) => e,
        None => {
            let Some(head) = head_of(d) else { return Ok(Value::Undefined) };
            let t = create_element(d, Namespace::Html, "title");
            insert(vm, t, head, None);
            t
        }
    };
    let node = if v.is_empty() { None } else { Some(create_text(d, v)) };
    replace_all(vm, node, el);
    Ok(Value::Undefined)
}

fn create_element_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    need(vm, ctx, 1, "Document", "createElement")?;
    let l = string(vm, &arg(vm, ctx, 0))?;
    if !is_valid_name(&l) {
        return throw_dom(vm, "InvalidCharacterError", &format!("The tag name provided ('{l}') is not a valid name."));
    }
    let html = is_html_doc(d);
    let l = if html { l.to_ascii_lowercase() } else { l };
    let ns = if html || page(|p| p.docs.get(&d).is_some_and(|m| m.content_type == "application/xhtml+xml")) {
        Namespace::Html
    } else {
        Namespace::None
    };
    let id = create_element(d, ns, &l);
    Ok(wrap(vm, id))
}

/// "create an element" for a namespace (createElementNS).
pub fn create_element_ns(vm: &mut Vm, d: usize, ns: Option<String>, q: &str) -> JsResult<usize> {
    let (ns, prefix, local) = validate_and_extract(vm, ns, q)?;
    let id = match ns_from_url(&ns) {
        Some(n) if prefix.is_none() => create_element(d, n, &local),
        Some(n) => {
            let id = create_element(d, n, &local);
            page(|p| p.ns_override.insert(id, (ns.clone(), prefix.clone())));
            id
        }
        None => {
            let id = create_element(d, Namespace::None, &local);
            page(|p| p.ns_override.insert(id, (ns.clone(), prefix.clone())));
            id
        }
    };
    Ok(id)
}

fn create_element_ns_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    need(vm, ctx, 2, "Document", "createElementNS")?;
    let ns = opt_string(vm, &arg(vm, ctx, 0))?;
    let q = string(vm, &arg(vm, ctx, 1))?;
    let id = create_element_ns(vm, d, ns, &q)?;
    Ok(wrap(vm, id))
}

fn create_document_fragment(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let id = with_doc_mut(|doc| doc.create(NodeData::DocumentFragment).0);
    set_node_doc(id, d);
    Ok(wrap(vm, id))
}

fn fragment_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'DocumentFragment': Please use the 'new' operator");
    }
    let id = with_doc_mut(|doc| doc.create(NodeData::DocumentFragment).0);
    Ok(wrap(vm, id))
}

fn create_text_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    need(vm, ctx, 1, "Document", "createTextNode")?;
    let t = string(vm, &arg(vm, ctx, 0))?;
    let id = create_text(d, t);
    Ok(wrap(vm, id))
}

fn create_comment_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    need(vm, ctx, 1, "Document", "createComment")?;
    let t = string(vm, &arg(vm, ctx, 0))?;
    let id = create_comment(d, t);
    Ok(wrap(vm, id))
}

fn create_cdata_section(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    if is_html_doc(d) {
        return throw_dom(vm, "NotSupportedError", "This operation is not supported for HTML documents.");
    }
    crate::ledger::record_dom("createCDATASection-as-text");
    let t = string(vm, &arg(vm, ctx, 0))?;
    if t.contains("]]>") {
        return throw_dom(vm, "InvalidCharacterError", "String cannot contain ']]>'.");
    }
    let id = create_text(d, t);
    Ok(wrap(vm, id))
}

fn create_processing_instruction(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    need(vm, ctx, 2, "Document", "createProcessingInstruction")?;
    let target = string(vm, &arg(vm, ctx, 0))?;
    let data = string(vm, &arg(vm, ctx, 1))?;
    if !is_valid_name(&target) || data.contains("?>") {
        return throw_dom(vm, "InvalidCharacterError", "Invalid processing instruction.");
    }
    let id = with_doc_mut(|doc| doc.create(NodeData::ProcessingInstruction { target, data }).0);
    set_node_doc(id, d);
    Ok(wrap(vm, id))
}

fn import_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    need(vm, ctx, 1, "Document", "importNode")?;
    let n = node_arg(vm, ctx, 0, "Document", "importNode")?;
    if kind(n) == NK::Document {
        return throw_dom(vm, "NotSupportedError", "The node provided is a document, which may not be imported.");
    }
    let deep = boolean(vm, &arg(vm, ctx, 1));
    let c = clone_node(n, deep, Some(d));
    Ok(wrap(vm, c))
}

fn adopt_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    need(vm, ctx, 1, "Document", "adoptNode")?;
    let n = node_arg(vm, ctx, 0, "Document", "adoptNode")?;
    if kind(n) == NK::Document {
        return throw_dom(vm, "NotSupportedError", "The node provided is a document, which may not be adopted.");
    }
    if let Some(p) = parent_of(n) {
        with_doc_mut(|doc| doc.detach(NodeId(n)));
        touch();
        super::loader::children_changed(vm, p);
    }
    adopt(n, d);
    Ok(wrap(vm, n))
}

fn create_attribute(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    need(vm, ctx, 1, "Document", "createAttribute")?;
    let l = string(vm, &arg(vm, ctx, 0))?;
    if !is_valid_name(&l) {
        return throw_dom(vm, "InvalidCharacterError", &format!("'{l}' is not a valid attribute name."));
    }
    let l = if is_html_doc(d) { l.to_ascii_lowercase() } else { l };
    Ok(Value::Object(new_attr(vm, None, "", None, &l, "")))
}

fn create_attribute_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_doc(vm, ctx)?;
    need(vm, ctx, 2, "Document", "createAttributeNS")?;
    let ns = opt_string(vm, &arg(vm, ctx, 0))?;
    let q = string(vm, &arg(vm, ctx, 1))?;
    let (ns, prefix, local) = validate_and_extract(vm, ns, &q)?;
    Ok(Value::Object(new_attr(vm, None, &ns, prefix.as_deref(), &local, "")))
}

fn get_element_by_id(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = this_kind(vm, ctx, &[NK::Document, NK::Fragment])?;
    need(vm, ctx, 1, "Document", "getElementById")?;
    let id = string(vm, &arg(vm, ctx, 0))?;
    if id.is_empty() {
        return Ok(Value::Null);
    }
    let hit = with_doc(|d| d.query_first(NodeId(n), |d, i| d.element_id(i) == Some(id.as_str())).map(|i| i.0));
    Ok(wrap_opt(vm, hit))
}

fn get_elements_by_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    need(vm, ctx, 1, "Document", "getElementsByName")?;
    let name = string(vm, &arg(vm, ctx, 0))?;
    Ok(Value::Object(make_collection(vm, CK_NAME, d, &name, "")))
}

fn doc_collection(vm: &mut Vm, ctx: &CallCtx, which: u8, filter: &str) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    let f = filter.to_string();
    Ok(cached(vm, d, which, |vm| make_collection(vm, CK_DOCFILTER, d, &f, "")))
}
fn doc_forms(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    doc_collection(vm, ctx, SO_FORMS, "forms")
}
fn doc_images(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    doc_collection(vm, ctx, SO_IMAGES, "images")
}
fn doc_links(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    doc_collection(vm, ctx, SO_LINKS, "links")
}
fn doc_scripts(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    doc_collection(vm, ctx, SO_SCRIPTS, "scripts")
}
fn doc_anchors(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    doc_collection(vm, ctx, SO_ANCHORS, "anchors")
}
fn doc_embeds(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    doc_collection(vm, ctx, SO_EMBEDS, "embeds")
}

fn implementation(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = this_doc(vm, ctx)?;
    Ok(cached(vm, d, SO_IMPL, |vm| {
        let p = iface("DOMImplementation").unwrap().proto;
        host_obj(vm, p, T_IMPL, vec![num(d as f64)])
    }))
}

fn impl_doc(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    match this_tagged(vm, &ctx.this, T_IMPL) {
        Some(o) => Ok(slot_num(vm, o, 0) as usize),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn impl_create_document_type(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = impl_doc(vm, ctx)?;
    need(vm, ctx, 3, "DOMImplementation", "createDocumentType")?;
    let name = string(vm, &arg(vm, ctx, 0))?;
    let public_id = string(vm, &arg(vm, ctx, 1))?;
    let system_id = string(vm, &arg(vm, ctx, 2))?;
    if name.chars().any(|c| matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ' | '\0' | '>')) {
        return throw_dom(vm, "InvalidCharacterError", "The doctype name is not valid.");
    }
    let id = with_doc_mut(|doc| doc.create(NodeData::Doctype { name, public_id, system_id }).0);
    set_node_doc(id, d);
    Ok(wrap(vm, id))
}

fn impl_create_document(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    impl_doc(vm, ctx)?;
    need(vm, ctx, 2, "DOMImplementation", "createDocument")?;
    let ns = opt_string(vm, &arg(vm, ctx, 0))?;
    let q = string_null_empty(vm, &arg(vm, ctx, 1))?;
    let dt = arg(vm, ctx, 2);
    let ct = match ns.as_deref() {
        Some("http://www.w3.org/1999/xhtml") => "application/xhtml+xml",
        Some("http://www.w3.org/2000/svg") => "image/svg+xml",
        _ => "application/xml",
    };
    let doc = new_document(false, ct, "about:blank");
    if let Some(dtn) = node_of(vm, &dt) {
        adopt(dtn, doc);
        with_doc_mut(|d| d.append(NodeId(doc), NodeId(dtn)));
    }
    if !q.is_empty() {
        let el = create_element_ns(vm, doc, ns, &q)?;
        with_doc_mut(|d| d.append(NodeId(doc), NodeId(el)));
    }
    Ok(wrap(vm, doc))
}

fn impl_create_html_document(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    impl_doc(vm, ctx)?;
    let title = arg(vm, ctx, 0);
    let title = if title.is_undefined() { None } else { Some(string(vm, &title)?) };
    let doc = new_document(true, "text/html", "about:blank");
    let dt = with_doc_mut(|d| d.create(NodeData::Doctype { name: "html".into(), public_id: String::new(), system_id: String::new() }).0);
    set_node_doc(dt, doc);
    with_doc_mut(|d| d.append(NodeId(doc), NodeId(dt)));
    let html = create_element(doc, Namespace::Html, "html");
    let head = create_element(doc, Namespace::Html, "head");
    let body = create_element(doc, Namespace::Html, "body");
    with_doc_mut(|d| {
        d.append(NodeId(doc), NodeId(html));
        d.append(NodeId(html), NodeId(head));
    });
    if let Some(t) = title {
        let te = create_element(doc, Namespace::Html, "title");
        let tx = create_text(doc, t);
        with_doc_mut(|d| {
            d.append(NodeId(head), NodeId(te));
            d.append(NodeId(te), NodeId(tx));
        });
    }
    with_doc_mut(|d| d.append(NodeId(html), NodeId(body)));
    Ok(wrap(vm, doc))
}

fn impl_has_feature(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(true))
}

// =================================================================================================
// Attr / NamedNodeMap
// =================================================================================================

// Attr slots: 0 owner element id (-1 = none), 1 namespace, 2 prefix (or null), 3 local name, 4 value
// (used while detached).

pub fn new_attr(vm: &mut Vm, owner: Option<usize>, ns: &str, prefix: Option<&str>, local: &str, value: &str) -> Obj {
    let p = iface("Attr").unwrap().proto;
    host_obj(
        vm,
        p,
        T_ATTR,
        vec![num(owner.map(|o| o as f64).unwrap_or(-1.0)), s(ns), prefix.map(s).unwrap_or(Value::Null), s(local), s(value)],
    )
}

/// The Attr object for attribute index `i` of `el` (one object per attribute while it is attached).
pub fn attr_object(vm: &mut Vm, el: usize, i: usize) -> Option<Value> {
    let a = attr_at(el, i)?;
    let key = (el, format!("{}|{}", a.ns.url(), a.local));
    if let Some((o, _)) = page(|p| p.attrs.get(&key).copied()) {
        return Some(Value::Object(o));
    }
    let o = new_attr(vm, Some(el), a.ns.url(), a.prefix, &a.local, &a.value);
    let r = root(vm, Value::Object(o));
    page(|p| p.attrs.insert(key, (o, r)));
    Some(Value::Object(o))
}

fn this_attr(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Obj> {
    match this_tagged(vm, &ctx.this, T_ATTR) {
        Some(o) => Ok(o),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn attr_owner(vm: &Vm, o: Obj) -> Option<usize> {
    let n = slot_num(vm, o, 0);
    (n >= 0.0).then_some(n as usize)
}

fn attr_str(vm: &Vm, o: Obj, i: usize) -> String {
    match slot(vm, o, i) {
        Value::String(s) => s.to_rust(),
        _ => String::new(),
    }
}

fn attr_index_of_obj(vm: &Vm, o: Obj) -> Option<(usize, usize)> {
    let el = attr_owner(vm, o)?;
    let ns = attr_str(vm, o, 1);
    let local = attr_str(vm, o, 3);
    find_attr_ns(el, &ns, &local).map(|i| (el, i))
}

fn attr_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_attr(vm, ctx)?;
    if let Some((el, i)) = attr_index_of_obj(vm, o) {
        return Ok(s(&attr_at(el, i).map(|a| a.value).unwrap_or_default()));
    }
    Ok(slot(vm, o, 4))
}

fn attr_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_attr(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    if let Some((el, i)) = attr_index_of_obj(vm, o) {
        let (old, local, ns) = with_doc_mut(|d| {
            let a = &mut d.element_mut(NodeId(el)).unwrap().attrs[i];
            (std::mem::replace(&mut a.value, v.clone()), a.local.clone(), a.ns)
        });
        attribute_changed(vm, el, &local, ns, Some(old), Some(v));
    } else {
        set_slot(vm, o, 4, s(&v));
    }
    Ok(Value::Undefined)
}

fn attr_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_attr(vm, ctx)?;
    let local = attr_str(vm, o, 3);
    Ok(match slot(vm, o, 2) {
        Value::String(p) => s(&format!("{}:{local}", p.to_rust())),
        _ => s(&local),
    })
}

fn attr_local_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_attr(vm, ctx)?;
    Ok(slot(vm, o, 3))
}

fn attr_namespace(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_attr(vm, ctx)?;
    let ns = attr_str(vm, o, 1);
    Ok(if ns.is_empty() { Value::Null } else { s(&ns) })
}

fn attr_prefix(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_attr(vm, ctx)?;
    Ok(slot(vm, o, 2))
}

fn attr_owner_element(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_attr(vm, ctx)?;
    if attr_index_of_obj(vm, o).is_some() {
        let el = attr_owner(vm, o);
        return Ok(wrap_opt(vm, el));
    }
    Ok(Value::Null)
}

fn attr_specified(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(true))
}

fn get_attribute_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "Element", "getAttributeNode")?;
    let q = string(vm, &arg(vm, ctx, 0))?;
    Ok(find_attr_qname(el, &q).and_then(|i| attr_object(vm, el, i)).unwrap_or(Value::Null))
}

fn get_attribute_node_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 2, "Element", "getAttributeNodeNS")?;
    let ns = opt_string(vm, &arg(vm, ctx, 0))?.unwrap_or_default();
    let l = string(vm, &arg(vm, ctx, 1))?;
    Ok(find_attr_ns(el, &ns, &l).and_then(|i| attr_object(vm, el, i)).unwrap_or(Value::Null))
}

/// `setAttributeNode` / `setNamedItem`: DOM "set an attribute".
fn set_attribute_node_impl(vm: &mut Vm, el: usize, a: Obj) -> JsResult<Value> {
    if let Some(owner) = attr_owner(vm, a) {
        if attr_index_of_obj(vm, a).is_some() {
            if owner != el {
                return throw_dom(vm, "InUseAttributeError", "The attribute is in use by another element.");
            }
            return Ok(Value::Object(a));
        }
    }
    let ns = attr_str(vm, a, 1);
    let local = attr_str(vm, a, 3);
    let prefix = match slot(vm, a, 2) {
        Value::String(p) => Some(p.to_rust()),
        _ => None,
    };
    let value = attr_str(vm, a, 4);
    let nse = ns_from_url(&ns).unwrap_or(Namespace::None);
    let old_obj = find_attr_ns(el, &ns, &local).and_then(|i| attr_object(vm, el, i));
    let old = with_doc_mut(|d| {
        let e = d.element_mut(NodeId(el)).unwrap();
        match e.attrs.iter_mut().find(|x| x.ns == nse && x.local == local) {
            Some(x) => Some(std::mem::replace(&mut x.value, value.clone())),
            None => {
                e.attrs.push(Attribute { prefix: prefix.as_deref().map(intern), ns: nse, local: local.clone(), value: value.clone() });
                None
            }
        }
    });
    // The old Attr (if any) becomes detached with its old value; `a` becomes the attribute's object.
    let key = (el, format!("{}|{}", nse.url(), local));
    if let Some(Value::Object(oo)) = &old_obj {
        set_slot(vm, *oo, 0, num(-1.0));
        set_slot(vm, *oo, 4, s(old.as_deref().unwrap_or("")));
    }
    set_slot(vm, a, 0, num(el as f64));
    let r = root(vm, Value::Object(a));
    if let Some((_, oldr)) = page(|p| p.attrs.insert(key, (a, r))) {
        unroot(vm, oldr);
    }
    touch();
    if nse == Namespace::None && local.starts_with("on") {
        super::events::handler_attribute_changed(vm, el, &local[2..], true);
    }
    Ok(old_obj.unwrap_or(Value::Null))
}

fn set_attribute_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "Element", "setAttributeNode")?;
    let v = arg(vm, ctx, 0);
    let Some(a) = v.as_object().filter(|o| tag_of(vm, *o) == Some(T_ATTR)) else {
        return vm.throw_type("Failed to execute 'setAttributeNode' on 'Element': parameter 1 is not of type 'Attr'.");
    };
    set_attribute_node_impl(vm, el, a)
}

fn remove_attribute_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    need(vm, ctx, 1, "Element", "removeAttributeNode")?;
    let v = arg(vm, ctx, 0);
    let Some(a) = v.as_object().filter(|o| tag_of(vm, *o) == Some(T_ATTR)) else {
        return vm.throw_type("Failed to execute 'removeAttributeNode' on 'Element': parameter 1 is not of type 'Attr'.");
    };
    match attr_index_of_obj(vm, a) {
        Some((owner, i)) if owner == el => {
            let at = with_doc_mut(|d| d.element_mut(NodeId(el)).unwrap().attrs.remove(i));
            attribute_changed(vm, el, &at.local, at.ns, Some(at.value), None);
            Ok(Value::Object(a))
        }
        _ => throw_dom(vm, "NotFoundError", "The attribute provided is not owned by this element."),
    }
}

// =================================================================================================
// Collections (NodeList, HTMLCollection, NamedNodeMap, DOMTokenList) as proxies
// =================================================================================================

pub const CK_CHILDNODES: u8 = 0;
pub const CK_CHILDREN: u8 = 1;
pub const CK_TAGNAME: u8 = 2;
pub const CK_TAGNAME_NS: u8 = 3;
pub const CK_CLASSNAME: u8 = 4;
pub const CK_STATIC: u8 = 5;
pub const CK_ATTRMAP: u8 = 6;
pub const CK_NAME: u8 = 7;
pub const CK_DOCFILTER: u8 = 8;
pub const CK_TOKENS: u8 = 9;
pub const CK_SELECTOR: u8 = 10;

thread_local! {
    static COLL_HANDLER: std::cell::Cell<Option<Obj>> = const { std::cell::Cell::new(None) };
}

fn coll_proto(k: u8) -> Obj {
    let name = match k {
        CK_CHILDNODES | CK_STATIC | CK_NAME => "NodeList",
        CK_ATTRMAP => "NamedNodeMap",
        CK_TOKENS => "DOMTokenList",
        _ => "HTMLCollection",
    };
    iface(name).unwrap().proto
}

/// A live collection proxy. `a`/`b` are the kind's string arguments (tag name, classes, attribute).
pub fn make_collection(vm: &mut Vm, k: u8, root_id: usize, a: &str, b: &str) -> Obj {
    let p = coll_proto(k);
    let target = host_obj(vm, p, T_COLL, vec![num(k as f64), num(root_id as f64), s(a), s(b)]);
    proxy_for(vm, target)
}

fn proxy_for(vm: &mut Vm, target: Obj) -> Obj {
    let handler = COLL_HANDLER.with(|h| h.get()).expect("collection handler installed");
    vm.alloc(ObjectData::new(
        None,
        Kind::Proxy(Some(Box::new(ProxyData { target, handler, callable: false, ctor: false, revoked: false }))),
    ))
}

/// A static NodeList (querySelectorAll).
pub fn static_node_list(vm: &mut Vm, ids: Vec<usize>) -> Value {
    let arr = vm.new_array(ids.into_iter().map(|i| num(i as f64)).collect());
    let p = coll_proto(CK_STATIC);
    let target = host_obj(vm, p, T_COLL, vec![num(CK_STATIC as f64), Value::Object(arr), s(""), s("")]);
    Value::Object(proxy_for(vm, target))
}

fn coll_kind(vm: &Vm, t: Obj) -> u8 {
    slot_num(vm, t, 0) as u8
}

fn coll_str(vm: &Vm, t: Obj, i: usize) -> String {
    match slot(vm, t, i) {
        Value::String(s) => s.to_rust(),
        _ => String::new(),
    }
}

/// ASCII-whitespace-split set (HTML "split on ASCII whitespace", order kept, duplicates dropped).
pub fn token_set(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in s.split(|c: char| matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')).filter(|t| !t.is_empty()) {
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// The current members of collection target `t` (node ids; for DOMTokenList, nothing — see tokens).
fn coll_items(vm: &Vm, t: Obj) -> Vec<usize> {
    let k = coll_kind(vm, t);
    if k == CK_STATIC {
        if let Value::Object(arr) = slot(vm, t, 1) {
            return vm.array_to_list(&Value::Object(arr)).into_iter().filter_map(|v| match v {
                Value::Number(n) => Some(n as usize),
                _ => None,
            }).collect();
        }
        return Vec::new();
    }
    let root = slot_num(vm, t, 1) as usize;
    let a = coll_str(vm, t, 2);
    let b = coll_str(vm, t, 3);
    with_doc(|d| match k {
        CK_CHILDNODES => d.children(NodeId(root)).map(|c| c.0).collect(),
        CK_CHILDREN => d.element_children(NodeId(root)).map(|c| c.0).collect(),
        CK_TAGNAME => {
            let all = a == "*";
            let lower = a.to_ascii_lowercase();
            let html_doc = is_html_doc(node_document(root));
            d.descendants(NodeId(root))
                .filter(|&i| {
                    if d.element(i).is_none() {
                        return false;
                    }
                    if all {
                        return true;
                    }
                    let q = qualified_name(d, i.0);
                    if html_doc && is_html_element(d, i.0) { q == lower } else { q == a }
                })
                .map(|i| i.0)
                .collect()
        }
        CK_TAGNAME_NS => d
            .descendants(NodeId(root))
            .filter(|&i| {
                d.element(i).is_some() && {
                    let (ns, _) = element_ns(d, i.0);
                    (a == "*" || ns == a) && (b == "*" || local_name_in(d, i.0) == b)
                }
            })
            .map(|i| i.0)
            .collect(),
        CK_CLASSNAME => {
            let want = token_set(&a);
            if want.is_empty() {
                return Vec::new();
            }
            let quirks = d.quirks_mode == html_core::QuirksMode::Quirks && node_document(root) == main_doc();
            d.descendants(NodeId(root))
                .filter(|&i| {
                    d.element(i).is_some_and(|e| {
                        let have = token_set(e.attr("class").unwrap_or(""));
                        want.iter().all(|w| {
                            have.iter().any(|h| if quirks { h.eq_ignore_ascii_case(w) } else { h == w })
                        })
                    })
                })
                .map(|i| i.0)
                .collect()
        }
        CK_NAME => d
            .descendants(NodeId(root))
            .filter(|&i| d.element(i).is_some_and(|e| e.ns == Namespace::Html && e.attr("name") == Some(a.as_str())))
            .map(|i| i.0)
            .collect(),
        CK_DOCFILTER if a == "rows" => {
            // HTMLTableElement.rows (HTML §4.9.1): thead rows, then tbody/direct rows in tree order, then
            // tfoot rows.
            let html_child = |p: NodeId, tag: &str| -> Vec<NodeId> {
                d.children(p).filter(|c| d.element(*c).is_some_and(|e| e.is_html(tag))).collect()
            };
            let mut out = Vec::new();
            for h in html_child(NodeId(root), "thead") {
                out.extend(html_child(h, "tr").into_iter().map(|x| x.0));
            }
            for c in d.children(NodeId(root)) {
                if d.element(c).is_some_and(|e| e.is_html("tbody")) {
                    out.extend(html_child(c, "tr").into_iter().map(|x| x.0));
                } else if d.element(c).is_some_and(|e| e.is_html("tr")) {
                    out.push(c.0);
                }
            }
            for f in html_child(NodeId(root), "tfoot") {
                out.extend(html_child(f, "tr").into_iter().map(|x| x.0));
            }
            out
        }
        CK_DOCFILTER if matches!(a.as_str(), "srows" | "cells" | "tbodies") => d
            .children(NodeId(root))
            .filter(|c| {
                d.element(*c).is_some_and(|e| {
                    e.ns == Namespace::Html
                        && match a.as_str() {
                            "srows" => e.local == "tr",
                            "cells" => e.local == "td" || e.local == "th",
                            _ => e.local == "tbody",
                        }
                })
            })
            .map(|c| c.0)
            .collect(),
        CK_DOCFILTER if a == "options" => d
            .descendants(NodeId(root))
            .filter(|i| d.element(*i).is_some_and(|e| e.is_html("option")))
            .map(|i| i.0)
            .collect(),
        CK_DOCFILTER => d
            .descendants(NodeId(root))
            .filter(|&i| {
                d.element(i).is_some_and(|e| {
                    e.ns == Namespace::Html
                        && match a.as_str() {
                            "forms" => e.local == "form",
                            "images" => e.local == "img",
                            "scripts" => e.local == "script",
                            "embeds" => e.local == "embed",
                            "links" => (e.local == "a" || e.local == "area") && e.attr("href").is_some(),
                            "anchors" => e.local == "a" && e.attr("name").is_some(),
                            _ => false,
                        }
                })
            })
            .map(|i| i.0)
            .collect(),
        _ => Vec::new(),
    })
}

/// DOMTokenList members (the attribute's ordered set).
fn tokens_of(vm: &Vm, t: Obj) -> Vec<String> {
    let el = slot_num(vm, t, 1) as usize;
    let attr = coll_str(vm, t, 2);
    token_set(&attr_value(el, &attr).unwrap_or_default())
}

/// Named properties of an HTMLCollection (HTML §2.7.2.1 "supported property names") in order.
fn coll_names(vm: &Vm, t: Obj, items: &[usize]) -> Vec<(String, usize)> {
    let k = coll_kind(vm, t);
    if matches!(k, CK_CHILDNODES | CK_STATIC | CK_NAME | CK_TOKENS) {
        return Vec::new();
    }
    if k == CK_ATTRMAP {
        let el = slot_num(vm, t, 1) as usize;
        return with_doc(|d| {
            d.element(NodeId(el))
                .map(|e| {
                    let mut v: Vec<(String, usize)> = Vec::new();
                    let lower = is_html_element(d, el) && is_html_doc(node_document(el));
                    for (i, a) in e.attrs.iter().enumerate() {
                        let q = attr_qname(a);
                        if lower && q.chars().any(|c| c.is_ascii_uppercase()) {
                            continue;
                        }
                        if !v.iter().any(|(n, _)| *n == q) {
                            v.push((q, i));
                        }
                    }
                    v
                })
                .unwrap_or_default()
        });
    }
    with_doc(|d| {
        let mut v: Vec<(String, usize)> = Vec::new();
        for &i in items {
            if let Some(e) = d.element(NodeId(i)) {
                if let Some(id) = e.attr("id").filter(|x| !x.is_empty()) {
                    if !v.iter().any(|(n, _)| n == id) {
                        v.push((id.to_string(), i));
                    }
                }
                if e.ns == Namespace::Html {
                    if let Some(nm) = e.attr("name").filter(|x| !x.is_empty()) {
                        if !v.iter().any(|(n, _)| n == nm) {
                            v.push((nm.to_string(), i));
                        }
                    }
                }
            }
        }
        v
    })
}

fn coll_len(vm: &Vm, t: Obj) -> usize {
    match coll_kind(vm, t) {
        CK_ATTRMAP => {
            let el = slot_num(vm, t, 1) as usize;
            with_doc(|d| d.element(NodeId(el)).map(|e| e.attrs.len()).unwrap_or(0))
        }
        CK_TOKENS => tokens_of(vm, t).len(),
        _ => coll_items(vm, t).len(),
    }
}

/// Item `i` of a collection as a JS value (undefined when out of range).
fn coll_item(vm: &mut Vm, t: Obj, i: usize) -> Option<Value> {
    match coll_kind(vm, t) {
        CK_ATTRMAP => {
            let el = slot_num(vm, t, 1) as usize;
            attr_object(vm, el, i)
        }
        CK_TOKENS => tokens_of(vm, t).get(i).map(|x| s(x)),
        _ => {
            let items = coll_items(vm, t);
            items.get(i).copied().map(|id| wrap(vm, id))
        }
    }
}

fn coll_named(vm: &mut Vm, t: Obj, name: &str) -> Option<Value> {
    if coll_kind(vm, t) == CK_ATTRMAP {
        let el = slot_num(vm, t, 1) as usize;
        let i = find_attr_qname(el, name)?;
        return attr_object(vm, el, i);
    }
    let items = coll_items(vm, t);
    let names = coll_names(vm, t, &items);
    let hit = names.iter().find(|(n, _)| n == name).map(|(_, i)| *i)?;
    Some(wrap(vm, hit))
}

fn trap_target(vm: &Vm, ctx: &CallCtx) -> Option<Obj> {
    let t = vm.arg(ctx, 0).as_object()?;
    (tag_of(vm, t) == Some(T_COLL)).then_some(t)
}

fn key_index(k: &PropertyKey) -> Option<usize> {
    match k {
        PropertyKey::Index(i) => Some(*i as usize),
        _ => None,
    }
}

fn key_name(k: &PropertyKey) -> Option<String> {
    match k {
        PropertyKey::Str(s) => Some(s.to_rust()),
        _ => None,
    }
}

/// Is `name` shadowed by the prototype chain (WebIDL "named property visibility")?
fn on_proto_chain(vm: &mut Vm, t: Obj, key: &PropertyKey) -> bool {
    let p = vm.heap.get(t).proto;
    match p {
        Some(p) => vm.has_property(p, key).unwrap_or(false),
        None => false,
    }
}

fn trap_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(t) = trap_target(vm, ctx) else { return Ok(Value::Undefined) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    let receiver = vm.arg(ctx, 2);
    if let Some(i) = key_index(&key) {
        return Ok(coll_item(vm, t, i).unwrap_or(Value::Undefined));
    }
    if let Some(name) = key_name(&key) {
        if !vm.heap.get(t).props.get(&key).is_some() && !on_proto_chain(vm, t, &key) {
            if let Some(v) = coll_named(vm, t, &name) {
                return Ok(v);
            }
        }
    }
    vm.get_with_receiver(t, &key, &receiver)
}

fn trap_has(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(t) = trap_target(vm, ctx) else { return Ok(Value::Bool(false)) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    if let Some(i) = key_index(&key) {
        return Ok(Value::Bool(i < coll_len(vm, t)));
    }
    if let Some(name) = key_name(&key) {
        if coll_named(vm, t, &name).is_some() {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(vm.has_property(t, &key)?))
}

fn desc_value(vm: &mut Vm, v: Value, enumerable: bool) -> Value {
    let o = vm.new_plain_object();
    let _ = vm.create_data_property(o, PropertyKey::from_str("value"), v);
    let _ = vm.create_data_property(o, PropertyKey::from_str("writable"), Value::Bool(false));
    let _ = vm.create_data_property(o, PropertyKey::from_str("enumerable"), Value::Bool(enumerable));
    let _ = vm.create_data_property(o, PropertyKey::from_str("configurable"), Value::Bool(true));
    Value::Object(o)
}

fn trap_gopd(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(t) = trap_target(vm, ctx) else { return Ok(Value::Undefined) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    if let Some(i) = key_index(&key) {
        return Ok(match coll_item(vm, t, i) {
            Some(v) => desc_value(vm, v, true),
            None => Value::Undefined,
        });
    }
    if let Some(name) = key_name(&key) {
        if !vm.heap.get(t).props.get(&key).is_some() && !on_proto_chain(vm, t, &key) {
            if let Some(v) = coll_named(vm, t, &name) {
                // [LegacyUnenumerableNamedProperties] for HTMLCollection and NamedNodeMap.
                return Ok(desc_value(vm, v, false));
            }
        }
    }
    match vm.get_own_property(t, &key)? {
        Some(d) => Ok(vm.from_property_descriptor(&d)),
        None => Ok(Value::Undefined),
    }
}

fn trap_own_keys(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(t) = trap_target(vm, ctx) else { return Ok(Value::Object(vm.new_array(Vec::new()))) };
    let n = coll_len(vm, t);
    let mut keys: Vec<Value> = (0..n).map(|i| s(&i.to_string())).collect();
    let items = if matches!(coll_kind(vm, t), CK_ATTRMAP | CK_TOKENS) { Vec::new() } else { coll_items(vm, t) };
    for (name, _) in coll_names(vm, t, &items) {
        keys.push(s(&name));
    }
    for k in vm.ordinary_own_keys(t) {
        keys.push(k.to_value());
    }
    Ok(Value::Object(vm.new_array(keys)))
}

fn trap_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(t) = trap_target(vm, ctx) else { return Ok(Value::Bool(false)) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    let v = vm.arg(ctx, 2);
    let receiver = vm.arg(ctx, 3);
    if key_index(&key).is_some() {
        return Ok(Value::Bool(false));
    }
    if let Some(name) = key_name(&key) {
        if !vm.heap.get(t).props.get(&key).is_some() && !on_proto_chain(vm, t, &key) && coll_named(vm, t, &name).is_some() {
            return Ok(Value::Bool(false));
        }
    }
    // Ordinary [[Set]] with the proxy as receiver lands in defineProperty below for new keys.
    let receiver = if receiver.is_object() { receiver } else { Value::Object(t) };
    let r = vm.set(t, key, v, &receiver)?;
    Ok(Value::Bool(r))
}

fn trap_define(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(t) = trap_target(vm, ctx) else { return Ok(Value::Bool(false)) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    let dv = vm.arg(ctx, 2);
    if key_index(&key).is_some() {
        return Ok(Value::Bool(false));
    }
    if let Some(name) = key_name(&key) {
        if !vm.heap.get(t).props.get(&key).is_some() && coll_named(vm, t, &name).is_some() {
            return Ok(Value::Bool(false));
        }
    }
    let d = vm.to_property_descriptor(&dv)?;
    Ok(Value::Bool(vm.define_own_property(t, key, d)?))
}

fn trap_delete(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(t) = trap_target(vm, ctx) else { return Ok(Value::Bool(false)) };
    let kv = vm.arg(ctx, 1);
    let key = vm.to_property_key(&kv)?;
    if let Some(i) = key_index(&key) {
        return Ok(Value::Bool(i >= coll_len(vm, t)));
    }
    if let Some(name) = key_name(&key) {
        if !vm.heap.get(t).props.get(&key).is_some() && coll_named(vm, t, &name).is_some() {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(vm.delete(t, &key)?))
}

fn this_coll(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Obj> {
    match this_tagged(vm, &ctx.this, T_COLL) {
        Some(o) => Ok(o),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn coll_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_coll(vm, ctx)?;
    Ok(num(coll_len(vm, t) as f64))
}

fn coll_item_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_coll(vm, ctx)?;
    need(vm, ctx, 1, "NodeList", "item")?;
    let i = unsigned_long(vm, &arg(vm, ctx, 0))? as usize;
    Ok(coll_item(vm, t, i).unwrap_or(Value::Null))
}

fn coll_named_item(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_coll(vm, ctx)?;
    need(vm, ctx, 1, "HTMLCollection", "namedItem")?;
    let n = string(vm, &arg(vm, ctx, 0))?;
    Ok(coll_named(vm, t, &n).unwrap_or(Value::Null))
}

fn attrmap_el(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    let t = this_coll(vm, ctx)?;
    if coll_kind(vm, t) != CK_ATTRMAP {
        return vm.throw_type("Illegal invocation");
    }
    Ok(slot_num(vm, t, 1) as usize)
}

fn attrmap_get_named_item_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = attrmap_el(vm, ctx)?;
    need(vm, ctx, 2, "NamedNodeMap", "getNamedItemNS")?;
    let ns = opt_string(vm, &arg(vm, ctx, 0))?.unwrap_or_default();
    let l = string(vm, &arg(vm, ctx, 1))?;
    Ok(find_attr_ns(el, &ns, &l).and_then(|i| attr_object(vm, el, i)).unwrap_or(Value::Null))
}

fn attrmap_set_named_item(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = attrmap_el(vm, ctx)?;
    need(vm, ctx, 1, "NamedNodeMap", "setNamedItem")?;
    let v = arg(vm, ctx, 0);
    let Some(a) = v.as_object().filter(|o| tag_of(vm, *o) == Some(T_ATTR)) else {
        return vm.throw_type("Failed to execute 'setNamedItem' on 'NamedNodeMap': parameter 1 is not of type 'Attr'.");
    };
    set_attribute_node_impl(vm, el, a)
}

fn attrmap_remove_named_item(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = attrmap_el(vm, ctx)?;
    need(vm, ctx, 1, "NamedNodeMap", "removeNamedItem")?;
    let q = string(vm, &arg(vm, ctx, 0))?;
    match find_attr_qname(el, &q) {
        Some(i) => {
            let obj = attr_object(vm, el, i).unwrap();
            let a = with_doc_mut(|d| d.element_mut(NodeId(el)).unwrap().attrs.remove(i));
            attribute_changed(vm, el, &a.local, a.ns, Some(a.value), None);
            Ok(obj)
        }
        None => throw_dom(vm, "NotFoundError", &format!("No item with name '{q}' was found.")),
    }
}

fn attrmap_remove_named_item_ns(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = attrmap_el(vm, ctx)?;
    need(vm, ctx, 2, "NamedNodeMap", "removeNamedItemNS")?;
    let ns = opt_string(vm, &arg(vm, ctx, 0))?.unwrap_or_default();
    let l = string(vm, &arg(vm, ctx, 1))?;
    match find_attr_ns(el, &ns, &l) {
        Some(i) => {
            let obj = attr_object(vm, el, i).unwrap();
            let a = with_doc_mut(|d| d.element_mut(NodeId(el)).unwrap().attrs.remove(i));
            attribute_changed(vm, el, &a.local, a.ns, Some(a.value), None);
            Ok(obj)
        }
        None => throw_dom(vm, "NotFoundError", "No item with that name was found."),
    }
}

// ---- DOMTokenList

fn tokens_el(vm: &mut Vm, ctx: &CallCtx) -> JsResult<(Obj, usize, String)> {
    let t = this_coll(vm, ctx)?;
    if coll_kind(vm, t) != CK_TOKENS {
        return vm.throw_type("Illegal invocation");
    }
    let el = slot_num(vm, t, 1) as usize;
    let a = coll_str(vm, t, 2);
    Ok((t, el, a))
}

fn validate_token(vm: &mut Vm, tok: &str) -> JsResult<()> {
    if tok.is_empty() {
        return throw_dom(vm, "SyntaxError", "The token provided must not be empty.");
    }
    if tok.chars().any(|c| matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r')) {
        return throw_dom(vm, "InvalidCharacterError", &format!("The token provided ('{tok}') contains HTML space characters."));
    }
    Ok(())
}

/// DOMTokenList "update steps".
fn tokens_update(vm: &mut Vm, el: usize, attr: &str, set: &[String]) {
    if attr_value(el, attr).is_none() && set.is_empty() {
        return;
    }
    set_attr(vm, el, attr, &set.join(" "));
}

fn tokens_contains(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (t, _, _) = tokens_el(vm, ctx)?;
    need(vm, ctx, 1, "DOMTokenList", "contains")?;
    let tok = string(vm, &arg(vm, ctx, 0))?;
    Ok(Value::Bool(tokens_of(vm, t).contains(&tok)))
}

fn tokens_add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (t, el, a) = tokens_el(vm, ctx)?;
    let mut toks = Vec::new();
    for i in 0..ctx.argc {
        let tok = string(vm, &arg(vm, ctx, i))?;
        validate_token(vm, &tok)?;
        toks.push(tok);
    }
    let mut set = tokens_of(vm, t);
    for tok in toks {
        if !set.contains(&tok) {
            set.push(tok);
        }
    }
    tokens_update(vm, el, &a, &set);
    Ok(Value::Undefined)
}

fn tokens_remove(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (t, el, a) = tokens_el(vm, ctx)?;
    let mut toks = Vec::new();
    for i in 0..ctx.argc {
        let tok = string(vm, &arg(vm, ctx, i))?;
        validate_token(vm, &tok)?;
        toks.push(tok);
    }
    let mut set = tokens_of(vm, t);
    set.retain(|x| !toks.contains(x));
    tokens_update(vm, el, &a, &set);
    Ok(Value::Undefined)
}

fn tokens_toggle(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (t, el, a) = tokens_el(vm, ctx)?;
    need(vm, ctx, 1, "DOMTokenList", "toggle")?;
    let tok = string(vm, &arg(vm, ctx, 0))?;
    validate_token(vm, &tok)?;
    let force = arg(vm, ctx, 1);
    let mut set = tokens_of(vm, t);
    if set.contains(&tok) {
        if force.is_undefined() || !boolean(vm, &force) {
            set.retain(|x| *x != tok);
            tokens_update(vm, el, &a, &set);
            return Ok(Value::Bool(false));
        }
        return Ok(Value::Bool(true));
    }
    if force.is_undefined() || boolean(vm, &force) {
        set.push(tok);
        tokens_update(vm, el, &a, &set);
        return Ok(Value::Bool(true));
    }
    Ok(Value::Bool(false))
}

fn tokens_replace(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (t, el, a) = tokens_el(vm, ctx)?;
    need(vm, ctx, 2, "DOMTokenList", "replace")?;
    let old = string(vm, &arg(vm, ctx, 0))?;
    let new = string(vm, &arg(vm, ctx, 1))?;
    validate_token(vm, &old)?;
    validate_token(vm, &new)?;
    let mut set = tokens_of(vm, t);
    let Some(pos) = set.iter().position(|x| *x == old) else { return Ok(Value::Bool(false)) };
    if set.contains(&new) {
        set.remove(pos);
        if let Some(p2) = set.iter().position(|x| *x == new) {
            // keep the first occurrence position (whichever came first)
            let _ = p2;
        }
    } else {
        set[pos] = new;
    }
    tokens_update(vm, el, &a, &set);
    Ok(Value::Bool(true))
}

fn tokens_supports(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, _, a) = tokens_el(vm, ctx)?;
    vm.throw_type(&format!("DOMTokenList has no supported tokens for the '{a}' attribute."))
}

fn tokens_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, el, a) = tokens_el(vm, ctx)?;
    Ok(s(&attr_value(el, &a).unwrap_or_default()))
}

fn tokens_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, el, a) = tokens_el(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    set_attr(vm, el, &a, &v);
    Ok(Value::Undefined)
}

// =================================================================================================
// Installation
// =================================================================================================

fn array_proto_fn(vm: &mut Vm, name: &str) -> Value {
    let ap = vm.intr().array_proto;
    vm.get(ap, &PropertyKey::from_str(name)).unwrap_or(Value::Undefined)
}

fn iterable_methods(vm: &mut Vm, proto: Obj) {
    for name in ["entries", "forEach", "keys", "values"] {
        let f = array_proto_fn(vm, name);
        data(vm, proto, name, f, WEC);
    }
    let values = array_proto_fn(vm, "values");
    let k = PropertyKey::Sym(vm.wk.iterator.clone());
    vm.heap.get_mut(proto).props.insert(k, Prop::data(values, WC));
}

pub fn install(vm: &mut Vm) {
    let et = iface("EventTarget").expect("EventTarget first");

    // ---- Node
    let node = interface(vm, "Node", Some(et), None);
    register_iface("Node", node);
    for (n, v) in [
        ("ELEMENT_NODE", 1),
        ("ATTRIBUTE_NODE", 2),
        ("TEXT_NODE", 3),
        ("CDATA_SECTION_NODE", 4),
        ("ENTITY_REFERENCE_NODE", 5),
        ("ENTITY_NODE", 6),
        ("PROCESSING_INSTRUCTION_NODE", 7),
        ("COMMENT_NODE", 8),
        ("DOCUMENT_NODE", 9),
        ("DOCUMENT_TYPE_NODE", 10),
        ("DOCUMENT_FRAGMENT_NODE", 11),
        ("NOTATION_NODE", 12),
        ("DOCUMENT_POSITION_DISCONNECTED", 0x01),
        ("DOCUMENT_POSITION_PRECEDING", 0x02),
        ("DOCUMENT_POSITION_FOLLOWING", 0x04),
        ("DOCUMENT_POSITION_CONTAINS", 0x08),
        ("DOCUMENT_POSITION_CONTAINED_BY", 0x10),
        ("DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC", 0x20),
    ] {
        konst(vm, node, n, v as f64);
    }
    let np = node.proto;
    attr(vm, np, "nodeType", node_type, None);
    attr(vm, np, "nodeName", node_name, None);
    attr(vm, np, "baseURI", base_uri, None);
    attr(vm, np, "isConnected", is_connected_get, None);
    attr(vm, np, "ownerDocument", owner_document, None);
    op(vm, np, "getRootNode", 0, get_root_node);
    attr(vm, np, "parentNode", parent_node, None);
    attr(vm, np, "parentElement", parent_element, None);
    op(vm, np, "hasChildNodes", 0, has_child_nodes);
    attr(vm, np, "childNodes", child_nodes, None);
    attr(vm, np, "firstChild", first_child, None);
    attr(vm, np, "lastChild", last_child, None);
    attr(vm, np, "previousSibling", previous_sibling, None);
    attr(vm, np, "nextSibling", next_sibling, None);
    attr(vm, np, "nodeValue", node_value_get, Some(node_value_set));
    attr(vm, np, "textContent", text_content_get, Some(text_content_set));
    op(vm, np, "normalize", 0, normalize);
    op(vm, np, "cloneNode", 0, clone_node_op);
    op(vm, np, "isEqualNode", 1, is_equal_node);
    op(vm, np, "isSameNode", 1, is_same_node);
    op(vm, np, "compareDocumentPosition", 1, compare_document_position);
    op(vm, np, "contains", 1, contains);
    op(vm, np, "lookupPrefix", 1, lookup_prefix);
    op(vm, np, "lookupNamespaceURI", 1, lookup_namespace_uri);
    op(vm, np, "isDefaultNamespace", 1, is_default_namespace);
    op(vm, np, "insertBefore", 2, insert_before_op);
    op(vm, np, "appendChild", 1, append_child_op);
    op(vm, np, "replaceChild", 2, replace_child_op);
    op(vm, np, "removeChild", 1, remove_child_op);

    // ---- Document, DocumentFragment, DocumentType, CharacterData family, Element
    let document = interface(vm, "Document", Some(node), Some((document_ctor, 0)));
    register_iface("Document", document);
    let html_document = interface(vm, "HTMLDocument", Some(document), None);
    register_iface("HTMLDocument", html_document);
    let xml_document = interface(vm, "XMLDocument", Some(document), None);
    register_iface("XMLDocument", xml_document);
    let fragment = interface(vm, "DocumentFragment", Some(node), Some((fragment_ctor, 0)));
    register_iface("DocumentFragment", fragment);
    let shadow = interface(vm, "ShadowRoot", Some(fragment), None);
    register_iface("ShadowRoot", shadow);
    let doctype = interface(vm, "DocumentType", Some(node), None);
    register_iface("DocumentType", doctype);
    let cdata = interface(vm, "CharacterData", Some(node), None);
    register_iface("CharacterData", cdata);
    let text = interface(vm, "Text", Some(cdata), Some((text_ctor, 0)));
    register_iface("Text", text);
    let cdsect = interface(vm, "CDATASection", Some(text), None);
    register_iface("CDATASection", cdsect);
    let comment = interface(vm, "Comment", Some(cdata), Some((comment_ctor, 0)));
    register_iface("Comment", comment);
    let pi = interface(vm, "ProcessingInstruction", Some(cdata), None);
    register_iface("ProcessingInstruction", pi);
    let element = interface(vm, "Element", Some(node), None);
    register_iface("Element", element);
    let attr_i = interface(vm, "Attr", Some(node), None);
    register_iface("Attr", attr_i);

    // Document
    let dp = document.proto;
    attr(vm, dp, "implementation", implementation, None);
    attr(vm, dp, "URL", doc_url, None);
    attr(vm, dp, "documentURI", doc_url, None);
    attr(vm, dp, "compatMode", compat_mode, None);
    attr(vm, dp, "characterSet", character_set, None);
    attr(vm, dp, "charset", character_set, None);
    attr(vm, dp, "inputEncoding", character_set, None);
    attr(vm, dp, "contentType", content_type, None);
    attr(vm, dp, "doctype", doctype_get, None);
    attr(vm, dp, "documentElement", document_element, None);
    op(vm, dp, "getElementsByTagName", 1, get_elements_by_tag_name);
    op(vm, dp, "getElementsByTagNameNS", 2, get_elements_by_tag_name_ns);
    op(vm, dp, "getElementsByClassName", 1, get_elements_by_class_name);
    op(vm, dp, "createElement", 1, create_element_op);
    op(vm, dp, "createElementNS", 2, create_element_ns_op);
    op(vm, dp, "createDocumentFragment", 0, create_document_fragment);
    op(vm, dp, "createTextNode", 1, create_text_node);
    op(vm, dp, "createCDATASection", 1, create_cdata_section);
    op(vm, dp, "createComment", 1, create_comment_op);
    op(vm, dp, "createProcessingInstruction", 2, create_processing_instruction);
    op(vm, dp, "importNode", 1, import_node);
    op(vm, dp, "adoptNode", 1, adopt_node);
    op(vm, dp, "createAttribute", 1, create_attribute);
    op(vm, dp, "createAttributeNS", 2, create_attribute_ns);
    op(vm, dp, "getElementById", 1, get_element_by_id);
    op(vm, dp, "getElementsByName", 1, get_elements_by_name);
    attr(vm, dp, "title", title_get, Some(title_set));
    attr(vm, dp, "body", body_get, Some(body_set));
    attr(vm, dp, "head", head_get, None);
    attr(vm, dp, "images", doc_images, None);
    attr(vm, dp, "embeds", doc_embeds, None);
    attr(vm, dp, "plugins", doc_embeds, None);
    attr(vm, dp, "links", doc_links, None);
    attr(vm, dp, "forms", doc_forms, None);
    attr(vm, dp, "scripts", doc_scripts, None);
    attr(vm, dp, "anchors", doc_anchors, None);
    // ParentNode on Document
    parent_node_mixin(vm, dp);

    // DocumentFragment
    let fp = fragment.proto;
    op(vm, fp, "getElementById", 1, get_element_by_id);
    parent_node_mixin(vm, fp);

    // DocumentType
    let dtp = doctype.proto;
    attr(vm, dtp, "name", doctype_name, None);
    attr(vm, dtp, "publicId", doctype_public, None);
    attr(vm, dtp, "systemId", doctype_system, None);
    child_node_mixin(vm, dtp);

    // CharacterData
    let cp = cdata.proto;
    attr(vm, cp, "data", cdata_get, Some(cdata_set));
    attr(vm, cp, "length", cdata_length, None);
    op(vm, cp, "substringData", 2, substring_data);
    op(vm, cp, "appendData", 1, append_data);
    op(vm, cp, "insertData", 2, insert_data);
    op(vm, cp, "deleteData", 2, delete_data);
    op(vm, cp, "replaceData", 3, replace_data_op);
    child_node_mixin(vm, cp);
    attr(vm, cp, "previousElementSibling", previous_element_sibling, None);
    attr(vm, cp, "nextElementSibling", next_element_sibling, None);
    // Text
    op(vm, text.proto, "splitText", 1, split_text);
    attr(vm, text.proto, "wholeText", whole_text, None);
    attr(vm, pi.proto, "target", pi_target, None);

    // Element
    let ep = element.proto;
    attr(vm, ep, "namespaceURI", namespace_uri, None);
    attr(vm, ep, "prefix", prefix_get, None);
    attr(vm, ep, "localName", local_name, None);
    attr(vm, ep, "tagName", tag_name_get, None);
    attr(vm, ep, "id", id_get, Some(id_set));
    attr(vm, ep, "className", class_name_get, Some(class_name_set));
    attr(vm, ep, "classList", class_list_get, Some(class_list_set));
    attr(vm, ep, "slot", slot_get, Some(slot_set));
    op(vm, ep, "hasAttributes", 0, has_attributes);
    attr(vm, ep, "attributes", attributes_get, None);
    op(vm, ep, "getAttributeNames", 0, get_attribute_names);
    op(vm, ep, "getAttribute", 1, get_attribute);
    op(vm, ep, "getAttributeNS", 2, get_attribute_ns);
    op(vm, ep, "setAttribute", 2, set_attribute);
    op(vm, ep, "setAttributeNS", 3, set_attribute_ns);
    op(vm, ep, "removeAttribute", 1, remove_attribute);
    op(vm, ep, "removeAttributeNS", 2, remove_attribute_ns);
    op(vm, ep, "toggleAttribute", 1, toggle_attribute);
    op(vm, ep, "hasAttribute", 1, has_attribute);
    op(vm, ep, "hasAttributeNS", 2, has_attribute_ns);
    op(vm, ep, "getAttributeNode", 1, get_attribute_node);
    op(vm, ep, "getAttributeNodeNS", 2, get_attribute_node_ns);
    op(vm, ep, "setAttributeNode", 1, set_attribute_node);
    op(vm, ep, "setAttributeNodeNS", 1, set_attribute_node);
    op(vm, ep, "removeAttributeNode", 1, remove_attribute_node);
    op(vm, ep, "closest", 1, closest_op);
    op(vm, ep, "matches", 1, matches_op);
    op(vm, ep, "webkitMatchesSelector", 1, matches_op);
    op(vm, ep, "getElementsByTagName", 1, get_elements_by_tag_name);
    op(vm, ep, "getElementsByTagNameNS", 2, get_elements_by_tag_name_ns);
    op(vm, ep, "getElementsByClassName", 1, get_elements_by_class_name);
    op(vm, ep, "insertAdjacentElement", 2, insert_adjacent_element);
    op(vm, ep, "insertAdjacentText", 2, insert_adjacent_text);
    op(vm, ep, "insertAdjacentHTML", 2, insert_adjacent_html);
    attr(vm, ep, "innerHTML", inner_html_get, Some(inner_html_set));
    attr(vm, ep, "outerHTML", outer_html_get, Some(outer_html_set));
    parent_node_mixin(vm, ep);
    child_node_mixin(vm, ep);
    attr(vm, ep, "previousElementSibling", previous_element_sibling, None);
    attr(vm, ep, "nextElementSibling", next_element_sibling, None);
    // ShadowRoot/innerHTML on fragments (Chromium exposes innerHTML on ShadowRoot only).
    attr(vm, shadow.proto, "innerHTML", inner_html_get, Some(inner_html_set));

    // Attr
    let ap = attr_i.proto;
    attr(vm, ap, "namespaceURI", attr_namespace, None);
    attr(vm, ap, "prefix", attr_prefix, None);
    attr(vm, ap, "localName", attr_local_name, None);
    attr(vm, ap, "name", attr_name, None);
    attr(vm, ap, "value", attr_value_get, Some(attr_value_set));
    attr(vm, ap, "ownerElement", attr_owner_element, None);
    attr(vm, ap, "specified", attr_specified, None);

    // ---- Collections
    let handler = vm.new_plain_object();
    for (name, f, len) in [
        ("get", trap_get as NativeFn, 3),
        ("has", trap_has, 2),
        ("getOwnPropertyDescriptor", trap_gopd, 2),
        ("ownKeys", trap_own_keys, 1),
        ("set", trap_set, 4),
        ("defineProperty", trap_define, 3),
        ("deleteProperty", trap_delete, 2),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(handler).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WEC));
    }
    root(vm, Value::Object(handler));
    COLL_HANDLER.with(|h| h.set(Some(handler)));

    let nodelist = interface(vm, "NodeList", None, None);
    register_iface("NodeList", nodelist);
    attr(vm, nodelist.proto, "length", coll_length, None);
    op(vm, nodelist.proto, "item", 1, coll_item_op);
    iterable_methods(vm, nodelist.proto);

    let htmlcoll = interface(vm, "HTMLCollection", None, None);
    register_iface("HTMLCollection", htmlcoll);
    attr(vm, htmlcoll.proto, "length", coll_length, None);
    op(vm, htmlcoll.proto, "item", 1, coll_item_op);
    op(vm, htmlcoll.proto, "namedItem", 1, coll_named_item);
    let values = array_proto_fn(vm, "values");
    let k = PropertyKey::Sym(vm.wk.iterator.clone());
    vm.heap.get_mut(htmlcoll.proto).props.insert(k.clone(), Prop::data(values.clone(), WC));

    let nnm = interface(vm, "NamedNodeMap", None, None);
    register_iface("NamedNodeMap", nnm);
    attr(vm, nnm.proto, "length", coll_length, None);
    op(vm, nnm.proto, "item", 1, coll_item_op);
    op(vm, nnm.proto, "getNamedItem", 1, coll_named_item);
    op(vm, nnm.proto, "getNamedItemNS", 2, attrmap_get_named_item_ns);
    op(vm, nnm.proto, "setNamedItem", 1, attrmap_set_named_item);
    op(vm, nnm.proto, "setNamedItemNS", 1, attrmap_set_named_item);
    op(vm, nnm.proto, "removeNamedItem", 1, attrmap_remove_named_item);
    op(vm, nnm.proto, "removeNamedItemNS", 2, attrmap_remove_named_item_ns);
    vm.heap.get_mut(nnm.proto).props.insert(k.clone(), Prop::data(values.clone(), WC));

    let dtl = interface(vm, "DOMTokenList", None, None);
    register_iface("DOMTokenList", dtl);
    let tp = dtl.proto;
    attr(vm, tp, "length", coll_length, None);
    op(vm, tp, "item", 1, coll_item_op);
    op(vm, tp, "contains", 1, tokens_contains);
    op(vm, tp, "add", 0, tokens_add);
    op(vm, tp, "remove", 0, tokens_remove);
    op(vm, tp, "toggle", 1, tokens_toggle);
    op(vm, tp, "replace", 2, tokens_replace);
    op(vm, tp, "supports", 1, tokens_supports);
    attr(vm, tp, "value", tokens_value_get, Some(tokens_value_set));
    let ts = vm.make_native("toString", 0, tokens_value_get, false);
    vm.heap.get_mut(tp).props.insert(PropertyKey::from_str("toString"), Prop::data(Value::Object(ts), WEC));
    iterable_methods(vm, tp);

    let g = vm.realm().global;
    let xs = vm.make_native("__xml_serialize", 1, xml_serialize_native, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__xml_serialize"), Prop::data(Value::Object(xs), WC));

    // ---- DOMImplementation
    let dimpl = interface(vm, "DOMImplementation", None, None);
    register_iface("DOMImplementation", dimpl);
    op(vm, dimpl.proto, "createDocumentType", 3, impl_create_document_type);
    op(vm, dimpl.proto, "createDocument", 2, impl_create_document);
    op(vm, dimpl.proto, "createHTMLDocument", 0, impl_create_html_document);
    op(vm, dimpl.proto, "hasFeature", 0, impl_has_feature);
}

fn parent_node_mixin(vm: &mut Vm, p: Obj) {
    attr(vm, p, "children", children_get, None);
    attr(vm, p, "firstElementChild", first_element_child, None);
    attr(vm, p, "lastElementChild", last_element_child, None);
    attr(vm, p, "childElementCount", child_element_count, None);
    op(vm, p, "prepend", 0, prepend_op);
    op(vm, p, "append", 0, append_op);
    op(vm, p, "replaceChildren", 0, replace_children_op);
    op(vm, p, "querySelector", 1, query_selector);
    op(vm, p, "querySelectorAll", 1, query_selector_all);
}

fn child_node_mixin(vm: &mut Vm, p: Obj) {
    op(vm, p, "before", 0, before_op);
    op(vm, p, "after", 0, after_op);
    op(vm, p, "replaceWith", 0, replace_with_op);
    op(vm, p, "remove", 0, remove_op);
}
