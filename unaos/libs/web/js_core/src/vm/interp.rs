//! The interpreter loop. Bytecode-to-bytecode calls push frames in this loop; native code that calls back into
//! JavaScript runs a nested loop (`run`) that returns when its entry frame completes or suspends.

use super::*;
use crate::bytecode::{BindKind, Const, Op};

pub enum Completion {
    Return(Value),
    /// The entry frame (a coroutine) suspended at a yield / await.
    Suspend(Value),
}

enum Flow {
    Next,
    /// The entry frame finished.
    Done(Completion),
}

impl Vm {
    #[inline]
    fn pop(&mut self) -> Value {
        self.stack.pop().unwrap_or(Value::Undefined)
    }
    #[inline]
    fn top(&self) -> &Value {
        self.stack.last().unwrap()
    }
    #[inline]
    fn fr(&self) -> &Frame {
        self.frames.last().unwrap()
    }
    #[inline]
    fn frm(&mut self) -> &mut Frame {
        self.frames.last_mut().unwrap()
    }

    /// Run until the entry frame (the innermost frame marked `entry` at the time of the call) completes.
    pub fn run(&mut self) -> JsResult<Completion> {
        let saved_native = self.in_native;
        self.in_native = false;
        let r = self.run_loop();
        self.in_native = saved_native;
        r
    }

    fn run_loop(&mut self) -> JsResult<Completion> {
        loop {
            match self.run_inner() {
                Ok(c) => return Ok(c),
                Err(e) => {
                    // Unwind to a handler, or out of the entry frame.
                    if let Some(c) = self.unwind(e)? {
                        return Ok(c);
                    }
                }
            }
        }
    }

    /// Find a handler for `err`. Ok(None): resumed at a handler. Ok(Some(c)): the entry frame was an async
    /// function that absorbed the error into its promise. Err: propagate out of this run.
    fn unwind(&mut self, err: Value) -> JsResult<Option<Completion>> {
        loop {
            let fi = self.frames.len() - 1;
            let hb = self.frames[fi].handler_base;
            if !self.terminated && self.handlers.len() > hb {
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
            // Async functions turn an escaping exception into a rejected promise.
            if let Some(co) = f.coroutine {
                let kind = match &self.heap.get(co).kind {
                    Kind::Coroutine(c) => c.kind,
                    _ => CoroKind::Generator,
                };
                if kind == CoroKind::Async && !self.terminated {
                    let p = self.coroutine_finish(co, Err(err.clone()));
                    if f.entry {
                        return Ok(Some(Completion::Return(p)));
                    }
                    self.stack.push(p);
                    return Ok(None);
                }
                if let Kind::Coroutine(c) = &mut self.heap.get_mut(co).kind {
                    c.state = CoroState::Completed;
                    c.frame = None;
                }
            }
            if f.entry {
                return Err(err);
            }
        }
    }

    fn run_inner(&mut self) -> JsResult<Completion> {
        loop {
            let fi = self.frames.len() - 1;
            let pc = self.frames[fi].pc;
            let op = self.frames[fi].code.ops[pc];
            self.frames[fi].pc = pc + 1;
            if let Some(b) = &mut self.budget {
                if *b == 0 {
                    self.terminated = true;
                    return Err(Value::str("execution budget exhausted"));
                }
                *b -= 1;
            }
            match op {
                Op::Undef => self.stack.push(Value::Undefined),
                Op::Null => self.stack.push(Value::Null),
                Op::True => self.stack.push(Value::Bool(true)),
                Op::False => self.stack.push(Value::Bool(false)),
                Op::Int(i) => self.stack.push(Value::Number(i as f64)),
                Op::Const(k) => {
                    let v = match &self.frames[fi].code.consts[k as usize] {
                        Const::Num(n) => Value::Number(*n),
                        Const::Str(s) => Value::String(s.clone()),
                        Const::BigInt(b) => Value::BigInt(b.clone()),
                        _ => Value::Undefined,
                    };
                    self.stack.push(v);
                }
                Op::PushEmpty => self.stack.push(Value::Empty),
                Op::Pop => {
                    self.stack.pop();
                }
                Op::Dup => {
                    let v = self.top().clone();
                    self.stack.push(v);
                }
                Op::Dup2 => {
                    let n = self.stack.len();
                    let a = self.stack[n - 2].clone();
                    let b = self.stack[n - 1].clone();
                    self.stack.push(a);
                    self.stack.push(b);
                }
                Op::Swap => {
                    let n = self.stack.len();
                    self.stack.swap(n - 1, n - 2);
                }
                Op::Over => {
                    let n = self.stack.len();
                    let a = self.stack[n - 2].clone();
                    self.stack.push(a);
                }
                Op::Rot3 => {
                    // a b c -> c a b
                    let c = self.pop();
                    let n = self.stack.len();
                    self.stack.insert(n - 2, c);
                }
                Op::Rot4 => {
                    let d = self.pop();
                    let n = self.stack.len();
                    self.stack.insert(n - 3, d);
                }
                Op::Rot3Up => {
                    // a b c -> b c a
                    let n = self.stack.len();
                    let a = self.stack.remove(n - 3);
                    self.stack.push(a);
                }
                Op::GetLocal(i) => {
                    let b = self.frames[fi].base;
                    let v = self.stack[b + i as usize].clone();
                    self.stack.push(v);
                }
                Op::SetLocal(i) => {
                    let b = self.frames[fi].base;
                    let v = self.top().clone();
                    self.stack[b + i as usize] = v;
                }
                Op::PutLocal(i) => {
                    let b = self.frames[fi].base;
                    let v = self.pop();
                    self.stack[b + i as usize] = v;
                }
                Op::GetLocalChk(i, k) => {
                    let b = self.frames[fi].base;
                    let v = self.stack[b + i as usize].clone();
                    if v.is_empty() {
                        let name = self.frames[fi].code.str(k).to_rust();
                        return Err(self.tdz_error(&name));
                    }
                    self.stack.push(v);
                }
                Op::SetLocalChk(i, k) => {
                    let b = self.frames[fi].base;
                    if self.stack[b + i as usize].is_empty() {
                        let name = self.frames[fi].code.str(k).to_rust();
                        return Err(self.tdz_error(&name));
                    }
                    let v = self.top().clone();
                    self.stack[b + i as usize] = v;
                }
                Op::GetEnv(d, i) => {
                    let e = self.env_at(d);
                    let v = self.env_slot(e, i);
                    self.stack.push(v);
                }
                Op::GetEnvChk(d, i) => {
                    let e = self.env_at(d);
                    let v = self.env_slot(e, i);
                    if v.is_empty() {
                        let name = self.env_name(e, i);
                        return Err(self.tdz_error(&name));
                    }
                    self.stack.push(v);
                }
                Op::SetEnv(d, i) => {
                    let e = self.env_at(d);
                    let v = self.top().clone();
                    self.set_env_slot(e, i, v);
                }
                Op::SetEnvChk(d, i) => {
                    let e = self.env_at(d);
                    if self.env_slot(e, i).is_empty() {
                        let name = self.env_name(e, i);
                        return Err(self.tdz_error(&name));
                    }
                    let v = self.top().clone();
                    self.set_env_slot(e, i, v);
                }
                Op::InitEnv(d, i) => {
                    let e = self.env_at(d);
                    let v = self.pop();
                    self.set_env_slot(e, i, v);
                }
                Op::ThrowConst(k) => {
                    let name = self.frames[fi].code.str(k).to_rust();
                    return Err(self.type_error(&alloc::format!("Assignment to constant variable '{}'", name)));
                }
                Op::GetName(k) => {
                    let name = self.frames[fi].code.str(k).clone();
                    let v = self.get_name(&name)?;
                    self.stack.push(v);
                }
                Op::SetName(k) => {
                    let name = self.frames[fi].code.str(k).clone();
                    let v = self.top().clone();
                    let strict = self.frames[fi].code.strict;
                    self.set_name(&name, v, strict)?;
                }
                Op::ResolveRef(k) => {
                    let name = self.frames[fi].code.str(k).clone();
                    let t = self.resolve_ref(&name)?;
                    self.stack.push(t);
                }
                Op::GetRef(k) => {
                    let name = self.frames[fi].code.str(k).clone();
                    let t = self.top().clone();
                    let v = self.get_ref(&t, &name)?;
                    self.stack.push(v);
                }
                Op::PutRef(k) => {
                    let name = self.frames[fi].code.str(k).clone();
                    let v = self.pop();
                    let t = self.pop();
                    let strict = self.frames[fi].code.strict;
                    self.put_ref(&t, &name, v.clone(), strict)?;
                    self.stack.push(v);
                }
                Op::InitName(k) => {
                    let name = self.frames[fi].code.str(k).clone();
                    let v = self.pop();
                    self.init_name(&name, v)?;
                }
                Op::TypeofName(k) => {
                    let name = self.frames[fi].code.str(k).clone();
                    let v = match self.lookup_name(&name)? {
                        NameRef::None => Value::Undefined,
                        r => self.get_name_ref(&name, r)?,
                    };
                    let t = self.type_of(&v);
                    self.stack.push(Value::String(t));
                }
                Op::DeleteName(k) => {
                    let name = self.frames[fi].code.str(k).clone();
                    let r = self.delete_name(&name)?;
                    self.stack.push(Value::Bool(r));
                }
                Op::GetNameThis(k) => {
                    let name = self.frames[fi].code.str(k).clone();
                    let r = self.lookup_name(&name)?;
                    let this = match &r {
                        NameRef::Object(o, true) => Value::Object(*o),
                        _ => Value::Undefined,
                    };
                    let v = match r {
                        NameRef::None => return Err(self.not_defined(&name)),
                        r => self.get_name_ref(&name, r)?,
                    };
                    self.stack.push(v);
                    self.stack.push(this);
                }
                Op::PushEnv(k) => {
                    let info = match &self.frames[fi].code.consts[k as usize] {
                        Const::Scope(s) => s.clone(),
                        _ => unreachable!(),
                    };
                    let slots = info
                        .kinds
                        .iter()
                        .map(|k| match k {
                            BindKind::Let | BindKind::Const | BindKind::Class => Value::Empty,
                            _ => Value::Undefined,
                        })
                        .collect();
                    let parent = self.frames[fi].env;
                    let e = self.alloc(ObjectData::new(None, Kind::Env(Box::new(EnvData { parent, slots, info, extra: None }))));
                    self.frames[fi].env = Some(e);
                }
                Op::PopEnv => {
                    let e = self.frames[fi].env.unwrap();
                    let p = self.env_parent(e);
                    self.frames[fi].env = p;
                }
                Op::CopyEnv => {
                    let e = self.frames[fi].env.unwrap();
                    let (parent, slots, info) = match &self.heap.get(e).kind {
                        Kind::Env(d) => (d.parent, d.slots.clone(), d.info.clone()),
                        _ => unreachable!(),
                    };
                    let n = self.alloc(ObjectData::new(None, Kind::Env(Box::new(EnvData { parent, slots, info, extra: None }))));
                    self.frames[fi].env = Some(n);
                }
                Op::PushWith => {
                    let v = self.pop();
                    let o = self.to_object(&v)?;
                    let o = o.as_object().unwrap();
                    let parent = self.frames[fi].env;
                    let e = self.alloc(ObjectData::new(None, Kind::ObjEnv(Box::new(ObjEnvData { parent, object: o, with: true }))));
                    self.frames[fi].env = Some(e);
                }
                Op::GetImport(k) => {
                    let v = self.get_import(k)?;
                    self.stack.push(v);
                }
                Op::GetArg(i) => {
                    let f = &self.frames[fi];
                    let v = if (i as usize) < f.argc { self.stack[f.args_base + i as usize].clone() } else { Value::Undefined };
                    self.stack.push(v);
                }
                Op::RestArgs(n) => {
                    let f = &self.frames[fi];
                    let v: Vec<Value> = if (n as usize) < f.argc { self.stack[f.args_base + n as usize..f.args_base + f.argc].to_vec() } else { Vec::new() };
                    let a = self.new_array(v);
                    self.stack.push(Value::Object(a));
                }
                Op::Arguments(k) => {
                    let a = self.create_arguments(k)?;
                    self.stack.push(Value::Object(a));
                }
                Op::This => {
                    let v = self.frames[fi].this.clone();
                    if v.is_empty() {
                        return self.throw_ref("Must call super constructor in derived class before accessing 'this'");
                    }
                    self.stack.push(v);
                }
                Op::ThisChk => {
                    let v = self.frames[fi].this.clone();
                    if v.is_empty() {
                        return self.throw_ref("Must call super constructor in derived class before accessing 'this'");
                    }
                    self.stack.push(v);
                }
                Op::GlobalThis => {
                    let v = self.realms[self.frames[fi].realm as usize].global_this.clone();
                    self.stack.push(v);
                }
                Op::NewTarget => {
                    let v = self.frames[fi].new_target.clone();
                    self.stack.push(v);
                }
                Op::Callee => {
                    let v = match self.frames[fi].func {
                        Some(o) => Value::Object(o),
                        None => Value::Undefined,
                    };
                    self.stack.push(v);
                }
                Op::LoadThisBinding => {}
                Op::InitThisLocal(i) => {
                    let b = self.frames[fi].base;
                    let v = self.pop();
                    if !self.stack[b + i as usize].is_empty() {
                        return self.throw_ref("Super constructor may only be called once");
                    }
                    self.stack[b + i as usize] = v.clone();
                    self.frames[fi].this = v;
                }
                Op::InitThisEnv(d, i) => {
                    let e = self.env_at(d);
                    let v = self.pop();
                    if !self.env_slot(e, i).is_empty() {
                        return self.throw_ref("Super constructor may only be called once");
                    }
                    self.set_env_slot(e, i, v.clone());
                    // The constructor frame's own `this` (if this is that frame).
                    if self.frames[fi].this.is_empty() && self.frames[fi].code.derived {
                        self.frames[fi].this = v;
                    }
                }
                Op::CheckDerivedReturn => {
                    let this = self.pop();
                    let v = self.pop();
                    let r = match v {
                        Value::Object(_) => v,
                        Value::Undefined => {
                            if this.is_empty() {
                                return self.throw_ref("Must call super constructor in derived class before returning");
                            }
                            this
                        }
                        _ => return self.throw_type("Derived constructors may only return object or undefined"),
                    };
                    self.stack.push(r);
                }

                // ---- objects
                Op::NewObject => {
                    let o = self.new_plain_object();
                    self.stack.push(Value::Object(o));
                }
                Op::NewArray(n) => {
                    let a = self.new_array(Vec::with_capacity(n as usize));
                    self.stack.push(Value::Object(a));
                }
                Op::ArrayPush => {
                    let v = self.pop();
                    let a = self.top().as_object().unwrap();
                    if let Kind::Array(ad) = &mut self.heap.get_mut(a).kind {
                        ad.elems.push(v);
                    }
                }
                Op::ArrayHole => {
                    let a = self.top().as_object().unwrap();
                    if let Kind::Array(ad) = &mut self.heap.get_mut(a).kind {
                        ad.elems.push(Value::Empty);
                    }
                }
                Op::ArraySpread => {
                    let it = self.pop();
                    let a = self.top().as_object().unwrap();
                    self.spread_into(a, it)?;
                }
                Op::DefineField => {
                    let v = self.pop();
                    let k = self.pop();
                    let o = self.top().as_object().unwrap();
                    let key = self.to_property_key(&k)?;
                    self.create_data_property_or_throw(o, key, v)?;
                }
                Op::DefineFieldNamed(k) => {
                    let v = self.pop();
                    let key = PropertyKey::from_js(self.frames[fi].code.str(k).clone());
                    let o = self.top().as_object().unwrap();
                    self.create_data_property_or_throw(o, key, v)?;
                }
                Op::DefineMethod(kind) => {
                    let f = self.pop().as_object().unwrap();
                    let k = self.pop();
                    let o = self.top().as_object().unwrap();
                    self.define_method(o, k, f, kind)?;
                }
                Op::SetProtoLit => {
                    let p = self.pop();
                    let o = self.top().as_object().unwrap();
                    match p {
                        Value::Object(po) => {
                            self.heap.get_mut(o).proto = Some(po);
                        }
                        Value::Null => {
                            self.heap.get_mut(o).proto = None;
                        }
                        _ => {}
                    }
                }
                Op::CopyDataProps => {
                    let src = self.pop();
                    let t = self.top().as_object().unwrap();
                    self.copy_data_properties(t, &src, &[])?;
                }
                Op::CopyDataPropsExcl(n) => {
                    let mut ex = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        let k = self.pop();
                        ex.push(self.to_property_key(&k)?);
                    }
                    let src = self.pop();
                    let t = self.top().as_object().unwrap();
                    self.copy_data_properties(t, &src, &ex)?;
                }
                Op::GetProp(k) => {
                    let o = self.pop();
                    let key = PropertyKey::from_js(self.frames[fi].code.str(k).clone());
                    let v = self.get_v(&o, &key)?;
                    self.stack.push(v);
                }
                Op::SetProp(k) => {
                    let v = self.pop();
                    let o = self.pop();
                    let key = PropertyKey::from_js(self.frames[fi].code.str(k).clone());
                    let strict = self.frames[fi].code.strict;
                    self.put_value(&o, key, v.clone(), strict)?;
                    self.stack.push(v);
                }
                Op::GetElem => {
                    let k = self.pop();
                    let o = self.pop();
                    let v = self.get_elem(&o, &k)?;
                    self.stack.push(v);
                }
                Op::SetElem => {
                    let v = self.pop();
                    let k = self.pop();
                    let o = self.pop();
                    if o.is_nullish() {
                        return self.throw_type(&alloc::format!("Cannot set properties of {}", if o.is_null() { "null" } else { "undefined" }));
                    }
                    let key = self.to_property_key(&k)?;
                    let strict = self.frames[fi].code.strict;
                    self.put_value(&o, key, v.clone(), strict)?;
                    self.stack.push(v);
                }
                Op::DeleteProp(k) => {
                    let o = self.pop();
                    let key = PropertyKey::from_js(self.frames[fi].code.str(k).clone());
                    let strict = self.frames[fi].code.strict;
                    let r = self.delete_value(&o, key, strict)?;
                    self.stack.push(Value::Bool(r));
                }
                Op::DeleteElem => {
                    let k = self.pop();
                    let o = self.pop();
                    if o.is_nullish() {
                        return self.throw_type("Cannot convert undefined or null to object");
                    }
                    let key = self.to_property_key(&k)?;
                    let strict = self.frames[fi].code.strict;
                    let r = self.delete_value(&o, key, strict)?;
                    self.stack.push(Value::Bool(r));
                }
                Op::GetSuper => {
                    let k = self.pop();
                    let f = self.pop();
                    let this = self.pop();
                    let v = self.super_get(&this, &f, &k)?;
                    self.stack.push(v);
                }
                Op::SetSuper => {
                    let v = self.pop();
                    let k = self.pop();
                    let f = self.pop();
                    let this = self.pop();
                    self.super_set(&this, &f, &k, v.clone())?;
                    self.stack.push(v);
                }
                Op::GetPrivate => {
                    let pn = self.pop();
                    let o = self.pop();
                    let v = self.private_get(&o, &pn)?;
                    self.stack.push(v);
                }
                Op::SetPrivate => {
                    let v = self.pop();
                    let pn = self.pop();
                    let o = self.pop();
                    self.private_set(&o, &pn, v.clone())?;
                    self.stack.push(v);
                }
                Op::PrivateIn => {
                    let o = self.pop();
                    let pn = self.pop();
                    let o = match o {
                        Value::Object(o) => o,
                        _ => return self.throw_type("Cannot use 'in' operator to search for a private field in a non-object"),
                    };
                    let r = self.private_find(o, &pn).is_some();
                    self.stack.push(Value::Bool(r));
                }
                Op::In => {
                    let o = self.pop();
                    let k = self.pop();
                    let o = match o {
                        Value::Object(o) => o,
                        _ => return self.throw_type("Cannot use 'in' operator to search for a key in a non-object"),
                    };
                    let key = self.to_property_key(&k)?;
                    let r = self.has_property(o, &key)?;
                    self.stack.push(Value::Bool(r));
                }
                Op::InstanceOf => {
                    let t = self.pop();
                    let v = self.pop();
                    let r = self.instance_of(&v, &t)?;
                    self.stack.push(Value::Bool(r));
                }
                Op::SuperBase => {
                    let k = self.pop();
                    let f = self.pop();
                    let base = self.super_base(&f)?;
                    let k = self.to_property_key(&k)?;
                    self.stack.push(base);
                    self.stack.push(k.to_value());
                }
                Op::ToPropertyKeyChecked => {
                    let k = self.pop();
                    let o = self.stack.last().unwrap();
                    if o.is_nullish() {
                        let msg = alloc::format!("Cannot read properties of {}", if o.is_null() { "null" } else { "undefined" });
                        return self.throw_type(&msg).map(|_: ()| unreachable!());
                    }
                    let k = self.to_property_key(&k)?;
                    self.stack.push(k.to_value());
                }
                Op::ToPropertyKey => {
                    let v = self.pop();
                    let k = self.to_property_key(&v)?;
                    self.stack.push(k.to_value());
                }
                Op::ToNumeric => {
                    let v = self.pop();
                    let n = self.to_numeric(&v)?;
                    self.stack.push(n);
                }
                Op::ToNumber => {
                    let v = self.pop();
                    let n = self.to_number(&v)?;
                    self.stack.push(Value::Number(n));
                }
                Op::ToStringOp => {
                    let v = self.pop();
                    let s = self.to_string(&v)?;
                    self.stack.push(Value::String(s));
                }
                Op::ToObject => {
                    let v = self.pop();
                    let o = self.to_object(&v)?;
                    self.stack.push(o);
                }
                Op::RequireObjectCoercible => {
                    if self.top().is_nullish() {
                        return self.throw_type("Cannot destructure 'undefined' or 'null'");
                    }
                }
                Op::RequireObjectCoercibleResult => {
                    let v = self.pop();
                    if !v.is_object() {
                        return self.throw_type("iterator result is not an object");
                    }
                }

                // ---- operators
                Op::Add => {
                    let b = self.pop();
                    let a = self.pop();
                    let r = match (&a, &b) {
                        (Value::Number(x), Value::Number(y)) => Value::Number(x + y),
                        (Value::String(x), Value::String(y)) => Value::String(self.concat(x, y)?),
                        _ => self.add(&a, &b)?,
                    };
                    self.stack.push(r);
                }
                Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Exp | Op::BitAnd | Op::BitOr | Op::BitXor | Op::Shl | Op::Shr | Op::UShr => {
                    let b = self.pop();
                    let a = self.pop();
                    let r = match (&a, &b, op) {
                        (Value::Number(x), Value::Number(y), Op::Sub) => Value::Number(x - y),
                        (Value::Number(x), Value::Number(y), Op::Mul) => Value::Number(x * y),
                        (Value::Number(x), Value::Number(y), Op::Div) => Value::Number(x / y),
                        _ => self.arith(op, &a, &b)?,
                    };
                    self.stack.push(r);
                }
                Op::Neg => {
                    let a = self.pop();
                    let r = match self.to_numeric(&a)? {
                        Value::Number(n) => Value::Number(-n),
                        Value::BigInt(b) => Value::BigInt(Rc::new(BigInt::from_mag(!b.neg, b.mag.clone()))),
                        _ => unreachable!(),
                    };
                    self.stack.push(r);
                }
                Op::Pos => {
                    let a = self.pop();
                    let n = self.to_number(&a)?;
                    self.stack.push(Value::Number(n));
                }
                Op::BitNot => {
                    let a = self.pop();
                    let r = match self.to_numeric(&a)? {
                        Value::Number(n) => Value::Number(!ops::to_int32(n) as f64),
                        Value::BigInt(b) => Value::BigInt(Rc::new(crate::builtins::bigint::bigint_not(&b))),
                        _ => unreachable!(),
                    };
                    self.stack.push(r);
                }
                Op::Not => {
                    let a = self.pop();
                    let b = self.to_boolean(&a);
                    self.stack.push(Value::Bool(!b));
                }
                Op::Inc | Op::Dec => {
                    let a = self.pop();
                    let d = if op == Op::Inc { 1.0 } else { -1.0 };
                    let r = match a {
                        Value::Number(n) => Value::Number(n + d),
                        Value::BigInt(b) => {
                            let one = BigInt::from_i64(d as i64);
                            Value::BigInt(Rc::new(crate::builtins::bigint::bigint_add(&b, &one)))
                        }
                        other => {
                            let n = self.to_numeric(&other)?;
                            self.stack.push(n);
                            self.frames[fi].pc = pc;
                            continue;
                        }
                    };
                    self.stack.push(r);
                }
                Op::Typeof => {
                    let a = self.pop();
                    let t = self.type_of(&a);
                    self.stack.push(Value::String(t));
                }
                Op::Eq | Op::Ne => {
                    let b = self.pop();
                    let a = self.pop();
                    let r = self.loose_eq(&a, &b)?;
                    self.stack.push(Value::Bool(if op == Op::Eq { r } else { !r }));
                }
                Op::StrictEq => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(Value::Bool(a.strict_eq(&b)));
                }
                Op::StrictNe => {
                    let b = self.pop();
                    let a = self.pop();
                    self.stack.push(Value::Bool(!a.strict_eq(&b)));
                }
                Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                    let b = self.pop();
                    let a = self.pop();
                    let r = match (&a, &b) {
                        (Value::Number(x), Value::Number(y)) => match op {
                            Op::Lt => x < y,
                            Op::Le => x <= y,
                            Op::Gt => x > y,
                            _ => x >= y,
                        },
                        _ => self.compare_op(op, &a, &b)?,
                    };
                    self.stack.push(Value::Bool(r));
                }

                // ---- control
                Op::Jump(t) => {
                    if (t as usize) < pc {
                        self.maybe_gc();
                    }
                    self.frames[fi].pc = t as usize;
                }
                Op::JumpIfFalse(t) => {
                    let v = self.pop();
                    if !self.to_boolean(&v) {
                        self.frames[fi].pc = t as usize;
                    }
                }
                Op::JumpIfTrue(t) => {
                    let v = self.pop();
                    if self.to_boolean(&v) {
                        if (t as usize) < pc {
                            self.maybe_gc();
                        }
                        self.frames[fi].pc = t as usize;
                    }
                }
                Op::JumpIfFalseKeep(t) => {
                    if !self.to_boolean(self.top()) {
                        self.frames[fi].pc = t as usize;
                    }
                }
                Op::JumpIfTrueKeep(t) => {
                    if self.to_boolean(self.top()) {
                        self.frames[fi].pc = t as usize;
                    }
                }
                Op::JumpIfNotNullishKeep(t) => {
                    if !self.top().is_nullish() {
                        self.frames[fi].pc = t as usize;
                    }
                }
                Op::JumpIfNullishKeep(t) => {
                    if self.top().is_nullish() {
                        self.frames[fi].pc = t as usize;
                    }
                }
                Op::JumpIfNullishUndef(t) => {
                    if self.top().is_nullish() {
                        self.pop();
                        self.stack.push(Value::Undefined);
                        self.frames[fi].pc = t as usize;
                    }
                }
                Op::JumpIfUndefined(t) => {
                    let v = self.pop();
                    if v.is_undefined() {
                        self.frames[fi].pc = t as usize;
                    }
                }
                Op::JumpIfNotUndefinedKeep(t) => {
                    if !self.top().is_undefined() {
                        self.frames[fi].pc = t as usize;
                    }
                }

                // ---- calls
                Op::Call(argc) => {
                    self.maybe_gc();
                    let base = self.stack.len() - argc as usize - 2;
                    self.do_call(base, argc as usize, fi)?;
                }
                Op::CallSpread => {
                    self.maybe_gc();
                    let arr = self.pop();
                    let args = self.array_to_list(&arr);
                    let n = args.len();
                    self.stack.extend(args);
                    let base = self.stack.len() - n - 2;
                    self.do_call(base, n, fi)?;
                }
                Op::New(argc) => {
                    self.maybe_gc();
                    let base = self.stack.len() - argc as usize - 1;
                    self.stack.insert(base + 1, Value::Undefined);
                    self.do_new(base, argc as usize)?;
                }
                Op::NewSpread => {
                    self.maybe_gc();
                    let arr = self.pop();
                    let args = self.array_to_list(&arr);
                    let n = args.len();
                    self.stack.push(Value::Undefined);
                    self.stack.extend(args);
                    let base = self.stack.len() - n - 2;
                    self.do_new(base, n)?;
                }
                Op::SuperCall(argc) => {
                    let n = argc as usize;
                    let args: Vec<Value> = self.stack.split_off(self.stack.len() - n);
                    let nt = self.pop();
                    let f = self.pop();
                    let r = self.super_call(&f, &nt, &args)?;
                    self.stack.push(r);
                }
                Op::SuperCallSpread => {
                    let arr = self.pop();
                    let args = self.array_to_list(&arr);
                    let nt = self.pop();
                    let f = self.pop();
                    let r = self.super_call(&f, &nt, &args)?;
                    self.stack.push(r);
                }
                Op::SuperCallForward => {
                    let f = &self.frames[fi];
                    let args: Vec<Value> = self.stack[f.args_base..f.args_base + f.argc].to_vec();
                    let nt = f.new_target.clone();
                    let func = Value::Object(f.func.unwrap());
                    let r = self.super_call(&func, &nt, &args)?;
                    self.frames[fi].this = r.clone();
                    if let Value::Object(o) = &r {
                        self.init_fields(*o, func.as_object().unwrap())?;
                    }
                    self.stack.push(r);
                }
                Op::DirectEval(argc) => {
                    let base = self.stack.len() - argc as usize - 2;
                    let is_eval = matches!(&self.stack[base], Value::Object(o) if *o == self.intr().eval_fn);
                    if is_eval {
                        let arg = if argc > 0 { self.stack[base + 2].clone() } else { Value::Undefined };
                        self.stack.truncate(base);
                        let r = self.direct_eval(arg)?;
                        self.stack.push(r);
                    } else {
                        self.do_call(base, argc as usize, fi)?;
                    }
                }
                Op::DirectEvalSpread => {
                    let arr = self.pop();
                    let args = self.array_to_list(&arr);
                    let n = args.len();
                    let base = self.stack.len() - 2;
                    let is_eval = matches!(&self.stack[base], Value::Object(o) if *o == self.intr().eval_fn);
                    if is_eval {
                        self.stack.truncate(base);
                        let r = self.direct_eval(args.first().cloned().unwrap_or(Value::Undefined))?;
                        self.stack.push(r);
                    } else {
                        self.stack.extend(args);
                        self.do_call(base, n, fi)?;
                    }
                }
                Op::Return => {
                    let v = self.pop();
                    if let Some(c) = self.do_return(v)? {
                        return Ok(c);
                    }
                }
                Op::Throw => {
                    let v = self.pop();
                    return Err(v);
                }
                Op::ThrowRef(k) => {
                    let m = self.frames[fi].code.str(k).to_rust();
                    return self.throw_ref(&m);
                }
                Op::ThrowType(k) => {
                    let m = self.frames[fi].code.str(k).to_rust();
                    return self.throw_type(&m);
                }
                Op::ThrowSyntax(k) => {
                    let m = self.frames[fi].code.str(k).to_rust();
                    return self.throw_syntax(&m);
                }
                Op::PushHandler(t) => {
                    let sp = self.stack.len() as u32;
                    let env = self.frames[fi].env;
                    self.handlers.push(Handler { pc: t, sp, env });
                }
                Op::PopHandler => {
                    self.handlers.pop();
                }

                // ---- iteration
                Op::GetIterator => {
                    let v = self.pop();
                    let (it, next) = self.get_iterator(&v)?;
                    self.stack.push(it);
                    self.stack.push(next);
                }
                Op::GetAsyncIterator => {
                    let v = self.pop();
                    let (it, next) = self.get_async_iterator(&v)?;
                    self.stack.push(it);
                    self.stack.push(next);
                }
                Op::IterNext => {
                    let n = self.stack.len();
                    let it = self.stack[n - 2].clone();
                    let next = self.stack[n - 1].clone();
                    let r = self.call(&next, &it, &[])?;
                    self.stack.push(r);
                }
                Op::IterStep(t) => {
                    let next = self.pop();
                    let it = self.pop();
                    match self.iterator_step_value(&it, &next)? {
                        Some(v) => self.stack.push(v),
                        None => self.frames[fi].pc = t as usize,
                    }
                }
                Op::IterClose => {
                    let _next = self.pop();
                    let it = self.pop();
                    self.iterator_close(&it)?;
                }
                Op::IterCloseQuiet => {
                    let _next = self.pop();
                    let it = self.pop();
                    let _ = self.iterator_close(&it);
                }
                Op::AsyncIterClose => {
                    let _next = self.pop();
                    let it = self.pop();
                    // GetMethod(iterator, "return"); undefined -> nothing to await (push a resolved value).
                    let m = self.get_method(&it, &PropertyKey::from_str("return"))?;
                    match m {
                        None => {
                            let o = self.new_plain_object();
                            self.stack.push(Value::Object(o));
                        }
                        Some(m) => {
                            let r = self.call(&m, &it, &[])?;
                            self.stack.push(r);
                        }
                    }
                }
                Op::IterResultDone => {
                    let r = self.pop();
                    if !r.is_object() {
                        return self.throw_type("iterator result is not an object");
                    }
                    let d = self.get_v(&r, &PropertyKey::from_str("done"))?;
                    let b = self.to_boolean(&d);
                    self.stack.push(Value::Bool(b));
                }
                Op::IterResultValue => {
                    let r = self.pop();
                    let v = self.get_v(&r, &PropertyKey::from_str("value"))?;
                    self.stack.push(v);
                }
                Op::ForInStart => {
                    let v = self.pop();
                    let e = self.for_in_start(&v)?;
                    self.stack.push(Value::Object(e));
                }
                Op::ForInNext(t) => {
                    let e = self.top().as_object().unwrap();
                    match self.for_in_next(e)? {
                        Some(k) => self.stack.push(Value::String(k)),
                        None => {
                            self.pop();
                            self.frames[fi].pc = t as usize;
                        }
                    }
                }

                // ---- functions / classes
                Op::Closure(k) => {
                    let code = match &self.frames[fi].code.consts[k as usize] {
                        Const::Code(c) => c.clone(),
                        _ => unreachable!(),
                    };
                    let env = self.frames[fi].env;
                    let script = self.frames[fi].script;
                    let f = self.make_closure(code, env, None, script);
                    self.stack.push(Value::Object(f));
                }
                Op::SetFunctionName(prefix) => {
                    let k = self.pop();
                    let f = self.top().as_object().unwrap();
                    let key = self.to_property_key(&k)?;
                    let p = match prefix {
                        1 => Some("get"),
                        2 => Some("set"),
                        _ => None,
                    };
                    self.set_function_name(f, &key, p);
                }
                Op::SetHome => {
                    let h = self.pop().as_object();
                    let f = self.top().as_object().unwrap();
                    if let Kind::Function(fd) = &mut self.heap.get_mut(f).kind {
                        fd.home = h;
                    }
                }
                Op::Class(k) => {
                    let code = match &self.frames[fi].code.consts[k as usize] {
                        Const::Code(c) => c.clone(),
                        _ => unreachable!(),
                    };
                    let heritage = self.pop();
                    let name = self.pop();
                    let (f, proto) = self.class_create(code, heritage, name)?;
                    self.stack.push(Value::Object(f));
                    self.stack.push(Value::Object(proto));
                }
                Op::ClassField(flags) => {
                    let init = self.pop();
                    let key = self.pop();
                    let ctor = self.top().as_object().unwrap();
                    self.class_field(ctor, key, init, flags)?;
                }
                Op::ClassPrivateMethod(kind) => {
                    let f = self.pop().as_object().unwrap();
                    let pn = self.pop();
                    let ctor = self.top().as_object().unwrap();
                    self.class_private_method(ctor, pn, f, kind)?;
                }
                Op::ClassStaticBlock => {
                    let f = self.pop().as_object().unwrap();
                    let ctor = self.top().as_object().unwrap();
                    if let Kind::Function(fd) = &mut self.heap.get_mut(f).kind {
                        fd.home = Some(ctor);
                    }
                    self.class_data_mut(ctor).statics.push((None, Some(f)));
                }
                Op::ClassFinish => {
                    let ctor = self.top().as_object().unwrap();
                    self.class_finish(ctor)?;
                }
                Op::NewPrivateName(k) => {
                    let d = self.frames[fi].code.str(k).clone();
                    self.stack.push(Value::Symbol(Sym::private(d)));
                }
                Op::InitFields => {
                    let f = self.pop();
                    let this = self.pop();
                    if let (Value::Object(o), Value::Object(f)) = (this, f) {
                        self.init_fields(o, f)?;
                    }
                }

                // ---- generators / async
                Op::GenStart => {
                    if let Some(c) = self.gen_start()? {
                        return Ok(c);
                    }
                }
                Op::Yield => {
                    let v = self.pop();
                    if let Some(c) = self.suspend(v, false)? {
                        return Ok(c);
                    }
                }
                Op::GenDispatch(t) => {
                    let kind = self.frames[fi].resume_kind;
                    self.frames[fi].resume_kind = 0;
                    match kind {
                        1 => {
                            let v = self.pop();
                            return Err(v);
                        }
                        2 => self.frames[fi].pc = t as usize,
                        _ => {}
                    }
                }
                Op::Await => {
                    let v = self.pop();
                    if let Some(c) = self.await_value(v)? {
                        return Ok(c);
                    }
                }
                Op::YieldStarCall(t_throw, t_ret) => {
                    let next = self.pop();
                    let iter = self.pop();
                    let received = self.pop();
                    let kind = self.frames[fi].resume_kind;
                    self.frames[fi].resume_kind = 0;
                    match kind {
                        1 => match self.get_method(&iter, &PropertyKey::from_str("throw"))? {
                            Some(m) => {
                                let r = self.call(&m, &iter, &[received])?;
                                self.stack.push(Value::Number(1.0));
                                self.stack.push(r);
                            }
                            None => self.frames[fi].pc = t_throw as usize,
                        },
                        2 => match self.get_method(&iter, &PropertyKey::from_str("return"))? {
                            Some(m) => {
                                let r = self.call(&m, &iter, &[received])?;
                                self.stack.push(Value::Number(2.0));
                                self.stack.push(r);
                            }
                            None => {
                                self.stack.push(received);
                                self.frames[fi].pc = t_ret as usize;
                            }
                        },
                        _ => {
                            let r = self.call(&next, &iter, &[received])?;
                            self.stack.push(Value::Number(0.0));
                            self.stack.push(r);
                        }
                    }
                }
                Op::YieldStarCheck(t_done, t_ret) => {
                    let r = self.pop();
                    let kind = self.pop();
                    if !r.is_object() {
                        return self.throw_type("iterator result is not an object");
                    }
                    let d = self.get_v(&r, &PropertyKey::from_str("done"))?;
                    if self.to_boolean(&d) {
                        let v = self.get_v(&r, &PropertyKey::from_str("value"))?;
                        self.stack.push(v);
                        let is_ret = matches!(kind, Value::Number(n) if n == 2.0);
                        self.frames[fi].pc = if is_ret { t_ret } else { t_done } as usize;
                    } else if self.frames[fi].code.is_async {
                        let v = self.get_v(&r, &PropertyKey::from_str("value"))?;
                        self.stack.push(v);
                    } else {
                        self.stack.push(r);
                    }
                }
                Op::YieldRaw => {
                    let v = self.pop();
                    // A yield* delegation point: throw() resumes into YieldStarCall instead of unwinding here.
                    self.frames[fi].resume_kind = 8;
                    if let Some(c) = self.suspend(v, true)? {
                        return Ok(c);
                    }
                }
                Op::IterResult(done) => {
                    let v = self.pop();
                    let o = self.iter_result(v, done);
                    self.stack.push(Value::Object(o));
                }
                Op::AsyncGenYield => {}

                // ---- misc
                Op::Debugger | Op::Nop => {}
                Op::TemplateObject(k) => {
                    let o = self.template_object(k)?;
                    self.stack.push(Value::Object(o));
                }
                Op::RegExp(k) => {
                    let (p, f) = match &self.frames[fi].code.consts[k as usize] {
                        Const::Regex(p, f) => (p.clone(), f.clone()),
                        _ => unreachable!(),
                    };
                    let r = crate::builtins::regexp::regexp_create_literal(self, p, f)?;
                    self.stack.push(Value::Object(r));
                }
                Op::ImportCall => {
                    let opts = self.pop();
                    let spec = self.pop();
                    let p = self.import_call(spec, opts)?;
                    self.stack.push(p);
                }
                Op::ImportMeta => {
                    let m = self.import_meta()?;
                    self.stack.push(m);
                }
                Op::GlobalInit(k) => {
                    let d = match &self.frames[fi].code.consts[k as usize] {
                        Const::Decls(d) => d.clone(),
                        _ => unreachable!(),
                    };
                    self.global_init(&d)?;
                }
                Op::EvalInit(k) => {
                    let d = match &self.frames[fi].code.consts[k as usize] {
                        Const::Decls(d) => d.clone(),
                        _ => unreachable!(),
                    };
                    self.eval_init(&d)?;
                }
                Op::BlockFnHoist(k) => {
                    let name = self.frames[fi].code.str(k).clone();
                    let v = self.pop();
                    self.block_fn_hoist(&name, v)?;
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------- call helpers

    fn do_call(&mut self, base: usize, argc: usize, _fi: usize) -> JsResult<()> {
        let callee = self.stack[base].clone();
        if let Value::Object(fo) = &callee {
            let fo = *fo;
            if let Kind::Function(fd) = &self.heap.get(fo).kind {
                if !self.heap.get(fo).class_ctor {
                    if self.frames.len() + self.native_depth >= self.max_depth {
                        return self.throw_range("Maximum call stack size exceeded");
                    }
                    let code = fd.code.clone();
                    if code.is_async && !code.is_generator {
                        let r = self.call_async(fo, code, base, argc)?;
                        self.stack.truncate(base);
                        self.stack.push(r);
                        return Ok(());
                    }
                    self.push_code_frame(fo, code, base, argc, Value::Undefined, false)?;
                    return Ok(());
                }
            }
            if !self.obj_is_callable(fo) {
                return self.throw_type(&self.not_callable_msg(&callee));
            }
            let r = self.call_at(base, argc, Value::Undefined, false);
            self.stack.truncate(base);
            self.stack.push(r?);
            return Ok(());
        }
        let m = self.not_callable_msg(&callee);
        self.throw_type(&m)
    }

    pub fn not_callable_msg(&self, v: &Value) -> alloc::string::String {
        let d = match v {
            Value::Undefined => "undefined",
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Number(_) => "a number",
            Value::String(_) => "a string",
            Value::Symbol(_) => "a symbol",
            Value::BigInt(_) => "a bigint",
            _ => "an object",
        };
        alloc::format!("{} is not a function", d)
    }

    /// Call an async function: run it synchronously until its first await; return its promise.
    pub fn call_async_at(&mut self, fo: Obj, code: Rc<Code>, base: usize, argc: usize) -> JsResult<Value> {
        self.call_async(fo, code, base, argc)
    }

    fn call_async(&mut self, fo: Obj, code: Rc<Code>, base: usize, argc: usize) -> JsResult<Value> {
        self.push_code_frame(fo, code, base, argc, Value::Undefined, false)?;
        let co = self.new_coroutine(CoroKind::Async);
        let fi = self.frames.len() - 1;
        self.frames[fi].coroutine = Some(co);
        self.frames[fi].entry = true;
        match self.run()? {
            Completion::Return(v) | Completion::Suspend(v) => Ok(v),
        }
    }

    fn do_new(&mut self, base: usize, argc: usize) -> JsResult<()> {
        let callee = self.stack[base].clone();
        if !self.is_constructor(&callee) {
            return self.throw_type("not a constructor");
        }
        let fo = callee.as_object().unwrap();
        if let Kind::Function(fd) = &self.heap.get(fo).kind {
            if self.frames.len() + self.native_depth >= self.max_depth {
                return self.throw_range("Maximum call stack size exceeded");
            }
            let code = fd.code.clone();
            self.push_code_frame(fo, code, base, argc, callee, true)?;
            return Ok(());
        }
        let r = self.call_at(base, argc, callee, true);
        self.stack.truncate(base);
        self.stack.push(r?);
        Ok(())
    }

    /// Return from the current frame. Some(c) when the entry frame completed.
    fn do_return(&mut self, v: Value) -> JsResult<Option<Completion>> {
        let f = self.frames.pop().unwrap();
        let mut v = v;
        if f.construct && !v.is_object() {
            v = f.this.clone();
        }
        self.handlers.truncate(f.handler_base);
        self.stack.truncate(f.args_base - 2);
        if let Some(caller) = self.frames.last() {
            self.cur_realm = caller.realm;
        }
        if let Some(co) = f.coroutine {
            v = self.coroutine_finish(co, Ok(v));
        }
        if f.entry {
            return Ok(Some(Completion::Return(v)));
        }
        self.stack.push(v);
        Ok(None)
    }

    pub fn tdz_error(&mut self, name: &str) -> Value {
        self.reference_error(&alloc::format!("Cannot access '{}' before initialization", name))
    }
    pub fn not_defined(&mut self, name: &JsStr) -> Value {
        self.reference_error(&alloc::format!("{} is not defined", name))
    }

    // ------------------------------------------------------------------------------------- environments

    #[inline]
    fn env_at(&self, d: u16) -> Obj {
        let mut e = self.frames.last().unwrap().env.unwrap();
        for _ in 0..d {
            e = self.env_parent(e).unwrap();
        }
        e
    }
    pub fn env_parent(&self, e: Obj) -> Option<Obj> {
        match &self.heap.get(e).kind {
            Kind::Env(d) => d.parent,
            Kind::ObjEnv(d) => d.parent,
            _ => None,
        }
    }
    #[inline]
    pub fn env_slot(&self, e: Obj, i: u32) -> Value {
        match &self.heap.get(e).kind {
            Kind::Env(d) => d.slots[i as usize].clone(),
            _ => Value::Undefined,
        }
    }
    #[inline]
    pub fn set_env_slot(&mut self, e: Obj, i: u32, v: Value) {
        if let Kind::Env(d) = &mut self.heap.get_mut(e).kind {
            d.slots[i as usize] = v;
        }
    }
    fn env_name(&self, e: Obj, i: u32) -> alloc::string::String {
        match &self.heap.get(e).kind {
            Kind::Env(d) => d.info.names[i as usize].to_rust(),
            _ => alloc::string::String::new(),
        }
    }
}
