//! Promise (§27.2): states, resolving functions, reaction / thenable jobs, then / catch / finally and the
//! combinators.

use super::*;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let proto = vm.new_object(Some(op));
    let c = ctor(vm, "Promise", 1, promise_ctor, proto);
    for (n, l, f) in [
        ("all", 1, all as NativeFn),
        ("allSettled", 1, all_settled),
        ("any", 1, any),
        ("race", 1, race),
        ("reject", 1, reject_static),
        ("resolve", 1, resolve_static),
        ("try", 1, try_static),
        ("withResolvers", 0, with_resolvers),
    ] {
        method(vm, c, n, l, f);
    }
    species_getter(vm, c);
    let then = method(vm, proto, "then", 2, then);
    method(vm, proto, "catch", 1, catch);
    method(vm, proto, "finally", 1, finally);
    to_str_tag(vm, proto, "Promise");
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.promise_proto = proto;
    vm.realms[r].intrinsics.promise_ctor = c;
    vm.realms[r].intrinsics.promise_then = then;
    global(vm, "Promise", Value::Object(c));
}

pub fn new_promise(vm: &mut Vm) -> Obj {
    let p = vm.intr().promise_proto;
    vm.alloc(ObjectData::new(Some(p), Kind::Promise(Box::new(PromiseData { state: PromiseState::Pending, result: Value::Undefined, fulfill_reactions: Vec::new(), reject_reactions: Vec::new(), handled: false }))))
}

fn is_promise(vm: &Vm, v: &Value) -> Option<Obj> {
    match v {
        Value::Object(o) if matches!(vm.heap.get(*o).kind, Kind::Promise(_)) => Some(*o),
        _ => None,
    }
}

fn state(vm: &Vm, p: Obj) -> PromiseState {
    match &vm.heap.get(p).kind {
        Kind::Promise(d) => d.state,
        _ => PromiseState::Pending,
    }
}

/// FulfillPromise / RejectPromise + TriggerPromiseReactions.
fn settle(vm: &mut Vm, p: Obj, v: Value, fulfill: bool) {
    let (reactions, handled) = match &mut vm.heap.get_mut(p).kind {
        Kind::Promise(d) => {
            if d.state != PromiseState::Pending {
                return;
            }
            d.state = if fulfill { PromiseState::Fulfilled } else { PromiseState::Rejected };
            d.result = v.clone();
            let f = core::mem::take(&mut d.fulfill_reactions);
            let r = core::mem::take(&mut d.reject_reactions);
            (if fulfill { f } else { r }, d.handled)
        }
        _ => return,
    };
    if !fulfill && !handled {
        vm.host.promise_rejection(p, 0);
    }
    let realm = vm.cur_realm;
    for r in reactions {
        vm.jobs.push_back(Job::Reaction { handler: r.handler, argument: v.clone(), capability: r.capability, fulfill, realm });
    }
}

pub fn reject_promise(vm: &mut Vm, p: Obj, reason: Value) {
    settle(vm, p, reason, false);
}

/// The [[Resolve]] semantics of a promise's resolving function (§27.2.1.3.2) without the already-resolved
/// record (callers guarantee single resolution).
pub fn resolve_promise(vm: &mut Vm, p: Obj, resolution: Value) -> JsResult<()> {
    if let Value::Object(r) = &resolution {
        if *r == p {
            let e = vm.type_error("Chaining cycle detected for promise");
            reject_promise(vm, p, e);
            return Ok(());
        }
        // The getter may run user code (and collect): p and the resolution may be held only here.
        let mark = vm.temp_roots.len();
        vm.temp_roots.push(Value::Object(p));
        vm.temp_roots.push(resolution.clone());
        let then = vm.get(*r, &PropertyKey::from_str("then"));
        vm.temp_roots.truncate(mark);
        let then = match then {
            Ok(t) => t,
            Err(e) => {
                if vm.terminated {
                    return Err(e);
                }
                reject_promise(vm, p, e);
                return Ok(());
            }
        };
        if vm.is_callable(&then) {
            let realm = vm.cur_realm;
            vm.jobs.push_back(Job::Thenable { promise: p, thenable: resolution.clone(), then, realm });
            return Ok(());
        }
    }
    settle(vm, p, resolution, true);
    Ok(())
}

/// CreateResolvingFunctions(promise): (resolve, reject) sharing an already-resolved record.
pub fn create_resolving_functions(vm: &mut Vm, p: Obj) -> (Obj, Obj) {
    let rec = vm.alloc(ObjectData::new(None, Kind::Internal(alloc::vec![Value::Bool(false)])));
    let res = vm.make_native_closure("", 1, resolve_fn, alloc::vec![Value::Object(p), Value::Object(rec)]);
    let rej = vm.make_native_closure("", 1, reject_fn, alloc::vec![Value::Object(p), Value::Object(rec)]);
    (res, rej)
}

fn take_resolved(vm: &mut Vm, rec: Obj) -> bool {
    match &mut vm.heap.get_mut(rec).kind {
        Kind::Internal(v) => {
            let was = matches!(v[0], Value::Bool(true));
            v[0] = Value::Bool(true);
            was
        }
        _ => true,
    }
}

fn resolve_fn(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let p = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let rec = vm.native_slot(ctx.callee, 1).as_object().unwrap();
    if take_resolved(vm, rec) {
        return Ok(Value::Undefined);
    }
    let v = vm.arg(ctx, 0);
    resolve_promise(vm, p, v)?;
    Ok(Value::Undefined)
}

fn reject_fn(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let p = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let rec = vm.native_slot(ctx.callee, 1).as_object().unwrap();
    if take_resolved(vm, rec) {
        return Ok(Value::Undefined);
    }
    let v = vm.arg(ctx, 0);
    reject_promise(vm, p, v);
    Ok(Value::Undefined)
}

pub fn resolve_thenable_job(vm: &mut Vm, p: Obj, thenable: Value, then: Value) -> JsResult<()> {
    let (res, rej) = create_resolving_functions(vm, p);
    if let Err(e) = vm.call(&then, &thenable, &[Value::Object(res), Value::Object(rej)]) {
        if vm.terminated {
            return Err(e);
        }
        vm.call(&Value::Object(rej), &Value::Undefined, &[e])?;
    }
    Ok(())
}

pub fn reaction_job(vm: &mut Vm, handler: Value, arg: Value, cap: Option<(Obj, Value, Value)>, fulfill: bool) -> JsResult<()> {
    let r = if handler.is_undefined() {
        if fulfill {
            Ok(arg)
        } else {
            Err(arg)
        }
    } else {
        vm.call(&handler, &Value::Undefined, &[arg])
    };
    if vm.terminated {
        return r.map(|_| ());
    }
    if let Some((_, res, rej)) = cap {
        match r {
            Ok(v) => {
                vm.call(&res, &Value::Undefined, &[v])?;
            }
            Err(e) => {
                vm.call(&rej, &Value::Undefined, &[e])?;
            }
        }
    }
    Ok(())
}

/// PerformPromiseThen (§27.2.5.4.1)
pub fn perform_then(vm: &mut Vm, p: Obj, on_ok: Value, on_err: Value, cap: Option<(Obj, Value, Value)>) {
    let on_ok = if vm.is_callable(&on_ok) { on_ok } else { Value::Undefined };
    let on_err = if vm.is_callable(&on_err) { on_err } else { Value::Undefined };
    let realm = vm.cur_realm;
    let (st, res, handled) = match &mut vm.heap.get_mut(p).kind {
        Kind::Promise(d) => {
            let h = d.handled;
            d.handled = true;
            (d.state, d.result.clone(), h)
        }
        _ => return,
    };
    match st {
        PromiseState::Pending => {
            if let Kind::Promise(d) = &mut vm.heap.get_mut(p).kind {
                d.fulfill_reactions.push(Reaction { capability: cap.clone(), fulfill: true, handler: on_ok });
                d.reject_reactions.push(Reaction { capability: cap, fulfill: false, handler: on_err });
            }
        }
        PromiseState::Fulfilled => vm.jobs.push_back(Job::Reaction { handler: on_ok, argument: res, capability: cap, fulfill: true, realm }),
        PromiseState::Rejected => {
            if !handled {
                vm.host.promise_rejection(p, 1);
            }
            vm.jobs.push_back(Job::Reaction { handler: on_err, argument: res, capability: cap, fulfill: false, realm })
        }
    }
}

/// PromiseResolve(%Promise%, x)
pub fn promise_resolve_intrinsic(vm: &mut Vm, x: Value) -> JsResult<Obj> {
    let c = vm.intr().promise_ctor;
    if let Some(p) = is_promise(vm, &x) {
        let mark = vm.temp_roots.len();
        vm.temp_roots.push(x.clone());
        let xc = vm.get(p, &PropertyKey::from_str("constructor"));
        vm.temp_roots.truncate(mark);
        let xc = xc?;
        if matches!(xc, Value::Object(o) if o == c) {
            return Ok(p);
        }
    }
    let p = new_promise(vm);
    resolve_promise(vm, p, x)?;
    Ok(p)
}

/// NewPromiseCapability(C) -> (promise, resolve, reject)
pub fn new_capability(vm: &mut Vm, c: &Value) -> JsResult<(Obj, Value, Value)> {
    if !vm.is_constructor(c) {
        return vm.throw_type("Promise resolver is not a constructor");
    }
    if let Value::Object(co) = c {
        if *co == vm.intr().promise_ctor {
            let p = new_promise(vm);
            let (res, rej) = create_resolving_functions(vm, p);
            return Ok((p, Value::Object(res), Value::Object(rej)));
        }
    }
    let rec = vm.alloc(ObjectData::new(None, Kind::Internal(alloc::vec![Value::Undefined, Value::Undefined])));
    let exec = vm.make_native_closure("", 2, capability_executor, alloc::vec![Value::Object(rec)]);
    let p = vm.construct(c, &[Value::Object(exec)], None)?;
    let (res, rej) = match &vm.heap.get(rec).kind {
        Kind::Internal(v) => (v[0].clone(), v[1].clone()),
        _ => unreachable!(),
    };
    if !vm.is_callable(&res) || !vm.is_callable(&rej) {
        return vm.throw_type("Promise resolve or reject function is not callable");
    }
    match p {
        Value::Object(po) => Ok((po, res, rej)),
        _ => vm.throw_type("Promise constructor returned a non-object"),
    }
}

fn capability_executor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let rec = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let (r0, r1) = match &vm.heap.get(rec).kind {
        Kind::Internal(v) => (v[0].clone(), v[1].clone()),
        _ => unreachable!(),
    };
    if !r0.is_undefined() || !r1.is_undefined() {
        return vm.throw_type("Promise executor has already been invoked with non-undefined arguments");
    }
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    if let Kind::Internal(v) = &mut vm.heap.get_mut(rec).kind {
        v[0] = a;
        v[1] = b;
    }
    Ok(Value::Undefined)
}

fn promise_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Promise constructor cannot be invoked without 'new'");
    }
    let exec = vm.arg(ctx, 0);
    if !vm.is_callable(&exec) {
        return vm.throw_type("Promise resolver is not a function");
    }
    let proto = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.promise_proto)?;
    let p = new_promise(vm);
    vm.heap.get_mut(p).proto = Some(proto);
    let (res, rej) = create_resolving_functions(vm, p);
    if let Err(e) = vm.call(&exec, &Value::Undefined, &[Value::Object(res), Value::Object(rej)]) {
        if vm.terminated {
            return Err(e);
        }
        vm.call(&Value::Object(rej), &Value::Undefined, &[e])?;
    }
    Ok(Value::Object(p))
}

fn then(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let p = match is_promise(vm, &ctx.this) {
        Some(p) => p,
        None => return vm.throw_type("Promise.prototype.then called on incompatible receiver"),
    };
    let def = vm.intr().promise_ctor;
    let c = vm.species_constructor(p, def)?;
    let cap = new_capability(vm, &c)?;
    let res = Value::Object(cap.0);
    let on_ok = vm.arg(ctx, 0);
    let on_err = vm.arg(ctx, 1);
    perform_then(vm, p, on_ok, on_err, Some(cap));
    Ok(res)
}

fn catch(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let this = ctx.this.clone();
    let on_err = vm.arg(ctx, 0);
    vm.invoke(&this, &PropertyKey::from_str("then"), &[Value::Undefined, on_err])
}

fn finally(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let p = match &ctx.this {
        Value::Object(o) => *o,
        _ => return vm.throw_type("Promise.prototype.finally called on a non-object"),
    };
    let def = vm.intr().promise_ctor;
    let c = vm.species_constructor(p, def)?;
    let on_finally = vm.arg(ctx, 0);
    let (a, b) = if !vm.is_callable(&on_finally) {
        (on_finally.clone(), on_finally)
    } else {
        let tf = vm.make_native_closure("", 1, then_finally, alloc::vec![on_finally.clone(), c.clone()]);
        let cf = vm.make_native_closure("", 1, catch_finally, alloc::vec![on_finally, c]);
        (Value::Object(tf), Value::Object(cf))
    };
    vm.invoke(&Value::Object(p), &PropertyKey::from_str("then"), &[a, b])
}

fn then_finally(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let f = vm.native_slot(ctx.callee, 0);
    let c = vm.native_slot(ctx.callee, 1);
    let v = vm.arg(ctx, 0);
    let r = vm.call(&f, &Value::Undefined, &[])?;
    let p = promise_resolve(vm, &c, r)?;
    let thunk = vm.make_native_closure("", 0, value_thunk, alloc::vec![v]);
    vm.invoke(&Value::Object(p), &PropertyKey::from_str("then"), &[Value::Object(thunk)])
}
fn catch_finally(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let f = vm.native_slot(ctx.callee, 0);
    let c = vm.native_slot(ctx.callee, 1);
    let v = vm.arg(ctx, 0);
    let r = vm.call(&f, &Value::Undefined, &[])?;
    let p = promise_resolve(vm, &c, r)?;
    let thrower = vm.make_native_closure("", 0, throw_thunk, alloc::vec![v]);
    vm.invoke(&Value::Object(p), &PropertyKey::from_str("then"), &[Value::Object(thrower)])
}
fn value_thunk(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(vm.native_slot(ctx.callee, 0))
}
fn throw_thunk(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Err(vm.native_slot(ctx.callee, 0))
}

/// PromiseResolve(C, x)
pub fn promise_resolve(vm: &mut Vm, c: &Value, x: Value) -> JsResult<Obj> {
    if !c.is_object() {
        return vm.throw_type("PromiseResolve called on non-object");
    }
    if let Some(p) = is_promise(vm, &x) {
        let mark = vm.temp_roots.len();
        vm.temp_roots.push(x.clone());
        let xc = vm.get(p, &PropertyKey::from_str("constructor"));
        vm.temp_roots.truncate(mark);
        let xc = xc?;
        if xc.same_value(c) {
            return Ok(p);
        }
    }
    let (p, res, _) = new_capability(vm, c)?;
    vm.call(&res, &Value::Undefined, &[x])?;
    Ok(p)
}

fn resolve_static(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let c = ctx.this.clone();
    if !c.is_object() {
        return vm.throw_type("Promise.resolve called on non-object");
    }
    let x = vm.arg(ctx, 0);
    Ok(Value::Object(promise_resolve(vm, &c, x)?))
}

fn reject_static(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let c = ctx.this.clone();
    let (p, _, rej) = new_capability(vm, &c)?;
    let r = vm.arg(ctx, 0);
    vm.call(&rej, &Value::Undefined, &[r])?;
    Ok(Value::Object(p))
}

fn try_static(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let c = ctx.this.clone();
    if !c.is_object() {
        return vm.throw_type("Promise.try called on non-object");
    }
    let f = vm.arg(ctx, 0);
    let args: Vec<Value> = if ctx.argc > 1 { vm.stack[ctx.args_base + 1..ctx.args_base + ctx.argc].to_vec() } else { Vec::new() };
    match vm.call(&f, &Value::Undefined, &args) {
        // A normal completion goes through PromiseResolve (a promise of this constructor is returned as is).
        Ok(v) => Ok(Value::Object(promise_resolve(vm, &c, v)?)),
        Err(e) => {
            if vm.terminated {
                return Err(e);
            }
            vm.root(&e);
            let (p, _res, rej) = new_capability(vm, &c)?;
            vm.call(&rej, &Value::Undefined, &[e])?;
            Ok(Value::Object(p))
        }
    }
}

fn with_resolvers(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let c = ctx.this.clone();
    let (p, res, rej) = new_capability(vm, &c)?;
    let o = vm.new_plain_object();
    vm.create_data_property_or_throw(o, PropertyKey::from_str("promise"), Value::Object(p))?;
    vm.create_data_property_or_throw(o, PropertyKey::from_str("resolve"), res)?;
    vm.create_data_property_or_throw(o, PropertyKey::from_str("reject"), rej)?;
    Ok(Value::Object(o))
}

// ------------------------------------------------------------------------------------------------ combinators

#[derive(Clone, Copy, PartialEq, Eq)]
enum Comb {
    All,
    AllSettled,
    Any,
    Race,
}

fn combinator(vm: &mut Vm, ctx: &CallCtx, kind: Comb) -> JsResult<Value> {
    let c = ctx.this.clone();
    let cap = new_capability(vm, &c)?;
    let (p, _res, rej) = cap.clone();
    vm.root(&Value::Object(p));
    // GetPromiseResolve
    let pr = match vm.get_v(&c, &PropertyKey::from_str("resolve")) {
        Ok(f) if vm.is_callable(&f) => f,
        Ok(_) => {
            let e = vm.type_error("Promise resolve is not a function");
            vm.call(&rej, &Value::Undefined, &[e])?;
            return Ok(Value::Object(p));
        }
        Err(e) => {
            vm.call(&rej, &Value::Undefined, &[e])?;
            return Ok(Value::Object(p));
        }
    };
    let iterable = vm.arg(ctx, 0);
    let (it, next) = match vm.get_iterator(&iterable) {
        Ok(x) => x,
        Err(e) => {
            if vm.terminated {
                return Err(e);
            }
            vm.call(&rej, &Value::Undefined, &[e])?;
            return Ok(Value::Object(p));
        }
    };
    vm.root(&it);
    let mut done = false;
    let r = perform_combinator(vm, kind, &c, &cap, &pr, &it, &next, &mut done);
    match r {
        Ok(v) => Ok(v),
        Err(e) => {
            if vm.terminated {
                return Err(e);
            }
            let e = if !done {
                match vm.iterator_close(&it) {
                    Ok(()) => e,
                    Err(_) => e,
                }
            } else {
                e
            };
            vm.call(&rej, &Value::Undefined, &[e])?;
            Ok(Value::Object(p))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn perform_combinator(vm: &mut Vm, kind: Comb, c: &Value, cap: &(Obj, Value, Value), pr: &Value, it: &Value, next: &Value, done: &mut bool) -> JsResult<Value> {
    let (p, res, rej) = cap.clone();
    // values list and remaining counter shared with element functions
    let values = vm.new_array(Vec::new());
    let counter = vm.alloc(ObjectData::new(None, Kind::Internal(alloc::vec![Value::Number(1.0)])));
    vm.root(&Value::Object(values));
    vm.root(&Value::Object(counter));
    let mut index = 0u32;
    loop {
        *done = true;
        let v = match vm.iterator_step_value(it, next)? {
            Some(v) => v,
            None => break,
        };
        *done = false;
        if kind != Comb::Race {
            if let Kind::Array(a) = &mut vm.heap.get_mut(values).kind {
                a.elems.push(Value::Undefined);
            }
        }
        let next_promise = vm.call(pr, c, &[v])?;
        let (on_ok, on_err) = match kind {
            Comb::Race => (res.clone(), rej.clone()),
            Comb::All => {
                let f = element_fn(vm, values, counter, index, res.clone(), 0, None);
                (f, rej.clone())
            }
            Comb::AllSettled => {
                let called = vm.alloc(ObjectData::new(None, Kind::Internal(alloc::vec![Value::Bool(false)])));
                let f = element_fn(vm, values, counter, index, res.clone(), 1, Some(called));
                let g = element_fn(vm, values, counter, index, res.clone(), 2, Some(called));
                (f, g)
            }
            Comb::Any => {
                let g = element_fn(vm, values, counter, index, rej.clone(), 3, None);
                (res.clone(), g)
            }
        };
        if kind != Comb::Race {
            bump(vm, counter, 1.0);
        }
        vm.invoke(&next_promise, &PropertyKey::from_str("then"), &[on_ok, on_err])?;
        index += 1;
    }
    if kind != Comb::Race && bump(vm, counter, -1.0) == 0.0 {
        if kind == Comb::Any {
            let ap = vm.intr().aggregate_error_proto;
            let e = vm.make_error(ap, "All promises were rejected");
            let eo = e.as_object().unwrap();
            vm.heap.get_mut(eo).props.insert(PropertyKey::from_str("errors"), Prop::data(Value::Object(values), WC));
            vm.call(&rej, &Value::Undefined, &[e])?;
        } else {
            vm.call(&res, &Value::Undefined, &[Value::Object(values)])?;
        }
    }
    Ok(Value::Object(p))
}

fn bump(vm: &mut Vm, counter: Obj, d: f64) -> f64 {
    match &mut vm.heap.get_mut(counter).kind {
        Kind::Internal(v) => {
            let n = match v[0] {
                Value::Number(n) => n + d,
                _ => d,
            };
            v[0] = Value::Number(n);
            n
        }
        _ => 0.0,
    }
}

/// Promise.all resolve element (mode 0), allSettled fulfilled (1) / rejected (2), any reject element (3).
fn element_fn(vm: &mut Vm, values: Obj, counter: Obj, index: u32, finish: Value, mode: u8, shared: Option<Obj>) -> Value {
    let called = match shared {
        Some(c) => c,
        None => vm.alloc(ObjectData::new(None, Kind::Internal(alloc::vec![Value::Bool(false)]))),
    };
    // allSettled shares one "already called" record between its two functions.
    let f = vm.make_native_closure("", 1, element_called, alloc::vec![Value::Object(values), Value::Object(counter), Value::Number(index as f64), finish, Value::Number(mode as f64), Value::Object(called)]);
    Value::Object(f)
}

fn element_called(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let values = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let counter = vm.native_slot(ctx.callee, 1).as_object().unwrap();
    let index = match vm.native_slot(ctx.callee, 2) {
        Value::Number(n) => n as usize,
        _ => 0,
    };
    let finish = vm.native_slot(ctx.callee, 3);
    let mode = match vm.native_slot(ctx.callee, 4) {
        Value::Number(n) => n as u8,
        _ => 0,
    };
    let called = vm.native_slot(ctx.callee, 5).as_object().unwrap();
    if take_resolved(vm, called) {
        return Ok(Value::Undefined);
    }
    let x = vm.arg(ctx, 0);
    let v = match mode {
        1 | 2 => {
            let o = vm.new_plain_object();
            let (s, k) = if mode == 1 { ("fulfilled", "value") } else { ("rejected", "reason") };
            vm.create_data_property_or_throw(o, PropertyKey::from_str("status"), Value::str(s))?;
            vm.create_data_property_or_throw(o, PropertyKey::from_str(k), x)?;
            Value::Object(o)
        }
        _ => x,
    };
    if let Kind::Array(a) = &mut vm.heap.get_mut(values).kind {
        if index < a.elems.len() {
            a.elems[index] = v;
        }
    }
    if bump(vm, counter, -1.0) == 0.0 {
        if mode == 3 {
            let ap = vm.intr().aggregate_error_proto;
            let e = vm.make_error(ap, "All promises were rejected");
            let eo = e.as_object().unwrap();
            vm.heap.get_mut(eo).props.insert(PropertyKey::from_str("errors"), Prop::data(Value::Object(values), WC));
            return vm.call(&finish, &Value::Undefined, &[e]);
        }
        return vm.call(&finish, &Value::Undefined, &[Value::Object(values)]);
    }
    Ok(Value::Undefined)
}

fn all(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    combinator(vm, ctx, Comb::All)
}
fn all_settled(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    combinator(vm, ctx, Comb::AllSettled)
}
fn any(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    combinator(vm, ctx, Comb::Any)
}
fn race(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    combinator(vm, ctx, Comb::Race)
}

pub fn _state(vm: &Vm, p: Obj) -> PromiseState {
    state(vm, p)
}
