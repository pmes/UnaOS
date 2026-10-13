//! AETHERSELECT (LEDGER SR69): the Selection API (W3C Selection API §2–§4) over the engine's one
//! selection (`crate::selection`), which the user's pointer and script share.
//!
//! - `document.getSelection()` / `window.getSelection()`: the document's one `Selection` object (the
//!   same object every call). `anchorNode`/`anchorOffset`/`focusNode`/`focusOffset`, `isCollapsed`,
//!   `rangeCount`, `type`, `direction`, `getRangeAt`, `addRange`, `removeRange`, `removeAllRanges`,
//!   `empty`, `collapse`/`setPosition`, `collapseToStart`/`collapseToEnd`, `extend`,
//!   `setBaseAndExtent`, `selectAllChildren`, `containsNode`, `toString` (the HTML `innerText`
//!   serialization of the selected range, as Chromium's).
//! - `selectionchange` fires at the document as a task after every change, coalesced while one is
//!   queued (Selection API §3).
//! - The selection's range is a real live DOM `Range` (AETHERRANGES, SR78; `js/range.rs` owns `Range`,
//!   `StaticRange` and `document.createRange` — one Range type, not two). The association is the live-
//!   range id plus the engine selection's generation it was made at and the direction ([`Assoc`]):
//!   `getRangeAt(0)` returns the same object until the selection changes (`===` holds; `addRange(r)`
//!   makes `r` that object); a DOM mutation moves the range by the §5.5 live-range steps and the engine's
//!   selection follows it ([`range_changed`]); script moving the associated Range moves the selection; a
//!   change from the pointer or a Selection method (a new generation) ends the association, and the next
//!   `getRangeAt` makes a new Range (Selection API: collapse/extend/setBaseAndExtent/selectAllChildren
//!   each "set this's range to a new range").
//!
//! CHARTER: Aether owns the browser (CODEX §2); this is inside the live handler (js_core binding, SR63).

use super::super::idl::*;
use super::super::{page, Task, TargetKey};
use super::super::range::{self, Bp};
use super::{kind, main_doc, node_arg, node_ref, root_of, with_doc, wrap_opt, NK};
use crate::selection::{self as model, Point, Selection};
use js_core::vm::*;
use std::cell::{Cell, RefCell};

/// Platform-object tag for `Selection` (idl.rs's list ends at 19; range.rs takes 40–41, traversal.rs
/// 42–43, controls.rs 44–45). Its one slot holds the associated Range object once script has seen it.
pub const T_SELECTION: u8 = 46;

/// The selection's associated live range (`range` registry id), valid while the engine selection's
/// generation is `epoch`; `backward` is the direction (anchor = end); `has_obj` = a Range object for `id`
/// is in the Selection object's slot 0 (script may hold it, so the id is never retired then).
#[derive(Clone, Copy)]
struct Assoc {
    id: usize,
    epoch: u64,
    backward: bool,
    has_obj: bool,
}

thread_local! {
    /// This page's Selection object (rooted; one per realm).
    static SEL_OBJ: Cell<Option<Obj>> = const { Cell::new(None) };
    static SEL_PROTO: Cell<Option<Obj>> = const { Cell::new(None) };
    static ASSOC: Cell<Option<Assoc>> = const { Cell::new(None) };
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

/// Installs `Selection` (called from `dom::install`; `Range` and `document.createRange` are range.rs's).
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
    // AETHERWPT3 (SR138): Selection API §3 modify() and getComposedRanges().
    op(vm, sp, "modify", 0, modify);
    op(vm, sp, "getComposedRanges", 0, get_composed_ranges);

    SEL_OBJ.with(|c| c.set(None));
    ASSOC.with(|c| c.set(None));
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
    let o = host_obj(vm, proto, T_SELECTION, vec![Value::Undefined]);
    root(vm, Value::Object(o));
    SEL_OBJ.with(|c| c.set(Some(o)));
    Ok(Value::Object(o))
}

// =================================================================================================
// The associated range (AETHERRANGES, SR78)
// =================================================================================================

fn bp(p: Point) -> Bp {
    Bp { node: p.node, off: p.offset }
}

fn point(b: Bp) -> Point {
    Point { node: b.node, offset: b.off }
}

/// (start, end, backward) of an engine selection, by the DOM's boundary-point order (§5.2).
fn start_end(x: Selection) -> (Bp, Bp, bool) {
    let (a, f) = (bp(x.anchor), bp(x.focus));
    let backward = root_of(a.node) == root_of(f.node) && range::cmp_bp(a, f) == std::cmp::Ordering::Greater;
    if backward {
        (f, a, true)
    } else {
        (a, f, false)
    }
}

/// The association if it is still the selection's (its generation is the current one). A stale one is
/// dropped, its live range retired unless script holds its object.
fn current() -> Option<Assoc> {
    let a = ASSOC.with(Cell::get)?;
    if a.epoch == model::generation() && model::get().is_some() && range::points(a.id).is_some() {
        return Some(a);
    }
    if !a.has_obj {
        range::retire(a.id);
    }
    ASSOC.with(|c| c.set(None));
    None
}

/// Makes the engine's selection (if any) a live range, so the coming DOM mutation moves it by the §5.5
/// steps. range.rs calls this first in every mutation hook; it is idempotent.
pub fn track() {
    if current().is_some() {
        return;
    }
    let Some(x) = model::get() else { return };
    let (s, e, backward) = start_end(x);
    let id = range::new_live(s, e);
    ASSOC.with(|c| c.set(Some(Assoc { id, epoch: model::generation(), backward, has_obj: false })));
}

/// range.rs calls this after every boundary-point write: when the associated range moved, the engine's
/// selection follows it (anchor/focus by the association's direction); a range moved out of the
/// document leaves no selection (Chromium; Selection API: the range's root must be the document).
pub fn range_changed() {
    let Some(a) = current() else { return };
    let Some((s, e)) = range::points(a.id) else { return };
    let sel = if a.backward {
        Selection { anchor: point(e), focus: point(s) }
    } else {
        Selection { anchor: point(s), focus: point(e) }
    };
    if model::get() == Some(sel) {
        return;
    }
    if !selectable(s.node, e.node) {
        model::set(None);
        current();
        return;
    }
    if model::set(Some(sel)) {
        ASSOC.with(|c| c.set(Some(Assoc { epoch: model::generation(), ..a })));
    }
}

/// The selection's Range object (made, and associated, on first sight): `getRangeAt(0)`.
fn range_object(vm: &mut Vm, sel: Obj) -> Option<Value> {
    model::get()?;
    track();
    let a = current()?;
    if a.has_obj {
        return Some(slot(vm, sel, 0));
    }
    let r = range::wrap_live(vm, a.id);
    set_slot(vm, sel, 0, r.clone());
    ASSOC.with(|c| c.set(Some(Assoc { has_obj: true, ..a })));
    Some(r)
}

// =================================================================================================
// Helpers
// =================================================================================================

fn this_sel(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Obj> {
    match this_tagged(vm, &ctx.this, T_SELECTION) {
        Some(o) => Ok(o),
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

/// AETHERWPT3 (SR138): whether a selection may hold `a` and `b` — both in one tree whose shadow-including root is
/// the document (the document tree, or one shadow tree in it: Selection API §2, the composed selection's
/// "shadow-including root is the document" check; a selection spanning two trees is not held).
fn selectable(a: usize, b: usize) -> bool {
    let (ra, rb) = (root_of(a), root_of(b));
    ra == rb && (ra == main_doc() || with_doc(|d| crate::dom::shadow::shadow_including_root(d, html_core::NodeId(ra)).0) == main_doc())
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
    let o = this_sel(vm, ctx)?;
    let i = offset_arg(vm, ctx, 0)?;
    match range_object(vm, o) {
        Some(r) if i == 0 => Ok(r),
        _ => throw_dom(vm, "IndexSizeError", &format!("The index provided ({i}) is greater than or equal to the maximum bound.")),
    }
}

/// `addRange(range)`: with no range yet and `range` in the document, `range` (that object, by
/// reference) becomes the selection's range, forwards. Chromium keeps an existing range (one at most).
fn add_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_sel(vm, ctx)?;
    need(vm, ctx, 1, "Selection", "addRange")?;
    let v = arg(vm, ctx, 0);
    let Some(id) = range::live_id(vm, &v) else {
        return vm.throw_type("Failed to execute 'addRange' on 'Selection': parameter 1 is not of type 'Range'.");
    };
    if model::get().is_some() {
        return Ok(Value::Undefined);
    }
    let Some((s, e)) = range::points(id) else { return Ok(Value::Undefined) };
    if !(in_document(s.node) && in_document(e.node)) {
        return Ok(Value::Undefined);
    }
    current();
    model::set(Some(Selection { anchor: point(s), focus: point(e) }));
    set_slot(vm, o, 0, v);
    ASSOC.with(|c| c.set(Some(Assoc { id, epoch: model::generation(), backward: false, has_obj: true })));
    Ok(Value::Undefined)
}

/// `removeRange(range)`: only the selection's own range object (by identity) can be removed.
fn remove_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_sel(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    let Some(id) = range::live_id(vm, &v) else {
        return vm.throw_type("Failed to execute 'removeRange' on 'Selection': parameter 1 is not of type 'Range'.");
    };
    match current() {
        Some(a) if a.id == id && a.has_obj => {
            model::set(None);
            current();
            set_slot(vm, o, 0, Value::Undefined);
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
    if selectable(n, n) {
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
    if selectable(x.anchor.node, n) {
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
    if selectable(a, f) {
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
    if selectable(n, n) {
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

/// `deleteFromDocument()`: `deleteContents()` on the selection's range (Selection API §2); the live
/// range collapses and the selection follows it.
fn delete_from_document(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    if model::get().is_none() {
        return Ok(Value::Undefined);
    }
    track();
    if let Some(a) = current() {
        range::delete_contents_impl(vm, a.id)?;
    }
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
// AETHERWPT3 (SR138): modify() and getComposedRanges()
// =================================================================================================

/// An optional DOMString argument ("" when absent: no value matches).
fn opt_str(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    match arg(vm, ctx, i) {
        Value::Undefined => Ok(String::new()),
        v => string(vm, &v),
    }
}

/// Selection API §3 `modify(alter, direction, granularity)` (js/caret.rs: the caret over the rendered text).
fn modify(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    let alter = opt_str(vm, ctx, 0)?;
    let direction = opt_str(vm, ctx, 1)?;
    let granularity = opt_str(vm, ctx, 2)?;
    let Some(x) = model::get() else { return Ok(Value::Undefined) };
    if !in_document(x.focus.node) {
        // The caret model reads the document tree's rendered text; a selection inside a shadow tree stays.
        return Ok(Value::Undefined);
    }
    if let Some(sel) = super::super::caret::modify(x, &alter, &direction, &granularity) {
        model::set(Some(sel));
    }
    Ok(Value::Undefined)
}

/// The ShadowRoot node behind `v`, or a TypeError naming `what`.
fn shadow_root_arg(vm: &mut Vm, v: &Value, what: &str) -> JsResult<usize> {
    let n = super::node_of(vm, v);
    match n.filter(|&n| with_doc(|d| crate::dom::shadow::is_shadow_root(d, html_core::NodeId(n)))) {
        Some(n) => Ok(n),
        None => vm.throw_type(&format!("Failed to execute 'getComposedRanges' on 'Selection': {what} is not of type 'ShadowRoot'.")),
    }
}

/// Selection API §3 `getComposedRanges(options)` (and the earlier `getComposedRanges(...shadowRoots)` form): the
/// selection's range as a StaticRange whose ends are rescoped out of every shadow tree that is not (inside) one
/// of the given shadow roots — an end inside such a tree moves to its host (before it for the start, after it
/// for the end), repeatedly.
fn get_composed_ranges(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_sel(vm, ctx)?;
    let mut roots: Vec<usize> = Vec::new();
    let first = arg(vm, ctx, 0);
    let first_is_root = super::node_of(vm, &first).is_some_and(|n| with_doc(|d| crate::dom::shadow::is_shadow_root(d, html_core::NodeId(n))));
    if first_is_root {
        for i in 0..ctx.argc {
            let v = arg(vm, ctx, i);
            roots.push(shadow_root_arg(vm, &v, &format!("parameter {}", i + 1))?);
        }
    } else if let Value::Object(o) = first {
        // GetComposedRangesOptions { sequence<ShadowRoot> shadowRoots = [] }
        let list = vm.get(o, &PropertyKey::from_str("shadowRoots"))?;
        match list {
            Value::Undefined => {}
            Value::Object(l) => {
                let n = vm.get(l, &PropertyKey::from_str("length"))?;
                let n = vm.to_number(&n)?;
                let n = if n.is_finite() && n > 0.0 { n as usize } else { 0 };
                for i in 0..n {
                    let v = vm.get(l, &PropertyKey::Index(i as u32))?;
                    roots.push(shadow_root_arg(vm, &v, "member shadowRoots")?);
                }
            }
            _ => return vm.throw_type("Failed to execute 'getComposedRanges' on 'Selection': The provided value cannot be converted to a sequence."),
        }
    } else if !first.is_nullish() {
        return vm.throw_type("Failed to execute 'getComposedRanges' on 'Selection': The provided value is not of type 'GetComposedRangesOptions'.");
    }
    let Some(x) = model::get() else { return Ok(Value::Object(vm.new_array(Vec::new()))) };
    let (s, e, _) = start_end(x);
    let rescope = |mut p: Bp, after: bool| -> Bp {
        loop {
            let r = root_of(p.node);
            let shadow = with_doc(|d| crate::dom::shadow::is_shadow_root(d, html_core::NodeId(r)));
            if !shadow {
                break;
            }
            let kept = roots.iter().any(|&sr| with_doc(|d| crate::dom::shadow::is_shadow_including_inclusive_ancestor(d, html_core::NodeId(r), html_core::NodeId(sr))));
            if kept {
                break;
            }
            let Some(host) = with_doc(|d| crate::dom::shadow::host_of(d, html_core::NodeId(r))) else { break };
            let Some((parent, i)) = index_in_parent(host.0) else { break };
            p = Bp { node: parent, off: if after { i + 1 } else { i } };
        }
        p
    };
    let (s, e) = (rescope(s, false), rescope(e, true));
    let r = range::make_static(vm, s, e);
    Ok(Value::Object(vm.new_array(vec![r])))
}
