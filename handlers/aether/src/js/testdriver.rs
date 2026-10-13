//! AETHERWPT5 (LEDGER SR144): the automation surface behind WPT's `testdriver.js`. The stand-in serves an
//! Aether `testdriver-vendor.js` (as it substitutes `testharnessreport.js`) that implements
//! `window.test_driver_internal`'s WebDriver commands over two natives this module installs in the page realm:
//!
//! - `__td_queue(kind, json)` → ticket: queues one command (Element Click, Element Send Keys, Perform Actions,
//!   Set Permission, the cookie commands) for the engine; nothing runs inside the script that queued it, as
//!   nothing runs inside a WebDriver session's script while the command is pending;
//! - `__td_node(element)` → the node id behind a wrapper (the vendor passes element origins and send-keys
//!   targets by id).
//!
//! The engine drains the queue between event-loop slices (`AetherEngine::pump_automation`, lib.rs): each command
//! is dispatched through the same input path the shell drives (`handle_event`, `pointer_moved`, the key and
//! button events of js/events.rs), and the vendor's `__td_settle(ticket, error, result)` resolves or rejects the
//! promise the command returned. Permission states set through `set_permission` are kept here for the
//! Permissions API to read ([`permission`]).

use super::idl::*;
use js_core::vm::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// A queued WebDriver command: the ticket the vendor script waits on and the command's JSON.
pub struct Pending {
    pub ticket: u32,
    pub kind: String,
    pub json: serde_json::Value,
}

thread_local! {
    static QUEUE: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
    static NEXT: Cell<u32> = const { Cell::new(1) };
    static PERMISSIONS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// A new page: the queue and the permission states start empty.
pub fn reset() {
    QUEUE.with(|q| q.borrow_mut().clear());
    PERMISSIONS.with(|p| p.borrow_mut().clear());
}

/// The commands queued since the last call, in order.
pub fn take() -> Vec<Pending> {
    QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// Whether a command is waiting for the engine.
pub fn has_pending() -> bool {
    QUEUE.with(|q| !q.borrow().is_empty())
}

/// The state `set_permission` gave `name` ("granted" / "denied" / "prompt"), if any.
pub fn permission(name: &str) -> Option<String> {
    PERMISSIONS.with(|p| p.borrow().get(name).cloned())
}

pub fn set_permission(name: &str, state: &str) {
    PERMISSIONS.with(|p| p.borrow_mut().insert(name.to_string(), state.to_string()));
}

fn td_queue(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 2, "testdriver", "__td_queue")?;
    let k = arg(vm, ctx, 0);
    let kind = string(vm, &k)?;
    let t = arg(vm, ctx, 1);
    let text = string(vm, &t)?;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    let ticket = NEXT.with(|n| {
        let t = n.get();
        n.set(t + 1);
        t
    });
    QUEUE.with(|q| q.borrow_mut().push(Pending { ticket, kind, json }));
    Ok(num(ticket as f64))
}

fn td_node(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    Ok(match super::dom::node_of(vm, &v) {
        Some(n) => num(n as f64),
        None => Value::Null,
    })
}

/// WebDriver §17.2 "normalised key value": the private-use code point `testdriver.js` sends for a named key
/// (the vendor passes them through; the engine maps them to the shell's key names, `api::uievents::key_init`).
pub fn key_name(ch: char) -> Option<&'static str> {
    Some(match ch as u32 {
        0xE000 => "Unidentified",
        0xE001 => "Cancel",
        0xE002 => "Help",
        0xE003 => "Backspace",
        0xE004 => "Tab",
        0xE005 => "Clear",
        0xE006 | 0xE007 => "Enter",
        0xE008 | 0xE050 => "Shift",
        0xE009 | 0xE051 => "Control",
        0xE00A | 0xE052 => "Alt",
        0xE00B => "Pause",
        0xE00C => "Escape",
        0xE00D => " ",
        0xE00E | 0xE054 => "PageUp",
        0xE00F | 0xE055 => "PageDown",
        0xE010 | 0xE056 => "End",
        0xE011 | 0xE057 => "Home",
        0xE012 | 0xE058 => "ArrowLeft",
        0xE013 | 0xE059 => "ArrowUp",
        0xE014 | 0xE05A => "ArrowRight",
        0xE015 | 0xE05B => "ArrowDown",
        0xE016 | 0xE05C => "Insert",
        0xE017 | 0xE05D => "Delete",
        0xE018 => ";",
        0xE019 => "=",
        0xE01A => "0",
        0xE01B => "1",
        0xE01C => "2",
        0xE01D => "3",
        0xE01E => "4",
        0xE01F => "5",
        0xE020 => "6",
        0xE021 => "7",
        0xE022 => "8",
        0xE023 => "9",
        0xE024 => "*",
        0xE025 => "+",
        0xE026 => ",",
        0xE027 => "-",
        0xE028 => ".",
        0xE029 => "/",
        0xE031 => "F1",
        0xE032 => "F2",
        0xE033 => "F3",
        0xE034 => "F4",
        0xE035 => "F5",
        0xE036 => "F6",
        0xE037 => "F7",
        0xE038 => "F8",
        0xE039 => "F9",
        0xE03A => "F10",
        0xE03B => "F11",
        0xE03C => "F12",
        0xE03D | 0xE053 => "Meta",
        0xE040 => "ZenkakuHankaku",
        _ => return None,
    })
}

/// Installs the two natives in the page realm.
pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    for (name, len, f) in [("__td_queue", 2, td_queue as NativeFn), ("__td_node", 1, td_node)] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
}
