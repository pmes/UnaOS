//! Coroutines: suspended frames for generators (§27.5), async functions (§27.7) and async generators
//! (§27.6). A suspended frame's stack segment, handlers and registers are moved into the coroutine object;
//! resumption pushes them back and runs a nested interpreter loop.

use super::interp::Completion;
use super::*;

impl Vm {
    pub fn new_coroutine(&mut self, kind: CoroKind) -> Obj {
        let promise = if kind == CoroKind::Async { Some(crate::builtins::promise::new_promise(self)) } else { None };
        let realm = self.cur_realm;
        self.alloc(ObjectData::new(
            None,
            Kind::Coroutine(Box::new(CoroData {
                kind,
                state: CoroState::Executing,
                frame: None,
                promise,
                resolve: Value::Undefined,
                reject: Value::Undefined,
                queue: VecDeque::new(),
                realm,
            })),
        ))
    }

    /// Move the top frame into a SavedFrame (stack segment from args_base, handlers rebased).
    fn save_top_frame(&mut self) -> Box<SavedFrame> {
        let f = self.frames.pop().unwrap();
        let ab = f.args_base;
        let stack: Vec<Value> = self.stack.split_off(ab);
        self.stack.truncate(ab - 2);
        let hs: Vec<Handler> = self.handlers.split_off(f.handler_base).into_iter().map(|mut h| {
            h.sp -= ab as u32;
            h
        }).collect();
        let mut frame = f;
        frame.base -= ab;
        frame.args_base = 0;
        if let Some(caller) = self.frames.last() {
            self.cur_realm = caller.realm;
        }
        Box::new(SavedFrame { frame, stack, handlers: hs })
    }

    /// GenStart: create the generator object and suspend at the start.
    pub fn gen_start(&mut self) -> JsResult<Option<Completion>> {
        let fi = self.frames.len() - 1;
        if self.frames[fi].code.is_module {
            // Module instantiation done: suspend the body until evaluation.
            let co = self.new_coroutine(CoroKind::Generator);
            self.frames[fi].coroutine = Some(co);
            let mut saved = self.save_top_frame();
            saved.frame.entry = false;
            if let Kind::Coroutine(c) = &mut self.heap.get_mut(co).kind {
                c.frame = Some(saved);
                c.state = CoroState::SuspendedStart;
            }
            return Ok(Some(Completion::Return(Value::Object(co))));
        }
        let func = self.frames[fi].func.unwrap();
        let is_async = self.frames[fi].code.is_async;
        let realm = self.frames[fi].realm;
        let p = self.get(func, &PropertyKey::from_str("prototype"))?;
        let proto = match p {
            Value::Object(o) => o,
            _ => {
                let i = &self.realms[realm as usize].intrinsics;
                if is_async {
                    i.async_generator_proto
                } else {
                    i.generator_proto
                }
            }
        };
        let entry = self.frames[fi].entry;
        let kind = if is_async { CoroKind::AsyncGenerator } else { CoroKind::Generator };
        let g = self.alloc(ObjectData::new(
            Some(proto),
            Kind::Coroutine(Box::new(CoroData {
                kind,
                state: CoroState::SuspendedStart,
                frame: None,
                promise: None,
                resolve: Value::Undefined,
                reject: Value::Undefined,
                queue: VecDeque::new(),
                realm,
            })),
        ));
        self.frames[fi].coroutine = Some(g);
        let mut saved = self.save_top_frame();
        saved.frame.entry = false;
        if let Kind::Coroutine(c) = &mut self.heap.get_mut(g).kind {
            c.frame = Some(saved);
        }
        if entry {
            return Ok(Some(Completion::Return(Value::Object(g))));
        }
        self.stack.push(Value::Object(g));
        Ok(None)
    }

    /// Yield (sync generators wrap the value in an iterator result unless `raw`).
    pub fn suspend(&mut self, v: Value, raw: bool) -> JsResult<Option<Completion>> {
        let fi = self.frames.len() - 1;
        let co = self.frames[fi].coroutine.unwrap();
        let is_async_gen = matches!(&self.heap.get(co).kind, Kind::Coroutine(c) if c.kind == CoroKind::AsyncGenerator);
        let out = if raw || is_async_gen { v } else { Value::Object(self.iter_result(v, false)) };
        let saved = self.save_top_frame();
        if let Kind::Coroutine(c) = &mut self.heap.get_mut(co).kind {
            c.frame = Some(saved);
            c.state = CoroState::SuspendedYield;
        }
        Ok(Some(Completion::Suspend(out)))
    }

    /// Await: subscribe the coroutine's resumption to the promise and suspend.
    pub fn await_value(&mut self, v: Value) -> JsResult<Option<Completion>> {
        let fi = self.frames.len() - 1;
        let co = match self.frames[fi].coroutine {
            Some(c) => c,
            None => return self.throw_syntax("await is only valid in async functions"),
        };
        let promise = crate::builtins::promise::promise_resolve_intrinsic(self, v)?;
        let on_ok = self.make_native_closure("", 1, await_fulfilled, vec![Value::Object(co)]);
        let on_err = self.make_native_closure("", 1, await_rejected, vec![Value::Object(co)]);
        crate::builtins::promise::perform_then(self, promise, Value::Object(on_ok), Value::Object(on_err), None);
        let saved = self.save_top_frame();
        let p = if let Kind::Coroutine(c) = &mut self.heap.get_mut(co).kind {
            c.frame = Some(saved);
            // While awaiting, the coroutine is still "executing" from the language's point of view.
            c.state = CoroState::Executing;
            c.promise.map(Value::Object).unwrap_or(Value::Undefined)
        } else {
            Value::Undefined
        };
        Ok(Some(Completion::Suspend(p)))
    }

    /// Resume a suspended coroutine: kind 0 next(value), 1 throw(value), 2 return(value).
    pub fn resume_coroutine(&mut self, co: Obj, kind: u8, value: Value) -> JsResult<Completion> {
        let (saved, started) = match &mut self.heap.get_mut(co).kind {
            Kind::Coroutine(c) => {
                let started = c.state != CoroState::SuspendedStart || c.kind == CoroKind::Async;
                c.state = CoroState::Executing;
                (c.frame.take(), started)
            }
            _ => (None, false),
        };
        let saved = match saved {
            Some(s) => s,
            None => return self.throw_type("generator is not suspended"),
        };
        if self.frames.len() + self.native_depth >= self.max_depth {
            if let Kind::Coroutine(c) = &mut self.heap.get_mut(co).kind {
                c.frame = Some(saved);
                c.state = if started { CoroState::SuspendedYield } else { CoroState::SuspendedStart };
            }
            return self.throw_range("Maximum call stack size exceeded");
        }
        let SavedFrame { mut frame, stack, handlers } = *saved;
        let base = self.stack.len() + 2;
        self.stack.push(Value::Undefined);
        self.stack.push(Value::Undefined);
        self.stack.extend(stack);
        frame.args_base = base;
        frame.base += base;
        frame.handler_base = self.handlers.len();
        for mut h in handlers {
            h.sp += base as u32;
            self.handlers.push(h);
        }
        frame.entry = true;
        frame.resume_kind = if kind == 2 { 2 } else { 0 };
        self.cur_realm = frame.realm;
        self.frames.push(frame);
        if kind == 1 {
            // Throw at the suspension point.
            return match self.unwind_from_resume(value)? {
                Some(c) => Ok(c),
                None => self.run(),
            };
        }
        if started {
            self.stack.push(value);
        }
        self.run()
    }

    fn unwind_from_resume(&mut self, err: Value) -> JsResult<Option<Completion>> {
        // Reuse the interpreter's unwinding by running a throw.
        let fi = self.frames.len() - 1;
        let hb = self.frames[fi].handler_base;
        if self.handlers.len() > hb {
            let h = self.handlers.pop().unwrap();
            self.stack.truncate(h.sp as usize);
            let f = &mut self.frames[fi];
            f.env = h.env;
            f.pc = h.pc as usize;
            self.stack.push(err);
            return Ok(None);
        }
        let f = self.frames.pop().unwrap();
        self.handlers.truncate(f.handler_base);
        self.stack.truncate(f.args_base - 2);
        if let Some(caller) = self.frames.last() {
            self.cur_realm = caller.realm;
        }
        if let Some(co) = f.coroutine {
            let kind = match &self.heap.get(co).kind {
                Kind::Coroutine(c) => c.kind,
                _ => CoroKind::Generator,
            };
            if kind == CoroKind::Async {
                let p = self.coroutine_finish(co, Err(err));
                return Ok(Some(Completion::Return(p)));
            }
            if let Kind::Coroutine(c) = &mut self.heap.get_mut(co).kind {
                c.state = CoroState::Completed;
            }
        }
        Err(err)
    }

    /// A coroutine frame completed normally (Ok) or with an escaping exception (Err, async only).
    /// Returns the value delivered to the frame's caller.
    pub fn coroutine_finish(&mut self, co: Obj, r: Result<Value, Value>) -> Value {
        let (kind, promise) = match &mut self.heap.get_mut(co).kind {
            Kind::Coroutine(c) => {
                c.state = CoroState::Completed;
                c.frame = None;
                (c.kind, c.promise)
            }
            _ => return r.unwrap_or(Value::Undefined),
        };
        match kind {
            CoroKind::Async => {
                let p = promise.unwrap();
                match r {
                    Ok(v) => {
                        let _ = crate::builtins::promise::resolve_promise(self, p, v);
                    }
                    Err(e) => crate::builtins::promise::reject_promise(self, p, e),
                }
                Value::Object(p)
            }
            _ => r.unwrap_or(Value::Undefined),
        }
    }

    pub fn iter_result(&mut self, v: Value, done: bool) -> Obj {
        let o = self.new_plain_object();
        let d = self.heap.get_mut(o);
        d.props.insert(PropertyKey::from_str("value"), Prop::data(v, WEC));
        d.props.insert(PropertyKey::from_str("done"), Prop::data(Value::Bool(done), WEC));
        o
    }
}

fn await_fulfilled(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let co = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let v = vm.arg(ctx, 0);
    resume_after_await(vm, co, 0, v)
}

fn await_rejected(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let co = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let v = vm.arg(ctx, 0);
    resume_after_await(vm, co, 1, v)
}

fn resume_after_await(vm: &mut Vm, co: Obj, kind: u8, v: Value) -> JsResult<Value> {
    let ck = match &vm.heap.get(co).kind {
        Kind::Coroutine(c) => c.kind,
        _ => return Ok(Value::Undefined),
    };
    let saved_in_native = vm.in_native;
    vm.in_native = false;
    let r = if ck == CoroKind::AsyncGenerator {
        crate::builtins::generator::async_gen_resume_from_await(vm, co, kind, v)
    } else {
        match vm.resume_coroutine(co, kind, v) {
            Ok(_) => Ok(Value::Undefined),
            Err(e) => {
                if vm.terminated {
                    Err(e)
                } else {
                    // Module bodies (TLA) report through their own capability; a stray error here is dropped.
                    Ok(Value::Undefined)
                }
            }
        }
    };
    vm.in_native = saved_in_native;
    r
}
