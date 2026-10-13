//! History (HTML §7.7.2) wired to the tab's session history (AETHERCHROME, SR84).
//!
//! The session history itself is the engine's (`chrome::history::SessionHistory`, one per tab — the
//! tab's engine owns this page thread, so these thread-locals are per tab). The binding talks to it in
//! two directions:
//!
//! - **page → session**: `pushState`, `replaceState`, a fragment navigation, and `back`/`forward`/`go`
//!   queue an [`Op`]; the engine drains the queue ([`take_ops`]) after every turn and applies it
//!   (`AetherEngine::drain_history_ops`). `pushState`/`replaceState` are synchronous for the page —
//!   `location` and `history.state` change before the call returns (§7.7.4 "URL and history update
//!   steps") and `history.length` is advanced here at once; the session catches up at the drain.
//!   Traversal is asynchronous by spec (§7.7.2: "traverse the history by a delta" queues a task), so
//!   the queue is exactly its shape.
//! - **session → page**: [`sync`] after each session change (`length`, the current index, and the
//!   current entry's serialized state, which a fresh `History` object starts from); and
//!   [`traverse_same_document`] when a traversal lands on an entry of this same document — the URL
//!   changes in place, `history.state` becomes the entry's state, and `popstate` fires (then
//!   `hashchange` when only the fragment moved), §7.4.6.3 / §7.4.6.4.
//!
//! State is serialized as JSON (StructuredSerializeForStorage approximated, as `pushState` always did
//! here): what survives a JSON round trip survives a traversal.

use std::cell::{Cell, RefCell};

use super::super::idl::*;
use super::super::{page, TargetKey, Task};
use js_core::vm::*;

/// One session-history request from the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// `pushState` (or a fragment navigation): a new same-document entry.
    Push { url: String, state: Option<String> },
    /// `replaceState` (or `location.replace` to a fragment): the current entry changes in place.
    Replace { url: String, state: Option<String> },
    /// `back()` (-1), `forward()` (+1), `go(n)`.
    Go(i32),
}

thread_local! {
    static OPS: RefCell<Vec<Op>> = const { RefCell::new(Vec::new()) };
    static LEN: Cell<u32> = const { Cell::new(1) };
    static INDEX: Cell<u32> = const { Cell::new(0) };
    static STATE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The requests the page made since the last drain, in order.
pub fn take_ops() -> Vec<Op> {
    OPS.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

pub(crate) fn queue(op: Op) {
    match &op {
        // `history.length` is synchronous for the page: a push drops the forward entries and adds one.
        Op::Push { state, .. } => {
            let i = INDEX.with(Cell::get) + 1;
            INDEX.with(|c| c.set(i));
            LEN.with(|c| c.set(i + 1));
            STATE.with(|s| *s.borrow_mut() = state.clone());
        }
        Op::Replace { state, .. } => STATE.with(|s| *s.borrow_mut() = state.clone()),
        Op::Go(_) => {}
    }
    OPS.with(|o| o.borrow_mut().push(op));
}

/// The engine's session history changed: `len` entries, the current at `index`, its serialized state.
pub fn sync(len: usize, index: usize, state: Option<String>) {
    LEN.with(|c| c.set(len.max(1) as u32));
    INDEX.with(|c| c.set(index as u32));
    STATE.with(|s| *s.borrow_mut() = state);
}

/// `history.length`.
pub fn length() -> u32 {
    LEN.with(Cell::get)
}

/// The current entry's serialized state.
pub fn current_state() -> Option<String> {
    STATE.with(|s| s.borrow().clone())
}

/// `JSON.parse(text)`, or `null` for `None` / unparseable text.
pub(crate) fn parse_state(vm: &mut Vm, text: Option<&str>) -> Value {
    let Some(text) = text else { return Value::Null };
    let json = vm.realm().intrinsics.json;
    let Ok(parse) = vm.get(json, &PropertyKey::from_str("parse")) else { return Value::Null };
    vm.call(&parse, &Value::Object(json), &[s(text)]).unwrap_or(Value::Null)
}

/// A traversal landed on an entry of this document: the URL becomes `url`, `history.state` the
/// entry's state, and `popstate` (then `hashchange`, when only the fragment differs from `from_url`)
/// is queued as a task on the page's event loop.
pub fn traverse_same_document(vm: &mut Vm, url: &str, from_url: &str, state: Option<String>) {
    page(|p| p.url = url.to_string());
    super::super::Engine::set_doc_url(url);
    STATE.with(|st| *st.borrow_mut() = state.clone());
    let fp = vm.intr().function_proto;
    let slot0 = match &state {
        Some(t) => s(t),
        None => Value::Null,
    };
    let f = vm.make_native_with("", 0, fire_popstate, false, Some(fp), vec![slot0]);
    let r = root(vm, Value::Object(f));
    page(|p| p.tasks.push_back(Task::Callback(r)));

    let strip = |u: &str| u.split('#').next().unwrap_or("").to_string();
    if strip(url) == strip(from_url) && url != from_url {
        let init = vm.new_plain_object();
        let _ = vm.create_data_property(init, PropertyKey::from_str("oldURL"), s(from_url));
        let _ = vm.create_data_property(init, PropertyKey::from_str("newURL"), s(url));
        let f = vm.make_native_with("", 0, super::fire_hashchange, false, Some(fp), vec![Value::Object(init)]);
        let r = root(vm, Value::Object(f));
        page(|p| p.tasks.push_back(Task::Callback(r)));
    }
}

/// The popstate task: `history.state` takes the entry's state, then `popstate` fires at the window with
/// it (PopStateEvent, not bubbling, not cancelable — §7.4.6.4 step 6).
fn fire_popstate(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let text = match vm.native_slot(ctx.callee, 0) {
        Value::Null | Value::Undefined => None,
        v => Some(string(vm, &v)?),
    };
    let state = parse_state(vm, text.as_deref());
    if let Value::Object(h) = super::history_obj(vm) {
        set_slot(vm, h, 0, state.clone());
    }
    let init = vm.new_plain_object();
    vm.create_data_property(init, PropertyKey::from_str("state"), state)?;
    let e = super::super::events::new_event(vm, "PopStateEvent", "popstate", false, false, Some(init));
    super::super::events::dispatch(vm, e, TargetKey::Window);
    Ok(Value::Undefined)
}
