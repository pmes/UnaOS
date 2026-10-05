//! %IteratorPrototype%, the Iterator constructor and iterator helpers (§27.1), %AsyncIteratorPrototype%,
//! and the Array iterator (§23.1.5).

use super::*;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let ip = vm.new_object(Some(op));
    let it_sym = vm.wk.iterator.clone();
    method_sym(vm, ip, it_sym.clone(), "[Symbol.iterator]", 0, return_this, WC);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.iterator_proto = ip;
    // Iterator constructor
    let c = vm.make_native("Iterator", 0, iterator_ctor, true);
    vm.heap.get_mut(c).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(ip), 0));
    accessor(vm, ip, PropertyKey::from_str("constructor"), "constructor", Some(ctor_get), Some(ctor_set), C);
    let tst = PropertyKey::Sym(vm.wk.to_string_tag.clone());
    accessor(vm, ip, tst, "[Symbol.toStringTag]", Some(tag_get), Some(tag_set), C);
    method(vm, c, "from", 1, iterator_from);
    for (n, l, f) in [
        ("map", 1, map as NativeFn),
        ("filter", 1, filter),
        ("take", 1, take),
        ("drop", 1, drop),
        ("flatMap", 1, flat_map),
        ("reduce", 1, reduce),
        ("toArray", 0, to_array),
        ("forEach", 1, for_each),
        ("some", 1, some),
        ("every", 1, every),
        ("find", 1, find),
    ] {
        method(vm, ip, n, l, f);
    }
    vm.realms[r].intrinsics.iterator_ctor = c;
    global(vm, "Iterator", Value::Object(c));
    // %IteratorHelperPrototype%
    let hp = vm.new_object(Some(ip));
    method(vm, hp, "next", 0, helper_next);
    method(vm, hp, "return", 0, helper_return);
    to_str_tag(vm, hp, "Iterator Helper");
    vm.realms[r].intrinsics.iterator_helper_proto = hp;
    // %WrapForValidIteratorPrototype%
    let wp = vm.new_object(Some(ip));
    method(vm, wp, "next", 0, wrap_next);
    method(vm, wp, "return", 0, wrap_return);
    vm.realms[r].intrinsics.wrap_for_valid_iterator_proto = wp;
    // %ArrayIteratorPrototype%
    let ap = vm.new_object(Some(ip));
    method(vm, ap, "next", 0, array_iter_next);
    to_str_tag(vm, ap, "Array Iterator");
    vm.realms[r].intrinsics.array_iterator_proto = ap;
    // %AsyncIteratorPrototype%
    let aip = vm.new_object(Some(op));
    let ai = vm.wk.async_iterator.clone();
    method_sym(vm, aip, ai, "[Symbol.asyncIterator]", 0, return_this, WC);
    vm.realms[r].intrinsics.async_iterator_proto = aip;
}

fn iterator_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    match &ctx.new_target {
        Value::Object(nt) if *nt != ctx.callee => {
            let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.iterator_proto)?;
            Ok(Value::Object(vm.new_object(Some(p))))
        }
        _ => vm.throw_type("Iterator is an abstract class and cannot be constructed directly"),
    }
}

/// SetterThatIgnoresPrototypeProperties (§27.1.3.2.1.1 note).
fn setter_ignoring_proto(vm: &mut Vm, this: &Value, home: Obj, key: PropertyKey, v: Value) -> JsResult<Value> {
    let o = match this {
        Value::Object(o) => *o,
        _ => return vm.throw_type("setter called on non-object"),
    };
    if o == home {
        return vm.throw_type("Cannot assign to this property of the prototype");
    }
    match vm.get_own_property(o, &key)? {
        None => {
            vm.create_data_property_or_throw(o, key, v)?;
        }
        Some(_) => {
            vm.set_prop(o, key, v, true)?;
        }
    }
    Ok(Value::Undefined)
}

fn ctor_get(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Object(vm.intr().iterator_ctor))
}
fn ctor_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let home = vm.intr().iterator_proto;
    let v = vm.arg(ctx, 0);
    setter_ignoring_proto(vm, &ctx.this, home, PropertyKey::from_str("constructor"), v)
}
fn tag_get(_vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::str("Iterator"))
}
fn tag_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let home = vm.intr().iterator_proto;
    let v = vm.arg(ctx, 0);
    let k = PropertyKey::Sym(vm.wk.to_string_tag.clone());
    setter_ignoring_proto(vm, &ctx.this, home, k, v)
}

/// GetIteratorFlattenable(obj, primitiveHandling)
fn get_iterator_flattenable(vm: &mut Vm, v: &Value, iterate_strings: bool) -> JsResult<(Value, Value)> {
    if !v.is_object() && !(iterate_strings && matches!(v, Value::String(_))) {
        return vm.throw_type("value is not an object");
    }
    let key = PropertyKey::Sym(vm.wk.iterator.clone());
    let m = vm.get_method(v, &key)?;
    let it = match m {
        None => v.clone(),
        Some(m) => vm.call(&m, v, &[])?,
    };
    if !it.is_object() {
        return vm.throw_type("iterator is not an object");
    }
    let next = vm.get_v(&it, &PropertyKey::from_str("next"))?;
    Ok((it, next))
}

fn iterator_from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = vm.arg(ctx, 0);
    let (it, next) = get_iterator_flattenable(vm, &o, true)?;
    // OrdinaryHasInstance(%Iterator%, iterator)
    let ic = Value::Object(vm.intr().iterator_ctor);
    if vm.ordinary_has_instance(&ic, &it)? {
        return Ok(it);
    }
    let wp = vm.intr().wrap_for_valid_iterator_proto;
    let w = vm.alloc(ObjectData::new(Some(wp), Kind::Iterator(Box::new(IterData::Wrap { iter: it, next }))));
    Ok(Value::Object(w))
}

fn wrap_parts(vm: &mut Vm, this: &Value) -> JsResult<(Value, Value)> {
    if let Value::Object(o) = this {
        if let Kind::Iterator(d) = &vm.heap.get(*o).kind {
            if let IterData::Wrap { iter, next } = &**d {
                return Ok((iter.clone(), next.clone()));
            }
        }
    }
    vm.throw_type("not a wrapped iterator")
}

fn wrap_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (it, next) = wrap_parts(vm, &ctx.this)?;
    vm.call(&next, &it, &[])
}

fn wrap_return(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (it, _) = wrap_parts(vm, &ctx.this)?;
    let m = vm.get_method(&it, &PropertyKey::from_str("return"))?;
    match m {
        None => Ok(Value::Object(vm.iter_result(Value::Undefined, true))),
        Some(m) => vm.call(&m, &it, &[]),
    }
}

/// GetIteratorDirect(this) for prototype methods.
fn direct(vm: &mut Vm, this: &Value) -> JsResult<(Value, Value)> {
    if !this.is_object() {
        return vm.throw_type("Iterator.prototype method called on non-object");
    }
    let next = vm.get_v(this, &PropertyKey::from_str("next"))?;
    Ok((this.clone(), next))
}

fn close_with<T>(vm: &mut Vm, it: &Value, e: Value) -> JsResult<T> {
    let _ = vm.iterator_close(it);
    Err(e)
}

fn require_callable(vm: &mut Vm, it: &Value, f: &Value) -> JsResult<()> {
    if !vm.is_callable(f) {
        let e = vm.type_error("argument is not a function");
        return close_with(vm, it, e);
    }
    Ok(())
}

fn make_helper(vm: &mut Vm, kind: u8, it: Value, next: Value, func: Value, remaining: f64) -> Value {
    let hp = vm.intr().iterator_helper_proto;
    let h = vm.alloc(ObjectData::new(
        Some(hp),
        Kind::Iterator(Box::new(IterData::Helper(Box::new(HelperData { kind, iter: it, next, func, counter: 0.0, remaining, inner: None, state: 0 })))),
    ));
    Value::Object(h)
}

fn map(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (it, next) = direct(vm, &ctx.this)?;
    let f = vm.arg(ctx, 0);
    require_callable(vm, &it, &f)?;
    Ok(make_helper(vm, 0, it, next, f, 0.0))
}
fn filter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (it, next) = direct(vm, &ctx.this)?;
    let f = vm.arg(ctx, 0);
    require_callable(vm, &it, &f)?;
    Ok(make_helper(vm, 1, it, next, f, 0.0))
}
fn limit_arg(vm: &mut Vm, ctx: &CallCtx, it: &Value) -> JsResult<f64> {
    let l = vm.arg(ctx, 0);
    let n = match vm.to_number(&l) {
        Ok(n) => n,
        Err(e) => return close_with(vm, it, e),
    };
    if n.is_nan() {
        let e = vm.range_error("limit must be a number");
        return close_with(vm, it, e);
    }
    let i = crate::vm::ops::integer_or_infinity(n);
    if i < 0.0 {
        let e = vm.range_error("limit must be non-negative");
        return close_with(vm, it, e);
    }
    Ok(i)
}
fn take(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (it, _) = (ctx.this.clone(), ());
    if !it.is_object() {
        return vm.throw_type("Iterator.prototype.take called on non-object");
    }
    let n = limit_arg(vm, ctx, &it)?;
    let (it, next) = direct(vm, &ctx.this)?;
    Ok(make_helper(vm, 2, it, next, Value::Undefined, n))
}
fn drop(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let it = ctx.this.clone();
    if !it.is_object() {
        return vm.throw_type("Iterator.prototype.drop called on non-object");
    }
    let n = limit_arg(vm, ctx, &it)?;
    let (it, next) = direct(vm, &ctx.this)?;
    Ok(make_helper(vm, 3, it, next, Value::Undefined, n))
}
fn flat_map(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (it, next) = direct(vm, &ctx.this)?;
    let f = vm.arg(ctx, 0);
    require_callable(vm, &it, &f)?;
    Ok(make_helper(vm, 4, it, next, f, 0.0))
}

fn helper_obj(vm: &mut Vm, this: &Value) -> JsResult<Obj> {
    if let Value::Object(o) = this {
        if let Kind::Iterator(d) = &vm.heap.get(*o).kind {
            if let IterData::Helper(_) = &**d {
                return Ok(*o);
            }
        }
    }
    vm.throw_type("not an Iterator Helper")
}

fn with_helper<R>(vm: &mut Vm, o: Obj, f: impl FnOnce(&mut HelperData) -> R) -> R {
    match &mut vm.heap.get_mut(o).kind {
        Kind::Iterator(d) => match &mut **d {
            IterData::Helper(h) => f(h),
            _ => unreachable!(),
        },
        _ => unreachable!(),
    }
}

fn helper_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = helper_obj(vm, &ctx.this)?;
    let state = with_helper(vm, o, |h| h.state);
    if state == 3 {
        return vm.throw_type("Iterator Helper is already running");
    }
    if state == 2 {
        return Ok(Value::Object(vm.iter_result(Value::Undefined, true)));
    }
    with_helper(vm, o, |h| h.state = 3);
    let r = helper_step(vm, o);
    match r {
        Ok(Some(v)) => {
            with_helper(vm, o, |h| h.state = 1);
            Ok(Value::Object(vm.iter_result(v, false)))
        }
        Ok(None) => {
            with_helper(vm, o, |h| h.state = 2);
            Ok(Value::Object(vm.iter_result(Value::Undefined, true)))
        }
        Err(e) => {
            with_helper(vm, o, |h| h.state = 2);
            Err(e)
        }
    }
}

fn helper_step(vm: &mut Vm, o: Obj) -> JsResult<Option<Value>> {
    let (kind, it, next, f) = with_helper(vm, o, |h| (h.kind, h.iter.clone(), h.next.clone(), h.func.clone()));
    match kind {
        0 | 1 => loop {
            let v = match vm.iterator_step_value(&it, &next)? {
                Some(v) => v,
                None => return Ok(None),
            };
            let c = with_helper(vm, o, |h| {
                let c = h.counter;
                h.counter += 1.0;
                c
            });
            let r = match vm.call(&f, &Value::Undefined, &[v.clone(), Value::Number(c)]) {
                Ok(r) => r,
                Err(e) => return close_with(vm, &it, e),
            };
            if kind == 0 {
                return Ok(Some(r));
            }
            if vm.to_boolean(&r) {
                return Ok(Some(v));
            }
        },
        2 => {
            let rem = with_helper(vm, o, |h| h.remaining);
            if rem == 0.0 {
                vm.iterator_close(&it)?;
                return Ok(None);
            }
            if rem != f64::INFINITY {
                with_helper(vm, o, |h| h.remaining -= 1.0);
            }
            vm.iterator_step_value(&it, &next)
        }
        3 => {
            loop {
                let rem = with_helper(vm, o, |h| h.remaining);
                if rem <= 0.0 {
                    break;
                }
                if rem != f64::INFINITY {
                    with_helper(vm, o, |h| h.remaining -= 1.0);
                }
                if vm.iterator_step_value(&it, &next)?.is_none() {
                    return Ok(None);
                }
            }
            vm.iterator_step_value(&it, &next)
        }
        _ => loop {
            let inner = with_helper(vm, o, |h| h.inner.clone());
            if let Some((ii, inext)) = inner {
                match vm.iterator_step_value(&ii, &inext) {
                    Ok(Some(v)) => return Ok(Some(v)),
                    Ok(None) => {
                        with_helper(vm, o, |h| h.inner = None);
                        continue;
                    }
                    Err(e) => return close_with(vm, &it, e),
                }
            }
            let v = match vm.iterator_step_value(&it, &next)? {
                Some(v) => v,
                None => return Ok(None),
            };
            let c = with_helper(vm, o, |h| {
                let c = h.counter;
                h.counter += 1.0;
                c
            });
            let mapped = match vm.call(&f, &Value::Undefined, &[v, Value::Number(c)]) {
                Ok(r) => r,
                Err(e) => return close_with(vm, &it, e),
            };
            match get_iterator_flattenable(vm, &mapped, false) {
                Ok(pair) => with_helper(vm, o, |h| h.inner = Some(pair)),
                Err(e) => return close_with(vm, &it, e),
            }
        },
    }
}

fn helper_return(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = helper_obj(vm, &ctx.this)?;
    let (state, it, inner) = with_helper(vm, o, |h| (h.state, h.iter.clone(), h.inner.clone()));
    if state == 3 {
        return vm.throw_type("Iterator Helper is already running");
    }
    if state == 2 {
        return Ok(Value::Object(vm.iter_result(Value::Undefined, true)));
    }
    with_helper(vm, o, |h| h.state = 2);
    if let Some((ii, _)) = inner {
        let r = vm.iterator_close(&ii);
        if let Err(e) = r {
            let _ = vm.iterator_close(&it);
            return Err(e);
        }
    }
    vm.iterator_close(&it)?;
    Ok(Value::Object(vm.iter_result(Value::Undefined, true)))
}

fn reduce(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (it, next) = direct(vm, &ctx.this)?;
    let f = vm.arg(ctx, 0);
    require_callable(vm, &it, &f)?;
    let mut counter;
    let mut acc = if ctx.argc < 2 {
        match vm.iterator_step_value(&it, &next)? {
            Some(v) => {
                counter = 1.0;
                v
            }
            None => return vm.throw_type("Reduce of empty iterator with no initial value"),
        }
    } else {
        counter = 0.0;
        vm.arg(ctx, 1)
    };
    loop {
        let v = match vm.iterator_step_value(&it, &next)? {
            Some(v) => v,
            None => return Ok(acc),
        };
        vm.root(&acc);
        acc = match vm.call(&f, &Value::Undefined, &[acc.clone(), v, Value::Number(counter)]) {
            Ok(r) => r,
            Err(e) => return close_with(vm, &it, e),
        };
        counter += 1.0;
    }
}

fn to_array(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (it, next) = direct(vm, &ctx.this)?;
    let mut out = Vec::new();
    while let Some(v) = vm.iterator_step_value(&it, &next)? {
        vm.root(&v);
        out.push(v);
    }
    Ok(Value::Object(vm.new_array(out)))
}

fn for_each(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (it, next) = direct(vm, &ctx.this)?;
    let f = vm.arg(ctx, 0);
    require_callable(vm, &it, &f)?;
    let mut c = 0.0;
    while let Some(v) = vm.iterator_step_value(&it, &next)? {
        if let Err(e) = vm.call(&f, &Value::Undefined, &[v, Value::Number(c)]) {
            return close_with(vm, &it, e);
        }
        c += 1.0;
    }
    Ok(Value::Undefined)
}

/// some / every / find: mode 0 some, 1 every, 2 find
fn predicate_walk(vm: &mut Vm, ctx: &CallCtx, mode: u8) -> JsResult<Value> {
    let (it, next) = direct(vm, &ctx.this)?;
    let f = vm.arg(ctx, 0);
    require_callable(vm, &it, &f)?;
    let mut c = 0.0;
    while let Some(v) = vm.iterator_step_value(&it, &next)? {
        let r = match vm.call(&f, &Value::Undefined, &[v.clone(), Value::Number(c)]) {
            Ok(r) => r,
            Err(e) => return close_with(vm, &it, e),
        };
        let b = vm.to_boolean(&r);
        match mode {
            0 if b => {
                vm.iterator_close(&it)?;
                return Ok(Value::Bool(true));
            }
            1 if !b => {
                vm.iterator_close(&it)?;
                return Ok(Value::Bool(false));
            }
            2 if b => {
                vm.iterator_close(&it)?;
                return Ok(v);
            }
            _ => {}
        }
        c += 1.0;
    }
    Ok(match mode {
        0 => Value::Bool(false),
        1 => Value::Bool(true),
        _ => Value::Undefined,
    })
}
fn some(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    predicate_walk(vm, ctx, 0)
}
fn every(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    predicate_walk(vm, ctx, 1)
}
fn find(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    predicate_walk(vm, ctx, 2)
}

// ------------------------------------------------------------------------------------------------ Array iterator

pub fn create_array_iterator(vm: &mut Vm, target: Value, kind: IterKind) -> Value {
    let p = vm.intr().array_iterator_proto;
    Value::Object(vm.alloc(ObjectData::new(Some(p), Kind::Iterator(Box::new(IterData::Array { target: Some(target), index: 0, kind })))))
}

fn array_iter_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = match &ctx.this {
        Value::Object(o) => *o,
        _ => return vm.throw_type("next called on incompatible receiver"),
    };
    let (target, index, kind) = match &vm.heap.get(o).kind {
        Kind::Iterator(d) => match &**d {
            IterData::Array { target, index, kind } => (target.clone(), *index, *kind),
            _ => return vm.throw_type("next called on incompatible receiver"),
        },
        _ => return vm.throw_type("next called on incompatible receiver"),
    };
    let t = match target {
        Some(t) => t,
        None => return Ok(Value::Object(vm.iter_result(Value::Undefined, true))),
    };
    let to = t.as_object().unwrap();
    let len = match &vm.heap.get(to).kind {
        Kind::TypedArray(_) => match crate::builtins::typedarray::ta_length(vm, to) {
            Some(n) => n as f64,
            None => return vm.throw_type("TypedArray is detached or out of bounds"),
        },
        Kind::Array(a) => a.length() as f64,
        _ => vm.length_of(to)?,
    };
    if index as f64 >= len {
        if let Kind::Iterator(d) = &mut vm.heap.get_mut(o).kind {
            if let IterData::Array { target, .. } = &mut **d {
                *target = None;
            }
        }
        return Ok(Value::Object(vm.iter_result(Value::Undefined, true)));
    }
    if let Kind::Iterator(d) = &mut vm.heap.get_mut(o).kind {
        if let IterData::Array { index: i, .. } = &mut **d {
            *i = index + 1;
        }
    }
    let r = match kind {
        IterKind::Keys => Value::Number(index as f64),
        IterKind::Values => vm.get(to, &PropertyKey::from(index as u32))?,
        IterKind::Entries => {
            let v = vm.get(to, &PropertyKey::from(index as u32))?;
            Value::Object(vm.new_array(alloc::vec![Value::Number(index as f64), v]))
        }
    };
    Ok(Value::Object(vm.iter_result(r, false)))
}
