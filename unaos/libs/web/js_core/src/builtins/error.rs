//! Error objects (§20.5): Error, the NativeError constructors and AggregateError.

use super::*;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let fp = vm.intr().function_proto;
    let proto = vm.new_object(Some(op));
    let c = ctor(vm, "Error", 1, error_ctor, proto);
    value(vm, proto, "name", Value::str("Error"), WC);
    value(vm, proto, "message", Value::str(""), WC);
    method(vm, proto, "toString", 0, to_string);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.error_proto = proto;
    vm.realms[r].intrinsics.error_ctor = c;
    global(vm, "Error", Value::Object(c));
    let _ = fp;
    for name in ["EvalError", "RangeError", "ReferenceError", "SyntaxError", "TypeError", "URIError", "AggregateError"] {
        let p = vm.new_object(Some(proto));
        let f: NativeFn = if name == "AggregateError" { aggregate_error_ctor } else { native_error_ctor };
        let len = if name == "AggregateError" { 2 } else { 1 };
        let nc = vm.make_native_with(name, len, f, true, Some(c), Vec::new());
        vm.heap.get_mut(nc).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(p), 0));
        vm.heap.get_mut(p).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(nc), WC));
        value(vm, p, "name", Value::str(name), WC);
        value(vm, p, "message", Value::str(""), WC);
        let i = &mut vm.realms[r].intrinsics;
        match name {
            "EvalError" => {
                i.eval_error_proto = p;
                i.eval_error_ctor = nc;
            }
            "RangeError" => {
                i.range_error_proto = p;
                i.range_error_ctor = nc;
            }
            "ReferenceError" => {
                i.reference_error_proto = p;
                i.reference_error_ctor = nc;
            }
            "SyntaxError" => {
                i.syntax_error_proto = p;
                i.syntax_error_ctor = nc;
            }
            "TypeError" => {
                i.type_error_proto = p;
                i.type_error_ctor = nc;
            }
            "URIError" => {
                i.uri_error_proto = p;
                i.uri_error_ctor = nc;
            }
            _ => {
                i.aggregate_error_proto = p;
                i.aggregate_error_ctor = nc;
            }
        }
        global(vm, name, Value::Object(nc));
    }
}

fn proto_for(vm: &Vm, callee: Obj) -> fn(&Intrinsics) -> Obj {
    let i = vm.intr();
    let c = callee;
    for r in &vm.realms {
        let i2 = &r.intrinsics;
        if c == i2.error_ctor {
            return |i: &Intrinsics| i.error_proto;
        }
        if c == i2.eval_error_ctor {
            return |i: &Intrinsics| i.eval_error_proto;
        }
        if c == i2.range_error_ctor {
            return |i: &Intrinsics| i.range_error_proto;
        }
        if c == i2.reference_error_ctor {
            return |i: &Intrinsics| i.reference_error_proto;
        }
        if c == i2.syntax_error_ctor {
            return |i: &Intrinsics| i.syntax_error_proto;
        }
        if c == i2.type_error_ctor {
            return |i: &Intrinsics| i.type_error_proto;
        }
        if c == i2.uri_error_ctor {
            return |i: &Intrinsics| i.uri_error_proto;
        }
        if c == i2.aggregate_error_ctor {
            return |i: &Intrinsics| i.aggregate_error_proto;
        }
    }
    let _ = i;
    |i: &Intrinsics| i.error_proto
}

/// OrdinaryCreateFromConstructor + message / cause installation shared by all error constructors.
fn make(vm: &mut Vm, ctx: &CallCtx, msg_idx: usize) -> JsResult<Obj> {
    let nt = if ctx.new_target.is_undefined() { Value::Object(ctx.callee) } else { ctx.new_target.clone() };
    let pf = proto_for(vm, ctx.callee);
    let proto = vm.get_prototype_from_ctor(&nt, pf)?;
    let o = vm.alloc(ObjectData::new(Some(proto), Kind::Error));
    let msg = vm.arg(ctx, msg_idx);
    if !msg.is_undefined() {
        let s = vm.to_string(&msg)?;
        vm.heap.get_mut(o).props.insert(PropertyKey::from_str("message"), Prop::data(Value::String(s), WC));
    }
    // InstallErrorCause
    let opts = vm.arg(ctx, msg_idx + 1);
    if let Value::Object(oo) = opts {
        let k = PropertyKey::from_str("cause");
        if vm.has_property(oo, &k)? {
            let cause = vm.get(oo, &k)?;
            vm.heap.get_mut(o).props.insert(k, Prop::data(cause, WC));
        }
    }
    Ok(o)
}

fn error_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Object(make(vm, ctx, 0)?))
}
fn native_error_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Object(make(vm, ctx, 0)?))
}
fn aggregate_error_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = make(vm, ctx, 1)?;
    let errs = vm.arg(ctx, 0);
    let list = vm.iterable_to_list(&errs)?;
    let arr = vm.new_array(list);
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str("errors"), Prop::data(Value::Object(arr), WC));
    Ok(Value::Object(o))
}

fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = match &ctx.this {
        Value::Object(o) => *o,
        _ => return vm.throw_type("Error.prototype.toString called on non-object"),
    };
    let n = vm.get(o, &PropertyKey::from_str("name"))?;
    let name = if n.is_undefined() { JsStr::from_str("Error") } else { vm.to_string(&n)? };
    let m = vm.get(o, &PropertyKey::from_str("message"))?;
    let msg = if m.is_undefined() { JsStr::empty() } else { vm.to_string(&m)? };
    if name.is_empty() {
        return Ok(Value::String(msg));
    }
    if msg.is_empty() {
        return Ok(Value::String(name));
    }
    Ok(Value::String(name.concat(&JsStr::from_str(": ")).concat(&msg)))
}
