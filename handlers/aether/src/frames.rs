//! AETHERFRAMES (LEDGER SR83): the browsing-context tree — HTML §7.1–7.3 navigables, Aether's side.
//!
//! Aether owns a TREE of documents. The top-level traversable is context [`TOP`] (the page the shell
//! loaded); every connected `<iframe>` of a context's document owns one child context, whose document
//! is another Document node in the page's ONE arena (as `DOMParser` / `createHTMLDocument` documents
//! already are, `js/dom.rs`), so the parent's script reaches `iframe.contentDocument` through the same
//! binding with no copying. Each child context has its own JavaScript realm and global (the script half,
//! `js/frames.rs`); all of them share the page's single event loop, as similar-origin windows do.
//!
//! This module is the data model and the paint seam only — no JavaScript types:
//!
//! - the tree itself: parent, container element, active document, URL, origin, the browsing-context
//!   name, the sandboxing flags snapshotted at navigation (§7.6.5), the document's readiness;
//! - origins: the serialization of a URL's origin; opaque origins (sandboxed without
//!   `allow-same-origin`, `data:` documents) are unique per context and never same-origin with
//!   anything else;
//! - [`replaced_size`]: the `<iframe>` replaced box's natural size (its `width`/`height` attributes,
//!   else 300×150, CSS 2.2 §10.3.2) — the hook layout (LAYOUTCORE's) calls for an iframe box;
//! - the child surfaces: each nested document is laid out and painted into its own pixel buffer
//!   (`AetherEngine::render_frame`), registered here by its container, and composed into the
//!   container's content box by `render::render_frame` through [`child_surface`].

use crate::dom::NodeRef;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A browsing context's identity (stable for its life; never reused within a page).
pub type ContextId = u32;

/// The top-level traversable.
pub const TOP: ContextId = 0;

/// Deepest nesting a page may build (an `<iframe>` chain past this gets no context).
pub const MAX_DEPTH: usize = 16;

/// Most child contexts one page may hold at once.
pub const MAX_CONTEXTS: usize = 128;

/// The iframe sandboxing flag set (HTML §7.6.5), as the `allow-*` keywords that LIFT a restriction.
/// `None` on a context means "not sandboxed"; `Some` with every field false is the fully sandboxed set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sandbox {
    pub scripts: bool,
    pub same_origin: bool,
    pub forms: bool,
    pub popups: bool,
    pub modals: bool,
    pub top_navigation: bool,
}

impl Sandbox {
    /// "Parse a sandboxing directive": the attribute's unordered set of unique space-separated tokens,
    /// ASCII case-insensitive; unknown tokens are ignored.
    pub fn parse(attr: &str) -> Sandbox {
        let mut s = Sandbox::default();
        for t in attr.split_ascii_whitespace() {
            match t.to_ascii_lowercase().as_str() {
                "allow-scripts" => s.scripts = true,
                "allow-same-origin" => s.same_origin = true,
                "allow-forms" => s.forms = true,
                "allow-popups" => s.popups = true,
                "allow-modals" => s.modals = true,
                "allow-top-navigation" => s.top_navigation = true,
                _ => {}
            }
        }
        s
    }
}

/// Where a context's active document is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// The initial `about:blank` a new child context starts with (HTML §7.3.1).
    InitialBlank,
    /// A navigation's document is being populated and its scripts run.
    Loading,
    /// The document has fired `load`.
    Complete,
}

/// One browsing context (a navigable and its active document).
#[derive(Clone, Debug)]
pub struct BrowsingContext {
    pub id: ContextId,
    /// None for [`TOP`].
    pub parent: Option<ContextId>,
    /// The `<iframe>` element (page-arena node id); None for [`TOP`].
    pub container: Option<usize>,
    /// The active document (page-arena Document node id).
    pub document: usize,
    pub url: String,
    /// The origin's serialization; `null#<id>` for an opaque origin (unique to this context).
    pub origin: String,
    /// The browsing-context name (`window.name`; the container's `name` attribute at creation).
    pub name: String,
    pub sandbox: Option<Sandbox>,
    pub readiness: Readiness,
    /// The container's (`src`, `srcdoc`) as last processed — a change re-runs "process the iframe
    /// attributes" (HTML §4.8.5).
    pub seen_attrs: (Option<String>, Option<String>),
}

/// A painted nested document: BGRA rows, `width × height`.
#[derive(Clone, Debug)]
pub struct ChildSurface {
    pub width: u32,
    pub height: u32,
    pub pixels: Rc<Vec<u8>>,
}

#[derive(Default)]
struct Tree {
    contexts: Vec<BrowsingContext>,
    next: ContextId,
    /// (arena pointer, container id) → the container's painted surface.
    surfaces: HashMap<(usize, usize), ChildSurface>,
    /// Surfaces re-keyed into a paint snapshot's arena while that snapshot paints (nested frames).
    aliases: HashMap<(usize, usize), ChildSurface>,
}

thread_local! {
    static TREE: RefCell<Tree> = RefCell::new(Tree::default());
}

fn with<R>(f: impl FnOnce(&mut Tree) -> R) -> R {
    TREE.with(|t| f(&mut t.borrow_mut()))
}

/// A new page: the tree is just the top-level traversable showing `top_document` at `url`.
pub fn reset(top_document: usize, url: &str) {
    with(|t| {
        *t = Tree::default();
        t.contexts.push(BrowsingContext {
            id: TOP,
            parent: None,
            container: None,
            document: top_document,
            url: url.to_string(),
            origin: url_origin(url),
            name: String::new(),
            sandbox: None,
            readiness: Readiness::Complete,
            seen_attrs: (None, None),
        });
        t.next = TOP + 1;
    });
}

/// A snapshot of context `id` (None once it is discarded).
pub fn get(id: ContextId) -> Option<BrowsingContext> {
    with(|t| t.contexts.iter().find(|c| c.id == id).cloned())
}

/// Every live context, the top first, then in creation order.
pub fn all() -> Vec<BrowsingContext> {
    with(|t| t.contexts.clone())
}

/// Changes context `id` in place (a no-op once it is discarded).
pub fn update(id: ContextId, f: impl FnOnce(&mut BrowsingContext)) {
    with(|t| {
        if let Some(c) = t.contexts.iter_mut().find(|c| c.id == id) {
            f(c);
        }
    });
}

/// "Create a new child navigable" (HTML §7.3.1): a context under `parent` whose container is the
/// element `container`, showing `document`. None when the page is at its depth or count cap.
pub fn create(parent: ContextId, container: usize, document: usize, url: &str, origin: &str, name: &str, sandbox: Option<Sandbox>) -> Option<ContextId> {
    if depth(parent) + 1 > MAX_DEPTH {
        crate::ledger::record_dom("frames-depth-cap");
        return None;
    }
    with(|t| {
        if t.contexts.len() > MAX_CONTEXTS || !t.contexts.iter().any(|c| c.id == parent) {
            if t.contexts.len() > MAX_CONTEXTS {
                crate::ledger::record_dom("frames-count-cap");
            }
            return None;
        }
        let id = t.next;
        t.next += 1;
        t.contexts.push(BrowsingContext {
            id,
            parent: Some(parent),
            container: Some(container),
            document,
            url: url.to_string(),
            origin: origin.to_string(),
            name: name.to_string(),
            sandbox,
            readiness: Readiness::InitialBlank,
            seen_attrs: (None, None),
        });
        Some(id)
    })
}

/// The context whose container is element `el`.
pub fn by_container(el: usize) -> Option<ContextId> {
    with(|t| t.contexts.iter().find(|c| c.container == Some(el)).map(|c| c.id))
}

/// The context whose active document is `doc`.
pub fn by_document(doc: usize) -> Option<ContextId> {
    with(|t| t.contexts.iter().find(|c| c.document == doc).map(|c| c.id))
}

/// The child contexts of `id`, in creation order (the script half sorts them into tree order).
pub fn children(id: ContextId) -> Vec<ContextId> {
    with(|t| t.contexts.iter().filter(|c| c.parent == Some(id)).map(|c| c.id).collect())
}

/// The ancestors of `id`, nearest first.
pub fn ancestors(id: ContextId) -> Vec<ContextId> {
    let mut out = Vec::new();
    let mut cur = get(id).and_then(|c| c.parent);
    while let Some(p) = cur {
        if out.contains(&p) {
            break;
        }
        out.push(p);
        cur = get(p).and_then(|c| c.parent);
    }
    out
}

/// How many ancestors `id` has (0 for the top).
pub fn depth(id: ContextId) -> usize {
    ancestors(id).len()
}

/// Discards context `id` and every descendant (HTML "destroy a child navigable"). Returns the ids
/// removed, descendants before their ancestors.
pub fn remove(id: ContextId) -> Vec<ContextId> {
    if id == TOP {
        return Vec::new();
    }
    let mut out = Vec::new();
    for k in children(id) {
        out.extend(remove(k));
    }
    with(|t| {
        if let Some(c) = t.contexts.iter().find(|c| c.id == id) {
            if let Some(el) = c.container {
                t.surfaces.retain(|k, _| k.1 != el);
            }
        }
        t.contexts.retain(|c| c.id != id);
    });
    out.push(id);
    out
}

/// Every child context, descendants before their ancestors (the order nested surfaces paint in).
pub fn post_order() -> Vec<ContextId> {
    fn walk(id: ContextId, out: &mut Vec<ContextId>) {
        for k in children(id) {
            walk(k, out);
            out.push(k);
        }
    }
    let mut out = Vec::new();
    walk(TOP, &mut out);
    out
}

// -------------------------------------------------------------------------------------------------
// Origins (HTML §7.5)
// -------------------------------------------------------------------------------------------------

/// The serialization of `url`'s origin ("null" when it is opaque or unparsable).
pub fn url_origin(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) if u.scheme() == "file" => "file://".to_string(),
        Ok(u) => u.origin().ascii_serialization(),
        Err(_) => "null".to_string(),
    }
}

/// Same origin: equal tuple origins. An opaque origin is same-origin only with itself, and the
/// serialization "null" (an origin nobody owns) with nothing.
pub fn same_origin(a: &str, b: &str) -> bool {
    a == b && a != "null"
}

/// The origin as script sees it (`window.origin`, `MessageEvent.origin`): opaque ones read "null".
pub fn display_origin(origin: &str) -> &str {
    if origin.starts_with("null") {
        "null"
    } else {
        origin
    }
}

// -------------------------------------------------------------------------------------------------
// The replaced box and the child surfaces (the layout and render seams)
// -------------------------------------------------------------------------------------------------

/// HTML "rules for parsing dimension values" for a non-negative length (a percentage is resolved by
/// layout against the containing block, so it is not a natural size: None).
fn parse_dimension(v: &str) -> Option<f32> {
    let t = v.trim_start_matches([' ', '\t', '\n', '\x0c', '\r']);
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if digits.is_empty() || t[digits.len()..].starts_with('%') {
        return None;
    }
    let n: f32 = digits.trim_end_matches('.').parse().ok()?;
    (n.is_finite()).then_some(n)
}

/// The natural size of an `<iframe>` replaced box: its `width`/`height` attributes (presentational
/// hints, HTML §15.4.3), else the CSS 2.2 §10.3.2 default 300×150. None when `node` is not an iframe.
pub fn replaced_size(node: &NodeRef) -> Option<(f32, f32)> {
    let el = node.as_element()?;
    if el.name.local.as_ref() != "iframe" {
        return None;
    }
    let attrs = el.attributes.borrow();
    let w = attrs.get("width").and_then(parse_dimension).unwrap_or(300.0);
    let h = attrs.get("height").and_then(parse_dimension).unwrap_or(150.0);
    Some((w, h))
}

fn key_of(node: &NodeRef) -> (usize, usize) {
    (Rc::as_ptr(node.arena()) as usize, node.id().0)
}

/// Registers the painted surface of the nested document whose container is `container`.
pub fn set_child_surface(container: &NodeRef, s: ChildSurface) {
    let k = key_of(container);
    with(|t| {
        t.surfaces.insert(k, s);
    });
}

/// Forgets every painted surface (a new frame's paint replaces them).
pub fn clear_surfaces() {
    with(|t| {
        t.surfaces.clear();
        t.aliases.clear();
    });
}

/// While the snapshot `snap` (a copy of a nested document, `map` pairing page-arena ids with the
/// snapshot's) paints, its own `<iframe>` elements find the surfaces their page-arena twins
/// registered.
pub fn alias_surfaces(page_node: &NodeRef, snap: &NodeRef, map: &[(usize, usize)]) {
    let src = Rc::as_ptr(page_node.arena()) as usize;
    let dst = Rc::as_ptr(snap.arena()) as usize;
    with(|t| {
        for &(a, b) in map {
            if let Some(s) = t.surfaces.get(&(src, a)).cloned() {
                t.aliases.insert((dst, b), s);
            }
        }
    });
}

/// Drops the snapshot aliases (the snapshot is gone after its paint).
pub fn clear_aliases() {
    with(|t| t.aliases.clear());
}

/// The painted surface of the nested document `node` (an `<iframe>`) shows, if one is registered —
/// what `render::render_frame` composes into the container's content box.
pub fn child_surface(node: &NodeRef) -> Option<ChildSurface> {
    let k = key_of(node);
    with(|t| t.surfaces.get(&k).or_else(|| t.aliases.get(&k)).cloned())
}

/// Copies nested document `document` (a Document node of `any`'s arena) into a fresh arena whose
/// `Document::ROOT` is the copy, for layout and the cascade (both walk from `ROOT`). Returns the copy's
/// Document node and the (page-arena id, copy id) pairs of every node copied.
pub fn snapshot(any: &NodeRef, document: usize) -> (NodeRef, Vec<(usize, usize)>) {
    let src_arena = any.arena().clone();
    let mut dst = html_core::Document::new();
    let mut map = Vec::new();
    {
        let src = src_arena.borrow();
        dst.quirks_mode = html_core::QuirksMode::NoQuirks;
        let kids: Vec<html_core::NodeId> = src.children(html_core::NodeId(document)).collect();
        for k in kids {
            let c = crate::dom::import_tree(&mut dst, &src, k, false);
            dst.append(html_core::Document::ROOT, c);
            let a: Vec<usize> = std::iter::once(k).chain(src.descendants(k)).map(|n| n.0).collect();
            let b: Vec<usize> = std::iter::once(c).chain(dst.descendants(c)).map(|n| n.0).collect();
            map.extend(a.into_iter().zip(b));
        }
    }
    (NodeRef::from_arena(Rc::new(RefCell::new(dst)), html_core::Document::ROOT), map)
}
