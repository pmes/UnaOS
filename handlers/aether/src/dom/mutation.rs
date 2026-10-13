//! Mutation observers (AETHERSHADOW, LEDGER SR82): DOM §4.3 as data, VM-free.
//!
//! The page's observers (creation order is delivery order), each node's registered observer list
//! (with transient registrations that remember their source), every observer's record queue, the
//! signal-slots set and the "mutation observer microtask queued" flag. The binding (`js::mutation`)
//! turns records into `MutationRecord`s and runs "notify mutation observers" as a microtask; the
//! mutation algorithms (`js::dom`, through `dom::*_steps`) report each change here.
//!
//! One page's script state lives on one thread, over the page's one arena: nodes are its `NodeId`s.
//! [`reset`] runs when a page's engine is created.

use html_core::{Document, NodeId};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

/// `MutationObserverInit`, after §4.3.1's defaulting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    pub child_list: bool,
    pub attributes: bool,
    pub character_data: bool,
    pub subtree: bool,
    pub attribute_old_value: bool,
    pub character_data_old_value: bool,
    pub attribute_filter: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordKind {
    Attributes,
    CharacterData,
    ChildList,
}

impl RecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RecordKind::Attributes => "attributes",
            RecordKind::CharacterData => "characterData",
            RecordKind::ChildList => "childList",
        }
    }
}

/// A `MutationRecord`, as data.
#[derive(Clone, Debug)]
pub struct Record {
    pub kind: RecordKind,
    pub target: NodeId,
    pub added: Vec<NodeId>,
    pub removed: Vec<NodeId>,
    pub prev: Option<NodeId>,
    pub next: Option<NodeId>,
    pub attr_name: Option<String>,
    pub attr_ns: Option<String>,
    pub old_value: Option<String>,
}

#[derive(Clone, Debug)]
struct Registration {
    observer: u32,
    options: Options,
    serial: u64,
    /// For a transient registration: the serial of the registration it was copied from.
    source: Option<u64>,
}

struct Observer {
    id: u32,
    /// Host-root indices of the callback and of the observer object (the binding's).
    callback: usize,
    object: usize,
    queue: VecDeque<Record>,
    nodes: Vec<NodeId>,
}

#[derive(Default)]
struct State {
    observers: Vec<Observer>,
    lists: HashMap<NodeId, Vec<Registration>>,
    serial: u64,
    next_id: u32,
    microtask_queued: bool,
    /// The microtask is owed but not yet on the VM's job queue (the binding pushes it).
    job_owed: bool,
    signal_slots: Vec<NodeId>,
    suppress: Vec<NodeId>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// A new page: no observers, no registrations, nothing queued.
pub fn reset() {
    with(|s| *s = State::default());
}

/// A new observer (`new MutationObserver(callback)`); returns its id.
pub fn new_observer(callback: usize, object: usize) -> u32 {
    with(|s| {
        s.next_id += 1;
        let id = s.next_id;
        s.observers.push(Observer { id, callback, object, queue: VecDeque::new(), nodes: Vec::new() });
        id
    })
}

/// The observer's (callback, object) host roots.
pub fn observer_roots(id: u32) -> Option<(usize, usize)> {
    with(|s| s.observers.iter().find(|o| o.id == id).map(|o| (o.callback, o.object)))
}

/// `observe(target, options)` steps 7–8 (the binding validated and defaulted `options`).
pub fn observe(id: u32, target: NodeId, options: Options) {
    with(|s| {
        let existing = s
            .lists
            .get(&target)
            .and_then(|l| l.iter().find(|r| r.observer == id && r.source.is_none()).map(|r| r.serial));
        match existing {
            Some(serial) => {
                let nodes = s.observers.iter().find(|o| o.id == id).map(|o| o.nodes.clone()).unwrap_or_default();
                for n in nodes {
                    if let Some(l) = s.lists.get_mut(&n) {
                        l.retain(|r| r.source != Some(serial));
                    }
                }
                if let Some(l) = s.lists.get_mut(&target) {
                    if let Some(r) = l.iter_mut().find(|r| r.serial == serial) {
                        r.options = options;
                    }
                }
            }
            None => {
                s.serial += 1;
                let serial = s.serial;
                s.lists.entry(target).or_default().push(Registration { observer: id, options, serial, source: None });
                if let Some(o) = s.observers.iter_mut().find(|o| o.id == id) {
                    if !o.nodes.contains(&target) {
                        o.nodes.push(target);
                    }
                }
            }
        }
    });
}

/// `disconnect()`: drop every registration of the observer and empty its queue.
pub fn disconnect(id: u32) {
    with(|s| {
        let nodes = match s.observers.iter_mut().find(|o| o.id == id) {
            Some(o) => {
                o.queue.clear();
                std::mem::take(&mut o.nodes)
            }
            None => return,
        };
        for n in nodes {
            if let Some(l) = s.lists.get_mut(&n) {
                l.retain(|r| r.observer != id);
                if l.is_empty() {
                    s.lists.remove(&n);
                }
            }
        }
    });
}

/// `takeRecords()`: the queue, emptied.
pub fn take_records(id: u32) -> Vec<Record> {
    with(|s| s.observers.iter_mut().find(|o| o.id == id).map(|o| o.queue.drain(..).collect()).unwrap_or_default())
}

// ---------------------------------------------------------------------------------------------------
// The microtask
// ---------------------------------------------------------------------------------------------------

/// "queue a mutation observer microtask": once until delivered.
fn queue_microtask(s: &mut State) {
    if !s.microtask_queued {
        s.microtask_queued = true;
        s.job_owed = true;
    }
}

/// True (and cleared) when the binding owes the VM a notify job.
pub fn take_job_request() -> bool {
    with(|s| std::mem::take(&mut s.job_owed))
}

/// "notify mutation observers" steps 1–3: clears the queued flag and returns the observers (in
/// creation order) and the signal slots (emptied).
pub fn begin_notify() -> (Vec<u32>, Vec<NodeId>) {
    with(|s| {
        s.microtask_queued = false;
        s.job_owed = false;
        (s.observers.iter().map(|o| o.id).collect(), std::mem::take(&mut s.signal_slots))
    })
}

/// "notify mutation observers" step 5.1–5.3 for one observer: its records (queue emptied) after the
/// transient registrations it left on its nodes are removed.
pub fn take_for_delivery(id: u32) -> Vec<Record> {
    with(|s| {
        let Some(o) = s.observers.iter_mut().find(|o| o.id == id) else { return Vec::new() };
        let records: Vec<Record> = o.queue.drain(..).collect();
        let nodes = o.nodes.clone();
        let mut keep = Vec::new();
        for n in &nodes {
            let mut still = false;
            if let Some(l) = s.lists.get_mut(n) {
                l.retain(|r| !(r.observer == id && r.source.is_some()));
                still = l.iter().any(|r| r.observer == id);
                if l.is_empty() {
                    s.lists.remove(n);
                }
            }
            if still {
                keep.push(*n);
            }
        }
        if let Some(o) = s.observers.iter_mut().find(|o| o.id == id) {
            o.nodes = keep;
        }
        records
    })
}

/// "signal a slot change": `slot` joins the signal slots; the microtask is queued.
pub fn signal_slot(slot: NodeId) {
    with(|s| {
        if !s.signal_slots.contains(&slot) {
            s.signal_slots.push(slot);
        }
        queue_microtask(s);
    });
}

// ---------------------------------------------------------------------------------------------------
// Queueing records (§4.3.2)
// ---------------------------------------------------------------------------------------------------

/// Suppresses the childList records of `parent` (the "suppress observers flag" of a removal from, or
/// an insertion into, `parent`) while the guard lives.
pub struct Suppress(NodeId);

pub fn suppress(parent: NodeId) -> Suppress {
    with(|s| s.suppress.push(parent));
    Suppress(parent)
}

impl Drop for Suppress {
    fn drop(&mut self) {
        let p = self.0;
        with(|s| {
            if let Some(i) = s.suppress.iter().rposition(|x| *x == p) {
                s.suppress.remove(i);
            }
        });
    }
}

fn suppressed(s: &State, parent: NodeId) -> bool {
    s.suppress.contains(&parent)
}

fn inclusive_ancestors(d: &Document, n: NodeId) -> Vec<NodeId> {
    let mut v = vec![n];
    let mut cur = n;
    while let Some(p) = d.parent(cur) {
        v.push(p);
        cur = p;
    }
    v
}

/// DOM "queue a mutation record".
fn queue_record(s: &mut State, d: &Document, rec: Record, old_value: Option<String>) {
    if s.lists.is_empty() {
        return;
    }
    let mut interested: Vec<(u32, Option<String>)> = Vec::new();
    for node in inclusive_ancestors(d, rec.target) {
        let Some(list) = s.lists.get(&node) else { continue };
        for r in list {
            let o = &r.options;
            if node != rec.target && !o.subtree {
                continue;
            }
            match rec.kind {
                RecordKind::Attributes => {
                    if !o.attributes {
                        continue;
                    }
                    if let Some(f) = &o.attribute_filter {
                        let name = rec.attr_name.as_deref().unwrap_or("");
                        if rec.attr_ns.is_some() || !f.iter().any(|x| x == name) {
                            continue;
                        }
                    }
                }
                RecordKind::CharacterData => {
                    if !o.character_data {
                        continue;
                    }
                }
                RecordKind::ChildList => {
                    if !o.child_list {
                        continue;
                    }
                }
            }
            let i = match interested.iter().position(|(id, _)| *id == r.observer) {
                Some(i) => i,
                None => {
                    interested.push((r.observer, None));
                    interested.len() - 1
                }
            };
            if (rec.kind == RecordKind::Attributes && o.attribute_old_value)
                || (rec.kind == RecordKind::CharacterData && o.character_data_old_value)
            {
                interested[i].1 = old_value.clone();
            }
        }
    }
    if interested.is_empty() {
        return;
    }
    for (id, old) in interested {
        if let Some(o) = s.observers.iter_mut().find(|o| o.id == id) {
            let mut r = rec.clone();
            r.old_value = old;
            o.queue.push_back(r);
        }
    }
    queue_microtask(s);
}

fn tree(target: NodeId, added: Vec<NodeId>, removed: Vec<NodeId>, prev: Option<NodeId>, next: Option<NodeId>) -> Record {
    Record { kind: RecordKind::ChildList, target, added, removed, prev, next, attr_name: None, attr_ns: None, old_value: None }
}

/// DOM "queue a tree mutation record" (unconditional; nothing when both lists are empty).
pub fn tree_record(d: &Document, target: NodeId, added: Vec<NodeId>, removed: Vec<NodeId>, prev: Option<NodeId>, next: Option<NodeId>) {
    if added.is_empty() && removed.is_empty() {
        return;
    }
    with(|s| queue_record(s, d, tree(target, added, removed, prev, next), None));
}

/// The observer steps of DOM "remove" for `node` (still in `parent`): transient registrations for the
/// subtree observers of `parent`'s inclusive ancestors, then the childList record unless suppressed.
pub fn removing(d: &Document, node: NodeId, parent: NodeId) {
    with(|s| {
        if s.lists.is_empty() {
            return;
        }
        let mut transients: Vec<Registration> = Vec::new();
        for a in inclusive_ancestors(d, parent) {
            if let Some(l) = s.lists.get(&a) {
                for r in l.iter().filter(|r| r.options.subtree) {
                    transients.push(r.clone());
                }
            }
        }
        for r in transients {
            s.serial += 1;
            let t = Registration { observer: r.observer, options: r.options.clone(), serial: s.serial, source: Some(r.serial) };
            s.lists.entry(node).or_default().push(t);
            if let Some(o) = s.observers.iter_mut().find(|o| o.id == r.observer) {
                if !o.nodes.contains(&node) {
                    o.nodes.push(node);
                }
            }
        }
        if !suppressed(s, parent) {
            let rec = tree(parent, Vec::new(), vec![node], d.prev_sibling(node), d.next_sibling(node));
            queue_record(s, d, rec, None);
        }
    });
}

/// The childList record of DOM "insert" (unless the insertion into `parent` is suppressed).
pub fn inserted(d: &Document, parent: NodeId, added: &[NodeId], prev: Option<NodeId>, next: Option<NodeId>) {
    with(|s| {
        if added.is_empty() || suppressed(s, parent) {
            return;
        }
        queue_record(s, d, tree(parent, added.to_vec(), Vec::new(), prev, next), None);
    });
}

/// The record of DOM "handle attribute changes".
pub fn attribute(d: &Document, el: NodeId, name: &str, ns: Option<&str>, old: Option<String>) {
    with(|s| {
        let rec = Record {
            kind: RecordKind::Attributes,
            target: el,
            added: Vec::new(),
            removed: Vec::new(),
            prev: None,
            next: None,
            attr_name: Some(name.to_string()),
            attr_ns: ns.filter(|n| !n.is_empty()).map(str::to_string),
            old_value: None,
        };
        queue_record(s, d, rec, old);
    });
}

/// The record of DOM "replace data".
pub fn character_data(d: &Document, node: NodeId, old: String) {
    with(|s| {
        let rec = Record {
            kind: RecordKind::CharacterData,
            target: node,
            added: Vec::new(),
            removed: Vec::new(),
            prev: None,
            next: None,
            attr_name: None,
            attr_ns: None,
            old_value: None,
        };
        queue_record(s, d, rec, Some(old));
    });
}
