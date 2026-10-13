//! AETHERWPT3 (LEDGER SR138): pointer capture (W3C Pointer Events §10): `Element.setPointerCapture` /
//! `releasePointerCapture` / `hasPointerCapture`, the pending and current pointer capture target overrides,
//! "process pending pointer capture" (`gotpointercapture` / `lostpointercapture`), the implicit release after
//! `pointerup`, the capture target as the target of the pointer events (and their compatibility mouse events)
//! the engine fires for its pointer (js/events.rs `user_button` / `user_mouse`), and implicit capture for a
//! direct-manipulation (touch) pointer at `pointerdown`.
//!
//! The engine has one pointer, the mouse (pointerId 1, `pointerType` "mouse", always active — §4.1.2); a touch
//! source would call [`down`] with [`Kind::Touch`].

use super::dom::{self, this_element};
use super::idl::*;
use js_core::vm::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// The mouse pointer's id (Pointer Events §4.1.2.1: the mouse's pointerId is fixed for the session).
pub const MOUSE_ID: i32 = 1;

/// A pointer's kind (the `pointerType`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Mouse,
    Touch,
}

thread_local! {
    static PENDING: RefCell<HashMap<i32, usize>> = RefCell::new(HashMap::new());
    static CURRENT: RefCell<HashMap<i32, usize>> = RefCell::new(HashMap::new());
    /// The mouse's buttons (the "active buttons state" is buttons != 0).
    static BUTTONS: Cell<u16> = const { Cell::new(0) };
    /// A canceled `pointerdown` suppresses the compatibility mouse events until the pointer is released
    /// (§11.1 "PREVENT MOUSE EVENT flag").
    static PREVENT_MOUSE: Cell<bool> = const { Cell::new(false) };
}

/// A new page.
pub fn reset() {
    PENDING.with(|p| p.borrow_mut().clear());
    CURRENT.with(|c| c.borrow_mut().clear());
    BUTTONS.with(|b| b.set(0));
    PREVENT_MOUSE.with(|p| p.set(false));
}

fn is_active(id: i32) -> bool {
    id == MOUSE_ID
}

/// WebIDL `long` (ToInt32).
fn to_int32(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    let m = n.trunc().rem_euclid(4294967296.0);
    (if m >= 2147483648.0 { m - 4294967296.0 } else { m }) as i32
}

fn pointer_id(vm: &mut Vm, ctx: &CallCtx, what: &str) -> JsResult<i32> {
    need(vm, ctx, 1, "Element", what)?;
    let v = arg(vm, ctx, 0);
    Ok(to_int32(vm.to_number(&v)?))
}

/// `setPointerCapture(pointerId)` (§10.1.1).
fn set_pointer_capture(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let id = pointer_id(vm, ctx, "setPointerCapture")?;
    if !is_active(id) {
        return throw_dom(vm, "NotFoundError", "Failed to execute 'setPointerCapture' on 'Element': No active pointer with the given id is found.");
    }
    if !dom::is_connected(el) {
        return throw_dom(vm, "InvalidStateError", "Failed to execute 'setPointerCapture' on 'Element': InvalidStateError.");
    }
    if BUTTONS.with(Cell::get) == 0 {
        return Ok(Value::Undefined);
    }
    PENDING.with(|p| p.borrow_mut().insert(id, el));
    Ok(Value::Undefined)
}

/// `releasePointerCapture(pointerId)` (§10.1.2).
fn release_pointer_capture(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let id = pointer_id(vm, ctx, "releasePointerCapture")?;
    if !is_active(id) {
        return throw_dom(vm, "NotFoundError", "Failed to execute 'releasePointerCapture' on 'Element': No active pointer with the given id is found.");
    }
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if p.get(&id) == Some(&el) {
            p.remove(&id);
        }
    });
    Ok(Value::Undefined)
}

/// `hasPointerCapture(pointerId)` (§10.1.3): the PENDING override is this element.
fn has_pointer_capture(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_element(vm, ctx)?;
    let id = pointer_id(vm, ctx, "hasPointerCapture")?;
    Ok(Value::Bool(PENDING.with(|p| p.borrow().get(&id) == Some(&el))))
}

/// §10.2 "process pending pointer capture" for `id`: a capture target that left the document loses
/// capture to the document; `lostpointercapture` at the old override, `gotpointercapture` at the new.
pub fn process_pending(vm: &mut Vm, id: i32, client: (f64, f64)) {
    if let Some(c) = CURRENT.with(|c| c.borrow().get(&id).copied()) {
        if !dom::is_connected(c) {
            CURRENT.with(|cur| cur.borrow_mut().remove(&id));
            PENDING.with(|p| {
                let mut p = p.borrow_mut();
                if p.get(&id) == Some(&c) {
                    p.remove(&id);
                }
            });
            let d = dom::main_doc();
            super::events::pointer_event(vm, d, "lostpointercapture", client, -1, buttons(), None);
        }
    }
    let pending = PENDING.with(|p| p.borrow().get(&id).copied());
    let current = CURRENT.with(|c| c.borrow().get(&id).copied());
    if pending == current {
        return;
    }
    if let Some(c) = current {
        super::events::pointer_event(vm, c, "lostpointercapture", client, -1, buttons(), None);
    }
    if let Some(p) = pending {
        super::events::pointer_event(vm, p, "gotpointercapture", client, -1, buttons(), None);
    }
    CURRENT.with(|c| {
        let mut c = c.borrow_mut();
        match pending {
            Some(p) => {
                c.insert(id, p);
            }
            None => {
                c.remove(&id);
            }
        }
    });
}

/// The pointer capture target override of `id` (the target of its events), if any.
pub fn capture_target(id: i32) -> Option<usize> {
    CURRENT.with(|c| c.borrow().get(&id).copied())
}

/// The mouse's current buttons bitmask.
pub fn buttons() -> u16 {
    BUTTONS.with(Cell::get)
}

/// A button went down on pointer `id` (the engine's mouse): the active buttons state; a touch pointer is
/// implicitly captured to the `pointerdown` target before its listeners run (§10.4).
pub fn down(id: i32, kind: Kind, target: usize) {
    if id == MOUSE_ID {
        BUTTONS.with(|b| b.set(1));
    }
    if kind == Kind::Touch {
        PENDING.with(|p| p.borrow_mut().insert(id, target));
    }
}

/// The button went up: no active buttons.
pub fn up(id: i32) {
    if id == MOUSE_ID {
        BUTTONS.with(|b| b.set(0));
    }
}

/// §10.3 implicit release: after `pointerup` / `pointercancel`, clear the pending override and process it
/// (`lostpointercapture`).
pub fn implicit_release(vm: &mut Vm, id: i32, client: (f64, f64)) {
    PENDING.with(|p| p.borrow_mut().remove(&id));
    process_pending(vm, id, client);
}

pub fn set_prevent_mouse(on: bool) {
    PREVENT_MOUSE.with(|p| p.set(on));
}

pub fn prevent_mouse() -> bool {
    PREVENT_MOUSE.with(Cell::get)
}

/// Installs the three Element methods.
pub fn install(vm: &mut Vm) {
    let p = dom::proto("Element");
    op(vm, p, "setPointerCapture", 1, set_pointer_capture);
    op(vm, p, "releasePointerCapture", 1, release_pointer_capture);
    op(vm, p, "hasPointerCapture", 1, has_pointer_capture);
}
