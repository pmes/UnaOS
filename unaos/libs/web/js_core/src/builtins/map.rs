//! Map (§24.1) and Set (§24.2), their iterators, and the ES2025 Set methods.

use super::*;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let ip = vm.intr().iterator_proto;
    let r = vm.cur_realm as usize;
    // Map
    let mp = vm.new_object(Some(op));
    let mc = ctor(vm, "Map", 0, map_ctor, mp);
    method(vm, mc, "groupBy", 2, map_group_by);
    species_getter(vm, mc);
    for (n, l, f) in [
        ("clear", 0, map_clear as NativeFn),
        ("delete", 1, map_delete),
        ("forEach", 1, map_for_each),
        ("get", 1, map_get),
        ("has", 1, map_has),
        ("set", 2, map_set),
        ("keys", 0, map_keys),
        ("values", 0, map_values),
    ] {
        method(vm, mp, n, l, f);
    }
    let entries = method(vm, mp, "entries", 0, map_entries);
    let it = PropertyKey::Sym(vm.wk.iterator.clone());
    vm.heap.get_mut(mp).props.insert(it.clone(), Prop::data(Value::Object(entries), WC));
    accessor(vm, mp, PropertyKey::from_str("size"), "size", Some(map_size), None, C);
    to_str_tag(vm, mp, "Map");
    let mip = vm.new_object(Some(ip));
    method(vm, mip, "next", 0, map_iter_next);
    to_str_tag(vm, mip, "Map Iterator");
    vm.realms[r].intrinsics.map_proto = mp;
    vm.realms[r].intrinsics.map_ctor = mc;
    vm.realms[r].intrinsics.map_iterator_proto = mip;
    global(vm, "Map", Value::Object(mc));
    // Set
    let sp = vm.new_object(Some(op));
    let sc = ctor(vm, "Set", 0, set_ctor, sp);
    species_getter(vm, sc);
    for (n, l, f) in [
        ("add", 1, set_add as NativeFn),
        ("clear", 0, set_clear),
        ("delete", 1, set_delete),
        ("entries", 0, set_entries),
        ("forEach", 1, set_for_each),
        ("has", 1, set_has),
        ("union", 1, set_union),
        ("intersection", 1, set_intersection),
        ("difference", 1, set_difference),
        ("symmetricDifference", 1, set_symmetric_difference),
        ("isSubsetOf", 1, set_is_subset_of),
        ("isSupersetOf", 1, set_is_superset_of),
        ("isDisjointFrom", 1, set_is_disjoint_from),
    ] {
        method(vm, sp, n, l, f);
    }
    let values = method(vm, sp, "values", 0, set_values);
    vm.heap.get_mut(sp).props.insert(PropertyKey::from_str("keys"), Prop::data(Value::Object(values), WC));
    vm.heap.get_mut(sp).props.insert(it, Prop::data(Value::Object(values), WC));
    accessor(vm, sp, PropertyKey::from_str("size"), "size", Some(set_size), None, C);
    to_str_tag(vm, sp, "Set");
    let sip = vm.new_object(Some(ip));
    method(vm, sip, "next", 0, set_iter_next);
    to_str_tag(vm, sip, "Set Iterator");
    vm.realms[r].intrinsics.set_proto = sp;
    vm.realms[r].intrinsics.set_ctor = sc;
    vm.realms[r].intrinsics.set_iterator_proto = sip;
    global(vm, "Set", Value::Object(sc));
}

fn this_map(vm: &mut Vm, v: &Value) -> JsResult<Obj> {
    if let Value::Object(o) = v {
        if let Kind::Map(_) = vm.heap.get(*o).kind {
            return Ok(*o);
        }
    }
    vm.throw_type("Map method called on incompatible receiver")
}
fn this_set(vm: &mut Vm, v: &Value) -> JsResult<Obj> {
    if let Value::Object(o) = v {
        if let Kind::Set(_) = vm.heap.get(*o).kind {
            return Ok(*o);
        }
    }
    vm.throw_type("Set method called on incompatible receiver")
}

pub fn map_data(vm: &mut Vm, o: Obj) -> &mut MapData {
    match &mut vm.heap.get_mut(o).kind {
        Kind::Map(m) | Kind::Set(m) | Kind::WeakMap(m) | Kind::WeakSet(m) => m,
        _ => unreachable!(),
    }
}
pub fn map_data_ref(vm: &Vm, o: Obj) -> &MapData {
    match &vm.heap.get(o).kind {
        Kind::Map(m) | Kind::Set(m) | Kind::WeakMap(m) | Kind::WeakSet(m) => m,
        _ => unreachable!(),
    }
}

/// AddEntriesFromIterable / the Set constructor loop.
pub fn add_from_iterable(vm: &mut Vm, target: Obj, iterable: &Value, adder_name: &str, entries: bool) -> JsResult<()> {
    let adder = vm.get(target, &PropertyKey::from_str(adder_name))?;
    if !vm.is_callable(&adder) {
        return vm.throw_type(&alloc::format!("'{}' is not a function", adder_name));
    }
    let (it, next) = vm.get_iterator(iterable)?;
    vm.root(&it);
    loop {
        let v = match vm.iterator_step_value(&it, &next)? {
            Some(v) => v,
            None => return Ok(()),
        };
        let r = if entries {
            match &v {
                Value::Object(eo) => {
                    let eo = *eo;
                    (|| -> JsResult<Value> {
                        let k = vm.get(eo, &PropertyKey::Index(0))?;
                        let val = vm.get(eo, &PropertyKey::Index(1))?;
                        vm.call(&adder, &Value::Object(target), &[k, val])
                    })()
                }
                _ => Err(vm.type_error("Iterator value is not an entry object")),
            }
        } else {
            vm.call(&adder, &Value::Object(target), &[v])
        };
        if let Err(e) = r {
            let _ = vm.iterator_close(&it);
            return Err(e);
        }
    }
}

fn map_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor Map requires 'new'");
    }
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.map_proto)?;
    let m = vm.alloc(ObjectData::new(Some(p), Kind::Map(Box::default())));
    let it = vm.arg(ctx, 0);
    if !it.is_nullish() {
        add_from_iterable(vm, m, &it, "set", true)?;
    }
    Ok(Value::Object(m))
}

fn map_group_by(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let groups = crate::builtins::object::group_by_impl(vm, ctx, false)?;
    let p = vm.intr().map_proto;
    let m = vm.alloc(ObjectData::new(Some(p), Kind::Map(Box::default())));
    for (k, vals) in groups {
        let arr = vm.new_array(vals);
        map_data(vm, m).set(k, Value::Object(arr));
    }
    Ok(Value::Object(m))
}

fn map_clear(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_map(vm, &ctx.this)?;
    map_data(vm, m).clear();
    Ok(Value::Undefined)
}
fn map_delete(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_map(vm, &ctx.this)?;
    let k = vm.arg(ctx, 0);
    Ok(Value::Bool(map_data(vm, m).delete(&k)))
}
fn map_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_map(vm, &ctx.this)?;
    let k = vm.arg(ctx, 0);
    Ok(map_data_ref(vm, m).get(&k).cloned().unwrap_or(Value::Undefined))
}
fn map_has(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_map(vm, &ctx.this)?;
    let k = vm.arg(ctx, 0);
    Ok(Value::Bool(map_data_ref(vm, m).has(&k)))
}
fn map_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_map(vm, &ctx.this)?;
    let k = vm.arg(ctx, 0);
    let v = vm.arg(ctx, 1);
    map_data(vm, m).set(k, v);
    Ok(Value::Object(m))
}
fn map_size(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_map(vm, &ctx.this)?;
    Ok(Value::Number(map_data_ref(vm, m).live as f64))
}
fn map_for_each(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_map(vm, &ctx.this)?;
    let f = vm.arg(ctx, 0);
    if !vm.is_callable(&f) {
        return vm.throw_type("callback is not a function");
    }
    let t = vm.arg(ctx, 1);
    let mut i = map_data_ref(vm, m).offset;
    loop {
        let end = map_data_ref(vm, m).end();
        if i >= end {
            break;
        }
        let e = map_data_ref(vm, m).at(i).cloned();
        if let Some((k, v)) = e {
            vm.call(&f, &t, &[v, k, Value::Object(m)])?;
        }
        i += 1;
        let off = map_data_ref(vm, m).offset;
        if i < off {
            i = off;
        }
    }
    Ok(Value::Undefined)
}

fn make_iter(vm: &mut Vm, o: Obj, kind: IterKind, map: bool) -> Value {
    let start = map_data_ref(vm, o).offset;
    let (p, d) = if map {
        (vm.intr().map_iterator_proto, IterData::Map { target: Some(o), index: start, kind })
    } else {
        (vm.intr().set_iterator_proto, IterData::Set { target: Some(o), index: start, kind })
    };
    Value::Object(vm.alloc(ObjectData::new(Some(p), Kind::Iterator(Box::new(d)))))
}
fn map_keys(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_map(vm, &ctx.this)?;
    Ok(make_iter(vm, m, IterKind::Keys, true))
}
fn map_values(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_map(vm, &ctx.this)?;
    Ok(make_iter(vm, m, IterKind::Values, true))
}
fn map_entries(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let m = this_map(vm, &ctx.this)?;
    Ok(make_iter(vm, m, IterKind::Entries, true))
}

fn iter_next(vm: &mut Vm, this: &Value, map: bool) -> JsResult<Value> {
    let o = match this {
        Value::Object(o) => *o,
        _ => return vm.throw_type("next called on incompatible receiver"),
    };
    let (target, index, kind) = match &vm.heap.get(o).kind {
        Kind::Iterator(d) => match (&**d, map) {
            (IterData::Map { target, index, kind }, true) | (IterData::Set { target, index, kind }, false) => (*target, *index, *kind),
            _ => return vm.throw_type("next called on incompatible receiver"),
        },
        _ => return vm.throw_type("next called on incompatible receiver"),
    };
    let t = match target {
        Some(t) => t,
        None => return Ok(Value::Object(vm.iter_result(Value::Undefined, true))),
    };
    let md = map_data_ref(vm, t);
    let mut i = index.max(md.offset);
    let end = md.end();
    let mut found = None;
    while i < end {
        if let Some((k, v)) = md.at(i) {
            found = Some((k.clone(), v.clone()));
            i += 1;
            break;
        }
        i += 1;
    }
    let set_state = |vm: &mut Vm, idx: usize, done: bool| {
        if let Kind::Iterator(d) = &mut vm.heap.get_mut(o).kind {
            match &mut **d {
                IterData::Map { target, index, .. } | IterData::Set { target, index, .. } => {
                    *index = idx;
                    if done {
                        *target = None;
                    }
                }
                _ => {}
            }
        }
    };
    match found {
        None => {
            set_state(vm, i, true);
            Ok(Value::Object(vm.iter_result(Value::Undefined, true)))
        }
        Some((k, v)) => {
            set_state(vm, i, false);
            let r = match kind {
                IterKind::Keys => k,
                IterKind::Values => {
                    if map {
                        v
                    } else {
                        k
                    }
                }
                IterKind::Entries => {
                    let second = if map { v } else { k.clone() };
                    Value::Object(vm.new_array(alloc::vec![k, second]))
                }
            };
            Ok(Value::Object(vm.iter_result(r, false)))
        }
    }
}
fn map_iter_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let this = ctx.this.clone();
    iter_next(vm, &this, true)
}
fn set_iter_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let this = ctx.this.clone();
    iter_next(vm, &this, false)
}

// ------------------------------------------------------------------------------------------------ Set

fn set_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor Set requires 'new'");
    }
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.set_proto)?;
    let s = vm.alloc(ObjectData::new(Some(p), Kind::Set(Box::default())));
    let it = vm.arg(ctx, 0);
    if !it.is_nullish() {
        add_from_iterable(vm, s, &it, "add", false)?;
    }
    Ok(Value::Object(s))
}
fn set_add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    let k = vm.arg(ctx, 0);
    if !map_data_ref(vm, s).has(&k) {
        map_data(vm, s).set(k, Value::Undefined);
    }
    Ok(Value::Object(s))
}
fn set_clear(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    map_data(vm, s).clear();
    Ok(Value::Undefined)
}
fn set_delete(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    let k = vm.arg(ctx, 0);
    Ok(Value::Bool(map_data(vm, s).delete(&k)))
}
fn set_has(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    let k = vm.arg(ctx, 0);
    Ok(Value::Bool(map_data_ref(vm, s).has(&k)))
}
fn set_size(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    Ok(Value::Number(map_data_ref(vm, s).live as f64))
}
fn set_values(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    Ok(make_iter(vm, s, IterKind::Values, false))
}
fn set_entries(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    Ok(make_iter(vm, s, IterKind::Entries, false))
}
fn set_for_each(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    let f = vm.arg(ctx, 0);
    if !vm.is_callable(&f) {
        return vm.throw_type("callback is not a function");
    }
    let t = vm.arg(ctx, 1);
    let mut i = map_data_ref(vm, s).offset;
    loop {
        let end = map_data_ref(vm, s).end();
        if i >= end {
            break;
        }
        let e = map_data_ref(vm, s).at(i).cloned();
        if let Some((k, _)) = e {
            vm.call(&f, &t, &[k.clone(), k, Value::Object(s)])?;
        }
        i += 1;
        let off = map_data_ref(vm, s).offset;
        if i < off {
            i = off;
        }
    }
    Ok(Value::Undefined)
}

struct SetRecord {
    obj: Value,
    size: f64,
    has: Value,
    keys: Value,
}

/// GetSetRecord (§24.2.1.2)
fn get_set_record(vm: &mut Vm, v: &Value) -> JsResult<SetRecord> {
    if !v.is_object() {
        return vm.throw_type("Set method argument is not an object");
    }
    let rs = vm.get_v(v, &PropertyKey::from_str("size"))?;
    let n = vm.to_number(&rs)?;
    if n.is_nan() {
        return vm.throw_type("The 'size' property must be a number");
    }
    let size = crate::vm::ops::integer_or_infinity(n);
    if size < 0.0 {
        return vm.throw_range("The 'size' property must not be negative");
    }
    let has = vm.get_v(v, &PropertyKey::from_str("has"))?;
    if !vm.is_callable(&has) {
        return vm.throw_type("The 'has' property must be a function");
    }
    let keys = vm.get_v(v, &PropertyKey::from_str("keys"))?;
    if !vm.is_callable(&keys) {
        return vm.throw_type("The 'keys' property must be a function");
    }
    Ok(SetRecord { obj: v.clone(), size, has, keys })
}

fn keys_iter(vm: &mut Vm, r: &SetRecord) -> JsResult<(Value, Value)> {
    let it = vm.call(&r.keys, &r.obj, &[])?;
    if !it.is_object() {
        return vm.throw_type("keys() result is not an object");
    }
    let next = vm.get_v(&it, &PropertyKey::from_str("next"))?;
    Ok((it, next))
}

fn set_elements(vm: &Vm, s: Obj) -> Vec<Value> {
    map_data_ref(vm, s).entries.iter().flatten().map(|(k, _)| k.clone()).collect()
}

fn new_set_from(vm: &mut Vm, items: Vec<Value>) -> Value {
    let p = vm.intr().set_proto;
    let s = vm.alloc(ObjectData::new(Some(p), Kind::Set(Box::default())));
    for k in items {
        let k = match k {
            Value::Number(n) if n == 0.0 => Value::Number(0.0),
            other => other,
        };
        if !map_data_ref(vm, s).has(&k) {
            map_data(vm, s).set(k, Value::Undefined);
        }
    }
    Value::Object(s)
}

fn norm(k: Value) -> Value {
    match k {
        Value::Number(n) if n == 0.0 => Value::Number(0.0),
        other => other,
    }
}

fn set_union(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    let other = vm.arg(ctx, 0);
    let r = get_set_record(vm, &other)?;
    let (it, next) = keys_iter(vm, &r)?;
    let mut result = set_elements(vm, s);
    while let Some(k) = vm.iterator_step_value(&it, &next)? {
        let k = norm(k);
        if !result.iter().any(|x| x.same_value_zero(&k)) {
            result.push(k);
        }
    }
    Ok(new_set_from(vm, result))
}

fn set_intersection(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    let other = vm.arg(ctx, 0);
    let r = get_set_record(vm, &other)?;
    let mut result: Vec<Value> = Vec::new();
    if (map_data_ref(vm, s).live as f64) <= r.size {
        let mut i = map_data_ref(vm, s).offset;
        loop {
            let end = map_data_ref(vm, s).end();
            if i >= end {
                break;
            }
            let e = map_data_ref(vm, s).at(i).cloned();
            i += 1;
            if let Some((k, _)) = e {
                let inr = vm.call(&r.has, &r.obj, &[k.clone()])?;
                if vm.to_boolean(&inr) && !result.iter().any(|x| x.same_value_zero(&k)) {
                    result.push(k);
                }
            }
        }
    } else {
        let (it, next) = keys_iter(vm, &r)?;
        while let Some(k) = vm.iterator_step_value(&it, &next)? {
            let k = norm(k);
            if map_data_ref(vm, s).has(&k) && !result.iter().any(|x| x.same_value_zero(&k)) {
                result.push(k);
            }
        }
    }
    Ok(new_set_from(vm, result))
}

fn set_difference(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    let other = vm.arg(ctx, 0);
    let r = get_set_record(vm, &other)?;
    let mut result = set_elements(vm, s);
    if (map_data_ref(vm, s).live as f64) <= r.size {
        let snapshot = result.clone();
        for k in snapshot {
            let inr = vm.call(&r.has, &r.obj, &[k.clone()])?;
            if vm.to_boolean(&inr) {
                result.retain(|x| !x.same_value_zero(&k));
            }
        }
    } else {
        let (it, next) = keys_iter(vm, &r)?;
        while let Some(k) = vm.iterator_step_value(&it, &next)? {
            let k = norm(k);
            result.retain(|x| !x.same_value_zero(&k));
        }
    }
    Ok(new_set_from(vm, result))
}

fn set_symmetric_difference(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    let other = vm.arg(ctx, 0);
    let r = get_set_record(vm, &other)?;
    let (it, next) = keys_iter(vm, &r)?;
    let mut result = set_elements(vm, s);
    while let Some(k) = vm.iterator_step_value(&it, &next)? {
        let k = norm(k);
        let in_this = map_data_ref(vm, s).has(&k);
        if in_this {
            result.retain(|x| !x.same_value_zero(&k));
        } else if !result.iter().any(|x| x.same_value_zero(&k)) {
            result.push(k);
        }
    }
    Ok(new_set_from(vm, result))
}

fn set_is_subset_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    let other = vm.arg(ctx, 0);
    let r = get_set_record(vm, &other)?;
    if (map_data_ref(vm, s).live as f64) > r.size {
        return Ok(Value::Bool(false));
    }
    let mut i = map_data_ref(vm, s).offset;
    loop {
        let end = map_data_ref(vm, s).end();
        if i >= end {
            break;
        }
        let e = map_data_ref(vm, s).at(i).cloned();
        i += 1;
        if let Some((k, _)) = e {
            let inr = vm.call(&r.has, &r.obj, &[k])?;
            if !vm.to_boolean(&inr) {
                return Ok(Value::Bool(false));
            }
        }
    }
    Ok(Value::Bool(true))
}

fn set_is_superset_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    let other = vm.arg(ctx, 0);
    let r = get_set_record(vm, &other)?;
    if (map_data_ref(vm, s).live as f64) < r.size {
        return Ok(Value::Bool(false));
    }
    let (it, next) = keys_iter(vm, &r)?;
    while let Some(k) = vm.iterator_step_value(&it, &next)? {
        if !map_data_ref(vm, s).has(&k) {
            vm.iterator_close(&it)?;
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn set_is_disjoint_from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_set(vm, &ctx.this)?;
    let other = vm.arg(ctx, 0);
    let r = get_set_record(vm, &other)?;
    if (map_data_ref(vm, s).live as f64) <= r.size {
        let mut i = map_data_ref(vm, s).offset;
        loop {
            let end = map_data_ref(vm, s).end();
            if i >= end {
                break;
            }
            let e = map_data_ref(vm, s).at(i).cloned();
            i += 1;
            if let Some((k, _)) = e {
                let inr = vm.call(&r.has, &r.obj, &[k])?;
                if vm.to_boolean(&inr) {
                    return Ok(Value::Bool(false));
                }
            }
        }
    } else {
        let (it, next) = keys_iter(vm, &r)?;
        while let Some(k) = vm.iterator_step_value(&it, &next)? {
            if map_data_ref(vm, s).has(&k) {
                vm.iterator_close(&it)?;
                return Ok(Value::Bool(false));
            }
        }
    }
    Ok(Value::Bool(true))
}
