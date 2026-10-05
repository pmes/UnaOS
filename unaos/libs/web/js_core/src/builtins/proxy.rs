//! Proxy exotic objects (§10.5) with all invariant checks, and the Proxy constructor (§28.2).

use super::*;
use crate::vm::object::PropDesc;

pub fn init(vm: &mut Vm) {
    let c = vm.make_native("Proxy", 2, proxy_ctor, true);
    method(vm, c, "revocable", 2, revocable);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.proxy_ctor = c;
    global(vm, "Proxy", Value::Object(c));
}

fn proxy_create(vm: &mut Vm, target: &Value, handler: &Value) -> JsResult<Obj> {
    let (t, h) = match (target, handler) {
        (Value::Object(t), Value::Object(h)) => (*t, *h),
        _ => return vm.throw_type("Cannot create proxy with a non-object as target or handler"),
    };
    let callable = vm.obj_is_callable(t);
    let ctor = vm.obj_is_constructor(t);
    Ok(vm.alloc(ObjectData::new(None, Kind::Proxy(Some(Box::new(ProxyData { target: t, handler: h, callable, ctor, revoked: false }))))))
}

fn proxy_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor Proxy requires 'new'");
    }
    let t = vm.arg(ctx, 0);
    let h = vm.arg(ctx, 1);
    Ok(Value::Object(proxy_create(vm, &t, &h)?))
}

fn revocable(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = vm.arg(ctx, 0);
    let h = vm.arg(ctx, 1);
    let p = proxy_create(vm, &t, &h)?;
    let rv = vm.make_native_closure("", 0, revoke, alloc::vec![Value::Object(p)]);
    let o = vm.new_plain_object();
    vm.create_data_property_or_throw(o, PropertyKey::from_str("proxy"), Value::Object(p))?;
    vm.create_data_property_or_throw(o, PropertyKey::from_str("revoke"), Value::Object(rv))?;
    Ok(Value::Object(o))
}

fn revoke(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if let Value::Object(p) = vm.native_slot(ctx.callee, 0) {
        // Keep callability for typeof after revocation.
        if let Kind::Proxy(Some(d)) = &mut vm.heap.get_mut(p).kind {
            d.revoked = true;
        }
        vm.set_native_slot(ctx.callee, 0, Value::Null);
    }
    Ok(Value::Undefined)
}

/// (target, handler) or TypeError if revoked.
fn parts(vm: &mut Vm, p: Obj) -> JsResult<(Obj, Obj)> {
    match &vm.heap.get(p).kind {
        Kind::Proxy(Some(d)) if !d.revoked => Ok((d.target, d.handler)),
        _ => vm.throw_type("Cannot perform operation on a revoked proxy"),
    }
}

fn trap(vm: &mut Vm, h: Obj, name: &str) -> JsResult<Option<Value>> {
    vm.get_method(&Value::Object(h), &PropertyKey::from_str(name))
}

fn is_compatible(vm: &mut Vm, ext: bool, desc: &PropDesc, cur: Option<PropDesc>) -> bool {
    let cur_prop = cur.map(|c| c.to_prop());
    vm.validate_and_apply(None, &PropertyKey::from_str(""), ext, desc, cur_prop)
}

pub fn get_prototype_of(vm: &mut Vm, p: Obj) -> JsResult<Option<Obj>> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "getPrototypeOf")? {
        None => return vm.get_prototype_of(t),
        Some(f) => f,
    };
    let r = vm.call(&tr, &Value::Object(h), &[Value::Object(t)])?;
    let proto = match r {
        Value::Object(o) => Some(o),
        Value::Null => None,
        _ => return vm.throw_type("'getPrototypeOf' on proxy: trap returned neither object nor null"),
    };
    if vm.is_extensible(t)? {
        return Ok(proto);
    }
    let tp = vm.get_prototype_of(t)?;
    if tp != proto {
        return vm.throw_type("'getPrototypeOf' on proxy: proxy target is non-extensible but the trap did not return its actual prototype");
    }
    Ok(proto)
}

pub fn set_prototype_of(vm: &mut Vm, p: Obj, v: Option<Obj>) -> JsResult<bool> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "setPrototypeOf")? {
        None => return vm.set_prototype_of(t, v),
        Some(f) => f,
    };
    let r = vm.call(&tr, &Value::Object(h), &[Value::Object(t), v.map(Value::Object).unwrap_or(Value::Null)])?;
    if !vm.to_boolean(&r) {
        return Ok(false);
    }
    if vm.is_extensible(t)? {
        return Ok(true);
    }
    let tp = vm.get_prototype_of(t)?;
    if tp != v {
        return vm.throw_type("'setPrototypeOf' on proxy: trap returned truish for setting a new prototype on the non-extensible proxy target");
    }
    Ok(true)
}

pub fn is_extensible(vm: &mut Vm, p: Obj) -> JsResult<bool> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "isExtensible")? {
        None => return vm.is_extensible(t),
        Some(f) => f,
    };
    let r = vm.call(&tr, &Value::Object(h), &[Value::Object(t)])?;
    let b = vm.to_boolean(&r);
    if b != vm.is_extensible(t)? {
        return vm.throw_type("'isExtensible' on proxy: trap result does not reflect extensibility of proxy target");
    }
    Ok(b)
}

pub fn prevent_extensions(vm: &mut Vm, p: Obj) -> JsResult<bool> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "preventExtensions")? {
        None => return vm.prevent_extensions(t),
        Some(f) => f,
    };
    let r = vm.call(&tr, &Value::Object(h), &[Value::Object(t)])?;
    let b = vm.to_boolean(&r);
    if b && vm.is_extensible(t)? {
        return vm.throw_type("'preventExtensions' on proxy: trap returned truish but the proxy target is extensible");
    }
    Ok(b)
}

pub fn get_own_property(vm: &mut Vm, p: Obj, key: &PropertyKey) -> JsResult<Option<PropDesc>> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "getOwnPropertyDescriptor")? {
        None => return vm.get_own_property(t, key),
        Some(f) => f,
    };
    let r = vm.call(&tr, &Value::Object(h), &[Value::Object(t), key.to_value()])?;
    if !r.is_object() && !r.is_undefined() {
        return vm.throw_type("'getOwnPropertyDescriptor' on proxy: trap returned neither object nor undefined");
    }
    let target_desc = vm.get_own_property(t, key)?;
    if r.is_undefined() {
        let td = match target_desc {
            None => return Ok(None),
            Some(td) => td,
        };
        if td.configurable == Some(false) {
            return vm.throw_type("'getOwnPropertyDescriptor' on proxy: trap returned undefined for a non-configurable property");
        }
        if !vm.is_extensible(t)? {
            return vm.throw_type("'getOwnPropertyDescriptor' on proxy: trap returned undefined for a property of a non-extensible target");
        }
        return Ok(None);
    }
    let ext = vm.is_extensible(t)?;
    let mut rd = vm.to_property_descriptor(&r)?;
    // CompletePropertyDescriptor
    if !rd.is_accessor() {
        rd.value.get_or_insert(Value::Undefined);
        rd.writable.get_or_insert(false);
    } else {
        rd.get.get_or_insert(Value::Undefined);
        rd.set.get_or_insert(Value::Undefined);
    }
    rd.enumerable.get_or_insert(false);
    rd.configurable.get_or_insert(false);
    if !is_compatible(vm, ext, &rd, target_desc.clone()) {
        return vm.throw_type("'getOwnPropertyDescriptor' on proxy: trap returned descriptor incompatible with the target property");
    }
    if rd.configurable == Some(false) {
        match &target_desc {
            None => return vm.throw_type("'getOwnPropertyDescriptor' on proxy: trap reported non-configurability for a property that does not exist on the target"),
            Some(td) => {
                if td.configurable == Some(true) {
                    return vm.throw_type("'getOwnPropertyDescriptor' on proxy: trap reported non-configurability for a configurable target property");
                }
                if rd.writable == Some(false) && td.writable == Some(true) {
                    return vm.throw_type("'getOwnPropertyDescriptor' on proxy: trap reported non-writable for a writable target property");
                }
            }
        }
    }
    Ok(Some(rd))
}

pub fn define_own_property(vm: &mut Vm, p: Obj, key: PropertyKey, desc: PropDesc) -> JsResult<bool> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "defineProperty")? {
        None => return vm.define_own_property(t, key, desc),
        Some(f) => f,
    };
    let dv = vm.from_property_descriptor(&desc);
    let r = vm.call(&tr, &Value::Object(h), &[Value::Object(t), key.to_value(), dv])?;
    if !vm.to_boolean(&r) {
        return Ok(false);
    }
    let td = vm.get_own_property(t, &key)?;
    let ext = vm.is_extensible(t)?;
    let setting_nc = desc.configurable == Some(false);
    match td {
        None => {
            if !ext {
                return vm.throw_type("'defineProperty' on proxy: trap returned truish for adding a property to a non-extensible target");
            }
            if setting_nc {
                return vm.throw_type("'defineProperty' on proxy: trap returned truish for defining a non-configurable property that does not exist on the target");
            }
        }
        Some(td) => {
            if !is_compatible(vm, ext, &desc, Some(td.clone())) {
                return vm.throw_type("'defineProperty' on proxy: trap returned truish for adding a property incompatible with the target");
            }
            if setting_nc && td.configurable == Some(true) {
                return vm.throw_type("'defineProperty' on proxy: trap returned truish for defining non-configurable a configurable target property");
            }
            if td.is_data() && td.configurable == Some(false) && td.writable == Some(true) && desc.writable == Some(false) {
                return vm.throw_type("'defineProperty' on proxy: trap returned truish for defining non-writable a writable non-configurable target property");
            }
        }
    }
    Ok(true)
}

pub fn has(vm: &mut Vm, p: Obj, key: &PropertyKey) -> JsResult<bool> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "has")? {
        None => return vm.has_property(t, key),
        Some(f) => f,
    };
    let r = vm.call(&tr, &Value::Object(h), &[Value::Object(t), key.to_value()])?;
    let b = vm.to_boolean(&r);
    if !b {
        if let Some(td) = vm.get_own_property(t, key)? {
            if td.configurable == Some(false) {
                return vm.throw_type("'has' on proxy: trap returned falsish for a non-configurable property");
            }
            if !vm.is_extensible(t)? {
                return vm.throw_type("'has' on proxy: trap returned falsish for a property of a non-extensible target");
            }
        }
    }
    Ok(b)
}

pub fn get(vm: &mut Vm, p: Obj, key: &PropertyKey, receiver: &Value) -> JsResult<Value> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "get")? {
        None => return vm.get_with_receiver(t, key, receiver),
        Some(f) => f,
    };
    let v = vm.call(&tr, &Value::Object(h), &[Value::Object(t), key.to_value(), receiver.clone()])?;
    if let Some(td) = vm.get_own_property(t, key)? {
        if td.configurable == Some(false) {
            if td.is_data() && td.writable == Some(false) && !v.same_value(td.value.as_ref().unwrap()) {
                return vm.throw_type("'get' on proxy: property is a read-only and non-configurable data property but the trap did not return its actual value");
            }
            if td.is_accessor() && matches!(td.get, Some(Value::Undefined)) && !v.is_undefined() {
                return vm.throw_type("'get' on proxy: property is a non-configurable accessor without a getter but the trap did not return undefined");
            }
        }
    }
    Ok(v)
}

pub fn set(vm: &mut Vm, p: Obj, key: PropertyKey, v: Value, receiver: &Value) -> JsResult<bool> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "set")? {
        None => return vm.set(t, key, v, receiver),
        Some(f) => f,
    };
    let r = vm.call(&tr, &Value::Object(h), &[Value::Object(t), key.to_value(), v.clone(), receiver.clone()])?;
    if !vm.to_boolean(&r) {
        return Ok(false);
    }
    if let Some(td) = vm.get_own_property(t, &key)? {
        if td.configurable == Some(false) {
            if td.is_data() && td.writable == Some(false) && !v.same_value(td.value.as_ref().unwrap()) {
                return vm.throw_type("'set' on proxy: trap returned truish for property which exists in the proxy target as a non-configurable and non-writable data property with a different value");
            }
            if td.is_accessor() && matches!(td.set, Some(Value::Undefined)) {
                return vm.throw_type("'set' on proxy: trap returned truish for property which exists in the proxy target as a non-configurable accessor without a setter");
            }
        }
    }
    Ok(true)
}

pub fn delete(vm: &mut Vm, p: Obj, key: &PropertyKey) -> JsResult<bool> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "deleteProperty")? {
        None => return vm.delete(t, key),
        Some(f) => f,
    };
    let r = vm.call(&tr, &Value::Object(h), &[Value::Object(t), key.to_value()])?;
    if !vm.to_boolean(&r) {
        return Ok(false);
    }
    if let Some(td) = vm.get_own_property(t, key)? {
        if td.configurable == Some(false) {
            return vm.throw_type("'deleteProperty' on proxy: trap returned truish for a non-configurable property");
        }
        if !vm.is_extensible(t)? {
            return vm.throw_type("'deleteProperty' on proxy: trap returned truish for a property of a non-extensible target");
        }
    }
    Ok(true)
}

pub fn own_keys(vm: &mut Vm, p: Obj) -> JsResult<Vec<PropertyKey>> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "ownKeys")? {
        None => return vm.own_property_keys(t),
        Some(f) => f,
    };
    let r = vm.call(&tr, &Value::Object(h), &[Value::Object(t)])?;
    let ro = match r {
        Value::Object(o) => o,
        _ => return vm.throw_type("'ownKeys' on proxy: trap returned a non-object"),
    };
    let len = vm.length_of(ro)?;
    let mut keys: Vec<PropertyKey> = Vec::new();
    let mut i = 0.0;
    while i < len {
        let v = vm.get(ro, &crate::builtins::array::key_of(i))?;
        let k = match v {
            Value::String(s) => PropertyKey::from_js(s),
            Value::Symbol(s) => PropertyKey::Sym(s),
            _ => return vm.throw_type("'ownKeys' on proxy: trap result contains a non-string, non-symbol element"),
        };
        if keys.contains(&k) {
            return vm.throw_type("'ownKeys' on proxy: trap returned duplicate entries");
        }
        keys.push(k);
        i += 1.0;
    }
    let ext = vm.is_extensible(t)?;
    let tkeys = vm.own_property_keys(t)?;
    let mut configurable = Vec::new();
    let mut nonconfigurable = Vec::new();
    for k in tkeys {
        match vm.get_own_property(t, &k)? {
            Some(d) if d.configurable == Some(false) => nonconfigurable.push(k),
            _ => configurable.push(k),
        }
    }
    if ext && nonconfigurable.is_empty() {
        return Ok(keys);
    }
    let mut unchecked = keys.clone();
    for k in &nonconfigurable {
        match unchecked.iter().position(|x| x == k) {
            Some(i) => {
                unchecked.remove(i);
            }
            None => return vm.throw_type("'ownKeys' on proxy: trap result did not include a non-configurable key"),
        }
    }
    if ext {
        return Ok(keys);
    }
    for k in &configurable {
        match unchecked.iter().position(|x| x == k) {
            Some(i) => {
                unchecked.remove(i);
            }
            None => return vm.throw_type("'ownKeys' on proxy: trap result did not include a key of the non-extensible target"),
        }
    }
    if !unchecked.is_empty() {
        return vm.throw_type("'ownKeys' on proxy: trap returned extra keys but proxy target is non-extensible");
    }
    Ok(keys)
}

pub fn proxy_call(vm: &mut Vm, p: Obj, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "apply")? {
        None => return vm.call(&Value::Object(t), this, args),
        Some(f) => f,
    };
    let arr = vm.new_array(args.to_vec());
    vm.call(&tr, &Value::Object(h), &[Value::Object(t), this.clone(), Value::Object(arr)])
}

pub fn proxy_construct(vm: &mut Vm, p: Obj, args: &[Value], nt: &Value) -> JsResult<Value> {
    let (t, h) = parts(vm, p)?;
    let tr = match trap(vm, h, "construct")? {
        None => return vm.construct(&Value::Object(t), args, Some(nt)),
        Some(f) => f,
    };
    let arr = vm.new_array(args.to_vec());
    let r = vm.call(&tr, &Value::Object(h), &[Value::Object(t), Value::Object(arr), nt.clone()])?;
    if !r.is_object() {
        return vm.throw_type("'construct' on proxy: trap returned non-object");
    }
    Ok(r)
}
