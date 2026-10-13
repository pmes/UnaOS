//! AETHERFRAMES (LEDGER SR83): the script half of the browsing-context tree (`crate::frames`).
//!
//! Every child browsing context gets its own js_core realm (`Vm::create_realm`): its own intrinsics
//! (`Object`, `Array`, …) and its own global object, which is branded as an EventTarget platform object
//! (`idl::T_ETARGET` with a target serial) so the binding's EventTarget methods, event handlers and
//! `dispatch` treat it as its own target (`TargetKey::Other`). The global's prototype is the top realm's
//! `Window.prototype`; the Window members that are per context (`window`/`self`/`frames`, `document`,
//! `location`, `top`, `parent`, `frameElement`, `length`, `name`, `opener`, `closed`, `origin`,
//! `postMessage`, `open`, `close`) are own natives that know their context; everything else on the top
//! global (the DOM interface objects, timers, the platform preludes) is copied over by reference — the
//! DOM bindings are one set over one arena, so a child's `document` is a Document of the page arena and
//! `iframe.contentDocument` is the same wrapper the child's script sees.
//!
//! Lifecycle (HTML §4.8.5, §7.3, §7.4), driven by [`service`] — the browsing-context task source,
//! run first in every `event_loop::fire_due_timers`, so it runs inside the loader's "spin the event loop
//! until nothing delays the load event" and the parent's `load` follows its iframes' — and by [`ensure`]
//! from every getter that must see the tree current:
//!
//! - an `<iframe>` connected to a context's document gets a child context with the initial
//!   `about:blank` document (`<html><head></head><body></body></html>`) at once; with no `src`/`srcdoc`
//!   (or `src` resolving to `about:blank`) only the iframe load event steps follow (a queued `load` at
//!   the element); otherwise a navigation is queued;
//! - a navigation fetches (`api::fetch::fetch_subdocument`; `srcdoc` is `about:srcdoc` with the
//!   parent's origin; `javascript:` URLs run in the child realm and a string result becomes the
//!   document), fires `beforeunload`/`pagehide`/`unload` at a document being replaced, creates a NEW
//!   Document, parses into it, runs its classic scripts in tree order in the child realm (unless
//!   sandboxed without `allow-scripts`), then fires `DOMContentLoaded`, `load` at the child window and
//!   `load` at the iframe;
//! - a change of `src`/`srcdoc` re-processes the iframe attributes; a container that is no longer
//!   connected destroys its context (children first): `pagehide` and `unload` fire at its window;
//! - the nesting guard refuses a navigation to a URL an ancestor already shows, and `crate::frames`
//!   caps depth and count.
//!
//! Origins: `contentDocument` is null and `contentWindow` / `parent` / `top` are the cross-origin
//! WindowProxy subset (HTML §7.2.3.3 — `window`/`self`/`frames`/`location`/`close`/`closed`/`focus`/
//! `blur`/`length`/`top`/`opener`/`parent`/`postMessage` and indices; anything else throws
//! "SecurityError"; a `location` that only takes `href =` and `replace()`) when the incumbent realm's
//! context is not same-origin with the target. `postMessage` (HTML §9.3.3) checks `targetOrigin`,
//! structured-clones the message (cycles, Map/Set, Date, RegExp, ArrayBuffer and views, Error; a
//! function, symbol or platform object is a "DataCloneError"), passes MessageChannel ports through
//! `transfer`, and queues a `MessageEvent` (data, origin, source, ports) on the target window.
//!
//! Honest gaps (ledgered where they occur): one realm per context for its whole life (WindowProxy
//! identity holds across navigations; a new document reuses the Window, which the spec does only for
//! the initial `about:blank`); the interface objects are the top realm's (`frames[0].Node === Node`);
//! module scripts in a nested document are skipped; `document.write` from a nested document's script
//! re-opens that document (it has no parser); XML documents are parsed as HTML; `loading=lazy` loads
//! eagerly (headless has no viewport intersection); removal is noticed at the next service, not inside
//! `removeChild`. (AETHERDEBT SR93: an event path inside a nested document now continues to its
//! window — [`window_target_of_document`] — and `isConnected` is true in any live context's document.)

use html_core::url;
use crate::frames::{self as tree, ContextId, Readiness, Sandbox, TOP};
use crate::js::dom::{self, with_doc, with_doc_mut, wrap};
use crate::js::events;
use crate::js::idl::{self, arg, need, num, root, s, string, throw_dom};
use crate::js::{page, TargetKey, Task};
use html_core::NodeId;
use js_core::vm::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// The script-side record of one context.
struct JsCtx {
    realm: u32,
    global: Obj,
    /// The target serial of a child global (`TargetKey::Other`); 0 for the top (`TargetKey::Window`).
    serial: u32,
    /// The document the context showed last (a discarded context's `document` still answers).
    last_doc: usize,
    location: Option<Obj>,
    xo: Option<Obj>,
}

/// A queued navigation of a child context.
struct NavRequest {
    ctx: ContextId,
    url: String,
    srcdoc: Option<String>,
}

thread_local! {
    static JS: RefCell<HashMap<ContextId, JsCtx>> = RefCell::new(HashMap::new());
    static NAVS: RefCell<Vec<NavRequest>> = const { RefCell::new(Vec::new()) };
    /// (DOM version, arena size) at the last scan: an unchanged page needs no rescan.
    static LAST_SCAN: Cell<(u64, usize)> = const { Cell::new((u64::MAX, usize::MAX)) };
    static SCANNING: Cell<bool> = const { Cell::new(false) };
    static SERVICING: Cell<bool> = const { Cell::new(false) };
    /// The top realm's `__aether_clone` and `__aether_xo` (rooted).
    static HELPERS: Cell<Option<(Obj, Obj)>> = const { Cell::new(None) };
}

/// A new page (called from `event_loop::reset`, inside `Engine::with_viewport`, once the page state
/// holds the new document): the tree is the top-level traversable alone.
pub fn reset() {
    JS.with(|j| j.borrow_mut().clear());
    NAVS.with(|n| n.borrow_mut().clear());
    LAST_SCAN.with(|l| l.set((u64::MAX, usize::MAX)));
    SCANNING.with(|b| b.set(false));
    SERVICING.with(|b| b.set(false));
    HELPERS.with(|h| h.set(None));
    tree::reset(dom::main_doc(), &crate::js::page_url());
}

// =================================================================================================
// Contexts, realms, origins
// =================================================================================================

fn global_of(id: ContextId) -> Option<Obj> {
    JS.with(|j| j.borrow().get(&id).map(|c| c.global))
}

fn realm_of(id: ContextId) -> Option<u32> {
    JS.with(|j| j.borrow().get(&id).map(|c| c.realm))
}

fn ctx_of_realm(r: u32) -> ContextId {
    JS.with(|j| j.borrow().iter().find(|(_, c)| c.realm == r).map(|(k, _)| *k)).unwrap_or(TOP)
}

/// The context whose global object is `o`.
pub fn ctx_of_global(o: Obj) -> Option<ContextId> {
    JS.with(|j| j.borrow().iter().find(|(_, c)| c.global == o).map(|(k, _)| *k))
}

/// The context of the incumbent realm: the realm of the script that called the running native (a
/// native pushes no frame, so the last frame is its caller's).
fn incumbent(vm: &Vm) -> ContextId {
    ctx_of_realm(vm.frames.last().map(|f| f.realm).unwrap_or(0))
}

/// The context's active document (a discarded one keeps answering with its last).
pub fn document_of(id: ContextId) -> usize {
    tree::get(id).map(|c| c.document).or_else(|| JS.with(|j| j.borrow().get(&id).map(|c| c.last_doc))).unwrap_or_else(dom::main_doc)
}

fn url_of(id: ContextId) -> String {
    if id == TOP {
        return crate::js::page_url();
    }
    tree::get(id).map(|c| c.url).unwrap_or_else(|| "about:blank".into())
}

fn origin_of(id: ContextId) -> String {
    if id == TOP {
        return tree::url_origin(&crate::js::page_url());
    }
    tree::get(id).map(|c| c.origin).unwrap_or_else(|| format!("null#{id}"))
}

fn same_origin(a: ContextId, b: ContextId) -> bool {
    a == b || tree::same_origin(&origin_of(a), &origin_of(b))
}

/// The event target a context's window is.
fn target_key(id: ContextId) -> Option<TargetKey> {
    if id == TOP {
        return Some(TargetKey::Window);
    }
    tree::get(id)?;
    JS.with(|j| j.borrow().get(&id).map(|c| TargetKey::Other(c.serial)))
}

/// The cross-origin WindowProxy subset of context `id`'s window (made once, then the same object).
fn xo_of(vm: &mut Vm, id: ContextId) -> Option<Value> {
    if let Some(o) = JS.with(|j| j.borrow().get(&id).and_then(|c| c.xo)) {
        return Some(Value::Object(o));
    }
    let (_, xo_fn) = HELPERS.with(Cell::get)?;
    let g = global_of(id)?;
    let v = vm.call(&Value::Object(xo_fn), &Value::Undefined, &[Value::Object(g)]).ok()?;
    let o = v.as_object()?;
    root(vm, v.clone());
    JS.with(|j| {
        if let Some(c) = j.borrow_mut().get_mut(&id) {
            c.xo = Some(o);
        }
    });
    Some(v)
}

/// Context `subject`'s window as context `viewer` may see it: the window itself when they are
/// same-origin, else its cross-origin subset.
fn window_seen_by(vm: &mut Vm, viewer: ContextId, subject: ContextId) -> Value {
    let Some(g) = global_of(subject) else { return Value::Null };
    if same_origin(viewer, subject) {
        return Value::Object(g);
    }
    xo_of(vm, subject).unwrap_or(Value::Null)
}

/// Context `id`'s window as the running script may see it.
fn window_for(vm: &mut Vm, id: ContextId) -> Value {
    let inc = incumbent(vm);
    window_seen_by(vm, inc, id)
}

/// Whether context `c`'s container is still connected to its parent's active document.
fn container_live(c: &tree::BrowsingContext) -> bool {
    let (Some(el), Some(p)) = (c.container, c.parent) else { return c.id == TOP };
    match tree::get(p) {
        Some(pc) => dom::root_of(el) == pc.document,
        None => false,
    }
}

/// The child contexts of `id` whose containers are connected, in tree order of the containers (HTML
/// "document-tree child navigables").
fn live_children(id: ContextId) -> Vec<ContextId> {
    let mut v: Vec<(usize, ContextId)> = tree::children(id)
        .into_iter()
        .filter_map(|k| {
            let c = tree::get(k)?;
            let el = c.container?;
            container_live(&c).then_some((el, k))
        })
        .collect();
    with_doc(|d| {
        v.sort_by(|a, b| {
            if a.0 == b.0 {
                std::cmp::Ordering::Equal
            } else if dom::precedes(d, a.0, b.0) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        })
    });
    v.into_iter().map(|(_, k)| k).collect()
}

/// `window.length` of context `id`.
pub fn child_count(vm: &mut Vm, id: ContextId) -> usize {
    ensure(vm);
    live_children(id).len()
}

/// The WindowProxy [[GetOwnProperty]] / named-properties half for child navigables: `window[i]` is the
/// i-th child (tree order), `window[name]` the first child whose browsing-context name is `name`.
pub fn named_or_indexed(vm: &mut Vm, owner: ContextId, key: &PropertyKey) -> Option<Value> {
    let id = match key {
        PropertyKey::Index(i) => {
            ensure(vm);
            live_children(owner).get(*i as usize).copied()
        }
        PropertyKey::Str(n) => {
            let n = n.to_rust();
            if n.is_empty() {
                return None;
            }
            ensure(vm);
            live_children(owner).into_iter().find(|k| tree::get(*k).is_some_and(|c| c.name == n))
        }
        _ => None,
    }?;
    Some(window_for(vm, id))
}

/// `document.defaultView` of a nested document: its context's window (null when it has none).
pub fn window_of_document(_vm: &mut Vm, doc: usize) -> Value {
    match tree::by_document(doc).and_then(global_of) {
        Some(g) => Value::Object(g),
        None => Value::Null,
    }
}

/// Is `doc` the active document of a live browsing context (the top's, or a child context whose
/// container has not been discarded)? AETHERDEBT SR93: `isConnected` and the event path read it.
pub fn is_active_document(doc: usize) -> bool {
    doc == dom::main_doc() || tree::by_document(doc).is_some()
}

/// The event target of the window whose active document is `doc` when that is a NESTED context's
/// (DOM §2.9 "get the parent" of a Document: its relevant global — AETHERDEBT SR93; the top's Window
/// is `js/events.rs`'s own step).
pub fn window_target_of_document(doc: usize) -> Option<TargetKey> {
    let id = tree::by_document(doc).filter(|&id| id != TOP)?;
    target_key(id)
}

/// `document.location` of a nested document.
pub fn location_of_document(vm: &mut Vm, doc: usize) -> Value {
    match tree::by_document(doc) {
        Some(id) if id != TOP => location_obj(vm, id),
        _ => Value::Null,
    }
}

/// `document.readyState` of a nested document.
pub fn readiness_of_document(doc: usize) -> &'static str {
    match tree::by_document(doc).and_then(tree::get).map(|c| c.readiness) {
        Some(Readiness::Loading) => "loading",
        _ => "complete",
    }
}

/// The URL of the document a nested context shows (None for a document with no context).
pub fn url_of_document(doc: usize) -> Option<String> {
    tree::by_document(doc).map(url_of)
}

// =================================================================================================
// Creating a child context
// =================================================================================================

/// Replaces `doc`'s children with the parse of `markup` (its scripts marked parser-inserted and
/// already started: [`run_scripts`] runs them in the context's realm). Returns whether the parse set
/// quirks mode.
fn populate(doc: usize, markup: &str) -> bool {
    let parsed = html_core::parse_document(markup, crate::dom::parse_opts());
    let quirks = parsed.quirks_mode != html_core::QuirksMode::NoQuirks;
    let kids = dom::children_of(doc);
    with_doc_mut(|d| {
        for k in kids {
            d.detach(NodeId(k));
        }
    });
    let top: Vec<NodeId> = parsed.children(html_core::Document::ROOT).collect();
    for t in top {
        let id = with_doc_mut(|d| {
            let c = crate::dom::import_tree(d, &parsed, t, false);
            d.append(NodeId(doc), c);
            c.0
        });
        let ids: Vec<usize> = with_doc(|d| std::iter::once(NodeId(id)).chain(d.descendants(NodeId(id))).map(|i| i.0).collect());
        for i in ids {
            dom::set_node_doc(i, doc);
            let (is_script, template) = with_doc(|d| match d.element(NodeId(i)) {
                Some(e) => (e.is_html("script"), e.template_contents),
                None => (false, None),
            });
            if is_script {
                page(|p| {
                    p.started.insert(i);
                    p.parser_inserted.insert(i);
                });
            }
            if let Some(t) = template {
                let inner: Vec<usize> = with_doc(|d| std::iter::once(t).chain(d.descendants(t)).map(|x| x.0).collect());
                for x in inner {
                    dom::set_node_doc(x, doc);
                }
            }
        }
    }
    crate::js::touch();
    quirks
}

/// "Create a new child navigable" for the `<iframe>` element `container` of context `parent`'s document:
/// the initial `about:blank` document, the realm and its global. None at the tree's caps.
fn create_child(vm: &mut Vm, parent: ContextId, container: usize) -> Option<ContextId> {
    let doc = dom::new_document(true, "text/html", "about:blank");
    populate(doc, "");
    let name = dom::attr_value(container, "name").unwrap_or_default();
    let sandbox = dom::attr_value(container, "sandbox").map(|v| Sandbox::parse(&v));
    // The initial about:blank's origin is its creator's (the parent document's), unless sandboxed.
    let origin = origin_of(parent);
    let id = tree::create(parent, container, doc, "about:blank", &origin, &name, sandbox)?;
    if sandbox.is_some_and(|s| !s.same_origin) {
        tree::update(id, |c| c.origin = format!("null#{id}"));
    }
    let saved = vm.cur_realm;
    let realm = vm.create_realm();
    // The members below are the binding's natives: they belong to the top realm, as every binding
    // native does (a native runs in its own realm; `events::target_of` compares against that realm's
    // global, so the child global resolves to its own target serial).
    vm.cur_realm = 0;
    let g = vm.realms[realm as usize].global;
    let serial = page(|p| {
        p.target_serial += 1;
        p.target_serial
    });
    vm.heap.get_mut(g).kind = Kind::Internal(vec![Value::Symbol(idl::brand()), Value::Number(idl::T_ETARGET as f64), Value::Number(serial as f64)]);
    if let Some(w) = dom::iface("Window") {
        vm.heap.get_mut(g).proto = Some(w.proto);
    }
    let r = root(vm, Value::Object(g));
    page(|p| {
        p.other_targets.insert(serial, (g, r));
    });
    JS.with(|j| {
        j.borrow_mut().insert(id, JsCtx { realm, global: g, serial, last_doc: doc, location: None, xo: None });
    });
    define_window_members(vm, id, g);
    let top_g = vm.realms[0].global;
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__aether_parent"), Prop::data(Value::Object(top_g), WC));
    vm.cur_realm = realm;
    let r = crate::js::enter(vm, "frame-realm", |vm| vm.run_script_str(SETUP).map(|_| ()));
    vm.cur_realm = saved;
    if let Some(Err(e)) = r {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("frame-realm-setup-failed:{}", crate::js::clip(&m, 64)));
    }
    Some(id)
}

/// The per-context Window members, as own properties of the child global.
fn define_window_members(vm: &mut Vm, id: ContextId, g: Obj) {
    member_accessor(vm, g, id, "window", w_self, None, true);
    member_accessor(vm, g, id, "document", w_document, None, true);
    member_accessor(vm, g, id, "location", w_location, Some(w_location_set), true);
    member_accessor(vm, g, id, "top", w_top, None, true);
    member_accessor(vm, g, id, "self", w_self, Some(w_replace), false);
    member_accessor(vm, g, id, "frames", w_self, Some(w_replace), false);
    member_accessor(vm, g, id, "parent", w_parent, Some(w_replace), false);
    member_accessor(vm, g, id, "frameElement", w_frame_element, None, false);
    member_accessor(vm, g, id, "length", w_length, Some(w_replace), false);
    member_accessor(vm, g, id, "name", w_name, Some(w_name_set), false);
    member_accessor(vm, g, id, "opener", w_null, Some(w_replace), false);
    member_accessor(vm, g, id, "closed", w_closed, None, false);
    member_accessor(vm, g, id, "origin", w_origin, Some(w_replace), false);
    member_op(vm, g, id, "postMessage", 1, w_post_message);
    member_op(vm, g, id, "open", 0, w_open);
    for name in ["close", "stop", "focus", "blur"] {
        member_op(vm, g, id, name, 0, w_noop);
    }
}

fn member_accessor(vm: &mut Vm, g: Obj, id: ContextId, name: &str, get: NativeFn, set: Option<NativeFn>, unforgeable: bool) {
    let fp = vm.intr().function_proto;
    let slots = vec![num(id as f64), s(name)];
    let gf = vm.make_native_with(&format!("get {name}"), 0, get, false, Some(fp), slots.clone());
    let sf = set.map(|f| vm.make_native_with(&format!("set {name}"), 1, f, false, Some(fp), slots));
    let flags = if unforgeable { E } else { E | C };
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop { slot: Slot::Accessor(Some(gf), sf), flags });
}

fn member_op(vm: &mut Vm, g: Obj, id: ContextId, name: &str, len: u32, f: NativeFn) {
    let fp = vm.intr().function_proto;
    let fo = vm.make_native_with(name, len, f, false, Some(fp), vec![num(id as f64), s(name)]);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WEC));
}

fn slot_id(vm: &Vm, ctx: &CallCtx) -> ContextId {
    match vm.native_slot(ctx.callee, 0) {
        Value::Number(n) if n.is_finite() && n >= 0.0 => n as ContextId,
        _ => TOP,
    }
}

fn slot_str(vm: &Vm, ctx: &CallCtx, i: usize) -> String {
    match vm.native_slot(ctx.callee, i) {
        Value::String(st) => st.to_rust(),
        _ => String::new(),
    }
}

/// Runs in the child realm once its own members are in place: the rest of the top global's own
/// properties are shared by reference, and the EventTarget operations get global-bound twins (a bare
/// `addEventListener(…)` in the child's script passes `undefined` as `this`, which the binding would
/// read as the TOP window).
const SETUP: &str = r#"
(function (P, G) {
    var own = Object.prototype.hasOwnProperty;
    var names = Object.getOwnPropertyNames(P);
    for (var i = 0; i < names.length; i++) {
        var k = names[i];
        if (k === '__aether_parent' || own.call(G, k)) { continue; }
        var d = Object.getOwnPropertyDescriptor(P, k);
        if (d) { try { Object.defineProperty(G, k, d); } catch (e) {} }
    }
    var ET = P.EventTarget && P.EventTarget.prototype;
    if (ET) {
        ['addEventListener', 'removeEventListener', 'dispatchEvent'].forEach(function (m) {
            var f = ET[m];
            var w = function () { return f.apply(this === undefined || this === null ? G : this, arguments); };
            Object.defineProperty(G, m, { value: w, writable: true, enumerable: true, configurable: true });
        });
    }
    delete G.__aether_parent;
})(globalThis.__aether_parent, globalThis);
"#;

// =================================================================================================
// The child Window members
// =================================================================================================

fn w_self(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    Ok(global_of(id).map(Value::Object).unwrap_or(Value::Null))
}

fn w_document(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    let d = document_of(id);
    Ok(wrap(vm, d))
}

fn w_top(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    if tree::get(id).is_none() {
        return Ok(Value::Null);
    }
    Ok(window_for(vm, TOP))
}

fn w_parent(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    match tree::get(id).and_then(|c| c.parent) {
        Some(p) => Ok(window_for(vm, p)),
        None => Ok(Value::Null),
    }
}

fn w_frame_element(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    let Some(c) = tree::get(id) else { return Ok(Value::Null) };
    let (Some(el), Some(p)) = (c.container, c.parent) else { return Ok(Value::Null) };
    // HTML §7.2.2.2: null unless the container's node document is same origin-domain with the caller.
    let inc = incumbent(vm);
    if !same_origin(inc, p) {
        return Ok(Value::Null);
    }
    Ok(wrap(vm, el))
}

fn w_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    Ok(num(child_count(vm, id) as f64))
}

fn w_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    Ok(s(&tree::get(id).map(|c| c.name).unwrap_or_default()))
}

fn w_name_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    let v = string(vm, &arg(vm, ctx, 0))?;
    tree::update(id, |c| c.name = v);
    Ok(Value::Undefined)
}

fn w_null(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Null)
}

fn w_closed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    Ok(Value::Bool(tree::get(id).is_none()))
}

fn w_origin(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    Ok(s(tree::display_origin(&origin_of(id))))
}

/// [Replaceable]: a write defines an own data property that shadows the accessor.
fn w_replace(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    let name = slot_str(vm, ctx, 1);
    let v = arg(vm, ctx, 0);
    if let Some(g) = global_of(id) {
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(&name), Prop::data(v, WEC));
    }
    Ok(Value::Undefined)
}

fn w_noop(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Undefined)
}

/// `window.open()` from a nested document: popups are policy, and Aether's policy refuses them (null,
/// as a blocked popup reads).
fn w_open(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    crate::ledger::record_js("window.open-refused");
    Ok(Value::Null)
}

fn w_post_message(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    post_message(vm, ctx, id)
}

fn w_location(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    Ok(location_obj(vm, id))
}

fn w_location_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    let v = string(vm, &arg(vm, ctx, 0))?;
    navigate_ctx(vm, id, &v)?;
    Ok(Value::Undefined)
}

// =================================================================================================
// A nested context's Location (HTML §7.10.4)
// =================================================================================================

const LOC_PARTS: [&str; 9] = ["href", "origin", "protocol", "host", "hostname", "port", "pathname", "search", "hash"];

/// Context `id`'s Location: an object inheriting `Location.prototype` whose URL members read and
/// navigate THIS context (the prototype's members are the top document's).
fn location_obj(vm: &mut Vm, id: ContextId) -> Value {
    if let Some(o) = JS.with(|j| j.borrow().get(&id).and_then(|c| c.location)) {
        return Value::Object(o);
    }
    let saved = vm.cur_realm;
    vm.cur_realm = 0;
    let proto = dom::iface("Location").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    let o = vm.new_object(Some(proto));
    let fp = vm.intr().function_proto;
    for part in LOC_PARTS {
        let slots = vec![num(id as f64), s(part)];
        let gf = vm.make_native_with(&format!("get {part}"), 0, loc_get, false, Some(fp), slots.clone());
        let sf = (part != "origin").then(|| vm.make_native_with(&format!("set {part}"), 1, loc_set, false, Some(fp), slots));
        vm.heap.get_mut(o).props.insert(PropertyKey::from_str(part), Prop { slot: Slot::Accessor(Some(gf), sf), flags: E });
    }
    for (name, f, len) in [("assign", loc_assign as NativeFn, 1), ("replace", loc_assign, 1), ("reload", loc_reload, 0), ("toString", loc_get, 0)] {
        let part = if name == "toString" { "href" } else { name };
        let fo = vm.make_native_with(name, len, f, false, Some(fp), vec![num(id as f64), s(part)]);
        vm.heap.get_mut(o).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), E));
    }
    vm.cur_realm = saved;
    root(vm, Value::Object(o));
    JS.with(|j| {
        if let Some(c) = j.borrow_mut().get_mut(&id) {
            c.location = Some(o);
        }
    });
    Value::Object(o)
}

fn loc_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    let part = slot_str(vm, ctx, 1);
    let u = url_of(id);
    Ok(s(&match url::Url::parse(&u) {
        Ok(p) => crate::js::html::url_part(&p, &part),
        Err(_) if part == "href" => u,
        Err(_) => String::new(),
    }))
}

fn loc_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    let part = slot_str(vm, ctx, 1);
    let v = string(vm, &arg(vm, ctx, 0))?;
    if part == "href" {
        navigate_ctx(vm, id, &v)?;
        return Ok(Value::Undefined);
    }
    let Ok(mut u) = url::Url::parse(&url_of(id)) else { return Ok(Value::Undefined) };
    crate::js::html::set_url_part(&mut u, &part, &v);
    navigate_ctx(vm, id, &u.href())?;
    Ok(Value::Undefined)
}

fn loc_assign(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    let which = slot_str(vm, ctx, 1);
    need(vm, ctx, 1, "Location", &which)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    navigate_ctx(vm, id, &v)?;
    Ok(Value::Undefined)
}

fn loc_reload(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = slot_id(vm, ctx);
    let u = url_of(id);
    queue_nav(id, u, None);
    Ok(Value::Undefined)
}

fn strip_fragment(u: &str) -> &str {
    u.split('#').next().unwrap_or(u)
}

/// Navigates child context `id` to `input` (parsed against its document's base URL, else its
/// parent's — an `about:blank` document has no base of its own here). A fragment-only change is a
/// same-document navigation (`hashchange`); anything else is queued for the next service.
fn navigate_ctx(vm: &mut Vm, id: ContextId, input: &str) -> JsResult<()> {
    if id == TOP {
        return Ok(());
    }
    let Some(c) = tree::get(id) else { return Ok(()) };
    let url = dom::resolve_url(c.document, input).or_else(|| c.parent.and_then(|p| dom::resolve_url(document_of(p), input)));
    let Some(url) = url else {
        return throw_dom(vm, "SyntaxError", &format!("Failed to set the 'href' property on 'Location': '{input}' is not a valid URL."));
    };
    if url.contains('#') && strip_fragment(&url) == strip_fragment(&c.url) && c.readiness == Readiness::Complete {
        let doc = c.document;
        tree::update(id, |c| c.url = url.clone());
        page(|p| {
            if let Some(m) = p.docs.get_mut(&doc) {
                m.url = url.clone();
            }
        });
        if let Some(t) = target_key(id) {
            page(|p| p.tasks.push_back(Task::Event(t, "hashchange".into(), false)));
        }
        return Ok(());
    }
    queue_nav(id, url, None);
    Ok(())
}

fn queue_nav(ctx: ContextId, url: String, srcdoc: Option<String>) {
    NAVS.with(|q| {
        let mut q = q.borrow_mut();
        q.retain(|n| n.ctx != ctx);
        q.push(NavRequest { ctx, url, srcdoc });
    });
}

fn is_about_blank(u: &str) -> bool {
    u.split(['#', '?']).next() == Some("about:blank")
}

/// HTML §4.8.5 "process the iframe attributes" for child `id` of element `el`.
fn process_attributes(id: ContextId, el: usize, initial: bool) {
    let src = dom::attr_value(el, "src");
    let srcdoc = dom::attr_value(el, "srcdoc");
    tree::update(id, |c| c.seen_attrs = (src.clone(), srcdoc.clone()));
    if let Some(sd) = srcdoc {
        queue_nav(id, "about:srcdoc".into(), Some(sd));
        return;
    }
    let url = src.as_deref().map(str::trim).filter(|v| !v.is_empty()).and_then(|v| dom::resolve_url(dom::node_document(el), v));
    match url {
        Some(u) if !is_about_blank(&u) => queue_nav(id, u, None),
        _ if initial => {
            // The iframe load event steps for the initial about:blank.
            page(|p| p.tasks.push_back(Task::Event(TargetKey::Node(el), "load".into(), false)));
        }
        _ => queue_nav(id, "about:blank".into(), None),
    }
}

// =================================================================================================
// The browsing-context task source
// =================================================================================================

/// Brings the tree in line with the DOM: destroys the contexts whose containers left their documents,
/// creates the contexts of newly connected `<iframe>`s, re-processes changed `src`/`srcdoc`. A no-op
/// when the DOM has not changed since the last scan. Returns how many contexts changed.
fn scan(vm: &mut Vm) -> usize {
    let key = || (page(|p| p.version), with_doc(|d| d.nodes.len()));
    if LAST_SCAN.with(Cell::get) == key() || SCANNING.with(|b| b.replace(true)) {
        return 0;
    }
    let mut n = 0;
    for c in tree::all() {
        if c.id == TOP || tree::get(c.id).is_none() {
            continue;
        }
        if !container_live(&c) {
            discard(vm, c.id);
            n += 1;
        }
    }
    let mut queue = vec![TOP];
    let mut i = 0;
    while i < queue.len() {
        let id = queue[i];
        i += 1;
        let Some(c) = tree::get(id) else { continue };
        let iframes: Vec<usize> = with_doc(|d| {
            d.query_all(NodeId(c.document), |d, x| d.element(x).is_some_and(|e| e.is_html("iframe"))).into_iter().map(|x| x.0).collect()
        });
        for el in iframes {
            match tree::by_container(el) {
                None => {
                    if let Some(child) = create_child(vm, id, el) {
                        process_attributes(child, el, true);
                        n += 1;
                    }
                }
                Some(child) => {
                    let src = dom::attr_value(el, "src");
                    let srcdoc = dom::attr_value(el, "srcdoc");
                    let seen = tree::get(child).map(|c| c.seen_attrs).unwrap_or_default();
                    if seen.1 != srcdoc || (seen.0 != src && src.is_some()) {
                        process_attributes(child, el, false);
                        n += 1;
                    } else if seen.0 != src {
                        tree::update(child, |c| c.seen_attrs = (src, srcdoc));
                    }
                }
            }
        }
        queue.extend(tree::children(id));
    }
    LAST_SCAN.with(|l| l.set(key()));
    SCANNING.with(|b| b.set(false));
    n
}

/// The tree current before a getter answers (`contentWindow`, `frames[i]`, `length`, …).
pub fn ensure(vm: &mut Vm) {
    if crate::js::engine_poisoned() {
        return;
    }
    scan(vm);
}

/// The browsing-context task source: the scan, then every queued navigation (each a task: fetch,
/// populate, scripts, `DOMContentLoaded`, `load` at the window, `load` at the iframe). Called first in
/// every `event_loop::fire_due_timers`. Returns how many tasks ran.
pub fn service(vm: &mut Vm) -> usize {
    if crate::js::engine_poisoned() || SERVICING.with(|b| b.replace(true)) {
        return 0;
    }
    let mut n = scan(vm);
    let navs: Vec<NavRequest> = NAVS.with(|q| std::mem::take(&mut *q.borrow_mut()));
    for nav in navs {
        if crate::js::engine_poisoned() {
            break;
        }
        if navigate_now(vm, nav) {
            n += 1;
        }
    }
    SERVICING.with(|b| b.set(false));
    n
}

/// "Destroy a child navigable" for `id` (descendants first): `pagehide` and `unload` at its window,
/// then the context leaves the tree. Its realm and global live on for script that still holds them.
fn discard(vm: &mut Vm, id: ContextId) {
    for k in tree::children(id) {
        discard(vm, k);
    }
    if tree::get(id).is_none() {
        return;
    }
    unload_events(vm, id, false);
    tree::remove(id);
    NAVS.with(|q| q.borrow_mut().retain(|n| n.ctx != id));
}

/// The events of unloading context `id`'s active document (HTML §7.4.2.4): `beforeunload` (only when
/// a navigation replaces it), `pagehide`, `unload`.
fn unload_events(vm: &mut Vm, id: ContextId, navigating: bool) {
    let Some(t) = target_key(id) else { return };
    if navigating {
        events::fire_simple_cancelable(vm, t, "beforeunload");
    }
    events::fire_simple(vm, t, "pagehide", false, false);
    events::fire_simple(vm, t, "unload", false, false);
    crate::js::checkpoint(vm);
}

fn escape_text(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The markup a response becomes (HTML §7.4.6 "loading a document" by type): HTML as is, XML parsed
/// as HTML (ledgered), plain text in a `<pre>`, an image in an `<img>`.
fn markup_for(r: &crate::api::fetch::SubDocument) -> String {
    let ct = r.content_type.as_str();
    if ct.is_empty() || ct.contains("html") && !ct.contains("xml") {
        return r.body.clone();
    }
    if ct.contains("xml") {
        crate::ledger::record_dom("frames-xml-parsed-as-html");
        return r.body.clone();
    }
    if ct.starts_with("image/") {
        return format!("<html><body style=\"margin:0\"><img src=\"{}\"></body></html>", escape_text(&r.url).replace('"', "&quot;"));
    }
    format!("<html><head></head><body><pre>{}</pre></body></html>", escape_text(&r.body))
}

/// Percent-decodes a `javascript:` URL's script.
fn percent_decode(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(x) = u8::from_str_radix(&v[i + 1..i + 3], 16) {
                out.push(x);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One navigation of a child context (HTML §7.4.2), run as a task. False when nothing happened.
fn navigate_now(vm: &mut Vm, nav: NavRequest) -> bool {
    let Some(c) = tree::get(nav.ctx) else { return false };
    let Some(container) = c.container else { return false };
    if !container_live(&c) {
        return false;
    }
    // The nesting guard: a URL an ancestor already shows is not navigated to.
    if !nav.url.starts_with("about:") && !nav.url.starts_with("javascript:") {
        let me = strip_fragment(&nav.url).to_string();
        if tree::ancestors(nav.ctx).into_iter().any(|a| strip_fragment(&url_of(a)) == me) {
            crate::ledger::record_dom("frames-recursive-navigation-refused");
            return false;
        }
    }
    let parent_origin = c.parent.map(origin_of).unwrap_or_default();
    let sandbox = dom::attr_value(container, "sandbox").map(|v| Sandbox::parse(&v));
    // `javascript:` URLs: the script runs in the context's realm; a string result is the new document.
    let (url, markup, ctype, origin) = if let Some(sd) = nav.srcdoc {
        ("about:srcdoc".to_string(), sd, "text/html".to_string(), parent_origin)
    } else if is_about_blank(&nav.url) {
        (nav.url.clone(), String::new(), "text/html".to_string(), parent_origin)
    } else if let Some(code) = nav.url.strip_prefix("javascript:") {
        if sandbox.is_some_and(|s| !s.scripts) {
            return false;
        }
        let Some(realm) = realm_of(nav.ctx) else { return false };
        let src = percent_decode(code);
        let saved = vm.cur_realm;
        vm.cur_realm = realm;
        let r = crate::js::run_classic(vm, &src, "javascript-url");
        vm.cur_realm = saved;
        match r {
            Ok(Value::String(st)) => (c.url.clone(), st.to_rust(), "text/html".to_string(), origin_of(nav.ctx)),
            _ => return true,
        }
    } else {
        match crate::api::fetch::fetch_subdocument(&nav.url) {
            Ok(r) => {
                let o = if r.url.starts_with("data:") { "null".to_string() } else { tree::url_origin(&r.url) };
                let m = markup_for(&r);
                (r.url.clone(), m, if r.content_type.is_empty() { "text/html".to_string() } else { r.content_type.clone() }, o)
            }
            Err(e) => {
                crate::ledger::record_dom(&format!("frames-fetch-failed:{}", crate::js::clip(&nav.url, 48)));
                if std::env::var("UNAOS_JSDEBUG").is_ok() {
                    eprintln!("[frames] fetch {} failed: {e}", nav.url);
                }
                (nav.url.clone(), String::new(), "text/html".to_string(), tree::url_origin(&nav.url))
            }
        }
    };
    // The document being replaced unloads.
    if c.readiness != Readiness::InitialBlank {
        unload_events(vm, nav.ctx, true);
    }
    let origin = if sandbox.is_some_and(|s| !s.same_origin) || origin == "null" { format!("null#{}", nav.ctx) } else { origin };
    // A new Document for the new entry.
    let doc = dom::new_document(true, &ctype, &url);
    let quirks = populate(doc, &markup);
    page(|p| {
        if let Some(m) = p.docs.get_mut(&doc) {
            m.quirks = quirks;
        }
    });
    tree::update(nav.ctx, |c| {
        c.document = doc;
        c.url = url.clone();
        c.origin = origin;
        c.sandbox = sandbox;
        c.readiness = Readiness::Loading;
    });
    JS.with(|j| {
        if let Some(jc) = j.borrow_mut().get_mut(&nav.ctx) {
            jc.last_doc = doc;
        }
    });
    if sandbox.map_or(true, |s| s.scripts) {
        run_scripts(vm, nav.ctx, doc);
    } else {
        crate::ledger::record_dom("frames-sandboxed-scripts-not-run");
    }
    // "The end" of the nested document, then the iframe load event steps.
    tree::update(nav.ctx, |c| c.readiness = Readiness::Complete);
    events::fire_simple(vm, TargetKey::Node(doc), "DOMContentLoaded", true, false);
    crate::js::checkpoint(vm);
    if let Some(t) = target_key(nav.ctx) {
        events::fire_simple(vm, t, "load", false, false);
        crate::js::checkpoint(vm);
    }
    if tree::get(nav.ctx).is_some_and(|c| container_live(&c)) {
        events::fire_simple(vm, TargetKey::Node(container), "load", false, false);
        crate::js::checkpoint(vm);
    }
    true
}

/// Whether script element `el` is a classic script (HTML §4.12.1.1 steps 8–11); Some(false) for a
/// module, None for a data block.
fn script_kind(el: usize) -> Option<bool> {
    let ty = dom::attr_value(el, "type");
    let lang = dom::attr_value(el, "language");
    let essence = match (ty, lang) {
        (Some(t), _) if !t.trim().is_empty() => t.trim().to_ascii_lowercase(),
        (None, Some(l)) if !l.is_empty() => format!("text/{}", l.to_ascii_lowercase()),
        _ => return Some(true),
    };
    let essence = essence.split(';').next().unwrap_or("").trim().to_string();
    if essence == "module" {
        return Some(false);
    }
    let js = essence.ends_with("javascript") || essence.ends_with("ecmascript") || essence.starts_with("text/javascript1.") || matches!(essence.as_str(), "text/jscript" | "text/livescript");
    js.then_some(true)
}

fn inline_text(el: usize) -> String {
    with_doc(|d| {
        d.children(NodeId(el))
            .filter_map(|c| match d.data(c) {
                html_core::NodeData::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    })
}

/// Runs nested document `doc`'s classic scripts in tree order, in context `id`'s realm, with
/// `document.currentScript` set; an external script's `load`/`error` fires at its element.
fn run_scripts(vm: &mut Vm, id: ContextId, doc: usize) {
    let Some(realm) = realm_of(id) else { return };
    let scripts: Vec<usize> = with_doc(|d| {
        d.query_all(NodeId(doc), |d, x| d.element(x).is_some_and(|e| e.is_html("script"))).into_iter().map(|x| x.0).collect()
    });
    for el in scripts {
        if crate::js::engine_poisoned() {
            break;
        }
        // A script an earlier one removed (or a document.write re-open replaced) does not run.
        if dom::root_of(el) != doc {
            continue;
        }
        match script_kind(el) {
            Some(true) => {}
            Some(false) => {
                crate::ledger::record_js("frames-module-script-skipped");
                continue;
            }
            None => continue,
        }
        if dom::attr_value(el, "nomodule").is_some() {
            continue;
        }
        let (src, external) = match dom::attr_value(el, "src") {
            Some(v) => {
                let fetched = dom::resolve_url(doc, &v).map(|u| match page(|p| p.script_sources.get(&u).cloned()) {
                    Some(t) => Ok(t),
                    None => crate::js::host::fetch_script_text(&u),
                });
                match fetched {
                    Some(Ok(t)) => (t, true),
                    _ => {
                        events::fire_simple(vm, TargetKey::Node(el), "error", false, false);
                        continue;
                    }
                }
            }
            None => {
                let t = inline_text(el);
                if t.is_empty() {
                    continue;
                }
                (t, false)
            }
        };
        let saved = vm.cur_realm;
        vm.cur_realm = realm;
        let prev = page(|p| p.current_script.replace(el));
        let _ = crate::js::run_classic(vm, &src, "frame-script");
        page(|p| p.current_script = prev);
        vm.cur_realm = saved;
        if external {
            events::fire_simple(vm, TargetKey::Node(el), "load", false, false);
        }
    }
}

// =================================================================================================
// postMessage (HTML §9.3.3)
// =================================================================================================

/// `postMessage(message, targetOrigin, transfer)` / `postMessage(message, options)` on context
/// `target`'s window.
pub fn post_message(vm: &mut Vm, ctx: &CallCtx, target: ContextId) -> JsResult<Value> {
    need(vm, ctx, 1, "Window", "postMessage")?;
    let message = arg(vm, ctx, 0);
    let a1 = arg(vm, ctx, 1);
    let (target_origin, transfer) = match &a1 {
        Value::Undefined => ("/".to_string(), Value::Undefined),
        Value::Object(_) if !vm.is_callable(&a1) => {
            let to = idl::dict_member(vm, &a1, "targetOrigin")?;
            let tr = idl::dict_member(vm, &a1, "transfer")?;
            (if to.is_undefined() { "/".to_string() } else { string(vm, &to)? }, tr)
        }
        other => {
            let o = other.clone();
            (string(vm, &o)?, arg(vm, ctx, 2))
        }
    };
    let inc = incumbent(vm);
    let wanted = match target_origin.as_str() {
        "*" => None,
        "/" => Some(origin_of(inc)),
        t => match url::Url::parse(t) {
            Ok(u) => Some(tree::url_origin(&u.href())),
            Err(_) => {
                return throw_dom(vm, "SyntaxError", &format!("Failed to execute 'postMessage' on 'Window': Invalid target origin '{t}' in a call to 'postMessage'."));
            }
        },
    };
    // StructuredSerializeWithTransfer: a DataCloneError surfaces here, before the origin check drops
    // the message (as the spec orders it).
    let (data, ports) = match HELPERS.with(Cell::get) {
        Some((clone_fn, _)) => {
            let pair = vm.call(&Value::Object(clone_fn), &Value::Undefined, &[message, transfer])?;
            match pair.as_object() {
                Some(o) => (vm.get(o, &PropertyKey::Index(0))?, vm.get(o, &PropertyKey::Index(1))?),
                None => (Value::Undefined, Value::Undefined),
            }
        }
        None => (message, Value::Undefined),
    };
    if let Some(w) = wanted {
        if !tree::same_origin(&w, &origin_of(target)) {
            return Ok(Value::Undefined);
        }
    }
    let source = window_seen_by(vm, target, inc);
    let origin = tree::display_origin(&origin_of(inc)).to_string();
    let fp = vm.intr().function_proto;
    let f = vm.make_native_with("", 0, deliver_message, false, Some(fp), vec![num(target as f64), data, s(&origin), source, ports]);
    let r = root(vm, Value::Object(f));
    page(|p| p.tasks.push_back(Task::Callback(r)));
    Ok(Value::Undefined)
}

/// The posted message task: a `MessageEvent` at the target window, if it still has a context.
fn deliver_message(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let target = slot_id(vm, ctx);
    let Some(key) = target_key(target) else { return Ok(Value::Undefined) };
    let data = vm.native_slot(ctx.callee, 1);
    let origin = vm.native_slot(ctx.callee, 2);
    let source = vm.native_slot(ctx.callee, 3);
    let ports = vm.native_slot(ctx.callee, 4);
    let init = vm.new_plain_object();
    vm.create_data_property(init, PropertyKey::from_str("data"), data)?;
    vm.create_data_property(init, PropertyKey::from_str("origin"), origin)?;
    vm.create_data_property(init, PropertyKey::from_str("lastEventId"), s(""))?;
    vm.create_data_property(init, PropertyKey::from_str("source"), source)?;
    let e = events::new_event(vm, "MessageEvent", "message", false, false, Some(init));
    let ports = if ports.is_undefined() { Value::Object(vm.new_array(Vec::new())) } else { ports };
    vm.heap.get_mut(e).props.insert(PropertyKey::from_str("ports"), Prop::data(ports, E));
    events::dispatch(vm, e, key);
    Ok(Value::Undefined)
}

// =================================================================================================
// HTMLIFrameElement
// =================================================================================================

fn iframe_content_document(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = dom::this_element(vm, ctx)?;
    ensure(vm);
    let Some(id) = tree::by_container(el) else { return Ok(Value::Null) };
    let inc = incumbent(vm);
    if !same_origin(inc, id) {
        return Ok(Value::Null);
    }
    match tree::get(id) {
        Some(c) => Ok(wrap(vm, c.document)),
        None => Ok(Value::Null),
    }
}

fn iframe_content_window(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = dom::this_element(vm, ctx)?;
    ensure(vm);
    match tree::by_container(el) {
        Some(id) => Ok(window_for(vm, id)),
        None => Ok(Value::Null),
    }
}

/// `loading` reflects the enumerated attribute (invalid and missing value default "eager").
fn iframe_loading_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = dom::this_element(vm, ctx)?;
    let lazy = dom::attr_value(el, "loading").is_some_and(|v| v.eq_ignore_ascii_case("lazy"));
    Ok(s(if lazy { "lazy" } else { "eager" }))
}

fn iframe_loading_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = dom::this_element(vm, ctx)?;
    let v = string(vm, &arg(vm, ctx, 0))?;
    dom::set_attr(vm, el, "loading", &v);
    Ok(Value::Undefined)
}

// =================================================================================================
// Installation (top realm)
// =================================================================================================

/// The top realm's hidden helpers: StructuredSerialize/Deserialize in one pass (same agent, so the
/// serialized form never needs to exist) and the cross-origin WindowProxy subset.
const PRELUDE: &str = r#"
(function () {
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: false, enumerable: false, configurable: false });
    };
    var cloneErr = function (what) {
        return new DOMException("Failed to execute 'postMessage' on 'Window': " + what + " could not be cloned.", 'DataCloneError');
    };
    var clone = function (v, transfer) {
        var ports = [];
        var moved = 0;
        if (transfer !== undefined && transfer !== null) {
            if (typeof transfer !== 'object' || typeof transfer[Symbol.iterator] !== 'function') {
                throw new TypeError("Failed to execute 'postMessage' on 'Window': The provided value cannot be converted to a sequence.");
            }
            var list = Array.from(transfer);
            for (var i = 0; i < list.length; i++) {
                var t = list[i];
                if (t instanceof ArrayBuffer) { moved++; }
                else if (t !== null && typeof t === 'object' && typeof t.postMessage === 'function') {
                    if (ports.indexOf(t) >= 0) { throw new DOMException("Failed to execute 'postMessage' on 'Window': Message port is a duplicate of an earlier port.", 'DataCloneError'); }
                    ports.push(t);
                }
                else { throw cloneErr('Value at index ' + i); }
            }
        }
        var memory = new Map();
        var walk = function (x) {
            if (typeof x === 'symbol') { throw cloneErr(String(x)); }
            if (typeof x === 'function') { throw cloneErr(x.name ? 'function ' + x.name : 'A function'); }
            if (x === null || typeof x !== 'object') { return x; }
            if (memory.has(x)) { return memory.get(x); }
            if (ports.indexOf(x) >= 0) { memory.set(x, x); return x; }
            var tag = Object.prototype.toString.call(x);
            var out, k, keys, i;
            switch (tag) {
            case '[object Boolean]': out = Object(Boolean.prototype.valueOf.call(x)); break;
            case '[object Number]': out = Object(Number.prototype.valueOf.call(x)); break;
            case '[object String]': out = Object(String.prototype.valueOf.call(x)); break;
            case '[object BigInt]': out = Object(BigInt.prototype.valueOf.call(x)); break;
            case '[object Date]': out = new Date(Date.prototype.getTime.call(x)); break;
            case '[object RegExp]': out = new RegExp(x.source, x.flags); break;
            case '[object ArrayBuffer]': out = x.slice(0); break;
            case '[object Map]':
                out = new Map(); memory.set(x, out);
                x.forEach(function (val, key) { out.set(walk(key), walk(val)); });
                return out;
            case '[object Set]':
                out = new Set(); memory.set(x, out);
                x.forEach(function (val) { out.add(walk(val)); });
                return out;
            case '[object Error]':
                var C = { EvalError: EvalError, RangeError: RangeError, ReferenceError: ReferenceError, SyntaxError: SyntaxError, TypeError: TypeError, URIError: URIError }[x.name] || Error;
                out = Object.prototype.hasOwnProperty.call(x, 'message') ? new C(String(x.message)) : new C();
                break;
            case '[object Array]':
                out = new Array(x.length); memory.set(x, out);
                keys = Object.keys(x);
                for (i = 0; i < keys.length; i++) { out[keys[i]] = walk(x[keys[i]]); }
                return out;
            case '[object Object]':
            case '[object Arguments]':
                out = {}; memory.set(x, out);
                keys = Object.keys(x);
                for (i = 0; i < keys.length; i++) { k = keys[i]; out[k] = walk(x[k]); }
                return out;
            default:
                if (ArrayBuffer.isView(x)) {
                    var buf = walk(x.buffer);
                    out = x instanceof DataView ? new DataView(buf, x.byteOffset, x.byteLength) : new x.constructor(buf, x.byteOffset, x.length);
                    break;
                }
                throw cloneErr(tag.slice(8, -1) + ' object');
            }
            memory.set(x, out);
            return out;
        };
        var data = walk(v);
        if (moved) { __ledger('postMessage-transfer-arraybuffer-copied'); }
        return [data, Object.freeze(ports)];
    };
    hide('__aether_clone', clone);

    var XO = ['window', 'self', 'location', 'close', 'closed', 'focus', 'blur', 'frames', 'length', 'top', 'opener', 'parent', 'postMessage'];
    var secErr = function (k) {
        return new DOMException("Blocked a frame from accessing a cross-origin frame (property '" + String(k) + "').", 'SecurityError');
    };
    var isIndex = function (k) { return typeof k === 'string' && /^(0|[1-9][0-9]*)$/.test(k); };
    var xo = function (w) {
        var loc = null, me = null;
        var locProxy = function () {
            if (loc) { return loc; }
            loc = new Proxy(Object.create(null), {
                get: function (t, k) {
                    if (k === 'replace') { return function replace(u) { w.location.replace(u); }; }
                    if (k === 'then' || typeof k === 'symbol') { return undefined; }
                    throw secErr('location.' + String(k));
                },
                set: function (t, k, v) {
                    if (k === 'href') { w.location.href = v; return true; }
                    throw secErr('location.' + String(k));
                },
                has: function (t, k) { return k === 'href' || k === 'replace'; },
                getOwnPropertyDescriptor: function (t, k) { throw secErr('location.' + String(k)); },
                defineProperty: function (t, k) { throw secErr('location.' + String(k)); },
                deleteProperty: function (t, k) { throw secErr('location.' + String(k)); },
                ownKeys: function () { return ['href', 'replace']; },
                getPrototypeOf: function () { return null; },
                setPrototypeOf: function (t, p) { return p === null; },
            });
            return loc;
        };
        me = new Proxy(Object.create(null), {
            get: function (t, k) {
                if (isIndex(k)) { return w[k]; }
                if (k === 'window' || k === 'self' || k === 'frames') { return me; }
                if (k === 'location') { return locProxy(); }
                if (k === 'then' || typeof k === 'symbol') { return undefined; }
                if (XO.indexOf(k) >= 0) { return w[k]; }
                throw secErr(k);
            },
            set: function (t, k, v) {
                if (k === 'location') { w.location.href = v; return true; }
                throw secErr(k);
            },
            has: function (t, k) { return isIndex(k) ? w[k] !== undefined : XO.indexOf(k) >= 0; },
            getOwnPropertyDescriptor: function (t, k) {
                if (isIndex(k) && w[k] !== undefined) { return { value: w[k], writable: false, enumerable: true, configurable: true }; }
                if (XO.indexOf(k) >= 0) { return { value: me[k], writable: false, enumerable: false, configurable: true }; }
                throw secErr(k);
            },
            defineProperty: function (t, k) { throw secErr(k); },
            deleteProperty: function (t, k) { throw secErr(k); },
            ownKeys: function () {
                var out = [];
                for (var i = 0; i < w.length; i++) { out.push(String(i)); }
                return out.concat(XO);
            },
            getPrototypeOf: function () { return null; },
            setPrototypeOf: function (t, p) { return p === null; },
        });
        return me;
    };
    hide('__aether_xo', xo);
})();
"#;

/// Installs the frames surface into the top realm (`window::install` calls it last): the helpers, the
/// top context's record, and `HTMLIFrameElement`'s `contentDocument` / `contentWindow` /
/// `getSVGDocument()` / `loading`.
pub fn install(vm: &mut Vm) {
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("frames-prelude-failed:{}", crate::js::clip(&m, 64)));
        eprintln!("[aether] frames prelude failed: {m}");
    }
    let g = vm.realm().global;
    let c = vm.get(g, &PropertyKey::from_str("__aether_clone")).ok().and_then(|v| v.as_object());
    let x = vm.get(g, &PropertyKey::from_str("__aether_xo")).ok().and_then(|v| v.as_object());
    if let (Some(c), Some(x)) = (c, x) {
        root(vm, Value::Object(c));
        root(vm, Value::Object(x));
        HELPERS.with(|h| h.set(Some((c, x))));
    }
    let realm = vm.cur_realm;
    JS.with(|j| {
        j.borrow_mut().insert(TOP, JsCtx { realm, global: g, serial: 0, last_doc: dom::main_doc(), location: None, xo: None });
    });
    if let Some(i) = dom::iface("HTMLIFrameElement") {
        idl::attr(vm, i.proto, "contentDocument", iframe_content_document, None);
        idl::attr(vm, i.proto, "contentWindow", iframe_content_window, None);
        idl::attr(vm, i.proto, "loading", iframe_loading_get, Some(iframe_loading_set));
        idl::op(vm, i.proto, "getSVGDocument", 0, iframe_content_document);
    }
}
