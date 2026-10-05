//! DOM §2 events: EventTarget, Event and its subclasses, the dispatch algorithm (capture, target and
//! bubble phases over the node tree, the Document and the Window), event handler attributes and IDL
//! attributes (HTML §8.1.8), and HTML's "report the exception" (§8.1.4.6).
//!
//! An Event is a platform object whose slots are its type, its flags, target, currentTarget, phase,
//! timeStamp, the subclass's initialized dictionary members and the dispatch path; `isTrusted` is the
//! [LegacyUnforgeable] own accessor the spec puts on every instance.

use super::dom::{self, node_of, parent_of, wrap};
use super::idl::{self, *};
use super::{page, Listener, TargetKey};
use js_core::vm::*;

// Event slots.
const E_TYPE: usize = 0;
const E_FLAGS: usize = 1;
const E_TARGET: usize = 2;
const E_CURRENT: usize = 3;
const E_PHASE: usize = 4;
const E_STAMP: usize = 5;
const E_INIT: usize = 6;
const E_PATH: usize = 7;

const F_BUBBLES: u32 = 1;
const F_CANCELABLE: u32 = 2;
const F_COMPOSED: u32 = 4;
const F_STOP: u32 = 8;
const F_STOP_IMMEDIATE: u32 = 16;
const F_CANCELED: u32 = 32;
const F_PASSIVE: u32 = 64;
const F_DISPATCH: u32 = 128;
const F_INITIALIZED: u32 = 256;
const F_TRUSTED: u32 = 512;

thread_local! {
    static IS_TRUSTED_GETTER: std::cell::Cell<Option<Obj>> = const { std::cell::Cell::new(None) };
}

fn flags(vm: &Vm, e: Obj) -> u32 {
    slot_num(vm, e, E_FLAGS) as u32
}

fn set_flag(vm: &mut Vm, e: Obj, f: u32, on: bool) {
    let cur = flags(vm, e);
    let v = if on { cur | f } else { cur & !f };
    set_slot(vm, e, E_FLAGS, num(v as f64));
}

/// Milliseconds since the page's time origin (DOMHighResTimeStamp).
pub fn now_hr() -> f64 {
    let origin = page(|p| p.time_origin);
    crate::event_loop::now_ms().saturating_sub(origin) as f64
}

// =================================================================================================
// Targets
// =================================================================================================

/// The event target behind a JS value (`undefined`/`null` this is the global, as WebIDL says for
/// operations called on [Global] interfaces).
pub fn target_of(vm: &Vm, v: &Value) -> Option<TargetKey> {
    match v {
        Value::Undefined | Value::Null => Some(TargetKey::Window),
        Value::Object(o) => {
            if *o == vm.realm().global {
                return Some(TargetKey::Window);
            }
            if let Some(n) = node_of(vm, v) {
                return Some(TargetKey::Node(n));
            }
            if tag_of(vm, *o) == Some(T_ETARGET) {
                return Some(TargetKey::Other(slot_num(vm, *o, 0) as u32));
            }
            None
        }
        _ => None,
    }
}

pub fn target_value(vm: &mut Vm, t: TargetKey) -> Value {
    match t {
        TargetKey::Node(n) => wrap(vm, n),
        TargetKey::Window => Value::Object(vm.realm().global),
        TargetKey::Other(id) => page(|p| p.other_targets.get(&id).map(|(o, _)| Value::Object(*o))).unwrap_or(Value::Null),
    }
}

fn this_target(vm: &mut Vm, ctx: &CallCtx) -> JsResult<TargetKey> {
    match target_of(vm, &ctx.this) {
        Some(t) => Ok(t),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn event_target_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'EventTarget': Please use the 'new' operator");
    }
    let id = page(|p| {
        p.target_serial += 1;
        p.target_serial
    });
    let default = dom::iface("EventTarget").unwrap().proto;
    let proto = proto_from_new_target(vm, &ctx.new_target, default)?;
    let o = host_obj(vm, proto, T_ETARGET, vec![num(id as f64)]);
    let r = root(vm, Value::Object(o));
    page(|p| p.other_targets.insert(id, (o, r)));
    Ok(Value::Object(o))
}

// =================================================================================================
// Listeners
// =================================================================================================

/// Flattens `options` (boolean or dictionary) into (capture, once, passive, signal).
fn flatten_options(vm: &mut Vm, v: &Value) -> JsResult<(bool, bool, bool, Value)> {
    match v {
        Value::Object(_) => {
            let c = dict_member(vm, v, "capture")?;
            let o = dict_member(vm, v, "once")?;
            let p = dict_member(vm, v, "passive")?;
            let sig = dict_member(vm, v, "signal")?;
            Ok((vm.to_boolean(&c), vm.to_boolean(&o), vm.to_boolean(&p), sig))
        }
        other => Ok((vm.to_boolean(other), false, false, Value::Undefined)),
    }
}

fn same_callback(vm: &Vm, l: &Listener, cb: &Value) -> bool {
    !l.handler && idl::rooted(vm, l.callback).same_value(cb)
}

/// DOM "add an event listener".
pub fn add_listener(vm: &mut Vm, t: TargetKey, ty: &str, cb: Value, capture: bool, once: bool, passive: bool) -> bool {
    let exists = {
        let list = page(|p| p.listeners.get(&t).cloned().unwrap_or_default());
        list.iter().any(|l| !l.removed && l.ty == ty && l.capture == capture && same_callback(vm, l, &cb))
    };
    if exists {
        return false;
    }
    let r = root(vm, cb);
    page(|p| {
        p.listener_serial += 1;
        let serial = p.listener_serial;
        p.listeners.entry(t).or_default().push(Listener {
            ty: ty.to_string(),
            callback: r,
            capture,
            once,
            passive,
            removed: false,
            handler: false,
            serial,
        });
    });
    true
}

pub fn remove_listener(vm: &mut Vm, t: TargetKey, ty: &str, cb: &Value, capture: bool) {
    let list = page(|p| p.listeners.get(&t).cloned().unwrap_or_default());
    let Some(pos) = list.iter().position(|l| !l.removed && l.ty == ty && l.capture == capture && same_callback(vm, l, cb)) else {
        return;
    };
    let serial = list[pos].serial;
    let r = list[pos].callback;
    page(|p| {
        if let Some(ls) = p.listeners.get_mut(&t) {
            if let Some(l) = ls.iter_mut().find(|l| l.serial == serial) {
                l.removed = true;
            }
            ls.retain(|l| l.serial != serial);
        }
    });
    unroot(vm, r);
}

fn add_event_listener(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_target(vm, ctx)?;
    need(vm, ctx, 2, "EventTarget", "addEventListener")?;
    let ty = string(vm, &arg(vm, ctx, 0))?;
    let cb = arg(vm, ctx, 1);
    let opts = arg(vm, ctx, 2);
    let (capture, once, passive, signal) = flatten_options(vm, &opts)?;
    if cb.is_null() {
        return Ok(Value::Undefined);
    }
    if !cb.is_object() {
        return vm.throw_type("Failed to execute 'addEventListener' on 'EventTarget': parameter 2 is not of type 'Object'.");
    }
    if let TargetKey::Node(n) = t {
        ensure_attr_handlers(vm, n);
    }
    if let Value::Object(sig) = &signal {
        let aborted = vm.get(*sig, &PropertyKey::from_str("aborted"))?;
        if vm.to_boolean(&aborted) {
            return Ok(Value::Undefined);
        }
    }
    let added = add_listener(vm, t, &ty, cb.clone(), capture, once, passive);
    if added {
        if let Value::Object(sig) = signal {
            // The abort steps remove this listener.
            let tv = target_value(vm, t);
            let fp = vm.intr().function_proto;
            let rm = vm.make_native_with("", 0, abort_remove, false, Some(fp), vec![tv, s(&ty), cb, Value::Bool(capture)]);
            let add = vm.get(sig, &PropertyKey::from_str("addEventListener"))?;
            if vm.is_callable(&add) {
                vm.call(&add, &Value::Object(sig), &[s("abort"), Value::Object(rm)])?;
            }
        }
    }
    Ok(Value::Undefined)
}

fn abort_remove(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let tv = vm.native_slot(ctx.callee, 0);
    let ty = vm.native_slot(ctx.callee, 1);
    let cb = vm.native_slot(ctx.callee, 2);
    let cap = vm.native_slot(ctx.callee, 3);
    if let (Some(t), Value::String(ty)) = (target_of(vm, &tv), ty) {
        let capture = vm.to_boolean(&cap);
        remove_listener(vm, t, &ty.to_rust(), &cb, capture);
    }
    Ok(Value::Undefined)
}

fn remove_event_listener(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_target(vm, ctx)?;
    need(vm, ctx, 2, "EventTarget", "removeEventListener")?;
    let ty = string(vm, &arg(vm, ctx, 0))?;
    let cb = arg(vm, ctx, 1);
    let opts = arg(vm, ctx, 2);
    let capture = match &opts {
        Value::Object(_) => {
            let c = dict_member(vm, &opts, "capture")?;
            vm.to_boolean(&c)
        }
        other => vm.to_boolean(other),
    };
    if cb.is_object() {
        remove_listener(vm, t, &ty, &cb, capture);
    }
    Ok(Value::Undefined)
}

fn dispatch_event_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = this_target(vm, ctx)?;
    need(vm, ctx, 1, "EventTarget", "dispatchEvent")?;
    let ev = arg(vm, ctx, 0);
    let Some(e) = this_tagged(vm, &ev, T_EVENT) else {
        return vm.throw_type("Failed to execute 'dispatchEvent' on 'EventTarget': parameter 1 is not of type 'Event'.");
    };
    let f = flags(vm, e);
    if f & F_DISPATCH != 0 || f & F_INITIALIZED == 0 {
        return throw_dom(vm, "InvalidStateError", "The event is already being dispatched or was not initialized.");
    }
    set_flag(vm, e, F_TRUSTED, false);
    let (ok, _) = dispatch(vm, e, t);
    Ok(Value::Bool(ok))
}

// =================================================================================================
// Event handlers (HTML §8.1.8)
// =================================================================================================

/// Event types whose handlers on `<body>`/`<frameset>` reflect the Window's (HTML §8.1.8.2).
pub fn is_window_reflecting(ty: &str) -> bool {
    matches!(
        ty,
        "blur" | "error" | "focus" | "load" | "resize" | "scroll" | "afterprint" | "beforeprint" | "beforeunload"
            | "hashchange" | "languagechange" | "message" | "messageerror" | "offline" | "online" | "pagehide"
            | "pageshow" | "popstate" | "rejectionhandled" | "storage" | "unhandledrejection" | "unload"
    )
}

/// The target whose handler an `on<ty>` attribute/property of node `el` sets.
fn handler_target(el: usize, ty: &str) -> TargetKey {
    if is_window_reflecting(ty) && dom::with_doc(|d| dom::is_html_tag(d, el, "body") || dom::is_html_tag(d, el, "frameset")) {
        if dom::node_document(el) == dom::main_doc() {
            return TargetKey::Window;
        }
    }
    TargetKey::Node(el)
}

/// Stores a handler value (function, uncompiled body string, or null) and registers the handler's
/// listener the first time it becomes non-null (HTML "set an event handler").
fn set_handler(vm: &mut Vm, t: TargetKey, ty: &str, v: Value) {
    let key = (t, ty.to_string());
    let non_null = !v.is_null();
    let existing = page(|p| p.handlers.get(&key).copied());
    match existing {
        Some(r) => vm.host_roots[r] = v,
        None => {
            let r = root(vm, v);
            page(|p| p.handlers.insert(key.clone(), r));
        }
    }
    if !non_null {
        // HTML "deactivate an event handler": its listener leaves the list (a later activation
        // registers it again, at the end).
        page(|p| {
            if let Some(ls) = p.listeners.get_mut(&t) {
                ls.retain(|l| !(l.handler && l.ty == ty));
            }
        });
    }
    if non_null {
        let registered = page(|p| p.listeners.get(&t).is_some_and(|ls| ls.iter().any(|l| l.handler && l.ty == ty)));
        if !registered {
            page(|p| {
                p.listener_serial += 1;
                let serial = p.listener_serial;
                p.listeners.entry(t).or_default().push(Listener {
                    ty: ty.to_string(),
                    callback: usize::MAX,
                    capture: false,
                    once: false,
                    passive: false,
                    removed: false,
                    handler: true,
                    serial,
                });
            });
        }
    }
}

/// An `on*` content attribute of `el` was set or removed (attribute change steps).
pub fn handler_attribute_changed(vm: &mut Vm, el: usize, ty: &str, present: bool) {
    let t = handler_target(el, ty);
    let v = if present {
        match dom::attr_value(el, &format!("on{ty}")) {
            Some(body) => s(&body),
            None => Value::Null,
        }
    } else {
        Value::Null
    };
    set_handler(vm, t, ty, v);
}

/// Registers handlers for `on*` attributes the parser put on `el` that no script has touched yet, so
/// they run in the order the spec gives them (registered when the attribute was set: before any
/// listener a later script adds).
pub fn ensure_attr_handlers(vm: &mut Vm, el: usize) {
    let names: Vec<String> = dom::with_doc(|d| {
        d.element(html_core::NodeId(el))
            .map(|e| {
                e.attrs
                    .iter()
                    .filter(|a| a.ns == html_core::Namespace::None && a.local.starts_with("on") && a.local.len() > 2)
                    .map(|a| a.local[2..].to_string())
                    .collect()
            })
            .unwrap_or_default()
    });
    for ty in names {
        let t = handler_target(el, &ty);
        let known = page(|p| p.handlers.contains_key(&(t, ty.clone())));
        if !known {
            handler_attribute_changed(vm, el, &ty, true);
        }
    }
}

/// "get the current value of the event handler": compiles an uncompiled body (as a function of
/// `event`, or for a Window `onerror` of `(event, source, lineno, colno, error)`).
fn handler_value(vm: &mut Vm, t: TargetKey, ty: &str) -> Value {
    let Some(r) = page(|p| p.handlers.get(&(t, ty.to_string())).copied()) else { return Value::Null };
    let v = idl::rooted(vm, r);
    if let Value::String(body) = &v {
        let params = if t == TargetKey::Window && ty == "error" { "event, source, lineno, colno, error" } else { "event" };
        let fc = vm.intr().function_ctor;
        let compiled = super::enter(vm, "handler-compile", |vm| vm.construct(&Value::Object(fc), &[s(params), Value::String(body.clone())], None));
        let f = match compiled {
            Some(Ok(f)) => f,
            Some(Err(e)) => {
                report_exception(vm, &e);
                Value::Null
            }
            None => Value::Null,
        };
        crate::ledger::record_dom("inline-handler-scope-chain-approximated");
        // The internal raw uncompiled handler's function is named for the handler ("onclick").
        if let Value::Object(fo) = &f {
            let name = format!("on{ty}");
            vm.heap.get_mut(*fo).props.insert(PropertyKey::from_str("name"), Prop::data(s(&name), C));
        }
        vm.host_roots[r] = f.clone();
        return f;
    }
    v
}

fn handler_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let ty = callee_str(vm, ctx);
    let Some(t) = target_of(vm, &ctx.this) else { return vm.throw_type("Illegal invocation") };
    let t = match t {
        TargetKey::Node(n) => {
            ensure_attr_handlers(vm, n);
            handler_target(n, &ty)
        }
        other => other,
    };
    if t == TargetKey::Window {
        if let Some(b) = dom::body_of(dom::main_doc()) {
            ensure_attr_handlers(vm, b);
        }
    }
    Ok(handler_value(vm, t, &ty))
}

fn handler_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let ty = callee_str(vm, ctx);
    let Some(t) = target_of(vm, &ctx.this) else { return vm.throw_type("Illegal invocation") };
    let t = match t {
        TargetKey::Node(n) => {
            ensure_attr_handlers(vm, n);
            handler_target(n, &ty)
        }
        other => other,
    };
    let v = arg(vm, ctx, 0);
    // [LegacyTreatNonObjectAsNull]
    let v = if v.is_object() { v } else { Value::Null };
    set_handler(vm, t, &ty, v);
    Ok(Value::Undefined)
}

/// The GlobalEventHandlers names (HTML §8.1.8.2) installed on HTMLElement, Document and Window.
pub const GLOBAL_HANDLERS: &[&str] = &[
    "abort", "auxclick", "beforeinput", "beforematch", "beforetoggle", "blur", "cancel", "canplay", "canplaythrough",
    "change", "click", "close", "contextlost", "contextmenu", "contextrestored", "copy", "cuechange", "cut", "dblclick",
    "drag", "dragend", "dragenter", "dragleave", "dragover", "dragstart", "drop", "durationchange", "emptied", "ended",
    "error", "focus", "formdata", "input", "invalid", "keydown", "keypress", "keyup", "load", "loadeddata",
    "loadedmetadata", "loadstart", "mousedown", "mouseenter", "mouseleave", "mousemove", "mouseout", "mouseover",
    "mouseup", "paste", "pause", "play", "playing", "progress", "ratechange", "reset", "resize", "scroll",
    "scrollend", "securitypolicyviolation", "seeked", "seeking", "select", "slotchange", "stalled", "submit",
    "suspend", "timeupdate", "toggle", "volumechange", "waiting", "wheel", "pointerdown", "pointerup", "pointermove",
    "pointerover", "pointerout", "pointerenter", "pointerleave", "pointercancel", "gotpointercapture",
    "lostpointercapture", "animationstart", "animationend", "animationiteration", "transitionend", "transitionstart",
    "transitionrun", "transitioncancel", "touchstart", "touchend", "touchmove", "touchcancel",
];

pub const WINDOW_HANDLERS: &[&str] = &[
    "afterprint", "beforeprint", "beforeunload", "hashchange", "languagechange", "message", "messageerror",
    "offline", "online", "pagehide", "pageshow", "popstate", "rejectionhandled", "storage", "unhandledrejection",
    "unload",
];

pub fn install_handlers(vm: &mut Vm, o: Obj, names: &[&str]) {
    for n in names {
        idl::attr_with(vm, o, &format!("on{n}"), handler_get, Some(handler_set), s(n));
    }
}

// =================================================================================================
// Dispatch (DOM §2.9)
// =================================================================================================

/// The "get the parent" chain of a target: node ancestors, then the Window above the main Document
/// (except for `load`, which does not propagate from the document to the window).
fn event_path(t: TargetKey, ty: &str) -> Vec<TargetKey> {
    let mut path = vec![t];
    if let TargetKey::Node(n) = t {
        let mut cur = n;
        while let Some(p) = parent_of(cur) {
            path.push(TargetKey::Node(p));
            cur = p;
        }
        if dom::kind(cur) == dom::NK::Document && cur == dom::main_doc() && ty != "load" {
            path.push(TargetKey::Window);
        }
    }
    path
}

/// DOM "dispatch". Returns (not canceled, whether any listener or handler ran).
pub fn dispatch(vm: &mut Vm, e: Obj, t: TargetKey) -> (bool, bool) {
    let ty = match slot(vm, e, E_TYPE) {
        Value::String(s) => s.to_rust(),
        _ => String::new(),
    };
    set_flag(vm, e, F_DISPATCH, true);
    let tv = target_value(vm, t);
    set_slot(vm, e, E_TARGET, tv);
    let path = event_path(t, &ty);
    let path_vals: Vec<Value> = path.iter().map(|k| target_value(vm, *k)).collect();
    let arr = vm.new_array(path_vals);
    set_slot(vm, e, E_PATH, Value::Object(arr));
    // Parser-set handlers on the path register now (first touch).
    for k in &path {
        match k {
            TargetKey::Node(n) => ensure_attr_handlers(vm, *n),
            TargetKey::Window => {
                if let Some(b) = dom::body_of(dom::main_doc()) {
                    ensure_attr_handlers(vm, b);
                }
            }
            _ => {}
        }
    }
    let mut ran = false;
    let bubbles = flags(vm, e) & F_BUBBLES != 0;
    for (i, k) in path.iter().enumerate().rev() {
        let phase = if i == 0 { 2.0 } else { 1.0 };
        set_slot(vm, e, E_PHASE, num(phase));
        ran |= invoke_listeners(vm, e, *k, &ty, true);
    }
    for (i, k) in path.iter().enumerate() {
        if i == 0 {
            set_slot(vm, e, E_PHASE, num(2.0));
        } else {
            if !bubbles {
                continue;
            }
            set_slot(vm, e, E_PHASE, num(3.0));
        }
        ran |= invoke_listeners(vm, e, *k, &ty, false);
    }
    set_slot(vm, e, E_PHASE, num(0.0));
    set_slot(vm, e, E_CURRENT, Value::Null);
    set_slot(vm, e, E_PATH, Value::Undefined);
    set_flag(vm, e, F_DISPATCH, false);
    set_flag(vm, e, F_STOP, false);
    set_flag(vm, e, F_STOP_IMMEDIATE, false);
    (flags(vm, e) & F_CANCELED == 0, ran)
}

/// DOM "invoke" + "inner invoke" for one path entry and phase (`capturing` = the capture pass).
fn invoke_listeners(vm: &mut Vm, e: Obj, t: TargetKey, ty: &str, capturing: bool) -> bool {
    if flags(vm, e) & F_STOP != 0 {
        return false;
    }
    let cur = target_value(vm, t);
    set_slot(vm, e, E_CURRENT, cur.clone());
    let listeners: Vec<Listener> = page(|p| p.listeners.get(&t).cloned().unwrap_or_default());
    let mut ran = false;
    for l in listeners {
        if l.ty != ty {
            continue;
        }
        // Skip what was removed by an earlier listener of this dispatch.
        let live = page(|p| p.listeners.get(&t).is_some_and(|ls| ls.iter().any(|x| x.serial == l.serial && !x.removed)));
        if !live {
            continue;
        }
        let phase = slot_num(vm, e, E_PHASE);
        if capturing && !l.capture {
            continue;
        }
        if !capturing && l.capture {
            continue;
        }
        let _ = phase;
        if l.once {
            let r = l.callback;
            page(|p| {
                if let Some(ls) = p.listeners.get_mut(&t) {
                    ls.retain(|x| x.serial != l.serial);
                }
            });
            // keep the callback alive for this call; unroot after
            let cb = idl::rooted(vm, r);
            call_listener(vm, e, &cur, &cb, l.passive, t, ty, false);
            unroot(vm, r);
            ran = true;
        } else if l.handler {
            let h = handler_value(vm, t, ty);
            if vm.is_callable(&h) {
                call_listener(vm, e, &cur, &h, l.passive, t, ty, true);
                ran = true;
            }
        } else {
            let cb = idl::rooted(vm, l.callback);
            call_listener(vm, e, &cur, &cb, l.passive, t, ty, false);
            ran = true;
        }
        if flags(vm, e) & F_STOP_IMMEDIATE != 0 {
            break;
        }
    }
    ran
}

#[allow(clippy::too_many_arguments)]
fn call_listener(vm: &mut Vm, e: Obj, this: &Value, cb: &Value, passive: bool, t: TargetKey, ty: &str, is_handler: bool) {
    if passive {
        set_flag(vm, e, F_PASSIVE, true);
    }
    let ev = Value::Object(e);
    let result = if is_handler && t == TargetKey::Window && ty == "error" && is_error_event(vm, e) {
        // The Window `onerror` special signature.
        let init = slot(vm, e, E_INIT);
        let msg = dict_member(vm, &init, "message").unwrap_or(Value::Undefined);
        let file = dict_member(vm, &init, "filename").unwrap_or(Value::Undefined);
        let line = dict_member(vm, &init, "lineno").unwrap_or(Value::Undefined);
        let col = dict_member(vm, &init, "colno").unwrap_or(Value::Undefined);
        let err = dict_member(vm, &init, "error").unwrap_or(Value::Undefined);
        super::invoke(vm, "event-handler", cb, this, &[msg, file, line, col, err])
    } else if vm.is_callable(cb) {
        super::invoke(vm, "event-listener", cb, this, &[ev.clone()])
    } else if let Value::Object(o) = cb {
        // A callback interface object: its handleEvent.
        let he = super::enter(vm, "handleEvent", |vm| vm.get(*o, &PropertyKey::from_str("handleEvent")));
        match he {
            Some(Ok(f)) if vm.is_callable(&f) => super::invoke(vm, "event-listener", &f, cb, &[ev.clone()]),
            Some(Ok(_)) => {
                let err = vm.type_error("handleEvent is not a function");
                report_exception(vm, &err);
                None
            }
            Some(Err(err)) => {
                report_exception(vm, &err);
                None
            }
            None => None,
        }
    } else {
        None
    };
    if passive {
        set_flag(vm, e, F_PASSIVE, false);
    }
    // HTML "the event handler processing algorithm": a false return cancels; for Window onerror a
    // true return does.
    if is_handler {
        if let Some(r) = result {
            let cancel = if t == TargetKey::Window && ty == "error" { matches!(r, Value::Bool(true)) } else { matches!(r, Value::Bool(false)) };
            if cancel {
                cancel_event(vm, e);
            }
        }
    }
}

fn cancel_event(vm: &mut Vm, e: Obj) {
    let f = flags(vm, e);
    if f & F_CANCELABLE != 0 && f & F_PASSIVE == 0 {
        set_flag(vm, e, F_CANCELED, true);
    }
}

fn is_error_event(vm: &Vm, e: Obj) -> bool {
    let p = vm.heap.get(e).proto;
    p.is_some() && p == dom::iface("ErrorEvent").map(|i| i.proto)
}

/// Creates a trusted event of interface `iface` (subclass members from `init`).
pub fn new_event(vm: &mut Vm, iface: &str, ty: &str, bubbles: bool, cancelable: bool, init: Option<Obj>) -> Obj {
    let p = dom::iface(iface).or_else(|| dom::iface("Event")).unwrap().proto;
    let mut f = F_INITIALIZED | F_TRUSTED;
    if bubbles {
        f |= F_BUBBLES;
    }
    if cancelable {
        f |= F_CANCELABLE;
    }
    let initv = init.map(Value::Object).unwrap_or(Value::Undefined);
    let e = host_obj(vm, p, T_EVENT, vec![s(ty), num(f as f64), Value::Null, Value::Null, num(0.0), num(now_hr()), initv, Value::Undefined]);
    if let Some(g) = IS_TRUSTED_GETTER.with(|g| g.get()) {
        unforgeable_attr(vm, e, "isTrusted", g);
    }
    e
}

/// Fires a simple trusted event (HTML "fire an event"). Returns whether any listener ran.
pub fn fire_simple(vm: &mut Vm, t: TargetKey, ty: &str, bubbles: bool, cancelable: bool) -> bool {
    if super::engine_poisoned() {
        return false;
    }
    let e = new_event(vm, "Event", ty, bubbles, cancelable, None);
    let r = root(vm, Value::Object(e));
    let (_, ran) = dispatch(vm, e, t);
    unroot(vm, r);
    ran
}

/// A trusted user `click` at element `n` (pointer activation): MouseEvent, with activation behavior.
pub fn user_click(vm: &mut Vm, n: usize) -> bool {
    let (_, ran) = click_with_activation(vm, n, true);
    ran
}

/// `HTMLElement.click()` (HTML §6.5.1 "fire a synthetic pointer event", untrusted).
pub fn synthetic_click(vm: &mut Vm, n: usize) {
    click_with_activation(vm, n, false);
}

/// Fires `click` at `n` with the checkbox/radio legacy-pre-activation behavior and the `input`/`change`
/// events that follow (HTML §4.10.5.1.15/16). Returns (not canceled, ran).
fn click_with_activation(vm: &mut Vm, n: usize, trusted: bool) -> (bool, bool) {
    let init = vm.new_plain_object();
    let _ = vm.create_data_property(init, PropertyKey::from_str("detail"), num(1.0));
    let e = new_event(vm, "MouseEvent", "click", true, true, Some(init));
    set_flag(vm, e, F_COMPOSED, true);
    if !trusted {
        set_flag(vm, e, F_TRUSTED, false);
    }
    let r = root(vm, Value::Object(e));
    let checkable = super::html::checkable_type(n);
    let old = checkable.map(|_| super::html::checkedness(n));
    if let Some(kind) = checkable {
        if kind == "checkbox" {
            super::html::set_checkedness(vm, n, !old.unwrap_or(false));
        } else {
            super::html::set_checkedness(vm, n, true);
        }
    }
    let (ok, ran) = dispatch(vm, e, TargetKey::Node(n));
    unroot(vm, r);
    if let Some(prev) = old {
        if !ok {
            super::html::set_checkedness(vm, n, prev);
        } else if super::html::checkedness(n) != prev {
            fire_simple(vm, TargetKey::Node(n), "input", true, false);
            fire_simple(vm, TargetKey::Node(n), "change", true, false);
        }
    }
    page(|p| p.last_click_canceled = !ok);
    (ok, ran)
}

// =================================================================================================
// Event interfaces
// =================================================================================================

fn this_event(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Obj> {
    match this_tagged(vm, &ctx.this, T_EVENT) {
        Some(o) => Ok(o),
        None => vm.throw_type("Illegal invocation"),
    }
}

/// Converts one dictionary member per its declared kind (`b` boolean, `n` double, `l` long, `s`
/// DOMString, `a` any, `o` object-or-null).
fn convert_member(vm: &mut Vm, v: Value, kind: char) -> JsResult<Value> {
    Ok(match kind {
        'b' => Value::Bool(vm.to_boolean(&v)),
        'n' => {
            if v.is_undefined() {
                num(0.0)
            } else {
                num(vm.to_number(&v)?)
            }
        }
        'l' => {
            if v.is_undefined() {
                num(0.0)
            } else {
                num(vm.to_int32(&v)? as f64)
            }
        }
        's' => {
            if v.is_undefined() {
                s("")
            } else {
                Value::String(vm.to_string(&v)?)
            }
        }
        'o' => {
            if v.is_undefined() {
                Value::Null
            } else {
                v
            }
        }
        _ => {
            if v.is_undefined() {
                Value::Null
            } else {
                v
            }
        }
    })
}

/// Dictionary members per event interface (name, kind), parents' included at construction.
fn members(iface: &str) -> &'static [(&'static str, char)] {
    match iface {
        "CustomEvent" => &[("detail", 'a')],
        "UIEvent" => &[("view", 'o'), ("detail", 'l'), ("which", 'l')],
        "MouseEvent" | "WheelEvent" | "PointerEvent" | "DragEvent" => &[
            ("screenX", 'n'),
            ("screenY", 'n'),
            ("clientX", 'n'),
            ("clientY", 'n'),
            ("button", 'l'),
            ("buttons", 'l'),
            ("relatedTarget", 'o'),
            ("ctrlKey", 'b'),
            ("shiftKey", 'b'),
            ("altKey", 'b'),
            ("metaKey", 'b'),
            ("movementX", 'n'),
            ("movementY", 'n'),
        ],
        "KeyboardEvent" => &[
            ("key", 's'),
            ("code", 's'),
            ("location", 'l'),
            ("repeat", 'b'),
            ("isComposing", 'b'),
            ("ctrlKey", 'b'),
            ("shiftKey", 'b'),
            ("altKey", 'b'),
            ("metaKey", 'b'),
            ("charCode", 'l'),
            ("keyCode", 'l'),
        ],
        "FocusEvent" => &[("relatedTarget", 'o')],
        "InputEvent" => &[("data", 'a'), ("inputType", 's'), ("isComposing", 'b')],
        "ErrorEvent" => &[("message", 's'), ("filename", 's'), ("lineno", 'l'), ("colno", 'l'), ("error", 'a')],
        "PromiseRejectionEvent" => &[("promise", 'a'), ("reason", 'a')],
        "PopStateEvent" => &[("state", 'a')],
        "HashChangeEvent" => &[("oldURL", 's'), ("newURL", 's')],
        "ProgressEvent" => &[("lengthComputable", 'b'), ("loaded", 'n'), ("total", 'n')],
        "MessageEvent" => &[("data", 'a'), ("origin", 's'), ("lastEventId", 's'), ("source", 'o')],
        "PageTransitionEvent" => &[("persisted", 'b')],
        "SubmitEvent" => &[("submitter", 'o')],
        "AnimationEvent" => &[("animationName", 's'), ("elapsedTime", 'n'), ("pseudoElement", 's')],
        "TransitionEvent" => &[("propertyName", 's'), ("elapsedTime", 'n'), ("pseudoElement", 's')],
        "StorageEvent" => &[("key", 'a'), ("oldValue", 'a'), ("newValue", 'a'), ("url", 's'), ("storageArea", 'o')],
        "CloseEvent" => &[("wasClean", 'b'), ("code", 'l'), ("reason", 's')],
        _ => &[],
    }
}

fn parent_event(iface: &str) -> Option<&'static str> {
    Some(match iface {
        "UIEvent" | "CustomEvent" | "ErrorEvent" | "PromiseRejectionEvent" | "PopStateEvent" | "HashChangeEvent"
        | "ProgressEvent" | "MessageEvent" | "PageTransitionEvent" | "SubmitEvent" | "AnimationEvent"
        | "TransitionEvent" | "StorageEvent" | "CloseEvent" => "Event",
        "MouseEvent" | "KeyboardEvent" | "FocusEvent" | "InputEvent" => "UIEvent",
        "WheelEvent" | "PointerEvent" | "DragEvent" => "MouseEvent",
        _ => return None,
    })
}

fn event_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let iface = callee_str(vm, ctx);
    let iface = if iface.is_empty() { "Event".to_string() } else { iface };
    if ctx.new_target.is_undefined() {
        return vm.throw_type(&format!("Failed to construct '{iface}': Please use the 'new' operator"));
    }
    need(vm, ctx, 1, &iface, "constructor")?;
    let ty = string(vm, &arg(vm, ctx, 0))?;
    let dict = arg(vm, ctx, 1);
    let b = dict_member(vm, &dict, "bubbles")?;
    let c = dict_member(vm, &dict, "cancelable")?;
    let comp = dict_member(vm, &dict, "composed")?;
    let mut f = F_INITIALIZED;
    if vm.to_boolean(&b) {
        f |= F_BUBBLES;
    }
    if vm.to_boolean(&c) {
        f |= F_CANCELABLE;
    }
    if vm.to_boolean(&comp) {
        f |= F_COMPOSED;
    }
    // Subclass members, the most-derived interface's chain.
    let init = vm.new_plain_object();
    let mut cur: Option<&str> = Some(iface.as_str());
    let mut chain: Vec<String> = Vec::new();
    while let Some(i) = cur {
        chain.push(i.to_string());
        cur = parent_event(i);
    }
    for i in chain.iter().rev() {
        for (m, k) in members(i) {
            let v = dict_member(vm, &dict, m)?;
            let cv = convert_member(vm, v, *k)?;
            vm.create_data_property(init, PropertyKey::from_str(m), cv)?;
        }
    }
    let default = dom::iface(&iface).unwrap().proto;
    let proto = proto_from_new_target(vm, &ctx.new_target, default)?;
    let e = host_obj(vm, proto, T_EVENT, vec![s(&ty), num(f as f64), Value::Null, Value::Null, num(0.0), num(now_hr()), Value::Object(init), Value::Undefined]);
    if let Some(g) = IS_TRUSTED_GETTER.with(|g| g.get()) {
        unforgeable_attr(vm, e, "isTrusted", g);
    }
    Ok(Value::Object(e))
}

fn ev_type(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    Ok(slot(vm, e, E_TYPE))
}
fn ev_target(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    Ok(slot(vm, e, E_TARGET))
}
fn ev_current_target(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    Ok(slot(vm, e, E_CURRENT))
}
fn ev_phase(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    Ok(slot(vm, e, E_PHASE))
}
fn ev_flag(vm: &mut Vm, ctx: &CallCtx, f: u32) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    Ok(Value::Bool(flags(vm, e) & f != 0))
}
fn ev_bubbles(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ev_flag(vm, ctx, F_BUBBLES)
}
fn ev_cancelable(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ev_flag(vm, ctx, F_CANCELABLE)
}
fn ev_composed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ev_flag(vm, ctx, F_COMPOSED)
}
fn ev_default_prevented(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ev_flag(vm, ctx, F_CANCELED)
}
fn ev_is_trusted(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ev_flag(vm, ctx, F_TRUSTED)
}
fn ev_timestamp(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    Ok(slot(vm, e, E_STAMP))
}
fn ev_return_value_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    Ok(Value::Bool(flags(vm, e) & F_CANCELED == 0))
}
fn ev_return_value_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    if !vm.to_boolean(&arg(vm, ctx, 0)) {
        cancel_event(vm, e);
    }
    Ok(Value::Undefined)
}
fn ev_cancel_bubble_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ev_flag(vm, ctx, F_STOP)
}
fn ev_cancel_bubble_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    if vm.to_boolean(&arg(vm, ctx, 0)) {
        set_flag(vm, e, F_STOP, true);
    }
    Ok(Value::Undefined)
}
fn ev_stop_propagation(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    set_flag(vm, e, F_STOP, true);
    Ok(Value::Undefined)
}
fn ev_stop_immediate(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    set_flag(vm, e, F_STOP | F_STOP_IMMEDIATE, true);
    Ok(Value::Undefined)
}
fn ev_prevent_default(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    cancel_event(vm, e);
    Ok(Value::Undefined)
}
fn ev_composed_path(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    match slot(vm, e, E_PATH) {
        Value::Object(a) => {
            let list = vm.array_to_list(&Value::Object(a));
            Ok(Value::Object(vm.new_array(list)))
        }
        _ => Ok(Value::Object(vm.new_array(Vec::new()))),
    }
}
fn ev_init_event(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    need(vm, ctx, 1, "Event", "initEvent")?;
    if flags(vm, e) & F_DISPATCH != 0 {
        return Ok(Value::Undefined);
    }
    let ty = string(vm, &arg(vm, ctx, 0))?;
    let b = vm.to_boolean(&arg(vm, ctx, 1));
    let c = vm.to_boolean(&arg(vm, ctx, 2));
    let mut f = flags(vm, e) & F_TRUSTED;
    f |= F_INITIALIZED;
    if b {
        f |= F_BUBBLES;
    }
    if c {
        f |= F_CANCELABLE;
    }
    set_slot(vm, e, E_FLAGS, num(f as f64));
    set_slot(vm, e, E_TYPE, s(&ty));
    set_slot(vm, e, E_TARGET, Value::Null);
    Ok(Value::Undefined)
}
fn ev_init_custom_event(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    ev_init_event(vm, ctx)?;
    let e = this_event(vm, ctx)?;
    let detail = arg(vm, ctx, 3);
    let init = match slot(vm, e, E_INIT) {
        Value::Object(o) => o,
        _ => {
            let o = vm.new_plain_object();
            set_slot(vm, e, E_INIT, Value::Object(o));
            o
        }
    };
    let detail = if detail.is_undefined() { Value::Null } else { detail };
    vm.create_data_property(init, PropertyKey::from_str("detail"), detail)?;
    Ok(Value::Undefined)
}

/// A subclass attribute: the initialized dictionary member named by the callee's slot.
fn ev_member(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    let name = callee_str(vm, ctx);
    match slot(vm, e, E_INIT) {
        Value::Object(o) => {
            let key = PropertyKey::from_str(&name);
            if vm.heap.get(o).props.get(&key).is_some() {
                return vm.get(o, &key);
            }
            Ok(default_member(&name))
        }
        _ => Ok(default_member(&name)),
    }
}

fn default_member(name: &str) -> Value {
    match name {
        "message" | "filename" | "inputType" | "code" | "oldURL" | "newURL" | "origin" | "lastEventId"
        | "animationName" | "propertyName" | "pseudoElement" | "url" | "key" => s(""),
        "lineno" | "colno" | "button" | "buttons" | "location" | "charCode" | "keyCode" | "which" | "screenX"
        | "screenY" | "clientX" | "clientY" | "movementX" | "movementY" | "loaded" | "total" | "elapsedTime" => {
            num(0.0)
        }
        "ctrlKey" | "shiftKey" | "altKey" | "metaKey" | "repeat" | "isComposing" | "lengthComputable" | "persisted"
        | "wasClean" => Value::Bool(false),
        _ => Value::Null,
    }
}

fn ev_get_modifier_state(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    let k = string(vm, &arg(vm, ctx, 0))?;
    let m = match k.as_str() {
        "Control" => "ctrlKey",
        "Shift" => "shiftKey",
        "Alt" => "altKey",
        "Meta" => "metaKey",
        _ => return Ok(Value::Bool(false)),
    };
    match slot(vm, e, E_INIT) {
        Value::Object(o) => {
            let v = vm.get(o, &PropertyKey::from_str(m))?;
            Ok(Value::Bool(vm.to_boolean(&v)))
        }
        _ => Ok(Value::Bool(false)),
    }
}

/// `document.createEvent(interface)` (DOM §4.5).
pub fn create_event(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 1, "Document", "createEvent")?;
    let name = string(vm, &arg(vm, ctx, 0))?.to_ascii_lowercase();
    let iface = match name.as_str() {
        "event" | "events" | "htmlevents" | "svgevents" => "Event",
        "customevent" => "CustomEvent",
        "uievent" | "uievents" => "UIEvent",
        "mouseevent" | "mouseevents" => "MouseEvent",
        "keyboardevent" => "KeyboardEvent",
        "focusevent" => "FocusEvent",
        "hashchangeevent" => "HashChangeEvent",
        "messageevent" => "MessageEvent",
        "storageevent" => "StorageEvent",
        "errorevent" => "ErrorEvent",
        "compositionevent" | "textevent" | "touchevent" | "dragevent" | "beforeunloadevent" | "devicemotionevent"
        | "deviceorientationevent" => "Event",
        _ => return throw_dom(vm, "NotSupportedError", &format!("The provided event type ('{name}') is invalid.")),
    };
    let p = dom::iface(iface).unwrap().proto;
    let init = vm.new_plain_object();
    let e = host_obj(vm, p, T_EVENT, vec![s(""), num(0.0), Value::Null, Value::Null, num(0.0), num(now_hr()), Value::Object(init), Value::Undefined]);
    if let Some(g) = IS_TRUSTED_GETTER.with(|g| g.get()) {
        unforgeable_attr(vm, e, "isTrusted", g);
    }
    Ok(Value::Object(e))
}

// =================================================================================================
// "report the exception" (HTML §8.1.4.6)
// =================================================================================================

/// Reports an uncaught exception: an `error` ErrorEvent at the Window (cancelable — a handler that
/// cancels it silences the console), then the console and the ledger.
pub fn report_exception(vm: &mut Vm, err: &Value) {
    if vm.terminated {
        // A budget/OOM termination is not a script exception; `enter` ledgers it.
        return;
    }
    let msg = super::enter(vm, "error-string", |vm| vm.error_string(err)).unwrap_or_else(|| "uncaught exception".into());
    let nested = page(|p| std::mem::replace(&mut p.reporting, true));
    let mut canceled = false;
    if !nested && !super::engine_poisoned() {
        let init = vm.new_plain_object();
        let url = super::page_url();
        let _ = vm.create_data_property(init, PropertyKey::from_str("message"), s(&format!("Uncaught {msg}")));
        let _ = vm.create_data_property(init, PropertyKey::from_str("filename"), s(&url));
        let _ = vm.create_data_property(init, PropertyKey::from_str("lineno"), num(0.0));
        let _ = vm.create_data_property(init, PropertyKey::from_str("colno"), num(0.0));
        let _ = vm.create_data_property(init, PropertyKey::from_str("error"), err.clone());
        let e = new_event(vm, "ErrorEvent", "error", false, true, Some(init));
        let r = root(vm, Value::Object(e));
        let (ok, _) = dispatch(vm, e, TargetKey::Window);
        unroot(vm, r);
        canceled = !ok;
    }
    if !nested {
        page(|p| p.reporting = false);
    }
    if !canceled {
        super::host::console_out(2, &format!("Uncaught {msg}"));
        crate::ledger::record_js(&format!("script-error:{}", super::clip(&msg, 64)));
        page(|p| {
            if p.errors.len() < 1000 {
                p.errors.push(msg.clone());
            }
        });
    }
}

// =================================================================================================
// Installation
// =================================================================================================

pub fn install(vm: &mut Vm) {
    let et = interface(vm, "EventTarget", None, Some((event_target_ctor, 0)));
    dom::register_iface("EventTarget", et);
    op(vm, et.proto, "addEventListener", 2, add_event_listener);
    op(vm, et.proto, "removeEventListener", 2, remove_event_listener);
    op(vm, et.proto, "dispatchEvent", 1, dispatch_event_op);

    let fp = vm.intr().function_proto;
    let ev = {
        let i = interface(vm, "Event", None, None);
        // Replace the illegal constructor with the real one (it reads its interface name from slot 0).
        let c = vm.make_native_with("Event", 1, event_ctor, true, Some(fp), vec![s("Event")]);
        rewire_ctor(vm, i, c, "Event");
        idl::Iface { ctor: c, proto: i.proto }
    };
    dom::register_iface("Event", ev);
    for (n, v) in [("NONE", 0.0), ("CAPTURING_PHASE", 1.0), ("AT_TARGET", 2.0), ("BUBBLING_PHASE", 3.0)] {
        konst(vm, ev, n, v);
    }
    let p = ev.proto;
    attr(vm, p, "type", ev_type, None);
    attr(vm, p, "target", ev_target, None);
    attr(vm, p, "srcElement", ev_target, None);
    attr(vm, p, "currentTarget", ev_current_target, None);
    op(vm, p, "composedPath", 0, ev_composed_path);
    attr(vm, p, "eventPhase", ev_phase, None);
    op(vm, p, "stopPropagation", 0, ev_stop_propagation);
    attr(vm, p, "cancelBubble", ev_cancel_bubble_get, Some(ev_cancel_bubble_set));
    op(vm, p, "stopImmediatePropagation", 0, ev_stop_immediate);
    attr(vm, p, "bubbles", ev_bubbles, None);
    attr(vm, p, "cancelable", ev_cancelable, None);
    attr(vm, p, "returnValue", ev_return_value_get, Some(ev_return_value_set));
    op(vm, p, "preventDefault", 0, ev_prevent_default);
    attr(vm, p, "defaultPrevented", ev_default_prevented, None);
    attr(vm, p, "composed", ev_composed, None);
    attr(vm, p, "timeStamp", ev_timestamp, None);
    op(vm, p, "initEvent", 1, ev_init_event);
    let g = vm.make_native("get isTrusted", 0, ev_is_trusted, false);
    root(vm, Value::Object(g));
    IS_TRUSTED_GETTER.with(|c| c.set(Some(g)));

    for name in [
        "CustomEvent",
        "UIEvent",
        "MouseEvent",
        "KeyboardEvent",
        "FocusEvent",
        "InputEvent",
        "WheelEvent",
        "PointerEvent",
        "DragEvent",
        "ErrorEvent",
        "PromiseRejectionEvent",
        "PopStateEvent",
        "HashChangeEvent",
        "ProgressEvent",
        "MessageEvent",
        "PageTransitionEvent",
        "SubmitEvent",
        "AnimationEvent",
        "TransitionEvent",
        "StorageEvent",
        "CloseEvent",
    ] {
        let parent = dom::iface(parent_event(name).unwrap()).unwrap();
        let i = interface(vm, name, Some(parent), None);
        let c = vm.make_native_with(name, 1, event_ctor, true, Some(parent.ctor), vec![s(name)]);
        rewire_ctor(vm, i, c, name);
        let i = idl::Iface { ctor: c, proto: i.proto };
        dom::register_iface(name, i);
        for (m, _) in members(name) {
            idl::attr_with(vm, i.proto, m, ev_member, None, s(m));
        }
        if name == "CustomEvent" {
            op(vm, i.proto, "initCustomEvent", 1, ev_init_custom_event);
        }
        if matches!(name, "MouseEvent" | "KeyboardEvent") {
            op(vm, i.proto, "getModifierState", 1, ev_get_modifier_state);
            if name == "MouseEvent" {
                for alias in [("pageX", "clientX"), ("pageY", "clientY"), ("x", "clientX"), ("y", "clientY"), ("offsetX", "clientX"), ("offsetY", "clientY")] {
                    idl::attr_with(vm, i.proto, alias.0, ev_member, None, s(alias.1));
                }
            }
        }
    }
}

/// Points the global binding, `prototype` and `constructor` links at constructor `c`.
fn rewire_ctor(vm: &mut Vm, i: idl::Iface, c: Obj, name: &str) {
    vm.heap.get_mut(c).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(i.proto), 0));
    vm.heap.get_mut(i.proto).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(c), WC));
    let g = vm.realm().global;
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(c), WC));
    root(vm, Value::Object(c));
}
