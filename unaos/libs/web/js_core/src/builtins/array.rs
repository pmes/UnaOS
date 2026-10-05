//! Array (§23.1). Every method follows the generic (array-like) algorithm of the specification; dense
//! arrays only take fast paths where no user code can observe the difference.

use super::*;
use crate::vm::object::PropDesc;

const MAX_SAFE: f64 = 9007199254740991.0;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let proto = vm.alloc(ObjectData::new(Some(op), Kind::Array(ArrayData { elems: Vec::new(), dense: true, len: 0, len_writable: true })));
    let c = ctor(vm, "Array", 1, array_ctor, proto);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.array_proto = proto;
    vm.realms[r].intrinsics.array_ctor = c;
    method(vm, c, "from", 1, from);
    method(vm, c, "isArray", 1, is_array);
    method(vm, c, "of", 0, of);
    species_getter(vm, c);
    for (n, l, f) in [
        ("at", 1, at as NativeFn),
        ("concat", 1, concat),
        ("copyWithin", 2, copy_within),
        ("entries", 0, entries),
        ("every", 1, every),
        ("fill", 1, fill),
        ("filter", 1, filter),
        ("find", 1, find),
        ("findIndex", 1, find_index),
        ("findLast", 1, find_last),
        ("findLastIndex", 1, find_last_index),
        ("flat", 0, flat),
        ("flatMap", 1, flat_map),
        ("forEach", 1, for_each),
        ("includes", 1, includes),
        ("indexOf", 1, index_of),
        ("join", 1, join),
        ("keys", 0, keys),
        ("lastIndexOf", 1, last_index_of),
        ("map", 1, map),
        ("pop", 0, pop),
        ("push", 1, push),
        ("reduce", 1, reduce),
        ("reduceRight", 1, reduce_right),
        ("reverse", 0, reverse),
        ("shift", 0, shift),
        ("slice", 2, slice),
        ("some", 1, some),
        ("sort", 1, sort),
        ("splice", 2, splice),
        ("toLocaleString", 0, to_locale_string),
        ("toReversed", 0, to_reversed),
        ("toSorted", 1, to_sorted),
        ("toSpliced", 2, to_spliced),
        ("toString", 0, to_string),
        ("unshift", 1, unshift),
        ("with", 2, with),
    ] {
        method(vm, proto, n, l, f);
    }
    let values = method(vm, proto, "values", 0, values);
    vm.realms[r].intrinsics.array_proto_values = values;
    let it = PropertyKey::Sym(vm.wk.iterator.clone());
    vm.heap.get_mut(proto).props.insert(it, Prop::data(Value::Object(values), WC));
    // @@unscopables
    let u = vm.new_object(None);
    for n in ["at", "copyWithin", "entries", "fill", "find", "findIndex", "findLast", "findLastIndex", "flat", "flatMap", "includes", "keys", "toReversed", "toSorted", "toSpliced", "values"] {
        value(vm, u, n, Value::Bool(true), WEC);
    }
    let us = PropertyKey::Sym(vm.wk.unscopables.clone());
    vm.heap.get_mut(proto).props.insert(us, Prop::data(Value::Object(u), C));
    global(vm, "Array", Value::Object(c));
}

fn array_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let nt = if ctx.new_target.is_undefined() { Value::Object(ctx.callee) } else { ctx.new_target.clone() };
    let proto = vm.get_prototype_from_ctor(&nt, |i| i.array_proto)?;
    if ctx.argc == 0 {
        return Ok(Value::Object(vm.array_create(0.0, Some(proto))?));
    }
    if ctx.argc == 1 {
        let len = vm.arg(ctx, 0);
        if let Value::Number(n) = len {
            let il = crate::vm::ops::to_uint32(n);
            if il as f64 != n {
                return vm.throw_range("Invalid array length");
            }
            return Ok(Value::Object(vm.array_create(il as f64, Some(proto))?));
        }
        let a = vm.array_create(0.0, Some(proto))?;
        vm.create_data_property_or_throw(a, PropertyKey::Index(0), len)?;
        return Ok(Value::Object(a));
    }
    let items = vm.args(ctx);
    let a = vm.array_create(0.0, Some(proto))?;
    if let Kind::Array(ad) = &mut vm.heap.get_mut(a).kind {
        ad.elems = items;
    }
    Ok(Value::Object(a))
}

fn is_array(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    Ok(Value::Bool(vm.is_array(&v)?))
}

fn of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = ctx.argc;
    let c = ctx.this.clone();
    let a = if vm.is_constructor(&c) {
        vm.construct(&c, &[Value::Number(n as f64)], None)?.as_object().ok_or(Value::Undefined).or_else(|_| vm.throw_type("constructor returned non-object"))?
    } else {
        vm.array_create(n as f64, None)?
    };
    for i in 0..n {
        let v = vm.arg(ctx, i);
        vm.create_data_property_or_throw(a, PropertyKey::from(i as u32), v)?;
    }
    vm.set_prop(a, PropertyKey::from_str("length"), Value::Number(n as f64), true)?;
    Ok(Value::Object(a))
}

fn from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let c = ctx.this.clone();
    let items = vm.arg(ctx, 0);
    let mapfn = vm.arg(ctx, 1);
    let this_arg = vm.arg(ctx, 2);
    let mapping = !mapfn.is_undefined();
    if mapping && !vm.is_callable(&mapfn) {
        return vm.throw_type("Array.from: mapper is not a function");
    }
    let it_key = PropertyKey::Sym(vm.wk.iterator.clone());
    let using = vm.get_method(&items, &it_key)?;
    if let Some(m) = using {
        let a = if vm.is_constructor(&c) {
            match vm.construct(&c, &[], None)? {
                Value::Object(o) => o,
                _ => return vm.throw_type("constructor returned non-object"),
            }
        } else {
            vm.array_create(0.0, None)?
        };
        let (it, next) = vm.get_iterator_from_method(&items, &m)?;
        vm.root(&it);
        let mut k = 0u64;
        loop {
            if k as f64 >= MAX_SAFE {
                let e = vm.type_error("Array.from: too many elements");
                let _ = vm.iterator_close(&it);
                return Err(e);
            }
            let v = match vm.iterator_step_value(&it, &next)? {
                Some(v) => v,
                None => {
                    vm.set_prop(a, PropertyKey::from_str("length"), Value::Number(k as f64), true)?;
                    return Ok(Value::Object(a));
                }
            };
            let v = if mapping {
                match vm.call(&mapfn, &this_arg, &[v, Value::Number(k as f64)]) {
                    Ok(x) => x,
                    Err(e) => {
                        let _ = vm.iterator_close(&it);
                        return Err(e);
                    }
                }
            } else {
                v
            };
            if let Err(e) = vm.create_data_property_or_throw(a, key_of(k as f64), v) {
                let _ = vm.iterator_close(&it);
                return Err(e);
            }
            k += 1;
        }
    }
    let al = vm.to_object(&items)?.as_object().unwrap();
    let len = vm.length_of(al)?;
    let a = if vm.is_constructor(&c) {
        match vm.construct(&c, &[Value::Number(len)], None)? {
            Value::Object(o) => o,
            _ => return vm.throw_type("constructor returned non-object"),
        }
    } else {
        vm.array_create(len, None)?
    };
    let mut k = 0.0;
    while k < len {
        let v = vm.get(al, &key_of(k))?;
        let v = if mapping { vm.call(&mapfn, &this_arg, &[v, Value::Number(k)])? } else { v };
        vm.create_data_property_or_throw(a, key_of(k), v)?;
        k += 1.0;
    }
    vm.set_prop(a, PropertyKey::from_str("length"), Value::Number(len), true)?;
    Ok(Value::Object(a))
}

/// A property key for an integral index (may exceed u32).
pub fn key_of(k: f64) -> PropertyKey {
    if k < 4294967295.0 {
        PropertyKey::Index(k as u32)
    } else {
        PropertyKey::from_f64(k)
    }
}

fn this_len(vm: &mut Vm, ctx: &CallCtx) -> JsResult<(Obj, f64)> {
    let o = this_obj(vm, ctx)?;
    let len = vm.length_of(o)?;
    Ok((o, len))
}

fn set_len(vm: &mut Vm, o: Obj, len: f64) -> JsResult<()> {
    vm.set_prop(o, PropertyKey::from_str("length"), Value::Number(len), true)
}

fn callback(vm: &mut Vm, ctx: &CallCtx) -> JsResult<(Value, Value)> {
    let f = vm.arg(ctx, 0);
    if !vm.is_callable(&f) {
        return vm.throw_type(&alloc::format!("{} is not a function", if f.is_undefined() { "undefined" } else { "callback" }));
    }
    Ok((f, vm.arg(ctx, 1)))
}

fn at(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let a = vm.arg(ctx, 0);
    let rel = vm.to_integer_or_infinity(&a)?;
    let k = if rel >= 0.0 { rel } else { len + rel };
    if k < 0.0 || k >= len {
        return Ok(Value::Undefined);
    }
    vm.get(o, &key_of(k))
}

fn is_concat_spreadable(vm: &mut Vm, v: &Value) -> JsResult<bool> {
    let o = match v {
        Value::Object(o) => *o,
        _ => return Ok(false),
    };
    let k = PropertyKey::Sym(vm.wk.is_concat_spreadable.clone());
    let s = vm.get(o, &k)?;
    if !s.is_undefined() {
        return Ok(vm.to_boolean(&s));
    }
    vm.is_array(v)
}

fn concat(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_obj(vm, ctx)?;
    let a = vm.array_species_create(o, 0.0)?;
    let mut n = 0.0;
    let mut items = alloc::vec![Value::Object(o)];
    items.extend(vm.args(ctx));
    for e in items {
        if is_concat_spreadable(vm, &e)? {
            let eo = e.as_object().unwrap();
            let len = vm.length_of(eo)?;
            if n + len > MAX_SAFE {
                return vm.throw_type("Array too long");
            }
            let mut k = 0.0;
            while k < len {
                let p = key_of(k);
                if vm.has_property(eo, &p)? {
                    let v = vm.get(eo, &p)?;
                    vm.create_data_property_or_throw(a, key_of(n), v)?;
                }
                n += 1.0;
                k += 1.0;
            }
        } else {
            if n >= MAX_SAFE {
                return vm.throw_type("Array too long");
            }
            vm.create_data_property_or_throw(a, key_of(n), e)?;
            n += 1.0;
        }
    }
    set_len(vm, a, n)?;
    Ok(Value::Object(a))
}

fn copy_within(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let t = vm.arg(ctx, 0);
    let to = relative_index(vm, &t, len, 0.0)?;
    let s = vm.arg(ctx, 1);
    let from = relative_index(vm, &s, len, 0.0)?;
    let e = vm.arg(ctx, 2);
    let fin = relative_index(vm, &e, len, len)?;
    let mut count = (fin - from).min(len - to);
    let (mut from, mut to, dir) = if from < to && to < from + count { (from + count - 1.0, to + count - 1.0, -1.0) } else { (from, to, 1.0) };
    while count > 0.0 {
        let fk = key_of(from);
        let tk = key_of(to);
        if vm.has_property(o, &fk)? {
            let v = vm.get(o, &fk)?;
            vm.set_prop(o, tk, v, true)?;
        } else {
            vm.delete_property_or_throw(o, &tk)?;
        }
        from += dir;
        to += dir;
        count -= 1.0;
    }
    Ok(Value::Object(o))
}

fn entries(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_obj(vm, ctx)?;
    Ok(crate::builtins::iterator::create_array_iterator(vm, Value::Object(o), IterKind::Entries))
}
fn keys(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_obj(vm, ctx)?;
    Ok(crate::builtins::iterator::create_array_iterator(vm, Value::Object(o), IterKind::Keys))
}
fn values(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_obj(vm, ctx)?;
    Ok(crate::builtins::iterator::create_array_iterator(vm, Value::Object(o), IterKind::Values))
}

/// every / some / forEach (mode 0 every, 1 some, 2 forEach)
fn iterate(vm: &mut Vm, ctx: &CallCtx, mode: u8) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let (f, t) = callback(vm, ctx)?;
    let mut k = 0.0;
    while k < len {
        let p = key_of(k);
        if vm.has_property(o, &p)? {
            let v = vm.get(o, &p)?;
            let r = vm.call(&f, &t, &[v, Value::Number(k), Value::Object(o)])?;
            let b = vm.to_boolean(&r);
            if mode == 0 && !b {
                return Ok(Value::Bool(false));
            }
            if mode == 1 && b {
                return Ok(Value::Bool(true));
            }
        }
        k += 1.0;
    }
    Ok(match mode {
        0 => Value::Bool(true),
        1 => Value::Bool(false),
        _ => Value::Undefined,
    })
}
fn every(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    iterate(vm, ctx, 0)
}
fn some(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    iterate(vm, ctx, 1)
}
fn for_each(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    iterate(vm, ctx, 2)
}

fn fill(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let v = vm.arg(ctx, 0);
    let s = vm.arg(ctx, 1);
    let mut k = relative_index(vm, &s, len, 0.0)?;
    let e = vm.arg(ctx, 2);
    let fin = relative_index(vm, &e, len, len)?;
    while k < fin {
        vm.set_prop(o, key_of(k), v.clone(), true)?;
        k += 1.0;
    }
    Ok(Value::Object(o))
}

fn filter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let (f, t) = callback(vm, ctx)?;
    let a = vm.array_species_create(o, 0.0)?;
    let mut to = 0.0;
    let mut k = 0.0;
    while k < len {
        let p = key_of(k);
        if vm.has_property(o, &p)? {
            let v = vm.get(o, &p)?;
            let r = vm.call(&f, &t, &[v.clone(), Value::Number(k), Value::Object(o)])?;
            if vm.to_boolean(&r) {
                vm.create_data_property_or_throw(a, key_of(to), v)?;
                to += 1.0;
            }
        }
        k += 1.0;
    }
    Ok(Value::Object(a))
}

/// find / findIndex / findLast / findLastIndex
fn find_impl(vm: &mut Vm, ctx: &CallCtx, last: bool, index: bool) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let f = vm.arg(ctx, 0);
    if !vm.is_callable(&f) {
        return vm.throw_type("predicate is not a function");
    }
    let t = vm.arg(ctx, 1);
    let mut k = if last { len - 1.0 } else { 0.0 };
    while if last { k >= 0.0 } else { k < len } {
        let v = vm.get(o, &key_of(k))?;
        let r = vm.call(&f, &t, &[v.clone(), Value::Number(k), Value::Object(o)])?;
        if vm.to_boolean(&r) {
            return Ok(if index { Value::Number(k) } else { v });
        }
        k += if last { -1.0 } else { 1.0 };
    }
    Ok(if index { Value::Number(-1.0) } else { Value::Undefined })
}
fn find(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    find_impl(vm, ctx, false, false)
}
fn find_index(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    find_impl(vm, ctx, false, true)
}
fn find_last(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    find_impl(vm, ctx, true, false)
}
fn find_last_index(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    find_impl(vm, ctx, true, true)
}

/// FlattenIntoArray (§23.1.3.13.1)
fn flatten_into(vm: &mut Vm, target: Obj, source: Obj, source_len: f64, start: f64, depth: f64, mapper: Option<(&Value, &Value)>) -> JsResult<f64> {
    let mut target_index = start;
    let mut si = 0.0;
    while si < source_len {
        let p = key_of(si);
        if vm.has_property(source, &p)? {
            let mut el = vm.get(source, &p)?;
            if let Some((f, t)) = mapper {
                el = vm.call(f, t, &[el, Value::Number(si), Value::Object(source)])?;
            }
            let mut should_flatten = false;
            if depth > 0.0 {
                should_flatten = vm.is_array(&el)?;
            }
            if should_flatten {
                let eo = el.as_object().unwrap();
                let el_len = vm.length_of(eo)?;
                target_index = flatten_into(vm, target, eo, el_len, target_index, depth - 1.0, None)?;
            } else {
                if target_index >= MAX_SAFE {
                    return vm.throw_type("Array too long");
                }
                vm.create_data_property_or_throw(target, key_of(target_index), el)?;
                target_index += 1.0;
            }
        }
        si += 1.0;
    }
    Ok(target_index)
}

fn flat(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let d = vm.arg(ctx, 0);
    let mut depth = 1.0;
    if !d.is_undefined() {
        depth = vm.to_integer_or_infinity(&d)?;
        if depth < 0.0 {
            depth = 0.0;
        }
    }
    let a = vm.array_species_create(o, 0.0)?;
    flatten_into(vm, a, o, len, 0.0, depth, None)?;
    Ok(Value::Object(a))
}

fn flat_map(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let (f, t) = callback(vm, ctx)?;
    let a = vm.array_species_create(o, 0.0)?;
    flatten_into(vm, a, o, len, 0.0, 1.0, Some((&f, &t)))?;
    Ok(Value::Object(a))
}

fn includes(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    if len == 0.0 {
        return Ok(Value::Bool(false));
    }
    let target = vm.arg(ctx, 0);
    let fa = vm.arg(ctx, 1);
    let n = vm.to_integer_or_infinity(&fa)?;
    if n == f64::INFINITY {
        return Ok(Value::Bool(false));
    }
    let mut k = if n >= 0.0 { n } else { (len + n).max(0.0) };
    while k < len {
        let v = vm.get(o, &key_of(k))?;
        if v.same_value_zero(&target) {
            return Ok(Value::Bool(true));
        }
        k += 1.0;
    }
    Ok(Value::Bool(false))
}

fn index_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    if len == 0.0 {
        return Ok(Value::Number(-1.0));
    }
    let target = vm.arg(ctx, 0);
    let fa = vm.arg(ctx, 1);
    let n = vm.to_integer_or_infinity(&fa)?;
    if n == f64::INFINITY {
        return Ok(Value::Number(-1.0));
    }
    let mut k = if n >= 0.0 { n } else { (len + n).max(0.0) };
    while k < len {
        let p = key_of(k);
        if vm.has_property(o, &p)? {
            let v = vm.get(o, &p)?;
            if v.strict_eq(&target) {
                return Ok(Value::Number(k));
            }
        }
        k += 1.0;
    }
    Ok(Value::Number(-1.0))
}

fn last_index_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    if len == 0.0 {
        return Ok(Value::Number(-1.0));
    }
    let target = vm.arg(ctx, 0);
    let n = if ctx.argc > 1 {
        let fa = vm.arg(ctx, 1);
        vm.to_integer_or_infinity(&fa)?
    } else {
        len - 1.0
    };
    if n == f64::NEG_INFINITY {
        return Ok(Value::Number(-1.0));
    }
    let mut k = if n >= 0.0 { n.min(len - 1.0) } else { len + n };
    while k >= 0.0 {
        let p = key_of(k);
        if vm.has_property(o, &p)? {
            let v = vm.get(o, &p)?;
            if v.strict_eq(&target) {
                return Ok(Value::Number(k));
            }
        }
        k -= 1.0;
    }
    Ok(Value::Number(-1.0))
}

fn join(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let s = vm.arg(ctx, 0);
    let sep = if s.is_undefined() { JsStr::from_str(",") } else { vm.to_string(&s)? };
    let mut out: Vec<u16> = Vec::new();
    let mut k = 0.0;
    while k < len {
        if k > 0.0 {
            out.extend_from_slice(sep.units());
        }
        let v = vm.get(o, &key_of(k))?;
        if !v.is_nullish() {
            let s = vm.to_string(&v)?;
            out.extend_from_slice(s.units());
            if out.len() > (1 << 30) {
                return vm.throw_range("Invalid string length");
            }
        }
        k += 1.0;
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn map(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let (f, t) = callback(vm, ctx)?;
    let a = vm.array_species_create(o, len)?;
    let mut k = 0.0;
    while k < len {
        let p = key_of(k);
        if vm.has_property(o, &p)? {
            let v = vm.get(o, &p)?;
            let r = vm.call(&f, &t, &[v, Value::Number(k), Value::Object(o)])?;
            vm.create_data_property_or_throw(a, p, r)?;
        }
        k += 1.0;
    }
    Ok(Value::Object(a))
}

fn pop(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    if len == 0.0 {
        set_len(vm, o, 0.0)?;
        return Ok(Value::Undefined);
    }
    let k = key_of(len - 1.0);
    let v = vm.get(o, &k)?;
    vm.delete_property_or_throw(o, &k)?;
    set_len(vm, o, len - 1.0)?;
    Ok(v)
}

fn push(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_obj(vm, ctx)?;
    // Fast path: plain dense array with writable length.
    if let Kind::Array(a) = &mut vm.heap.get_mut(o).kind {
        if a.dense && a.len_writable && (a.elems.len() + ctx.argc) < (1 << 31) {
            let ext = true;
            let _ = ext;
        }
    }
    let len = vm.length_of(o)?;
    if len + ctx.argc as f64 > MAX_SAFE {
        return vm.throw_type("Pushing would exceed the maximum array length");
    }
    let dense_ok = matches!(&vm.heap.get(o).kind, Kind::Array(a) if a.dense && a.len_writable) && vm.heap.get(o).extensible;
    if dense_ok {
        let args = vm.args(ctx);
        if let Kind::Array(a) = &mut vm.heap.get_mut(o).kind {
            a.elems.extend(args);
            return Ok(Value::Number(a.elems.len() as f64));
        }
    }
    let mut n = len;
    for i in 0..ctx.argc {
        let v = vm.arg(ctx, i);
        vm.set_prop(o, key_of(n), v, true)?;
        n += 1.0;
    }
    set_len(vm, o, n)?;
    Ok(Value::Number(n))
}

fn reduce_impl(vm: &mut Vm, ctx: &CallCtx, right: bool) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let f = vm.arg(ctx, 0);
    if !vm.is_callable(&f) {
        return vm.throw_type("reducer is not a function");
    }
    let mut k = if right { len - 1.0 } else { 0.0 };
    let step = if right { -1.0 } else { 1.0 };
    let in_range = |k: f64| if right { k >= 0.0 } else { k < len };
    let mut acc;
    if ctx.argc >= 2 {
        acc = vm.arg(ctx, 1);
    } else {
        loop {
            if !in_range(k) {
                return vm.throw_type("Reduce of empty array with no initial value");
            }
            let p = key_of(k);
            k += step;
            if vm.has_property(o, &p)? {
                acc = vm.get(o, &p)?;
                break;
            }
        }
    }
    while in_range(k) {
        let p = key_of(k);
        if vm.has_property(o, &p)? {
            let v = vm.get(o, &p)?;
            vm.root(&acc);
            acc = vm.call(&f, &Value::Undefined, &[acc.clone(), v, Value::Number(k), Value::Object(o)])?;
        }
        k += step;
    }
    Ok(acc)
}
fn reduce(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    reduce_impl(vm, ctx, false)
}
fn reduce_right(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    reduce_impl(vm, ctx, true)
}

fn reverse(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let middle = crate::numconv::libm_floor(len / 2.0);
    let mut lower = 0.0;
    while lower != middle {
        let upper = len - lower - 1.0;
        let lk = key_of(lower);
        let uk = key_of(upper);
        let le = vm.has_property(o, &lk)?;
        let lv = if le { vm.get(o, &lk)? } else { Value::Undefined };
        let ue = vm.has_property(o, &uk)?;
        let uv = if ue { vm.get(o, &uk)? } else { Value::Undefined };
        match (le, ue) {
            (true, true) => {
                vm.set_prop(o, lk, uv, true)?;
                vm.set_prop(o, uk, lv, true)?;
            }
            (false, true) => {
                vm.set_prop(o, lk, uv, true)?;
                vm.delete_property_or_throw(o, &uk)?;
            }
            (true, false) => {
                vm.delete_property_or_throw(o, &lk)?;
                vm.set_prop(o, uk, lv, true)?;
            }
            _ => {}
        }
        lower += 1.0;
    }
    Ok(Value::Object(o))
}

fn shift(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    if len == 0.0 {
        set_len(vm, o, 0.0)?;
        return Ok(Value::Undefined);
    }
    let first = vm.get(o, &PropertyKey::Index(0))?;
    // Fast path: dense array without holes, ordinary prototype chain irrelevant (no holes means no lookups).
    let fast = matches!(&vm.heap.get(o).kind, Kind::Array(a) if a.dense && a.len_writable && a.elems.len() as f64 == len && a.elems.iter().all(|v| !v.is_empty()));
    if fast {
        if let Kind::Array(a) = &mut vm.heap.get_mut(o).kind {
            a.elems.remove(0);
        }
        return Ok(first);
    }
    let mut k = 1.0;
    while k < len {
        let from = key_of(k);
        let to = key_of(k - 1.0);
        if vm.has_property(o, &from)? {
            let v = vm.get(o, &from)?;
            vm.set_prop(o, to, v, true)?;
        } else {
            vm.delete_property_or_throw(o, &to)?;
        }
        k += 1.0;
    }
    vm.delete_property_or_throw(o, &key_of(len - 1.0))?;
    set_len(vm, o, len - 1.0)?;
    Ok(first)
}

fn unshift(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let n = ctx.argc as f64;
    if n > 0.0 {
        if len + n > MAX_SAFE {
            return vm.throw_type("Unshift would exceed the maximum array length");
        }
        let mut k = len;
        while k > 0.0 {
            let from = key_of(k - 1.0);
            let to = key_of(k + n - 1.0);
            if vm.has_property(o, &from)? {
                let v = vm.get(o, &from)?;
                vm.set_prop(o, to, v, true)?;
            } else {
                vm.delete_property_or_throw(o, &to)?;
            }
            k -= 1.0;
        }
        for j in 0..ctx.argc {
            let v = vm.arg(ctx, j);
            vm.set_prop(o, key_of(j as f64), v, true)?;
        }
    }
    set_len(vm, o, len + n)?;
    Ok(Value::Number(len + n))
}

fn slice(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let s = vm.arg(ctx, 0);
    let mut k = relative_index(vm, &s, len, 0.0)?;
    let e = vm.arg(ctx, 1);
    let fin = relative_index(vm, &e, len, len)?;
    let count = (fin - k).max(0.0);
    let a = vm.array_species_create(o, count)?;
    let mut n = 0.0;
    while k < fin {
        let p = key_of(k);
        if vm.has_property(o, &p)? {
            let v = vm.get(o, &p)?;
            vm.create_data_property_or_throw(a, key_of(n), v)?;
        }
        k += 1.0;
        n += 1.0;
    }
    set_len(vm, a, n)?;
    Ok(Value::Object(a))
}

/// SortCompare with an optional comparator (§23.1.3.30.2).
fn sort_compare(vm: &mut Vm, cmp: &Value, x: &Value, y: &Value) -> JsResult<core::cmp::Ordering> {
    use core::cmp::Ordering;
    if x.is_undefined() && y.is_undefined() {
        return Ok(Ordering::Equal);
    }
    if x.is_undefined() {
        return Ok(Ordering::Greater);
    }
    if y.is_undefined() {
        return Ok(Ordering::Less);
    }
    if !cmp.is_undefined() {
        let r = vm.call(cmp, &Value::Undefined, &[x.clone(), y.clone()])?;
        let n = vm.to_number(&r)?;
        return Ok(if n < 0.0 {
            Ordering::Less
        } else if n > 0.0 {
            Ordering::Greater
        } else {
            Ordering::Equal
        });
    }
    let xs = vm.to_string(x)?;
    let ys = vm.to_string(y)?;
    Ok(xs.units().cmp(ys.units()))
}

/// Stable merge sort calling back into JavaScript (errors abort the sort).
pub fn sort_values(vm: &mut Vm, items: &mut Vec<Value>, cmp: &Value) -> JsResult<()> {
    let n = items.len();
    if n < 2 {
        return Ok(());
    }
    let mut buf: Vec<Value> = items.clone();
    let mut width = 1;
    // insertion sort small runs of 8 first
    let run = 8;
    let mut i = 0;
    while i < n {
        let end = (i + run).min(n);
        for j in i + 1..end {
            let mut k = j;
            while k > i {
                if sort_compare(vm, cmp, &items[k - 1], &items[k])? == core::cmp::Ordering::Greater {
                    items.swap(k - 1, k);
                    k -= 1;
                } else {
                    break;
                }
            }
        }
        i = end;
    }
    width = width.max(run);
    while width < n {
        let mut lo = 0;
        while lo < n {
            let mid = (lo + width).min(n);
            let hi = (lo + 2 * width).min(n);
            let (mut a, mut b, mut o) = (lo, mid, lo);
            while a < mid && b < hi {
                if sort_compare(vm, cmp, &items[b], &items[a])? == core::cmp::Ordering::Less {
                    buf[o] = items[b].clone();
                    b += 1;
                } else {
                    buf[o] = items[a].clone();
                    a += 1;
                }
                o += 1;
            }
            while a < mid {
                buf[o] = items[a].clone();
                a += 1;
                o += 1;
            }
            while b < hi {
                buf[o] = items[b].clone();
                b += 1;
                o += 1;
            }
            lo = hi;
        }
        core::mem::swap(items, &mut buf);
        width *= 2;
    }
    Ok(())
}

/// SortIndexedProperties: collect (skipping holes), sort, return the list.
fn sort_indexed(vm: &mut Vm, o: Obj, len: f64, cmp: &Value, skip_holes: bool) -> JsResult<Vec<Value>> {
    let mut items = Vec::new();
    let mut k = 0.0;
    while k < len {
        let p = key_of(k);
        if skip_holes {
            if vm.has_property(o, &p)? {
                let v = vm.get(o, &p)?;
                vm.root(&v);
                items.push(v);
            }
        } else {
            let v = vm.get(o, &p)?;
            vm.root(&v);
            items.push(v);
        }
        k += 1.0;
    }
    sort_values(vm, &mut items, cmp)?;
    Ok(items)
}

fn sort(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let cmp = vm.arg(ctx, 0);
    if !cmp.is_undefined() && !vm.is_callable(&cmp) {
        return vm.throw_type("The comparison function must be either a function or undefined");
    }
    let (o, len) = this_len(vm, ctx)?;
    let items = sort_indexed(vm, o, len, &cmp, true)?;
    let n = items.len();
    for (i, v) in items.into_iter().enumerate() {
        vm.set_prop(o, PropertyKey::from(i as u32), v, true)?;
    }
    let mut k = n as f64;
    while k < len {
        vm.delete_property_or_throw(o, &key_of(k))?;
        k += 1.0;
    }
    Ok(Value::Object(o))
}

fn splice(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let s = vm.arg(ctx, 0);
    let start = relative_index(vm, &s, len, 0.0)?;
    let items: Vec<Value> = if ctx.argc > 2 { vm.stack[ctx.args_base + 2..ctx.args_base + ctx.argc].to_vec() } else { Vec::new() };
    let item_count = items.len() as f64;
    let del = if ctx.argc == 0 {
        0.0
    } else if ctx.argc == 1 {
        len - start
    } else {
        let dc = vm.arg(ctx, 1);
        let d = vm.to_integer_or_infinity(&dc)?;
        d.clamp(0.0, len - start)
    };
    if len + item_count - del > MAX_SAFE {
        return vm.throw_type("Array too long");
    }
    let a = vm.array_species_create(o, del)?;
    let mut k = 0.0;
    while k < del {
        let from = key_of(start + k);
        if vm.has_property(o, &from)? {
            let v = vm.get(o, &from)?;
            vm.create_data_property_or_throw(a, key_of(k), v)?;
        }
        k += 1.0;
    }
    set_len(vm, a, del)?;
    if item_count < del {
        let mut k = start;
        while k < len - del {
            let from = key_of(k + del);
            let to = key_of(k + item_count);
            if vm.has_property(o, &from)? {
                let v = vm.get(o, &from)?;
                vm.set_prop(o, to, v, true)?;
            } else {
                vm.delete_property_or_throw(o, &to)?;
            }
            k += 1.0;
        }
        let mut k = len;
        while k > len - del + item_count {
            vm.delete_property_or_throw(o, &key_of(k - 1.0))?;
            k -= 1.0;
        }
    } else if item_count > del {
        let mut k = len - del;
        while k > start {
            let from = key_of(k + del - 1.0);
            let to = key_of(k + item_count - 1.0);
            if vm.has_property(o, &from)? {
                let v = vm.get(o, &from)?;
                vm.set_prop(o, to, v, true)?;
            } else {
                vm.delete_property_or_throw(o, &to)?;
            }
            k -= 1.0;
        }
    }
    for (i, v) in items.into_iter().enumerate() {
        vm.set_prop(o, key_of(start + i as f64), v, true)?;
    }
    set_len(vm, o, len - del + item_count)?;
    Ok(Value::Object(a))
}

fn to_locale_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let mut out: Vec<u16> = Vec::new();
    let mut k = 0.0;
    while k < len {
        if k > 0.0 {
            out.push(b',' as u16);
        }
        let v = vm.get(o, &key_of(k))?;
        if !v.is_nullish() {
            let r = vm.invoke(&v, &PropertyKey::from_str("toLocaleString"), &[])?;
            let s = vm.to_string(&r)?;
            out.extend_from_slice(s.units());
        }
        k += 1.0;
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_obj(vm, ctx)?;
    let f = vm.get(o, &PropertyKey::from_str("join"))?;
    if vm.is_callable(&f) {
        return vm.call(&f, &Value::Object(o), &[]);
    }
    let c = ctx.callee;
    let _ = c;
    let ctx2 = CallCtx { this: Value::Object(o), args_base: ctx.args_base, argc: 0, new_target: Value::Undefined, callee: ctx.callee };
    crate::builtins::object::to_string(vm, &ctx2)
}

fn to_reversed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let a = vm.array_create(len, None)?;
    let mut k = 0.0;
    while k < len {
        let v = vm.get(o, &key_of(len - k - 1.0))?;
        vm.create_data_property_or_throw(a, key_of(k), v)?;
        k += 1.0;
    }
    Ok(Value::Object(a))
}

fn to_sorted(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let cmp = vm.arg(ctx, 0);
    if !cmp.is_undefined() && !vm.is_callable(&cmp) {
        return vm.throw_type("The comparison function must be either a function or undefined");
    }
    let (o, len) = this_len(vm, ctx)?;
    let a = vm.array_create(len, None)?;
    let items = sort_indexed(vm, o, len, &cmp, false)?;
    for (i, v) in items.into_iter().enumerate() {
        vm.create_data_property_or_throw(a, PropertyKey::from(i as u32), v)?;
    }
    Ok(Value::Object(a))
}

fn to_spliced(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let s = vm.arg(ctx, 0);
    let start = relative_index(vm, &s, len, 0.0)?;
    let items: Vec<Value> = if ctx.argc > 2 { vm.stack[ctx.args_base + 2..ctx.args_base + ctx.argc].to_vec() } else { Vec::new() };
    let skip = if ctx.argc == 0 {
        0.0
    } else if ctx.argc == 1 {
        len - start
    } else {
        let dc = vm.arg(ctx, 1);
        vm.to_integer_or_infinity(&dc)?.clamp(0.0, len - start)
    };
    let new_len = len + items.len() as f64 - skip;
    if new_len > MAX_SAFE {
        return vm.throw_type("Array too long");
    }
    let a = vm.array_create(new_len, None)?;
    let mut i = 0.0;
    let mut r = start + skip;
    while i < start {
        let v = vm.get(o, &key_of(i))?;
        vm.create_data_property_or_throw(a, key_of(i), v)?;
        i += 1.0;
    }
    for v in items {
        vm.create_data_property_or_throw(a, key_of(i), v)?;
        i += 1.0;
    }
    while i < new_len {
        let v = vm.get(o, &key_of(r))?;
        vm.create_data_property_or_throw(a, key_of(i), v)?;
        i += 1.0;
        r += 1.0;
    }
    Ok(Value::Object(a))
}

fn with(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, len) = this_len(vm, ctx)?;
    let ia = vm.arg(ctx, 0);
    let rel = vm.to_integer_or_infinity(&ia)?;
    let actual = if rel >= 0.0 { rel } else { len + rel };
    if actual >= len || actual < 0.0 {
        return vm.throw_range("Invalid index");
    }
    let v = vm.arg(ctx, 1);
    let a = vm.array_create(len, None)?;
    let mut k = 0.0;
    while k < len {
        let x = if k == actual { v.clone() } else { vm.get(o, &key_of(k))? };
        vm.create_data_property_or_throw(a, key_of(k), x)?;
        k += 1.0;
    }
    Ok(Value::Object(a))
}

pub fn _unused(_d: PropDesc) {}
