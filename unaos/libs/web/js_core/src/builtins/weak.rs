//! WeakMap (§24.3), WeakSet (§24.4), WeakRef (§26.1) and FinalizationRegistry (§26.2).

use super::*;
use crate::builtins::map::{map_data, map_data_ref};

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let r = vm.cur_realm as usize;
    let wmp = vm.new_object(Some(op));
    let wmc = ctor(vm, "WeakMap", 0, weakmap_ctor, wmp);
    method(vm, wmp, "delete", 1, wm_delete);
    method(vm, wmp, "get", 1, wm_get);
    method(vm, wmp, "has", 1, wm_has);
    method(vm, wmp, "set", 2, wm_set);
    to_str_tag(vm, wmp, "WeakMap");
    vm.realms[r].intrinsics.weakmap_proto = wmp;
    vm.realms[r].intrinsics.weakmap_ctor = wmc;
    global(vm, "WeakMap", Value::Object(wmc));
    let wsp = vm.new_object(Some(op));
    let wsc = ctor(vm, "WeakSet", 0, weakset_ctor, wsp);
    method(vm, wsp, "add", 1, ws_add);
    method(vm, wsp, "delete", 1, ws_delete);
    method(vm, wsp, "has", 1, ws_has);
    to_str_tag(vm, wsp, "WeakSet");
    vm.realms[r].intrinsics.weakset_proto = wsp;
    vm.realms[r].intrinsics.weakset_ctor = wsc;
    global(vm, "WeakSet", Value::Object(wsc));
    let wrp = vm.new_object(Some(op));
    let wrc = ctor(vm, "WeakRef", 1, weakref_ctor, wrp);
    method(vm, wrp, "deref", 0, wr_deref);
    to_str_tag(vm, wrp, "WeakRef");
    vm.realms[r].intrinsics.weakref_proto = wrp;
    vm.realms[r].intrinsics.weakref_ctor = wrc;
    global(vm, "WeakRef", Value::Object(wrc));
    let frp = vm.new_object(Some(op));
    let frc = ctor(vm, "FinalizationRegistry", 1, finreg_ctor, frp);
    method(vm, frp, "register", 2, fr_register);
    method(vm, frp, "unregister", 1, fr_unregister);
    method(vm, frp, "cleanupSome", 0, fr_cleanup_some);
    to_str_tag(vm, frp, "FinalizationRegistry");
    vm.realms[r].intrinsics.finreg_proto = frp;
    vm.realms[r].intrinsics.finreg_ctor = frc;
    global(vm, "FinalizationRegistry", Value::Object(frc));
}

/// CanBeHeldWeakly (§9.13)
pub fn can_be_held_weakly(v: &Value) -> bool {
    match v {
        Value::Object(_) => true,
        Value::Symbol(s) => !s.0.registered.get(),
        _ => false,
    }
}

fn this_kind(vm: &mut Vm, v: &Value, map: bool) -> JsResult<Obj> {
    if let Value::Object(o) = v {
        match (&vm.heap.get(*o).kind, map) {
            (Kind::WeakMap(_), true) | (Kind::WeakSet(_), false) => return Ok(*o),
            _ => {}
        }
    }
    vm.throw_type(if map { "WeakMap method called on incompatible receiver" } else { "WeakSet method called on incompatible receiver" })
}

fn weakmap_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor WeakMap requires 'new'");
    }
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.weakmap_proto)?;
    let m = vm.alloc(ObjectData::new(Some(p), Kind::WeakMap(Box::default())));
    let it = vm.arg(ctx, 0);
    if !it.is_nullish() {
        crate::builtins::map::add_from_iterable(vm, m, &it, "set", true)?;
    }
    Ok(Value::Object(m))
}
fn wm_delete(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_kind(vm, &ctx.this, true)?;
    let k = vm.arg(ctx, 0);
    if !can_be_held_weakly(&k) {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(map_data(vm, m).delete(&k)))
}
fn wm_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_kind(vm, &ctx.this, true)?;
    let k = vm.arg(ctx, 0);
    Ok(map_data_ref(vm, m).get(&k).cloned().unwrap_or(Value::Undefined))
}
fn wm_has(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_kind(vm, &ctx.this, true)?;
    let k = vm.arg(ctx, 0);
    Ok(Value::Bool(map_data_ref(vm, m).has(&k)))
}
fn wm_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_kind(vm, &ctx.this, true)?;
    let k = vm.arg(ctx, 0);
    if !can_be_held_weakly(&k) {
        return vm.throw_type("Invalid value used as weak map key");
    }
    let v = vm.arg(ctx, 1);
    map_data(vm, m).set(k, v);
    Ok(Value::Object(m))
}

fn weakset_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor WeakSet requires 'new'");
    }
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.weakset_proto)?;
    let s = vm.alloc(ObjectData::new(Some(p), Kind::WeakSet(Box::default())));
    let it = vm.arg(ctx, 0);
    if !it.is_nullish() {
        crate::builtins::map::add_from_iterable(vm, s, &it, "add", false)?;
    }
    Ok(Value::Object(s))
}
fn ws_add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_kind(vm, &ctx.this, false)?;
    let k = vm.arg(ctx, 0);
    if !can_be_held_weakly(&k) {
        return vm.throw_type("Invalid value used in weak set");
    }
    if !map_data_ref(vm, s).has(&k) {
        map_data(vm, s).set(k, Value::Undefined);
    }
    Ok(Value::Object(s))
}
fn ws_delete(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_kind(vm, &ctx.this, false)?;
    let k = vm.arg(ctx, 0);
    if !can_be_held_weakly(&k) {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(map_data(vm, s).delete(&k)))
}
fn ws_has(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_kind(vm, &ctx.this, false)?;
    let k = vm.arg(ctx, 0);
    Ok(Value::Bool(map_data_ref(vm, s).has(&k)))
}

fn weakref_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor WeakRef requires 'new'");
    }
    let t = vm.arg(ctx, 0);
    if !can_be_held_weakly(&t) {
        return vm.throw_type("WeakRef: target must be an object or non-registered symbol");
    }
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.weakref_proto)?;
    if let Value::Object(o) = &t {
        vm.kept_alive.push(*o);
    }
    Ok(Value::Object(vm.alloc(ObjectData::new(Some(p), Kind::WeakRef(Box::new(WeakRefData { target: t }))))))
}
fn wr_deref(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = match &ctx.this {
        Value::Object(o) if matches!(vm.heap.get(*o).kind, Kind::WeakRef(_)) => *o,
        _ => return vm.throw_type("WeakRef.prototype.deref called on incompatible receiver"),
    };
    let t = match &vm.heap.get(o).kind {
        Kind::WeakRef(w) => w.target.clone(),
        _ => Value::Undefined,
    };
    if let Value::Object(x) = &t {
        vm.kept_alive.push(*x);
    }
    Ok(t)
}

fn finreg_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor FinalizationRegistry requires 'new'");
    }
    let cb = vm.arg(ctx, 0);
    if !vm.is_callable(&cb) {
        return vm.throw_type("FinalizationRegistry: cleanup must be callable");
    }
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.finreg_proto)?;
    Ok(Value::Object(vm.alloc(ObjectData::new(Some(p), Kind::FinReg(Box::new(FinRegData { cleanup: cb, cells: Vec::new() }))))))
}
fn this_fr(vm: &mut Vm, v: &Value) -> JsResult<Obj> {
    match v {
        Value::Object(o) if matches!(vm.heap.get(*o).kind, Kind::FinReg(_)) => Ok(*o),
        _ => vm.throw_type("FinalizationRegistry method called on incompatible receiver"),
    }
}
fn fr_register(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let fr = this_fr(vm, &ctx.this)?;
    let target = vm.arg(ctx, 0);
    let held = vm.arg(ctx, 1);
    let token = vm.arg(ctx, 2);
    if !can_be_held_weakly(&target) {
        return vm.throw_type("FinalizationRegistry.prototype.register: invalid target");
    }
    if target.same_value(&held) {
        return vm.throw_type("FinalizationRegistry.prototype.register: target and holdings must not be same");
    }
    if !token.is_undefined() && !can_be_held_weakly(&token) {
        return vm.throw_type("FinalizationRegistry.prototype.register: invalid unregister token");
    }
    if let Kind::FinReg(f) = &mut vm.heap.get_mut(fr).kind {
        f.cells.push((target, held, token));
    }
    Ok(Value::Undefined)
}
fn fr_unregister(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let fr = this_fr(vm, &ctx.this)?;
    let token = vm.arg(ctx, 0);
    if !can_be_held_weakly(&token) {
        return vm.throw_type("FinalizationRegistry.prototype.unregister: invalid token");
    }
    let mut removed = false;
    if let Kind::FinReg(f) = &mut vm.heap.get_mut(fr).kind {
        let before = f.cells.len();
        f.cells.retain(|(_, _, t)| !t.same_value(&token));
        removed = f.cells.len() != before;
    }
    Ok(Value::Bool(removed))
}
fn fr_cleanup_some(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let _ = this_fr(vm, &ctx.this)?;
    let cb = vm.arg(ctx, 0);
    if !cb.is_undefined() && !vm.is_callable(&cb) {
        return vm.throw_type("cleanupSome: callback must be callable");
    }
    Ok(Value::Undefined)
}
