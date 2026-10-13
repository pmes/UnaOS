//! Shadow trees (AETHERSHADOW, LEDGER SR82): DOM §4.2.2 over html_core's arena.
//!
//! A shadow root is a DocumentFragment node created in its host's arena. The arena knows nothing of
//! shadow trees: a shadow root has no parent, so every existing walk (layout, the cascade, the
//! serializer, `children`) keeps seeing the light tree, and nothing inside a shadow tree leaks out.
//! What makes the fragment a shadow root is this module's side table — host → [`ShadowRootInfo`],
//! root → host — keyed by the arena's address and validated by a `Weak` of the arena, so an address
//! recycled by a later document never inherits a dead page's shadow roots.
//!
//! On top of the table: roots and shadow-including roots, shadow-including tree order, the slot
//! algorithms (§4.2.2.3–4: find a slot, find slottables, find flattened slottables, manual
//! assignment), slot-change detection ([`refresh`], which signals through `dom::mutation`), the
//! flat tree style and layout walk ([`flat_children`], [`NodeRef::flat_children`]), and the event
//! path pieces dispatch needs: [`retarget`] (§2.5), [`event_parent`] (§2.9 "get the parent") and
//! [`composed_path`] (§2.9 `composedPath()` with closed-tree hiding).
//!
//! Slot assignment is COMPUTED from the tree on every query (it is a pure function of the tree, the
//! `slot`/`name` attributes and the manual assignments), so a static render with no script sees the
//! same assignment script does. The stored copy per slot exists only to notice a change and fire
//! `slotchange`.

use super::{Arena, NodeRef};
use html_core::{Document, Namespace, NodeData, NodeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Weak;

/// `ShadowRootMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowMode {
    Open,
    Closed,
}

impl ShadowMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ShadowMode::Open => "open",
            ShadowMode::Closed => "closed",
        }
    }
}

/// `SlotAssignmentMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotAssignment {
    Named,
    Manual,
}

impl SlotAssignment {
    pub fn as_str(self) -> &'static str {
        match self {
            SlotAssignment::Named => "named",
            SlotAssignment::Manual => "manual",
        }
    }
}

/// A shadow root's state (DOM §4.8 ShadowRoot).
#[derive(Clone, Copy, Debug)]
pub struct ShadowRootInfo {
    pub root: NodeId,
    pub host: NodeId,
    pub mode: ShadowMode,
    pub delegates_focus: bool,
    pub slot_assignment: SlotAssignment,
    pub clonable: bool,
    pub serializable: bool,
    pub declarative: bool,
}

/// `attachShadow`'s validated init.
#[derive(Clone, Copy, Debug)]
pub struct ShadowInit {
    pub mode: ShadowMode,
    pub delegates_focus: bool,
    pub slot_assignment: SlotAssignment,
    pub clonable: bool,
    pub serializable: bool,
}

#[derive(Default)]
struct DocTable {
    /// host → its shadow root.
    by_host: HashMap<NodeId, ShadowRootInfo>,
    /// shadow root → host.
    host_of: HashMap<NodeId, NodeId>,
    /// slot → manually assigned nodes (`HTMLSlotElement.assign`), in order.
    manual: HashMap<NodeId, Vec<NodeId>>,
    /// slottable → the slot whose manual list holds it ("manual slot assignment").
    manual_slot: HashMap<NodeId, NodeId>,
    /// slot → the assigned nodes last seen (slot-change detection only).
    assigned: HashMap<NodeId, Vec<NodeId>>,
}

struct Entry {
    arena: Weak<RefCell<Document>>,
    table: DocTable,
}

thread_local! {
    static TABLES: RefCell<HashMap<usize, Entry>> = RefCell::new(HashMap::new());
}

/// The table key of a document: its address (stable while its `Rc` lives).
pub fn key(d: &Document) -> usize {
    d as *const Document as usize
}

fn arena_key(a: &Arena) -> usize {
    a.as_ptr() as *const Document as usize
}

/// Runs `f` over `d`'s live table (None when `d` has no shadow roots, or the entry is a dead
/// document's).
fn with_table<R>(d: &Document, f: impl FnOnce(&DocTable) -> R) -> Option<R> {
    TABLES.with(|t| {
        let mut t = t.borrow_mut();
        let k = key(d);
        let dead = match t.get(&k) {
            None => return None,
            Some(e) => e.arena.strong_count() == 0,
        };
        if dead {
            t.remove(&k);
            return None;
        }
        t.get(&k).map(|e| f(&e.table))
    })
}

fn with_table_mut<R>(d: &Document, f: impl FnOnce(&mut DocTable) -> R) -> Option<R> {
    TABLES.with(|t| {
        let mut t = t.borrow_mut();
        let k = key(d);
        match t.get_mut(&k) {
            Some(e) if e.arena.strong_count() > 0 => Some(f(&mut e.table)),
            _ => None,
        }
    })
}

/// Does `d` hold any shadow root? (the fast path every hook takes first)
pub fn any(d: &Document) -> bool {
    with_table(d, |t| !t.by_host.is_empty()).unwrap_or(false)
}

// ---------------------------------------------------------------------------------------------------
// Attaching, querying
// ---------------------------------------------------------------------------------------------------

/// The element local names `attachShadow` accepts besides valid custom element names (DOM §4.9
/// "valid shadow host name").
pub fn is_valid_shadow_host_name(local: &str) -> bool {
    matches!(
        local,
        "article" | "aside" | "blockquote" | "body" | "div" | "footer" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            | "header" | "main" | "nav" | "p" | "section" | "span"
    ) || super::custom::valid_custom_element_name(local)
}

/// Creates `host`'s shadow root (a DocumentFragment in `arena`) and records it. The caller has run
/// "attach a shadow root"'s checks (namespace, name, disabled shadow, already a host).
pub fn attach(arena: &Arena, host: NodeId, init: ShadowInit) -> NodeId {
    let root = arena.borrow_mut().create(NodeData::DocumentFragment);
    let info = ShadowRootInfo {
        root,
        host,
        mode: init.mode,
        delegates_focus: init.delegates_focus,
        slot_assignment: init.slot_assignment,
        clonable: init.clonable,
        serializable: init.serializable,
        declarative: false,
    };
    let k = arena_key(arena);
    TABLES.with(|t| {
        let mut t = t.borrow_mut();
        let fresh = t.get(&k).is_none_or(|e| e.arena.strong_count() == 0);
        if fresh {
            t.insert(k, Entry { arena: std::rc::Rc::downgrade(arena), table: DocTable::default() });
        }
        let e = t.get_mut(&k).expect("entry");
        e.table.by_host.insert(host, info);
        e.table.host_of.insert(root, host);
    });
    root
}

/// Marks shadow root `root` as declarative (HTML §13.2.6.4.4: a `<template shadowrootmode>` attached
/// it) or not (an `attachShadow` that reused a declarative root clears it).
pub fn set_declarative(d: &Document, root: NodeId, declarative: bool) {
    with_table_mut(d, |t| {
        if let Some(h) = t.host_of.get(&root).copied() {
            if let Some(i) = t.by_host.get_mut(&h) {
                i.declarative = declarative;
            }
        }
    });
}

/// `host`'s shadow root, if `host` is a shadow host.
pub fn shadow_root_of(d: &Document, host: NodeId) -> Option<ShadowRootInfo> {
    with_table(d, |t| t.by_host.get(&host).copied()).flatten()
}

/// The host of shadow root `root`.
pub fn host_of(d: &Document, root: NodeId) -> Option<NodeId> {
    with_table(d, |t| t.host_of.get(&root).copied()).flatten()
}

/// The shadow root info of `root` when `root` is a shadow root.
pub fn info_of_root(d: &Document, root: NodeId) -> Option<ShadowRootInfo> {
    with_table(d, |t| t.host_of.get(&root).and_then(|h| t.by_host.get(h)).copied()).flatten()
}

pub fn is_shadow_root(d: &Document, id: NodeId) -> bool {
    host_of(d, id).is_some()
}

/// DOM "root": the topmost inclusive ancestor (a shadow root for a node in a shadow tree).
pub fn root(d: &Document, id: NodeId) -> NodeId {
    let mut cur = id;
    while let Some(p) = d.parent(cur) {
        cur = p;
    }
    cur
}

/// The host of the shadow tree `id` lives in (None in a light tree).
pub fn containing_host(d: &Document, id: NodeId) -> Option<NodeId> {
    host_of(d, root(d, id))
}

/// DOM "shadow-including root": the root, then the host's shadow-including root while the root is a
/// shadow root. Bounded: a host can never sit inside its own shadow tree (the binding's hierarchy
/// check refuses it), but a page must not be able to hang the engine if one ever did.
pub fn shadow_including_root(d: &Document, id: NodeId) -> NodeId {
    let mut cur = root(d, id);
    if !any(d) {
        return cur;
    }
    for _ in 0..4096 {
        match host_of(d, cur) {
            Some(h) => cur = root(d, h),
            None => break,
        }
    }
    cur
}

/// Connected: the shadow-including root is the page's Document (the arena root).
pub fn is_connected(d: &Document, id: NodeId) -> bool {
    shadow_including_root(d, id) == Document::ROOT
}

/// Is `a` a shadow-including inclusive ancestor of `b`?
pub fn is_shadow_including_inclusive_ancestor(d: &Document, a: NodeId, b: NodeId) -> bool {
    let mut cur = b;
    for _ in 0..4096 {
        if d.is_inclusive_ancestor(a, cur) {
            return true;
        }
        match host_of(d, root(d, cur)) {
            Some(h) => cur = h,
            None => return false,
        }
    }
    false
}

/// The shadow-including (inclusive) descendants of `id` in shadow-including tree order: preorder,
/// each host's shadow tree right after the host and before its children. Iterative (depth is the
/// page's).
pub fn shadow_including_descendants(d: &Document, id: NodeId, inclusive: bool) -> Vec<NodeId> {
    let mut out = Vec::new();
    let has = any(d);
    let mut stack = vec![id];
    while let Some(n) = stack.pop() {
        if n != id || inclusive {
            out.push(n);
        }
        let kids: Vec<NodeId> = d.children(n).collect();
        for k in kids.into_iter().rev() {
            stack.push(k);
        }
        if has {
            if let Some(sr) = shadow_root_of(d, n) {
                stack.push(sr.root);
            }
        }
    }
    // the shadow root itself is a node of the traversal only when reached through its host
    out
}

// ---------------------------------------------------------------------------------------------------
// Slots (DOM §4.2.2.3, §4.2.2.4)
// ---------------------------------------------------------------------------------------------------

/// An HTML `<slot>` element.
pub fn is_slot(d: &Document, id: NodeId) -> bool {
    d.element(id).is_some_and(|e| e.is_html("slot"))
}

/// A slottable: an Element or a Text node.
pub fn is_slottable(d: &Document, id: NodeId) -> bool {
    matches!(d.data(id), NodeData::Element(_) | NodeData::Text(_))
}

/// A slot's name (its `name` attribute, "" when absent).
pub fn slot_name(d: &Document, slot: NodeId) -> String {
    d.element(slot).and_then(|e| e.attr("name")).unwrap_or("").to_string()
}

/// A slottable's name (an element's `slot` attribute; "" for Text and when absent).
pub fn slottable_name(d: &Document, n: NodeId) -> String {
    d.element(n).and_then(|e| e.attr("slot")).unwrap_or("").to_string()
}

fn slots_in(d: &Document, root: NodeId) -> Vec<NodeId> {
    d.descendants(root).filter(|&n| is_slot(d, n)).collect()
}

/// DOM "find a slot" for `slottable` (`open`: only in an open shadow root).
pub fn find_slot(d: &Document, slottable: NodeId, open: bool) -> Option<NodeId> {
    if !is_slottable(d, slottable) {
        return None;
    }
    let parent = d.parent(slottable)?;
    let sr = shadow_root_of(d, parent)?;
    if open && sr.mode != ShadowMode::Open {
        return None;
    }
    match sr.slot_assignment {
        SlotAssignment::Manual => {
            let s = with_table(d, |t| t.manual_slot.get(&slottable).copied()).flatten()?;
            (root(d, s) == sr.root && is_slot(d, s)).then_some(s)
        }
        SlotAssignment::Named => {
            let name = slottable_name(d, slottable);
            d.descendants(sr.root).find(|&n| is_slot(d, n) && slot_name(d, n) == name)
        }
    }
}

/// DOM "find slottables" for `slot`: the nodes assigned to it.
pub fn find_slottables(d: &Document, slot: NodeId) -> Vec<NodeId> {
    let r = root(d, slot);
    let Some(sr) = info_of_root(d, r) else { return Vec::new() };
    let host = sr.host;
    match sr.slot_assignment {
        SlotAssignment::Manual => {
            let list = with_table(d, |t| t.manual.get(&slot).cloned()).flatten().unwrap_or_default();
            list.into_iter().filter(|&n| d.parent(n) == Some(host)).collect()
        }
        SlotAssignment::Named => {
            d.children(host).filter(|&c| is_slottable(d, c) && find_slot(d, c, false) == Some(slot)).collect()
        }
    }
}

/// DOM "find flattened slottables" for `slot`.
pub fn flattened_slottables(d: &Document, slot: NodeId) -> Vec<NodeId> {
    fn go(d: &Document, slot: NodeId, out: &mut Vec<NodeId>, depth: u32) {
        if depth > 64 || !is_shadow_root(d, root(d, slot)) {
            return;
        }
        let mut list = find_slottables(d, slot);
        if list.is_empty() {
            list = d.children(slot).filter(|&c| is_slottable(d, c)).collect();
        }
        for n in list {
            if is_slot(d, n) && is_shadow_root(d, root(d, n)) {
                go(d, n, out, depth + 1);
            } else {
                out.push(n);
            }
        }
    }
    let mut out = Vec::new();
    go(d, slot, &mut out, 0);
    out
}

/// `HTMLSlotElement.assign(...nodes)`: the manual assignment bookkeeping; returns the slot's root for
/// the caller to [`refresh`].
pub fn assign_manual(d: &Document, slot: NodeId, nodes: &[NodeId]) {
    with_table_mut(d, |t| {
        if let Some(old) = t.manual.remove(&slot) {
            for n in old {
                if t.manual_slot.get(&n) == Some(&slot) {
                    t.manual_slot.remove(&n);
                }
            }
        }
        let mut set: Vec<NodeId> = Vec::new();
        for &n in nodes {
            if let Some(prev) = t.manual_slot.get(&n).copied() {
                if let Some(l) = t.manual.get_mut(&prev) {
                    l.retain(|x| *x != n);
                }
            }
            t.manual_slot.insert(n, slot);
            if !set.contains(&n) {
                set.push(n);
            }
        }
        t.manual.insert(slot, set);
    });
}

/// "assign slottables for a tree" + "signal a slot change": recomputes every slot of shadow root
/// `root`, signalling each slot whose assigned nodes changed since last seen.
pub fn refresh(d: &Document, root: NodeId) {
    if !is_shadow_root(d, root) {
        return;
    }
    for s in slots_in(d, root) {
        let now = find_slottables(d, s);
        let before = with_table(d, |t| t.assigned.get(&s).cloned()).flatten().unwrap_or_default();
        if now != before {
            super::mutation::signal_slot(s);
            with_table_mut(d, |t| {
                if now.is_empty() {
                    t.assigned.remove(&s);
                } else {
                    t.assigned.insert(s, now);
                }
            });
        }
    }
}

fn subtree_has_slot(d: &Document, n: NodeId) -> bool {
    is_slot(d, n) || d.descendants(n).any(|c| is_slot(d, c))
}

/// The slot steps of DOM "insert" (§4.2.3 step 7.4–7.6) for `nodes` just inserted into `parent`.
pub fn after_insert(d: &Document, parent: NodeId, nodes: &[NodeId]) {
    if !any(d) {
        return;
    }
    if let Some(sr) = shadow_root_of(d, parent) {
        if sr.slot_assignment == SlotAssignment::Named && nodes.iter().any(|&n| is_slottable(d, n)) {
            refresh(d, sr.root);
        }
    }
    let pr = root(d, parent);
    if is_shadow_root(d, pr) {
        if is_slot(d, parent) && find_slottables(d, parent).is_empty() {
            super::mutation::signal_slot(parent);
        }
        if nodes.iter().any(|&n| subtree_has_slot(d, n)) {
            refresh(d, pr);
        }
    }
}

/// The slot steps of DOM "remove" (§4.2.3 steps 7–9) once `node` has left `parent`.
pub fn after_remove(d: &Document, node: NodeId, parent: NodeId) {
    if !any(d) {
        return;
    }
    if let Some(sr) = shadow_root_of(d, parent) {
        refresh(d, sr.root);
    }
    let pr = root(d, parent);
    if is_shadow_root(d, pr) {
        if is_slot(d, parent) && find_slottables(d, parent).is_empty() {
            super::mutation::signal_slot(parent);
        }
        if subtree_has_slot(d, node) {
            refresh(d, pr);
        }
    }
    // Slots that left a shadow tree with `node` lose their assigned nodes.
    let gone: Vec<NodeId> = std::iter::once(node).chain(d.descendants(node)).filter(|&n| is_slot(d, n)).collect();
    for s in gone {
        if is_shadow_root(d, root(d, s)) {
            continue;
        }
        let had = with_table_mut(d, |t| t.assigned.remove(&s).is_some_and(|v| !v.is_empty())).unwrap_or(false);
        if had {
            super::mutation::signal_slot(s);
        }
    }
}

/// The slot steps of the attribute change steps: a slottable's `slot`, a slot's `name`.
pub fn after_attribute(d: &Document, el: NodeId, local: &str) {
    if !any(d) {
        return;
    }
    if local == "slot" {
        if let Some(p) = d.parent(el) {
            if let Some(sr) = shadow_root_of(d, p) {
                refresh(d, sr.root);
            }
        }
    }
    if local == "name" && is_slot(d, el) {
        let r = root(d, el);
        refresh(d, r);
    }
}

/// The flat tree (CSS Scoping §2.2): a host's children are its shadow root's children; a slot in a
/// shadow tree has its assigned nodes, else its own children as fallback; anything else its children.
pub fn flat_children(d: &Document, id: NodeId) -> Vec<NodeId> {
    if any(d) {
        if let Some(sr) = shadow_root_of(d, id) {
            return d.children(sr.root).collect();
        }
        if is_slot(d, id) && is_shadow_root(d, root(d, id)) {
            let a = find_slottables(d, id);
            if !a.is_empty() {
                return a;
            }
        }
    }
    d.children(id).collect()
}

// ---------------------------------------------------------------------------------------------------
// Events (DOM §2.5, §2.9)
// ---------------------------------------------------------------------------------------------------

/// DOM §2.5 "retarget A against B" (`b` None = a non-node such as the Window): while A's root is a
/// shadow root that is not a shadow-including inclusive ancestor of B, A becomes that root's host.
pub fn retarget(d: &Document, a: NodeId, b: Option<NodeId>) -> NodeId {
    if !any(d) {
        return a;
    }
    let mut a = a;
    for _ in 0..4096 {
        let r = root(d, a);
        let Some(h) = host_of(d, r) else { return a };
        if let Some(b) = b {
            if is_shadow_including_inclusive_ancestor(d, r, b) {
                return a;
            }
        }
        a = h;
    }
    a
}

/// DOM §2.9 "get the parent" of `node` for an event first dispatched at `target`: a slottable's
/// assigned slot; a shadow root's host, unless the event is not composed and the shadow root is the
/// root of `target`; else the parent.
pub fn event_parent(d: &Document, node: NodeId, composed: bool, target: NodeId) -> Option<NodeId> {
    if !any(d) {
        return d.parent(node);
    }
    if let Some(s) = find_slot(d, node, false) {
        return Some(s);
    }
    if let Some(h) = host_of(d, node) {
        if !composed && root(d, target) == node {
            return None;
        }
        return Some(h);
    }
    d.parent(node)
}

/// The indices of `path` (`None` = a non-node entry such as the Window) that `composedPath()` returns
/// when the current target is `path[current]` (DOM §2.9 `composedPath()`, closed shadow trees hidden).
pub fn composed_path(d: &Document, path: &[Option<NodeId>], current: usize) -> Vec<usize> {
    let closed_root = |i: usize| path[i].and_then(|n| info_of_root(d, n)).is_some_and(|i| i.mode == ShadowMode::Closed);
    let closed_slot = |i: usize| {
        path[i].is_some_and(|n| is_slot(d, n) && info_of_root(d, root(d, n)).is_some_and(|i| i.mode == ShadowMode::Closed))
    };
    if path.is_empty() || current >= path.len() {
        return Vec::new();
    }
    let mut level: i64 = 0;
    let mut index = path.len() as i64 - 1;
    let mut current_index = current;
    while index >= 0 {
        let i = index as usize;
        if closed_root(i) {
            level += 1;
        }
        if i == current {
            current_index = i;
            break;
        }
        if closed_slot(i) {
            level -= 1;
        }
        index -= 1;
    }
    let mut before: Vec<usize> = Vec::new();
    let (mut cur, mut max) = (level, level);
    let mut i = current_index as i64 - 1;
    while i >= 0 {
        let k = i as usize;
        if closed_root(k) {
            cur += 1;
        }
        if cur <= max {
            before.push(k);
        }
        if closed_slot(k) {
            cur -= 1;
            if cur < max {
                max = cur;
            }
        }
        i -= 1;
    }
    before.reverse();
    let mut out = before;
    out.push(current_index);
    let (mut cur, mut max) = (level, level);
    for k in current_index + 1..path.len() {
        if closed_slot(k) {
            cur += 1;
        }
        if cur <= max {
            out.push(k);
        }
        if closed_root(k) {
            cur -= 1;
            if cur < max {
                max = cur;
            }
        }
    }
    out
}

/// Is `id` an HTML element (the namespace shadow hosts and slots must be in)?
pub fn is_html_element(d: &Document, id: NodeId) -> bool {
    d.element(id).is_some_and(|e| e.ns == Namespace::Html)
}

// ---------------------------------------------------------------------------------------------------
// NodeRef conveniences (the hooks layout and the cascade call)
// ---------------------------------------------------------------------------------------------------

impl NodeRef {
    /// Children in the flat tree (see [`flat_children`]) — what style and layout walk once they are
    /// shadow-aware.
    pub fn flat_children(&self) -> Vec<NodeRef> {
        let ids = self.with_doc(flat_children);
        ids.into_iter().map(|i| self.node_at(i)).collect()
    }

    /// This element's shadow root, open or closed.
    pub fn shadow_root(&self) -> Option<NodeRef> {
        let r = self.with_doc(|d, id| shadow_root_of(d, id).map(|i| i.root));
        r.map(|i| self.node_at(i))
    }

    /// The host when this node is a shadow root.
    pub fn shadow_host(&self) -> Option<NodeRef> {
        let h = self.with_doc(host_of);
        h.map(|i| self.node_at(i))
    }
}
