//! Boolean (§20.3).

use super::*;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let proto = vm.alloc(ObjectData::new(Some(op), Kind::Boolean(false)));
    let c = ctor(vm, "Boolean", 1, boolean_ctor, proto);
    method(vm, proto, "toString", 0, to_string);
    method(vm, proto, "valueOf", 0, value_of);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.boolean_proto = proto;
    vm.realms[r].intrinsics.boolean_ctor = c;
    global(vm, "Boolean", Value::Object(c));
}

fn boolean_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = vm.to_boolean(&vm.arg(ctx, 0));
    if ctx.new_target.is_undefined() {
        return Ok(Value::Bool(b));
    }
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.boolean_proto)?;
    Ok(Value::Object(vm.alloc(ObjectData::new(Some(p), Kind::Boolean(b)))))
}

fn this_bool(vm: &mut Vm, v: &Value) -> JsResult<bool> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Object(o) => match &vm.heap.get(*o).kind {
            Kind::Boolean(b) => Ok(*b),
            _ => vm.throw_type("Boolean.prototype method called on incompatible receiver"),
        },
        _ => vm.throw_type("Boolean.prototype method called on incompatible receiver"),
    }
}

fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = this_bool(vm, &ctx.this)?;
    Ok(Value::str(if b { "true" } else { "false" }))
}
fn value_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(this_bool(vm, &ctx.this)?))
}
