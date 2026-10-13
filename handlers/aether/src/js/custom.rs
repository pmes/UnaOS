//! Custom elements' binding (AETHERSHADOW, LEDGER SR82) over `dom::custom`: the
//! CustomElementRegistry (`window.customElements`: define / get / getName / whenDefined / upgrade,
//! HTML §4.13.4), the [HTMLConstructor] steps (§3.2.3) as the [[Call]] of every HTML element
//! interface object, "upgrade an element" (§4.13.5), "create an element" for `createElement`
//! (synchronous custom elements flag set), and [`flush`]: "invoke custom element reactions" over the
//! element queue (§4.13.6), which [`super::mutation::settle`] runs at the end of every mutation
//! algorithm and of every [CEReactions] operation here.

use super::dom::{self, iface, main_doc, node_document, node_of, with_doc, wrap};
use super::idl::*;
use crate::dom::custom::{self as ce, Arg, Callback, CeState, Definition, Reaction};
use html_core::{Namespace, NodeId};
use js_core::vm::*;

/// Platform-object tag of the registry.
pub const T_CEREG: u8 = 42;

const FAIL_DEFINE: &str = "Failed to execute 'define' on 'CustomElementRegistry': ";

// ---------------------------------------------------------------------------------------------------
// Reactions
// ---------------------------------------------------------------------------------------------------

/// HTML "invoke custom element reactions" over the element queue. An element already being processed
/// by an outer flush (a reaction that mutated the DOM) is left to that flush, so one element's
/// reactions run in order.
pub fn flush(vm: &mut Vm) {
    // AETHERWPT3 (SR138): form-associated custom elements whose form owner / disabled state changed.
    super::internals::sync();
    if !ce::pending() || super::engine_poisoned() {
        return;
    }
    for _ in 0..64 {
        let els = ce::take_queue();
        if els.is_empty() {
            break;
        }
        for el in els {
            if !ce::begin(el) {
                continue;
            }
            while let Some(r) = ce::pop_reaction(el) {
                match r {
                    Reaction::Upgrade(def) => {
                        if let Some(Err(e)) = super::enter(vm, "ce-upgrade", |vm| upgrade(vm, el, def)) {
                            super::events::report_exception(vm, &e);
                        }
                    }
                    Reaction::Callback(cb, args) => invoke_callback(vm, el, cb, &args),
                }
            }
            ce::end(el);
        }
    }
}

fn invoke_callback(vm: &mut Vm, el: NodeId, cb: Callback, args: &[Arg]) {
    let Some(def) = ce::element_definition(el).and_then(ce::definition) else { return };
    let Some(r) = def.callbacks[cb as usize] else { return };
    let f = rooted(vm, r);
    let this = wrap(vm, el.0);
    let mut vals = Vec::with_capacity(args.len());
    for a in args {
        vals.push(match a {
            Arg::Null => Value::Null,
            Arg::Str(t) => s(t),
            Arg::Node(n) => wrap(vm, n.0),
            Arg::Bool(b) => Value::Bool(*b),
        });
    }
    super::invoke(vm, cb.name(), &f, &this, &vals);
}

/// HTML §4.13.5 "upgrade an element".
fn upgrade(vm: &mut Vm, el: NodeId, def_i: usize) -> JsResult<()> {
    let state = with_doc(|d| ce::state(d, el));
    if !matches!(state, CeState::Undefined | CeState::Uncustomized) {
        return Ok(());
    }
    let Some(def) = ce::definition(def_i) else { return Ok(()) };
    ce::set_definition(el, Some(def_i));
    ce::set_state(el, CeState::Failed);
    let attrs: Vec<(String, String, Namespace)> = with_doc(|d| {
        d.element(el).map(|e| e.attrs.iter().map(|a| (a.local.clone(), a.value.clone(), a.ns)).collect()).unwrap_or_default()
    });
    for (local, value, ns) in attrs {
        let ns_arg = if ns == Namespace::None { Arg::Null } else { Arg::Str(ns.url().to_string()) };
        ce::enqueue_callback(el, Callback::AttributeChanged, vec![Arg::Str(local), Arg::Null, Arg::Str(value), ns_arg]);
    }
    if dom::is_connected(el.0) {
        ce::enqueue_callback(el, Callback::Connected, Vec::new());
    }
    ce::push_construction(def_i, el);
    if def.disable_shadow && with_doc(|d| crate::dom::shadow::shadow_root_of(d, el)).is_some() {
        ce::pop_construction(def_i);
        ce::set_definition(el, None);
        ce::clear_reactions(el);
        return throw_dom(vm, "NotSupportedError", "The element's shadow root was attached before its definition disabled shadow.");
    }
    ce::set_state(el, CeState::Precustomized);
    let ctor = rooted(vm, def.constructor);
    let r = vm.construct(&ctor, &[], None);
    ce::pop_construction(def_i);
    let r = match r {
        Ok(v) if node_of(vm, &v) == Some(el.0) => Ok(()),
        Ok(_) => vm.throw_type("Failed to upgrade the custom element: the constructor did not return the element (call super() first and return nothing)."),
        Err(e) => Err(e),
    };
    if let Err(e) = r {
        ce::set_definition(el, None);
        ce::clear_reactions(el);
        return Err(e);
    }
    ce::set_state(el, CeState::Custom);
    super::internals::note_custom(el);
    Ok(())
}

// ---------------------------------------------------------------------------------------------------
// [HTMLConstructor]
// ---------------------------------------------------------------------------------------------------

/// The HTML element constructor steps (HTML §3.2.3), installed as every HTML element interface
/// object's [[Call]]; the interface's name rides in the native's slot 0.
fn html_constructor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let active = callee_str(vm, ctx);
    let nt = match &ctx.new_target {
        Value::Object(o) if *o != ctx.callee => *o,
        _ => return vm.throw_type("Illegal constructor"),
    };
    let found = (0..ce::definition_count()).find(|&i| {
        ce::definition(i).is_some_and(|d| matches!(rooted(vm, d.constructor), Value::Object(o) if o == nt))
    });
    let Some(def_i) = found else { return vm.throw_type("Illegal constructor") };
    let def = ce::definition(def_i).expect("definition");
    let is_value = if def.is_autonomous() {
        if active != "HTMLElement" {
            return vm.throw_type("Illegal constructor: autonomous custom elements must extend HTMLElement");
        }
        None
    } else {
        if super::html::interface_for_tag(&def.local_name) != active {
            return vm.throw_type("Illegal constructor: the customized built-in element's interface does not match");
        }
        Some(def.name.clone())
    };
    let default = iface(&active).map(|i| i.proto).unwrap_or_else(|| dom::proto("HTMLElement"));
    let proto = match get(vm, nt, "prototype")? {
        Value::Object(p) => p,
        _ => default,
    };
    match ce::last_construction(def_i) {
        None => {
            let id = dom::create_element(main_doc(), Namespace::Html, &def.local_name);
            let n = NodeId(id);
            ce::set_state(n, CeState::Custom);
            ce::set_definition(n, Some(def_i));
            super::internals::note_custom(n);
            ce::set_is_value(n, is_value);
            let w = wrap(vm, id);
            if let Value::Object(o) = &w {
                vm.heap.get_mut(*o).proto = Some(proto);
            }
            Ok(w)
        }
        Some(None) => vm.throw_type("Failed to construct the custom element: this element was already constructed (the constructor may call super() only once)."),
        Some(Some(el)) => {
            let w = wrap(vm, el.0);
            if let Value::Object(o) = &w {
                vm.heap.get_mut(*o).proto = Some(proto);
            }
            ce::mark_constructed(def_i);
            Ok(w)
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// create an element (createElement / createElementNS)
// ---------------------------------------------------------------------------------------------------

/// The `is` of `createElement`'s `(DOMString or ElementCreationOptions)` argument.
pub fn creation_is(vm: &mut Vm, options: &Value) -> JsResult<Option<String>> {
    match options {
        Value::Object(_) => {
            let v = dict_member(vm, options, "is")?;
            if v.is_undefined() { Ok(None) } else { Ok(Some(string(vm, &v)?)) }
        }
        _ => Ok(None),
    }
}

/// HTML "create an element" (synchronous custom elements flag set) for the element `id` that
/// `createElement` just made in document `doc`: a defined autonomous name constructs through its
/// constructor (whose element replaces `id`); a customized built-in upgrades in place; an undefined
/// valid custom element name or `is` value leaves the element "undefined". Returns the element.
pub fn created(vm: &mut Vm, doc: usize, id: usize, is: Option<String>) -> usize {
    let n = NodeId(id);
    let (ns, local) = with_doc(|d| d.element(n).map(|e| (e.ns, e.local.clone()))).unwrap_or((Namespace::None, String::new()));
    if ns != Namespace::Html {
        return id;
    }
    if is.is_some() {
        ce::set_is_value(n, is.clone());
    }
    let Some(def_i) = ce::look_up(ns, &local, is.as_deref()) else {
        if ce::valid_custom_element_name(&local) || is.is_some() {
            ce::set_state(n, CeState::Undefined);
        }
        return id;
    };
    let Some(def) = ce::definition(def_i) else { return id };
    if !def.is_autonomous() {
        ce::set_state(n, CeState::Undefined);
        if let Some(Err(e)) = super::enter(vm, "ce-upgrade", |vm| upgrade(vm, n, def_i)) {
            super::events::report_exception(vm, &e);
        }
        flush(vm);
        return id;
    }
    let ctor = rooted(vm, def.constructor);
    let r = super::enter(vm, "ce-construct", |vm| vm.construct(&ctor, &[], None));
    let checked: Result<usize, Value> = match r {
        Some(Ok(v)) => match node_of(vm, &v) {
            Some(made) => {
                let ok = with_doc(|d| {
                    d.element(NodeId(made)).is_some_and(|e| e.ns == Namespace::Html && e.local == local && e.attrs.is_empty())
                        && d.first_child(NodeId(made)).is_none()
                        && d.parent(NodeId(made)).is_none()
                }) && node_document(made) == doc;
                if ok { Ok(made) } else { Err(dom_exception(vm, "NotSupportedError", "The result of constructing a custom element must be a fresh element of the requested name.")) }
            }
            None => Err(vm.type_error("The result of constructing a custom element must be an HTMLElement.")),
        },
        Some(Err(e)) => Err(e),
        None => Err(Value::Undefined),
    };
    match checked {
        Ok(made) => made,
        Err(e) => {
            if !e.is_undefined() {
                super::events::report_exception(vm, &e);
            }
            ce::set_state(n, CeState::Failed);
            let w = wrap(vm, id);
            if let (Value::Object(o), Some(u)) = (&w, iface("HTMLUnknownElement")) {
                vm.heap.get_mut(*o).proto = Some(u.proto);
            }
            id
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// CustomElementRegistry
// ---------------------------------------------------------------------------------------------------

fn this_registry(vm: &mut Vm, ctx: &CallCtx) -> JsResult<()> {
    match this_tagged(vm, &ctx.this, T_CEREG) {
        Some(_) => Ok(()),
        None => vm.throw_type("Illegal invocation"),
    }
}

fn callable_or_none(vm: &mut Vm, proto: Obj, name: &str) -> JsResult<Option<usize>> {
    let v = get(vm, proto, name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    if !vm.is_callable(&v) {
        return vm.throw_type(&format!("{FAIL_DEFINE}The '{name}' property on the prototype is not a function."));
    }
    Ok(Some(root(vm, v)))
}

fn string_sequence(vm: &mut Vm, v: &Value, what: &str) -> JsResult<Vec<String>> {
    if !matches!(v, Value::Object(_)) {
        return vm.throw_type(&format!("{FAIL_DEFINE}'{what}' cannot be converted to a sequence."));
    }
    let items = vm.iterable_to_list(v)?;
    let mut out = Vec::with_capacity(items.len());
    for i in items {
        out.push(string(vm, &i)?);
    }
    Ok(out)
}

/// `define(name, constructor, options)` (HTML §4.13.4).
fn define(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_registry(vm, ctx)?;
    need(vm, ctx, 2, "CustomElementRegistry", "define")?;
    let name = string(vm, &arg(vm, ctx, 0))?;
    let ctor = arg(vm, ctx, 1);
    if !vm.is_callable(&ctor) {
        return vm.throw_type(&format!("{FAIL_DEFINE}parameter 2 is not of type 'CustomElementConstructor'."));
    }
    let options = arg(vm, ctx, 2);
    let ext_v = dict_member(vm, &options, "extends")?;
    let extends = if ext_v.is_undefined() { None } else { Some(string(vm, &ext_v)?) };
    if !vm.is_constructor(&ctor) {
        return vm.throw_type(&format!("{FAIL_DEFINE}The provided constructor is not a constructor."));
    }
    if !ce::valid_custom_element_name(&name) {
        return throw_dom(vm, "SyntaxError", &format!("{FAIL_DEFINE}\"{name}\" is not a valid custom element name"));
    }
    if ce::by_name(&name).is_some() {
        return throw_dom(vm, "NotSupportedError", &format!("{FAIL_DEFINE}the name \"{name}\" has already been used with this registry"));
    }
    let ctor_obj = ctor.as_object();
    let same = (0..ce::definition_count())
        .any(|i| ce::definition(i).is_some_and(|d| rooted(vm, d.constructor).as_object() == ctor_obj));
    if same {
        return throw_dom(vm, "NotSupportedError", &format!("{FAIL_DEFINE}this constructor has already been used with this registry"));
    }
    let mut local = name.clone();
    if let Some(ext) = &extends {
        if ce::valid_custom_element_name(ext) {
            return throw_dom(vm, "NotSupportedError", &format!("{FAIL_DEFINE}\"{ext}\" is a valid custom element name"));
        }
        if super::html::interface_for_tag(ext) == "HTMLUnknownElement" {
            return throw_dom(vm, "NotSupportedError", &format!("{FAIL_DEFINE}\"{ext}\" is an HTMLUnknownElement"));
        }
        local = ext.clone();
    }
    if ce::running() {
        return throw_dom(vm, "NotSupportedError", &format!("{FAIL_DEFINE}this registry is already running an element definition"));
    }
    ce::set_running(true);
    let ctor_o = ctor_obj.expect("constructor is an object");
    let read = (|| -> JsResult<(Vec<String>, [Option<usize>; 8], bool, bool, bool)> {
        let proto = match get(vm, ctor_o, "prototype")? {
            Value::Object(p) => p,
            _ => return vm.throw_type(&format!("{FAIL_DEFINE}The prototype of this constructor is not an object.")),
        };
        let mut callbacks: [Option<usize>; 8] = [None; 8];
        callbacks[Callback::Connected as usize] = callable_or_none(vm, proto, "connectedCallback")?;
        callbacks[Callback::Disconnected as usize] = callable_or_none(vm, proto, "disconnectedCallback")?;
        callbacks[Callback::Adopted as usize] = callable_or_none(vm, proto, "adoptedCallback")?;
        if let Some(r) = callable_or_none(vm, proto, "connectedMoveCallback")? {
            unroot(vm, r);
        }
        callbacks[Callback::AttributeChanged as usize] = callable_or_none(vm, proto, "attributeChangedCallback")?;
        let mut observed = Vec::new();
        if callbacks[Callback::AttributeChanged as usize].is_some() {
            let v = get(vm, ctor_o, "observedAttributes")?;
            if !v.is_undefined() {
                observed = string_sequence(vm, &v, "observedAttributes")?;
            }
        }
        let mut disable_internals = false;
        let mut disable_shadow = false;
        let v = get(vm, ctor_o, "disabledFeatures")?;
        if !v.is_undefined() {
            let f = string_sequence(vm, &v, "disabledFeatures")?;
            disable_internals = f.iter().any(|x| x == "internals");
            disable_shadow = f.iter().any(|x| x == "shadow");
        }
        let v = get(vm, ctor_o, "formAssociated")?;
        let form_associated = boolean(vm, &v);
        if form_associated {
            // AETHERWPT3 (SR138): kept (they were read and dropped): js/internals.rs enqueues them.
            for (cb, n) in [
                (Callback::FormAssociated, "formAssociatedCallback"),
                (Callback::FormReset, "formResetCallback"),
                (Callback::FormDisabled, "formDisabledCallback"),
                (Callback::FormStateRestore, "formStateRestoreCallback"),
            ] {
                callbacks[cb as usize] = callable_or_none(vm, proto, n)?;
            }
        }
        Ok((observed, callbacks, disable_internals, disable_shadow, form_associated))
    })();
    ce::set_running(false);
    let (observed, callbacks, disable_internals, disable_shadow, form_associated) = read?;
    let constructor = root(vm, ctor.clone());
    let def_i = ce::define(Definition {
        name: name.clone(),
        local_name: local,
        constructor,
        observed,
        callbacks,
        construction_stack: Vec::new(),
        disable_shadow,
        disable_internals,
        form_associated,
    });
    for el in with_doc(|d| ce::upgrade_candidates(d, def_i)) {
        ce::enqueue(el, Reaction::Upgrade(def_i));
    }
    if let Some(r) = ce::take_when_defined(&name) {
        if let Value::Object(p) = rooted(vm, r) {
            let _ = js_core::builtins::promise::resolve_promise(vm, p, ctor.clone());
        }
        unroot(vm, r);
    }
    super::mutation::settle(vm);
    Ok(Value::Undefined)
}

fn registry_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_registry(vm, ctx)?;
    need(vm, ctx, 1, "CustomElementRegistry", "get")?;
    let name = string(vm, &arg(vm, ctx, 0))?;
    match ce::by_name(&name).and_then(ce::definition) {
        Some(d) => Ok(rooted(vm, d.constructor)),
        None => Ok(Value::Undefined),
    }
}

fn registry_get_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_registry(vm, ctx)?;
    need(vm, ctx, 1, "CustomElementRegistry", "getName")?;
    let c = arg(vm, ctx, 0);
    if !vm.is_callable(&c) {
        return vm.throw_type("Failed to execute 'getName' on 'CustomElementRegistry': parameter 1 is not of type 'CustomElementConstructor'.");
    }
    for i in 0..ce::definition_count() {
        if let Some(d) = ce::definition(i) {
            if rooted(vm, d.constructor).as_object() == c.as_object() {
                return Ok(s(&d.name));
            }
        }
    }
    Ok(Value::Null)
}

fn registry_when_defined(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_registry(vm, ctx)?;
    need(vm, ctx, 1, "CustomElementRegistry", "whenDefined")?;
    let name = string(vm, &arg(vm, ctx, 0))?;
    use js_core::builtins::promise::{new_promise, reject_promise, resolve_promise};
    if !ce::valid_custom_element_name(&name) {
        let p = new_promise(vm);
        let e = dom_exception(vm, "SyntaxError", &format!("Failed to execute 'whenDefined' on 'CustomElementRegistry': \"{name}\" is not a valid custom element name"));
        reject_promise(vm, p, e);
        return Ok(Value::Object(p));
    }
    if let Some(d) = ce::by_name(&name).and_then(ce::definition) {
        let p = new_promise(vm);
        let c = rooted(vm, d.constructor);
        resolve_promise(vm, p, c)?;
        return Ok(Value::Object(p));
    }
    if let Some(r) = ce::when_defined(&name) {
        return Ok(rooted(vm, r));
    }
    let p = new_promise(vm);
    let r = root(vm, Value::Object(p));
    ce::set_when_defined(&name, r);
    Ok(Value::Object(p))
}

/// `upgrade(root)`: try to upgrade every shadow-including inclusive descendant element of `root`.
fn registry_upgrade(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    this_registry(vm, ctx)?;
    need(vm, ctx, 1, "CustomElementRegistry", "upgrade")?;
    let r = dom::node_arg(vm, ctx, 0, "CustomElementRegistry", "upgrade")?;
    with_doc(|d| {
        for n in crate::dom::shadow::shadow_including_descendants(d, NodeId(r), true) {
            if d.element(n).is_some() {
                ce::try_upgrade(d, n);
            }
        }
    });
    super::mutation::settle(vm);
    Ok(Value::Undefined)
}

fn custom_elements_get(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(dom::cached(vm, main_doc(), SO_REGISTRY, |vm| {
        let p = iface("CustomElementRegistry").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
        host_obj(vm, p, T_CEREG, Vec::new())
    }))
}

/// The [SameObject] slot of the registry on the main document (beyond dom.rs's SO_* range).
const SO_REGISTRY: u8 = 60;

/// Installs CustomElementRegistry, `window.customElements` and the [HTMLConstructor] steps (after
/// window::install: replaces the platform.rs stub).
pub fn install(vm: &mut Vm) {
    let i = interface(vm, "CustomElementRegistry", None, None);
    dom::register_iface("CustomElementRegistry", i);
    op(vm, i.proto, "define", 2, define);
    op(vm, i.proto, "get", 1, registry_get);
    op(vm, i.proto, "getName", 1, registry_get_name);
    op(vm, i.proto, "whenDefined", 1, registry_when_defined);
    op(vm, i.proto, "upgrade", 1, registry_upgrade);
    // Window's attributes are own accessors of the global (WebIDL §3.7.2 [Global]).
    let g = vm.realm().global;
    let getter = vm.make_native("get customElements", 0, custom_elements_get, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("customElements"), Prop { slot: Slot::Accessor(Some(getter), None), flags: E | C });
    // [HTMLConstructor] on every HTML element interface object (HTMLUnknownElement has none).
    let names: Vec<&'static str> = super::page(|p| {
        p.ifaces.keys().copied().filter(|n| n.starts_with("HTML") && n.ends_with("Element") && *n != "HTMLUnknownElement").collect()
    });
    for n in names {
        let Some(it) = iface(n) else { continue };
        if let Kind::Native(nd) = &mut vm.heap.get_mut(it.ctor).kind {
            nd.f = html_constructor;
            nd.slots = vec![s(n)];
        }
    }
}
