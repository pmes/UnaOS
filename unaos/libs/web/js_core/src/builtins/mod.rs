//! The ECMAScript standard library (§19–§28). `init_realm` creates every intrinsic of a realm and the global
//! object's properties.

pub mod array;
pub mod bigint;
pub mod boolean;
pub mod date;
pub mod error;
pub mod function;
pub mod generator;
pub mod global;
pub mod iterator;
pub mod json;
pub mod map;
pub mod math;
pub mod number;
pub mod object;
pub mod promise;
pub mod proxy;
pub mod reflect;
pub mod regexp;
pub mod string;
pub mod symbol;
pub mod typedarray;
pub mod weak;

use crate::string::JsStr;
use crate::vm::*;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;

/// Define a built-in method (writable, non-enumerable, configurable).
pub fn method(vm: &mut Vm, o: Obj, name: &str, len: u32, f: NativeFn) -> Obj {
    let fo = vm.make_native(name, len, f, false);
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    fo
}

/// A method keyed by a well-known symbol (name "[Symbol.x]").
pub fn method_sym(vm: &mut Vm, o: Obj, sym: Sym, name: &str, len: u32, f: NativeFn, flags: u8) -> Obj {
    let fo = vm.make_native(name, len, f, false);
    vm.heap.get_mut(o).props.insert(PropertyKey::Sym(sym), Prop::data(Value::Object(fo), flags));
    fo
}

/// An accessor property with a native getter (and optional setter).
pub fn accessor(vm: &mut Vm, o: Obj, key: PropertyKey, name: &str, get: Option<NativeFn>, set: Option<NativeFn>, flags: u8) {
    let g = get.map(|f| vm.make_native(&alloc::format!("get {}", name), 0, f, false));
    let s = set.map(|f| vm.make_native(&alloc::format!("set {}", name), 1, f, false));
    vm.heap.get_mut(o).props.insert(key, Prop { slot: Slot::Accessor(g, s), flags });
}

pub fn value(vm: &mut Vm, o: Obj, name: &str, v: Value, flags: u8) {
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str(name), Prop::data(v, flags));
}

pub fn value_key(vm: &mut Vm, o: Obj, key: PropertyKey, v: Value, flags: u8) {
    vm.heap.get_mut(o).props.insert(key, Prop::data(v, flags));
}

/// Create a constructor function with a prototype object linked both ways.
pub fn ctor(vm: &mut Vm, name: &str, len: u32, f: NativeFn, proto: Obj) -> Obj {
    let c = vm.make_native(name, len, f, true);
    vm.heap.get_mut(c).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(proto), 0));
    vm.heap.get_mut(proto).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(c), WC));
    c
}

pub fn global(vm: &mut Vm, name: &str, v: Value) {
    let g = vm.realm().global;
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(v, WC));
}

pub fn to_str_tag(vm: &mut Vm, o: Obj, tag: &str) {
    let k = PropertyKey::Sym(vm.wk.to_string_tag.clone());
    vm.heap.get_mut(o).props.insert(k, Prop::data(Value::str(tag), C));
}

/// `get [Symbol.species]() { return this; }`
pub fn species_getter(vm: &mut Vm, c: Obj) {
    let k = PropertyKey::Sym(vm.wk.species.clone());
    accessor(vm, c, k, "[Symbol.species]", Some(return_this), None, C);
}

pub fn return_this(_vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(ctx.this.clone())
}

fn function_proto_call(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Undefined)
}

/// Create a new realm's intrinsics and global object (§9.3.2 CreateIntrinsics, SetDefaultGlobalBindings).
pub fn init_realm(vm: &mut Vm, idx: u32) {
    let object_proto = vm.alloc(ObjectData::new(None, Kind::Ordinary));
    vm.realms[idx as usize].intrinsics.object_proto = object_proto;
    let fp = {
        let realm = idx;
        let mut d = ObjectData::new(Some(object_proto), Kind::Native(Box::new(NativeData { f: function_proto_call, ctor: false, slots: Vec::new(), realm })));
        d.props.insert(PropertyKey::from_str("length"), Prop::data(Value::Number(0.0), C));
        d.props.insert(PropertyKey::from_str("name"), Prop::data(Value::String(JsStr::empty()), C));
        vm.alloc(d)
    };
    vm.realms[idx as usize].intrinsics.function_proto = fp;
    let global_obj = vm.alloc(ObjectData::new(Some(object_proto), Kind::Ordinary));
    let genv = vm.alloc(ObjectData::new(None, Kind::GlobalEnv(Box::new(GlobalEnvData { object: global_obj, lex: PropMap::new(), var_names: Vec::new() }))));
    {
        let r = &mut vm.realms[idx as usize];
        r.global = global_obj;
        r.global_env = genv;
        r.global_this = Value::Object(global_obj);
    }
    object::init(vm);
    function::init(vm);
    error::init(vm);
    symbol::init(vm);
    iterator::init(vm);
    array::init(vm);
    string::init(vm);
    boolean::init(vm);
    number::init(vm);
    bigint::init(vm);
    math::init(vm);
    json::init(vm);
    global::init(vm);
    generator::init(vm);
    promise::init(vm);
    reflect::init(vm);
    proxy::init(vm);
    map::init(vm);
    weak::init(vm);
    regexp::init(vm);
    typedarray::init(vm);
    date::init(vm);
    global(vm, "globalThis", Value::Object(global_obj));
    let g = vm.realm().global;
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("Infinity"), Prop::data(Value::Number(f64::INFINITY), 0));
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("NaN"), Prop::data(Value::Number(f64::NAN), 0));
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("undefined"), Prop::data(Value::Undefined, 0));
}

/// `this` coerced to an object (or TypeError) for prototype methods.
pub fn this_obj(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Obj> {
    Ok(vm.to_object(&ctx.this)?.as_object().unwrap())
}

/// Relative index helper for slice-like arguments: clamp(ToIntegerOrInfinity(v)) into [0, len].
pub fn relative_index(vm: &mut Vm, v: &Value, len: f64, default: f64) -> JsResult<f64> {
    if v.is_undefined() {
        return Ok(default);
    }
    let r = vm.to_integer_or_infinity(v)?;
    Ok(if r < 0.0 { (len + r).max(0.0) } else { r.min(len) })
}
