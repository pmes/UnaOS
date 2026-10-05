//! Symbol (§20.4).

use super::*;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let proto = vm.new_object(Some(op));
    let c = ctor(vm, "Symbol", 0, symbol_ctor, proto);
    let wk = [
        ("asyncIterator", vm.wk.async_iterator.clone()),
        ("hasInstance", vm.wk.has_instance.clone()),
        ("isConcatSpreadable", vm.wk.is_concat_spreadable.clone()),
        ("iterator", vm.wk.iterator.clone()),
        ("match", vm.wk.match_.clone()),
        ("matchAll", vm.wk.match_all.clone()),
        ("replace", vm.wk.replace.clone()),
        ("search", vm.wk.search.clone()),
        ("species", vm.wk.species.clone()),
        ("split", vm.wk.split.clone()),
        ("toPrimitive", vm.wk.to_primitive.clone()),
        ("toStringTag", vm.wk.to_string_tag.clone()),
        ("unscopables", vm.wk.unscopables.clone()),
    ];
    for (n, s) in wk {
        value(vm, c, n, Value::Symbol(s), 0);
    }
    method(vm, c, "for", 1, symbol_for);
    method(vm, c, "keyFor", 1, key_for);
    method(vm, proto, "toString", 0, to_string);
    method(vm, proto, "valueOf", 0, value_of);
    accessor(vm, proto, PropertyKey::from_str("description"), "description", Some(description), None, C);
    let tp = vm.wk.to_primitive.clone();
    method_sym(vm, proto, tp, "[Symbol.toPrimitive]", 1, value_of, C);
    to_str_tag(vm, proto, "Symbol");
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.symbol_proto = proto;
    vm.realms[r].intrinsics.symbol_ctor = c;
    global(vm, "Symbol", Value::Object(c));
}

fn symbol_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if !ctx.new_target.is_undefined() {
        return vm.throw_type("Symbol is not a constructor");
    }
    let d = vm.arg(ctx, 0);
    let desc = if d.is_undefined() { None } else { Some(vm.to_string(&d)?) };
    Ok(Value::Symbol(Sym::new(desc)))
}

fn symbol_for(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let k = vm.to_string(&a)?;
    if let Some((_, s)) = vm.symbol_registry.iter().find(|(kk, _)| *kk == k) {
        return Ok(Value::Symbol(s.clone()));
    }
    let s = Sym::new(Some(k.clone()));
    s.0.registered.set(true);
    vm.symbol_registry.push((k, s.clone()));
    Ok(Value::Symbol(s))
}

fn key_for(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    match vm.arg(ctx, 0) {
        Value::Symbol(s) => Ok(match vm.symbol_registry.iter().find(|(_, x)| *x == s) {
            Some((k, _)) => Value::String(k.clone()),
            None => Value::Undefined,
        }),
        _ => vm.throw_type("Symbol.keyFor requires a symbol"),
    }
}

fn this_sym(vm: &mut Vm, v: &Value) -> JsResult<Sym> {
    match v {
        Value::Symbol(s) => Ok(s.clone()),
        Value::Object(o) => match &vm.heap.get(*o).kind {
            Kind::Symbol(s) => Ok(s.clone()),
            _ => vm.throw_type("Symbol.prototype method called on incompatible receiver"),
        },
        _ => vm.throw_type("Symbol.prototype method called on incompatible receiver"),
    }
}

pub fn descriptive_string(s: &Sym) -> JsStr {
    JsStr::from_str("Symbol(").concat(&s.desc().cloned().unwrap_or_else(JsStr::empty)).concat(&JsStr::from_str(")"))
}

fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_sym(vm, &ctx.this)?;
    Ok(Value::String(descriptive_string(&s)))
}
fn value_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Symbol(this_sym(vm, &ctx.this)?))
}
fn description(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_sym(vm, &ctx.this)?;
    Ok(s.desc().cloned().map(Value::String).unwrap_or(Value::Undefined))
}
