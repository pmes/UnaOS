//! The virtual machine: agent state (heap, value stack, frames, job queue), realms, function creation and the
//! [[Call]] / [[Construct]] machinery. The interpreter loop is in `interp`, abstract operations in `ops`,
//! object internal methods in `object`.

pub mod collections;
pub mod heap;
pub mod interp;
pub mod module;
pub mod names;
pub mod object;
pub mod coroutine;
pub mod class;
pub mod iter;
pub mod ops;
pub mod propmap;
pub mod realm;
pub mod value;

use crate::bytecode::Code;
use crate::string::JsStr;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
pub use heap::*;
pub use propmap::{Prop, PropMap, Slot, C, E, W, WC, WEC};
pub use realm::{Intrinsics, Realm};
pub use value::{BigInt, Obj, PropertyKey, Sym, Value};
pub use names::NameRef;
pub use heap::PromiseState;
pub use object::PropDesc;

/// Host hooks (§9.5 host-defined operations): the embedding (Aether, the test262 runner) implements these.
pub trait Host {
    /// console.log / warn / error: level 0 log, 1 warn, 2 error, 3 debug, 4 info.
    fn console(&mut self, _level: u8, _msg: &str) {}
    /// HostLoadImportedModule: return the source text of `specifier` resolved against `referrer`, with the
    /// resolved name used as the module's identity.
    fn load_module(&mut self, _referrer: Option<&str>, _specifier: &str) -> Result<(String, String), String> {
        Err(String::from("module loading is not supported by this host"))
    }
    /// Milliseconds since the epoch (Date.now, new Date()).
    fn now_ms(&mut self) -> f64 {
        0.0
    }
    /// Local time zone offset in milliseconds for a UTC time (LocalTZA). Default: UTC.
    fn tz_offset_ms(&mut self, _utc_ms: f64, _is_utc: bool) -> f64 {
        0.0
    }
    /// HostPromiseRejectionTracker: operation 0 reject, 1 handle.
    fn promise_rejection(&mut self, _promise: Obj, _operation: u8) {}
    /// Timers (setTimeout / setInterval): the host owns the event loop. Returns a timer id.
    fn set_timer(&mut self, _callback: Value, _delay_ms: f64, _repeat: bool) -> Option<u32> {
        None
    }
    fn clear_timer(&mut self, _id: u32) {}
    /// Called by `$262.agent` / host-specific globals installed by the embedding.
    fn random_seed(&mut self) -> u64 {
        0x9E37_79B9_7F4A_7C15
    }
}

pub struct NullHost;
impl Host for NullHost {}

pub enum Job {
    /// PromiseReactionJob
    Reaction { handler: Value, argument: Value, capability: Option<(Obj, Value, Value)>, fulfill: bool, realm: u32 },
    /// PromiseResolveThenableJob
    Thenable { promise: Obj, thenable: Value, then: Value, realm: u32 },
    /// A generic callback job (FinalizationRegistry cleanup, host tasks).
    Call { func: Value, this: Value, args: Vec<Value> },
}

pub struct WellKnown {
    pub async_iterator: Sym,
    pub has_instance: Sym,
    pub is_concat_spreadable: Sym,
    pub iterator: Sym,
    pub match_: Sym,
    pub match_all: Sym,
    pub replace: Sym,
    pub search: Sym,
    pub species: Sym,
    pub split: Sym,
    pub to_primitive: Sym,
    pub to_string_tag: Sym,
    pub unscopables: Sym,
}

pub struct Vm {
    pub heap: Heap,
    pub stack: Vec<Value>,
    pub frames: Vec<Frame>,
    pub handlers: Vec<Handler>,
    pub realms: Vec<Realm>,
    pub cur_realm: u32,
    /// Values held by native code across calls back into JavaScript (GC roots).
    pub temp_roots: Vec<Value>,
    pub in_native: bool,
    pub jobs: VecDeque<Job>,
    pub symbol_registry: Vec<(JsStr, Sym)>,
    pub wk: WellKnown,
    pub host: Box<dyn Host>,
    pub max_depth: usize,
    /// Local time zone rule (POSIX TZ data); None defers LocalTZA to the host.
    pub tz: Option<crate::tz::PosixTz>,
    /// Instruction budget (fuzzing / watchdog); None = unlimited.
    pub budget: Option<u64>,
    pub terminated: bool,
    pub kept_alive: Vec<Obj>,
    pub rng: u64,
    pub modules: Vec<(JsStr, Obj)>,
    /// Extra host roots (embedding-held objects).
    pub host_roots: Vec<Value>,
    pub native_depth: usize,
    /// The error stack trace of the last thrown error (source positions), for diagnostics.
    pub last_positions: Vec<u32>,
    /// Limit on heap cells (bounded-memory fuzzing): exceeding it throws a RangeError.
    pub max_cells: usize,
    pub async_counter: u64,
}

pub(crate) fn wk_sym(desc: &str) -> Sym {
    Sym::new(Some(JsStr::from_str(desc)))
}

impl Vm {
    pub fn new(host: Box<dyn Host>) -> Vm {
        let wk = WellKnown {
            async_iterator: wk_sym("Symbol.asyncIterator"),
            has_instance: wk_sym("Symbol.hasInstance"),
            is_concat_spreadable: wk_sym("Symbol.isConcatSpreadable"),
            iterator: wk_sym("Symbol.iterator"),
            match_: wk_sym("Symbol.match"),
            match_all: wk_sym("Symbol.matchAll"),
            replace: wk_sym("Symbol.replace"),
            search: wk_sym("Symbol.search"),
            species: wk_sym("Symbol.species"),
            split: wk_sym("Symbol.split"),
            to_primitive: wk_sym("Symbol.toPrimitive"),
            to_string_tag: wk_sym("Symbol.toStringTag"),
            unscopables: wk_sym("Symbol.unscopables"),
        };
        let mut host = host;
        let seed = host.random_seed();
        let mut vm = Vm {
            heap: Heap::new(),
            stack: Vec::with_capacity(4096),
            frames: Vec::new(),
            handlers: Vec::new(),
            realms: Vec::new(),
            cur_realm: 0,
            temp_roots: Vec::new(),
            in_native: false,
            jobs: VecDeque::new(),
            symbol_registry: Vec::new(),
            wk,
            host,
            max_depth: 1800,
            tz: None,
            budget: None,
            terminated: false,
            kept_alive: Vec::new(),
            rng: seed | 1,
            modules: Vec::new(),
            host_roots: Vec::new(),
            native_depth: 0,
            last_positions: Vec::new(),
            max_cells: usize::MAX,
            async_counter: 0,
        };
        let r = vm.create_realm();
        vm.cur_realm = r;
        vm
    }

    /// CreateRealm + SetDefaultGlobalBindings. Returns the realm index.
    pub fn create_realm(&mut self) -> u32 {
        let idx = self.realms.len() as u32;
        let saved = self.cur_realm;
        self.realms.push(Realm::placeholder());
        self.cur_realm = idx;
        crate::builtins::init_realm(self, idx);
        self.cur_realm = saved;
        idx
    }

    #[inline]
    pub fn realm(&self) -> &Realm {
        &self.realms[self.cur_realm as usize]
    }
    #[inline]
    pub fn intr(&self) -> &Intrinsics {
        &self.realms[self.cur_realm as usize].intrinsics
    }

    // ------------------------------------------------------------------------------------- allocation

    pub fn alloc(&mut self, d: ObjectData) -> Obj {
        let o = self.heap.alloc(d);
        if self.in_native {
            self.temp_roots.push(Value::Object(o));
        }
        o
    }

    pub fn new_object(&mut self, proto: Option<Obj>) -> Obj {
        self.alloc(ObjectData::new(proto, Kind::Ordinary))
    }

    pub fn new_plain_object(&mut self) -> Obj {
        let p = self.intr().object_proto;
        self.new_object(Some(p))
    }

    pub fn new_array(&mut self, elems: Vec<Value>) -> Obj {
        let p = self.intr().array_proto;
        let mut d = ObjectData::new(Some(p), Kind::Array(ArrayData { elems, dense: true, len: 0, len_writable: true }));
        d.extensible = true;
        self.alloc(d)
    }

    /// Run a garbage collection now (only at safe points: the interpreter calls this between instructions).
    pub fn collect_garbage(&mut self) {
        let mut roots: Vec<Obj> = Vec::new();
        let push = |v: &Value, roots: &mut Vec<Obj>| {
            if let Value::Object(o) = v {
                roots.push(*o);
            }
        };
        for v in &self.stack {
            push(v, &mut roots);
        }
        for v in &self.temp_roots {
            push(v, &mut roots);
        }
        for v in &self.host_roots {
            push(v, &mut roots);
        }
        for f in &self.frames {
            trace_frame(f, &mut roots);
        }
        for h in &self.handlers {
            if let Some(e) = h.env {
                roots.push(e);
            }
        }
        for r in &self.realms {
            r.trace(&mut roots);
        }
        for j in &self.jobs {
            match j {
                Job::Reaction { handler, argument, capability, .. } => {
                    push(handler, &mut roots);
                    push(argument, &mut roots);
                    if let Some((o, a, b)) = capability {
                        roots.push(*o);
                        push(a, &mut roots);
                        push(b, &mut roots);
                    }
                }
                Job::Thenable { promise, thenable, then, .. } => {
                    roots.push(*promise);
                    push(thenable, &mut roots);
                    push(then, &mut roots);
                }
                Job::Call { func, this, args } => {
                    push(func, &mut roots);
                    push(this, &mut roots);
                    args.iter().for_each(|a| push(a, &mut roots));
                }
            }
        }
        roots.extend_from_slice(&self.kept_alive);
        for (_, m) in &self.modules {
            roots.push(*m);
        }
        let cleanup = self.heap.collect(&mut roots, &|s: &Sym| !s.0.registered.get() || true);
        for (reg, held) in cleanup {
            let cb = match &self.heap.get(reg).kind {
                Kind::FinReg(f) => f.cleanup.clone(),
                _ => continue,
            };
            self.jobs.push_back(Job::Call { func: cb, this: Value::Undefined, args: vec![held] });
        }
    }

    #[inline]
    pub fn maybe_gc(&mut self) {
        if self.heap.should_collect() {
            self.collect_garbage();
        }
    }

    // ------------------------------------------------------------------------------------- errors

    pub fn make_error(&mut self, proto: Obj, msg: &str) -> Value {
        let trace = self.stack_trace();
        let o = self.alloc(ObjectData::new(Some(proto), Kind::Error(trace)));
        if !msg.is_empty() {
            let m = Value::String(JsStr::from_str(msg));
            self.heap.get_mut(o).props.insert(PropertyKey::from_str("message"), Prop::data(m, WC));
        }
        Value::Object(o)
    }

    /// The active call stack (innermost first, at most 16 frames) as "    at name" lines.
    pub fn stack_trace(&self) -> JsStr {
        let mut out = String::new();
        for f in self.frames.iter().rev().take(16) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("    at ");
            if f.code.name.is_empty() {
                out.push_str("<anonymous>");
            } else {
                out.push_str(&f.code.name.to_rust());
            }
        }
        JsStr::from_str(&out)
    }

    pub fn type_error(&mut self, msg: &str) -> Value {
        let p = self.intr().type_error_proto;
        self.make_error(p, msg)
    }
    pub fn range_error(&mut self, msg: &str) -> Value {
        let p = self.intr().range_error_proto;
        self.make_error(p, msg)
    }
    pub fn reference_error(&mut self, msg: &str) -> Value {
        let p = self.intr().reference_error_proto;
        self.make_error(p, msg)
    }
    pub fn syntax_error(&mut self, msg: &str) -> Value {
        let p = self.intr().syntax_error_proto;
        self.make_error(p, msg)
    }
    pub fn throw_type<T>(&mut self, msg: &str) -> JsResult<T> {
        Err(self.type_error(msg))
    }
    pub fn throw_range<T>(&mut self, msg: &str) -> JsResult<T> {
        Err(self.range_error(msg))
    }
    pub fn throw_ref<T>(&mut self, msg: &str) -> JsResult<T> {
        Err(self.reference_error(msg))
    }
    pub fn throw_syntax<T>(&mut self, msg: &str) -> JsResult<T> {
        Err(self.syntax_error(msg))
    }

    // ------------------------------------------------------------------------------------- functions

    /// OrdinaryFunctionCreate + MakeConstructor as appropriate for the code's kind.
    pub fn make_closure(&mut self, code: Rc<Code>, env: Option<Obj>, home: Option<Obj>, script: Option<Obj>) -> Obj {
        use crate::ast::FnKind;
        let intr = self.intr();
        let proto = match (code.is_async, code.is_generator) {
            (false, false) => intr.function_proto,
            (true, false) => intr.async_function_proto,
            (false, true) => intr.generator_function_proto,
            (true, true) => intr.async_generator_function_proto,
        };
        let gen_proto = intr.generator_proto;
        let agen_proto = intr.async_generator_proto;
        let realm = self.cur_realm;
        let name = code.name.clone();
        let length = code.length;
        let is_ctor = code.is_constructor() && code.kind == FnKind::Normal;
        let is_gen = code.is_generator;
        let is_async = code.is_async;
        let mut d = ObjectData::new(Some(proto), Kind::Function(Box::new(FuncData { code, env, home, realm, class: None, script })));
        d.props.insert(PropertyKey::from_str("length"), Prop::data(Value::Number(length as f64), C));
        d.props.insert(PropertyKey::from_str("name"), Prop::data(Value::String(name), C));
        let f = self.alloc(d);
        if is_gen {
            let p = self.new_object(Some(if is_async { agen_proto } else { gen_proto }));
            self.heap.get_mut(f).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(p), W));
        } else if is_ctor {
            let p = self.new_plain_object();
            self.heap.get_mut(p).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(f), WC));
            self.heap.get_mut(f).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(p), W));
        }
        f
    }

    /// CreateBuiltinFunction.
    pub fn make_native(&mut self, name: &str, length: u32, f: NativeFn, ctor: bool) -> Obj {
        let proto = self.intr().function_proto;
        self.make_native_with(name, length, f, ctor, Some(proto), Vec::new())
    }

    pub fn make_native_with(&mut self, name: &str, length: u32, f: NativeFn, ctor: bool, proto: Option<Obj>, slots: Vec<Value>) -> Obj {
        let realm = self.cur_realm;
        let mut d = ObjectData::new(proto, Kind::Native(Box::new(NativeData { f, ctor, slots, realm })));
        d.props.insert(PropertyKey::from_str("length"), Prop::data(Value::Number(length as f64), C));
        d.props.insert(PropertyKey::from_str("name"), Prop::data(Value::str(name), C));
        self.alloc(d)
    }

    /// A native closure with captured slots (promise resolving functions, bound helpers…).
    pub fn make_native_closure(&mut self, name: &str, length: u32, f: NativeFn, slots: Vec<Value>) -> Obj {
        let proto = self.intr().function_proto;
        self.make_native_with(name, length, f, false, Some(proto), slots)
    }

    pub fn native_slot(&self, callee: Obj, i: usize) -> Value {
        match &self.heap.get(callee).kind {
            Kind::Native(n) => n.slots.get(i).cloned().unwrap_or(Value::Undefined),
            _ => Value::Undefined,
        }
    }
    pub fn set_native_slot(&mut self, callee: Obj, i: usize, v: Value) {
        if let Kind::Native(n) = &mut self.heap.get_mut(callee).kind {
            if i < n.slots.len() {
                n.slots[i] = v;
            }
        }
    }

    pub fn is_callable(&self, v: &Value) -> bool {
        match v {
            Value::Object(o) => self.obj_is_callable(*o),
            _ => false,
        }
    }
    pub fn obj_is_callable(&self, o: Obj) -> bool {
        match &self.heap.get(o).kind {
            Kind::Function(_) | Kind::Native(_) | Kind::Bound(_) => true,
            Kind::Proxy(Some(p)) => p.callable,
            Kind::Proxy(None) => false,
            _ => false,
        }
    }
    pub fn is_constructor(&self, v: &Value) -> bool {
        match v {
            Value::Object(o) => self.obj_is_constructor(*o),
            _ => false,
        }
    }
    pub fn obj_is_constructor(&self, o: Obj) -> bool {
        let d = self.heap.get(o);
        match &d.kind {
            Kind::Function(f) => f.code.is_constructor(),
            Kind::Native(n) => n.ctor,
            Kind::Bound(b) => self.obj_is_constructor(b.target),
            Kind::Proxy(Some(p)) => p.ctor,
            _ => false,
        }
    }

    /// GetFunctionRealm (§7.3.24).
    pub fn function_realm(&mut self, o: Obj) -> JsResult<u32> {
        match &self.heap.get(o).kind {
            Kind::Function(f) => Ok(f.realm),
            Kind::Native(n) => Ok(n.realm),
            Kind::Bound(b) => {
                let t = b.target;
                self.function_realm(t)
            }
            Kind::Proxy(Some(p)) if !p.revoked => {
                let t = p.target;
                self.function_realm(t)
            }
            Kind::Proxy(_) => self.throw_type("proxy has been revoked"),
            _ => Ok(self.cur_realm),
        }
    }

    // ------------------------------------------------------------------------------------- call / construct

    /// Call(F, V, args).
    pub fn call(&mut self, f: &Value, this: &Value, args: &[Value]) -> JsResult<Value> {
        let fo = match f {
            Value::Object(o) if self.obj_is_callable(*o) => *o,
            _ => return self.throw_type("not a function"),
        };
        let base = self.stack.len();
        self.stack.push(Value::Object(fo));
        self.stack.push(this.clone());
        self.stack.extend_from_slice(args);
        let r = self.call_at(base, args.len(), Value::Undefined, false);
        self.stack.truncate(base);
        r
    }

    /// Construct(F, args, newTarget).
    pub fn construct(&mut self, f: &Value, args: &[Value], new_target: Option<&Value>) -> JsResult<Value> {
        let fo = match f {
            Value::Object(o) if self.obj_is_constructor(*o) => *o,
            _ => return self.throw_type("not a constructor"),
        };
        let nt = new_target.cloned().unwrap_or(Value::Object(fo));
        let base = self.stack.len();
        self.stack.push(Value::Object(fo));
        self.stack.push(Value::Undefined);
        self.stack.extend_from_slice(args);
        let r = self.call_at(base, args.len(), nt, true);
        self.stack.truncate(base);
        r
    }

    /// Invoke the function at stack[base] with this at base+1 and argc args after. Runs to completion
    /// (a nested interpreter loop for bytecode functions). The caller truncates the stack.
    pub fn call_at(&mut self, base: usize, argc: usize, new_target: Value, construct: bool) -> JsResult<Value> {
        if self.frames.len() + self.native_depth >= self.max_depth {
            return self.throw_range("Maximum call stack size exceeded");
        }
        let fo = match &self.stack[base] {
            Value::Object(o) => *o,
            _ => return self.throw_type("not a function"),
        };
        enum K {
            Code(Rc<Code>),
            Native(NativeFn),
            Bound(Obj, Value, Vec<Value>),
            Proxy,
            ClassCtor,
            Bad,
        }
        let k = match &self.heap.get(fo).kind {
            Kind::Function(f) => {
                if self.heap.get(fo).class_ctor && !construct {
                    K::ClassCtor
                } else {
                    K::Code(f.code.clone())
                }
            }
            Kind::Native(n) => K::Native(n.f),
            Kind::Bound(b) => K::Bound(b.target, b.this.clone(), b.args.clone()),
            Kind::Proxy(_) => K::Proxy,
            _ => K::Bad,
        };
        match k {
            K::ClassCtor => {
                // The TypeError is created in the constructor's realm (§10.2.1 step 2).
                let saved = self.cur_realm;
                if let Ok(r) = self.function_realm(fo) {
                    self.cur_realm = r;
                }
                let e = self.type_error("Class constructor cannot be invoked without 'new'");
                self.cur_realm = saved;
                Err(e)
            }
            K::Bad => self.throw_type("not a function"),
            K::Native(f) => {
                let realm = match &self.heap.get(fo).kind {
                    Kind::Native(n) => n.realm,
                    _ => self.cur_realm,
                };
                let ctx = CallCtx { this: self.stack[base + 1].clone(), args_base: base + 2, argc, new_target, callee: fo };
                let saved_realm = self.cur_realm;
                let saved_native = self.in_native;
                let mark = self.temp_roots.len();
                self.cur_realm = realm;
                self.in_native = true;
                self.native_depth += 1;
                let r = f(self, &ctx);
                self.native_depth -= 1;
                self.in_native = saved_native;
                self.temp_roots.truncate(mark);
                self.cur_realm = saved_realm;
                r
            }
            K::Bound(target, this, bargs) => {
                // Rebuild the call with the bound arguments prepended.
                let args: Vec<Value> = self.stack[base + 2..base + 2 + argc].to_vec();
                let mut all = bargs;
                all.extend(args);
                if construct {
                    let nt = if let Value::Object(n) = &new_target { if *n == fo { Value::Object(target) } else { new_target.clone() } } else { new_target.clone() };
                    self.construct(&Value::Object(target), &all, Some(&nt))
                } else {
                    self.call(&Value::Object(target), &this, &all)
                }
            }
            K::Proxy => {
                let args: Vec<Value> = self.stack[base + 2..base + 2 + argc].to_vec();
                let this = self.stack[base + 1].clone();
                if construct {
                    crate::builtins::proxy::proxy_construct(self, fo, &args, &new_target)
                } else {
                    crate::builtins::proxy::proxy_call(self, fo, &this, &args)
                }
            }
            K::Code(code) => {
                if code.is_async && !code.is_generator && !construct {
                    return self.call_async_at(fo, code, base, argc);
                }
                self.push_code_frame(fo, code, base, argc, new_target, construct)?;
                let depth = self.frames.len();
                self.frames[depth - 1].entry = true;
                match self.run()? {
                    interp::Completion::Return(v) => Ok(v),
                    interp::Completion::Suspend(v) => Ok(v),
                }
            }
        }
    }

    /// Set up a frame for a bytecode function call (callee at stack[base], this at base+1, args after).
    pub fn push_code_frame(&mut self, fo: Obj, code: Rc<Code>, base: usize, argc: usize, new_target: Value, construct: bool) -> JsResult<()> {
        let (env, realm, script, home_is_derived) = match &self.heap.get(fo).kind {
            Kind::Function(f) => (f.env, f.realm, f.script, code.derived),
            _ => unreachable!(),
        };
        let mut this = self.stack[base + 1].clone();
        let mut is_base_construct = false;
        if construct {
            if home_is_derived {
                this = Value::Empty;
            } else {
                // OrdinaryCreateFromConstructor(newTarget, "%Object.prototype%")
                let saved = self.cur_realm;
                self.cur_realm = realm;
                let proto = self.get_prototype_from_ctor(&new_target, |i| i.object_proto);
                self.cur_realm = saved;
                let proto = proto?;
                let o = self.new_object(Some(proto));
                this = Value::Object(o);
                is_base_construct = true;
            }
        } else if code.kind == crate::ast::FnKind::Arrow {
            this = Value::Undefined;
        } else if !code.strict {
            // OrdinaryCallBindThis: sloppy functions box primitives and use the global this for nullish.
            this = match this {
                Value::Undefined | Value::Null => self.realms[realm as usize].global_this.clone(),
                Value::Object(_) => this,
                other => {
                    let saved = self.cur_realm;
                    self.cur_realm = realm;
                    let r = self.to_object(&other);
                    self.cur_realm = saved;
                    r?
                }
            };
        }
        let args_base = base + 2;
        let locals = args_base + argc;
        let nlocals = code.nlocals as usize;
        self.stack.resize(locals + nlocals, Value::Undefined);
        let hb = self.handlers.len();
        self.frames.push(Frame {
            code,
            pc: 0,
            args_base,
            argc,
            base: locals,
            func: Some(fo),
            this,
            new_target: if construct { new_target } else { Value::Undefined },
            env,
            handler_base: hb,
            realm,
            construct: is_base_construct,
            entry: false,
            coroutine: None,
            resume_kind: 0,
            script,
        });
        self.cur_realm = realm;
        Ok(())
    }

    /// GetPrototypeFromConstructor (§10.1.14).
    pub fn get_prototype_from_ctor(&mut self, ctor: &Value, default: impl Fn(&Intrinsics) -> Obj) -> JsResult<Obj> {
        if let Value::Object(c) = ctor {
            let p = self.get(*c, &PropertyKey::from_str("prototype"))?;
            if let Value::Object(p) = p {
                return Ok(p);
            }
            let realm = self.function_realm(*c)?;
            return Ok(default(&self.realms[realm as usize].intrinsics));
        }
        Ok(default(self.intr()))
    }

    /// Native helper: argument i of a native call.
    #[inline]
    pub fn arg(&self, ctx: &CallCtx, i: usize) -> Value {
        if i < ctx.argc {
            self.stack[ctx.args_base + i].clone()
        } else {
            Value::Undefined
        }
    }
    pub fn args(&self, ctx: &CallCtx) -> Vec<Value> {
        self.stack[ctx.args_base..ctx.args_base + ctx.argc].to_vec()
    }

    pub fn root(&mut self, v: &Value) {
        if let Value::Object(_) = v {
            self.temp_roots.push(v.clone());
        }
    }

    // ------------------------------------------------------------------------------------- jobs

    /// Perform a microtask checkpoint: run queued jobs until the queue is empty.
    pub fn run_jobs(&mut self) -> JsResult<()> {
        while let Some(job) = self.jobs.pop_front() {
            self.run_job(job)?;
            if self.terminated {
                break;
            }
        }
        self.kept_alive.clear();
        Ok(())
    }

    pub fn run_job(&mut self, job: Job) -> JsResult<()> {
        match job {
            Job::Reaction { handler, argument, capability, fulfill, realm } => {
                let saved = self.cur_realm;
                self.cur_realm = realm;
                let r = crate::builtins::promise::reaction_job(self, handler, argument, capability, fulfill);
                self.cur_realm = saved;
                r
            }
            Job::Thenable { promise, thenable, then, realm } => {
                let saved = self.cur_realm;
                self.cur_realm = realm;
                let r = crate::builtins::promise::resolve_thenable_job(self, promise, thenable, then);
                self.cur_realm = saved;
                r
            }
            Job::Call { func, this, args } => {
                let r = self.call(&func, &this, &args);
                match r {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        if self.terminated {
                            return Err(e);
                        }
                        // Errors in host callbacks are reported, not propagated.
                        Ok(())
                    }
                }
            }
        }
    }

    pub fn random(&mut self) -> f64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        let r = x.wrapping_mul(0x2545F4914F6CDD1D);
        (r >> 11) as f64 / (1u64 << 53) as f64
    }
}

impl Vm {
    /// ScriptEvaluation (§16.1.6) of source text in the current realm: parse, compile, run. Parse errors are
    /// thrown as SyntaxError objects.
    pub fn run_script(&mut self, src: &[u16]) -> JsResult<Value> {
        let prog = match crate::parser::parse_script(src) {
            Ok(p) => p,
            Err(e) => return self.throw_syntax(&e.msg),
        };
        let code = crate::compiler::compile_script(&prog);
        let r = self.cur_realm;
        let genv = self.realms[r as usize].global_env;
        let gt = self.realms[r as usize].global_this.clone();
        let base = self.stack.len();
        self.stack.push(Value::Undefined);
        self.stack.push(gt.clone());
        let nl = code.nlocals as usize;
        self.stack.resize(base + 2 + nl, Value::Undefined);
        let hb = self.handlers.len();
        self.frames.push(Frame {
            code,
            pc: 0,
            args_base: base + 2,
            argc: 0,
            base: base + 2,
            func: None,
            this: gt,
            new_target: Value::Undefined,
            env: Some(genv),
            handler_base: hb,
            realm: r,
            construct: false,
            entry: true,
            coroutine: None,
            resume_kind: 0,
            script: None,
        });
        let res = self.run();
        self.stack.truncate(base);
        self.cur_realm = r;
        match res? {
            interp::Completion::Return(v) | interp::Completion::Suspend(v) => Ok(v),
        }
    }

    pub fn run_script_str(&mut self, src: &str) -> JsResult<Value> {
        let u: Vec<u16> = src.encode_utf16().collect();
        self.run_script(&u)
    }

    /// A human-readable rendering of a thrown value ("TypeError: message").
    pub fn error_string(&mut self, v: &Value) -> String {
        if let Value::Object(o) = v {
            let name = self.get(*o, &PropertyKey::from_str("name")).ok();
            let msg = self.get(*o, &PropertyKey::from_str("message")).ok();
            if let (Some(Value::String(n)), Some(m)) = (name, msg) {
                let m = match m {
                    Value::String(s) => s.to_rust(),
                    _ => String::new(),
                };
                return alloc::format!("{}: {}", n, m);
            }
        }
        match self.to_string(v) {
            Ok(s) => s.to_rust(),
            Err(_) => String::from("<unprintable value>"),
        }
    }
}
