//! The WebIDL layer (WebIDL §3) of Aether's bindings, over js_core's object model.
//!
//! Every platform object is a js_core object of kind `Internal` whose first two slots are a brand
//! symbol no script can reach and a numeric tag naming the interface family; the remaining slots are
//! the object's internal state (a node id, an event's flags, a collection's root, …). Being GC-traced
//! slots rather than Rust-side maps, they live and die with the object. A native's brand check is
//! therefore exact: a script can forge an object with `__node_id` (the boa-era scheme) but not one with
//! this kind and brand.
//!
//! Interfaces are installed the way WebIDL §3.7 lays them out: an interface object (a function whose
//! [[Call]] throws "Illegal constructor" unless the interface has a constructor; its [[Prototype]] is the
//! parent interface object), an interface prototype object (`constructor`, `@@toStringTag`, its
//! [[Prototype]] the parent's prototype), regular operations as {writable, enumerable, configurable}
//! data properties, attributes as {enumerable, configurable} accessors named `get x` / `set x`, and
//! constants as non-writable, non-configurable data properties on both objects. The global property is
//! {writable, configurable}, non-enumerable.

use js_core::string::JsStr;
use js_core::vm::*;
use std::cell::RefCell;

// ------------------------------------------------------------------------------------------------ brand + tags

thread_local! {
    static BRAND: Sym = Sym::new(Some(JsStr::from_str("aether.platform-object")));
    /// Host-held values (wrappers, interface objects, listeners): indices into `vm.host_roots`.
    static FREE_ROOTS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

pub fn brand() -> Sym {
    BRAND.with(|b| b.clone())
}

pub const T_NODE: u8 = 1;
pub const T_EVENT: u8 = 2;
pub const T_COLL: u8 = 3;
pub const T_TOKENS: u8 = 4;
pub const T_STYLE: u8 = 5;
pub const T_ATTR: u8 = 6;
pub const T_ATTRMAP: u8 = 7;
pub const T_RECT: u8 = 8;
pub const T_DOMEXC: u8 = 9;
pub const T_ETARGET: u8 = 10;
pub const T_IMPL: u8 = 11;
pub const T_COMPSTYLE: u8 = 12;
pub const T_RECTLIST: u8 = 13;
pub const T_LOCATION: u8 = 14;
pub const T_HISTORY: u8 = 15;
pub const T_NAVIGATOR: u8 = 16;
pub const T_STRINGMAP: u8 = 17;
pub const T_MQL: u8 = 18;
pub const T_SCREEN: u8 = 19;

/// A new platform object: `Internal([brand, tag, slots…])` with prototype `proto`.
pub fn host_obj(vm: &mut Vm, proto: Obj, tag: u8, slots: Vec<Value>) -> Obj {
    let mut v = Vec::with_capacity(slots.len() + 2);
    v.push(Value::Symbol(brand()));
    v.push(Value::Number(tag as f64));
    v.extend(slots);
    vm.alloc(ObjectData::new(Some(proto), Kind::Internal(v)))
}

/// The platform-object tag of `o`, if it is one of ours.
pub fn tag_of(vm: &Vm, o: Obj) -> Option<u8> {
    match &vm.heap.get(o).kind {
        Kind::Internal(v) if v.len() >= 2 => match (&v[0], &v[1]) {
            (Value::Symbol(s), Value::Number(n)) if *s == brand() => Some(*n as u8),
            _ => None,
        },
        _ => None,
    }
}

/// Internal slot `i` (0 = the first slot after the brand and tag).
pub fn slot(vm: &Vm, o: Obj, i: usize) -> Value {
    match &vm.heap.get(o).kind {
        Kind::Internal(v) => v.get(i + 2).cloned().unwrap_or(Value::Undefined),
        _ => Value::Undefined,
    }
}

pub fn set_slot(vm: &mut Vm, o: Obj, i: usize, val: Value) {
    if let Kind::Internal(v) = &mut vm.heap.get_mut(o).kind {
        while v.len() <= i + 2 {
            v.push(Value::Undefined);
        }
        v[i + 2] = val;
    }
}

pub fn slot_num(vm: &Vm, o: Obj, i: usize) -> f64 {
    match slot(vm, o, i) {
        Value::Number(n) => n,
        _ => f64::NAN,
    }
}

/// `o` itself, or — for a collection proxy — its branded target.
pub fn unproxy(vm: &Vm, o: Obj) -> Obj {
    if let Kind::Proxy(Some(p)) = &vm.heap.get(o).kind {
        if tag_of(vm, p.target).is_some() {
            return p.target;
        }
    }
    o
}

/// `this` as a platform object of family `tag` (collection proxies unwrapped), else None.
pub fn this_tagged(vm: &Vm, this: &Value, tag: u8) -> Option<Obj> {
    let o = unproxy(vm, this.as_object()?);
    (tag_of(vm, o) == Some(tag)).then_some(o)
}

// ------------------------------------------------------------------------------------------------ host roots

/// Keeps `v` alive for as long as the host holds the returned index.
pub fn root(vm: &mut Vm, v: Value) -> usize {
    if let Some(i) = FREE_ROOTS.with(|f| f.borrow_mut().pop()) {
        if i < vm.host_roots.len() {
            vm.host_roots[i] = v;
            return i;
        }
    }
    vm.host_roots.push(v);
    vm.host_roots.len() - 1
}

pub fn unroot(vm: &mut Vm, i: usize) {
    if i < vm.host_roots.len() {
        vm.host_roots[i] = Value::Undefined;
        FREE_ROOTS.with(|f| f.borrow_mut().push(i));
    }
}

pub fn rooted(vm: &Vm, i: usize) -> Value {
    vm.host_roots.get(i).cloned().unwrap_or(Value::Undefined)
}

pub fn reset_roots() {
    FREE_ROOTS.with(|f| f.borrow_mut().clear());
}

// ------------------------------------------------------------------------------------------------ interfaces

#[derive(Clone, Copy, Debug)]
pub struct Iface {
    pub ctor: Obj,
    pub proto: Obj,
}

fn illegal_ctor(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    vm.throw_type("Illegal constructor")
}

/// WebIDL §3.7.1/§3.7.3: the interface object and interface prototype object for `name`, wired under
/// `parent` and exposed on the global. `ctor` is the constructor operation, when the interface has one.
pub fn interface(vm: &mut Vm, name: &str, parent: Option<Iface>, ctor: Option<(NativeFn, u32)>) -> Iface {
    let op = vm.intr().object_proto;
    let fp = vm.intr().function_proto;
    let proto = vm.new_object(Some(parent.map(|p| p.proto).unwrap_or(op)));
    let (f, len) = ctor.unwrap_or((illegal_ctor, 0));
    let c = vm.make_native_with(name, len, f, true, Some(parent.map(|p| p.ctor).unwrap_or(fp)), Vec::new());
    vm.heap.get_mut(c).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(proto), 0));
    vm.heap.get_mut(proto).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(c), WC));
    let tag = PropertyKey::Sym(vm.wk.to_string_tag.clone());
    vm.heap.get_mut(proto).props.insert(tag, Prop::data(Value::str(name), C));
    let g = vm.realm().global;
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(c), WC));
    root(vm, Value::Object(c));
    root(vm, Value::Object(proto));
    Iface { ctor: c, proto }
}

/// A regular operation (WebIDL §3.7.6): {writable, enumerable, configurable}.
pub fn op(vm: &mut Vm, o: Obj, name: &str, len: u32, f: NativeFn) -> Obj {
    let fo = vm.make_native(name, len, f, false);
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WEC));
    fo
}

/// An operation whose native reads `data` from its slot 0 (one native serving many names).
pub fn op_with(vm: &mut Vm, o: Obj, name: &str, len: u32, f: NativeFn, data: Value) -> Obj {
    let fp = vm.intr().function_proto;
    let fo = vm.make_native_with(name, len, f, false, Some(fp), vec![data]);
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WEC));
    fo
}

/// An attribute (WebIDL §3.7.5): an {enumerable, configurable} accessor.
pub fn attr(vm: &mut Vm, o: Obj, name: &str, get: NativeFn, set: Option<NativeFn>) {
    let g = vm.make_native(&format!("get {name}"), 0, get, false);
    let s = set.map(|f| vm.make_native(&format!("set {name}"), 1, f, false));
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str(name), Prop { slot: Slot::Accessor(Some(g), s), flags: E | C });
}

/// An attribute whose getter/setter natives read `data` from slot 0 (reflected attributes, CSS
/// properties: one native pair per kind, the name in the slot).
pub fn attr_with(vm: &mut Vm, o: Obj, name: &str, get: NativeFn, set: Option<NativeFn>, data: Value) {
    let fp = vm.intr().function_proto;
    let g = vm.make_native_with(&format!("get {name}"), 0, get, false, Some(fp), vec![data.clone()]);
    let s = set.map(|f| vm.make_native_with(&format!("set {name}"), 1, f, false, Some(fp), vec![data]));
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str(name), Prop { slot: Slot::Accessor(Some(g), s), flags: E | C });
}

/// A [LegacyUnforgeable] attribute: an own, non-configurable accessor on the instance.
pub fn unforgeable_attr(vm: &mut Vm, o: Obj, name: &str, getter: Obj) {
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str(name), Prop { slot: Slot::Accessor(Some(getter), None), flags: E });
}

/// A constant (WebIDL §3.7.4) on both the interface object and the prototype.
pub fn konst(vm: &mut Vm, i: Iface, name: &str, v: f64) {
    vm.heap.get_mut(i.ctor).props.insert(PropertyKey::from_str(name), Prop::data(Value::Number(v), E));
    vm.heap.get_mut(i.proto).props.insert(PropertyKey::from_str(name), Prop::data(Value::Number(v), E));
}

/// A plain data property (writable, configurable, non-enumerable unless `flags` says otherwise).
pub fn data(vm: &mut Vm, o: Obj, name: &str, v: Value, flags: u8) {
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str(name), Prop::data(v, flags));
}

/// The callee's slot-0 datum as a Rust string (the name an `*_with` native serves).
pub fn callee_str(vm: &Vm, ctx: &CallCtx) -> String {
    match vm.native_slot(ctx.callee, 0) {
        Value::String(s) => s.to_rust(),
        _ => String::new(),
    }
}

// ------------------------------------------------------------------------------------------------ arguments

pub fn arg(vm: &Vm, ctx: &CallCtx, i: usize) -> Value {
    vm.arg(ctx, i)
}

/// WebIDL overload resolution step: too few arguments is a TypeError.
pub fn need(vm: &mut Vm, ctx: &CallCtx, n: usize, iface: &str, method: &str) -> JsResult<()> {
    if ctx.argc < n {
        let msg = format!(
            "Failed to execute '{method}' on '{iface}': {n} argument{} required, but only {} present.",
            if n == 1 { "" } else { "s" },
            ctx.argc
        );
        return vm.throw_type(&msg);
    }
    Ok(())
}

/// DOMString conversion (ToString).
pub fn string(vm: &mut Vm, v: &Value) -> JsResult<String> {
    Ok(vm.to_string(v)?.to_rust())
}

/// `DOMString?`-ish: null and undefined map to `None`.
pub fn opt_string(vm: &mut Vm, v: &Value) -> JsResult<Option<String>> {
    if v.is_nullish() {
        return Ok(None);
    }
    Ok(Some(string(vm, v)?))
}

/// [LegacyNullToEmptyString] DOMString.
pub fn string_null_empty(vm: &mut Vm, v: &Value) -> JsResult<String> {
    if v.is_null() {
        return Ok(String::new());
    }
    string(vm, v)
}

/// `unsigned long` (WebIDL §3.2.4.8, modular).
pub fn unsigned_long(vm: &mut Vm, v: &Value) -> JsResult<u32> {
    vm.to_uint32(v)
}

/// `long`.
pub fn long(vm: &mut Vm, v: &Value) -> JsResult<i32> {
    vm.to_int32(v)
}

pub fn boolean(vm: &Vm, v: &Value) -> bool {
    vm.to_boolean(v)
}

pub fn s(v: &str) -> Value {
    Value::String(JsStr::from_str(v))
}

pub fn num(n: f64) -> Value {
    Value::Number(n)
}

/// Property lookup by Rust string.
pub fn get(vm: &mut Vm, o: Obj, name: &str) -> JsResult<Value> {
    vm.get(o, &PropertyKey::from_str(name))
}

pub fn set(vm: &mut Vm, o: Obj, name: &str, v: Value) -> JsResult<()> {
    vm.set_prop(o, PropertyKey::from_str(name), v, true)
}

/// A dictionary member (WebIDL §3.2.17): `undefined` when the dictionary is absent or lacks it.
pub fn dict_member(vm: &mut Vm, dict: &Value, name: &str) -> JsResult<Value> {
    match dict {
        Value::Object(o) => get(vm, *o, name),
        Value::Undefined | Value::Null => Ok(Value::Undefined),
        _ => vm.throw_type("dictionary argument is not an object"),
    }
}

// ------------------------------------------------------------------------------------------------ DOMException

thread_local! {
    static DOMEXC_PROTO: std::cell::Cell<Option<Obj>> = const { std::cell::Cell::new(None) };
}

/// The legacy code of a DOMException name (WebIDL §2.8.1 table).
pub fn exception_code(name: &str) -> u16 {
    match name {
        "IndexSizeError" => 1,
        "HierarchyRequestError" => 3,
        "WrongDocumentError" => 4,
        "InvalidCharacterError" => 5,
        "NoModificationAllowedError" => 7,
        "NotFoundError" => 8,
        "NotSupportedError" => 9,
        "InUseAttributeError" => 10,
        "InvalidStateError" => 11,
        "SyntaxError" => 12,
        "InvalidModificationError" => 13,
        "NamespaceError" => 14,
        "InvalidAccessError" => 15,
        "TypeMismatchError" => 17,
        "SecurityError" => 18,
        "NetworkError" => 19,
        "AbortError" => 20,
        "URLMismatchError" => 21,
        "QuotaExceededError" => 22,
        "TimeoutError" => 23,
        "InvalidNodeTypeError" => 24,
        "DataCloneError" => 25,
        _ => 0,
    }
}

/// A new DOMException value.
pub fn dom_exception(vm: &mut Vm, name: &str, message: &str) -> Value {
    let proto = DOMEXC_PROTO.with(|p| p.get()).unwrap_or_else(|| vm.intr().error_proto);
    let o = host_obj(vm, proto, T_DOMEXC, vec![s(name), s(message)]);
    let trace = vm.stack_trace();
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str("stack"), Prop::data(Value::String(trace), WC));
    Value::Object(o)
}

pub fn throw_dom<T>(vm: &mut Vm, name: &str, message: &str) -> JsResult<T> {
    Err(dom_exception(vm, name, message))
}

fn domexc_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'DOMException': Please use the 'new' operator");
    }
    let m = arg(vm, ctx, 0);
    let message = if m.is_undefined() { String::new() } else { string(vm, &m)? };
    let n = arg(vm, ctx, 1);
    let name = if n.is_undefined() { "Error".to_string() } else { string(vm, &n)? };
    let default = DOMEXC_PROTO.with(|p| p.get()).unwrap_or_else(|| vm.intr().error_proto);
    let proto = proto_from_new_target(vm, &ctx.new_target, default)?;
    Ok(Value::Object(host_obj(vm, proto, T_DOMEXC, vec![s(&name), s(&message)])))
}

fn domexc_name(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    match this_tagged(vm, &ctx.this, T_DOMEXC) {
        Some(o) => Ok(slot(vm, o, 0)),
        None => vm.throw_type("Illegal invocation"),
    }
}
fn domexc_message(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    match this_tagged(vm, &ctx.this, T_DOMEXC) {
        Some(o) => Ok(slot(vm, o, 1)),
        None => vm.throw_type("Illegal invocation"),
    }
}
fn domexc_code(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    match this_tagged(vm, &ctx.this, T_DOMEXC) {
        Some(o) => {
            let n = match slot(vm, o, 0) {
                Value::String(s) => s.to_rust(),
                _ => String::new(),
            };
            Ok(num(exception_code(&n) as f64))
        }
        None => vm.throw_type("Illegal invocation"),
    }
}

/// GetPrototypeFromConstructor for host constructors (so `class X extends Event` works).
pub fn proto_from_new_target(vm: &mut Vm, nt: &Value, default: Obj) -> JsResult<Obj> {
    if let Value::Object(c) = nt {
        if let Value::Object(p) = vm.get(*c, &PropertyKey::from_str("prototype"))? {
            return Ok(p);
        }
    }
    Ok(default)
}

pub fn install_domexception(vm: &mut Vm) {
    let i = interface(vm, "DOMException", None, Some((domexc_ctor, 0)));
    // DOMException.prototype.[[Prototype]] is %Error.prototype% (WebIDL §3.14.1).
    let ep = vm.intr().error_proto;
    vm.heap.get_mut(i.proto).proto = Some(ep);
    attr(vm, i.proto, "name", domexc_name, None);
    attr(vm, i.proto, "message", domexc_message, None);
    attr(vm, i.proto, "code", domexc_code, None);
    for (n, c) in [
        ("INDEX_SIZE_ERR", 1),
        ("DOMSTRING_SIZE_ERR", 2),
        ("HIERARCHY_REQUEST_ERR", 3),
        ("WRONG_DOCUMENT_ERR", 4),
        ("INVALID_CHARACTER_ERR", 5),
        ("NO_DATA_ALLOWED_ERR", 6),
        ("NO_MODIFICATION_ALLOWED_ERR", 7),
        ("NOT_FOUND_ERR", 8),
        ("NOT_SUPPORTED_ERR", 9),
        ("INUSE_ATTRIBUTE_ERR", 10),
        ("INVALID_STATE_ERR", 11),
        ("SYNTAX_ERR", 12),
        ("INVALID_MODIFICATION_ERR", 13),
        ("NAMESPACE_ERR", 14),
        ("INVALID_ACCESS_ERR", 15),
        ("VALIDATION_ERR", 16),
        ("TYPE_MISMATCH_ERR", 17),
        ("SECURITY_ERR", 18),
        ("NETWORK_ERR", 19),
        ("ABORT_ERR", 20),
        ("URL_MISMATCH_ERR", 21),
        ("QUOTA_EXCEEDED_ERR", 22),
        ("TIMEOUT_ERR", 23),
        ("INVALID_NODE_TYPE_ERR", 24),
        ("DATA_CLONE_ERR", 25),
    ] {
        konst(vm, i, n, c as f64);
    }
    DOMEXC_PROTO.with(|p| p.set(Some(i.proto)));
}

pub fn reset() {
    DOMEXC_PROTO.with(|p| p.set(None));
    reset_roots();
}
