//! Environment records by name (§9.1): dynamic identifier resolution (with, sloppy eval, global code),
//! GlobalDeclarationInstantiation, EvalDeclarationInstantiation, PerformEval, arguments objects.

use super::*;
use crate::bytecode::{BindKind, Const, Decls};
use super::object::PropDesc;

pub enum NameRef {
    None,
    /// Declarative binding: (env, slot)
    Slot(Obj, u32),
    /// Eval-created (deletable) binding in a var env's extra map.
    Extra(Obj),
    /// Object environment: (object, is_with)
    Object(Obj, bool),
    /// Global lexical declaration.
    GlobalLex(Obj),
}

impl Vm {
    /// ResolveBinding by walking the current environment chain.
    pub fn lookup_name(&mut self, name: &JsStr) -> JsResult<NameRef> {
        let mut e = self.frames.last().and_then(|f| f.env);
        while let Some(env) = e {
            enum Step {
                Found(NameRef),
                Obj(Obj, bool, Option<Obj>),
                Global(Obj),
                Next(Option<Obj>),
            }
            let step = match &self.heap.get(env).kind {
                Kind::Env(d) => {
                    if let Some(i) = d.info.find(name) {
                        Step::Found(NameRef::Slot(env, i as u32))
                    } else if d.extra.as_ref().map(|x| x.get(&PropertyKey::Str(name.clone())).is_some()).unwrap_or(false) {
                        Step::Found(NameRef::Extra(env))
                    } else {
                        Step::Next(d.parent)
                    }
                }
                Kind::ObjEnv(d) => Step::Obj(d.object, d.with, d.parent),
                Kind::GlobalEnv(g) => {
                    if g.lex.get(&PropertyKey::Str(name.clone())).is_some() {
                        Step::Found(NameRef::GlobalLex(env))
                    } else {
                        Step::Global(g.object)
                    }
                }
                _ => Step::Next(None),
            };
            match step {
                Step::Found(r) => return Ok(r),
                Step::Next(p) => e = p,
                Step::Obj(o, with, parent) => {
                    let key = PropertyKey::from_js(name.clone());
                    if self.has_property(o, &key)? {
                        if with {
                            // @@unscopables
                            let us = self.wk.unscopables.clone();
                            let u = self.get(o, &PropertyKey::Sym(us))?;
                            if let Value::Object(uo) = u {
                                let blocked = self.get(uo, &key)?;
                                if self.to_boolean(&blocked) {
                                    e = parent;
                                    continue;
                                }
                            }
                        }
                        return Ok(NameRef::Object(o, with));
                    }
                    e = parent;
                }
                Step::Global(o) => {
                    let key = PropertyKey::from_js(name.clone());
                    if self.has_property(o, &key)? {
                        return Ok(NameRef::Object(o, false));
                    }
                    return Ok(NameRef::None);
                }
            }
        }
        Ok(NameRef::None)
    }

    pub fn get_name_ref(&mut self, name: &JsStr, r: NameRef) -> JsResult<Value> {
        match r {
            NameRef::None => Err(self.not_defined(name)),
            NameRef::Slot(e, i) => {
                let v = self.env_slot(e, i);
                if v.is_empty() {
                    return Err(self.tdz_error(&name.to_rust()));
                }
                Ok(v)
            }
            NameRef::Extra(e) => {
                let k = PropertyKey::Str(name.clone());
                match &self.heap.get(e).kind {
                    Kind::Env(d) => match d.extra.as_ref().and_then(|x| x.get(&k)) {
                        Some(Prop { slot: Slot::Data(v), .. }) => Ok(v.clone()),
                        _ => Ok(Value::Undefined),
                    },
                    _ => Ok(Value::Undefined),
                }
            }
            NameRef::Object(o, with) => {
                let key = PropertyKey::from_js(name.clone());
                if with {
                    // GetBindingValue of an object record: the binding may have disappeared (strict: error).
                    if !self.has_property(o, &key)? {
                        let strict = self.frames.last().map(|f| f.code.strict).unwrap_or(false);
                        if strict {
                            return Err(self.not_defined(name));
                        }
                        return Ok(Value::Undefined);
                    }
                }
                self.get(o, &key)
            }
            NameRef::GlobalLex(g) => {
                let k = PropertyKey::Str(name.clone());
                let v = match &self.heap.get(g).kind {
                    Kind::GlobalEnv(d) => match d.lex.get(&k) {
                        Some(Prop { slot: Slot::Data(v), .. }) => v.clone(),
                        _ => Value::Undefined,
                    },
                    _ => Value::Undefined,
                };
                if v.is_empty() {
                    return Err(self.tdz_error(&name.to_rust()));
                }
                Ok(v)
            }
        }
    }

    /// Reference token for `name`: the environment record that holds it (Undefined = unresolvable).
    pub fn resolve_ref(&mut self, name: &JsStr) -> JsResult<Value> {
        let mut e = self.frames.last().and_then(|f| f.env);
        while let Some(env) = e {
            match self.lookup_in_env(env, name)? {
                Some(true) => return Ok(Value::Object(env)),
                Some(false) => e = self.env_parent(env),
                None => return Ok(Value::Undefined),
            }
        }
        Ok(Value::Undefined)
    }

    /// Does this single environment record hold `name`? Some(false) = look further; None = end of chain.
    fn lookup_in_env(&mut self, env: Obj, name: &JsStr) -> JsResult<Option<bool>> {
        enum S {
            Decl(bool),
            Obj(Obj, bool),
            Global(Obj, bool),
        }
        let s = match &self.heap.get(env).kind {
            Kind::Env(d) => S::Decl(d.info.find(name).is_some() || d.extra.as_ref().map(|x| x.get(&PropertyKey::Str(name.clone())).is_some()).unwrap_or(false)),
            Kind::ObjEnv(d) => S::Obj(d.object, d.with),
            Kind::GlobalEnv(g) => S::Global(g.object, g.lex.get(&PropertyKey::Str(name.clone())).is_some()),
            _ => return Ok(None),
        };
        match s {
            S::Decl(b) => Ok(Some(b)),
            S::Global(o, lex) => {
                if lex {
                    return Ok(Some(true));
                }
                let key = PropertyKey::from_js(name.clone());
                if self.has_property(o, &key)? {
                    Ok(Some(true))
                } else {
                    Ok(None)
                }
            }
            S::Obj(o, with) => {
                let key = PropertyKey::from_js(name.clone());
                if !self.has_property(o, &key)? {
                    return Ok(Some(false));
                }
                if with {
                    let us = self.wk.unscopables.clone();
                    let u = self.get(o, &PropertyKey::Sym(us))?;
                    if let Value::Object(uo) = u {
                        let blocked = self.get(uo, &key)?;
                        if self.to_boolean(&blocked) {
                            return Ok(Some(false));
                        }
                    }
                }
                Ok(Some(true))
            }
        }
    }

    /// The NameRef of `name` within one specific environment record (from a reference token).
    fn ref_in_env(&self, env: Obj, name: &JsStr) -> NameRef {
        match &self.heap.get(env).kind {
            Kind::Env(d) => match d.info.find(name) {
                Some(i) => NameRef::Slot(env, i as u32),
                None => NameRef::Extra(env),
            },
            Kind::ObjEnv(d) => NameRef::Object(d.object, d.with),
            Kind::GlobalEnv(g) => {
                if g.lex.get(&PropertyKey::Str(name.clone())).is_some() {
                    NameRef::GlobalLex(env)
                } else {
                    NameRef::Object(g.object, false)
                }
            }
            _ => NameRef::None,
        }
    }

    pub fn get_ref(&mut self, token: &Value, name: &JsStr) -> JsResult<Value> {
        match token {
            Value::Object(env) => {
                let r = self.ref_in_env(*env, name);
                self.get_name_ref(name, r)
            }
            _ => Err(self.not_defined(name)),
        }
    }

    pub fn put_ref(&mut self, token: &Value, name: &JsStr, v: Value, strict: bool) -> JsResult<()> {
        let env = match token {
            Value::Object(e) => *e,
            _ => {
                if strict {
                    return Err(self.not_defined(name));
                }
                let g = self.realm().global;
                self.set(g, PropertyKey::from_js(name.clone()), v, &Value::Object(g))?;
                return Ok(());
            }
        };
        match self.ref_in_env(env, name) {
            NameRef::Object(o, _) => {
                let key = PropertyKey::from_js(name.clone());
                if strict && !self.has_property(o, &key)? {
                    return Err(self.not_defined(name));
                }
                let ok = self.set(o, key, v, &Value::Object(o))?;
                if !ok && strict {
                    return self.throw_type(&alloc::format!("Cannot assign to read only property '{}'", name));
                }
                Ok(())
            }
            _ => {
                // Declarative bindings never disappear (except eval vars): assign through the normal path.
                let saved = self.frames.last().and_then(|f| f.env);
                if let Some(f) = self.frames.last_mut() {
                    f.env = Some(env);
                }
                let r = self.set_name(name, v, strict);
                if let Some(f) = self.frames.last_mut() {
                    f.env = saved;
                }
                r
            }
        }
    }

    pub fn get_name(&mut self, name: &JsStr) -> JsResult<Value> {
        let r = self.lookup_name(name)?;
        if let NameRef::None = r {
            if name.eq_str("this") {
                let r = self.frames.last().map(|f| f.realm).unwrap_or(self.cur_realm);
                let is_module = self.frames.iter().rev().any(|f| f.code.is_module);
                if is_module {
                    return Ok(Value::Undefined);
                }
                return Ok(self.realms[r as usize].global_this.clone());
            }
        }
        self.get_name_ref(name, r)
    }

    pub fn set_name(&mut self, name: &JsStr, v: Value, strict: bool) -> JsResult<()> {
        let r = self.lookup_name(name)?;
        match r {
            NameRef::None => {
                if strict {
                    return Err(self.not_defined(name));
                }
                let g = self.realm().global;
                self.set(g, PropertyKey::from_js(name.clone()), v, &Value::Object(g))?;
                Ok(())
            }
            NameRef::Slot(e, i) => {
                let (kind_const, kind_fnname) = match &self.heap.get(e).kind {
                    Kind::Env(d) => (matches!(d.info.kinds[i as usize], BindKind::Const | BindKind::Import), matches!(d.info.kinds[i as usize], BindKind::FnName)),
                    _ => (false, false),
                };
                if self.env_slot(e, i).is_empty() {
                    return Err(self.tdz_error(&name.to_rust()));
                }
                if kind_fnname {
                    if strict {
                        return self.throw_type(&alloc::format!("Assignment to constant variable '{}'", name));
                    }
                    return Ok(());
                }
                if kind_const {
                    return self.throw_type(&alloc::format!("Assignment to constant variable '{}'", name));
                }
                self.set_env_slot(e, i, v);
                Ok(())
            }
            NameRef::Extra(e) => {
                if let Kind::Env(d) = &mut self.heap.get_mut(e).kind {
                    if let Some(x) = &mut d.extra {
                        x.insert(PropertyKey::Str(name.clone()), Prop::data(v, W | C));
                    }
                }
                Ok(())
            }
            NameRef::Object(o, with) => {
                let key = PropertyKey::from_js(name.clone());
                if with || strict {
                    let still = self.has_property(o, &key)?;
                    if !still && strict {
                        return Err(self.not_defined(name));
                    }
                }
                let ok = self.set(o, key, v, &Value::Object(o))?;
                if !ok && strict {
                    return self.throw_type(&alloc::format!("Cannot assign to read only property '{}'", name));
                }
                Ok(())
            }
            NameRef::GlobalLex(g) => {
                let k = PropertyKey::Str(name.clone());
                let (cur, writable) = match &self.heap.get(g).kind {
                    Kind::GlobalEnv(d) => match d.lex.get(&k) {
                        Some(p) => (if let Slot::Data(v) = &p.slot { v.clone() } else { Value::Undefined }, p.writable()),
                        None => (Value::Undefined, true),
                    },
                    _ => (Value::Undefined, true),
                };
                if cur.is_empty() {
                    return Err(self.tdz_error(&name.to_rust()));
                }
                if !writable {
                    return self.throw_type(&alloc::format!("Assignment to constant variable '{}'", name));
                }
                if let Kind::GlobalEnv(d) = &mut self.heap.get_mut(g).kind {
                    if let Some(p) = d.lex.get_mut(&k) {
                        p.slot = Slot::Data(v);
                    }
                }
                Ok(())
            }
        }
    }

    /// InitializeBinding by name (lexical declarations of global / eval code).
    pub fn init_name(&mut self, name: &JsStr, v: Value) -> JsResult<()> {
        let mut e = self.frames.last().and_then(|f| f.env);
        while let Some(env) = e {
            match &mut self.heap.get_mut(env).kind {
                Kind::Env(d) => {
                    if let Some(i) = d.info.find(name) {
                        d.slots[i] = v;
                        return Ok(());
                    }
                    if let Some(x) = &mut d.extra {
                        let k = PropertyKey::Str(name.clone());
                        if x.get(&k).is_some() {
                            x.insert(k, Prop::data(v, W | C));
                            return Ok(());
                        }
                    }
                    e = d.parent;
                }
                Kind::ObjEnv(d) => e = d.parent,
                Kind::GlobalEnv(d) => {
                    let k = PropertyKey::Str(name.clone());
                    if let Some(p) = d.lex.get_mut(&k) {
                        p.slot = Slot::Data(v);
                        return Ok(());
                    }
                    let o = d.object;
                    self.set(o, PropertyKey::from_js(name.clone()), v, &Value::Object(o))?;
                    return Ok(());
                }
                _ => break,
            }
        }
        Ok(())
    }

    pub fn delete_name(&mut self, name: &JsStr) -> JsResult<bool> {
        match self.lookup_name(name)? {
            NameRef::None => Ok(true),
            NameRef::Slot(..) | NameRef::GlobalLex(_) => Ok(false),
            NameRef::Extra(e) => {
                if let Kind::Env(d) = &mut self.heap.get_mut(e).kind {
                    if let Some(x) = &mut d.extra {
                        x.remove(&PropertyKey::Str(name.clone()));
                    }
                }
                Ok(true)
            }
            NameRef::Object(o, _) => {
                let key = PropertyKey::from_js(name.clone());
                let r = self.delete(o, &key)?;
                if r {
                    // A deleted global var is no longer in VarNames.
                    let ge = self.realm().global_env;
                    if let Kind::GlobalEnv(g) = &mut self.heap.get_mut(ge).kind {
                        if g.object == o {
                            g.var_names.retain(|n| n != name);
                        }
                    }
                }
                Ok(r)
            }
        }
    }

    // ------------------------------------------------------------------------------------- global code

    fn global_env_parts(&self) -> (Obj, Obj) {
        let ge = self.realm().global_env;
        let go = match &self.heap.get(ge).kind {
            Kind::GlobalEnv(g) => g.object,
            _ => unreachable!(),
        };
        (ge, go)
    }

    fn has_lexical_declaration(&self, ge: Obj, n: &JsStr) -> bool {
        match &self.heap.get(ge).kind {
            Kind::GlobalEnv(g) => g.lex.get(&PropertyKey::Str(n.clone())).is_some(),
            _ => false,
        }
    }
    fn has_var_declaration(&self, ge: Obj, n: &JsStr) -> bool {
        match &self.heap.get(ge).kind {
            Kind::GlobalEnv(g) => g.var_names.contains(n),
            _ => false,
        }
    }

    /// GlobalDeclarationInstantiation (§16.1.7).
    pub fn global_init(&mut self, d: &Decls) -> JsResult<()> {
        let (ge, go) = self.global_env_parts();
        for (n, _) in &d.lex {
            if self.has_var_declaration(ge, n) || self.has_lexical_declaration(ge, n) {
                return self.throw_syntax(&alloc::format!("Identifier '{}' has already been declared", n));
            }
            // HasRestrictedGlobalProperty
            if let Some(p) = self.get_own_property(go, &PropertyKey::from_js(n.clone()))? {
                if p.configurable == Some(false) {
                    return self.throw_syntax(&alloc::format!("Identifier '{}' has already been declared", n));
                }
            }
        }
        for n in d.var_names.iter().chain(d.functions.iter().map(|(n, _)| n)) {
            if self.has_lexical_declaration(ge, n) {
                return self.throw_syntax(&alloc::format!("Identifier '{}' has already been declared", n));
            }
        }
        for (n, _) in d.functions.iter().rev() {
            if !self.can_declare_global_function(go, n)? {
                return self.throw_type(&alloc::format!("Cannot declare global function '{}'", n));
            }
        }
        for n in &d.var_names {
            if !self.can_declare_global_var(go, n)? {
                return self.throw_type(&alloc::format!("Cannot declare global variable '{}'", n));
            }
        }
        // Annex B.3.2.2
        let mut annexb_ok = Vec::new();
        for n in &d.annexb_funcs {
            if d.functions.iter().any(|(f, _)| f == n) || d.lex.iter().any(|(l, _)| l == n) {
                continue;
            }
            if !self.has_lexical_declaration(ge, n) && self.can_declare_global_var(go, n)? {
                annexb_ok.push(n.clone());
            }
        }
        for (n, is_const) in &d.lex {
            if let Kind::GlobalEnv(g) = &mut self.heap.get_mut(ge).kind {
                g.lex.insert(PropertyKey::Str(n.clone()), Prop::data(Value::Empty, if *is_const { 0 } else { W }));
            }
        }
        let fi = self.frames.len() - 1;
        for (n, k) in &d.functions {
            let code = match &self.frames[fi].code.consts[*k as usize] {
                Const::Code(c) => c.clone(),
                _ => unreachable!(),
            };
            let env = self.frames[fi].env;
            let script = self.frames[fi].script;
            let f = self.make_closure(code, env, None, script);
            self.create_global_function_binding(go, ge, n, Value::Object(f))?;
        }
        for n in d.var_names.iter().chain(annexb_ok.iter()) {
            self.create_global_var_binding(go, ge, n)?;
        }
        Ok(())
    }

    fn can_declare_global_function(&mut self, go: Obj, n: &JsStr) -> JsResult<bool> {
        match self.get_own_property(go, &PropertyKey::from_js(n.clone()))? {
            None => self.is_extensible(go),
            Some(p) => Ok(p.configurable == Some(true) || (p.is_data() && p.writable == Some(true) && p.enumerable == Some(true))),
        }
    }
    fn can_declare_global_var(&mut self, go: Obj, n: &JsStr) -> JsResult<bool> {
        if self.has_own_property(go, &PropertyKey::from_js(n.clone()))? {
            return Ok(true);
        }
        self.is_extensible(go)
    }
    fn create_global_function_binding(&mut self, go: Obj, ge: Obj, n: &JsStr, v: Value) -> JsResult<()> {
        let key = PropertyKey::from_js(n.clone());
        let existing = self.get_own_property(go, &key)?;
        let desc = match existing {
            Some(p) if p.configurable != Some(true) => PropDesc { value: Some(v.clone()), ..Default::default() },
            _ => PropDesc::data(v.clone(), true, true, false),
        };
        self.define_property_or_throw(go, key.clone(), desc)?;
        self.set_prop(go, key, v, false)?;
        if let Kind::GlobalEnv(g) = &mut self.heap.get_mut(ge).kind {
            if !g.var_names.contains(n) {
                g.var_names.push(n.clone());
            }
        }
        Ok(())
    }
    fn create_global_var_binding(&mut self, go: Obj, ge: Obj, n: &JsStr) -> JsResult<()> {
        let key = PropertyKey::from_js(n.clone());
        let has = self.has_own_property(go, &key)?;
        if !has && self.is_extensible(go)? {
            self.define_property_or_throw(go, key, PropDesc::data(Value::Undefined, true, true, false))?;
        }
        if let Kind::GlobalEnv(g) = &mut self.heap.get_mut(ge).kind {
            if !g.var_names.contains(n) {
                g.var_names.push(n.clone());
            }
        }
        Ok(())
    }

    /// Annex B.3.2.2 / B.3.2.3: assign a block function to the var-scoped binding when evaluated.
    pub fn block_fn_hoist(&mut self, name: &JsStr, v: Value) -> JsResult<()> {
        // Find the variable environment: the nearest var-scope Env or the global env.
        let mut e = self.frames.last().and_then(|f| f.env);
        while let Some(env) = e {
            match &mut self.heap.get_mut(env).kind {
                Kind::Env(d) => {
                    if d.info.var_scope {
                        if let Some(i) = d.info.find(name) {
                            d.slots[i] = v;
                            return Ok(());
                        }
                        if let Some(x) = &mut d.extra {
                            let k = PropertyKey::Str(name.clone());
                            if x.get(&k).is_some() {
                                x.insert(k, Prop::data(v, W | C));
                                return Ok(());
                            }
                        }
                        return Ok(());
                    }
                    e = d.parent;
                }
                Kind::ObjEnv(d) => e = d.parent,
                Kind::GlobalEnv(g) => {
                    if g.var_names.contains(name) {
                        let o = g.object;
                        self.set(o, PropertyKey::from_js(name.clone()), v, &Value::Object(o))?;
                    }
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------- eval

    /// PerformEval for a direct call (§19.2.1.1) from the current frame.
    pub fn direct_eval(&mut self, arg: Value) -> JsResult<Value> {
        let src = match arg {
            Value::String(s) => s,
            other => return Ok(other),
        };
        let fi = self.frames.len() - 1;
        let caller = &self.frames[fi];
        let strict_caller = caller.code.strict;
        // Context for early errors.
        let mut ec = crate::parser::EvalContext { strict: strict_caller, ..Default::default() };
        // Walk the static function context: find the nearest non-arrow function frame code kind.
        let (in_fn, new_target, super_prop, super_call, field_init) = self.eval_context_flags();
        ec.in_function = in_fn;
        ec.new_target = new_target;
        ec.super_prop = super_prop;
        ec.super_call = super_call;
        ec.in_field_init = field_init;
        ec.private_names = self.visible_private_names();
        let prog = match crate::parser::parse_eval(src.units(), &ec) {
            Ok(p) => p,
            Err(e) => return self.throw_syntax(&e.msg),
        };
        let code = crate::compiler::compile_eval(&prog);
        let env = self.frames[fi].env;
        let this = self.frames[fi].this.clone();
        let nt = self.frames[fi].new_target.clone();
        let func = self.frames[fi].func;
        let realm = self.frames[fi].realm;
        let script = self.frames[fi].script;
        self.run_eval_code(code, env, this, nt, func, realm, script, prog.strict)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_eval_code(&mut self, code: Rc<Code>, env: Option<Obj>, this: Value, nt: Value, func: Option<Obj>, realm: u32, script: Option<Obj>, strict: bool) -> JsResult<Value> {
        let _ = strict;
        let base = self.stack.len();
        self.stack.push(Value::Undefined);
        self.stack.push(this.clone());
        let nlocals = code.nlocals as usize;
        self.stack.resize(base + 2 + nlocals, Value::Undefined);
        let hb = self.handlers.len();
        if self.frames.len() + self.native_depth >= self.max_depth {
            self.stack.truncate(base);
            return self.throw_range("Maximum call stack size exceeded");
        }
        self.frames.push(Frame {
            code,
            pc: 0,
            args_base: base + 2,
            argc: 0,
            base: base + 2,
            func,
            this,
            new_target: nt,
            env,
            handler_base: hb,
            realm,
            construct: false,
            entry: true,
            coroutine: None,
            resume_kind: 0,
            script,
        });
        let saved = self.cur_realm;
        self.cur_realm = realm;
        let r = self.run();
        self.cur_realm = saved;
        self.stack.truncate(base);
        match r? {
            interp::Completion::Return(v) | interp::Completion::Suspend(v) => Ok(v),
        }
    }

    /// Flags describing the function context of the current frame for eval early errors.
    fn eval_context_flags(&self) -> (bool, bool, bool, bool, bool) {
        use crate::ast::FnKind;
        for f in self.frames.iter().rev() {
            let c = &f.code;
            if c.is_eval {
                continue;
            }
            match c.kind {
                FnKind::Arrow => continue,
                FnKind::Normal => {
                    if c.is_script || c.is_module {
                        return (false, false, false, false, false);
                    }
                    return (true, true, false, false, false);
                }
                FnKind::Method | FnKind::Getter | FnKind::Setter => return (true, true, true, false, false),
                FnKind::ClassConstructor => return (true, true, true, c.derived, false),
                FnKind::FieldInit | FnKind::StaticBlock => return (true, true, true, false, true),
            }
        }
        (false, false, false, false, false)
    }

    fn visible_private_names(&self) -> Vec<crate::lexer::Atom> {
        let mut out = Vec::new();
        let mut e = self.frames.last().and_then(|f| f.env);
        while let Some(env) = e {
            match &self.heap.get(env).kind {
                Kind::Env(d) => {
                    for n in &d.info.names {
                        let s = n.to_rust();
                        if let Some(p) = s.strip_prefix('#') {
                            out.push(Rc::from(p));
                        }
                    }
                    e = d.parent;
                }
                Kind::ObjEnv(d) => e = d.parent,
                _ => break,
            }
        }
        out
    }

    /// EvalDeclarationInstantiation (§19.2.1.3) for sloppy eval code.
    pub fn eval_init(&mut self, d: &Decls) -> JsResult<()> {
        let fi = self.frames.len() - 1;
        // The eval's own lexical env is the current env; its parent chain leads to the var env.
        let lex_env = self.frames[fi].env;
        let start = lex_env.and_then(|e| self.env_parent(e)).or(lex_env);
        // Find the variable environment.
        let mut var_env = None;
        let mut e = start;
        let mut chain_lex: Vec<Obj> = Vec::new();
        while let Some(env) = e {
            match &self.heap.get(env).kind {
                Kind::Env(dd) => {
                    if dd.info.var_scope {
                        var_env = Some(env);
                        break;
                    }
                    chain_lex.push(env);
                    e = dd.parent;
                }
                Kind::ObjEnv(dd) => e = dd.parent,
                Kind::GlobalEnv(_) => {
                    var_env = Some(env);
                    break;
                }
                _ => break,
            }
        }
        let var_env = match var_env {
            Some(v) => v,
            None => return Ok(()),
        };
        let all_vars: Vec<JsStr> = d.var_names.iter().cloned().chain(d.functions.iter().map(|(n, _)| n.clone())).collect();
        let is_global = matches!(self.heap.get(var_env).kind, Kind::GlobalEnv(_));
        if is_global {
            for n in &all_vars {
                if self.has_lexical_declaration(var_env, n) {
                    return self.throw_syntax(&alloc::format!("Identifier '{}' has already been declared", n));
                }
            }
        }
        // Lexical declarations between the eval and the var env conflict with var names.
        for env in &chain_lex {
            if let Kind::Env(dd) = &self.heap.get(*env).kind {
                for n in &all_vars {
                    if let Some(i) = dd.info.find(n) {
                        if !matches!(dd.info.kinds[i], BindKind::CatchParam | BindKind::Var | BindKind::Func | BindKind::Param) {
                            return self.throw_syntax(&alloc::format!("Identifier '{}' has already been declared", n));
                        }
                    }
                }
            }
        }
        if let Kind::Env(dd) = &self.heap.get(var_env).kind {
            let _ = dd;
        }
        let go = if is_global {
            match &self.heap.get(var_env).kind {
                Kind::GlobalEnv(g) => Some(g.object),
                _ => None,
            }
        } else {
            None
        };
        if let Some(go) = go {
            for (n, _) in d.functions.iter().rev() {
                if !self.can_declare_global_function(go, n)? {
                    return self.throw_type(&alloc::format!("Cannot declare global function '{}'", n));
                }
            }
            for n in &d.var_names {
                if !self.can_declare_global_var(go, n)? {
                    return self.throw_type(&alloc::format!("Cannot declare global variable '{}'", n));
                }
            }
        }
        // Annex B.3.2.3: hoist block functions as vars when no conflicting lexical binding.
        let mut annexb_ok = Vec::new();
        for n in &d.annexb_funcs {
            if d.functions.iter().any(|(f, _)| f == n) {
                continue;
            }
            let mut conflict = false;
            for env in &chain_lex {
                if let Kind::Env(dd) = &self.heap.get(*env).kind {
                    if let Some(i) = dd.info.find(n) {
                        if !matches!(dd.info.kinds[i], BindKind::CatchParam | BindKind::Var | BindKind::Func | BindKind::Param) {
                            conflict = true;
                        }
                    }
                }
            }
            if let Some(lex) = lex_env {
                if let Kind::Env(dd) = &self.heap.get(lex).kind {
                    if dd.info.find(n).is_some() {
                        conflict = true;
                    }
                }
            }
            if is_global && self.has_lexical_declaration(var_env, n) {
                conflict = true;
            }
            if !conflict {
                if let Some(go) = go {
                    if !self.can_declare_global_var(go, n)? {
                        continue;
                    }
                }
                annexb_ok.push(n.clone());
            }
        }
        // Functions.
        for (n, k) in &d.functions {
            let code = match &self.frames[fi].code.consts[*k as usize] {
                Const::Code(c) => c.clone(),
                _ => unreachable!(),
            };
            let script = self.frames[fi].script;
            let f = self.make_closure(code, lex_env, None, script);
            if let Some(go) = go {
                self.create_global_function_binding(go, var_env, n, Value::Object(f))?;
            } else {
                self.eval_declare_var(var_env, n, Some(Value::Object(f)));
            }
        }
        for n in d.var_names.iter().chain(annexb_ok.iter()) {
            if let Some(go) = go {
                self.create_global_var_binding(go, var_env, n)?;
            } else {
                self.eval_declare_var(var_env, n, None);
            }
        }
        Ok(())
    }

    fn eval_declare_var(&mut self, var_env: Obj, n: &JsStr, v: Option<Value>) {
        if let Kind::Env(dd) = &mut self.heap.get_mut(var_env).kind {
            if let Some(i) = dd.info.find(n) {
                if let Some(v) = v {
                    dd.slots[i] = v;
                }
                return;
            }
            let x = dd.extra.get_or_insert_with(|| Box::new(PropMap::new()));
            let k = PropertyKey::Str(n.clone());
            match v {
                Some(v) => x.insert(k, Prop::data(v, W | C)),
                None => {
                    if x.get(&k).is_none() {
                        x.insert(k, Prop::data(Value::Undefined, W | C));
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------- arguments

    pub fn create_arguments(&mut self, k: u32) -> JsResult<Obj> {
        let fi = self.frames.len() - 1;
        let f = &self.frames[fi];
        let args: Vec<Value> = self.stack[f.args_base..f.args_base + f.argc].to_vec();
        let mapped = k != u32::MAX;
        let map: Vec<Option<u32>> = if mapped {
            match &f.code.consts[k as usize] {
                Const::Slots(s) => s.iter().map(|&x| if x == u32::MAX { None } else { Some(x) }).collect(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let env = f.env;
        let callee = f.func;
        let intr = self.intr();
        let proto = intr.object_proto;
        let values_fn = intr.array_proto_values;
        let thrower = intr.throw_type_error;
        let n = args.len();
        let mut map2 = map;
        map2.truncate(n);
        let mut d = ObjectData::new(Some(proto), Kind::Arguments(Box::new(ArgsData { env: if mapped { env } else { None }, map: map2 })));
        for (i, v) in args.into_iter().enumerate() {
            d.props.insert(PropertyKey::Index(i as u32), Prop::data(v, WEC));
        }
        d.props.insert(PropertyKey::from_str("length"), Prop::data(Value::Number(n as f64), WC));
        d.props.insert(PropertyKey::Sym(self.wk.iterator.clone()), Prop::data(Value::Object(values_fn), WC));
        if mapped {
            d.props.insert(PropertyKey::from_str("callee"), Prop::data(callee.map(Value::Object).unwrap_or(Value::Undefined), WC));
        } else {
            d.props.insert(PropertyKey::from_str("callee"), Prop { slot: Slot::Accessor(Some(thrower), Some(thrower)), flags: 0 });
        }
        Ok(self.alloc(d))
    }
}
