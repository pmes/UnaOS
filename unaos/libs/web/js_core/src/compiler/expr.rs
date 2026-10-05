//! Expression code generation, functions and classes.

use super::*;

impl Gen {
    // ------------------------------------------------------------------------------------- special bindings

    /// Push `this` (TDZ-checked in derived constructors).
    pub(crate) fn load_this(&mut self) {
        match self.special("this") {
            Some((r, true)) => self.load_ref(r, "this"),
            Some((_, false)) => {
                self.emit(Op::This);
            }
            None => {
                if self.mode == Mode::Module {
                    self.emit(Op::Undef);
                } else if self.mode == Mode::Eval {
                    let k = self.str_const("this");
                    self.emit(Op::GetName(k));
                } else {
                    self.emit(Op::GlobalThis);
                }
            }
        }
    }

    /// Push the raw `this` binding value (Empty if uninitialised) — derived-constructor returns.
    pub(crate) fn load_this_raw(&mut self) {
        match self.special("this") {
            Some((Ref::Local(s, _), true)) => {
                self.emit(Op::GetLocal(s));
            }
            Some((Ref::Env(d, i, _, _), true)) => {
                self.emit(Op::GetEnv(d, i));
            }
            _ => {
                self.emit(Op::This);
            }
        }
    }

    pub(crate) fn load_new_target(&mut self) {
        match self.special("new.target") {
            Some((r, true)) => self.load_ref(r, "new.target"),
            Some((_, false)) => {
                self.emit(Op::NewTarget);
            }
            None => {
                let k = self.str_const("new.target");
                self.emit(Op::GetName(k));
            }
        }
    }

    pub(crate) fn load_home_fn(&mut self) {
        match self.special("%fn") {
            Some((r, true)) => self.load_ref(r, "%fn"),
            Some((_, false)) => {
                self.emit(Op::Callee);
            }
            None => {
                let k = self.str_const("%fn");
                self.emit(Op::GetName(k));
            }
        }
    }

    /// Resolve a special binding: Some((ref, materialised)). Not materialised means the binding is in the
    /// current function and no inner function captures it: the frame's own value is used.
    fn special(&self, name: &str) -> Option<(Ref, bool)> {
        let mut s = Some(self.f.scope);
        let mut depth: u16 = 0;
        let mut dynamic = false;
        while let Some(i) = s {
            let sc = &self.tree.scopes[i as usize];
            if let Some(bi) = sc.find(name) {
                if dynamic {
                    return None;
                }
                let b = &sc.bindings[bi];
                let tdz = name == "this" && sc.derived_this;
                if b.env {
                    return Some((Ref::Env(depth, b.slot, tdz, false), true));
                }
                if tdz {
                    return Some((Ref::Local(b.slot, true), true));
                }
                return Some((Ref::Local(b.slot, false), false));
            }
            if sc.dynamic {
                dynamic = true;
            }
            if sc.needs_env {
                depth += 1;
            }
            s = sc.parent;
        }
        None
    }

    fn load_ref(&mut self, r: Ref, name: &str) {
        match r {
            Ref::Local(slot, tdz) => {
                if tdz {
                    let k = self.str_const(name);
                    self.emit(Op::GetLocalChk(slot, k));
                } else {
                    self.emit(Op::GetLocal(slot));
                }
            }
            Ref::Env(d, i, tdz, _) => {
                self.emit(if tdz { Op::GetEnvChk(d, i) } else { Op::GetEnv(d, i) });
            }
            _ => {
                let k = self.str_const(name);
                self.emit(Op::GetName(k));
            }
        }
    }

    // ------------------------------------------------------------------------------------- expressions

    pub(crate) fn expr_named(&mut self, e: &Expr, name: &JsStr) {
        match e.unparen() {
            Expr::Function(f) | Expr::Arrow(f) if f.id.is_none() => self.closure(f, Some(name.clone())),
            Expr::Class(c) if c.id.is_none() => self.class(c, Some(name.clone())),
            _ => self.expr(e),
        }
    }

    pub(crate) fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Ident(i) => {
                if &*i.name == "undefined" && matches!(self.resolve("undefined"), Ref::Global) {
                    self.emit(Op::Undef);
                    return;
                }
                self.pos(i.span.start);
                self.load_name(&i.name);
            }
            Expr::This(_) => self.load_this(),
            Expr::Num(v, _) => self.num(*v),
            Expr::Str(s, _) => {
                let k = self.js_const(s);
                self.emit(Op::Const(k));
            }
            Expr::BigInt(d, _) => {
                let mag = crate::bignum::BigUint::from_digits(d.as_bytes(), 10);
                let k = self.konst(Const::BigInt(Rc::new(crate::vm::value::BigInt::from_mag(false, mag))));
                self.emit(Op::Const(k));
            }
            Expr::Bool(b, _) => {
                self.emit(if *b { Op::True } else { Op::False });
            }
            Expr::Null(_) => {
                self.emit(Op::Null);
            }
            Expr::Regex(p, fl, sp) => {
                self.pos(sp.start);
                let k = self.konst(Const::Regex(p.clone(), fl.clone()));
                self.emit(Op::RegExp(k));
            }
            Expr::Template(t) => {
                let q0 = t.quasis[0].0.clone().unwrap_or_else(JsStr::empty);
                let k = self.js_const(&q0);
                self.emit(Op::Const(k));
                for (i, x) in t.exprs.iter().enumerate() {
                    self.expr(x);
                    self.emit(Op::ToStringOp);
                    self.emit(Op::Add);
                    let q = t.quasis[i + 1].0.clone().unwrap_or_else(JsStr::empty);
                    if !q.is_empty() {
                        let k = self.js_const(&q);
                        self.emit(Op::Const(k));
                        self.emit(Op::Add);
                    }
                }
            }
            Expr::TaggedTemplate(tag, t, site, sp) => {
                self.callee_and_this(tag);
                let info = TemplateInfo { site: *site, cooked: t.quasis.iter().map(|q| q.0.clone()).collect(), raw: t.quasis.iter().map(|q| q.1.clone()).collect() };
                let k = self.konst(Const::Template(Rc::new(info)));
                self.emit(Op::TemplateObject(k));
                for x in &t.exprs {
                    self.expr(x);
                }
                self.pos(sp.start);
                self.emit(Op::Call(t.exprs.len() as u32 + 1));
            }
            Expr::Array(els, _) => {
                self.emit(Op::NewArray(els.len() as u32));
                for el in els {
                    match el {
                        ArrayElem::Hole => {
                            self.emit(Op::ArrayHole);
                        }
                        ArrayElem::Expr(x) => {
                            self.expr(x);
                            self.emit(Op::ArrayPush);
                        }
                        ArrayElem::Spread(x) => {
                            self.expr(x);
                            self.emit(Op::ArraySpread);
                        }
                    }
                }
            }
            Expr::Object(props, _) => self.object_literal(props),
            Expr::Function(f) => self.closure(f, None),
            Expr::Arrow(f) => self.closure(f, None),
            Expr::Class(c) => self.class(c, None),
            Expr::Unary(op, a, sp) => self.unary(*op, a, *sp),
            Expr::Update(inc, prefix, a, sp) => {
                self.pos(sp.start);
                self.update(*inc, *prefix, a)
            }
            Expr::Binary(op, a, b, sp) => {
                self.expr(a);
                self.expr(b);
                self.pos(sp.start);
                self.binop(*op);
            }
            Expr::Logical(op, a, b, _) => {
                self.expr(a);
                let j = match op {
                    LogicalOp::And => self.emit(Op::JumpIfFalseKeep(0)),
                    LogicalOp::Or => self.emit(Op::JumpIfTrueKeep(0)),
                    LogicalOp::Nullish => self.emit(Op::JumpIfNotNullishKeep(0)),
                };
                self.emit(Op::Pop);
                self.expr(b);
                self.patch(j);
            }
            Expr::Assign(op, target, value, sp) => {
                self.pos(sp.start);
                self.assign(*op, target, value)
            }
            Expr::Cond(t, a, b, _) => {
                self.expr(t);
                let j = self.emit(Op::JumpIfFalse(0));
                self.expr(a);
                let j2 = self.emit(Op::Jump(0));
                self.patch(j);
                self.expr(b);
                self.patch(j2);
            }
            Expr::Call(..) | Expr::Member(..) | Expr::SuperMember(..) => {
                let mut pads = Vec::new();
                self.chain_part(e, &mut pads);
                debug_assert!(pads.is_empty());
            }
            Expr::Chain(inner, _) => {
                let mut pads: Vec<(usize, u32)> = Vec::new();
                self.chain_part(inner, &mut pads);
                self.chain_pads(pads, false);
            }
            Expr::New(callee, args, sp) => {
                self.expr(callee);
                if args.iter().any(|a| matches!(a, Arg::Spread(_))) {
                    self.spread_args(args);
                    self.pos(sp.start);
                    self.emit(Op::NewSpread);
                } else {
                    for a in args {
                        if let Arg::Expr(x) = a {
                            self.expr(x);
                        }
                    }
                    self.pos(sp.start);
                    self.emit(Op::New(args.len() as u32));
                }
            }
            Expr::SuperCall(args, sp) => {
                self.load_home_fn();
                self.load_new_target();
                if args.iter().any(|a| matches!(a, Arg::Spread(_))) {
                    self.spread_args(args);
                    self.pos(sp.start);
                    self.emit(Op::SuperCallSpread);
                } else {
                    for a in args {
                        if let Arg::Expr(x) = a {
                            self.expr(x);
                        }
                    }
                    self.pos(sp.start);
                    self.emit(Op::SuperCall(args.len() as u32));
                }
                // Bind `this`, then initialise fields.
                self.emit(Op::Dup);
                match self.special("this") {
                    Some((Ref::Local(s, _), _)) => {
                        self.emit(Op::InitThisLocal(s));
                    }
                    Some((Ref::Env(d, i, _, _), _)) => {
                        self.emit(Op::InitThisEnv(d, i));
                    }
                    _ => {
                        self.emit(Op::Pop);
                    }
                }
                self.emit(Op::Dup);
                self.load_home_fn();
                self.emit(Op::InitFields);
            }
            Expr::Seq(v, _) => {
                for (i, x) in v.iter().enumerate() {
                    self.expr(x);
                    if i + 1 < v.len() {
                        self.emit(Op::Pop);
                    }
                }
            }
            Expr::Yield(arg, delegate, sp) => {
                self.pos(sp.start);
                if *delegate {
                    self.yield_star(arg.as_ref().unwrap());
                } else {
                    match arg {
                        Some(a) => self.expr(a),
                        None => {
                            self.emit(Op::Undef);
                        }
                    }
                    self.yield_value();
                }
            }
            Expr::Await(a, sp) => {
                self.expr(a);
                self.pos(sp.start);
                self.emit(Op::Await);
            }
            Expr::NewTarget(_) => self.load_new_target(),
            Expr::ImportMeta(_) => {
                self.emit(Op::ImportMeta);
            }
            Expr::ImportCall(spec, opts, sp) => {
                self.expr(spec);
                match opts {
                    Some(o) => self.expr(o),
                    None => {
                        self.emit(Op::Undef);
                    }
                }
                self.pos(sp.start);
                self.emit(Op::ImportCall);
            }
            Expr::PrivateIn(n, obj, sp) => {
                self.load_private_name(n);
                self.expr(obj);
                self.pos(sp.start);
                self.emit(Op::PrivateIn);
            }
            Expr::Paren(inner, _) => self.expr(inner),
            Expr::CoverParams(..) | Expr::CoverPat(_) => {
                self.emit(Op::Undef);
            }
        }
    }

    /// Landing pads for optional-chain short circuits: drop the partial values, push undefined (or true for
    /// `delete`), and join the end of the chain.
    fn chain_pads(&mut self, pads: Vec<(usize, u32)>, delete: bool) {
        if pads.is_empty() {
            return;
        }
        let mut ends = vec![self.emit(Op::Jump(0))];
        for (j, extra) in pads {
            self.patch(j);
            for _ in 0..extra {
                self.emit(Op::Pop);
            }
            self.emit(if delete { Op::True } else { Op::Undef });
            ends.push(self.emit(Op::Jump(0)));
        }
        for e in ends {
            self.patch(e);
        }
    }

    pub(crate) fn load_private_name(&mut self, n: &str) {
        let name = alloc::format!("#{}", n);
        self.load_name(&name);
    }

    pub(crate) fn binop(&mut self, op: BinOp) {
        self.emit(match op {
            BinOp::Eq => Op::Eq,
            BinOp::Ne => Op::Ne,
            BinOp::StrictEq => Op::StrictEq,
            BinOp::StrictNe => Op::StrictNe,
            BinOp::Lt => Op::Lt,
            BinOp::Le => Op::Le,
            BinOp::Gt => Op::Gt,
            BinOp::Ge => Op::Ge,
            BinOp::Shl => Op::Shl,
            BinOp::Shr => Op::Shr,
            BinOp::UShr => Op::UShr,
            BinOp::Add => Op::Add,
            BinOp::Sub => Op::Sub,
            BinOp::Mul => Op::Mul,
            BinOp::Div => Op::Div,
            BinOp::Mod => Op::Mod,
            BinOp::Exp => Op::Exp,
            BinOp::BitOr => Op::BitOr,
            BinOp::BitXor => Op::BitXor,
            BinOp::BitAnd => Op::BitAnd,
            BinOp::In => Op::In,
            BinOp::InstanceOf => Op::InstanceOf,
        });
    }

    fn unary(&mut self, op: UnaryOp, a: &Expr, sp: Span) {
        match op {
            UnaryOp::Typeof => {
                if let Expr::Ident(i) = a.unparen() {
                    match self.resolve(&i.name) {
                        Ref::Dynamic | Ref::Global => {
                            let k = self.str_const(&i.name);
                            self.emit(Op::TypeofName(k));
                            return;
                        }
                        _ => {}
                    }
                }
                self.expr(a);
                self.emit(Op::Typeof);
            }
            UnaryOp::Delete => self.delete(a, sp),
            UnaryOp::Void => {
                self.expr(a);
                self.emit(Op::Pop);
                self.emit(Op::Undef);
            }
            _ => {
                self.expr(a);
                self.pos(sp.start);
                self.emit(match op {
                    UnaryOp::Minus => Op::Neg,
                    UnaryOp::Plus => Op::Pos,
                    UnaryOp::Not => Op::Not,
                    UnaryOp::BitNot => Op::BitNot,
                    _ => unreachable!(),
                });
            }
        }
    }

    fn delete(&mut self, a: &Expr, sp: Span) {
        self.pos(sp.start);
        match a.unparen() {
            Expr::Ident(i) => match self.resolve(&i.name) {
                Ref::Dynamic | Ref::Global => {
                    let k = self.str_const(&i.name);
                    self.emit(Op::DeleteName(k));
                }
                _ => {
                    self.emit(Op::False);
                }
            },
            Expr::Member(o, p, false, _) => {
                self.expr(o);
                match &**p {
                    MemberProp::Name(n) => {
                        let k = self.str_const(n);
                        self.emit(Op::DeleteProp(k));
                    }
                    MemberProp::Computed(x) => {
                        self.expr(x);
                        self.emit(Op::DeleteElem);
                    }
                    MemberProp::Private(_) => {
                        self.emit(Op::Pop);
                        self.emit(Op::True);
                    }
                }
            }
            Expr::SuperMember(p, _) => {
                // `delete super.x`: evaluate the key, then ReferenceError.
                self.load_this();
                self.emit(Op::Pop);
                if let MemberProp::Computed(x) = &**p {
                    self.expr(x);
                    self.emit(Op::Pop);
                }
                let k = self.str_const("Unsupported reference to 'super'");
                self.emit(Op::ThrowRef(k));
            }
            Expr::Chain(inner, _) => {
                // delete a?.b: true when short-circuited.
                if let Expr::Member(o, p, opt, _) = &**inner {
                    let mut pads = Vec::new();
                    self.chain_part(o, &mut pads);
                    let mut my_pad = None;
                    if *opt {
                        my_pad = Some(self.emit(Op::JumpIfNullishKeep(0)));
                    }
                    match &**p {
                        MemberProp::Name(n) => {
                            let k = self.str_const(n);
                            self.emit(Op::DeleteProp(k));
                        }
                        MemberProp::Computed(x) => {
                            self.expr(x);
                            self.emit(Op::DeleteElem);
                        }
                        MemberProp::Private(_) => {
                            self.emit(Op::Pop);
                            self.emit(Op::True);
                        }
                    }
                    let mut pads = pads;
                    if let Some(j) = my_pad {
                        pads.push((j, 1));
                    }
                    self.chain_pads(pads, true);
                } else {
                    self.expr(a);
                    self.emit(Op::Pop);
                    self.emit(Op::True);
                }
            }
            _ => {
                self.expr(a);
                self.emit(Op::Pop);
                self.emit(Op::True);
            }
        }
    }

    fn update(&mut self, inc: bool, prefix: bool, a: &Expr) {
        let opc = if inc { Op::Inc } else { Op::Dec };
        match a.unparen() {
            Expr::Ident(i) => {
                self.load_name(&i.name);
                self.emit(Op::ToNumeric);
                if prefix {
                    self.emit(opc);
                    self.store_name(&i.name);
                } else {
                    self.emit(Op::Dup);
                    self.emit(opc);
                    self.store_name(&i.name);
                    self.emit(Op::Pop);
                }
            }
            Expr::Member(o, p, false, _) => {
                self.expr(o);
                match &**p {
                    MemberProp::Name(n) => {
                        let k = self.str_const(n);
                        self.emit(Op::Dup);
                        self.emit(Op::GetProp(k));
                        self.emit(Op::ToNumeric);
                        if prefix {
                            self.emit(opc);
                            self.emit(Op::SetProp(k));
                        } else {
                            self.emit(Op::Dup);
                            self.emit(Op::Rot3);
                            self.emit(opc);
                            self.emit(Op::SetProp(k));
                            self.emit(Op::Pop);
                        }
                    }
                    MemberProp::Computed(x) => {
                        self.expr(x);
                        self.emit(Op::ToPropertyKey);
                        self.emit(Op::Dup2);
                        self.emit(Op::GetElem);
                        self.emit(Op::ToNumeric);
                        if prefix {
                            self.emit(opc);
                            self.emit(Op::SetElem);
                        } else {
                            // obj key old -> old obj key old
                            self.emit(Op::Dup);
                            self.emit(Op::Rot4);
                            self.emit(opc);
                            self.emit(Op::SetElem);
                            self.emit(Op::Pop);
                        }
                    }
                    MemberProp::Private(n) => {
                        self.load_private_name(n);
                        self.emit(Op::Dup2);
                        self.emit(Op::GetPrivate);
                        self.emit(Op::ToNumeric);
                        if prefix {
                            self.emit(opc);
                            self.emit(Op::SetPrivate);
                        } else {
                            self.emit(Op::Dup);
                            self.emit(Op::Rot4);
                            self.emit(opc);
                            self.emit(Op::SetPrivate);
                            self.emit(Op::Pop);
                        }
                    }
                }
            }
            Expr::SuperMember(p, _) => {
                self.load_this();
                self.load_home_fn();
                self.super_key(p);
                // this fn key -> this fn key this fn key
                self.emit(Op::Rot3Up); // fn key this
                self.emit(Op::Rot3Up); // key this fn
                self.emit(Op::Rot3Up); // this fn key
                self.dup3();
                self.emit(Op::GetSuper);
                self.emit(Op::ToNumeric);
                if prefix {
                    self.emit(opc);
                    self.emit(Op::SetSuper);
                } else {
                    let t = self.temp();
                    self.emit(Op::Dup);
                    self.emit(Op::PutLocal(t));
                    self.emit(opc);
                    self.emit(Op::SetSuper);
                    self.emit(Op::Pop);
                    self.emit(Op::GetLocal(t));
                    self.free_temp(t);
                }
            }
            _ => {
                // Annex B call targets: evaluate then ReferenceError.
                self.expr(a);
                self.emit(Op::Pop);
                let k = self.str_const("Invalid left-hand side in assignment");
                self.emit(Op::ThrowRef(k));
            }
        }
    }

    /// a b c -> a b c a b c
    pub(crate) fn dup3(&mut self) {
        let t = self.temp();
        let t2 = self.temp();
        let t3 = self.temp();
        self.emit(Op::PutLocal(t3));
        self.emit(Op::PutLocal(t2));
        self.emit(Op::PutLocal(t));
        for _ in 0..2 {
            self.emit(Op::GetLocal(t));
            self.emit(Op::GetLocal(t2));
            self.emit(Op::GetLocal(t3));
        }
        self.free_temp(t3);
        self.free_temp(t2);
        self.free_temp(t);
    }

    pub(crate) fn super_key(&mut self, p: &MemberProp) {
        match p {
            MemberProp::Name(n) => {
                let k = self.str_const(n);
                self.emit(Op::Const(k));
            }
            MemberProp::Computed(x) => {
                self.expr(x);
                self.emit(Op::ToPropertyKey);
            }
            MemberProp::Private(_) => {
                self.emit(Op::Undef);
            }
        }
    }

    fn assign(&mut self, op: AssignOp, target: &Pat, value: &Expr) {
        match op {
            AssignOp::Assign => match target {
                Pat::Ident(id) => {
                    if value.is_anonymous_fn() {
                        self.expr_named(value, &JsStr::from_str(&id.name));
                    } else {
                        self.expr(value);
                    }
                    self.store_name(&id.name);
                }
                Pat::Expr(e) => self.assign_member(e, value),
                _ => {
                    self.expr(value);
                    self.emit(Op::Dup);
                    self.bind_pattern(target, BindMode::Assign);
                }
            },
            AssignOp::Bin(b) => self.compound(target, value, Some(b), None),
            AssignOp::Logical(l) => self.compound(target, value, None, Some(l)),
        }
    }

    fn assign_member(&mut self, e: &Expr, value: &Expr) {
        match e.unparen() {
            Expr::Member(o, p, _, _) => {
                self.expr(o);
                match &**p {
                    MemberProp::Name(n) => {
                        self.expr(value);
                        let k = self.str_const(n);
                        self.emit(Op::SetProp(k));
                    }
                    MemberProp::Computed(x) => {
                        self.expr(x);
                        self.expr(value);
                        self.emit(Op::SetElem);
                    }
                    MemberProp::Private(n) => {
                        self.load_private_name(n);
                        self.expr(value);
                        self.emit(Op::SetPrivate);
                    }
                }
            }
            Expr::SuperMember(p, _) => {
                self.load_this();
                self.load_home_fn();
                self.super_key(p);
                self.expr(value);
                self.emit(Op::SetSuper);
            }
            _ => {
                // Annex B: `f() = v` evaluates f() then throws ReferenceError.
                self.expr(e);
                self.emit(Op::Pop);
                let k = self.str_const("Invalid left-hand side in assignment");
                self.emit(Op::ThrowRef(k));
            }
        }
    }

    fn compound(&mut self, target: &Pat, value: &Expr, bin: Option<BinOp>, logical: Option<LogicalOp>) {
        // Load current value with the reference parts left on the stack; `finish` stores.
        enum Kind {
            Name(String),
            Prop(u32),
            Elem,
            Private,
            Super,
            Invalid,
        }
        let kind = match target {
            Pat::Ident(id) => {
                self.load_name(&id.name);
                Kind::Name(String::from(&*id.name))
            }
            Pat::Expr(e) => match e.unparen() {
                Expr::Member(o, p, _, _) => {
                    self.expr(o);
                    match &**p {
                        MemberProp::Name(n) => {
                            let k = self.str_const(n);
                            self.emit(Op::Dup);
                            self.emit(Op::GetProp(k));
                            Kind::Prop(k)
                        }
                        MemberProp::Computed(x) => {
                            self.expr(x);
                            self.emit(Op::ToPropertyKey);
                            self.emit(Op::Dup2);
                            self.emit(Op::GetElem);
                            Kind::Elem
                        }
                        MemberProp::Private(n) => {
                            self.load_private_name(n);
                            self.emit(Op::Dup2);
                            self.emit(Op::GetPrivate);
                            Kind::Private
                        }
                    }
                }
                Expr::SuperMember(p, _) => {
                    self.load_this();
                    self.load_home_fn();
                    self.super_key(p);
                    self.dup3();
                    self.emit(Op::GetSuper);
                    Kind::Super
                }
                other => {
                    self.expr(other);
                    Kind::Invalid
                }
            },
            _ => Kind::Invalid,
        };
        if let Kind::Invalid = kind {
            self.emit(Op::Pop);
            let k = self.str_const("Invalid left-hand side in assignment");
            self.emit(Op::ThrowRef(k));
            return;
        }
        let name_for_fn = match &kind {
            Kind::Name(n) => Some(JsStr::from_str(n)),
            _ => None,
        };
        let mut short = None;
        if let Some(l) = logical {
            short = Some(match l {
                LogicalOp::And => self.emit(Op::JumpIfFalseKeep(0)),
                LogicalOp::Or => self.emit(Op::JumpIfTrueKeep(0)),
                LogicalOp::Nullish => self.emit(Op::JumpIfNotNullishKeep(0)),
            });
            self.emit(Op::Pop);
            match &name_for_fn {
                Some(n) if value.is_anonymous_fn() => self.expr_named(value, n),
                _ => self.expr(value),
            }
        } else {
            self.expr(value);
            self.binop(bin.unwrap());
        }
        match &kind {
            Kind::Name(n) => {
                let n = n.clone();
                self.store_name(&n);
            }
            Kind::Prop(k) => {
                self.emit(Op::SetProp(*k));
            }
            Kind::Elem => {
                self.emit(Op::SetElem);
            }
            Kind::Private => {
                self.emit(Op::SetPrivate);
            }
            Kind::Super => {
                self.emit(Op::SetSuper);
            }
            Kind::Invalid => {}
        }
        if let Some(j) = short {
            let end = self.emit(Op::Jump(0));
            self.patch(j);
            // Short-circuited: drop the reference parts below the current value.
            let extra = match &kind {
                Kind::Name(_) => 0,
                Kind::Prop(_) => 1,
                Kind::Elem | Kind::Private => 2,
                Kind::Super => 3,
                Kind::Invalid => 0,
            };
            for _ in 0..extra {
                self.emit(Op::Swap);
                self.emit(Op::Pop);
            }
            self.patch(end);
        }
    }

    // ------------------------------------------------------------------------------------- calls & chains

    /// Push [callee, this] for a call.
    pub(crate) fn callee_and_this(&mut self, callee: &Expr) {
        match callee {
            Expr::Member(o, p, false, _) => {
                self.expr(o);
                self.emit(Op::Dup);
                self.member_get(p);
                self.emit(Op::Swap);
            }
            Expr::SuperMember(p, _) => {
                self.load_this();
                self.load_home_fn();
                self.super_key(p);
                self.emit(Op::GetSuper);
                self.load_this();
            }
            Expr::Ident(i) => match self.resolve(&i.name) {
                Ref::Dynamic => {
                    let k = self.str_const(&i.name);
                    self.emit(Op::GetNameThis(k));
                }
                _ => {
                    self.load_name(&i.name);
                    self.emit(Op::Undef);
                }
            },
            Expr::Paren(inner, _) if matches!(&**inner, Expr::Member(..) | Expr::SuperMember(..)) => {
                // (a.b)() keeps the base as this
                self.callee_and_this(inner);
            }
            _ => {
                self.expr(callee);
                self.emit(Op::Undef);
            }
        }
    }

    fn member_get(&mut self, p: &MemberProp) {
        match p {
            MemberProp::Name(n) => {
                let k = self.str_const(n);
                self.emit(Op::GetProp(k));
            }
            MemberProp::Computed(x) => {
                self.expr(x);
                self.emit(Op::GetElem);
            }
            MemberProp::Private(n) => {
                self.load_private_name(n);
                self.emit(Op::GetPrivate);
            }
        }
    }

    pub(crate) fn spread_args(&mut self, args: &[Arg]) {
        self.emit(Op::NewArray(args.len() as u32));
        for a in args {
            match a {
                Arg::Expr(x) => {
                    self.expr(x);
                    self.emit(Op::ArrayPush);
                }
                Arg::Spread(x) => {
                    self.expr(x);
                    self.emit(Op::ArraySpread);
                }
            }
        }
    }

    /// Compile part of a (possibly optional) chain. Short-circuit jumps are recorded in `pads` with the
    /// number of stack values to drop.
    fn chain_part(&mut self, e: &Expr, pads: &mut Vec<(usize, u32)>) {
        match e {
            Expr::Member(o, p, optional, sp) => {
                self.chain_part(o, pads);
                if *optional {
                    let j = self.emit(Op::JumpIfNullishKeep(0));
                    pads.push((j, 1));
                }
                self.pos(sp.start);
                self.member_get(p);
            }
            Expr::Call(callee, args, optional, sp) => {
                // callee + this
                let direct_eval = !*optional && matches!(&**callee, Expr::Ident(i) if &*i.name == "eval");
                match &**callee {
                    Expr::Member(o, p, mopt, _) => {
                        self.chain_part(o, pads);
                        if *mopt {
                            let j = self.emit(Op::JumpIfNullishKeep(0));
                            pads.push((j, 1));
                        }
                        self.emit(Op::Dup);
                        self.member_get(p);
                        self.emit(Op::Swap);
                    }
                    Expr::Call(..) | Expr::Chain(..) => {
                        self.chain_part(callee, pads);
                        self.emit(Op::Undef);
                    }
                    other => self.callee_and_this(other),
                }
                if *optional {
                    // [fn this]: if fn is nullish, short-circuit.
                    self.emit(Op::Swap);
                    let j = self.emit(Op::JumpIfNullishKeep(0));
                    pads.push((j, 2));
                    self.emit(Op::Swap);
                }
                let spread = args.iter().any(|a| matches!(a, Arg::Spread(_)));
                if spread {
                    self.spread_args(args);
                    self.pos(sp.start);
                    self.emit(if direct_eval { Op::DirectEvalSpread } else { Op::CallSpread });
                } else {
                    for a in args {
                        if let Arg::Expr(x) = a {
                            self.expr(x);
                        }
                    }
                    self.pos(sp.start);
                    self.emit(if direct_eval { Op::DirectEval(args.len() as u32) } else { Op::Call(args.len() as u32) });
                }
            }
            Expr::SuperMember(p, sp) => {
                self.load_this();
                self.load_home_fn();
                self.super_key(p);
                self.pos(sp.start);
                self.emit(Op::GetSuper);
            }
            _ => self.expr(e),
        }
    }

    // ------------------------------------------------------------------------------------- generators

    pub(crate) fn yield_value(&mut self) {
        if self.f.is_async {
            self.emit(Op::Await);
        }
        self.emit(Op::Yield);
        let d = self.emit(Op::GenDispatch(0));
        let over = self.emit(Op::Jump(0));
        self.patch(d);
        // Return resumption: value on the stack.
        if self.f.is_async {
            self.emit(Op::Await);
        }
        self.jump_out(JumpKind::Return, None);
        self.patch(over);
    }

    fn yield_star(&mut self, arg: &Expr) {
        let is_async = self.f.is_async;
        self.expr(arg);
        let iter = self.temp();
        let next = self.temp();
        self.emit(if is_async { Op::GetAsyncIterator } else { Op::GetIterator });
        self.emit(Op::PutLocal(next));
        self.emit(Op::PutLocal(iter));
        self.emit(Op::Undef);
        let top = self.here();
        self.emit(Op::GetLocal(iter));
        self.emit(Op::GetLocal(next));
        let call = self.emit(Op::YieldStarCall(0, 0));
        if is_async {
            self.emit(Op::Await);
        }
        let check = self.emit(Op::YieldStarCheck(0, 0));
        self.emit(if is_async { Op::Yield } else { Op::YieldRaw });
        self.emit(Op::Jump(top as u32));
        // throw() missing: close the iterator, then TypeError.
        let t_throw = self.here() as u32;
        self.emit(Op::GetLocal(iter));
        self.emit(Op::GetLocal(next));
        if is_async {
            self.emit(Op::AsyncIterClose);
            self.emit(Op::Await);
            self.emit(Op::RequireObjectCoercibleResult);
        } else {
            self.emit(Op::IterClose);
        }
        let k = self.str_const("The iterator does not provide a 'throw' method");
        self.emit(Op::ThrowType(k));
        // return() missing: return the received value (awaited in async generators).
        let t_ret_nomethod = self.here() as u32;
        if is_async {
            self.emit(Op::Await);
        }
        let t_ret_done = self.here() as u32;
        self.jump_out(JumpKind::Return, None);
        let t_done = self.here() as u32;
        self.f.ops[call] = Op::YieldStarCall(t_throw, t_ret_nomethod);
        self.f.ops[check] = Op::YieldStarCheck(t_done, t_ret_done);
        self.free_temp(iter);
        self.free_temp(next);
    }

    // ------------------------------------------------------------------------------------- object literals

    fn object_literal(&mut self, props: &[Prop]) {
        self.emit(Op::NewObject);
        for p in props {
            match p {
                Prop::KeyValue(k, v) => match k {
                    PropKey::Computed(e) => {
                        self.expr(e);
                        self.emit(Op::ToPropertyKey);
                        if v.is_anonymous_fn() {
                            self.emit(Op::Dup);
                            self.expr(v);
                            self.emit(Op::Swap);
                            self.emit(Op::SetFunctionName(0));
                        } else {
                            self.expr(v);
                        }
                        self.emit(Op::DefineField);
                    }
                    _ => {
                        let name = self.static_key(k);
                        if v.is_anonymous_fn() {
                            self.expr_named(v, &name);
                        } else {
                            self.expr(v);
                        }
                        let kk = self.js_const(&name);
                        self.emit(Op::DefineFieldNamed(kk));
                    }
                },
                Prop::Shorthand(id) => {
                    self.load_name(&id.name);
                    let kk = self.str_const(&id.name);
                    self.emit(Op::DefineFieldNamed(kk));
                }
                Prop::CoverInit(id, _) => {
                    self.load_name(&id.name);
                    let kk = self.str_const(&id.name);
                    self.emit(Op::DefineFieldNamed(kk));
                }
                Prop::Method(k, f, kind) => {
                    self.push_key(k);
                    self.closure(f, None);
                    let kd = match kind {
                        MethodKind::Method => 0,
                        MethodKind::Get => 1,
                        MethodKind::Set => 2,
                    };
                    self.emit(Op::DefineMethod(kd | 4));
                }
                Prop::Spread(x) => {
                    self.expr(x);
                    self.emit(Op::CopyDataProps);
                }
                Prop::Proto(v, _) => {
                    self.expr(v);
                    self.emit(Op::SetProtoLit);
                }
            }
        }
    }

    pub(crate) fn static_key(&self, k: &PropKey) -> JsStr {
        match k {
            PropKey::Name(n) => JsStr::from_str(n),
            PropKey::Str(s) => s.clone(),
            PropKey::Num(v) => JsStr::from_str(&crate::numconv::f64_to_js_string(*v)),
            PropKey::BigInt(d) => JsStr::from_str(d),
            PropKey::Private(n) => JsStr::from_str(&alloc::format!("#{}", n)),
            PropKey::Computed(_) => JsStr::empty(),
        }
    }

    /// Push a property key value (computed keys are converted with ToPropertyKey).
    pub(crate) fn push_key(&mut self, k: &PropKey) {
        match k {
            PropKey::Computed(e) => {
                self.expr(e);
                self.emit(Op::ToPropertyKey);
            }
            _ => {
                let s = self.static_key(k);
                let kk = self.js_const(&s);
                self.emit(Op::Const(kk));
            }
        }
    }

    // ------------------------------------------------------------------------------------- functions

    pub(crate) fn closure(&mut self, f: &Rc<Function>, name: Option<JsStr>) {
        // Named function expressions: bind the name in an intermediate scope captured by the closure.
        let named_expr = f.id.is_some() && f.kind == FnKind::Normal && self.tree.by_id.get(f.name_scope as usize).map(|&x| x != u32::MAX).unwrap_or(false);
        let code = self.compile_function(f, name);
        let k = self.konst(Const::Code(code));
        if named_expr {
            let ns = self.tree.get(f.name_scope);
            let saved = self.f.scope;
            self.enter_scope(ns);
            self.emit(Op::Closure(k));
            let (env, slot) = {
                let b = &self.tree.scopes[ns as usize].bindings[0];
                (b.env, b.slot)
            };
            if env {
                self.emit(Op::Dup);
                self.emit(Op::InitEnv(0, slot));
            }
            self.exit_scope(ns);
            self.f.scope = saved;
        } else {
            self.emit(Op::Closure(k));
        }
    }

    pub(crate) fn compile_function(&mut self, f: &Function, name: Option<JsStr>) -> Rc<Code> {
        let fs = self.tree.get(f.scope);
        let fname = match (&f.id, name) {
            (Some(id), _) => JsStr::from_str(&id.name),
            (None, Some(n)) => n,
            (None, None) => JsStr::empty(),
        };
        let saved = core::mem::replace(
            &mut self.f,
            FnState {
                ops: Vec::new(),
                consts: Vec::new(),
                nlocals: 0,
                scope: fs,
                func_scope: fs,
                ctl: Vec::new(),
                cv: None,
                is_async: f.is_async,
                is_generator: f.is_generator,
                kind: f.kind,
                derived: f.kind == FnKind::ClassConstructor && f.derived,
                pending_labels: Vec::new(),
                positions: Vec::new(),
                free_temps: Vec::new(),
                strict: f.strict,
            },
        );
        self.function_body(f, fs);
        let src = SourceRef { src: self.src.clone(), start: f.span.start, end: f.span.end };
        let mut nparams = f.params.len() as u32;
        if f.rest.is_some() {
            nparams += 0;
        }
        let mut code = self.finish(fname, nparams, f.length, f.kind, Some(src), false, false, false);
        code.simple_params = f.simple_params;
        code.has_fields = self.class_has_fields;
        self.f = saved;
        Rc::new(code)
    }

    fn function_body(&mut self, f: &Function, fs: u32) {
        self.enter_scope(fs);
        let derived = f.kind == FnKind::ClassConstructor && f.derived;
        // Special bindings that live in slots.
        for name in ["this", "new.target", "%fn"] {
            let Some(bi) = self.tree.scopes[fs as usize].find(name) else { continue };
            let b = &self.tree.scopes[fs as usize].bindings[bi];
            let (env, slot) = (b.env, b.slot);
            if name == "this" && derived {
                if !env {
                    self.emit(Op::PushEmpty);
                    self.emit(Op::PutLocal(slot));
                }
                continue;
            }
            if !env {
                continue;
            }
            match name {
                "this" => self.emit(Op::This),
                "new.target" => self.emit(Op::NewTarget),
                _ => self.emit(Op::Callee),
            };
            if env {
                self.emit(Op::InitEnv(0, slot));
            } else {
                self.emit(Op::PutLocal(slot));
            }
        }
        // Parameters in TDZ.
        if !f.simple_params {
            let n = self.tree.scopes[fs as usize].bindings.len();
            for i in 0..n {
                let b = &self.tree.scopes[fs as usize].bindings[i];
                if matches!(b.kind, BindKind::Param) && !b.env {
                    let s = b.slot;
                    self.emit(Op::PushEmpty);
                    self.emit(Op::PutLocal(s));
                }
            }
        }
        // arguments object
        if let Some(bi) = self.tree.scopes[fs as usize].find("arguments") {
            let sc = &self.tree.scopes[fs as usize];
            let b = &sc.bindings[bi];
            if b.used || sc.eval_visible {
                let mapped = sc.mapped_args_possible;
                let op = if mapped {
                    // Slots of the formal parameters (in order) for the mapping.
                    // The last occurrence of a duplicated parameter name is the mapped one.
                    let mut slots = vec![u32::MAX; f.params.len()];
                    let mut seen: Vec<&str> = Vec::new();
                    for (pi, p) in f.params.iter().enumerate().rev() {
                        if let Pat::Ident(id) = p {
                            if seen.contains(&&*id.name) {
                                continue;
                            }
                            seen.push(&id.name);
                            let pb = sc.find(&id.name).unwrap();
                            slots[pi] = sc.bindings[pb].slot;
                        }
                    }
                    let k = self.konst(Const::Slots(slots));
                    Op::Arguments(k)
                } else {
                    Op::Arguments(u32::MAX)
                };
                self.emit(op);
                self.init_name("arguments");
            }
        }
        // Formal parameters.
        for (i, p) in f.params.iter().enumerate() {
            match p {
                Pat::Ident(id) => {
                    self.emit(Op::GetArg(i as u32));
                    self.init_param(&id.name);
                }
                Pat::Assign(t, d, _) => {
                    self.emit(Op::GetArg(i as u32));
                    let j = self.emit(Op::JumpIfNotUndefinedKeep(0));
                    self.emit(Op::Pop);
                    match &**t {
                        Pat::Ident(id) if d.is_anonymous_fn() => self.expr_named(d, &JsStr::from_str(&id.name)),
                        _ => self.expr(d),
                    }
                    self.patch(j);
                    self.bind_pattern(t, BindMode::Init);
                }
                _ => {
                    self.emit(Op::GetArg(i as u32));
                    self.bind_pattern(p, BindMode::Init);
                }
            }
        }
        if let Some(r) = &f.rest {
            self.emit(Op::RestArgs(f.params.len() as u32));
            self.bind_pattern(r, BindMode::Init);
        }
        // Separate variable scope.
        let (vs, bs) = self.tree.fn_scopes.get(&f.scope).copied().unwrap_or((fs, fs));
        if vs != fs {
            self.enter_scope(vs);
            let n = self.tree.scopes[vs as usize].bindings.len();
            for i in 0..n {
                let name = self.tree.scopes[vs as usize].bindings[i].name.clone();
                if self.tree.scopes[fs as usize].find(&name).is_some() && &*name != "arguments" {
                    // var with the same name as a parameter starts with the parameter's value
                    let r = self.resolve_from(fs, &name);
                    let extra = if self.tree.scopes[vs as usize].needs_env { 1 } else { 0 };
                    match r {
                        Ref::Local(s, _) => {
                            self.emit(Op::GetLocal(s));
                        }
                        Ref::Env(d, i2, _, _) => {
                            self.emit(Op::GetEnv(d + extra, i2));
                        }
                        _ => {
                            self.emit(Op::Undef);
                        }
                    }
                    self.init_name(&name);
                } else if &*name == "arguments" && self.tree.scopes[fs as usize].find("arguments").is_some() {
                    let r = self.resolve_from(fs, "arguments");
                    let extra = if self.tree.scopes[vs as usize].needs_env { 1 } else { 0 };
                    match r {
                        Ref::Local(s, _) => {
                            self.emit(Op::GetLocal(s));
                        }
                        Ref::Env(d, i2, _, _) => {
                            self.emit(Op::GetEnv(d + extra, i2));
                        }
                        _ => {
                            self.emit(Op::Undef);
                        }
                    }
                    self.init_name("arguments");
                }
            }
        }
        // Body lexical scope, then hoisted functions (closed over the lexical environment).
        self.enter_scope(bs);
        let fns = scope::top_functions(&f.body);
        self.hoist_functions(&fns);
        if f.is_generator {
            self.emit(Op::GenStart);
        }
        if f.kind == FnKind::ClassConstructor && !f.derived && self.class_has_fields {
            self.emit(Op::This);
            self.emit(Op::Callee);
            self.emit(Op::InitFields);
        }
        if let Some(e) = &f.expr_body {
            match (f.kind, self.field_name.take()) {
                (FnKind::FieldInit, Some(n)) if e.is_anonymous_fn() => self.expr_named(e, &n),
                _ => self.expr(e),
            }
            self.emit_return();
        } else {
            self.stmts(&f.body);
            self.emit(Op::Undef);
            self.emit_return();
        }
    }

    fn init_param(&mut self, name: &str) {
        self.init_name(name);
    }

    // ------------------------------------------------------------------------------------- classes

    pub(crate) fn class(&mut self, c: &Class, name: Option<JsStr>) {
        let cidx = self.tree.get(c.scope);
        let saved = self.f.scope;
        self.enter_scope(cidx);
        // Private names.
        let mut done: Vec<Atom> = Vec::new();
        for m in &c.members {
            let key = match m {
                ClassMember::Method { key, .. } | ClassMember::Field { key, .. } => Some(key),
                _ => None,
            };
            if let Some(PropKey::Private(n)) = key {
                if done.contains(n) {
                    continue;
                }
                done.push(n.clone());
                let k = self.str_const(&alloc::format!("#{}", n));
                self.emit(Op::NewPrivateName(k));
                let bn = alloc::format!("#{}", n);
                self.init_name(&bn);
            }
        }
        // The class's name (for SetFunctionName) and heritage.
        let cname = c.id.as_ref().map(|i| JsStr::from_str(&i.name)).or(name);
        match &cname {
            Some(n) => {
                let k = self.js_const(n);
                self.emit(Op::Const(k));
            }
            None => {
                let k = self.str_const("");
                self.emit(Op::Const(k));
            }
        }
        match &c.super_class {
            Some(h) => self.expr(h),
            None => {
                self.emit(Op::PushEmpty);
            }
        }
        let has_fields = c.members.iter().any(|m| match m {
            ClassMember::Field { is_static, .. } => !*is_static,
            ClassMember::Method { key: PropKey::Private(_), is_static, .. } => !*is_static,
            _ => false,
        });
        let saved_hf = self.class_has_fields;
        self.class_has_fields = has_fields;
        let ctor_code = match &c.constructor {
            Some(f) => self.compile_function(f, cname.clone()),
            None => self.default_ctor(c.super_class.is_some(), has_fields, cname.clone().unwrap_or_else(JsStr::empty), c.span),
        };
        self.class_has_fields = saved_hf;
        let k = self.konst(Const::Code(ctor_code));
        self.emit(Op::Class(k));
        // [F proto]
        for m in &c.members {
            match m {
                ClassMember::Method { key, func, kind, is_static } => {
                    let kd: u8 = match kind {
                        MethodKind::Method => 0,
                        MethodKind::Get => 1,
                        MethodKind::Set => 2,
                    };
                    if let PropKey::Private(n) = key {
                        self.emit(Op::Over);
                        if !*is_static {
                            // home object: proto
                        }
                        self.load_private_name(n);
                        self.closure(func, Some(JsStr::from_str(&alloc::format!("#{}", n))));
                        self.emit(Op::ClassPrivateMethod(kd | if *is_static { 4 } else { 0 }));
                        self.emit(Op::Pop);
                        continue;
                    }
                    if *is_static {
                        self.emit(Op::Over);
                    } else {
                        self.emit(Op::Dup);
                    }
                    self.push_key(key);
                    self.closure(func, None);
                    self.emit(Op::DefineMethod(kd));
                    self.emit(Op::Pop);
                }
                ClassMember::Field { key, init, is_static, .. } => {
                    self.emit(Op::Over);
                    let mut flags: u8 = if *is_static { 1 } else { 0 };
                    match key {
                        PropKey::Private(n) => {
                            self.load_private_name(n);
                            flags |= 2;
                        }
                        _ => self.push_key(key),
                    }
                    match init {
                        Some(f) => {
                            let fname = match key {
                                PropKey::Computed(_) => None,
                                _ => Some(self.static_key(key)),
                            };
                            if matches!(key, PropKey::Computed(_)) && f.expr_body.as_ref().map(|e| e.is_anonymous_fn()).unwrap_or(false) {
                                flags |= 4;
                            }
                            let saved_name = core::mem::replace(&mut self.field_name, fname);
                            self.closure(f, None);
                            self.field_name = saved_name;
                        }
                        None => {
                            self.emit(Op::Undef);
                        }
                    }
                    self.emit(Op::ClassField(flags));
                    self.emit(Op::Pop);
                }
                ClassMember::StaticBlock(f) => {
                    self.emit(Op::Over);
                    self.closure(f, None);
                    self.emit(Op::ClassStaticBlock);
                    self.emit(Op::Pop);
                }
            }
        }
        self.emit(Op::Pop); // proto
        if let Some(id) = &c.id {
            self.emit(Op::Dup);
            let n = String::from(&*id.name);
            self.init_name(&n);
        }
        self.emit(Op::ClassFinish);
        self.exit_scope(cidx);
        self.f.scope = saved;
    }

    fn default_ctor(&mut self, derived: bool, has_fields: bool, name: JsStr, span: Span) -> Rc<Code> {
        let mut ops = Vec::new();
        if derived {
            ops.push(Op::SuperCallForward);
            ops.push(Op::Return);
        } else {
            if has_fields {
                ops.push(Op::This);
                ops.push(Op::Callee);
                ops.push(Op::InitFields);
            }
            ops.push(Op::Undef);
            ops.push(Op::Return);
        }
        Rc::new(Code {
            name,
            ops,
            consts: Vec::new(),
            nlocals: 0,
            nparams: 0,
            length: 0,
            kind: FnKind::ClassConstructor,
            is_async: false,
            is_generator: false,
            strict: true,
            simple_params: true,
            derived,
            has_fields,
            source: Some(SourceRef { src: self.src.clone(), start: span.start, end: span.end }),
            positions: Vec::new(),
            is_module: false,
            is_script: false,
            is_eval: false,
            module: None,
        })
    }
}
