//! DOM §6 traversal (AETHERWPT, SR72): the NodeFilter legacy callback interface object, NodeIterator
//! (with the §6.1 pre-removing steps the remove algorithm runs) and TreeWalker, both over the shared
//! "filter" algorithm (whatToShow mask, then the user filter under the active flag).

use super::dom::{self, kind, node_of, wrap, NK};
use super::idl::*;
use html_core::NodeId;
use js_core::vm::*;
use std::cell::RefCell;

pub const T_NODE_ITERATOR: u8 = 42;
pub const T_TREE_WALKER: u8 = 43;

const ACCEPT: u32 = 1;
const REJECT: u32 = 2;
const SKIP: u32 = 3;

#[derive(Clone, Copy)]
struct Iter {
    root: usize,
    reference: usize,
    before: bool,
    active: bool,
}

thread_local! {
    static ITERS: RefCell<Vec<Iter>> = const { RefCell::new(Vec::new()) };
}

pub fn reset() {
    ITERS.with(|i| i.borrow_mut().clear());
}

fn parent(n: usize) -> Option<usize> {
    dom::parent_of(n)
}
fn first_child(n: usize) -> Option<usize> {
    dom::with_doc(|d| d.first_child(NodeId(n)).map(|c| c.0))
}
fn last_child(n: usize) -> Option<usize> {
    dom::with_doc(|d| d.last_child(NodeId(n)).map(|c| c.0))
}
fn next_sibling(n: usize) -> Option<usize> {
    dom::with_doc(|d| d.next_sibling(NodeId(n)).map(|c| c.0))
}
fn prev_sibling(n: usize) -> Option<usize> {
    dom::with_doc(|d| d.prev_sibling(NodeId(n)).map(|c| c.0))
}

fn node_type(n: usize) -> u32 {
    match kind(n) {
        NK::Element => 1,
        NK::Text => 3,
        NK::Pi => 7,
        NK::Comment => 8,
        NK::Document => 9,
        NK::Doctype => 10,
        NK::Fragment => 11,
    }
}

/// The node after `n` in tree order, staying within `root` (None past the end).
fn following(n: usize, root: usize) -> Option<usize> {
    if let Some(c) = first_child(n) {
        return Some(c);
    }
    let mut cur = n;
    loop {
        if cur == root {
            return None;
        }
        if let Some(s) = next_sibling(cur) {
            return Some(s);
        }
        cur = parent(cur)?;
    }
}

/// The node after `n`'s subtree in tree order within `root` (skipping `n`'s descendants).
fn following_not_descendant(n: usize, root: usize) -> Option<usize> {
    let mut cur = n;
    loop {
        if cur == root {
            return None;
        }
        if let Some(s) = next_sibling(cur) {
            return Some(s);
        }
        cur = parent(cur)?;
    }
}

fn last_inclusive_descendant(n: usize) -> usize {
    let mut cur = n;
    while let Some(c) = last_child(cur) {
        cur = c;
    }
    cur
}

/// The node before `n` in tree order within `root`.
fn preceding(n: usize, root: usize) -> Option<usize> {
    if n == root {
        return None;
    }
    match prev_sibling(n) {
        Some(s) => Some(last_inclusive_descendant(s)),
        None => parent(n),
    }
}

/// Whose active flag a filter run guards.
#[derive(Clone, Copy)]
enum Owner {
    Iter(usize),
    Walker(Obj),
}

fn active(vm: &Vm, o: Owner) -> bool {
    match o {
        Owner::Iter(id) => ITERS.with(|i| i.borrow()[id].active),
        Owner::Walker(w) => slot_num(vm, w, 4) == 1.0,
    }
}

fn set_active(vm: &mut Vm, o: Owner, v: bool) {
    match o {
        Owner::Iter(id) => ITERS.with(|i| i.borrow_mut()[id].active = v),
        Owner::Walker(w) => set_slot(vm, w, 4, num(if v { 1.0 } else { 0.0 })),
    }
}

/// The §6 "filter" algorithm.
fn filter(vm: &mut Vm, n: usize, what: u32, f: &Value, owner: Owner) -> JsResult<u32> {
    if active(vm, owner) {
        return throw_dom(vm, "InvalidStateError", "The filter is already running.");
    }
    let t = node_type(n);
    if (1u32 << (t - 1)) & what == 0 {
        return Ok(SKIP);
    }
    if f.is_nullish() {
        return Ok(ACCEPT);
    }
    set_active(vm, owner, true);
    let node = wrap(vm, n);
    let r = (|| -> JsResult<Value> {
        if vm.is_callable(f) {
            vm.call(f, &Value::Undefined, &[node])
        } else {
            let o = match f {
                Value::Object(o) => *o,
                _ => return vm.throw_type("NodeFilter is not an object"),
            };
            let m = get(vm, o, "acceptNode")?;
            if !vm.is_callable(&m) {
                return vm.throw_type("NodeFilter.acceptNode is not a function");
            }
            vm.call(&m, f, &[node])
        }
    })();
    set_active(vm, owner, false);
    let v = r?;
    let n = vm.to_number(&v)?;
    Ok(if n.is_finite() { n.trunc().rem_euclid(65536.0) as u32 } else { 0 })
}

// ------------------------------------------------------------------------------------------------ NodeIterator

/// §6.1 NodeIterator pre-removing steps, run for every iterator before `node` is removed.
pub fn removing(node: usize) {
    let any = ITERS.with(|i| !i.borrow().is_empty());
    if !any {
        return;
    }
    let iters: Vec<(usize, Iter)> = ITERS.with(|i| i.borrow().iter().copied().enumerate().collect());
    for (id, it) in iters {
        if !super::range::inclusive_ancestor(node, it.reference) || node == it.root {
            continue;
        }
        // Only iterators whose root is in node's tree can see node.
        if !super::range::inclusive_ancestor(it.root, node) {
            continue;
        }
        let mut new = it;
        if it.before {
            if let Some(next) = following_not_descendant(node, it.root) {
                new.reference = next;
                ITERS.with(|i| i.borrow_mut()[id] = new);
                continue;
            }
            new.before = false;
        }
        new.reference = match prev_sibling(node) {
            None => parent(node).unwrap_or(it.root),
            Some(s) => last_inclusive_descendant(s),
        };
        ITERS.with(|i| i.borrow_mut()[id] = new);
    }
}

fn this_iter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Obj> {
    match this_tagged(vm, &ctx.this, T_NODE_ITERATOR) {
        Some(o) => Ok(o),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn iter_id(vm: &Vm, o: Obj) -> usize {
    slot_num(vm, o, 0) as usize
}

fn iter_state(id: usize) -> Iter {
    ITERS.with(|i| i.borrow()[id])
}

fn ni_root(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_iter(vm, ctx)?;
    let r = iter_state(iter_id(vm, o)).root;
    Ok(wrap(vm, r))
}
fn ni_reference(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_iter(vm, ctx)?;
    let r = iter_state(iter_id(vm, o)).reference;
    Ok(wrap(vm, r))
}
fn ni_before(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_iter(vm, ctx)?;
    Ok(Value::Bool(iter_state(iter_id(vm, o)).before))
}
fn ni_what(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_iter(vm, ctx)?;
    Ok(slot(vm, o, 1))
}
fn ni_filter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_iter(vm, ctx)?;
    Ok(slot(vm, o, 2))
}

fn ni_traverse(vm: &mut Vm, ctx: &CallCtx, next: bool) -> JsResult<Value> {
    let o = this_iter(vm, ctx)?;
    let id = iter_id(vm, o);
    let what = slot_num(vm, o, 1) as u32;
    let f = slot(vm, o, 2);
    let st = iter_state(id);
    let mut node = st.reference;
    let mut before = st.before;
    loop {
        if next {
            if !before {
                match following(node, st.root) {
                    Some(n) => node = n,
                    None => return Ok(Value::Null),
                }
            } else {
                before = false;
            }
        } else if before {
            match preceding(node, st.root) {
                Some(n) => node = n,
                None => return Ok(Value::Null),
            }
        } else {
            before = true;
        }
        let r = filter(vm, node, what, &f, Owner::Iter(id))?;
        if r == ACCEPT {
            break;
        }
    }
    ITERS.with(|i| {
        let mut i = i.borrow_mut();
        i[id].reference = node;
        i[id].before = before;
    });
    Ok(wrap(vm, node))
}

fn ni_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ni_traverse(vm, ctx, true)
}
fn ni_previous(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ni_traverse(vm, ctx, false)
}
fn ni_detach(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_iter(vm, ctx)?;
    Ok(Value::Undefined)
}

fn traversal_args(vm: &mut Vm, ctx: &CallCtx, method: &str) -> JsResult<(usize, f64, Value)> {
    need(vm, ctx, 1, "Document", method)?;
    let v = arg(vm, ctx, 0);
    let Some(root) = node_of(vm, &v) else {
        return vm.throw_type(&format!("Failed to execute '{method}' on 'Document': parameter 1 is not of type 'Node'."));
    };
    let w = arg(vm, ctx, 1);
    let what = if w.is_undefined() { 0xFFFF_FFFFu32 } else { unsigned_long(vm, &w)? };
    let f = arg(vm, ctx, 2);
    let f = if f.is_undefined() { Value::Null } else { f };
    if !f.is_null() && !matches!(f, Value::Object(_)) {
        return vm.throw_type(&format!("Failed to execute '{method}' on 'Document': The provided value is not of type 'NodeFilter'."));
    }
    Ok((root, what as f64, f))
}

fn create_node_iterator(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if !node_of(vm, &ctx.this).is_some_and(|n| kind(n) == NK::Document) {
        return vm.throw_type("Illegal invocation");
    }
    let (root, what, f) = traversal_args(vm, ctx, "createNodeIterator")?;
    let id = ITERS.with(|i| {
        let mut i = i.borrow_mut();
        i.push(Iter { root, reference: root, before: true, active: false });
        i.len() - 1
    });
    let p = dom::proto("NodeIterator");
    Ok(Value::Object(host_obj(vm, p, T_NODE_ITERATOR, vec![num(id as f64), num(what), f])))
}

// ------------------------------------------------------------------------------------------------ TreeWalker

// TreeWalker slots: 0 root, 1 whatToShow, 2 filter, 3 currentNode, 4 active (0/1).

fn this_walker(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Obj> {
    match this_tagged(vm, &ctx.this, T_TREE_WALKER) {
        Some(o) => Ok(o),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn tw_root(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_walker(vm, ctx)?;
    let r = slot_num(vm, o, 0) as usize;
    Ok(wrap(vm, r))
}
fn tw_what(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_walker(vm, ctx)?;
    Ok(slot(vm, o, 1))
}
fn tw_filter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_walker(vm, ctx)?;
    Ok(slot(vm, o, 2))
}
fn tw_current_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_walker(vm, ctx)?;
    let c = slot_num(vm, o, 3) as usize;
    Ok(wrap(vm, c))
}
fn tw_current_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_walker(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    let Some(n) = node_of(vm, &v) else {
        return vm.throw_type("Failed to set the 'currentNode' property on 'TreeWalker': The provided value is not of type 'Node'.");
    };
    set_slot(vm, o, 3, num(n as f64));
    Ok(Value::Undefined)
}

struct Walker {
    o: Obj,
    root: usize,
    what: u32,
    f: Value,
}

impl Walker {
    fn of(vm: &Vm, o: Obj) -> Walker {
        Walker { o, root: slot_num(vm, o, 0) as usize, what: slot_num(vm, o, 1) as u32, f: slot(vm, o, 2) }
    }
    fn current(&self, vm: &Vm) -> usize {
        slot_num(vm, self.o, 3) as usize
    }
    fn set_current(&self, vm: &mut Vm, n: usize) {
        set_slot(vm, self.o, 3, num(n as f64));
    }
    fn filter(&self, vm: &mut Vm, n: usize) -> JsResult<u32> {
        let f = self.f.clone();
        filter(vm, n, self.what, &f, Owner::Walker(self.o))
    }
}

fn tw_parent(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_walker(vm, ctx)?;
    let w = Walker::of(vm, o);
    let mut node = Some(w.current(vm));
    while let Some(n) = node {
        if n == w.root {
            break;
        }
        node = parent(n);
        if let Some(p) = node {
            if w.filter(vm, p)? == ACCEPT {
                w.set_current(vm, p);
                return Ok(wrap(vm, p));
            }
        }
    }
    Ok(Value::Null)
}

fn tw_children(vm: &mut Vm, ctx: &CallCtx, first: bool) -> JsResult<Value> {
    let o = this_walker(vm, ctx)?;
    let w = Walker::of(vm, o);
    let current = w.current(vm);
    let mut node = if first { first_child(current) } else { last_child(current) };
    while let Some(n) = node {
        let r = w.filter(vm, n)?;
        if r == ACCEPT {
            w.set_current(vm, n);
            return Ok(wrap(vm, n));
        }
        if r == SKIP {
            let child = if first { first_child(n) } else { last_child(n) };
            if child.is_some() {
                node = child;
                continue;
            }
        }
        let mut cur = Some(n);
        node = None;
        while let Some(c) = cur {
            let sib = if first { next_sibling(c) } else { prev_sibling(c) };
            if sib.is_some() {
                node = sib;
                break;
            }
            let p = parent(c);
            if p.is_none() || p == Some(w.root) || p == Some(current) {
                return Ok(Value::Null);
            }
            cur = p;
        }
    }
    Ok(Value::Null)
}

fn tw_siblings(vm: &mut Vm, ctx: &CallCtx, next: bool) -> JsResult<Value> {
    let o = this_walker(vm, ctx)?;
    let w = Walker::of(vm, o);
    let mut node = w.current(vm);
    if node == w.root {
        return Ok(Value::Null);
    }
    loop {
        let mut sibling = if next { next_sibling(node) } else { prev_sibling(node) };
        while let Some(s) = sibling {
            node = s;
            let r = w.filter(vm, node)?;
            if r == ACCEPT {
                w.set_current(vm, node);
                return Ok(wrap(vm, node));
            }
            sibling = if next { first_child(node) } else { last_child(node) };
            if r == REJECT || sibling.is_none() {
                sibling = if next { next_sibling(node) } else { prev_sibling(node) };
            }
        }
        match parent(node) {
            None => return Ok(Value::Null),
            Some(p) if p == w.root => return Ok(Value::Null),
            Some(p) => node = p,
        }
        if w.filter(vm, node)? == ACCEPT {
            return Ok(Value::Null);
        }
    }
}

fn tw_previous_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_walker(vm, ctx)?;
    let w = Walker::of(vm, o);
    let mut node = w.current(vm);
    while node != w.root {
        let mut sibling = prev_sibling(node);
        while let Some(s) = sibling {
            node = s;
            let mut r = w.filter(vm, node)?;
            while r != REJECT {
                match last_child(node) {
                    Some(c) => {
                        node = c;
                        r = w.filter(vm, node)?;
                    }
                    None => break,
                }
            }
            if r == ACCEPT {
                w.set_current(vm, node);
                return Ok(wrap(vm, node));
            }
            sibling = prev_sibling(node);
        }
        let Some(p) = parent(node) else { return Ok(Value::Null) };
        if node == w.root {
            return Ok(Value::Null);
        }
        node = p;
        if w.filter(vm, node)? == ACCEPT {
            w.set_current(vm, node);
            return Ok(wrap(vm, node));
        }
    }
    Ok(Value::Null)
}

fn tw_next_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_walker(vm, ctx)?;
    let w = Walker::of(vm, o);
    let mut node = w.current(vm);
    let mut r = ACCEPT;
    loop {
        while r != REJECT {
            let Some(c) = first_child(node) else { break };
            node = c;
            r = w.filter(vm, node)?;
            if r == ACCEPT {
                w.set_current(vm, node);
                return Ok(wrap(vm, node));
            }
        }
        let mut temporary = Some(node);
        let mut found = false;
        while let Some(t) = temporary {
            if t == w.root {
                return Ok(Value::Null);
            }
            if let Some(s) = next_sibling(t) {
                node = s;
                found = true;
                break;
            }
            temporary = parent(t);
        }
        if !found {
            return Ok(Value::Null);
        }
        r = w.filter(vm, node)?;
        if r == ACCEPT {
            w.set_current(vm, node);
            return Ok(wrap(vm, node));
        }
    }
}

fn tw_first_child(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    tw_children(vm, ctx, true)
}
fn tw_last_child(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    tw_children(vm, ctx, false)
}
fn tw_next_sibling(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    tw_siblings(vm, ctx, true)
}
fn tw_previous_sibling(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    tw_siblings(vm, ctx, false)
}

fn create_tree_walker(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if !node_of(vm, &ctx.this).is_some_and(|n| kind(n) == NK::Document) {
        return vm.throw_type("Illegal invocation");
    }
    let (root, what, f) = traversal_args(vm, ctx, "createTreeWalker")?;
    let p = dom::proto("TreeWalker");
    Ok(Value::Object(host_obj(vm, p, T_TREE_WALKER, vec![num(root as f64), num(what), f, num(root as f64), num(0.0)])))
}

fn node_filter_call(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    vm.throw_type("Illegal constructor")
}

pub fn install(vm: &mut Vm) {
    // NodeFilter: a legacy callback interface object (WebIDL §3.7.2) carrying the constants.
    let nf = vm.make_native("NodeFilter", 0, node_filter_call, false);
    for (n, v) in [
        ("FILTER_ACCEPT", 1.0),
        ("FILTER_REJECT", 2.0),
        ("FILTER_SKIP", 3.0),
        ("SHOW_ALL", 4294967295.0),
        ("SHOW_ELEMENT", 1.0),
        ("SHOW_ATTRIBUTE", 2.0),
        ("SHOW_TEXT", 4.0),
        ("SHOW_CDATA_SECTION", 8.0),
        ("SHOW_ENTITY_REFERENCE", 16.0),
        ("SHOW_ENTITY", 32.0),
        ("SHOW_PROCESSING_INSTRUCTION", 64.0),
        ("SHOW_COMMENT", 128.0),
        ("SHOW_DOCUMENT", 256.0),
        ("SHOW_DOCUMENT_TYPE", 512.0),
        ("SHOW_DOCUMENT_FRAGMENT", 1024.0),
        ("SHOW_NOTATION", 2048.0),
    ] {
        vm.heap.get_mut(nf).props.insert(PropertyKey::from_str(n), Prop::data(Value::Number(v), E));
    }
    let g = vm.realm().global;
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("NodeFilter"), Prop::data(Value::Object(nf), WC));
    root(vm, Value::Object(nf));

    let ni = interface(vm, "NodeIterator", None, None);
    dom::register_iface("NodeIterator", ni);
    attr(vm, ni.proto, "root", ni_root, None);
    attr(vm, ni.proto, "referenceNode", ni_reference, None);
    attr(vm, ni.proto, "pointerBeforeReferenceNode", ni_before, None);
    attr(vm, ni.proto, "whatToShow", ni_what, None);
    attr(vm, ni.proto, "filter", ni_filter, None);
    op(vm, ni.proto, "nextNode", 0, ni_next);
    op(vm, ni.proto, "previousNode", 0, ni_previous);
    op(vm, ni.proto, "detach", 0, ni_detach);

    let tw = interface(vm, "TreeWalker", None, None);
    dom::register_iface("TreeWalker", tw);
    attr(vm, tw.proto, "root", tw_root, None);
    attr(vm, tw.proto, "whatToShow", tw_what, None);
    attr(vm, tw.proto, "filter", tw_filter, None);
    attr(vm, tw.proto, "currentNode", tw_current_get, Some(tw_current_set));
    op(vm, tw.proto, "parentNode", 0, tw_parent);
    op(vm, tw.proto, "firstChild", 0, tw_first_child);
    op(vm, tw.proto, "lastChild", 0, tw_last_child);
    op(vm, tw.proto, "previousSibling", 0, tw_previous_sibling);
    op(vm, tw.proto, "nextSibling", 0, tw_next_sibling);
    op(vm, tw.proto, "previousNode", 0, tw_previous_node);
    op(vm, tw.proto, "nextNode", 0, tw_next_node);

    let dp = dom::proto("Document");
    op(vm, dp, "createNodeIterator", 1, create_node_iterator);
    op(vm, dp, "createTreeWalker", 1, create_tree_walker);
}
