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
    let clock = crate::event_loop::now_ms().saturating_sub(origin) as f64;
    // The page clock has millisecond grain and stands still inside a task in virtual time; a script that
    // polls it would spin forever. Successive readings advance by at least 5 µs (the coarsening floor
    // timing-attack mitigations require) and never run backwards.
    LAST_HR.with(|l| {
        let v = if clock > l.get() { clock } else { l.get() + 0.005 };
        let v = (v * 1000.0).round() / 1000.0;
        l.set(v);
        v
    })
}

thread_local! {
    static LAST_HR: std::cell::Cell<f64> = const { std::cell::Cell::new(-1.0) };
}

/// Per-page event state: the timestamp floor and any queued details toggle (a new page reuses node
/// ids).
pub fn reset_clock() {
    LAST_HR.with(|l| l.set(-1.0));
    TOGGLE_PENDING.with(|t| t.borrow_mut().clear());
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
fn flatten_options(vm: &mut Vm, v: &Value) -> JsResult<(bool, bool, Option<bool>, Value)> {
    match v {
        Value::Object(_) => {
            let c = dict_member(vm, v, "capture")?;
            let o = dict_member(vm, v, "once")?;
            let p = dict_member(vm, v, "passive")?;
            let sig = dict_member(vm, v, "signal")?;
            // `signal` is a non-nullable AbortSignal member: a present value of any other type throws
            // during conversion, before the operation's steps (even with a null callback).
            if !sig.is_undefined() && !is_abort_signal(vm, &sig)? {
                return vm.throw_type("Failed to read the 'signal' property from 'AddEventListenerOptions': Failed to convert value to 'AbortSignal'.");
            }
            let passive = if p.is_undefined() { None } else { Some(vm.to_boolean(&p)) };
            Ok((vm.to_boolean(&c), vm.to_boolean(&o), passive, sig))
        }
        other => Ok((vm.to_boolean(other), false, None, Value::Undefined)),
    }
}

/// Whether `v` is an AbortSignal (an object whose prototype chain holds `AbortSignal.prototype`; with
/// no AbortSignal interface installed, any object is accepted).
fn is_abort_signal(vm: &mut Vm, v: &Value) -> JsResult<bool> {
    let Value::Object(o) = v else { return Ok(false) };
    let g = vm.realm().global;
    let ctor = vm.get(g, &PropertyKey::from_str("AbortSignal"))?;
    let Value::Object(c) = ctor else { return Ok(true) };
    let Value::Object(proto) = vm.get(c, &PropertyKey::from_str("prototype"))? else { return Ok(true) };
    let mut p = vm.heap.get(*o).proto;
    while let Some(x) = p {
        if x == proto {
            return Ok(true);
        }
        p = vm.heap.get(x).proto;
    }
    Ok(false)
}

/// DOM "default passive value": touchstart/touchmove/wheel/mousewheel listeners on the Window, the
/// Document, its document element or its body are passive unless the options say otherwise.
fn default_passive(t: TargetKey, ty: &str) -> bool {
    if !matches!(ty, "touchstart" | "touchmove" | "wheel" | "mousewheel") {
        return false;
    }
    match t {
        TargetKey::Window => true,
        TargetKey::Node(n) => {
            let d = dom::node_document(n);
            n == d || dom::document_element_of(d) == Some(n) || dom::body_of(d) == Some(n)
        }
        TargetKey::Other(_) => false,
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
    let passive = passive.unwrap_or_else(|| default_passive(t, &ty));
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

/// The webkit-prefixed handlers (HTML §8.1.8.2): IDL/content name suffix → event handler event type.
const PREFIXED_HANDLERS: &[(&str, &str)] = &[
    ("webkitanimationend", "webkitAnimationEnd"),
    ("webkitanimationiteration", "webkitAnimationIteration"),
    ("webkitanimationstart", "webkitAnimationStart"),
    ("webkittransitionend", "webkitTransitionEnd"),
];

/// The event handler event type an `on<name>` attribute listens for (the name itself, except the
/// camel-cased webkit-prefixed types).
pub fn handler_event_type(name: &str) -> &str {
    PREFIXED_HANDLERS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t).unwrap_or(name)
}

/// DOM §2.10 "invoke" step 8: the legacy type a trusted event of type `ty` falls back to when no
/// listener of its own type is on the object.
fn legacy_type(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "animationend" => "webkitAnimationEnd",
        "animationiteration" => "webkitAnimationIteration",
        "animationstart" => "webkitAnimationStart",
        "transitionend" => "webkitTransitionEnd",
        _ => return None,
    })
}

/// An `on*` content attribute of `el` was set or removed (attribute change steps).
pub fn handler_attribute_changed(vm: &mut Vm, el: usize, name: &str, present: bool) {
    let ty = handler_event_type(name);
    let t = handler_target(el, ty);
    let v = if present {
        match dom::attr_value(el, &format!("on{name}")) {
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
    for name in names {
        let ty = handler_event_type(&name).to_string();
        let t = handler_target(el, &ty);
        let known = page(|p| p.handlers.contains_key(&(t, ty.clone())));
        if !known {
            handler_attribute_changed(vm, el, &name, true);
        }
    }
}

/// "get the current value of the event handler": compiles an uncompiled body (as a function of
/// `event`, or for a Window `onerror` of `(event, source, lineno, colno, error)`).
fn handler_value(vm: &mut Vm, t: TargetKey, ty: &str) -> Value {
    let Some(r) = page(|p| p.handlers.get(&(t, ty.to_string())).copied()) else { return Value::Null };
    let v = idl::rooted(vm, r);
    if let Value::String(body) = &v {
        // AETHERDEBT SR93 (HTML §8.1.8.1 step 1.3, CSP3 §4.2.3): the attribute's body compiles only when
        // `script-src-attr` admits it; refused, the handler's value is null.
        let el = match t {
            TargetKey::Node(n) => Some(n),
            _ => dom::body_of(dom::main_doc()),
        };
        if !crate::security::inline_handler_allowed(el, &body.to_rust()) {
            vm.host_roots[r] = Value::Null;
            return Value::Null;
        }
        let params = if t == TargetKey::Window && ty == "error" { "event, source, lineno, colno, error" } else { "event" };
        let fc = vm.intr().function_ctor;
        let compiled = crate::security::compiling_trusted(|| {
            super::enter(vm, "handler-compile", |vm| vm.construct(&Value::Object(fc), &[s(params), Value::String(body.clone())], None))
        });
        let mut f = match compiled {
            Some(Ok(f)) => f,
            Some(Err(e)) => {
                report_exception(vm, &e);
                Value::Null
            }
            None => Value::Null,
        };
        // HTML §8.1.8.1 step 1.10: an element's handler is scoped by its document, its form owner (a
        // form-associated element) and the element itself — object environments, outermost first, as
        // `with` statements build them. The body parsed alone above, so it cannot close the wrapper's
        // braces; the wrapper only adds the scopes and names the function for the handler.
        if let (Value::Object(_), TargetKey::Node(n)) = (&f, t) {
            if let Some(scoped) = scoped_handler(vm, n, ty, params, body) {
                f = scoped;
            }
        }
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

/// Compiles `body` as `function on<ty>(<params>) { body }` inside `with (document) with (form) with (element)`.
fn scoped_handler(vm: &mut Vm, el: usize, ty: &str, params: &str, body: &js_core::string::JsStr) -> Option<Value> {
    let local = dom::with_doc(|d| d.element(html_core::NodeId(el)).map(|e| e.local.clone()))?;
    let element = wrap(vm, el);
    let document = wrap(vm, dom::node_document(el));
    let form = match &element {
        Value::Object(o) => {
            let o = *o;
            let is_form_associated =
                matches!(local.as_str(), "button" | "fieldset" | "input" | "object" | "output" | "select" | "textarea" | "img");
            if is_form_associated {
                super::enter(vm, "handler-form", |vm| vm.get(o, &PropertyKey::from_str("form"))).and_then(|r| r.ok()).unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        }
        _ => return None,
    };
    let form = if form.is_object() { form } else { Value::Object(vm.new_object(None)) };
    let src = format!(
        "with (__aether_doc) with (__aether_form) with (__aether_el) {{ return function on{ty}({params}) {{\n{}\n}}; }}",
        body.to_rust()
    );
    let fc = vm.intr().function_ctor;
    let maker = crate::security::compiling_trusted(|| {
        super::enter(vm, "handler-compile", |vm| {
            vm.construct(&Value::Object(fc), &[s("__aether_doc, __aether_form, __aether_el"), s(&src)], None)
        })
    })?
    .ok()?;
    let f = super::enter(vm, "handler-scope", |vm| vm.call(&maker, &Value::Undefined, &[document, form, element]))?.ok()?;
    f.is_object().then_some(f)
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
    // GlobalEventHandlers also carries the webkit-prefixed animation/transition handlers, whose event
    // type is the camel-cased prefixed name (`onwebkitanimationend` listens for `webkitAnimationEnd`).
    if names.contains(&"animationend") {
        for (n, ty) in PREFIXED_HANDLERS {
            idl::attr_with(vm, o, &format!("on{n}"), handler_get, Some(handler_set), s(ty));
        }
    }
}

// =================================================================================================
// Dispatch (DOM §2.9)
// =================================================================================================

/// The "get the parent" chain of a target: node ancestors, then the Window above the main Document
/// (except for `load`, which does not propagate from the document to the window).
fn event_path(t: TargetKey, ty: &str, composed: bool, related: Option<usize>) -> Vec<TargetKey> {
    let mut path = vec![t];
    if let TargetKey::Node(n) = t {
        let mut cur = n;
        // AETHERDEBT SR93 (DOM §2.9 step 5.9.6): the shadow-adjusted target so far — a parent outside
        // its tree that IS the relatedTarget retargeted against it ends the path (a mouseover
        // between two nodes of one shadow tree does not reach the host's side).
        let mut adjusted = n;
        // AETHERSHADOW: DOM §2.9 "get the parent" (assigned slot, shadow root → host unless contained).
        while let Some(p) = dom::with_doc(|d| crate::dom::shadow::event_parent(d, html_core::NodeId(cur), composed, html_core::NodeId(n))).map(|p| p.0) {
            if let Some(rt) = related {
                let stop = dom::with_doc(|d| {
                    use crate::dom::shadow as sh;
                    let in_tree = sh::is_shadow_including_inclusive_ancestor(d, sh::root(d, html_core::NodeId(adjusted)), html_core::NodeId(p));
                    if in_tree {
                        return Some(false);
                    }
                    if sh::retarget(d, html_core::NodeId(rt), Some(html_core::NodeId(p))).0 == p {
                        return None;
                    }
                    Some(true)
                });
                match stop {
                    None => break,
                    Some(true) => adjusted = p,
                    Some(false) => {}
                }
            }
            path.push(TargetKey::Node(p));
            cur = p;
        }
        if dom::kind(cur) == dom::NK::Document && cur == dom::main_doc() && ty != "load" {
            path.push(TargetKey::Window);
        } else if dom::kind(cur) == dom::NK::Document && ty != "load" {
            // AETHERDEBT SR93: a nested document's parent is its own window (AETHERFRAMES gap).
            if let Some(w) = super::window::frames::window_target_of_document(cur) {
                path.push(w);
            }
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
    // AETHERDEBT SR93: relatedTarget retargeting (DOM §2.9 steps 5.4–5.5, 5.9, 7) — only when the
    // page has shadow trees (retargeting is the identity otherwise).
    let related = related_node(vm, e).filter(|_| dom::with_doc(crate::dom::shadow::any));
    if let (Some(rt), TargetKey::Node(n)) = (related, t) {
        let r0 = dom::with_doc(|d| crate::dom::shadow::retarget(d, html_core::NodeId(rt), Some(html_core::NodeId(n)))).0;
        if r0 == n && rt != n {
            // "If target is not relatedTarget or target is event's relatedTarget" fails: nothing
            // is invoked (a mouse move between two nodes of one closed tree, seen from outside).
            set_flag(vm, e, F_DISPATCH, false);
            set_flag(vm, e, F_STOP, false);
            set_flag(vm, e, F_STOP_IMMEDIATE, false);
            return (flags(vm, e) & F_CANCELED == 0, false);
        }
    }
    let path = event_path(t, &ty, flags(vm, e) & F_COMPOSED != 0, related);
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
    // DOM §2.9 steps 5.5/5.9.6: a MouseEvent `click` has an activation target — the target when it
    // has activation behavior, else (only while the event bubbles) the nearest ancestor that does —
    // whose legacy-pre-activation behavior runs before the listeners and whose activation (or
    // legacy-canceled-activation) behavior runs after.
    let activation = if ty == "click" && is_mouse_event(vm, e) {
        let bubbles = flags(vm, e) & F_BUBBLES != 0;
        path.iter().take(if bubbles { path.len() } else { 1 }).find_map(|k| match k {
            TargetKey::Node(n) if has_activation_behavior(*n) => Some(*n),
            _ => None,
        })
    } else {
        None
    };
    let legacy = activation.and_then(|a| legacy_pre_activation(vm, a));
    let mut ran = false;
    let bubbles = flags(vm, e) & F_BUBBLES != 0;
    for (i, k) in path.iter().enumerate().rev() {
        let phase = if i == 0 { 2.0 } else { 1.0 };
        set_slot(vm, e, E_PHASE, num(phase));
        shadow_adjusted_target(vm, e, t, *k);
        shadow_adjusted_related(vm, e, related, *k);
        ran |= invoke_listeners(vm, e, *k, &ty, true);
    }
    for (i, k) in path.iter().enumerate() {
        shadow_adjusted_target(vm, e, t, *k);
        shadow_adjusted_related(vm, e, related, *k);
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
    // AETHERSHADOW: clearTargets — a target left inside a shadow tree is not exposed after dispatch.
    if let (TargetKey::Node(o), Some(last)) = (t, path.last()) {
        let against = if let TargetKey::Node(l) = last { Some(html_core::NodeId(*l)) } else { None };
        let fin = dom::with_doc(|d| {
            let r = crate::dom::shadow::retarget(d, html_core::NodeId(o), against);
            (!crate::dom::shadow::is_shadow_root(d, crate::dom::shadow::root(d, r))).then_some(r.0)
        });
        let v = match fin {
            Some(r) => wrap(vm, r),
            None => Value::Null,
        };
        set_slot(vm, e, E_TARGET, v);
    }
    // relatedTarget after dispatch: the original, or null when it is left inside a shadow tree.
    if let Some(rt) = related {
        let against = match path.last() {
            Some(TargetKey::Node(l)) => Some(html_core::NodeId(*l)),
            _ => None,
        };
        let fin = dom::with_doc(|d| {
            let r = crate::dom::shadow::retarget(d, html_core::NodeId(rt), against);
            !crate::dom::shadow::is_shadow_root(d, crate::dom::shadow::root(d, r))
        });
        let v = if fin { wrap(vm, rt) } else { Value::Null };
        set_related(vm, e, v);
    }
    set_flag(vm, e, F_DISPATCH, false);
    set_flag(vm, e, F_STOP, false);
    set_flag(vm, e, F_STOP_IMMEDIATE, false);
    let ok = flags(vm, e) & F_CANCELED == 0;
    if let Some(a) = activation {
        if ok {
            let target = if let TargetKey::Node(n) = t { n } else { a };
            let trusted = flags(vm, e) & F_TRUSTED != 0;
            activation_behavior(vm, a, target, trusted, legacy);
        } else if let Some(l) = legacy {
            legacy_canceled_activation(vm, a, l);
        }
    }
    (ok, ran)
}

/// AETHERSHADOW: the event's target while `k`'s listeners run — the original target retargeted
/// against `k` (DOM §2.5 via `dom::shadow::retarget`, the one retargeting function).
fn shadow_adjusted_target(vm: &mut Vm, e: Obj, t: TargetKey, k: TargetKey) {
    if let TargetKey::Node(o) = t {
        let against = if let TargetKey::Node(n) = k { Some(html_core::NodeId(n)) } else { None };
        let r = dom::with_doc(|d| crate::dom::shadow::retarget(d, html_core::NodeId(o), against)).0;
        let v = wrap(vm, r);
        set_slot(vm, e, E_TARGET, v);
    }
}

/// The event's `relatedTarget` when it is a Node (MouseEvent, FocusEvent, … keep it in the
/// initialized dictionary).
fn related_node(vm: &mut Vm, e: Obj) -> Option<usize> {
    let Value::Object(init) = slot(vm, e, E_INIT) else { return None };
    let key = PropertyKey::from_str("relatedTarget");
    vm.heap.get(init).props.get(&key)?;
    let v = vm.get(init, &key).ok()?;
    node_of(vm, &v)
}

fn set_related(vm: &mut Vm, e: Obj, v: Value) {
    if let Value::Object(init) = slot(vm, e, E_INIT) {
        let _ = vm.create_data_property(init, PropertyKey::from_str("relatedTarget"), v);
    }
}

/// AETHERDEBT SR93: the event's relatedTarget while `k`'s listeners run — the original
/// relatedTarget retargeted against `k` (DOM §2.9 "invoke": the struct's shadow-adjusted
/// relatedTarget).
fn shadow_adjusted_related(vm: &mut Vm, e: Obj, related: Option<usize>, k: TargetKey) {
    let Some(rt) = related else { return };
    let against = if let TargetKey::Node(n) = k { Some(html_core::NodeId(n)) } else { None };
    let r = dom::with_doc(|d| crate::dom::shadow::retarget(d, html_core::NodeId(rt), against)).0;
    let v = wrap(vm, r);
    set_related(vm, e, v);
}

fn is_mouse_event(vm: &Vm, e: Obj) -> bool {
    is_iface(vm, e, "MouseEvent")
}

fn html_tag(n: usize) -> Option<String> {
    dom::with_doc(|d| d.element(html_core::NodeId(n)).filter(|e| e.ns == html_core::Namespace::Html).map(|e| e.local.to_string()))
}

/// The summary that is its details' summary (HTML §4.11.2).
fn is_details_summary(n: usize) -> bool {
    let Some(p) = parent_of(n) else { return false };
    if html_tag(p).as_deref() != Some("details") {
        return false;
    }
    dom::children_of(p).into_iter().find(|c| html_tag(*c).as_deref() == Some("summary")) == Some(n)
}

/// Elements with activation behavior (HTML: a/area with href, buttons, the input types that act on a
/// click, a details' summary, label).
fn has_activation_behavior(n: usize) -> bool {
    match html_tag(n).as_deref() {
        Some("a" | "area") => dom::attr_value(n, "href").is_some(),
        Some("button" | "label") => true,
        Some("input") => matches!(super::controls::input_type(n), "checkbox" | "radio" | "submit" | "image" | "reset" | "button" | "color" | "file"),
        Some("summary") => is_details_summary(n),
        _ => false,
    }
}

/// What legacy-pre-activation behavior changed, to undo on cancel.
#[derive(Clone, Copy)]
pub enum Legacy {
    Checkbox(bool),
    Radio(Option<usize>),
}

fn legacy_pre_activation(vm: &mut Vm, a: usize) -> Option<Legacy> {
    if html_tag(a).as_deref() != Some("input") {
        return None;
    }
    match super::controls::input_type(a) {
        "checkbox" => {
            let old = super::html::checkedness(a);
            super::html::set_checkedness(vm, a, !old);
            super::controls::set_indeterminate(a, false);
            Some(Legacy::Checkbox(old))
        }
        "radio" => {
            let prev = super::controls::checked_radio_in_group(a);
            super::html::set_checkedness(vm, a, true);
            Some(Legacy::Radio(prev))
        }
        _ => None,
    }
}

fn legacy_canceled_activation(vm: &mut Vm, a: usize, l: Legacy) {
    match l {
        Legacy::Checkbox(old) => super::html::set_checkedness(vm, a, old),
        Legacy::Radio(prev) => {
            if prev != Some(a) {
                page(|p| p.checked.insert(a, false));
                super::touch();
            }
            if let Some(p) = prev.filter(|p| *p != a) {
                if super::controls::input_type(p) == "radio" {
                    super::html::set_checkedness(vm, p, true);
                }
            }
        }
    }
}

/// Interactive content (HTML §3.2.5.2.7) for the label activation rule.
fn interactive(n: usize) -> bool {
    match html_tag(n).as_deref() {
        Some("a") => dom::attr_value(n, "href").is_some(),
        Some("button" | "details" | "embed" | "iframe" | "label" | "select" | "textarea") => true,
        Some("input") => super::controls::input_type(n) != "hidden",
        Some("audio" | "video") => dom::attr_value(n, "controls").is_some(),
        Some("img") => dom::attr_value(n, "usemap").is_some(),
        _ => false,
    }
}

fn activation_behavior(vm: &mut Vm, a: usize, target: usize, trusted: bool, legacy: Option<Legacy>) {
    let tag = html_tag(a).unwrap_or_default();
    match tag.as_str() {
        "input" => match super::controls::input_type(a) {
            "checkbox" | "radio" => {
                if dom::is_connected(a) && legacy.is_some() {
                    let changed = match legacy {
                        Some(Legacy::Checkbox(old)) => super::html::checkedness(a) != old,
                        Some(Legacy::Radio(prev)) => prev != Some(a),
                        None => false,
                    };
                    if !changed {
                        return;
                    }
                    fire_bubbling_composed(vm, a, "input");
                    fire_simple(vm, TargetKey::Node(a), "change", true, false);
                }
            }
            "submit" | "image" => {
                if !super::controls::disabled(a) {
                    // HTML form submission step 1: a form that is not connected cannot navigate (AETHEREVENTS);
                    // a script-started submission continues to the entry list (AETHERFORMS: `!trusted`).
                    if let Some(f) = super::controls::form_owner(a).filter(|f| dom::is_connected(*f)) {
                        super::controls::submit_form_with(vm, f, Some(a), !trusted);
                    }
                }
            }
            "reset" => {
                if !super::controls::disabled(a) {
                    if let Some(f) = super::controls::form_owner(a) {
                        super::controls::reset_form(vm, f);
                    }
                }
            }
            _ => {}
        },
        "button" => {
            if super::controls::disabled(a) {
                return;
            }
            let ty = dom::attr_value(a, "type").unwrap_or_default().to_ascii_lowercase();
            if let Some(f) = super::controls::form_owner(a) {
                match ty.as_str() {
                    "reset" => super::controls::reset_form(vm, f),
                    "button" => {}
                    _ if !dom::is_connected(f) => {}
                    _ => super::controls::submit_form_with(vm, f, Some(a), !trusted),
                }
            }
        }
        "a" | "area" => {
            // A user's click follows an ordinary link in the shell; a script-dispatched click
            // navigates here (fragment navigations stay in the document). A `javascript:` URL is
            // evaluated by the page either way (the shell cannot run it).
            if let Some(href) = dom::attr_value(a, "href") {
                if javascript_url_source(&href).is_some() || !trusted {
                    follow_hyperlink_from(vm, Some(a), &href);
                }
            }
        }
        "summary" => {
            let Some(d) = parent_of(a) else { return };
            let was_open = dom::attr_value(d, "open").is_some();
            // The toggle task is queued by the `open` attribute change steps (js/dom.rs), for script
            // and user alike.
            if was_open {
                dom::remove_attr(vm, d, "open");
            } else {
                dom::set_attr(vm, d, "open", "");
            }
        }
        "label" => {
            let control = dom::with_doc(|d| super::controls::label_control_in(d, a));
            let Some(c) = control else { return };
            if target == c || super::controls::disabled(c) {
                return;
            }
            // A click on interactive content inside the label belongs to that content.
            let mut cur = Some(target);
            while let Some(n) = cur {
                if n == a {
                    break;
                }
                if interactive(n) {
                    return;
                }
                cur = parent_of(n);
            }
            synthetic_click(vm, c);
        }
        _ => {}
    }
}

fn fire_bubbling_composed(vm: &mut Vm, n: usize, ty: &str) {
    let e = new_event(vm, "Event", ty, true, false, None);
    set_flag(vm, e, F_COMPOSED, true);
    let r = root(vm, Value::Object(e));
    dispatch(vm, e, TargetKey::Node(n));
    unroot(vm, r);
}

/// The script of a `javascript:` URL (HTML §7.4.2.3.3 "evaluate a javascript: URL": the URL after
/// the scheme, percent-decoded), or `None` for any other URL.
fn javascript_url_source(href: &str) -> Option<String> {
    let t = href.trim_matches(|c: char| c.is_ascii_whitespace() || c.is_ascii_control());
    if t.len() < 11 || !t[..11].eq_ignore_ascii_case("javascript:") {
        return None;
    }
    let raw = t[11..].as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(raw[i + 1]), hex(raw[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// HTML "follow the hyperlink": a `javascript:` URL queues a task (navigation and traversal task
/// source) that runs its script as a classic script; any other URL navigates.
/// `el`: the hyperlink element (the CSP violation's target).
fn follow_hyperlink_from(vm: &mut Vm, el: Option<usize>, href: &str) {
    match javascript_url_source(href) {
        // AETHERDEBT SR93 (CSP3 §4.2.4): a `javascript:` navigation runs only when `script-src-elem`
        // admits the URL as inline script.
        Some(_) if !crate::security::javascript_url_allowed(el, href.trim()) => {}
        Some(src) => {
            let fp = vm.intr().function_proto;
            let f = vm.make_native_with("", 0, run_javascript_url, false, Some(fp), vec![s(&src)]);
            let r = root(vm, Value::Object(f));
            page(|p| p.tasks.push_back(super::Task::Callback(r)));
        }
        None => {
            let _ = super::window::navigate_from_link(vm, href);
        }
    }
}

fn run_javascript_url(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if let Value::String(src) = vm.native_slot(ctx.callee, 0) {
        let _ = super::run_classic(vm, &src.to_rust(), "javascript-url");
    }
    Ok(Value::Undefined)
}

thread_local! {
    /// Details elements with a toggle task queued, and the old state that task reports.
    static TOGGLE_PENDING: std::cell::RefCell<std::collections::HashMap<usize, bool>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// HTML §4.11.1 "queue a details toggle event task": a pending task for `d` keeps its oldState (the
/// new state is read when it runs); otherwise a task is queued that fires a ToggleEvent at `d`.
pub(crate) fn queue_details_toggle(vm: &mut Vm, d: usize, was_open: bool) {
    let pending = TOGGLE_PENDING.with(|t| t.borrow().contains_key(&d));
    if pending {
        return;
    }
    TOGGLE_PENDING.with(|t| t.borrow_mut().insert(d, was_open));
    let dv = wrap(vm, d);
    let fp = vm.intr().function_proto;
    let f = vm.make_native_with("", 0, fire_details_toggle, false, Some(fp), vec![dv]);
    let r = root(vm, Value::Object(f));
    page(|p| p.tasks.push_back(super::Task::Callback(r)));
}

fn fire_details_toggle(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let dv = vm.native_slot(ctx.callee, 0);
    let Some(d) = node_of(vm, &dv) else { return Ok(Value::Undefined) };
    let Some(was_open) = TOGGLE_PENDING.with(|t| t.borrow_mut().remove(&d)) else { return Ok(Value::Undefined) };
    let is_open = dom::attr_value(d, "open").is_some();
    let state = |o: bool| s(if o { "open" } else { "closed" });
    let init = vm.new_plain_object();
    vm.create_data_property(init, PropertyKey::from_str("oldState"), state(was_open))?;
    vm.create_data_property(init, PropertyKey::from_str("newState"), state(is_open))?;
    let e = new_event(vm, "ToggleEvent", "toggle", false, false, Some(init));
    let r = root(vm, Value::Object(e));
    dispatch(vm, e, TargetKey::Node(d));
    unroot(vm, r);
    Ok(Value::Undefined)
}

/// DOM "invoke" for one path entry and phase (`capturing` = the capture pass): sets currentTarget,
/// runs "inner invoke", and — step 8 — for a trusted animation/transition event that found no listener
/// of its own type on this object, the webkit-prefixed listeners with the type swapped and restored.
/// Returns whether any listener or handler ran.
fn invoke_listeners(vm: &mut Vm, e: Obj, t: TargetKey, ty: &str, capturing: bool) -> bool {
    if flags(vm, e) & F_STOP != 0 {
        return false;
    }
    let cur = target_value(vm, t);
    set_slot(vm, e, E_CURRENT, cur.clone());
    let (found, mut ran) = inner_invoke(vm, e, t, ty, capturing, &cur);
    if !found && flags(vm, e) & F_TRUSTED != 0 {
        if let Some(legacy) = legacy_type(ty) {
            let original = slot(vm, e, E_TYPE);
            set_slot(vm, e, E_TYPE, s(legacy));
            ran |= inner_invoke(vm, e, t, legacy, capturing, &cur).1;
            set_slot(vm, e, E_TYPE, original);
        }
    }
    ran
}

/// DOM "inner invoke" over a clone of `t`'s listener list. Returns (found: a live listener of type
/// `ty` is on the object, whatever its phase; ran: one was called).
fn inner_invoke(vm: &mut Vm, e: Obj, t: TargetKey, ty: &str, capturing: bool, cur: &Value) -> (bool, bool) {
    let listeners: Vec<Listener> = page(|p| p.listeners.get(&t).cloned().unwrap_or_default());
    let mut found = false;
    let mut ran = false;
    for l in listeners {
        // Skip what was removed by an earlier listener of this dispatch.
        let live = page(|p| p.listeners.get(&t).is_some_and(|ls| ls.iter().any(|x| x.serial == l.serial && !x.removed)));
        if !live || l.ty != ty {
            continue;
        }
        found = true;
        if capturing != l.capture {
            continue;
        }
        if l.once {
            let r = l.callback;
            page(|p| {
                if let Some(ls) = p.listeners.get_mut(&t) {
                    ls.retain(|x| x.serial != l.serial);
                }
            });
            // keep the callback alive for this call; unroot after
            let cb = idl::rooted(vm, r);
            call_listener(vm, e, cur, &cb, l.passive, t, ty, false);
            unroot(vm, r);
            ran = true;
        } else if l.handler {
            let h = handler_value(vm, t, ty);
            if vm.is_callable(&h) {
                call_listener(vm, e, cur, &h, l.passive, t, ty, true);
                ran = true;
            }
        } else {
            let cb = idl::rooted(vm, l.callback);
            call_listener(vm, e, cur, &cb, l.passive, t, ty, false);
            ran = true;
        }
        if flags(vm, e) & F_STOP_IMMEDIATE != 0 {
            break;
        }
    }
    (found, ran)
}

#[allow(clippy::too_many_arguments)]
fn call_listener(vm: &mut Vm, e: Obj, this: &Value, cb: &Value, passive: bool, t: TargetKey, ty: &str, is_handler: bool) {
    let saved = CURRENT_EVENT.with(|c| c.replace(Some(e)));
    call_listener_inner(vm, e, this, cb, passive, t, ty, is_handler);
    CURRENT_EVENT.with(|c| c.set(saved));
}

#[allow(clippy::too_many_arguments)]
fn call_listener_inner(vm: &mut Vm, e: Obj, this: &Value, cb: &Value, passive: bool, t: TargetKey, ty: &str, is_handler: bool) {
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
            if ty == "beforeunload" && is_iface(vm, e, "BeforeUnloadEvent") {
                // A BeforeUnloadEvent: a non-null (DOMString?) return value sets the canceled flag
                // and fills an empty returnValue.
                if !r.is_nullish() {
                    set_flag(vm, e, F_CANCELED, true);
                    if let Value::Object(init) = slot(vm, e, E_INIT) {
                        let key = PropertyKey::from_str("returnValue");
                        let empty = match vm.get(init, &key) {
                            Ok(Value::String(v)) => v.to_rust().is_empty(),
                            _ => true,
                        };
                        if empty {
                            if let Some(Ok(sv)) = super::enter(vm, "beforeunload-return", |vm| vm.to_string(&r)) {
                                let _ = vm.create_data_property(init, key, Value::String(sv));
                            }
                        }
                    }
                }
                return;
            }
            let cancel = if t == TargetKey::Window && ty == "error" { matches!(r, Value::Bool(true)) } else { matches!(r, Value::Bool(false)) };
            if cancel {
                cancel_event(vm, e);
            }
        }
    }
}

/// Whether `e`'s prototype chain holds interface `name`'s prototype.
fn is_iface(vm: &Vm, e: Obj, name: &str) -> bool {
    let Some(ip) = dom::iface(name).map(|i| i.proto) else { return false };
    let mut p = vm.heap.get(e).proto;
    while let Some(o) = p {
        if o == ip {
            return true;
        }
        p = vm.heap.get(o).proto;
    }
    false
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
/// Fires a trusted, cancelable, non-bubbling event; returns false when a listener canceled it.
pub fn fire_simple_cancelable(vm: &mut Vm, t: TargetKey, ty: &str) -> bool {
    if super::engine_poisoned() {
        return true;
    }
    let e = new_event(vm, "Event", ty, false, true, None);
    let r = root(vm, Value::Object(e));
    let (ok, _) = dispatch(vm, e, t);
    unroot(vm, r);
    ok
}

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

/// HTML "fire a focus event" (§6.6.4, AETHERDEBT SR93): a trusted, composed FocusEvent `ty` at `n`
/// whose `relatedTarget` is `related` (the element focus moves from or to; null when none), `view`
/// the Window. `focus`/`blur` do not bubble, `focusin`/`focusout` do (UI Events §5.2.2).
pub fn fire_focus(vm: &mut Vm, n: usize, ty: &str, related: Option<usize>) -> bool {
    if super::engine_poisoned() {
        return false;
    }
    let init = vm.new_plain_object();
    let rel = match related {
        Some(r) => wrap(vm, r),
        None => Value::Null,
    };
    let _ = vm.create_data_property(init, PropertyKey::from_str("relatedTarget"), rel);
    let view = target_value(vm, TargetKey::Window);
    let _ = vm.create_data_property(init, PropertyKey::from_str("view"), view);
    let _ = vm.create_data_property(init, PropertyKey::from_str("detail"), num(0.0));
    let bubbles = matches!(ty, "focusin" | "focusout");
    let e = new_event(vm, "FocusEvent", ty, bubbles, false, Some(init));
    set_flag(vm, e, F_COMPOSED, true);
    let r = root(vm, Value::Object(e));
    let (_, ran) = dispatch(vm, e, TargetKey::Node(n));
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

/// A trusted pointer-motion event at element `n` (UI Events §5.3, AETHERHOVER SR67): `mousemove`,
/// `mouseover` and `mouseout` bubble, are cancelable and composed; `mouseenter` and `mouseleave` do
/// neither (§5.3.4: the engine fires one per element entered / left, so an ancestor's listener hears its
/// own boundary, not a bubbled child's). `client` is the viewport position (`clientX/Y`, mirrored into
/// `screenX/Y`, and `pageX/x/offsetX` alias it); `related` the element the pointer came from or went to
/// (`relatedTarget`, null at the viewport edge). Returns whether any listener ran.
pub fn user_mouse(vm: &mut Vm, n: usize, ty: &str, client: (f64, f64), related: Option<usize>) -> bool {
    if super::engine_poisoned() {
        return false;
    }
    let boundary_pair = matches!(ty, "mouseenter" | "mouseleave");
    let init = vm.new_plain_object();
    let ri = root(vm, Value::Object(init));
    for (k, v) in [("clientX", client.0), ("clientY", client.1), ("screenX", client.0), ("screenY", client.1)] {
        let _ = vm.create_data_property(init, PropertyKey::from_str(k), num(v));
    }
    let rel = match related {
        Some(r) => target_value(vm, TargetKey::Node(r)),
        None => Value::Null,
    };
    let _ = vm.create_data_property(init, PropertyKey::from_str("relatedTarget"), rel);
    let e = new_event(vm, "MouseEvent", ty, !boundary_pair, !boundary_pair, Some(init));
    if !boundary_pair {
        set_flag(vm, e, F_COMPOSED, true);
    }
    let r = root(vm, Value::Object(e));
    let (_, ran) = dispatch(vm, e, TargetKey::Node(n));
    unroot(vm, r);
    unroot(vm, ri);
    ran
}

/// A trusted `mousedown`/`mouseup` at the element under the pointer (UI Events §5.3.3: bubbles,
/// cancelable, composed; the primary button, `buttons` the state after the transition, `detail` 1).
/// A text-node hit is retargeted to its parent element. Returns false when a listener canceled it.
pub fn user_button(vm: &mut Vm, n: usize, ty: &str, client: (f64, f64)) -> bool {
    if super::engine_poisoned() {
        return true;
    }
    let n = if dom::kind(n) == dom::NK::Element { n } else { parent_of(n).unwrap_or(n) };
    let init = vm.new_plain_object();
    let ri = root(vm, Value::Object(init));
    let buttons = if ty == "mousedown" { 1.0 } else { 0.0 };
    let g = Value::Object(vm.realm().global);
    let _ = vm.create_data_property(init, PropertyKey::from_str("view"), g);
    for (k, v) in [("clientX", client.0), ("clientY", client.1), ("screenX", client.0), ("screenY", client.1), ("button", 0.0), ("buttons", buttons), ("detail", 1.0)] {
        let _ = vm.create_data_property(init, PropertyKey::from_str(k), num(v));
    }
    let e = new_event(vm, "MouseEvent", ty, true, true, Some(init));
    set_flag(vm, e, F_COMPOSED, true);
    let r = root(vm, Value::Object(e));
    let (ok, _) = dispatch(vm, e, TargetKey::Node(n));
    unroot(vm, r);
    unroot(vm, ri);
    ok
}

/// A trusted `keydown`/`keyup` (UI Events §5.6: a KeyboardEvent that bubbles, is cancelable and
/// composed) at `focused`, else the body, else the document element, else the document. The members
/// come from the shell's key string (`crate::api::uievents::key_init`). Returns false when a listener
/// canceled it.
pub fn user_key(vm: &mut Vm, focused: Option<usize>, ty: &str, k: &crate::api::uievents::KeyInit) -> bool {
    if super::engine_poisoned() {
        return true;
    }
    let d = dom::main_doc();
    let t = focused.or_else(|| dom::body_of(d)).or_else(|| dom::document_element_of(d)).unwrap_or(d);
    let init = vm.new_plain_object();
    let ri = root(vm, Value::Object(init));
    let g = Value::Object(vm.realm().global);
    let _ = vm.create_data_property(init, PropertyKey::from_str("view"), g);
    let _ = vm.create_data_property(init, PropertyKey::from_str("key"), s(&k.key));
    let _ = vm.create_data_property(init, PropertyKey::from_str("code"), s(&k.code));
    for (m, v) in [("location", k.location as f64), ("keyCode", k.key_code as f64), ("which", k.key_code as f64), ("charCode", 0.0), ("detail", 0.0)] {
        let _ = vm.create_data_property(init, PropertyKey::from_str(m), num(v));
    }
    for (m, v) in [("ctrlKey", k.ctrl), ("shiftKey", k.shift), ("altKey", k.alt), ("metaKey", k.meta), ("repeat", false), ("isComposing", false)] {
        let _ = vm.create_data_property(init, PropertyKey::from_str(m), Value::Bool(v));
    }
    for m in crate::api::uievents::MODIFIER_MEMBERS {
        let _ = vm.create_data_property(init, PropertyKey::from_str(m), Value::Bool(false));
    }
    let e = new_event(vm, "KeyboardEvent", ty, true, true, Some(init));
    set_flag(vm, e, F_COMPOSED, true);
    let r = root(vm, Value::Object(e));
    let (ok, _) = dispatch(vm, e, TargetKey::Node(t));
    unroot(vm, r);
    unroot(vm, ri);
    ok
}

/// Fires `click` at `n` with the checkbox/radio legacy-pre-activation behavior and the `input`/`change`
/// events that follow (HTML §4.10.5.1.15/16). Returns (not canceled, ran).
fn click_with_activation(vm: &mut Vm, n: usize, trusted: bool) -> (bool, bool) {
    // HTMLElement.click(): the "click in progress" flag stops re-entrant clicks.
    if CLICKING.with(|c| c.borrow().contains(&n)) {
        return (true, false);
    }
    CLICKING.with(|c| c.borrow_mut().insert(n));
    let init = vm.new_plain_object();
    let _ = vm.create_data_property(init, PropertyKey::from_str("detail"), num(1.0));
    let e = new_event(vm, "MouseEvent", "click", true, true, Some(init));
    set_flag(vm, e, F_COMPOSED, true);
    if !trusted {
        set_flag(vm, e, F_TRUSTED, false);
    }
    let r = root(vm, Value::Object(e));
    let (ok, ran) = dispatch(vm, e, TargetKey::Node(n));
    unroot(vm, r);
    CLICKING.with(|c| c.borrow_mut().remove(&n));
    page(|p| p.last_click_canceled = !ok);
    (ok, ran)
}

thread_local! {
    static CLICKING: std::cell::RefCell<std::collections::HashSet<usize>> = std::cell::RefCell::new(std::collections::HashSet::new());
    /// The Window's "current event" (HTML `window.event`).
    static CURRENT_EVENT: std::cell::Cell<Option<Obj>> = const { std::cell::Cell::new(None) };
}

/// `window.event`: the event whose listener is running, else undefined.
pub fn current_event() -> Value {
    CURRENT_EVENT.with(|c| c.get()).map(Value::Object).unwrap_or(Value::Undefined)
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
        'u' => {
            if v.is_undefined() {
                num(0.0)
            } else {
                num(vm.to_uint32(&v)? as f64)
            }
        }
        '1' => {
            if v.is_undefined() {
                num(1.0)
            } else {
                num(vm.to_number(&v)?)
            }
        }
        's' => {
            if v.is_undefined() {
                s("")
            } else {
                Value::String(vm.to_string(&v)?)
            }
        }
        // DOMString? (null and undefined stay null)
        'S' => {
            if v.is_nullish() {
                Value::Null
            } else {
                Value::String(vm.to_string(&v)?)
            }
        }
        // sequence<…> defaulting to the empty list
        'L' => {
            if v.is_undefined() {
                Value::Object(vm.new_array(Vec::new()))
            } else {
                let list = vm.array_to_list(&v);
                Value::Object(vm.new_array(list))
            }
        }
        'o' => {
            if v.is_undefined() {
                Value::Null
            } else {
                v
            }
        }
        // A required FormData (FormDataEventInit.formData; AETHERFORMS).
        'F' => {
            if !super::forms::is_form_data(vm, &v)? {
                return vm.throw_type("Failed to construct 'FormDataEvent': Failed to read the 'formData' property from 'FormDataEventInit': required member formData is not of type 'FormData'.");
            }
            v
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
        "WheelEvent" => &[("deltaX", 'n'), ("deltaY", 'n'), ("deltaZ", 'n'), ("deltaMode", 'u')],
        "PointerEvent" => &[
            ("pointerId", 'l'),
            ("width", '1'),
            ("height", '1'),
            ("pressure", 'n'),
            ("tangentialPressure", 'n'),
            ("tiltX", 'l'),
            ("tiltY", 'l'),
            ("twist", 'l'),
            ("pointerType", 's'),
            ("isPrimary", 'b'),
        ],
        "DragEvent" => &[("dataTransfer", 'o')],
        "CompositionEvent" => &[("data", 's')],
        "MouseEvent" => &[
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
        "InputEvent" => &[("data", 'S'), ("dataTransfer", 'o'), ("inputType", 's'), ("isComposing", 'b')],
        "TouchEvent" => &[
            ("touches", 'L'),
            ("targetTouches", 'L'),
            ("changedTouches", 'L'),
            ("altKey", 'b'),
            ("metaKey", 'b'),
            ("ctrlKey", 'b'),
            ("shiftKey", 'b'),
        ],
        "ToggleEvent" => &[("oldState", 's'), ("newState", 's')],
        "ErrorEvent" => &[("message", 's'), ("filename", 's'), ("lineno", 'l'), ("colno", 'l'), ("error", 'a')],
        "PromiseRejectionEvent" => &[("promise", 'a'), ("reason", 'a')],
        "PopStateEvent" => &[("state", 'a')],
        "HashChangeEvent" => &[("oldURL", 's'), ("newURL", 's')],
        "ProgressEvent" => &[("lengthComputable", 'b'), ("loaded", 'n'), ("total", 'n')],
        "MessageEvent" => &[("data", 'a'), ("origin", 's'), ("lastEventId", 's'), ("source", 'o')],
        "PageTransitionEvent" => &[("persisted", 'b')],
        "SubmitEvent" => &[("submitter", 'o')],
        "FormDataEvent" => &[("formData", 'F')],
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
        | "ProgressEvent" | "MessageEvent" | "PageTransitionEvent" | "SubmitEvent" | "FormDataEvent" | "AnimationEvent"
        | "TransitionEvent" | "StorageEvent" | "CloseEvent" | "ToggleEvent" | "BeforeUnloadEvent" => "Event",
        "MouseEvent" | "KeyboardEvent" | "FocusEvent" | "InputEvent" | "CompositionEvent" | "TouchEvent" => "UIEvent",
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
            if *m == "view" && !v.is_nullish() && v.as_object() != Some(vm.realm().global) {
                return vm.throw_type(&format!("Failed to construct '{iface}': member view is not of type Window."));
            }
            let cv = convert_member(vm, v, *k)?;
            vm.create_data_property(init, PropertyKey::from_str(m), cv)?;
        }
        // EventModifierInit's dictionary-only modifier* members (UI Events §3.6.4).
        if matches!(i.as_str(), "MouseEvent" | "KeyboardEvent" | "TouchEvent") {
            for m in crate::api::uievents::MODIFIER_MEMBERS {
                let v = dict_member(vm, &dict, m)?;
                let b = Value::Bool(vm.to_boolean(&v));
                vm.create_data_property(init, PropertyKey::from_str(m), b)?;
            }
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
            // AETHERSHADOW: entries inside closed shadow trees the current target cannot see are hidden.
            let cur = slot(vm, e, E_CURRENT);
            let ids: Vec<Option<html_core::NodeId>> = list.iter().map(|v| node_of(vm, v).map(html_core::NodeId)).collect();
            let at = list.iter().position(|v| v.same_value(&cur)).unwrap_or(0);
            let keep = dom::with_doc(|d| crate::dom::shadow::composed_path(d, &ids, at));
            let out: Vec<Value> = keep.into_iter().filter_map(|i| list.get(i).cloned()).collect();
            Ok(Value::Object(vm.new_array(out)))
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
    let e = this_event(vm, ctx)?;
    if flags(vm, e) & F_DISPATCH != 0 {
        return Ok(Value::Undefined);
    }
    ev_init_event(vm, ctx)?;
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

/// The legacy `initUIEvent` / `initMouseEvent` / `initKeyboardEvent`: initEvent, then the positional
/// members (a no-op while dispatching).
fn ev_init_legacy(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    if flags(vm, e) & F_DISPATCH != 0 {
        return Ok(Value::Undefined);
    }
    ev_init_event(vm, ctx)?;
    let which = callee_str(vm, ctx);
    let names: &[(&str, char)] = match which.as_str() {
        "initMouseEvent" => &[
            ("view", 'o'),
            ("detail", 'l'),
            ("screenX", 'l'),
            ("screenY", 'l'),
            ("clientX", 'l'),
            ("clientY", 'l'),
            ("ctrlKey", 'b'),
            ("altKey", 'b'),
            ("shiftKey", 'b'),
            ("metaKey", 'b'),
            ("button", 'l'),
            ("relatedTarget", 'o'),
        ],
        "initKeyboardEvent" => &[
            ("view", 'o'),
            ("key", 's'),
            ("location", 'u'),
            ("ctrlKey", 'b'),
            ("altKey", 'b'),
            ("shiftKey", 'b'),
            ("metaKey", 'b'),
        ],
        "initCompositionEvent" => &[("view", 'o'), ("data", 's')],
        _ => &[("view", 'o'), ("detail", 'l')],
    };
    let init = match slot(vm, e, E_INIT) {
        Value::Object(o) => o,
        _ => {
            let o = vm.new_plain_object();
            set_slot(vm, e, E_INIT, Value::Object(o));
            o
        }
    };
    for (i, (m, k)) in names.iter().enumerate() {
        let v = arg(vm, ctx, 3 + i);
        let cv = convert_member(vm, v, *k)?;
        vm.create_data_property(init, PropertyKey::from_str(m), cv)?;
    }
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
        | "animationName" | "propertyName" | "pseudoElement" | "url" | "key" | "oldState" | "newState" => s(""),
        "lineno" | "colno" | "button" | "buttons" | "location" | "charCode" | "keyCode" | "which" | "screenX"
        | "screenY" | "clientX" | "clientY" | "movementX" | "movementY" | "loaded" | "total" | "elapsedTime" | "deltaX"
        | "deltaY" | "deltaZ" | "deltaMode" | "pointerId" | "pressure" | "tangentialPressure" | "tiltX" | "tiltY"
        | "twist" => {
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
    // UI Events §3.6.3 / §5.6.2: the modifier key names, "Accel" being Control on this platform.
    let k = if k == "Accel" { "Control".to_string() } else { k };
    let Some(m) = crate::api::uievents::MODIFIERS.iter().find(|(n, _)| *n == k).map(|(_, m)| *m) else {
        return Ok(Value::Bool(false));
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
        "compositionevent" => "CompositionEvent",
        "touchevent" => "TouchEvent",
        "dragevent" => "DragEvent",
        "beforeunloadevent" => "BeforeUnloadEvent",
        "textevent" | "devicemotionevent" | "deviceorientationevent" => "Event",
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
        "CompositionEvent",
        "PopStateEvent",
        "HashChangeEvent",
        "ProgressEvent",
        "MessageEvent",
        "PageTransitionEvent",
        "SubmitEvent",
        "FormDataEvent",
        "AnimationEvent",
        "TransitionEvent",
        "StorageEvent",
        "CloseEvent",
        "TouchEvent",
        "ToggleEvent",
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
        if name == "WheelEvent" {
            for (n, v) in [("DOM_DELTA_PIXEL", 0.0), ("DOM_DELTA_LINE", 1.0), ("DOM_DELTA_PAGE", 2.0)] {
                konst(vm, i, n, v);
            }
        }
        if name == "KeyboardEvent" {
            for (n, v) in [("DOM_KEY_LOCATION_STANDARD", 0.0), ("DOM_KEY_LOCATION_LEFT", 1.0), ("DOM_KEY_LOCATION_RIGHT", 2.0), ("DOM_KEY_LOCATION_NUMPAD", 3.0)] {
                konst(vm, i, n, v);
            }
        }
        if name == "InputEvent" {
            op(vm, i.proto, "getTargetRanges", 0, ev_empty_list);
        }
        let legacy_init = match name {
            "UIEvent" => Some(("initUIEvent", 1)),
            "MouseEvent" => Some(("initMouseEvent", 1)),
            "KeyboardEvent" => Some(("initKeyboardEvent", 1)),
            "CompositionEvent" => Some(("initCompositionEvent", 1)),
            _ => None,
        };
        if let Some((m, len)) = legacy_init {
            idl::op_with(vm, i.proto, m, len, ev_init_legacy, s(m));
        }
        if matches!(name, "MouseEvent" | "KeyboardEvent") {
            op(vm, i.proto, "getModifierState", 1, ev_get_modifier_state);
            if name == "MouseEvent" {
                for alias in [("x", "clientX"), ("y", "clientY")] {
                    idl::attr_with(vm, i.proto, alias.0, ev_member, None, s(alias.1));
                }
                // CSSOM View §10: pageX/Y add the scroll offset; offsetX/Y are target-relative while
                // the event is being dispatched.
                for (n, axis) in [("pageX", "x"), ("pageY", "y")] {
                    idl::attr_with(vm, i.proto, n, ev_page, None, s(axis));
                }
                for (n, axis) in [("offsetX", "x"), ("offsetY", "y")] {
                    idl::attr_with(vm, i.proto, n, ev_offset, None, s(axis));
                }
            }
        }
    }

    // BeforeUnloadEvent (HTML §7.1.6): no constructor; its returnValue is a DOMString.
    let bu = interface(vm, "BeforeUnloadEvent", Some(ev), None);
    dom::register_iface("BeforeUnloadEvent", bu);
    attr(vm, bu.proto, "returnValue", ev_unload_return_get, Some(ev_unload_return_set));
}

fn ev_empty_list(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_event(vm, ctx)?;
    Ok(Value::Object(vm.new_array(Vec::new())))
}

fn ev_unload_return_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    match slot(vm, e, E_INIT) {
        Value::Object(o) => match vm.get(o, &PropertyKey::from_str("returnValue"))? {
            v @ Value::String(_) => Ok(v),
            _ => Ok(s("")),
        },
        _ => Ok(s("")),
    }
}

fn ev_unload_return_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    let v = Value::String(vm.to_string(&arg(vm, ctx, 0))?);
    let init = match slot(vm, e, E_INIT) {
        Value::Object(o) => o,
        _ => {
            let o = vm.new_plain_object();
            set_slot(vm, e, E_INIT, Value::Object(o));
            o
        }
    };
    vm.create_data_property(init, PropertyKey::from_str("returnValue"), v)?;
    Ok(Value::Undefined)
}

/// The initialized `clientX`/`clientY` of a mouse event (`axis` "x" or "y").
fn client_coord(vm: &mut Vm, e: Obj, axis: &str) -> JsResult<f64> {
    let name = if axis == "x" { "clientX" } else { "clientY" };
    match slot(vm, e, E_INIT) {
        Value::Object(o) => {
            let v = vm.get(o, &PropertyKey::from_str(name))?;
            if v.is_undefined() {
                Ok(0.0)
            } else {
                vm.to_number(&v)
            }
        }
        _ => Ok(0.0),
    }
}

fn ev_page(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    let axis = callee_str(vm, ctx);
    let c = client_coord(vm, e, &axis)?;
    let (sx, sy) = page(|p| p.scroll);
    Ok(num(c + if axis == "x" { sx } else { sy }))
}

/// `offsetX/Y`: while the dispatch flag is set and the target is an element with a box, the client
/// coordinate relative to that box's origin; otherwise the page coordinate.
fn ev_offset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = this_event(vm, ctx)?;
    let axis = callee_str(vm, ctx);
    let c = client_coord(vm, e, &axis)?;
    if flags(vm, e) & F_DISPATCH != 0 {
        let tv = slot(vm, e, E_TARGET);
        if let Some(n) = node_of(vm, &tv).filter(|n| dom::kind(*n) == dom::NK::Element) {
            if let Some(r) = super::cssom::client_rects(n).first() {
                return Ok(num(c - if axis == "x" { r.0 } else { r.1 }));
            }
        }
    }
    let (sx, sy) = page(|p| p.scroll);
    Ok(num(c + if axis == "x" { sx } else { sy }))
}

/// Points the global binding, `prototype` and `constructor` links at constructor `c`.
fn rewire_ctor(vm: &mut Vm, i: idl::Iface, c: Obj, name: &str) {
    vm.heap.get_mut(c).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(i.proto), 0));
    vm.heap.get_mut(i.proto).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(c), WC));
    let g = vm.realm().global;
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(c), WC));
    root(vm, Value::Object(c));
}
