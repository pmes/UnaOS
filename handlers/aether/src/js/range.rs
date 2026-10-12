//! DOM §5 ranges (AETHERWPT, SR72): AbstractRange, StaticRange and the live Range, with the boundary-
//! point algebra, the content operations (deleteContents, extractContents, cloneContents, insertNode,
//! surroundContents), the stringifier, and the live-range mutation rules (§4.2.3 insert/remove, §4.10
//! replace data, §4.11 split a Text node, §4.4 normalize) that the mutation algorithms in [`super::dom`]
//! call into.
//!
//! A live Range is a platform object whose one slot is its serial; its boundary points live in a
//! thread-local registry the mutation hooks walk. (No finalizer tells the host a Range died, so the
//! registry lives as long as the page — a page's ranges are few next to its nodes.)

use super::dom::{self, kind, node_of, parent_of, wrap, NK};
use super::idl::*;
use html_core::{NodeData, NodeId};
use js_core::string::JsStr;
use js_core::vm::*;
use std::cell::RefCell;
use std::cmp::Ordering;

/// Platform-object tags of this family (idl.rs numbers 1–19; ranges and traversal take 40+).
pub const T_RANGE: u8 = 40;
pub const T_STATIC_RANGE: u8 = 41;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bp {
    pub node: usize,
    pub off: u32,
}

#[derive(Clone, Copy, Debug)]
struct Live {
    start: Bp,
    end: Bp,
    /// Temporary ranges (the extract/clone subranges) are retired after use.
    alive: bool,
}

thread_local! {
    static RANGES: RefCell<Vec<Live>> = const { RefCell::new(Vec::new()) };
}

pub fn reset() {
    RANGES.with(|r| r.borrow_mut().clear());
}

fn get(id: usize) -> (Bp, Bp) {
    RANGES.with(|r| {
        let r = r.borrow();
        let l = r[id];
        (l.start, l.end)
    })
}

fn set_start_raw(id: usize, b: Bp) {
    RANGES.with(|r| r.borrow_mut()[id].start = b);
}

fn set_end_raw(id: usize, b: Bp) {
    RANGES.with(|r| r.borrow_mut()[id].end = b);
}

fn new_live(start: Bp, end: Bp) -> usize {
    RANGES.with(|r| {
        let mut r = r.borrow_mut();
        r.push(Live { start, end, alive: true });
        r.len() - 1
    })
}

fn retire(id: usize) {
    RANGES.with(|r| r.borrow_mut()[id].alive = false);
}

/// Applies `f` to every live boundary point.
fn each_bp(mut f: impl FnMut(&mut Bp)) {
    RANGES.with(|r| {
        for l in r.borrow_mut().iter_mut().filter(|l| l.alive) {
            f(&mut l.start);
            f(&mut l.end);
        }
    });
}

fn any_ranges() -> bool {
    RANGES.with(|r| r.borrow().iter().any(|l| l.alive))
}

// ------------------------------------------------------------------------------------------------ tree helpers

pub fn is_char_data(n: usize) -> bool {
    matches!(kind(n), NK::Text | NK::Comment | NK::Pi)
}

/// DOM "length" of a node.
pub fn node_len(n: usize) -> u32 {
    match kind(n) {
        NK::Doctype => 0,
        NK::Text | NK::Comment | NK::Pi => dom::with_doc(|d| dom::char_data(d, n)).encode_utf16().count() as u32,
        _ => dom::with_doc(|d| d.children(NodeId(n)).count() as u32),
    }
}

/// DOM "index" of a node among its siblings.
pub fn index_of(n: usize) -> u32 {
    dom::with_doc(|d| {
        let mut i = 0;
        let mut cur = d.prev_sibling(NodeId(n));
        while let Some(p) = cur {
            i += 1;
            cur = d.prev_sibling(p);
        }
        i
    })
}

/// Is `a` an inclusive ancestor of `n`?
pub fn inclusive_ancestor(a: usize, n: usize) -> bool {
    dom::with_doc(|d| {
        let mut cur = Some(NodeId(n));
        while let Some(c) = cur {
            if c.0 == a {
                return true;
            }
            cur = d.parent(c);
        }
        false
    })
}

fn child_at(n: usize, i: u32) -> Option<usize> {
    dom::with_doc(|d| d.children(NodeId(n)).nth(i as usize).map(|c| c.0))
}

/// Position of boundary point `a` relative to `b` (both in one tree): Less = before.
pub fn cmp_bp(a: Bp, b: Bp) -> Ordering {
    if a.node == b.node {
        return a.off.cmp(&b.off);
    }
    if dom::with_doc(|d| dom::precedes(d, b.node, a.node)) {
        return cmp_bp(b, a).reverse();
    }
    if inclusive_ancestor(a.node, b.node) {
        let mut child = b.node;
        while let Some(p) = parent_of(child) {
            if p == a.node {
                break;
            }
            child = p;
        }
        if index_of(child) < a.off {
            return Ordering::Greater;
        }
    }
    Ordering::Less
}

fn utf16_of(n: usize) -> Vec<u16> {
    dom::with_doc(|d| dom::char_data(d, n)).encode_utf16().collect()
}

fn substring(n: usize, off: u32, count: u32) -> String {
    let v = utf16_of(n);
    let a = (off as usize).min(v.len());
    let b = (off as usize + count as usize).min(v.len());
    String::from_utf16_lossy(&v[a..b])
}

// ------------------------------------------------------------------------------------------------ mutation hooks

/// §4.2.3 "remove" steps 4–7, run before `node` leaves `parent` (where it sits at `index`).
pub fn removing(node: usize, parent: usize, index: u32) {
    if !any_ranges() {
        return;
    }
    each_bp(|b| {
        if inclusive_ancestor(node, b.node) {
            *b = Bp { node: parent, off: index };
        } else if b.node == parent && b.off > index {
            b.off -= 1;
        }
    });
}

/// §4.2.3 "insert" step 4: `count` nodes went in at `index` of `parent`.
pub fn inserted(parent: usize, index: u32, count: u32) {
    if !any_ranges() {
        return;
    }
    each_bp(|b| {
        if b.node == parent && b.off > index {
            b.off += count;
        }
    });
}

/// §4.10 "replace data" steps 8–11.
pub fn replaced_data(node: usize, offset: u32, count: u32, added: u32) {
    if !any_ranges() {
        return;
    }
    each_bp(|b| {
        if b.node != node {
            return;
        }
        if b.off > offset && b.off <= offset.saturating_add(count) {
            b.off = offset;
        } else if b.off > offset.saturating_add(count) {
            b.off = b.off - count + added;
        }
    });
}

/// §4.11 "split a Text node" step 7.2–7.5 (after `new_node` went in after `node` under `parent`).
pub fn split(node: usize, new_node: usize, offset: u32, parent: Option<usize>) {
    if !any_ranges() {
        return;
    }
    let idx1 = parent.map(|_| index_of(node) + 1);
    each_bp(|b| {
        if b.node == node && b.off > offset {
            *b = Bp { node: new_node, off: b.off - offset };
        } else if Some(b.node) == parent && Some(b.off) == idx1 {
            b.off += 1;
        }
    });
}

/// §4.4 normalize, per merged sibling: `current` (at `index` under `parent`) folds into `into` at `len`.
pub fn merged(into: usize, current: usize, parent: usize, index: u32, len: u32) {
    if !any_ranges() {
        return;
    }
    each_bp(|b| {
        if b.node == current {
            *b = Bp { node: into, off: b.off + len };
        } else if b.node == parent && b.off == index {
            *b = Bp { node: into, off: len };
        }
    });
}

// ------------------------------------------------------------------------------------------------ objects

fn proto_of(name: &str) -> Obj {
    dom::proto(name)
}

pub fn make_range(vm: &mut Vm, start: Bp, end: Bp) -> Value {
    let id = new_live(start, end);
    let p = proto_of("Range");
    Value::Object(host_obj(vm, p, T_RANGE, vec![num(id as f64)]))
}

fn this_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    match this_tagged(vm, &ctx.this, T_RANGE) {
        Some(o) => Ok(slot_num(vm, o, 0) as usize),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn range_arg(vm: &mut Vm, v: &Value, method: &str) -> JsResult<usize> {
    match v.as_object().filter(|o| tag_of(vm, *o) == Some(T_RANGE)) {
        Some(o) => Ok(slot_num(vm, o, 0) as usize),
        None => vm.throw_type(&format!("Failed to execute '{method}' on 'Range': parameter 1 is not of type 'Range'.")),
    }
}

/// The boundary points of `this` as an AbstractRange (live or static).
fn this_abstract(vm: &mut Vm, ctx: &CallCtx) -> JsResult<(Bp, Bp)> {
    if let Some(o) = this_tagged(vm, &ctx.this, T_RANGE) {
        return Ok(get(slot_num(vm, o, 0) as usize));
    }
    if let Some(o) = this_tagged(vm, &ctx.this, T_STATIC_RANGE) {
        let g = |i| slot_num(vm, o, i);
        return Ok((Bp { node: g(0) as usize, off: g(1) as u32 }, Bp { node: g(2) as usize, off: g(3) as u32 }));
    }
    vm.throw_type("Illegal invocation")
}

fn node_param(vm: &mut Vm, ctx: &CallCtx, i: usize, method: &str) -> JsResult<usize> {
    let v = arg(vm, ctx, i);
    match node_of(vm, &v) {
        Some(n) => Ok(n),
        None => vm.throw_type(&format!("Failed to execute '{method}' on 'Range': parameter {} is not of type 'Node'.", i + 1)),
    }
}

// ------------------------------------------------------------------------------------------------ AbstractRange

fn ar_start_container(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (s, _) = this_abstract(vm, ctx)?;
    Ok(wrap(vm, s.node))
}
fn ar_start_offset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (s, _) = this_abstract(vm, ctx)?;
    Ok(num(s.off as f64))
}
fn ar_end_container(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, e) = this_abstract(vm, ctx)?;
    Ok(wrap(vm, e.node))
}
fn ar_end_offset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, e) = this_abstract(vm, ctx)?;
    Ok(num(e.off as f64))
}
fn ar_collapsed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (s, e) = this_abstract(vm, ctx)?;
    Ok(Value::Bool(s == e))
}

// ------------------------------------------------------------------------------------------------ StaticRange

fn static_range_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'StaticRange': Please use the 'new' operator");
    }
    let init = arg(vm, ctx, 0);
    if !matches!(init, Value::Object(_)) {
        return vm.throw_type("Failed to construct 'StaticRange': 1 argument required, but only 0 present.");
    }
    let mut vals = Vec::new();
    for (m, is_node) in [("startContainer", true), ("startOffset", false), ("endContainer", true), ("endOffset", false)] {
        let v = dict_member(vm, &init, m)?;
        if v.is_undefined() {
            return vm.throw_type(&format!("Failed to construct 'StaticRange': required member {m} is undefined."));
        }
        if is_node {
            let is_attr = v.as_object().is_some_and(|o| tag_of(vm, o) == Some(T_ATTR));
            if is_attr {
                return throw_dom(vm, "InvalidNodeTypeError", "The container is an Attr.");
            }
            let Some(n) = node_of(vm, &v) else {
                return vm.throw_type(&format!("Failed to construct 'StaticRange': member {m} is not of type 'Node'."));
            };
            if kind(n) == NK::Doctype {
                return throw_dom(vm, "InvalidNodeTypeError", "The container is a DocumentType.");
            }
            vals.push(num(n as f64));
        } else {
            let n = unsigned_long(vm, &v)?;
            vals.push(num(n as f64));
        }
    }
    let default = proto_of("StaticRange");
    let p = proto_from_new_target(vm, &ctx.new_target, default)?;
    Ok(Value::Object(host_obj(vm, p, T_STATIC_RANGE, vals)))
}

// ------------------------------------------------------------------------------------------------ Range

fn range_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'Range': Please use the 'new' operator");
    }
    let d = dom::main_doc();
    let r = make_range(vm, Bp { node: d, off: 0 }, Bp { node: d, off: 0 });
    let default = proto_of("Range");
    let p = proto_from_new_target(vm, &ctx.new_target, default)?;
    if p != default {
        vm.heap.get_mut(r.as_object().unwrap()).proto = Some(p);
    }
    Ok(r)
}

fn create_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let d = match node_of(vm, &ctx.this) {
        Some(n) if kind(n) == NK::Document => n,
        _ => return vm.throw_type("Illegal invocation"),
    };
    Ok(make_range(vm, Bp { node: d, off: 0 }, Bp { node: d, off: 0 }))
}

/// "set the start or end" (§5.5).
fn set_bp(vm: &mut Vm, id: usize, node: usize, off: u32, start: bool) -> JsResult<()> {
    if kind(node) == NK::Doctype {
        return throw_dom(vm, "InvalidNodeTypeError", "The node provided is of type 'DocumentType'.");
    }
    if off > node_len(node) {
        return throw_dom(vm, "IndexSizeError", &format!("There is no child at offset {off}."));
    }
    let bp = Bp { node, off };
    let (s, e) = get(id);
    let root = dom::root_of(node);
    if start {
        if dom::root_of(e.node) != root || cmp_bp(bp, e) == Ordering::Greater {
            set_end_raw(id, bp);
        }
        set_start_raw(id, bp);
    } else {
        if dom::root_of(s.node) != root || cmp_bp(bp, s) == Ordering::Less {
            set_start_raw(id, bp);
        }
        set_end_raw(id, bp);
    }
    Ok(())
}

fn set_start(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 2, "Range", "setStart")?;
    let n = node_param(vm, ctx, 0, "setStart")?;
    let o = unsigned_long(vm, &arg(vm, ctx, 1))?;
    set_bp(vm, id, n, o, true)?;
    Ok(Value::Undefined)
}

fn set_end(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 2, "Range", "setEnd")?;
    let n = node_param(vm, ctx, 0, "setEnd")?;
    let o = unsigned_long(vm, &arg(vm, ctx, 1))?;
    set_bp(vm, id, n, o, false)?;
    Ok(Value::Undefined)
}

fn parent_or_throw(vm: &mut Vm, n: usize) -> JsResult<usize> {
    match parent_of(n) {
        Some(p) => Ok(p),
        None => throw_dom(vm, "InvalidNodeTypeError", "the given Node has no parent."),
    }
}

fn set_start_before(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 1, "Range", "setStartBefore")?;
    let n = node_param(vm, ctx, 0, "setStartBefore")?;
    let p = parent_or_throw(vm, n)?;
    set_bp(vm, id, p, index_of(n), true)?;
    Ok(Value::Undefined)
}

fn set_start_after(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 1, "Range", "setStartAfter")?;
    let n = node_param(vm, ctx, 0, "setStartAfter")?;
    let p = parent_or_throw(vm, n)?;
    set_bp(vm, id, p, index_of(n) + 1, true)?;
    Ok(Value::Undefined)
}

fn set_end_before(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 1, "Range", "setEndBefore")?;
    let n = node_param(vm, ctx, 0, "setEndBefore")?;
    let p = parent_or_throw(vm, n)?;
    set_bp(vm, id, p, index_of(n), false)?;
    Ok(Value::Undefined)
}

fn set_end_after(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 1, "Range", "setEndAfter")?;
    let n = node_param(vm, ctx, 0, "setEndAfter")?;
    let p = parent_or_throw(vm, n)?;
    set_bp(vm, id, p, index_of(n) + 1, false)?;
    Ok(Value::Undefined)
}

fn collapse(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    let to_start = boolean(vm, &arg(vm, ctx, 0));
    let (s, e) = get(id);
    if to_start {
        set_end_raw(id, s);
    } else {
        set_start_raw(id, e);
    }
    Ok(Value::Undefined)
}

fn select_node_impl(vm: &mut Vm, id: usize, n: usize) -> JsResult<()> {
    let p = parent_or_throw(vm, n)?;
    let i = index_of(n);
    set_start_raw(id, Bp { node: p, off: i });
    set_end_raw(id, Bp { node: p, off: i + 1 });
    Ok(())
}

fn select_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 1, "Range", "selectNode")?;
    let n = node_param(vm, ctx, 0, "selectNode")?;
    select_node_impl(vm, id, n)?;
    Ok(Value::Undefined)
}

fn select_node_contents(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 1, "Range", "selectNodeContents")?;
    let n = node_param(vm, ctx, 0, "selectNodeContents")?;
    if kind(n) == NK::Doctype {
        return throw_dom(vm, "InvalidNodeTypeError", "The node provided is of type 'DocumentType'.");
    }
    let l = node_len(n);
    set_start_raw(id, Bp { node: n, off: 0 });
    set_end_raw(id, Bp { node: n, off: l });
    Ok(Value::Undefined)
}

fn common_ancestor(s: Bp, e: Bp) -> usize {
    let mut c = s.node;
    while !inclusive_ancestor(c, e.node) {
        match parent_of(c) {
            Some(p) => c = p,
            None => break,
        }
    }
    c
}

fn common_ancestor_container(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    let (s, e) = get(id);
    let c = common_ancestor(s, e);
    Ok(wrap(vm, c))
}

fn compare_boundary_points(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 2, "Range", "compareBoundaryPoints")?;
    let how = vm.to_number(&arg(vm, ctx, 0))?;
    let how = if how.is_finite() { (how.trunc().rem_euclid(65536.0)) as u32 } else { 0 };
    let src = arg(vm, ctx, 1);
    let other = range_arg(vm, &src, "compareBoundaryPoints")?;
    if how > 3 {
        return throw_dom(vm, "NotSupportedError", "The comparison method provided must be one of 'START_TO_START', 'START_TO_END', 'END_TO_END', or 'END_TO_START'.");
    }
    let (s, e) = get(id);
    let (os, oe) = get(other);
    if dom::root_of(s.node) != dom::root_of(os.node) {
        return throw_dom(vm, "WrongDocumentError", "The source range is in a different document than this range.");
    }
    let (a, b) = match how {
        0 => (s, os),
        1 => (e, os),
        2 => (e, oe),
        _ => (s, oe),
    };
    Ok(num(match cmp_bp(a, b) {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    }))
}

fn point_checks(vm: &mut Vm, n: usize, off: u32) -> JsResult<()> {
    if kind(n) == NK::Doctype {
        return throw_dom(vm, "InvalidNodeTypeError", "The node provided is of type 'DocumentType'.");
    }
    if off > node_len(n) {
        return throw_dom(vm, "IndexSizeError", &format!("The offset {off} is larger than the node's length."));
    }
    Ok(())
}

fn is_point_in_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 2, "Range", "isPointInRange")?;
    let n = node_param(vm, ctx, 0, "isPointInRange")?;
    let off = unsigned_long(vm, &arg(vm, ctx, 1))?;
    let (s, e) = get(id);
    if dom::root_of(n) != dom::root_of(s.node) {
        return Ok(Value::Bool(false));
    }
    point_checks(vm, n, off)?;
    let bp = Bp { node: n, off };
    Ok(Value::Bool(cmp_bp(bp, s) != Ordering::Less && cmp_bp(bp, e) != Ordering::Greater))
}

fn compare_point(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 2, "Range", "comparePoint")?;
    let n = node_param(vm, ctx, 0, "comparePoint")?;
    let off = unsigned_long(vm, &arg(vm, ctx, 1))?;
    let (s, e) = get(id);
    if dom::root_of(n) != dom::root_of(s.node) {
        return throw_dom(vm, "WrongDocumentError", "The node provided and this Range are not in the same tree.");
    }
    point_checks(vm, n, off)?;
    let bp = Bp { node: n, off };
    Ok(num(if cmp_bp(bp, s) == Ordering::Less {
        -1.0
    } else if cmp_bp(bp, e) == Ordering::Greater {
        1.0
    } else {
        0.0
    }))
}

fn intersects_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 1, "Range", "intersectsNode")?;
    let n = node_param(vm, ctx, 0, "intersectsNode")?;
    let (s, e) = get(id);
    if dom::root_of(n) != dom::root_of(s.node) {
        return Ok(Value::Bool(false));
    }
    let Some(p) = parent_of(n) else { return Ok(Value::Bool(true)) };
    let i = index_of(n);
    Ok(Value::Bool(cmp_bp(Bp { node: p, off: i }, e) == Ordering::Less && cmp_bp(Bp { node: p, off: i + 1 }, s) == Ordering::Greater))
}

/// "contained" (§5.5): `n` lies wholly inside the range.
fn contained(n: usize, s: Bp, e: Bp) -> bool {
    dom::root_of(n) == dom::root_of(s.node)
        && cmp_bp(Bp { node: n, off: 0 }, s) == Ordering::Greater
        && cmp_bp(Bp { node: n, off: node_len(n) }, e) == Ordering::Less
}

/// "partially contained": an inclusive ancestor of exactly one of the boundary nodes.
fn partially_contained(n: usize, s: Bp, e: Bp) -> bool {
    inclusive_ancestor(n, s.node) != inclusive_ancestor(n, e.node)
}

fn stringify(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    let (s, e) = get(id);
    let mut out = String::new();
    if s.node == e.node && kind(s.node) == NK::Text {
        out = substring(s.node, s.off, e.off.saturating_sub(s.off));
        return Ok(Value::String(JsStr::from_str(&out)));
    }
    if kind(s.node) == NK::Text {
        out.push_str(&substring(s.node, s.off, u32::MAX));
    }
    let ca = common_ancestor(s, e);
    let texts: Vec<usize> = dom::with_doc(|d| d.descendants(NodeId(ca)).filter(|c| matches!(d.data(*c), NodeData::Text(_))).map(|c| c.0).collect());
    for t in texts {
        if contained(t, s, e) {
            out.push_str(&dom::with_doc(|d| dom::char_data(d, t)));
        }
    }
    if kind(e.node) == NK::Text {
        out.push_str(&substring(e.node, 0, e.off));
    }
    Ok(Value::String(JsStr::from_str(&out)))
}

fn new_fragment(doc: usize) -> usize {
    let f = dom::with_doc_mut(|d| d.create(NodeData::DocumentFragment).0);
    dom::set_node_doc(f, doc);
    f
}

fn clone_with_data(vm: &mut Vm, n: usize, data: &str) -> JsResult<usize> {
    let c = dom::clone_node(n, false, None);
    dom::replace_data(vm, c, 0, u32::MAX, data)?;
    Ok(c)
}

/// The insertion point after removing the range's contents (deleteContents/extractContents steps 5–6).
fn collapse_point(s: Bp, e: Bp) -> Bp {
    if inclusive_ancestor(s.node, e.node) {
        return s;
    }
    let mut reference = s.node;
    while let Some(p) = parent_of(reference) {
        if inclusive_ancestor(p, e.node) {
            break;
        }
        reference = p;
    }
    match parent_of(reference) {
        Some(p) => Bp { node: p, off: index_of(reference) + 1 },
        None => s,
    }
}

fn delete_contents_impl(vm: &mut Vm, id: usize) -> JsResult<()> {
    let (s, e) = get(id);
    if s == e {
        return Ok(());
    }
    if s.node == e.node && is_char_data(s.node) {
        dom::replace_data(vm, s.node, s.off, e.off - s.off, "")?;
        return Ok(());
    }
    let ca = common_ancestor(s, e);
    let all: Vec<usize> = dom::with_doc(|d| d.descendants(NodeId(ca)).map(|c| c.0).collect());
    let mut to_remove = Vec::new();
    for n in all {
        if contained(n, s, e) && !parent_of(n).is_some_and(|p| contained(p, s, e)) {
            to_remove.push(n);
        }
    }
    let np = collapse_point(s, e);
    if is_char_data(s.node) {
        let l = node_len(s.node);
        dom::replace_data(vm, s.node, s.off, l - s.off, "")?;
    }
    for n in to_remove {
        dom::remove(vm, n);
    }
    if is_char_data(e.node) {
        dom::replace_data(vm, e.node, 0, e.off, "")?;
    }
    set_start_raw(id, np);
    set_end_raw(id, np);
    Ok(())
}

fn delete_contents(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    delete_contents_impl(vm, id)?;
    Ok(Value::Undefined)
}

/// extractContents (`clone` false) and cloneContents (`clone` true), §5.5.
fn extract_or_clone(vm: &mut Vm, id: usize, clone: bool) -> JsResult<usize> {
    let (s, e) = get(id);
    let frag = new_fragment(dom::node_document(s.node));
    if s == e {
        return Ok(frag);
    }
    if s.node == e.node && is_char_data(s.node) {
        let c = clone_with_data(vm, s.node, &substring(s.node, s.off, e.off - s.off))?;
        dom::insert(vm, c, frag, None);
        if !clone {
            dom::replace_data(vm, s.node, s.off, e.off - s.off, "")?;
        }
        return Ok(frag);
    }
    let ca = common_ancestor(s, e);
    let kids = dom::children_of(ca);
    let first_pc = if inclusive_ancestor(s.node, e.node) { None } else { kids.iter().copied().find(|&c| partially_contained(c, s, e)) };
    let last_pc = if inclusive_ancestor(e.node, s.node) { None } else { kids.iter().rev().copied().find(|&c| partially_contained(c, s, e)) };
    let contained_kids: Vec<usize> = kids.iter().copied().filter(|&c| contained(c, s, e)).collect();
    if contained_kids.iter().any(|&c| kind(c) == NK::Doctype) {
        return throw_dom(vm, "HierarchyRequestError", "The Range contains a doctype node.");
    }
    let np = if clone { s } else { collapse_point(s, e) };
    if let Some(f) = first_pc {
        if is_char_data(f) {
            let l = node_len(s.node);
            let c = clone_with_data(vm, s.node, &substring(s.node, s.off, l - s.off))?;
            dom::insert(vm, c, frag, None);
            if !clone {
                dom::replace_data(vm, s.node, s.off, l - s.off, "")?;
            }
        } else {
            let c = dom::clone_node(f, false, None);
            dom::insert(vm, c, frag, None);
            let sub = new_live(s, Bp { node: f, off: node_len(f) });
            let r = extract_or_clone(vm, sub, clone);
            retire(sub);
            let sf = r?;
            dom::insert(vm, sf, c, None);
        }
    }
    for k in contained_kids {
        if clone {
            let c = dom::clone_node(k, true, None);
            dom::insert(vm, c, frag, None);
        } else {
            dom::insert(vm, k, frag, None);
        }
    }
    if let Some(l) = last_pc {
        if is_char_data(l) {
            let c = clone_with_data(vm, e.node, &substring(e.node, 0, e.off))?;
            dom::insert(vm, c, frag, None);
            if !clone {
                dom::replace_data(vm, e.node, 0, e.off, "")?;
            }
        } else {
            let c = dom::clone_node(l, false, None);
            dom::insert(vm, c, frag, None);
            let sub = new_live(Bp { node: l, off: 0 }, e);
            let r = extract_or_clone(vm, sub, clone);
            retire(sub);
            let sf = r?;
            dom::insert(vm, sf, c, None);
        }
    }
    if !clone {
        set_start_raw(id, np);
        set_end_raw(id, np);
    }
    Ok(frag)
}

fn extract_contents(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    let f = extract_or_clone(vm, id, false)?;
    Ok(wrap(vm, f))
}

fn clone_contents(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    let f = extract_or_clone(vm, id, true)?;
    Ok(wrap(vm, f))
}

/// "insert" a node into a range (§5.5).
fn insert_node_impl(vm: &mut Vm, id: usize, node: usize) -> JsResult<()> {
    let (s, _) = get(id);
    let sk = kind(s.node);
    if matches!(sk, NK::Pi | NK::Comment) || (sk == NK::Text && parent_of(s.node).is_none()) || s.node == node {
        return throw_dom(vm, "HierarchyRequestError", "The range's start container cannot take the node.");
    }
    let mut reference = if sk == NK::Text { Some(s.node) } else { child_at(s.node, s.off) };
    let parent = match reference {
        None => s.node,
        Some(r) => parent_of(r).unwrap_or(s.node),
    };
    dom::ensure_pre_insert_validity(vm, node, parent, reference)?;
    if sk == NK::Text {
        reference = Some(dom::split_text_at(vm, s.node, s.off)?);
    }
    if reference == Some(node) {
        reference = dom::with_doc(|d| d.next_sibling(NodeId(node)).map(|c| c.0));
    }
    if parent_of(node).is_some() {
        dom::remove(vm, node);
    }
    let mut new_off = match reference {
        None => node_len(parent),
        Some(r) => index_of(r),
    };
    new_off += if kind(node) == NK::Fragment { node_len(node) } else { 1 };
    dom::pre_insert(vm, node, parent, reference)?;
    let (s2, e2) = get(id);
    if s2 == e2 {
        set_end_raw(id, Bp { node: parent, off: new_off });
    }
    Ok(())
}

fn insert_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 1, "Range", "insertNode")?;
    let n = node_param(vm, ctx, 0, "insertNode")?;
    insert_node_impl(vm, id, n)?;
    Ok(Value::Undefined)
}

fn surround_contents(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 1, "Range", "surroundContents")?;
    let np = node_param(vm, ctx, 0, "surroundContents")?;
    let (s, e) = get(id);
    let ca = common_ancestor(s, e);
    let bad = dom::with_doc(|d| d.descendants(NodeId(ca)).map(|c| c.0).collect::<Vec<_>>())
        .into_iter()
        .any(|n| kind(n) != NK::Text && partially_contained(n, s, e));
    if bad {
        return throw_dom(vm, "InvalidStateError", "The Range has partially selected a non-Text node.");
    }
    if matches!(kind(np), NK::Document | NK::Doctype | NK::Fragment) {
        return throw_dom(vm, "InvalidNodeTypeError", "The node provided cannot be a range's new parent.");
    }
    let frag = extract_or_clone(vm, id, false)?;
    if !dom::children_of(np).is_empty() {
        dom::replace_all(vm, None, np);
    }
    insert_node_impl(vm, id, np)?;
    dom::pre_insert(vm, frag, np, None)?;
    select_node_impl(vm, id, np)?;
    Ok(Value::Undefined)
}

fn clone_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    let (s, e) = get(id);
    Ok(make_range(vm, s, e))
}

fn detach(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_range(vm, ctx)?;
    Ok(Value::Undefined)
}

fn create_contextual_fragment(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let id = this_range(vm, ctx)?;
    need(vm, ctx, 1, "Range", "createContextualFragment")?;
    let markup = string(vm, &arg(vm, ctx, 0))?;
    let (s, _) = get(id);
    let el = match kind(s.node) {
        NK::Element => Some(s.node),
        NK::Text | NK::Comment | NK::Pi => parent_of(s.node).filter(|p| kind(*p) == NK::Element),
        _ => None,
    };
    let el = match el {
        Some(e) if !dom::with_doc(|d| dom::is_html_tag(d, e, "html")) => e,
        _ => dom::create_element(dom::node_document(s.node), html_core::Namespace::Html, "body"),
    };
    let f = dom::parse_fragment(el, &markup);
    Ok(wrap(vm, f))
}

pub fn install(vm: &mut Vm) {
    let ar = interface(vm, "AbstractRange", None, None);
    dom::register_iface("AbstractRange", ar);
    attr(vm, ar.proto, "startContainer", ar_start_container, None);
    attr(vm, ar.proto, "startOffset", ar_start_offset, None);
    attr(vm, ar.proto, "endContainer", ar_end_container, None);
    attr(vm, ar.proto, "endOffset", ar_end_offset, None);
    attr(vm, ar.proto, "collapsed", ar_collapsed, None);

    let sr = interface(vm, "StaticRange", Some(ar), Some((static_range_ctor, 1)));
    dom::register_iface("StaticRange", sr);

    let r = interface(vm, "Range", Some(ar), Some((range_ctor, 0)));
    dom::register_iface("Range", r);
    for (n, v) in [("START_TO_START", 0.0), ("START_TO_END", 1.0), ("END_TO_END", 2.0), ("END_TO_START", 3.0)] {
        konst(vm, r, n, v);
    }
    let p = r.proto;
    attr(vm, p, "commonAncestorContainer", common_ancestor_container, None);
    op(vm, p, "setStart", 2, set_start);
    op(vm, p, "setEnd", 2, set_end);
    op(vm, p, "setStartBefore", 1, set_start_before);
    op(vm, p, "setStartAfter", 1, set_start_after);
    op(vm, p, "setEndBefore", 1, set_end_before);
    op(vm, p, "setEndAfter", 1, set_end_after);
    op(vm, p, "collapse", 0, collapse);
    op(vm, p, "selectNode", 1, select_node);
    op(vm, p, "selectNodeContents", 1, select_node_contents);
    op(vm, p, "compareBoundaryPoints", 2, compare_boundary_points);
    op(vm, p, "deleteContents", 0, delete_contents);
    op(vm, p, "extractContents", 0, extract_contents);
    op(vm, p, "cloneContents", 0, clone_contents);
    op(vm, p, "insertNode", 1, insert_node);
    op(vm, p, "surroundContents", 1, surround_contents);
    op(vm, p, "cloneRange", 0, clone_range);
    op(vm, p, "detach", 0, detach);
    op(vm, p, "isPointInRange", 2, is_point_in_range);
    op(vm, p, "comparePoint", 2, compare_point);
    op(vm, p, "intersectsNode", 1, intersects_node);
    op(vm, p, "toString", 0, stringify);
    op(vm, p, "createContextualFragment", 1, create_contextual_fragment);

    let dp = dom::proto("Document");
    op(vm, dp, "createRange", 0, create_range);
}
