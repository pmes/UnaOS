//! Object (§20.1).

use super::*;
use crate::vm::object::PropDesc;

pub fn init(vm: &mut Vm) {
    let proto = vm.intr().object_proto;
    let c = ctor(vm, "Object", 1, object_ctor, proto);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.object_ctor = c;
    for (n, l, f) in [
        ("assign", 2, assign as NativeFn),
        ("create", 2, create),
        ("defineProperties", 2, define_properties),
        ("defineProperty", 3, define_property),
        ("entries", 1, entries),
        ("freeze", 1, freeze),
        ("fromEntries", 1, from_entries),
        ("getOwnPropertyDescriptor", 2, get_own_property_descriptor),
        ("getOwnPropertyDescriptors", 1, get_own_property_descriptors),
        ("getOwnPropertyNames", 1, get_own_property_names),
        ("getOwnPropertySymbols", 1, get_own_property_symbols),
        ("getPrototypeOf", 1, get_prototype_of),
        ("groupBy", 2, group_by),
        ("hasOwn", 2, has_own),
        ("is", 2, is),
        ("isExtensible", 1, is_extensible),
        ("isFrozen", 1, is_frozen),
        ("isSealed", 1, is_sealed),
        ("keys", 1, keys),
        ("preventExtensions", 1, prevent_extensions),
        ("seal", 1, seal),
        ("setPrototypeOf", 2, set_prototype_of),
        ("values", 1, values),
    ] {
        method(vm, c, n, l, f);
    }
    for (n, l, f) in [
        ("hasOwnProperty", 1, has_own_property as NativeFn),
        ("isPrototypeOf", 1, is_prototype_of),
        ("propertyIsEnumerable", 1, property_is_enumerable),
        ("toLocaleString", 0, to_locale_string),
        ("valueOf", 0, value_of),
        ("__defineGetter__", 2, define_getter),
        ("__defineSetter__", 2, define_setter),
        ("__lookupGetter__", 1, lookup_getter),
        ("__lookupSetter__", 1, lookup_setter),
    ] {
        method(vm, proto, n, l, f);
    }
    let ts = method(vm, proto, "toString", 0, to_string);
    vm.realms[r].intrinsics.object_proto_to_string = ts;
    accessor(vm, proto, PropertyKey::from_str("__proto__"), "__proto__", Some(proto_get), Some(proto_set), C);
    global(vm, "Object", Value::Object(c));
}

fn object_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if let Value::Object(nt) = &ctx.new_target {
        if *nt != ctx.callee {
            let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.object_proto)?;
            return Ok(Value::Object(vm.new_object(Some(p))));
        }
    }
    let v = vm.arg(ctx, 0);
    if v.is_nullish() {
        return Ok(Value::Object(vm.new_plain_object()));
    }
    vm.to_object(&v)
}

fn obj_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<Obj> {
    let v = vm.arg(ctx, i);
    Ok(vm.to_object(&v)?.as_object().unwrap())
}

fn assign(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let to = obj_arg(vm, ctx, 0)?;
    for i in 1..ctx.argc {
        let src = vm.arg(ctx, i);
        if src.is_nullish() {
            continue;
        }
        let from = vm.to_object(&src)?.as_object().unwrap();
        let keys = vm.own_property_keys(from)?;
        for k in keys {
            if let Some(d) = vm.get_own_property(from, &k)? {
                if d.enumerable == Some(true) {
                    let v = vm.get(from, &k)?;
                    vm.set_prop(to, k, v, true)?;
                }
            }
        }
    }
    Ok(Value::Object(to))
}

fn create(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let p = vm.arg(ctx, 0);
    let proto = match p {
        Value::Object(o) => Some(o),
        Value::Null => None,
        _ => return vm.throw_type("Object prototype may only be an Object or null"),
    };
    let o = vm.new_object(proto);
    let props = vm.arg(ctx, 1);
    if !props.is_undefined() {
        object_define_properties(vm, o, &props)?;
    }
    Ok(Value::Object(o))
}

fn object_define_properties(vm: &mut Vm, o: Obj, props: &Value) -> JsResult<()> {
    let p = vm.to_object(props)?.as_object().unwrap();
    let keys = vm.own_property_keys(p)?;
    let mut descs = Vec::new();
    for k in keys {
        if let Some(pd) = vm.get_own_property(p, &k)? {
            if pd.enumerable == Some(true) {
                let dv = vm.get(p, &k)?;
                let d = vm.to_property_descriptor(&dv)?;
                descs.push((k, d));
            }
        }
    }
    for (k, d) in descs {
        vm.define_property_or_throw(o, k, d)?;
    }
    Ok(())
}

fn define_properties(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = match vm.arg(ctx, 0) {
        Value::Object(o) => o,
        _ => return vm.throw_type("Object.defineProperties called on non-object"),
    };
    let props = vm.arg(ctx, 1);
    object_define_properties(vm, o, &props)?;
    Ok(Value::Object(o))
}

fn define_property(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = match vm.arg(ctx, 0) {
        Value::Object(o) => o,
        _ => return vm.throw_type("Object.defineProperty called on non-object"),
    };
    let k = vm.arg(ctx, 1);
    let key = vm.to_property_key(&k)?;
    let dv = vm.arg(ctx, 2);
    let d = vm.to_property_descriptor(&dv)?;
    vm.define_property_or_throw(o, key, d)?;
    Ok(Value::Object(o))
}

/// EnumerableOwnProperties(O, kind): 0 keys, 1 values, 2 entries.
pub fn enumerable_own(vm: &mut Vm, o: Obj, kind: u8) -> JsResult<Vec<Value>> {
    let keys = vm.own_property_keys(o)?;
    let mut out = Vec::new();
    for k in keys {
        if k.is_symbol() {
            continue;
        }
        if let Some(d) = vm.get_own_property(o, &k)? {
            if d.enumerable == Some(true) {
                match kind {
                    0 => out.push(k.to_value()),
                    1 => {
                        let v = vm.get(o, &k)?;
                        out.push(v);
                    }
                    _ => {
                        let v = vm.get(o, &k)?;
                        let e = vm.new_array(alloc::vec![k.to_value(), v]);
                        out.push(Value::Object(e));
                    }
                }
            }
        }
    }
    Ok(out)
}

fn keys(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = obj_arg(vm, ctx, 0)?;
    let v = enumerable_own(vm, o, 0)?;
    Ok(Value::Object(vm.new_array(v)))
}
fn values(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = obj_arg(vm, ctx, 0)?;
    let v = enumerable_own(vm, o, 1)?;
    Ok(Value::Object(vm.new_array(v)))
}
fn entries(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = obj_arg(vm, ctx, 0)?;
    let v = enumerable_own(vm, o, 2)?;
    Ok(Value::Object(vm.new_array(v)))
}

fn freeze(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    if let Value::Object(o) = v {
        if !vm.set_integrity_level(o, true)? {
            return vm.throw_type("Cannot freeze");
        }
    }
    Ok(v)
}
fn seal(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    if let Value::Object(o) = v {
        if !vm.set_integrity_level(o, false)? {
            return vm.throw_type("Cannot seal");
        }
    }
    Ok(v)
}
fn prevent_extensions(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    if let Value::Object(o) = v {
        if !vm.prevent_extensions(o)? {
            return vm.throw_type("Cannot prevent extensions");
        }
    }
    Ok(v)
}
fn is_frozen(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    match vm.arg(ctx, 0) {
        Value::Object(o) => Ok(Value::Bool(vm.test_integrity_level(o, true)?)),
        _ => Ok(Value::Bool(true)),
    }
}
fn is_sealed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    match vm.arg(ctx, 0) {
        Value::Object(o) => Ok(Value::Bool(vm.test_integrity_level(o, false)?)),
        _ => Ok(Value::Bool(true)),
    }
}
fn is_extensible(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    match vm.arg(ctx, 0) {
        Value::Object(o) => Ok(Value::Bool(vm.is_extensible(o)?)),
        _ => Ok(Value::Bool(false)),
    }
}

fn from_entries(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let it = vm.arg(ctx, 0);
    if it.is_nullish() {
        return vm.throw_type("Object.fromEntries requires an iterable");
    }
    let o = vm.new_plain_object();
    let (iter, next) = vm.get_iterator(&it)?;
    vm.root(&iter);
    loop {
        let e = match vm.iterator_step_value(&iter, &next)? {
            Some(e) => e,
            None => break,
        };
        let r = (|| -> JsResult<()> {
            let eo = match &e {
                Value::Object(x) => *x,
                _ => return vm.throw_type("Iterator value is not an entry object"),
            };
            let k = vm.get(eo, &PropertyKey::Index(0))?;
            let v = vm.get(eo, &PropertyKey::Index(1))?;
            let key = vm.to_property_key(&k)?;
            vm.create_data_property_or_throw(o, key, v)
        })();
        if let Err(err) = r {
            let _ = vm.iterator_close(&iter);
            return Err(err);
        }
    }
    Ok(Value::Object(o))
}

fn get_own_property_descriptor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = obj_arg(vm, ctx, 0)?;
    let k = vm.arg(ctx, 1);
    let key = vm.to_property_key(&k)?;
    match vm.get_own_property(o, &key)? {
        Some(d) => Ok(vm.from_property_descriptor(&d)),
        None => Ok(Value::Undefined),
    }
}

fn get_own_property_descriptors(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = obj_arg(vm, ctx, 0)?;
    let keys = vm.own_property_keys(o)?;
    let r = vm.new_plain_object();
    for k in keys {
        if let Some(d) = vm.get_own_property(o, &k)? {
            let dv = vm.from_property_descriptor(&d);
            vm.create_data_property_or_throw(r, k, dv)?;
        }
    }
    Ok(Value::Object(r))
}

fn get_own_property_names(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = obj_arg(vm, ctx, 0)?;
    let keys = vm.own_property_keys(o)?;
    let v: Vec<Value> = keys.into_iter().filter(|k| !k.is_symbol()).map(|k| k.to_value()).collect();
    Ok(Value::Object(vm.new_array(v)))
}

fn get_own_property_symbols(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = obj_arg(vm, ctx, 0)?;
    let keys = vm.own_property_keys(o)?;
    let v: Vec<Value> = keys.into_iter().filter(|k| k.is_symbol()).map(|k| k.to_value()).collect();
    Ok(Value::Object(vm.new_array(v)))
}

fn get_prototype_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = obj_arg(vm, ctx, 0)?;
    Ok(vm.get_prototype_of(o)?.map(Value::Object).unwrap_or(Value::Null))
}

fn set_prototype_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = vm.arg(ctx, 0);
    if o.is_nullish() {
        return vm.throw_type("Object.setPrototypeOf called on null or undefined");
    }
    let p = match vm.arg(ctx, 1) {
        Value::Object(p) => Some(p),
        Value::Null => None,
        _ => return vm.throw_type("Object prototype may only be an Object or null"),
    };
    if let Value::Object(ob) = o {
        if !vm.set_prototype_of(ob, p)? {
            return vm.throw_type("Cannot set prototype");
        }
    }
    Ok(o)
}

fn group_by(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let groups = group_by_impl(vm, ctx, true)?;
    let o = vm.new_object(None);
    for (k, vals) in groups {
        let arr = vm.new_array(vals);
        let key = vm.to_property_key(&k)?;
        vm.create_data_property_or_throw(o, key, Value::Object(arr))?;
    }
    Ok(Value::Object(o))
}

/// GroupBy(items, callback, keyCoercion): property keys (true) or zero-normalised values (false).
pub fn group_by_impl(vm: &mut Vm, ctx: &CallCtx, property: bool) -> JsResult<Vec<(Value, Vec<Value>)>> {
    let items = vm.arg(ctx, 0);
    let cb = vm.arg(ctx, 1);
    if items.is_nullish() {
        return vm.throw_type("groupBy called on null or undefined");
    }
    if !vm.is_callable(&cb) {
        return vm.throw_type("callback is not a function");
    }
    let (iter, next) = vm.get_iterator(&items)?;
    vm.root(&iter);
    let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
    let mut k = 0f64;
    loop {
        let v = match vm.iterator_step_value(&iter, &next)? {
            Some(v) => v,
            None => break,
        };
        vm.root(&v);
        let r = vm.call(&cb, &Value::Undefined, &[v.clone(), Value::Number(k)]);
        let key = match r {
            Ok(x) => x,
            Err(e) => {
                let _ = vm.iterator_close(&iter);
                return Err(e);
            }
        };
        let key = if property {
            match vm.to_property_key(&key) {
                Ok(pk) => pk.to_value(),
                Err(e) => {
                    let _ = vm.iterator_close(&iter);
                    return Err(e);
                }
            }
        } else {
            match key {
                Value::Number(n) if n == 0.0 => Value::Number(0.0),
                other => other,
            }
        };
        vm.root(&key);
        match groups.iter_mut().find(|(g, _)| g.same_value_zero(&key)) {
            Some((_, list)) => list.push(v),
            None => groups.push((key, alloc::vec![v])),
        }
        k += 1.0;
    }
    Ok(groups)
}

fn has_own(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = obj_arg(vm, ctx, 0)?;
    let k = vm.arg(ctx, 1);
    let key = vm.to_property_key(&k)?;
    Ok(Value::Bool(vm.has_own_property(o, &key)?))
}

fn is(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(vm.arg(ctx, 0).same_value(&vm.arg(ctx, 1))))
}

fn has_own_property(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let k = vm.arg(ctx, 0);
    let key = vm.to_property_key(&k)?;
    let o = this_obj(vm, ctx)?;
    Ok(Value::Bool(vm.has_own_property(o, &key)?))
}

fn is_prototype_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = match vm.arg(ctx, 0) {
        Value::Object(v) => v,
        _ => return Ok(Value::Bool(false)),
    };
    let o = this_obj(vm, ctx)?;
    let mut cur = v;
    loop {
        match vm.get_prototype_of(cur)? {
            None => return Ok(Value::Bool(false)),
            Some(p) => {
                if p == o {
                    return Ok(Value::Bool(true));
                }
                cur = p;
            }
        }
    }
}

fn property_is_enumerable(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let k = vm.arg(ctx, 0);
    let key = vm.to_property_key(&k)?;
    let o = this_obj(vm, ctx)?;
    Ok(Value::Bool(matches!(vm.get_own_property(o, &key)?, Some(d) if d.enumerable == Some(true))))
}

fn to_locale_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let this = ctx.this.clone();
    vm.invoke(&this, &PropertyKey::from_str("toString"), &[])
}

fn value_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    vm.to_object(&ctx.this)
}

/// Object.prototype.toString (§20.1.3.6).
pub fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let this = ctx.this.clone();
    match this {
        Value::Undefined => return Ok(Value::str("[object Undefined]")),
        Value::Null => return Ok(Value::str("[object Null]")),
        _ => {}
    }
    let o = vm.to_object(&this)?.as_object().unwrap();
    let builtin = if vm.is_array(&Value::Object(o))? {
        "Array"
    } else {
        match &vm.heap.get(o).kind {
            Kind::Arguments(_) => "Arguments",
            Kind::Function(_) | Kind::Native(_) | Kind::Bound(_) => "Function",
            Kind::Proxy(Some(p)) if p.callable => "Function",
            Kind::Error(_) => "Error",
            Kind::Boolean(_) => "Boolean",
            Kind::Number(_) => "Number",
            Kind::String(_) => "String",
            Kind::Date(_) => "Date",
            Kind::RegExp(_) => "RegExp",
            _ => "Object",
        }
    };
    let tag_key = PropertyKey::Sym(vm.wk.to_string_tag.clone());
    let tag = vm.get(o, &tag_key)?;
    let t = match tag {
        Value::String(s) => s,
        _ => JsStr::from_str(builtin),
    };
    Ok(Value::String(JsStr::from_str("[object ").concat(&t).concat(&JsStr::from_str("]"))))
}

fn proto_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_obj(vm, ctx)?;
    Ok(vm.get_prototype_of(o)?.map(Value::Object).unwrap_or(Value::Null))
}

fn proto_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let this = ctx.this.clone();
    if this.is_nullish() {
        return vm.throw_type("Object.prototype.__proto__ called on null or undefined");
    }
    let p = match vm.arg(ctx, 0) {
        Value::Object(p) => Some(p),
        Value::Null => None,
        _ => return Ok(Value::Undefined),
    };
    if let Value::Object(o) = this {
        if !vm.set_prototype_of(o, p)? {
            return vm.throw_type("Object.prototype.__proto__ setter failed");
        }
    }
    Ok(Value::Undefined)
}

fn define_getter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_obj(vm, ctx)?;
    let g = vm.arg(ctx, 1);
    if !vm.is_callable(&g) {
        return vm.throw_type("getter is not a function");
    }
    let k = vm.arg(ctx, 0);
    let key = vm.to_property_key(&k)?;
    vm.define_property_or_throw(o, key, PropDesc { get: Some(g), enumerable: Some(true), configurable: Some(true), ..Default::default() })?;
    Ok(Value::Undefined)
}
fn define_setter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_obj(vm, ctx)?;
    let s = vm.arg(ctx, 1);
    if !vm.is_callable(&s) {
        return vm.throw_type("setter is not a function");
    }
    let k = vm.arg(ctx, 0);
    let key = vm.to_property_key(&k)?;
    vm.define_property_or_throw(o, key, PropDesc { set: Some(s), enumerable: Some(true), configurable: Some(true), ..Default::default() })?;
    Ok(Value::Undefined)
}
fn lookup_accessor(vm: &mut Vm, ctx: &CallCtx, getter: bool) -> JsResult<Value> {
    let mut o = this_obj(vm, ctx)?;
    let k = vm.arg(ctx, 0);
    let key = vm.to_property_key(&k)?;
    loop {
        if let Some(d) = vm.get_own_property(o, &key)? {
            if d.is_accessor() {
                return Ok(if getter { d.get.unwrap_or(Value::Undefined) } else { d.set.unwrap_or(Value::Undefined) });
            }
            return Ok(Value::Undefined);
        }
        match vm.get_prototype_of(o)? {
            Some(p) => o = p,
            None => return Ok(Value::Undefined),
        }
    }
}
fn lookup_getter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    lookup_accessor(vm, ctx, true)
}
fn lookup_setter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    lookup_accessor(vm, ctx, false)
}
