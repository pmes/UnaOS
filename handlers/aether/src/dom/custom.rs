//! Custom elements (AETHERSHADOW, LEDGER SR82): HTML §4.13 as data, VM-free.
//!
//! The page's registry of definitions (the constructor and lifecycle callbacks are host-root indices
//! the binding owns), each element's custom element state / definition / `is` value, the per-element
//! reaction queues and the element queue the binding drains at its [CEReactions] boundaries
//! (`js::custom::flush`), "look up a custom element definition", upgrade candidates, and the
//! connected / disconnected / attribute-changed enqueueing the mutation algorithms report through
//! `dom::*_steps`. [`is_defined`] answers `:defined` for the selector matcher.
//!
//! Like the observers, this is one page's script state over the page's one arena; [`reset`] binds it to
//! that arena (a `Weak`, so the matcher can tell the page's document from any other it styles).

use super::Arena;
use html_core::{Document, Namespace, NodeId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Weak;

/// An element's custom element state (HTML §4.13.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CeState {
    Undefined,
    Failed,
    Uncustomized,
    Precustomized,
    Custom,
}

/// The lifecycle callbacks a definition may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Callback {
    Connected = 0,
    Disconnected = 1,
    Adopted = 2,
    AttributeChanged = 3,
}

impl Callback {
    pub fn name(self) -> &'static str {
        match self {
            Callback::Connected => "connectedCallback",
            Callback::Disconnected => "disconnectedCallback",
            Callback::Adopted => "adoptedCallback",
            Callback::AttributeChanged => "attributeChangedCallback",
        }
    }
}

/// A callback argument.
#[derive(Clone, Debug)]
pub enum Arg {
    Null,
    Str(String),
    Node(NodeId),
}

/// A custom element reaction (HTML §4.13.6).
#[derive(Clone, Debug)]
pub enum Reaction {
    /// Upgrade with definition index.
    Upgrade(usize),
    Callback(Callback, Vec<Arg>),
}

/// A custom element definition (HTML §4.13.4).
#[derive(Clone, Debug)]
pub struct Definition {
    pub name: String,
    pub local_name: String,
    /// Host root of the constructor.
    pub constructor: usize,
    pub observed: Vec<String>,
    /// Host roots of connected/disconnected/adopted/attributeChanged callbacks.
    pub callbacks: [Option<usize>; 4],
    /// The construction stack: an element being upgraded, or `None` = the already-constructed marker.
    pub construction_stack: Vec<Option<NodeId>>,
    pub disable_shadow: bool,
    pub disable_internals: bool,
    pub form_associated: bool,
}

impl Definition {
    pub fn is_autonomous(&self) -> bool {
        self.name == self.local_name
    }
}

#[derive(Default)]
struct ElementInfo {
    state: Option<CeState>,
    def: Option<usize>,
    is: Option<String>,
    reactions: VecDeque<Reaction>,
}

#[derive(Default)]
struct Registry {
    arena: Weak<RefCell<Document>>,
    key: usize,
    defs: Vec<Definition>,
    elements: HashMap<NodeId, ElementInfo>,
    queue: VecDeque<NodeId>,
    busy: HashSet<NodeId>,
    running: bool,
    when_defined: HashMap<String, usize>,
}

thread_local! {
    static REG: RefCell<Registry> = RefCell::new(Registry::default());
}

fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    REG.with(|r| f(&mut r.borrow_mut()))
}

/// A new page: an empty registry bound to `arena`.
pub fn reset(arena: &Arena) {
    with(|r| {
        *r = Registry::default();
        r.arena = std::rc::Rc::downgrade(arena);
        r.key = arena.as_ptr() as *const Document as usize;
    });
}

fn is_page_doc(r: &Registry, d: &Document) -> bool {
    r.key == super::shadow::key(d) && r.arena.strong_count() > 0
}

// ---------------------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------------------

fn pcen_char(c: char) -> bool {
    matches!(c,
        '-' | '.' | '0'..='9' | '_' | 'a'..='z' | '\u{B7}'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}' | '\u{203F}'..='\u{2040}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}'
        | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

/// HTML §4.13.2 "valid custom element name".
pub fn valid_custom_element_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some('a'..='z') => {}
        _ => return false,
    }
    if !name.contains('-') || !chars.all(pcen_char) {
        return false;
    }
    !matches!(
        name,
        "annotation-xml" | "color-profile" | "font-face" | "font-face-src" | "font-face-uri" | "font-face-format"
            | "font-face-name" | "missing-glyph"
    )
}

// ---------------------------------------------------------------------------------------------------
// Definitions
// ---------------------------------------------------------------------------------------------------

/// Adds a definition; returns its index.
pub fn define(def: Definition) -> usize {
    with(|r| {
        r.defs.push(def);
        r.defs.len() - 1
    })
}

pub fn definition(i: usize) -> Option<Definition> {
    with(|r| r.defs.get(i).cloned())
}

pub fn definition_count() -> usize {
    with(|r| r.defs.len())
}

pub fn by_name(name: &str) -> Option<usize> {
    with(|r| r.defs.iter().position(|d| d.name == name))
}

/// "element definition is running".
pub fn running() -> bool {
    with(|r| r.running)
}

pub fn set_running(v: bool) {
    with(|r| r.running = v);
}

/// HTML "look up a custom element definition" (namespace, local name, is value).
pub fn look_up(ns: Namespace, local: &str, is: Option<&str>) -> Option<usize> {
    if ns != Namespace::Html {
        return None;
    }
    with(|r| {
        r.defs.iter().position(|d| d.name == local && d.local_name == local).or_else(|| {
            let is = is?;
            r.defs.iter().position(|d| d.name == is && d.local_name == local)
        })
    })
}

pub fn push_construction(def: usize, el: NodeId) {
    with(|r| {
        if let Some(d) = r.defs.get_mut(def) {
            d.construction_stack.push(Some(el));
        }
    });
}

pub fn pop_construction(def: usize) {
    with(|r| {
        if let Some(d) = r.defs.get_mut(def) {
            d.construction_stack.pop();
        }
    });
}

/// The construction stack's last entry: None = empty, Some(None) = the already-constructed marker.
pub fn last_construction(def: usize) -> Option<Option<NodeId>> {
    with(|r| r.defs.get(def).and_then(|d| d.construction_stack.last().copied()))
}

/// Replaces the last construction stack entry with the already-constructed marker.
pub fn mark_constructed(def: usize) {
    with(|r| {
        if let Some(last) = r.defs.get_mut(def).and_then(|d| d.construction_stack.last_mut()) {
            *last = None;
        }
    });
}

pub fn set_when_defined(name: &str, promise_root: usize) {
    with(|r| r.when_defined.insert(name.to_string(), promise_root));
}

pub fn when_defined(name: &str) -> Option<usize> {
    with(|r| r.when_defined.get(name).copied())
}

pub fn take_when_defined(name: &str) -> Option<usize> {
    with(|r| r.when_defined.remove(name))
}

// ---------------------------------------------------------------------------------------------------
// Element state
// ---------------------------------------------------------------------------------------------------

fn default_state(d: &Document, id: NodeId) -> CeState {
    match d.element(id) {
        Some(e) if e.ns == Namespace::Html && (valid_custom_element_name(&e.local) || e.attr("is").is_some()) => {
            CeState::Undefined
        }
        _ => CeState::Uncustomized,
    }
}

/// An element's custom element state (elements never touched by the registry take the state the
/// parser would have given them: "undefined" for a valid custom element name or an `is` attribute).
pub fn state(d: &Document, id: NodeId) -> CeState {
    with(|r| r.elements.get(&id).and_then(|e| e.state)).unwrap_or_else(|| default_state(d, id))
}

pub fn set_state(id: NodeId, s: CeState) {
    with(|r| r.elements.entry(id).or_default().state = Some(s));
}

pub fn element_definition(id: NodeId) -> Option<usize> {
    with(|r| r.elements.get(&id).and_then(|e| e.def))
}

pub fn set_definition(id: NodeId, def: Option<usize>) {
    with(|r| r.elements.entry(id).or_default().def = def);
}

/// The element's `is` value (set at creation; a parser-created element's comes from its attribute).
pub fn is_value(d: &Document, id: NodeId) -> Option<String> {
    with(|r| r.elements.get(&id).and_then(|e| e.is.clone()))
        .or_else(|| d.element(id).and_then(|e| e.attr("is")).map(str::to_string))
}

pub fn set_is_value(id: NodeId, is: Option<String>) {
    with(|r| r.elements.entry(id).or_default().is = is);
}

/// `:defined`: the element's state is "uncustomized" or "custom".
pub fn is_defined(d: &Document, id: NodeId) -> bool {
    let stored = with(|r| if is_page_doc(r, d) { r.elements.get(&id).and_then(|e| e.state) } else { None });
    matches!(stored.unwrap_or_else(|| default_state(d, id)), CeState::Uncustomized | CeState::Custom)
}

// ---------------------------------------------------------------------------------------------------
// Reactions (HTML §4.13.6)
// ---------------------------------------------------------------------------------------------------

/// Adds `reaction` to `el`'s reaction queue and `el` to the element queue.
pub fn enqueue(el: NodeId, reaction: Reaction) {
    with(|r| {
        r.elements.entry(el).or_default().reactions.push_back(reaction);
        r.queue.push_back(el);
    });
}

/// "enqueue a custom element callback reaction": nothing when the definition lacks the callback, or
/// for an attribute it does not observe.
pub fn enqueue_callback(el: NodeId, cb: Callback, args: Vec<Arg>) {
    let ok = with(|r| {
        let Some(def) = r.elements.get(&el).and_then(|e| e.def).and_then(|i| r.defs.get(i)) else { return false };
        if def.callbacks[cb as usize].is_none() {
            return false;
        }
        if cb == Callback::AttributeChanged {
            let name = match args.first() {
                Some(Arg::Str(s)) => s.as_str(),
                _ => "",
            };
            return def.observed.iter().any(|o| o == name);
        }
        true
    });
    if ok {
        enqueue(el, Reaction::Callback(cb, args));
    }
}

/// HTML "try to upgrade an element".
pub fn try_upgrade(d: &Document, el: NodeId) {
    let Some(e) = d.element(el) else { return };
    let is = is_value(d, el);
    if let Some(def) = look_up(e.ns, &e.local, is.as_deref()) {
        enqueue(el, Reaction::Upgrade(def));
    }
}

/// The elements with queued reactions, drained (the binding processes each).
pub fn take_queue() -> Vec<NodeId> {
    with(|r| r.queue.drain(..).collect())
}

/// Marks `el` busy for processing; false when an outer flush is already processing it.
pub fn begin(el: NodeId) -> bool {
    with(|r| r.busy.insert(el))
}

pub fn end(el: NodeId) {
    with(|r| {
        r.busy.remove(&el);
    });
}

pub fn pop_reaction(el: NodeId) -> Option<Reaction> {
    with(|r| r.elements.get_mut(&el).and_then(|e| e.reactions.pop_front()))
}

pub fn clear_reactions(el: NodeId) {
    with(|r| {
        if let Some(e) = r.elements.get_mut(&el) {
            e.reactions.clear();
        }
    });
}

/// Is anything queued? (the binding's fast path)
pub fn pending() -> bool {
    with(|r| !r.queue.is_empty())
}

/// The connected steps of DOM "insert" for a node that became connected: each shadow-including
/// inclusive descendant element gets connectedCallback when custom, else is tried for upgrade.
pub fn connected(d: &Document, node: NodeId) {
    if with(|r| r.defs.is_empty() && r.elements.is_empty()) {
        return;
    }
    for n in super::shadow::shadow_including_descendants(d, node, true) {
        if d.element(n).is_none() {
            continue;
        }
        if state(d, n) == CeState::Custom {
            enqueue_callback(n, Callback::Connected, Vec::new());
        } else {
            try_upgrade(d, n);
        }
    }
}

/// The disconnected steps of DOM "remove" for `node`, which was connected.
pub fn disconnected(d: &Document, node: NodeId) {
    if with(|r| r.defs.is_empty()) {
        return;
    }
    for n in super::shadow::shadow_including_descendants(d, node, true) {
        if d.element(n).is_some() && state(d, n) == CeState::Custom {
            enqueue_callback(n, Callback::Disconnected, Vec::new());
        }
    }
}

/// The custom element half of "handle attribute changes".
pub fn attribute_changed(d: &Document, el: NodeId, local: &str, ns: Namespace, old: Option<String>, new: Option<String>) {
    if with(|r| r.defs.is_empty()) || state(d, el) != CeState::Custom {
        return;
    }
    let opt = |v: Option<String>| v.map(Arg::Str).unwrap_or(Arg::Null);
    let ns_arg = if ns == Namespace::None { Arg::Null } else { Arg::Str(ns.url().to_string()) };
    enqueue_callback(el, Callback::AttributeChanged, vec![Arg::Str(local.to_string()), opt(old), opt(new), ns_arg]);
}

/// HTML `define()` step 15: the shadow-including descendants of the page document that the new
/// definition upgrades, in shadow-including tree order.
pub fn upgrade_candidates(d: &Document, def: usize) -> Vec<NodeId> {
    let Some(def) = definition(def) else { return Vec::new() };
    super::shadow::shadow_including_descendants(d, Document::ROOT, false)
        .into_iter()
        .filter(|&n| {
            d.element(n).is_some_and(|e| e.ns == Namespace::Html && e.local == def.local_name)
                && (def.is_autonomous() || is_value(d, n).as_deref() == Some(def.name.as_str()))
        })
        .collect()
}
