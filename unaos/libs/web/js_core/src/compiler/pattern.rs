//! Destructuring: binding and assignment patterns (§8.6.2 BindingInitialization, §13.15.5).

use super::*;

/// The reference parts of an assignment target evaluated before its value is fetched.
pub(crate) enum TargetRef {
    None,
    Prop(u32),
    Elem,
    Private,
    Super,
}

impl Gen {
    /// Bind the value on the top of the stack to a pattern (consumes it).
    pub(crate) fn bind_pattern(&mut self, p: &Pat, mode: BindMode) {
        match p {
            Pat::Ident(id) => match mode {
                BindMode::Assign => {
                    self.store_name(&id.name);
                    self.emit(Op::Pop);
                }
                BindMode::Init => self.init_name(&id.name),
            },
            Pat::Expr(e) => {
                let t = self.temp();
                self.emit(Op::PutLocal(t));
                let r = self.target_prepare(e);
                self.emit(Op::GetLocal(t));
                self.free_temp(t);
                self.target_store(r);
            }
            Pat::Assign(t, d, _) => {
                let j = self.emit(Op::JumpIfNotUndefinedKeep(0));
                self.emit(Op::Pop);
                match &**t {
                    Pat::Ident(id) if d.is_anonymous_fn() => self.expr_named(d, &JsStr::from_str(&id.name)),
                    _ => self.expr(d),
                }
                self.patch(j);
                self.bind_pattern(t, mode);
            }
            Pat::Object(props, rest, _) => self.object_pattern(props, rest.as_deref(), mode),
            Pat::Array(elems, rest, _) => self.array_pattern(elems, rest.as_deref(), mode),
        }
    }

    /// Evaluate the reference parts of a member target.
    pub(crate) fn target_prepare(&mut self, e: &Expr) -> TargetRef {
        match e.unparen() {
            Expr::Member(o, p, _, _) => {
                self.expr(o);
                match &**p {
                    MemberProp::Name(n) => TargetRef::Prop(self.str_const(n)),
                    MemberProp::Computed(x) => {
                        self.expr(x);
                        TargetRef::Elem
                    }
                    MemberProp::Private(n) => {
                        self.load_private_name(n);
                        TargetRef::Private
                    }
                }
            }
            Expr::SuperMember(p, _) => {
                self.load_this();
                self.load_home_fn();
                self.super_key(p);
                TargetRef::Super
            }
            other => {
                // Annex B call target: evaluate, then throw on store.
                self.expr(other);
                self.emit(Op::Pop);
                TargetRef::None
            }
        }
    }

    /// Store the value on top of the stack into a prepared target (consumes parts and value).
    pub(crate) fn target_store(&mut self, r: TargetRef) {
        match r {
            TargetRef::Prop(k) => {
                self.emit(Op::SetProp(k));
            }
            TargetRef::Elem => {
                self.emit(Op::SetElem);
            }
            TargetRef::Private => {
                self.emit(Op::SetPrivate);
            }
            TargetRef::Super => {
                self.emit(Op::SetSuper);
            }
            TargetRef::None => {
                let k = self.str_const("Invalid destructuring assignment target");
                self.emit(Op::ThrowRef(k));
            }
        }
        self.emit(Op::Pop);
    }

    /// For an element / property whose target is a member expression (possibly with a default), evaluate the
    /// reference first (spec order). Returns the prepared ref and the inner target.
    fn prepare_elem<'a>(&mut self, p: &'a Pat) -> (Option<TargetRef>, &'a Pat) {
        let inner = match p {
            Pat::Assign(t, _, _) => &**t,
            other => other,
        };
        if let Pat::Expr(e) = inner {
            let r = self.target_prepare(e);
            return (Some(r), p);
        }
        (None, p)
    }

    /// Finish an element: the value is on the stack; apply the default then bind.
    fn finish_elem(&mut self, prepared: Option<TargetRef>, p: &Pat, mode: BindMode) {
        match prepared {
            Some(r) => {
                if let Pat::Assign(_, d, _) = p {
                    let j = self.emit(Op::JumpIfNotUndefinedKeep(0));
                    self.emit(Op::Pop);
                    self.expr(d);
                    self.patch(j);
                }
                self.target_store(r);
            }
            None => self.bind_pattern(p, mode),
        }
    }

    fn object_pattern(&mut self, props: &[PatProp], rest: Option<&Pat>, mode: BindMode) {
        self.emit(Op::RequireObjectCoercible);
        let src = self.temp();
        self.emit(Op::PutLocal(src));
        let mut key_temps: Vec<u32> = Vec::new();
        for pp in props {
            let (prepared, p) = self.prepare_elem(&pp.value);
            self.emit(Op::GetLocal(src));
            match &pp.key {
                PropKey::Computed(e) => {
                    self.expr(e);
                    self.emit(Op::ToPropertyKey);
                    if rest.is_some() {
                        let t = self.temp();
                        self.emit(Op::Dup);
                        self.emit(Op::PutLocal(t));
                        key_temps.push(t);
                    }
                    self.emit(Op::GetElem);
                }
                k => {
                    let s = self.static_key(k);
                    if rest.is_some() {
                        let t = self.temp();
                        let kk = self.js_const(&s);
                        self.emit(Op::Const(kk));
                        self.emit(Op::PutLocal(t));
                        key_temps.push(t);
                    }
                    let kk = self.js_const(&s);
                    self.emit(Op::GetProp(kk));
                }
            }
            self.finish_elem(prepared, p, mode);
        }
        if let Some(r) = rest {
            let prepared = if let Pat::Expr(e) = r { Some(self.target_prepare(e)) } else { None };
            self.emit(Op::NewObject);
            self.emit(Op::GetLocal(src));
            for t in &key_temps {
                self.emit(Op::GetLocal(*t));
            }
            self.emit(Op::CopyDataPropsExcl(key_temps.len() as u32));
            self.finish_elem(prepared, r, mode);
        }
        for t in key_temps {
            self.free_temp(t);
        }
        self.free_temp(src);
    }

    fn array_pattern(&mut self, elems: &[Option<Pat>], rest: Option<&Pat>, mode: BindMode) {
        let iter = self.temp();
        let next = self.temp();
        let done = self.temp();
        self.emit(Op::GetIterator);
        self.emit(Op::PutLocal(next));
        self.emit(Op::PutLocal(iter));
        self.emit(Op::False);
        self.emit(Op::PutLocal(done));
        let h = self.emit(Op::PushHandler(0));
        self.f.ctl.push(Ctl::Handler);
        for el in elems {
            match el {
                None => {
                    // Elision: step once if not done.
                    self.emit(Op::GetLocal(done));
                    let skip = self.emit(Op::JumpIfTrue(0));
                    self.step_value(iter, next, done);
                    self.emit(Op::Pop);
                    self.patch(skip);
                }
                Some(p) => {
                    let (prepared, p) = self.prepare_elem(p);
                    self.emit(Op::GetLocal(done));
                    let is_done = self.emit(Op::JumpIfTrue(0));
                    self.step_value(iter, next, done);
                    let have = self.emit(Op::Jump(0));
                    self.patch(is_done);
                    self.emit(Op::Undef);
                    self.patch(have);
                    self.finish_elem(prepared, p, mode);
                }
            }
        }
        if let Some(r) = rest {
            let prepared = if let Pat::Expr(e) = r { Some(self.target_prepare(e)) } else { None };
            self.emit(Op::NewArray(0));
            let top = self.here();
            self.emit(Op::GetLocal(done));
            let exit = self.emit(Op::JumpIfTrue(0));
            self.step_value(iter, next, done);
            // step_value leaves undefined when it hits the end (done set); check again
            self.emit(Op::GetLocal(done));
            let fin = self.emit(Op::JumpIfTrue(0));
            self.emit(Op::ArrayPush);
            self.emit(Op::Jump(top as u32));
            self.patch(fin);
            self.emit(Op::Pop);
            self.patch(exit);
            self.finish_elem(prepared, r, mode);
        }
        self.f.ctl.pop();
        self.emit(Op::PopHandler);
        // Normal completion: close if not done.
        self.emit(Op::GetLocal(done));
        let skip_close = self.emit(Op::JumpIfTrue(0));
        self.emit(Op::GetLocal(iter));
        self.emit(Op::GetLocal(next));
        self.emit(Op::IterClose);
        let end = self.emit(Op::Jump(0));
        self.patch(h);
        // Abrupt: close quietly if not done, rethrow.
        self.emit(Op::GetLocal(done));
        let rethrow = self.emit(Op::JumpIfTrue(0));
        self.emit(Op::GetLocal(iter));
        self.emit(Op::GetLocal(next));
        self.emit(Op::IterCloseQuiet);
        self.patch(rethrow);
        self.emit(Op::Throw);
        self.patch(skip_close);
        self.patch(end);
        self.free_temp(done);
        self.free_temp(next);
        self.free_temp(iter);
    }

    /// Step the iterator: pushes the next value, or undefined with `done` set when exhausted. A throwing
    /// next() leaves `done` true (no IteratorClose).
    fn step_value(&mut self, iter: u32, next: u32, done: u32) {
        self.emit(Op::True);
        self.emit(Op::PutLocal(done));
        self.emit(Op::GetLocal(iter));
        self.emit(Op::GetLocal(next));
        let d = self.emit(Op::IterStep(0));
        self.emit(Op::False);
        self.emit(Op::PutLocal(done));
        let over = self.emit(Op::Jump(0));
        self.patch(d);
        self.emit(Op::Undef);
        self.patch(over);
    }
}
