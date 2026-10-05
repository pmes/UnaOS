//! Generator objects (§27.5), async generators (§27.6) and %AsyncFromSyncIteratorPrototype% (§27.1.6).

use super::*;
use crate::vm::interp::Completion;

pub fn init(vm: &mut Vm) {
    let r = vm.cur_realm as usize;
    let ip = vm.realms[r].intrinsics.iterator_proto;
    let gfp = vm.realms[r].intrinsics.generator_function_proto;
    let gp = vm.new_object(Some(ip));
    method(vm, gp, "next", 1, gen_next);
    method(vm, gp, "return", 1, gen_return);
    method(vm, gp, "throw", 1, gen_throw);
    to_str_tag(vm, gp, "Generator");
    vm.heap.get_mut(gp).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(gfp), C));
    vm.heap.get_mut(gfp).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(gp), C));
    vm.realms[r].intrinsics.generator_proto = gp;
    let aip = vm.realms[r].intrinsics.async_iterator_proto;
    let agfp = vm.realms[r].intrinsics.async_generator_function_proto;
    let agp = vm.new_object(Some(aip));
    method(vm, agp, "next", 1, agen_next);
    method(vm, agp, "return", 1, agen_return);
    method(vm, agp, "throw", 1, agen_throw);
    to_str_tag(vm, agp, "AsyncGenerator");
    vm.heap.get_mut(agp).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(agfp), C));
    vm.heap.get_mut(agfp).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(agp), C));
    vm.realms[r].intrinsics.async_generator_proto = agp;
    let afs = vm.new_object(Some(aip));
    method(vm, afs, "next", 1, afs_next);
    method(vm, afs, "return", 1, afs_return);
    method(vm, afs, "throw", 1, afs_throw);
    vm.realms[r].intrinsics.async_from_sync_iterator_proto = afs;
}

fn coro(vm: &Vm, v: &Value, kind: CoroKind) -> Option<Obj> {
    match v {
        Value::Object(o) => match &vm.heap.get(*o).kind {
            Kind::Coroutine(c) if c.kind == kind => Some(*o),
            _ => None,
        },
        _ => None,
    }
}

fn coro_state(vm: &Vm, g: Obj) -> CoroState {
    match &vm.heap.get(g).kind {
        Kind::Coroutine(c) => c.state,
        _ => CoroState::Completed,
    }
}

fn set_state(vm: &mut Vm, g: Obj, s: CoroState) {
    if let Kind::Coroutine(c) = &mut vm.heap.get_mut(g).kind {
        c.state = s;
        if s == CoroState::Completed {
            c.frame = None;
        }
    }
}

// ------------------------------------------------------------------------------------------------ sync generators

fn gen_resume(vm: &mut Vm, this: &Value, kind: u8, v: Value) -> JsResult<Value> {
    let g = match coro(vm, this, CoroKind::Generator) {
        Some(g) => g,
        None => return vm.throw_type("next method called on incompatible receiver"),
    };
    let mut st = coro_state(vm, g);
    if st == CoroState::Executing {
        return vm.throw_type("Generator is already running");
    }
    if st == CoroState::SuspendedStart && kind != 0 {
        set_state(vm, g, CoroState::Completed);
        st = CoroState::Completed;
    }
    if st == CoroState::Completed {
        return match kind {
            1 => Err(v),
            2 => Ok(Value::Object(vm.iter_result(v, true))),
            _ => Ok(Value::Object(vm.iter_result(Value::Undefined, true))),
        };
    }
    match vm.resume_coroutine(g, kind, v)? {
        Completion::Suspend(r) => Ok(r),
        Completion::Return(r) => Ok(Value::Object(vm.iter_result(r, true))),
    }
}

fn gen_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    let this = ctx.this.clone();
    gen_resume(vm, &this, 0, v)
}
fn gen_return(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    let this = ctx.this.clone();
    gen_resume(vm, &this, 2, v)
}
fn gen_throw(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    let this = ctx.this.clone();
    gen_resume(vm, &this, 1, v)
}

// ------------------------------------------------------------------------------------------------ async generators

fn new_cap(vm: &mut Vm) -> (Obj, Value, Value) {
    let p = crate::builtins::promise::new_promise(vm);
    let (res, rej) = crate::builtins::promise::create_resolving_functions(vm, p);
    (p, Value::Object(res), Value::Object(rej))
}

fn agen_enqueue(vm: &mut Vm, g: Obj, kind: u8, value: Value, cap: &(Obj, Value, Value)) {
    if let Kind::Coroutine(c) = &mut vm.heap.get_mut(g).kind {
        c.queue.push_back(AsyncGenRequest { kind, value, promise: cap.0, resolve: cap.1.clone(), reject: cap.2.clone() });
    }
}

fn agen_entry(vm: &mut Vm, ctx: &CallCtx, kind: u8) -> JsResult<Value> {
    let cap = new_cap(vm);
    let v = vm.arg(ctx, 0);
    let g = match coro(vm, &ctx.this, CoroKind::AsyncGenerator) {
        Some(g) => g,
        None => {
            let e = vm.type_error("AsyncGenerator method called on incompatible receiver");
            vm.call(&cap.2, &Value::Undefined, &[e])?;
            return Ok(Value::Object(cap.0));
        }
    };
    let mut st = coro_state(vm, g);
    match kind {
        0 => {
            if st == CoroState::Completed {
                let r = vm.iter_result(Value::Undefined, true);
                vm.call(&cap.1, &Value::Undefined, &[Value::Object(r)])?;
                return Ok(Value::Object(cap.0));
            }
            agen_enqueue(vm, g, 0, v.clone(), &cap);
            if st == CoroState::SuspendedStart || st == CoroState::SuspendedYield {
                agen_resume(vm, g, 0, v)?;
            }
        }
        2 => {
            agen_enqueue(vm, g, 2, v.clone(), &cap);
            if st == CoroState::SuspendedStart || st == CoroState::Completed {
                set_state(vm, g, CoroState::AwaitingReturn);
                agen_await_return(vm, g)?;
            } else if st == CoroState::SuspendedYield {
                agen_resume(vm, g, 2, v)?;
            }
        }
        _ => {
            if st == CoroState::SuspendedStart {
                set_state(vm, g, CoroState::Completed);
                st = CoroState::Completed;
            }
            if st == CoroState::Completed {
                vm.call(&cap.2, &Value::Undefined, &[v])?;
                return Ok(Value::Object(cap.0));
            }
            agen_enqueue(vm, g, 1, v.clone(), &cap);
            if st == CoroState::SuspendedYield {
                agen_resume(vm, g, 1, v)?;
            }
        }
    }
    Ok(Value::Object(cap.0))
}

fn agen_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    agen_entry(vm, ctx, 0)
}
fn agen_return(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    agen_entry(vm, ctx, 2)
}
fn agen_throw(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    agen_entry(vm, ctx, 1)
}

/// AsyncGeneratorCompleteStep: settle the first request.
fn complete_step(vm: &mut Vm, g: Obj, r: Result<Value, Value>, done: bool) -> JsResult<()> {
    let req = match &mut vm.heap.get_mut(g).kind {
        Kind::Coroutine(c) => c.queue.pop_front(),
        _ => None,
    };
    let req = match req {
        Some(r) => r,
        None => return Ok(()),
    };
    match r {
        Ok(v) => {
            let o = vm.iter_result(v, done);
            vm.call(&req.resolve, &Value::Undefined, &[Value::Object(o)])?;
        }
        Err(e) => {
            vm.call(&req.reject, &Value::Undefined, &[e])?;
        }
    }
    Ok(())
}

fn queue_front(vm: &Vm, g: Obj) -> Option<(u8, Value)> {
    match &vm.heap.get(g).kind {
        Kind::Coroutine(c) => c.queue.front().map(|r| (r.kind, r.value.clone())),
        _ => None,
    }
}

/// AsyncGeneratorResume + handling of how the body stopped (yield / await / completion).
fn agen_resume(vm: &mut Vm, g: Obj, kind: u8, v: Value) -> JsResult<()> {
    let r = vm.resume_coroutine(g, kind, v);
    agen_after(vm, g, r)
}

fn agen_after(vm: &mut Vm, g: Obj, r: JsResult<Completion>) -> JsResult<()> {
    let mut r = r;
    loop {
        match r {
            Ok(Completion::Suspend(v)) => {
                if coro_state(vm, g) != CoroState::SuspendedYield {
                    // Suspended at an await: resumption comes from the promise job.
                    return Ok(());
                }
                // AsyncGeneratorYield
                complete_step(vm, g, Ok(v), false)?;
                match queue_front(vm, g) {
                    Some((k, val)) => {
                        r = vm.resume_coroutine(g, k, val);
                        continue;
                    }
                    None => return Ok(()),
                }
            }
            Ok(Completion::Return(v)) => {
                set_state(vm, g, CoroState::Completed);
                complete_step(vm, g, Ok(v), true)?;
                return agen_drain(vm, g);
            }
            Err(e) => {
                if vm.terminated {
                    return Err(e);
                }
                set_state(vm, g, CoroState::Completed);
                complete_step(vm, g, Err(e), true)?;
                return agen_drain(vm, g);
            }
        }
    }
}

pub fn async_gen_resume_from_await(vm: &mut Vm, g: Obj, kind: u8, v: Value) -> JsResult<Value> {
    let r = vm.resume_coroutine(g, kind, v);
    agen_after(vm, g, r)?;
    Ok(Value::Undefined)
}

fn agen_drain(vm: &mut Vm, g: Obj) -> JsResult<()> {
    loop {
        let (k, v) = match queue_front(vm, g) {
            Some(x) => x,
            None => return Ok(()),
        };
        if k == 2 {
            set_state(vm, g, CoroState::AwaitingReturn);
            return agen_await_return(vm, g);
        }
        if k == 1 {
            complete_step(vm, g, Err(v), true)?;
        } else {
            complete_step(vm, g, Ok(Value::Undefined), true)?;
        }
    }
}

fn agen_await_return(vm: &mut Vm, g: Obj) -> JsResult<()> {
    let v = match queue_front(vm, g) {
        Some((_, v)) => v,
        None => return Ok(()),
    };
    let p = match crate::builtins::promise::promise_resolve_intrinsic(vm, v) {
        Ok(p) => p,
        Err(e) => {
            if vm.terminated {
                return Err(e);
            }
            set_state(vm, g, CoroState::Completed);
            complete_step(vm, g, Err(e), true)?;
            return agen_drain(vm, g);
        }
    };
    let ok = vm.make_native_closure("", 1, await_return_ok, alloc::vec![Value::Object(g)]);
    let err = vm.make_native_closure("", 1, await_return_err, alloc::vec![Value::Object(g)]);
    crate::builtins::promise::perform_then(vm, p, Value::Object(ok), Value::Object(err), None);
    Ok(())
}

fn await_return_ok(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let g = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let v = vm.arg(ctx, 0);
    set_state(vm, g, CoroState::Completed);
    complete_step(vm, g, Ok(v), true)?;
    agen_drain(vm, g)?;
    Ok(Value::Undefined)
}
fn await_return_err(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let g = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let v = vm.arg(ctx, 0);
    set_state(vm, g, CoroState::Completed);
    complete_step(vm, g, Err(v), true)?;
    agen_drain(vm, g)?;
    Ok(Value::Undefined)
}

// ------------------------------------------------------------------------------------------------ async-from-sync

pub fn create_async_from_sync(vm: &mut Vm, it: Value, next: Value) -> JsResult<(Value, Value)> {
    let p = vm.intr().async_from_sync_iterator_proto;
    let o = vm.alloc(ObjectData::new(Some(p), Kind::Iterator(Box::new(IterData::AsyncFromSync { iter: it, next, done: false }))));
    let n = vm.get(o, &PropertyKey::from_str("next"))?;
    Ok((Value::Object(o), n))
}

fn afs_parts(vm: &Vm, this: &Value) -> Option<(Value, Value)> {
    if let Value::Object(o) = this {
        if let Kind::Iterator(d) = &vm.heap.get(*o).kind {
            if let IterData::AsyncFromSync { iter, next, .. } = &**d {
                return Some((iter.clone(), next.clone()));
            }
        }
    }
    None
}

fn afs_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let cap = new_cap(vm);
    let (it, next) = match afs_parts(vm, &ctx.this) {
        Some(x) => x,
        None => return vm.throw_type("not an async-from-sync iterator"),
    };
    let args: Vec<Value> = if ctx.argc > 0 { alloc::vec![vm.arg(ctx, 0)] } else { Vec::new() };
    let r = vm.call(&next, &it, &args).and_then(|r| if r.is_object() { Ok(r) } else { vm.throw_type("iterator result is not an object") });
    match r {
        Ok(r) => afs_continuation(vm, r, cap, it, true),
        Err(e) => {
            vm.call(&cap.2, &Value::Undefined, &[e])?;
            Ok(Value::Object(cap.0))
        }
    }
}

fn afs_return(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let cap = new_cap(vm);
    let (it, _) = match afs_parts(vm, &ctx.this) {
        Some(x) => x,
        None => return vm.throw_type("not an async-from-sync iterator"),
    };
    let m = match vm.get_method(&it, &PropertyKey::from_str("return")) {
        Ok(m) => m,
        Err(e) => {
            vm.call(&cap.2, &Value::Undefined, &[e])?;
            return Ok(Value::Object(cap.0));
        }
    };
    let m = match m {
        None => {
            let v = vm.arg(ctx, 0);
            let o = vm.iter_result(v, true);
            vm.call(&cap.1, &Value::Undefined, &[Value::Object(o)])?;
            return Ok(Value::Object(cap.0));
        }
        Some(m) => m,
    };
    let args: Vec<Value> = if ctx.argc > 0 { alloc::vec![vm.arg(ctx, 0)] } else { Vec::new() };
    let r = vm.call(&m, &it, &args).and_then(|r| if r.is_object() { Ok(r) } else { vm.throw_type("iterator result is not an object") });
    match r {
        Ok(r) => afs_continuation(vm, r, cap, it, false),
        Err(e) => {
            vm.call(&cap.2, &Value::Undefined, &[e])?;
            Ok(Value::Object(cap.0))
        }
    }
}

fn afs_throw(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let cap = new_cap(vm);
    let (it, _) = match afs_parts(vm, &ctx.this) {
        Some(x) => x,
        None => return vm.throw_type("not an async-from-sync iterator"),
    };
    let m = match vm.get_method(&it, &PropertyKey::from_str("throw")) {
        Ok(m) => m,
        Err(e) => {
            vm.call(&cap.2, &Value::Undefined, &[e])?;
            return Ok(Value::Object(cap.0));
        }
    };
    let m = match m {
        None => {
            // Close the sync iterator, then reject with a TypeError.
            let r = vm.iterator_close(&it);
            let e = match r {
                Err(e) => e,
                Ok(()) => vm.type_error("The iterator does not provide a 'throw' method"),
            };
            vm.call(&cap.2, &Value::Undefined, &[e])?;
            return Ok(Value::Object(cap.0));
        }
        Some(m) => m,
    };
    let v = vm.arg(ctx, 0);
    let r = vm.call(&m, &it, &[v]).and_then(|r| if r.is_object() { Ok(r) } else { vm.throw_type("iterator result is not an object") });
    match r {
        Ok(r) => afs_continuation(vm, r, cap, it, true),
        Err(e) => {
            vm.call(&cap.2, &Value::Undefined, &[e])?;
            Ok(Value::Object(cap.0))
        }
    }
}

fn afs_continuation(vm: &mut Vm, result: Value, cap: (Obj, Value, Value), it: Value, close_on_rejection: bool) -> JsResult<Value> {
    let r = (|| -> JsResult<(bool, Value)> {
        let d = vm.get_v(&result, &PropertyKey::from_str("done"))?;
        let done = vm.to_boolean(&d);
        let value = vm.get_v(&result, &PropertyKey::from_str("value"))?;
        Ok((done, value))
    })();
    let (done, value) = match r {
        Ok(x) => x,
        Err(e) => {
            vm.call(&cap.2, &Value::Undefined, &[e])?;
            return Ok(Value::Object(cap.0));
        }
    };
    let wrapper = match crate::builtins::promise::promise_resolve_intrinsic(vm, value) {
        Ok(p) => p,
        Err(e) => {
            let e = if !done && close_on_rejection {
                match vm.iterator_close(&it) {
                    Err(e2) => e2,
                    Ok(()) => e,
                }
            } else {
                e
            };
            vm.call(&cap.2, &Value::Undefined, &[e])?;
            return Ok(Value::Object(cap.0));
        }
    };
    let on_ok = vm.make_native_closure("", 1, afs_unwrap, alloc::vec![Value::Bool(done)]);
    let on_err = if done || !close_on_rejection { Value::Undefined } else { Value::Object(vm.make_native_closure("", 1, afs_close_reject, alloc::vec![it])) };
    let p = cap.0;
    crate::builtins::promise::perform_then(vm, wrapper, Value::Object(on_ok), on_err, Some(cap));
    Ok(Value::Object(p))
}

fn afs_unwrap(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let done = vm.to_boolean(&vm.native_slot(ctx.callee, 0));
    let v = vm.arg(ctx, 0);
    Ok(Value::Object(vm.iter_result(v, done)))
}

fn afs_close_reject(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let it = vm.native_slot(ctx.callee, 0);
    let e = vm.arg(ctx, 0);
    let _ = vm.iterator_close(&it);
    Err(e)
}
