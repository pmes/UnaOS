//! Reflect (§28.1).

use super::*;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let r = vm.new_object(Some(op));
    for (n, l, f) in [
        ("apply", 3, apply as NativeFn),
        ("construct", 2, construct),
        ("defineProperty", 3, define_property),
        ("deleteProperty", 2, delete_property),
        ("get", 2, get),
        ("getOwnPropertyDescriptor", 2, get_own_property_descriptor),
        ("getPrototypeOf", 1, get_prototype_of),
        ("has", 2, has),
        ("isExtensible", 1, is_extensible),
        ("ownKeys", 1, own_keys),
        ("preventExtensions", 1, prevent_extensions),
        ("set", 3, set),
        ("setPrototypeOf", 2, set_prototype_of),
    ] {
        method(vm, r, n, l, f);
    }
    to_str_tag(vm, r, "Reflect");
    let ri = vm.cur_realm as usize;
    vm.realms[ri].intrinsics.reflect = r;
    global(vm, "Reflect", Value::Object(r));
}

fn target(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Obj> {
    match vm.arg(ctx, 0) {
        Value::Object(o) => Ok(o),
        _ => vm.throw_type("Reflect method called on non-object"),
    }
}

fn apply(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let f = vm.arg(ctx, 0);
    if !vm.is_callable(&f) {
        return vm.throw_type("Reflect.apply target is not callable");
    }
    let this = vm.arg(ctx, 1);
    let a = vm.arg(ctx, 2);
    let args = vm.list_from_array_like(&a)?;
    vm.call(&f, &this, &args)
}

fn construct(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let f = vm.arg(ctx, 0);
    if !vm.is_constructor(&f) {
        return vm.throw_type("Reflect.construct target is not a constructor");
    }
    let nt = if ctx.argc > 2 { vm.arg(ctx, 2) } else { f.clone() };
    if !vm.is_constructor(&nt) {
        return vm.throw_type("Reflect.construct newTarget is not a constructor");
    }
    let a = vm.arg(ctx, 1);
    let args = vm.list_from_array_like(&a)?;
    vm.construct(&f, &args, Some(&nt))
}

fn define_property(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = target(vm, ctx)?;
    let k = vm.arg(ctx, 1);
    let key = vm.to_property_key(&k)?;
    let dv = vm.arg(ctx, 2);
    let d = vm.to_property_descriptor(&dv)?;
    Ok(Value::Bool(vm.define_own_property(o, key, d)?))
}

fn delete_property(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = target(vm, ctx)?;
    let k = vm.arg(ctx, 1);
    let key = vm.to_property_key(&k)?;
    Ok(Value::Bool(vm.delete(o, &key)?))
}

fn get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = target(vm, ctx)?;
    let k = vm.arg(ctx, 1);
    let key = vm.to_property_key(&k)?;
    let recv = if ctx.argc > 2 { vm.arg(ctx, 2) } else { Value::Object(o) };
    vm.get_with_receiver(o, &key, &recv)
}

fn get_own_property_descriptor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = target(vm, ctx)?;
    let k = vm.arg(ctx, 1);
    let key = vm.to_property_key(&k)?;
    match vm.get_own_property(o, &key)? {
        Some(d) => Ok(vm.from_property_descriptor(&d)),
        None => Ok(Value::Undefined),
    }
}

fn get_prototype_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = target(vm, ctx)?;
    Ok(vm.get_prototype_of(o)?.map(Value::Object).unwrap_or(Value::Null))
}

fn has(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = target(vm, ctx)?;
    let k = vm.arg(ctx, 1);
    let key = vm.to_property_key(&k)?;
    Ok(Value::Bool(vm.has_property(o, &key)?))
}

fn is_extensible(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = target(vm, ctx)?;
    Ok(Value::Bool(vm.is_extensible(o)?))
}

fn own_keys(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = target(vm, ctx)?;
    let keys = vm.own_property_keys(o)?;
    let v = keys.into_iter().map(|k| k.to_value()).collect();
    Ok(Value::Object(vm.new_array(v)))
}

fn prevent_extensions(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = target(vm, ctx)?;
    Ok(Value::Bool(vm.prevent_extensions(o)?))
}

fn set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = target(vm, ctx)?;
    let k = vm.arg(ctx, 1);
    let key = vm.to_property_key(&k)?;
    let v = vm.arg(ctx, 2);
    let recv = if ctx.argc > 3 { vm.arg(ctx, 3) } else { Value::Object(o) };
    Ok(Value::Bool(vm.set(o, key, v, &recv)?))
}

fn set_prototype_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = target(vm, ctx)?;
    let p = match vm.arg(ctx, 1) {
        Value::Object(p) => Some(p),
        Value::Null => None,
        _ => return vm.throw_type("Object prototype may only be an Object or null"),
    };
    Ok(Value::Bool(vm.set_prototype_of(o, p)?))
}
