//! AETHERSELECT (LEDGER SR69): the Selection API (W3C Selection API §2–§4) and the DOM Standard's
//! `Range` (§5.5) over the engine's one selection (`crate::selection`), which the user's pointer and
//! script share.
//!
//! - `document.getSelection()` / `window.getSelection()`: the document's one `Selection` object (the
//!   same object every call). `anchorNode`/`anchorOffset`/`focusNode`/`focusOffset`, `isCollapsed`,
//!   `rangeCount`, `type`, `direction`, `getRangeAt`, `addRange`, `removeRange`, `removeAllRanges`,
//!   `empty`, `collapse`/`setPosition`, `collapseToStart`/`collapseToEnd`, `extend`,
//!   `setBaseAndExtent`, `selectAllChildren`, `containsNode`, `toString` (the HTML `innerText`
//!   serialization of the selected range, as Chromium's).
//! - `selectionchange` fires at the document as a task after every change, coalesced while one is
//!   queued (Selection API §3).
//! - `Range` (`new Range()`, `document.createRange()`): boundary points, `setStart`/`setEnd` (+ Before /
//!   After), `collapse`, `selectNode`, `selectNodeContents`, `cloneRange`, `commonAncestorContainer`,
//!   `toString` (the Text data in the range, §5.5). A Range here is a static snapshot: it does not track
//!   later DOM mutations (live-range update steps are owed), and `getRangeAt(0)` returns a new Range
//!   describing the selection, not the object `addRange` was given.
//!
//! CHARTER: Aether owns the browser (CODEX §2); this is inside the live handler (js_core binding, SR63).

use super::super::idl::*;
use super::super::{page, Task, TargetKey};
use super::{kind, main_doc, node_arg, node_of, node_ref, root_of, with_doc, wrap, wrap_opt, NK};
use crate::selection::{self as model, Point, Selection};
use js_core::vm::*;
use std::cell::{Cell, RefCell};

/// Platform-object tags for the two interfaces (idl.rs's list ends at 19).
pub const T_SELECTION: u8 = 40;
pub const T_RANGE: u8 = 41;

thread_local! {
    /// This page's Selection object (rooted; one per realm).
    static SEL_OBJ: Cell<Option<Obj>> = const { Cell::new(None) };
    static SEL_PROTO: Cell<Option<Obj>> = const { Cell::new(None) };
    static RANGE_PROTO: Cell<Option<Obj>> = const { Cell::new(None) };
    /// A layout of the document for `toString`, by DOM version.
    static LAYOUT: RefCell<Option<(u64, crate::layout::LayoutTree)>> = const { RefCell::new(None) };
}

/// Queues `selectionchange` at the document unless one is already queued (the "has scheduled
/// selectionchange event" flag, Selection API §3).
pub fn queue_selectionchange() {
    page(|p| {
        let Some(doc) = p.doc.as_ref().map(|d| d.id().0) else { return };
        let queued = p.tasks.iter().any(|t| matches!(t, Task::Event(TargetKey::Node(n), ty, _) if *n == doc && ty == "selectionchange"));
        if !queued {
            p.tasks.push_back(Task::Event(TargetKey::Node(doc), "selectionchange".into(), false));
        }
    });
}

// =================================================================================================
// Install
// =================================================================================================

/// Installs `Selection`, `Range` and `document.createRange` (called from `dom::install`).
pub fn install(vm: &mut Vm) {
    let sel = interface(vm, "Selection", None, None);
    let sp = sel.proto;
    SEL_PROTO.with(|c| c.set(Some(sp)));
    attr(vm, sp, "anchorNode", anchor_node, None);
    attr(vm, sp, "anchorOffset", anchor_offset, None);
    attr(vm, sp, "focusNode", focus_node, None);
    attr(vm, sp, "focusOffset", focus_offset, None);
    attr(vm, sp, "isCollapsed", is_collapsed, None);
    attr(vm, sp, "rangeCount", range_count, None);
    attr(vm, sp, "type", sel_type, None);
    attr(vm, sp, "direction", direction, None);
    op(vm, sp, "getRangeAt", 1, get_range_at);
    op(vm, sp, "addRange", 1, add_range);
    op(vm, sp, "removeRange", 1, remove_range);
    op(vm, sp, "removeAllRanges", 0, remove_all_ranges);
    op(vm, sp, "empty", 0, remove_all_ranges);
    op(vm, sp, "collapse", 1, collapse);
    op(vm, sp, "setPosition", 1, collapse);
    op(vm, sp, "collapseToStart", 0, collapse_to_start);
    op(vm, sp, "collapseToEnd", 0, collapse_to_end);
    op(vm, sp, "extend", 1, extend);
    op(vm, sp, "setBaseAndExtent", 4, set_base_and_extent);
    op(vm, sp, "selectAllChildren", 1, select_all_children);
    op(vm, sp, "containsNode", 1, contains_node);
    op(vm, sp, "deleteFromDocument", 0, delete_from_document);
    op(vm, sp, "toString", 0, sel_to_string);

    let range = interface(vm, "Range", None, Some((range_ctor, 0)));
    let rp = range.proto;
    RANGE_PROTO.with(|c| c.set(Some(rp)));
    for (n, v) in [("START_TO_START", 0.0), ("START_TO_END", 1.0), ("END_TO_END", 2.0), ("END_TO_START", 3.0)] {
        konst(vm, range, n, v);
    }
    attr(vm, rp, "startContainer", range_start_container, None);
    attr(vm, rp, "startOffset", range_start_offset, None);
    attr(vm, rp, "endContainer", range_end_container, None);
    attr(vm, rp, "endOffset", range_end_offset, None);
    attr(vm, rp, "collapsed", range_collapsed, None);
    attr(vm, rp, "commonAncestorContainer", range_common_ancestor, None);
    op(vm, rp, "setStart", 2, range_set_start);
    op(vm, rp, "setEnd", 2, range_set_end);
    op(vm, rp, "setStartBefore", 1, range_set_start_before);
    op(vm, rp, "setStartAfter", 1, range_set_start_after);
    op(vm, rp, "setEndBefore", 1, range_set_end_before);
    op(vm, rp, "setEndAfter", 1, range_set_end_after);
    op(vm, rp, "collapse", 0, range_collapse);
    op(vm, rp, "selectNode", 1, range_select_node);
    op(vm, rp, "selectNodeContents", 1, range_select_node_contents);
    op(vm, rp, "cloneRange", 0, range_clone);
    op(vm, rp, "detach", 0, range_detach);
    op(vm, rp, "toString", 0, range_to_string);

    if let Some(doc) = page(|p| p.ifaces.get("Document").copied()) {
        op(vm, doc.proto, "createRange", 0, create_range);
    }
    SEL_OBJ.with(|c| c.set(None));
    LAYOUT.with(|c| *c.borrow_mut() = None);
}

/// Points `window.getSelection` and `Document.prototype.getSelection` at the live Selection (window.rs
/// installs a stub after `dom::install`; the engine calls this once the realm is built).
pub fn bind_get_selection(vm: &mut Vm) {
    let g = vm.realm().global;
    let f = vm.make_native("getSelection", 0, get_selection, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("getSelection"), Prop::data(Value::Object(f), WEC));
    if let Some(doc) = page(|p| p.ifaces.get("Document").copied()) {
        op(vm, doc.proto, "getSelection", 0, get_selection);
    }
}

/// `getSelection()`: the document's Selection object.
pub fn get_selection(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    if let Some(o) = SEL_OBJ.with(Cell::get) {
        return Ok(Value::Object(o));
    }
    let Some(proto) = SEL_PROTO.with(Cell::get) else { return Ok(Value::Null) };
    let o = host_obj(vm, proto, T_SELECTION, Vec::new());
    root(vm, Value::Object(o));
    SEL_OBJ.with(|c| c.set(Some(o)));
    Ok(Value::Object(o))
}

// =================================================================================================
// Helpers
// =================================================================================================

fn this_sel(vm: &mut Vm, ctx: &CallCtx) -> JsResult<()> {
    match this_tagged(vm, &ctx.this, T_SELECTION) {
        Some(_) => Ok(()),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn node_len(id: usize) -> u32 {
    with_doc(|d| model::node_length(d, id))
}

/// Whether `id` is in the document's tree (selections only hold such points).
fn in_document(id: usize) -> bool {
    root_of(id) == main_doc()
}

/// Validates a boundary point for the Selection/Range setters (DOM §5.2 "set the start or end").
fn check_point(vm: &mut Vm, id: usize, offset: u32) -> JsResult<()> {
    if kind(id) == NK::Doctype {
        return throw_dom(vm, "InvalidNodeTypeError", "The node provided is of type 'DocumentType'.");
    }
    let len = node_len(id);
    if offset > len {
        return throw_dom(vm, "IndexSizeError", &format!("The offset {offset} is larger than the node's length ({len})."));
    }
    Ok(())
}

fn offset_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<u32> {
    let v = arg(vm, ctx, i);
    unsigned_long(vm, &v)
}

fn key_of(p: Point) -> Option<model::Key> {
    let doc = node_ref(main_doc());
    let o = model::order(&doc);
    with_doc(|d| o.key(d, p))
}

fn ordered_points(a: Point, b: Point) -> (Point, Point) {
    match (key_of(a), key_of(b)) {
        (Some(ka), Some(kb)) if kb < ka => (b, a),
        _ => (a, b),
    }
}

fn index_in_parent(id: usize) -> Option<(usize, u32)> {
    with_doc(|d| {
        let p = d.parent(html_core::NodeId(id))?;
        let i = d.children(p).position(|c| c.0 == id)? as u32;
        Some((p.0, i))
    })
}

/// A fresh layout of the page for text serialization (cached by DOM version).
fn with_page_layout<R>(f: impl FnOnce(&crate::layout::LayoutTree) -> R) -> R {
    let version = page(|p| p.version);
    let cached = LAYOUT.with(|c| c.borrow_mut().take()).filter(|(v, _)| *v == version);
    let tree = match cached {
        Some((_, t)) => t,
        None => {
            let (w, h) = page(|p| p.viewport);
            let doc = node_ref(main_doc());
            let mut sheets: Vec<String> = with_doc(|d| {
                d.descendants(html_core::NodeId(main_doc()))
                    .filter(|i| d.element(*i).is_some_and(|e| e.is_html("style")))
                    .map(|i| d.text_content(i))
                    .collect()
            });
            sheets.extend(page(|p| p.external_css.clone()));
            let mut t = crate::layout::build_tree(&doc, w, h);
            crate::css::apply_stylesheets(&mut t, &sheets);
            t
        }
    };
    let r = f(&tree);
    LAYOUT.with(|c| *c.borrow_mut() = Some((version, tree)));
    r
}

fn new_range(vm: &mut Vm, start: Point, end: Point) -> Value {
    let proto = RANGE_PROTO.with(Cell::get).unwrap_or_else(|| vm.intr().object_proto);
    let o = host_obj(
        vm,
        proto,
        T_RANGE,
        vec![num(start.node as f64), num(start.offset as f64), num(end.node as f64), num(end.offset as f64)],
    );
    Value::Object(o)
}

fn range_points(vm: &Vm, o: Obj) -> (Point, Point) {
    let n = |i| slot_num(vm, o, i);
    (
        Point { node: n(0) as usize, offset: n(1) as u32 },
        Point { node: n(2) as usize, offset: n(3) as u32 },
    )
}

fn this_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Obj> {
    match this_tagged(vm, &ctx.this, T_RANGE) {
        Some(o) => Ok(o),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn set_range(vm: &mut Vm, o: Obj, start: Point, end: Point) {
    set_slot(vm, o, 0, num(start.node as f64));
    set_slot(vm, o, 1, num(start.offset as f64));
    set_slot(vm, o, 2, num(end.node as f64));
    set_slot(vm, o, 3, num(end.offset as f64));
}

// =================================================================================================
// Selection
// =================================================================================================

fn anchor_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    Ok(wrap_opt(vm, model::get().map(|s| s.anchor.node)))
}

fn anchor_offset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    Ok(num(model::get().map(|s| s.anchor.offset).unwrap_or(0) as f64))
}

fn focus_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    Ok(wrap_opt(vm, model::get().map(|s| s.focus.node)))
}

fn focus_offset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    Ok(num(model::get().map(|s| s.focus.offset).unwrap_or(0) as f64))
}

fn is_collapsed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    Ok(Value::Bool(model::get().is_none_or(|s| s.is_collapsed())))
}

fn range_count(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    Ok(num(if model::get().is_some() { 1.0 } else { 0.0 }))
}

fn sel_type(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    Ok(s(match model::get() {
        None => "None",
        Some(x) if x.is_collapsed() => "Caret",
        Some(_) => "Range",
    }))
}

fn direction(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    Ok(s(match model::get() {
        Some(x) if !x.is_collapsed() => match (key_of(x.anchor), key_of(x.focus)) {
            (Some(a), Some(f)) if f < a => "backward",
            _ => "forward",
        },
        _ => "none",
    }))
}

fn get_range_at(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    let i = offset_arg(vm, ctx, 0)?;
    match model::get() {
        Some(x) if i == 0 => {
            let (a, b) = ordered_points(x.anchor, x.focus);
            Ok(new_range(vm, a, b))
        }
        _ => throw_dom(vm, "IndexSizeError", &format!("The index provided ({i}) is greater than or equal to the maximum bound.")),
    }
}

fn add_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    need(vm, ctx, 1, "Selection", "addRange")?;
    let v = arg(vm, ctx, 0);
    let Some(o) = v.as_object().filter(|o| tag_of(vm, *o) == Some(T_RANGE)) else {
        return vm.throw_type("Failed to execute 'addRange' on 'Selection': parameter 1 is not of type 'Range'.");
    };
    // Chromium keeps an existing range (one range at most); a range outside the document is ignored.
    if model::get().is_none() {
        let (a, b) = range_points(vm, o);
        if in_document(a.node) && in_document(b.node) {
            model::set(Some(Selection { anchor: a, focus: b }));
        }
    }
    Ok(Value::Undefined)
}

fn remove_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    let Some(o) = v.as_object().filter(|o| tag_of(vm, *o) == Some(T_RANGE)) else {
        return vm.throw_type("Failed to execute 'removeRange' on 'Selection': parameter 1 is not of type 'Range'.");
    };
    let (a, b) = range_points(vm, o);
    match model::get() {
        Some(x) if ordered_points(x.anchor, x.focus) == (a, b) => {
            model::set(None);
            Ok(Value::Undefined)
        }
        _ => throw_dom(vm, "NotFoundError", "The given range isn't in document."),
    }
}

fn remove_all_ranges(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    model::set(None);
    Ok(Value::Undefined)
}

fn collapse(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    if v.is_nullish() {
        model::set(None);
        return Ok(Value::Undefined);
    }
    let n = node_arg(vm, ctx, 0, "Selection", "collapse")?;
    let off = offset_arg(vm, ctx, 1)?;
    check_point(vm, n, off)?;
    if in_document(n) {
        model::set(Some(Selection::collapsed(Point { node: n, offset: off })));
    }
    Ok(Value::Undefined)
}

fn collapse_to_start(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    let Some(x) = model::get() else {
        return throw_dom(vm, "InvalidStateError", "There is no selection to collapse.");
    };
    let (a, _) = ordered_points(x.anchor, x.focus);
    model::set(Some(Selection::collapsed(a)));
    Ok(Value::Undefined)
}

fn collapse_to_end(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    let Some(x) = model::get() else {
        return throw_dom(vm, "InvalidStateError", "There is no selection to collapse.");
    };
    let (_, b) = ordered_points(x.anchor, x.focus);
    model::set(Some(Selection::collapsed(b)));
    Ok(Value::Undefined)
}

fn extend(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    let n = node_arg(vm, ctx, 0, "Selection", "extend")?;
    let off = offset_arg(vm, ctx, 1)?;
    let Some(x) = model::get() else {
        return throw_dom(vm, "InvalidStateError", "This Selection object doesn't have any Ranges.");
    };
    check_point(vm, n, off)?;
    if in_document(n) {
        model::set(Some(Selection { anchor: x.anchor, focus: Point { node: n, offset: off } }));
    }
    Ok(Value::Undefined)
}

fn set_base_and_extent(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    need(vm, ctx, 4, "Selection", "setBaseAndExtent")?;
    let a = node_arg(vm, ctx, 0, "Selection", "setBaseAndExtent")?;
    let ao = offset_arg(vm, ctx, 1)?;
    let f = node_arg(vm, ctx, 2, "Selection", "setBaseAndExtent")?;
    let fo = offset_arg(vm, ctx, 3)?;
    check_point(vm, a, ao)?;
    check_point(vm, f, fo)?;
    if in_document(a) && in_document(f) {
        model::set(Some(Selection { anchor: Point { node: a, offset: ao }, focus: Point { node: f, offset: fo } }));
    }
    Ok(Value::Undefined)
}

fn select_all_children(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    let n = node_arg(vm, ctx, 0, "Selection", "selectAllChildren")?;
    if kind(n) == NK::Doctype {
        return throw_dom(vm, "InvalidNodeTypeError", "The node provided is of type 'DocumentType'.");
    }
    if in_document(n) {
        let len = node_len(n);
        model::set(Some(Selection { anchor: Point { node: n, offset: 0 }, focus: Point { node: n, offset: len } }));
    }
    Ok(Value::Undefined)
}

fn contains_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    let n = node_arg(vm, ctx, 0, "Selection", "containsNode")?;
    let partial = boolean(vm, &arg(vm, ctx, 1));
    let Some(x) = model::get() else { return Ok(Value::Bool(false)) };
    let Some((p, i)) = index_in_parent(n) else { return Ok(Value::Bool(false)) };
    let (a, b) = ordered_points(x.anchor, x.focus);
    let (ka, kb) = (key_of(a), key_of(b));
    let before = key_of(Point { node: p, offset: i });
    let after = key_of(Point { node: p, offset: i + 1 });
    let r = match (ka, kb, before, after) {
        (Some(ka), Some(kb), Some(s), Some(e)) => {
            if partial {
                // Some part of the node is in the range.
                s < kb && e > ka
            } else {
                ka <= s && e <= kb
            }
        }
        _ => false,
    };
    Ok(Value::Bool(r))
}

fn delete_from_document(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    crate::ledger::record_dom("Selection.deleteFromDocument");
    Ok(Value::Undefined)
}

fn sel_to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    let Some(x) = model::get() else { return Ok(s("")) };
    if x.is_collapsed() {
        return Ok(s(""));
    }
    let doc = node_ref(main_doc());
    let text = with_page_layout(|l| model::range_text(l, &doc, x.anchor, x.focus));
    Ok(s(&text))
}

// =================================================================================================
// Range
// =================================================================================================

fn range_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'Range': Please use the 'new' operator");
    }
    let d = main_doc();
    let p = Point { node: d, offset: 0 };
    Ok(new_range(vm, p, p))
}

fn create_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = node_of(vm, &ctx.this).unwrap_or_else(main_doc);
    let p = Point { node: d, offset: 0 };
    Ok(new_range(vm, p, p))
}

fn range_start_container(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let (a, _) = range_points(vm, o);
    Ok(wrap(vm, a.node))
}
fn range_start_offset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    Ok(num(range_points(vm, o).0.offset as f64))
}
fn range_end_container(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let (_, b) = range_points(vm, o);
    Ok(wrap(vm, b.node))
}
fn range_end_offset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    Ok(num(range_points(vm, o).1.offset as f64))
}
fn range_collapsed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let (a, b) = range_points(vm, o);
    Ok(Value::Bool(a == b))
}

fn range_common_ancestor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let (a, b) = range_points(vm, o);
    let anc = with_doc(|d| {
        let mut chain = vec![a.node];
        let mut n = d.parent(html_core::NodeId(a.node));
        while let Some(p) = n {
            chain.push(p.0);
            n = d.parent(p);
        }
        let mut m = Some(html_core::NodeId(b.node));
        while let Some(x) = m {
            if chain.contains(&x.0) {
                return x.0;
            }
            m = d.parent(x);
        }
        a.node
    });
    Ok(wrap(vm, anc))
}

/// DOM §5.5 "set the start or end": a start after the end (or in another tree) collapses the range there.
fn set_boundary(vm: &mut Vm, o: Obj, p: Point, start: bool) {
    let (a, b) = range_points(vm, o);
    let other_tree = root_of(p.node) != root_of(if start { b.node } else { a.node });
    let (ka, kb, kp) = (key_of(a), key_of(b), key_of(p));
    if start {
        let collapse = other_tree || matches!((kp, kb), (Some(kp), Some(kb)) if kp > kb);
        set_range(vm, o, p, if collapse { p } else { b });
    } else {
        let collapse = other_tree || matches!((kp, ka), (Some(kp), Some(ka)) if kp < ka);
        set_range(vm, o, if collapse { p } else { a }, p);
    }
}

fn range_set_start(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    need(vm, ctx, 2, "Range", "setStart")?;
    let n = node_arg(vm, ctx, 0, "Range", "setStart")?;
    let off = offset_arg(vm, ctx, 1)?;
    check_point(vm, n, off)?;
    set_boundary(vm, o, Point { node: n, offset: off }, true);
    Ok(Value::Undefined)
}

fn range_set_end(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    need(vm, ctx, 2, "Range", "setEnd")?;
    let n = node_arg(vm, ctx, 0, "Range", "setEnd")?;
    let off = offset_arg(vm, ctx, 1)?;
    check_point(vm, n, off)?;
    set_boundary(vm, o, Point { node: n, offset: off }, false);
    Ok(Value::Undefined)
}

fn parent_point(vm: &mut Vm, ctx: &CallCtx, method: &str, after: bool) -> JsResult<Point> {
    let n = node_arg(vm, ctx, 0, "Range", method)?;
    match index_in_parent(n) {
        Some((p, i)) => Ok(Point { node: p, offset: i + u32::from(after) }),
        None => throw_dom(vm, "InvalidNodeTypeError", "the given Node has no parent."),
    }
}

fn range_set_start_before(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let p = parent_point(vm, ctx, "setStartBefore", false)?;
    set_boundary(vm, o, p, true);
    Ok(Value::Undefined)
}
fn range_set_start_after(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let p = parent_point(vm, ctx, "setStartAfter", true)?;
    set_boundary(vm, o, p, true);
    Ok(Value::Undefined)
}
fn range_set_end_before(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let p = parent_point(vm, ctx, "setEndBefore", false)?;
    set_boundary(vm, o, p, false);
    Ok(Value::Undefined)
}
fn range_set_end_after(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let p = parent_point(vm, ctx, "setEndAfter", true)?;
    set_boundary(vm, o, p, false);
    Ok(Value::Undefined)
}

fn range_collapse(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let to_start = boolean(vm, &arg(vm, ctx, 0));
    let (a, b) = range_points(vm, o);
    let p = if to_start { a } else { b };
    set_range(vm, o, p, p);
    Ok(Value::Undefined)
}

fn range_select_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let n = node_arg(vm, ctx, 0, "Range", "selectNode")?;
    match index_in_parent(n) {
        Some((p, i)) => {
            set_range(vm, o, Point { node: p, offset: i }, Point { node: p, offset: i + 1 });
            Ok(Value::Undefined)
        }
        None => throw_dom(vm, "InvalidNodeTypeError", "the given Node has no parent."),
    }
}

fn range_select_node_contents(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let n = node_arg(vm, ctx, 0, "Range", "selectNodeContents")?;
    if kind(n) == NK::Doctype {
        return throw_dom(vm, "InvalidNodeTypeError", "The node provided is of type 'DocumentType'.");
    }
    let len = node_len(n);
    set_range(vm, o, Point { node: n, offset: 0 }, Point { node: n, offset: len });
    Ok(Value::Undefined)
}

fn range_clone(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let (a, b) = range_points(vm, o);
    Ok(new_range(vm, a, b))
}

fn range_detach(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_range(vm, ctx)?;
    Ok(Value::Undefined)
}

/// DOM §5.5 stringifier: the data of the Text nodes in the range (partial at either end).
fn range_to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_range(vm, ctx)?;
    let (a, b) = range_points(vm, o);
    let doc = node_ref(main_doc());
    let ord = model::order(&doc);
    let out = with_doc(|d| {
        let (Some(ka), Some(kb)) = (ord.key(d, a), ord.key(d, b)) else { return String::new() };
        let mut out = String::new();
        for id in std::iter::once(html_core::Document::ROOT).chain(d.descendants(html_core::Document::ROOT)) {
            let html_core::NodeData::Text(t) = d.data(id) else { continue };
            let Some(pre) = ord.pre(id.0) else { continue };
            let len = t.encode_utf16().count() as u32;
            let s = if ka.0 < pre { 0 } else if ka.0 == pre { ka.1.max(0) as u32 } else { continue };
            let e = if kb.0 > pre { len } else if kb.0 == pre { (kb.1.max(0) as u32).min(len) } else { continue };
            if e > s {
                out.push_str(&t[model::utf16_to_byte(t, s)..model::utf16_to_byte(t, e)]);
            }
        }
        out
    });
    Ok(s(&out))
}
