//! Pass 2: bytecode generation. Statements leave the operand stack empty; loop state (iterators, enumerators,
//! switch discriminants) lives in registers, so break / continue / return need no stack bookkeeping, only
//! PopEnv / PopHandler / iterator closing / finally routing, all known statically from the control stack.

pub mod scope;
mod expr;
mod pattern;

use crate::ast::*;
use crate::bytecode::*;
use crate::lexer::Atom;
use crate::string::JsStr;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use scope::{Analyzer, SKind, ScopeTree};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Script,
    Module,
    /// Direct or indirect eval.
    Eval,
}

pub(crate) enum Ref {
    Local(u32, bool),
    Env(u16, u32, bool, bool),
    Dynamic,
    Global,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum JumpKind {
    Break,
    Continue,
    Return,
}

pub(crate) enum Ctl {
    /// Loop / labelled statement / switch: break & continue targets.
    Target { labels: Vec<Atom>, is_loop: bool, is_switch: bool, breaks: Vec<usize>, continues: Vec<usize>, scope: u32, labelled_only: bool },
    /// try { } catch: a handler that must be popped when jumping out.
    Handler,
    /// try { } finally: jumps out are routed through the finally block.
    Finally { kind: u32, value: u32, pending: Vec<(JumpKind, Option<Atom>, usize)>, entry_jumps: Vec<usize>, scope: u32 },
    /// for-of / for-await iterator (closed on break / return).
    Iter { iter: u32, next: u32, is_async: bool },
    /// Destructuring-in-progress handlers etc. that only need PopHandler.
    Scope(u32),
}

pub(crate) struct FnState {
    pub ops: Vec<Op>,
    pub consts: Vec<Const>,
    pub nlocals: u32,
    pub scope: u32,
    pub func_scope: u32,
    pub ctl: Vec<Ctl>,
    pub cv: Option<u32>,
    pub is_async: bool,
    pub is_generator: bool,
    pub kind: FnKind,
    pub derived: bool,
    pub pending_labels: Vec<Atom>,
    pub positions: Vec<(u32, u32)>,
    pub free_temps: Vec<u32>,
    pub strict: bool,
}

pub(crate) struct Gen {
    pub tree: ScopeTree,
    pub src: Rc<[u16]>,
    pub f: FnState,
    pub mode: Mode,
    /// The class being compiled has instance fields / private methods.
    pub class_has_fields: bool,
    /// Static name of the class field whose initialiser is being compiled (NamedEvaluation).
    pub field_name: Option<JsStr>,
    /// Module code: local names of import entries (GetImport operand = index).
    pub imports: Vec<JsStr>,
}

pub struct CompileError(pub String);

/// Compile a parsed Script.
pub fn compile_script(p: &Program) -> Rc<Code> {
    let mut a = Analyzer::new(p.scope_count);
    a.program(p, false);
    let tree = finalize(a.t);
    let mut g = Gen::new(tree, p.source.clone(), Mode::Script, p.strict);
    g.program(p);
    Rc::new(g.finish(JsStr::empty(), 0, 0, FnKind::Normal, None, false, true, false))
}

/// Compile eval code (direct or indirect). References not bound inside resolve dynamically.
pub fn compile_eval(p: &Program) -> Rc<Code> {
    let mut a = Analyzer::new(p.scope_count);
    a.program(p, true);
    let tree = finalize(a.t);
    let mut g = Gen::new(tree, p.source.clone(), Mode::Eval, p.strict);
    g.program(p);
    Rc::new(g.finish(JsStr::empty(), 0, 0, FnKind::Normal, None, false, false, true))
}

/// Compile module code.
pub fn compile_module(p: &Program) -> Rc<Code> {
    let mut a = Analyzer::new(p.scope_count);
    a.program(p, false);
    let tree = finalize(a.t);
    let mut g = Gen::new(tree, p.source.clone(), Mode::Module, true);
    g.f.is_async = p.has_top_await;
    let info = module_info(p);
    g.imports = info.imports.iter().map(|i| i.local.clone()).collect();
    g.program(p);
    let mut c = g.finish(JsStr::empty(), 0, 0, FnKind::Normal, None, true, false, false);
    c.is_async = p.has_top_await;
    c.module = Some(Rc::new(info));
    Rc::new(c)
}

/// Decide environment vs register allocation and number the environment slots.
fn finalize(mut t: ScopeTree) -> ScopeTree {
    for i in 0..t.scopes.len() {
        let mapped_args = {
            let s = &t.scopes[i];
            s.kind == SKind::Function && s.find("arguments").map(|b| s.bindings[b].used || s.eval_visible).unwrap_or(false) && s.mapped_args_possible
        };
        let s = &mut t.scopes[i];
        let all_env = s.kind == SKind::Module || s.eval_visible;
        let mut n = 0u32;
        for b in s.bindings.iter_mut() {
            b.env = all_env || b.captured || (mapped_args && matches!(b.kind, BindKind::Param));
            if b.env {
                b.slot = n;
                n += 1;
            }
        }
        s.needs_env = n > 0 || s.kind == SKind::With || (s.dynamic && matches!(s.kind, SKind::Function | SKind::Var | SKind::Eval));
    }
    t
}

impl Gen {
    fn new(tree: ScopeTree, src: Rc<[u16]>, mode: Mode, strict: bool) -> Gen {
        Gen {
            tree,
            src,
            f: FnState {
                ops: Vec::new(),
                consts: Vec::new(),
                nlocals: 0,
                scope: 0,
                func_scope: 0,
                ctl: Vec::new(),
                cv: None,
                is_async: false,
                is_generator: false,
                kind: FnKind::Normal,
                derived: false,
                pending_labels: Vec::new(),
                positions: Vec::new(),
                free_temps: Vec::new(),
                strict,
            },
            mode,
            class_has_fields: false,
            field_name: None,
            imports: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(&mut self, name: JsStr, nparams: u32, length: u32, kind: FnKind, source: Option<SourceRef>, is_module: bool, is_script: bool, is_eval: bool) -> Code {
        let f = &mut self.f;
        Code {
            name,
            ops: core::mem::take(&mut f.ops),
            consts: core::mem::take(&mut f.consts),
            nlocals: f.nlocals,
            nparams,
            length,
            kind,
            is_async: f.is_async,
            is_generator: f.is_generator,
            strict: f.strict,
            simple_params: true,
            derived: f.derived,
            has_fields: false,
            source,
            positions: core::mem::take(&mut f.positions),
            is_module,
            is_script,
            is_eval,
            module: None,
        }
    }

    // ------------------------------------------------------------------------------------- emission helpers

    pub(crate) fn emit(&mut self, op: Op) -> usize {
        self.f.ops.push(op);
        self.f.ops.len() - 1
    }
    pub(crate) fn pos(&mut self, at: u32) {
        let pc = self.f.ops.len() as u32;
        if self.f.positions.last().map(|p| p.0 != pc).unwrap_or(true) {
            self.f.positions.push((pc, at));
        }
    }
    pub(crate) fn here(&self) -> usize {
        self.f.ops.len()
    }
    pub(crate) fn patch(&mut self, at: usize) {
        let t = self.f.ops.len() as u32;
        self.patch_to(at, t);
    }
    pub(crate) fn patch_to(&mut self, at: usize, t: u32) {
        let op = &mut self.f.ops[at];
        *op = match *op {
            Op::Jump(_) => Op::Jump(t),
            Op::JumpIfFalse(_) => Op::JumpIfFalse(t),
            Op::JumpIfTrue(_) => Op::JumpIfTrue(t),
            Op::JumpIfFalseKeep(_) => Op::JumpIfFalseKeep(t),
            Op::JumpIfTrueKeep(_) => Op::JumpIfTrueKeep(t),
            Op::JumpIfNotNullishKeep(_) => Op::JumpIfNotNullishKeep(t),
            Op::JumpIfNullishUndef(_) => Op::JumpIfNullishUndef(t),
            Op::JumpIfUndefined(_) => Op::JumpIfUndefined(t),
            Op::JumpIfNotUndefinedKeep(_) => Op::JumpIfNotUndefinedKeep(t),
            Op::PushHandler(_) => Op::PushHandler(t),
            Op::IterStep(_) => Op::IterStep(t),
            Op::ForInNext(_) => Op::ForInNext(t),
            Op::GenDispatch(_) => Op::GenDispatch(t),
            Op::JumpIfNullishKeep(_) => Op::JumpIfNullishKeep(t),
            other => other,
        };
    }
    pub(crate) fn konst(&mut self, c: Const) -> u32 {
        // Deduplicate strings and numbers.
        match &c {
            Const::Str(s) => {
                for (i, x) in self.f.consts.iter().enumerate() {
                    if let Const::Str(y) = x {
                        if y == s {
                            return i as u32;
                        }
                    }
                }
            }
            Const::Num(n) => {
                for (i, x) in self.f.consts.iter().enumerate() {
                    if let Const::Num(y) = x {
                        if y.to_bits() == n.to_bits() {
                            return i as u32;
                        }
                    }
                }
            }
            _ => {}
        }
        self.f.consts.push(c);
        self.f.consts.len() as u32 - 1
    }
    pub(crate) fn str_const(&mut self, s: &str) -> u32 {
        self.konst(Const::Str(JsStr::from_str(s)))
    }
    pub(crate) fn js_const(&mut self, s: &JsStr) -> u32 {
        self.konst(Const::Str(s.clone()))
    }
    pub(crate) fn alloc_local(&mut self) -> u32 {
        let n = self.f.nlocals;
        self.f.nlocals += 1;
        n
    }
    pub(crate) fn temp(&mut self) -> u32 {
        match self.f.free_temps.pop() {
            Some(t) => t,
            None => self.alloc_local(),
        }
    }
    pub(crate) fn free_temp(&mut self, t: u32) {
        self.f.free_temps.push(t);
    }
    pub(crate) fn num(&mut self, v: f64) {
        if v.abs() < 1e9 && (v as i32) as f64 == v && !(v == 0.0 && v.is_sign_negative()) {
            self.emit(Op::Int(v as i32));
        } else {
            let k = self.konst(Const::Num(v));
            self.emit(Op::Const(k));
        }
    }

    // ------------------------------------------------------------------------------------- scopes

    /// Enter a scope: push its environment if needed, allocate registers, reset lexical bindings to TDZ.
    pub(crate) fn enter_scope(&mut self, idx: u32) {
        self.f.scope = idx;
        let needs_env = self.tree.scopes[idx as usize].needs_env;
        let kind = self.tree.scopes[idx as usize].kind;
        if needs_env && kind != SKind::With {
            let info = self.scope_info(idx);
            let k = self.konst(Const::Scope(info));
            self.emit(Op::PushEnv(k));
        }
        let n = self.tree.scopes[idx as usize].bindings.len();
        for i in 0..n {
            if !self.tree.scopes[idx as usize].bindings[i].env {
                let slot = self.alloc_local();
                self.tree.scopes[idx as usize].bindings[i].slot = slot;
                let b = &self.tree.scopes[idx as usize].bindings[i];
                if matches!(b.kind, BindKind::Let | BindKind::Const | BindKind::Class) {
                    self.emit(Op::PushEmpty);
                    self.emit(Op::PutLocal(slot));
                } else if matches!(b.kind, BindKind::Var | BindKind::Func) && kind != SKind::Function && kind != SKind::Var {
                    self.emit(Op::Undef);
                    self.emit(Op::PutLocal(slot));
                }
            }
        }
    }

    pub(crate) fn exit_scope(&mut self, idx: u32) {
        let (needs, parent) = {
            let sc = &self.tree.scopes[idx as usize];
            (sc.needs_env, sc.parent.unwrap_or(0))
        };
        if needs {
            self.emit(Op::PopEnv);
        }
        self.f.scope = parent;
    }

    pub(crate) fn scope_info(&self, idx: u32) -> Rc<ScopeInfo> {
        let sc = &self.tree.scopes[idx as usize];
        let mut names = Vec::new();
        let mut kinds = Vec::new();
        for b in &sc.bindings {
            if b.env {
                names.push(JsStr::from_str(&b.name));
                kinds.push(match b.kind {
                    BindKind::Var => BindKind::Var,
                    BindKind::Let => BindKind::Let,
                    BindKind::Const => BindKind::Const,
                    BindKind::Class => BindKind::Class,
                    BindKind::Func => BindKind::Func,
                    BindKind::Param => if sc.param_tdz { BindKind::Let } else { BindKind::Param },
                    BindKind::FnName => BindKind::FnName,
                    BindKind::CatchParam => BindKind::CatchParam,
                    BindKind::Internal => if &*b.name == "this" && sc.derived_this { BindKind::Let } else { BindKind::Internal },
                    BindKind::Import => BindKind::Import,
                });
            }
        }
        Rc::new(ScopeInfo {
            names,
            kinds,
            var_scope: (matches!(sc.kind, SKind::Function | SKind::Var) && !(sc.kind == SKind::Function && sc.has_var_child)) || (sc.kind == SKind::Eval && sc.strict),
            function: sc.kind == SKind::Function,
        })
    }

    /// Resolve a name from the current scope.
    pub(crate) fn resolve(&self, name: &str) -> Ref {
        self.resolve_from(self.f.scope, name)
    }

    pub(crate) fn resolve_from(&self, from: u32, name: &str) -> Ref {
        let mut depth: u16 = 0;
        let mut dynamic = false;
        let mut s = Some(from);
        while let Some(i) = s {
            let sc = &self.tree.scopes[i as usize];
            if let Some(bi) = sc.find(name) {
                if dynamic {
                    return Ref::Dynamic;
                }
                let b = &sc.bindings[bi];
                let tdz = matches!(b.kind, BindKind::Let | BindKind::Const | BindKind::Class)
                    || (matches!(b.kind, BindKind::Param) && sc.param_tdz)
                    || (&*b.name == "this" && sc.derived_this);
                let is_const = matches!(b.kind, BindKind::Const | BindKind::FnName | BindKind::Import);
                if b.env {
                    return Ref::Env(depth, b.slot, tdz, is_const);
                }
                return Ref::Local(b.slot, tdz);
            }
            if sc.dynamic {
                dynamic = true;
            }
            if sc.needs_env {
                depth += 1;
            }
            s = sc.parent;
        }
        if dynamic || self.mode == Mode::Eval {
            Ref::Dynamic
        } else {
            Ref::Global
        }
    }

    /// The binding kind of a resolved static name (for assignment checks).
    pub(crate) fn binding_kind(&self, name: &str) -> Option<&BindKind> {
        let mut s = Some(self.f.scope);
        while let Some(i) = s {
            let sc = &self.tree.scopes[i as usize];
            if let Some(bi) = sc.find(name) {
                return Some(&sc.bindings[bi].kind);
            }
            if sc.dynamic {
                return None;
            }
            s = sc.parent;
        }
        None
    }

    pub(crate) fn import_index_pub(&self, name: &str) -> Option<u32> {
        self.import_index(name)
    }

    fn import_index(&self, name: &str) -> Option<u32> {
        if !matches!(self.binding_kind(name), Some(BindKind::Import)) {
            return None;
        }
        self.imports.iter().position(|n| n.eq_str(name)).map(|i| i as u32)
    }

    /// Load a binding's value by name (with TDZ checks).
    pub(crate) fn load_name(&mut self, name: &str) {
        if let Some(i) = self.import_index(name) {
            self.emit(Op::GetImport(i));
            return;
        }
        match self.resolve(name) {
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
            Ref::Dynamic | Ref::Global => {
                let k = self.str_const(name);
                self.emit(Op::GetName(k));
            }
        }
    }

    /// Store the top of stack into a binding (assignment semantics; value stays on the stack).
    pub(crate) fn store_name(&mut self, name: &str) {
        if let Some(i) = self.import_index(name) {
            self.emit(Op::GetImport(i));
            self.emit(Op::Pop);
            let k = self.str_const(name);
            self.emit(Op::ThrowConst(k));
            return;
        }
        let kind_fnname = matches!(self.binding_kind(name), Some(BindKind::FnName));
        match self.resolve(name) {
            Ref::Local(slot, tdz) => {
                if kind_fnname {
                    if self.f.strict {
                        let k = self.str_const(name);
                        self.emit(Op::ThrowConst(k));
                    }
                    return;
                }
                if matches!(self.binding_kind(name), Some(BindKind::Const)) {
                    let k = self.str_const(name);
                    if tdz {
                        self.emit(Op::GetLocalChk(slot, k));
                        self.emit(Op::Pop);
                    }
                    self.emit(Op::ThrowConst(k));
                    return;
                }
                if tdz {
                    let k = self.str_const(name);
                    self.emit(Op::SetLocalChk(slot, k));
                } else {
                    self.emit(Op::SetLocal(slot));
                }
            }
            Ref::Env(d, i, tdz, is_const) => {
                if kind_fnname {
                    if self.f.strict {
                        let k = self.str_const(name);
                        self.emit(Op::ThrowConst(k));
                    }
                    return;
                }
                if is_const {
                    if tdz {
                        self.emit(Op::GetEnvChk(d, i));
                        self.emit(Op::Pop);
                    }
                    let k = self.str_const(name);
                    self.emit(Op::ThrowConst(k));
                    return;
                }
                self.emit(if tdz { Op::SetEnvChk(d, i) } else { Op::SetEnv(d, i) });
            }
            Ref::Dynamic | Ref::Global => {
                let k = self.str_const(name);
                self.emit(Op::SetName(k));
            }
        }
    }

    /// Initialise a declared binding (let/const/class/var-with-init in its own scope): pops the value.
    pub(crate) fn init_name(&mut self, name: &str) {
        match self.resolve(name) {
            Ref::Local(slot, _) => {
                self.emit(Op::PutLocal(slot));
            }
            Ref::Env(d, i, _, _) => {
                self.emit(Op::InitEnv(d, i));
            }
            Ref::Dynamic | Ref::Global => {
                let k = self.str_const(name);
                self.emit(Op::InitName(k));
            }
        }
    }

    // ------------------------------------------------------------------------------------- programs

    fn program(&mut self, p: &Program) {
        let root = self.tree.get(p.scope);
        self.f.scope = root;
        self.f.func_scope = root;
        if self.mode != Mode::Module {
            let cv = self.alloc_local();
            self.emit(Op::Undef);
            self.emit(Op::PutLocal(cv));
            self.f.cv = Some(cv);
        }
        match self.mode {
            Mode::Script => {
                let decls = self.global_decls(&p.body, p.strict);
                let k = self.konst(Const::Decls(Rc::new(decls)));
                self.emit(Op::GlobalInit(k));
                self.enter_scope(root);
            }
            Mode::Eval => {
                self.enter_scope(root);
                if !p.strict {
                    let decls = self.global_decls(&p.body, false);
                    let k = self.konst(Const::Decls(Rc::new(decls)));
                    self.emit(Op::EvalInit(k));
                } else {
                    self.hoist_functions(&scope::top_functions(&p.body));
                }
            }
            Mode::Module => {
                self.enter_scope(root);
                // Imports are resolved by the module linker; hoisted functions are instantiated in InitModule.
                self.hoist_functions(&scope::top_functions(&p.body));
                for s in &p.body {
                    if let Stmt::Export(e) = s {
                        if let ExportDecl::DefaultFunction(f) = &**e {
                            self.closure(f, None);
                            let n: &str = f.id.as_ref().map(|i| &*i.name).unwrap_or("*default*");
                            if f.id.is_none() {
                                let k = self.str_const("default");
                                self.emit(Op::Const(k));
                                self.emit(Op::SetFunctionName(0));
                            }
                            let n = String::from(n);
                            self.init_name(&n);
                        }
                    }
                }
                // Instantiation ends here; evaluation resumes the suspended body.
                self.emit(Op::GenStart);
            }
        }
        for s in &p.body {
            self.stmt(s);
        }
        if let Some(cv) = self.f.cv {
            self.emit(Op::GetLocal(cv));
        } else {
            self.emit(Op::Undef);
        }
        self.emit(Op::Return);
    }

    fn global_decls(&mut self, body: &[Stmt], strict: bool) -> Decls {
        let mut d = Decls { strict, ..Default::default() };
        let mut vars = Vec::new();
        scope::var_names(body, &mut vars);
        let fns = scope::top_functions(body);
        // Functions: the last declaration of a name wins; instantiate in source order of last occurrences.
        let mut seen: Vec<Atom> = Vec::new();
        let mut fdecls: Vec<(JsStr, u32)> = Vec::new();
        for f in fns.iter().rev() {
            let name = f.id.as_ref().unwrap().name.clone();
            if seen.contains(&name) {
                continue;
            }
            seen.push(name.clone());
            let code = self.compile_function(f, None);
            let k = self.konst(Const::Code(code));
            fdecls.push((JsStr::from_str(&name), k));
        }
        fdecls.reverse();
        d.functions = fdecls;
        for v in vars {
            if !seen.contains(&v) {
                d.var_names.push(JsStr::from_str(&v));
            }
        }
        if self.mode == Mode::Script || self.mode == Mode::Eval {
            for (n, k) in scope::lexical_names(body, false) {
                d.lex.push((JsStr::from_str(&n), matches!(k, BindKind::Const)));
            }
        }
        // Annex B.3.3 candidates in this code.
        let mut ab = Vec::new();
        collect_annexb(body, &self.tree.annexb, &mut ab);
        for n in ab {
            let js = JsStr::from_str(&n);
            if !d.annexb_funcs.contains(&js) {
                d.annexb_funcs.push(js);
            }
        }
        d
    }

    /// Instantiate hoisted function declarations of the current scope.
    pub(crate) fn hoist_functions(&mut self, fns: &[Rc<Function>]) {
        let mut seen: Vec<Atom> = Vec::new();
        for f in fns.iter().rev() {
            let name = f.id.as_ref().unwrap().name.clone();
            if seen.contains(&name) {
                continue;
            }
            seen.push(name);
        }
        // Source order, last declaration of each name wins.
        for (i, f) in fns.iter().enumerate() {
            let name = f.id.as_ref().unwrap().name.clone();
            if fns[i + 1..].iter().any(|g| g.id.as_ref().unwrap().name == name) {
                continue;
            }
            self.closure(f, None);
            self.init_name(&name);
        }
    }

    // ------------------------------------------------------------------------------------- statements

    pub(crate) fn set_cv(&mut self) {
        // value on stack -> completion register (or popped)
        match self.f.cv {
            Some(cv) => {
                self.emit(Op::PutLocal(cv));
            }
            None => {
                self.emit(Op::Pop);
            }
        }
    }
    fn cv_undefined(&mut self) {
        if let Some(cv) = self.f.cv {
            self.emit(Op::Undef);
            self.emit(Op::PutLocal(cv));
        }
    }

    pub(crate) fn stmts(&mut self, list: &[Stmt]) {
        for s in list {
            self.stmt(s);
        }
    }

    pub(crate) fn stmt(&mut self, s: &Stmt) {
        let labels = core::mem::take(&mut self.f.pending_labels);
        match s {
            Stmt::Expr(e, sp) => {
                self.pos(sp.start);
                self.expr(e);
                self.set_cv();
            }
            Stmt::Var(v) => self.var_decl(v),
            Stmt::Function(f) => {
                // Hoisted. Annex B.3.3: copy the block binding into the var binding when evaluated.
                let ptr = Rc::as_ptr(f) as usize;
                if self.tree.annexb.contains(&ptr) {
                    let name = String::from(&*f.id.as_ref().unwrap().name);
                    self.load_name(&name);
                    let parent = self.tree.scopes[self.f.scope as usize].parent.unwrap_or(0);
                    match self.resolve_from(parent, &name) {
                        Ref::Local(slot, _) => {
                            self.emit(Op::PutLocal(slot));
                        }
                        Ref::Env(d, i, _, _) => {
                            // The depth is relative to the parent scope; add the current env if it exists.
                            let extra = if self.tree.scopes[self.f.scope as usize].needs_env { 1 } else { 0 };
                            self.emit(Op::SetEnv(d + extra, i));
                            self.emit(Op::Pop);
                        }
                        Ref::Dynamic | Ref::Global => {
                            let k = self.str_const(&name);
                            self.emit(Op::BlockFnHoist(k));
                        }
                    }
                }
            }
            Stmt::Class(c) => {
                self.class(c, None);
                let name = String::from(&*c.id.as_ref().unwrap().name);
                self.init_name(&name);
            }
            Stmt::Return(e, sp) => {
                self.pos(sp.start);
                match e {
                    Some(e) => {
                        self.expr(e);
                        if self.f.is_async && self.f.is_generator {
                            self.emit(Op::Await);
                        }
                    }
                    None => {
                        self.emit(Op::Undef);
                    }
                }
                self.jump_out(JumpKind::Return, None);
            }
            Stmt::If(t, a, b, _) => {
                self.cv_undefined();
                self.expr(t);
                let j = self.emit(Op::JumpIfFalse(0));
                self.stmt(a);
                if let Some(b) = b {
                    let j2 = self.emit(Op::Jump(0));
                    self.patch(j);
                    self.stmt(b);
                    self.patch(j2);
                } else {
                    self.patch(j);
                }
            }
            Stmt::Block(b) => {
                let has_labels = !labels.is_empty();
                if has_labels {
                    self.push_target(labels, false, false, true);
                }
                self.block(b.scope, &b.body);
                if has_labels {
                    self.pop_target(None);
                }
            }
            Stmt::Empty(_) | Stmt::Debugger(_) => {
                if !labels.is_empty() {
                    // nothing to break out of
                }
            }
            Stmt::For(f) => self.for_stmt(f, labels),
            Stmt::ForIn(f) => self.for_in(f, labels),
            Stmt::ForOf(f) => self.for_of(f, labels),
            Stmt::While(t, b, _) => {
                self.cv_undefined();
                let top = self.here();
                self.push_target(labels, true, false, false);
                self.expr(t);
                let exit = self.emit(Op::JumpIfFalse(0));
                self.stmt(b);
                self.patch_continues(top as u32);
                self.emit(Op::Jump(top as u32));
                self.patch(exit);
                self.pop_target(None);
            }
            Stmt::DoWhile(b, t, _) => {
                self.cv_undefined();
                let top = self.here();
                self.push_target(labels, true, false, false);
                self.stmt(b);
                let cont = self.here() as u32;
                self.patch_continues(cont);
                self.expr(t);
                self.emit(Op::JumpIfTrue(top as u32));
                self.pop_target(None);
            }
            Stmt::Break(l, _) => self.jump_out(JumpKind::Break, l.clone()),
            Stmt::Continue(l, _) => self.jump_out(JumpKind::Continue, l.clone()),
            Stmt::Throw(e, sp) => {
                self.expr(e);
                self.pos(sp.start);
                self.emit(Op::Throw);
            }
            Stmt::Try(t) => self.try_stmt(t, labels),
            Stmt::Switch(sw) => self.switch(sw, labels),
            Stmt::Labeled(l, b, _) => {
                let mut labels = labels;
                labels.push(l.clone());
                match &**b {
                    Stmt::For(_) | Stmt::ForIn(_) | Stmt::ForOf(_) | Stmt::While(..) | Stmt::DoWhile(..) | Stmt::Labeled(..) | Stmt::Block(_) => {
                        self.f.pending_labels = labels;
                        self.stmt(b);
                    }
                    _ => {
                        self.push_target(labels, false, false, true);
                        self.stmt(b);
                        self.pop_target(None);
                    }
                }
            }
            Stmt::With(o, b, id, _) => {
                self.cv_undefined();
                self.expr(o);
                self.emit(Op::PushWith);
                let idx = self.tree.get(*id);
                let saved = self.f.scope;
                self.f.scope = idx;
                self.f.ctl.push(Ctl::Scope(idx));
                self.stmt(b);
                self.f.ctl.pop();
                self.emit(Op::PopEnv);
                self.f.scope = saved;
            }
            Stmt::Import(_) => {}
            Stmt::Export(e) => self.export(e),
        }
    }

    fn export(&mut self, e: &ExportDecl) {
        match e {
            ExportDecl::Decl(d) => self.stmt(d),
            ExportDecl::DefaultExpr(e, _) => {
                if e.is_anonymous_fn() {
                    self.expr_named(e, &JsStr::from_str("default"));
                } else {
                    self.expr(e);
                }
                self.init_name("*default*");
            }
            ExportDecl::DefaultFunction(_) => {}
            ExportDecl::DefaultClass(c) => {
                self.class(c, if c.id.is_none() { Some(JsStr::from_str("default")) } else { None });
                let n = c.id.as_ref().map(|i| String::from(&*i.name)).unwrap_or_else(|| String::from("*default*"));
                self.init_name(&n);
            }
            _ => {}
        }
    }

    pub(crate) fn block(&mut self, id: ScopeId, body: &[Stmt]) {
        let idx = self.tree.get(id);
        let saved = self.f.scope;
        self.enter_scope(idx);
        self.f.ctl.push(Ctl::Scope(idx));
        // Block-level function declarations are initialised at block entry.
        let fns: Vec<Rc<Function>> = body.iter().filter_map(|s| if let Stmt::Function(f) = s { Some(f.clone()) } else { None }).collect();
        self.hoist_functions(&fns);
        self.stmts(body);
        self.f.ctl.pop();
        self.exit_scope(idx);
        self.f.scope = saved;
    }

    fn var_decl(&mut self, v: &VarDecl) {
        for d in &v.decls {
            match (&d.target, &d.init) {
                (Pat::Ident(id), Some(init)) => {
                    if init.is_anonymous_fn() {
                        self.expr_named(init, &JsStr::from_str(&id.name));
                    } else {
                        self.expr(init);
                    }
                    if v.kind == VarKind::Var {
                        self.store_name(&id.name);
                        self.emit(Op::Pop);
                    } else {
                        self.init_name(&id.name);
                    }
                }
                (Pat::Ident(id), None) => {
                    if v.kind != VarKind::Var {
                        self.emit(Op::Undef);
                        self.init_name(&id.name);
                    }
                }
                (p, Some(init)) => {
                    self.expr(init);
                    self.bind_pattern(p, if v.kind == VarKind::Var { BindMode::Assign } else { BindMode::Init });
                }
                (_, None) => {}
            }
        }
    }

    // ------------------------------------------------------------------------------------- loops

    pub(crate) fn push_target(&mut self, labels: Vec<Atom>, is_loop: bool, is_switch: bool, labelled_only: bool) {
        let scope = self.f.scope;
        self.f.ctl.push(Ctl::Target { labels, is_loop, is_switch, breaks: Vec::new(), continues: Vec::new(), scope, labelled_only });
    }

    pub(crate) fn patch_continues(&mut self, target: u32) {
        let idx = self.f.ctl.iter().rposition(|c| matches!(c, Ctl::Target { is_loop: true, .. })).unwrap();
        if let Ctl::Target { continues, .. } = &mut self.f.ctl[idx] {
            let cs = core::mem::take(continues);
            for c in cs {
                self.patch_to(c, target);
            }
        }
    }

    pub(crate) fn pop_target(&mut self, _cont: Option<u32>) {
        let c = self.f.ctl.pop().unwrap();
        if let Ctl::Target { breaks, continues, .. } = c {
            for b in breaks {
                self.patch(b);
            }
            for c in continues {
                self.patch(c);
            }
        }
    }

    fn for_stmt(&mut self, f: &ForStmt, labels: Vec<Atom>) {
        self.cv_undefined();
        let idx = self.tree.get(f.scope);
        let saved = self.f.scope;
        let lexical = matches!(&f.init, Some(ForInit::Var(v)) if v.kind != VarKind::Var);
        self.enter_scope(idx);
        self.f.ctl.push(Ctl::Scope(idx));
        match &f.init {
            Some(ForInit::Var(v)) => self.var_decl(v),
            Some(ForInit::Expr(e)) => {
                self.expr(e);
                self.emit(Op::Pop);
            }
            None => {}
        }
        let per_iter = lexical && self.tree.scopes[idx as usize].needs_env;
        if per_iter {
            self.emit(Op::CopyEnv);
        }
        let top = self.here();
        self.push_target(labels, true, false, false);
        let exit = match &f.test {
            Some(t) => {
                self.expr(t);
                Some(self.emit(Op::JumpIfFalse(0)))
            }
            None => None,
        };
        self.stmt(&f.body);
        let cont = self.here() as u32;
        self.patch_continues(cont);
        if per_iter {
            self.emit(Op::CopyEnv);
        }
        if let Some(u) = &f.update {
            self.expr(u);
            self.emit(Op::Pop);
        }
        self.emit(Op::Jump(top as u32));
        if let Some(e) = exit {
            self.patch(e);
        }
        self.pop_target(None);
        self.f.ctl.pop();
        self.exit_scope(idx);
        self.f.scope = saved;
    }

    fn for_in(&mut self, f: &ForInStmt, labels: Vec<Atom>) {
        self.cv_undefined();
        let idx = self.tree.get(f.scope);
        let saved = self.f.scope;
        if let ForHead::VarInit(p, init) = &f.left {
            // Annex B: the initialiser is evaluated and assigned before the loop.
            if let Pat::Ident(id) = p {
                if init.is_anonymous_fn() {
                    self.expr_named(init, &JsStr::from_str(&id.name));
                } else {
                    self.expr(init);
                }
                self.store_name(&id.name);
                self.emit(Op::Pop);
            }
        }
        // The RHS is evaluated in a scope where the loop's lexical bindings are in TDZ.
        self.enter_scope(idx);
        self.expr(&f.right);
        self.exit_scope(idx);
        self.f.scope = saved;
        let en = self.temp();
        let skip = self.emit(Op::JumpIfNullishUndef(0));
        self.emit(Op::ForInStart);
        self.emit(Op::PutLocal(en));
        let top = self.here();
        self.push_target(labels, true, false, false);
        self.emit(Op::GetLocal(en));
        let done = self.emit(Op::ForInNext(0));
        self.emit(Op::Swap);
        self.emit(Op::PutLocal(en));
        self.for_body_binding(f, idx);
        let cont = self.here() as u32;
        self.patch_continues(cont);
        self.emit(Op::Jump(top as u32));
        self.patch(done);
        self.pop_target(None);
        let end = self.emit(Op::Jump(0));
        self.patch(skip);
        self.emit(Op::Pop);
        self.patch(end);
        self.free_temp(en);
    }

    /// Bind the iteration value (on the stack) to the loop head, then run the body in a fresh scope.
    fn for_body_binding(&mut self, f: &ForInStmt, idx: u32) {
        let saved = self.f.scope;
        match &f.left {
            ForHead::Decl(VarKind::Var, p) | ForHead::VarInit(p, _) => {
                self.bind_pattern(p, BindMode::Assign);
                self.stmt(&f.body);
            }
            ForHead::Decl(_, p) => {
                self.enter_scope(idx);
                self.f.ctl.push(Ctl::Scope(idx));
                self.bind_pattern(p, BindMode::Init);
                self.stmt(&f.body);
                self.f.ctl.pop();
                self.exit_scope(idx);
                self.f.scope = saved;
            }
            ForHead::Target(p) => {
                self.bind_pattern(p, BindMode::Assign);
                self.stmt(&f.body);
            }
        }
    }

    fn for_of(&mut self, f: &ForInStmt, labels: Vec<Atom>) {
        self.cv_undefined();
        let idx = self.tree.get(f.scope);
        let saved = self.f.scope;
        self.enter_scope(idx);
        self.expr(&f.right);
        self.exit_scope(idx);
        self.f.scope = saved;
        let iter = self.temp();
        let next = self.temp();
        self.emit(if f.is_await { Op::GetAsyncIterator } else { Op::GetIterator });
        self.emit(Op::PutLocal(next));
        self.emit(Op::PutLocal(iter));
        let top = self.here();
        self.push_target(labels, true, false, false);
        self.emit(Op::GetLocal(iter));
        self.emit(Op::GetLocal(next));
        let done;
        if f.is_await {
            self.emit(Op::IterNext);
            // iter next result -> await result
            self.emit(Op::Rot3);
            self.emit(Op::Pop);
            self.emit(Op::Pop);
            self.emit(Op::Await);
            self.emit(Op::Dup);
            self.emit(Op::RequireObjectCoercible);
            let not_obj = self.str_const("iterator result is not an object");
            let _ = not_obj;
            self.emit(Op::Dup);
            self.emit(Op::IterResultDone);
            let d = self.emit(Op::JumpIfTrue(0));
            self.emit(Op::IterResultValue);
            done = (d, true);
        } else {
            let d = self.emit(Op::IterStep(0));
            done = (d, false);
        }
        // Body: an abrupt completion closes the iterator.
        self.f.ctl.push(Ctl::Iter { iter, next, is_async: f.is_await });
        let h = self.emit(Op::PushHandler(0));
        self.f.ctl.push(Ctl::Handler);
        self.for_body_binding(f, idx);
        self.f.ctl.pop();
        self.emit(Op::PopHandler);
        self.f.ctl.pop();
        let cont = self.here() as u32;
        self.patch_continues(cont);
        self.emit(Op::Jump(top as u32));
        // Exception in body or binding: close quietly and rethrow.
        self.patch(h);
        self.emit(Op::GetLocal(iter));
        self.emit(Op::GetLocal(next));
        if f.is_await {
            self.emit(Op::IterCloseQuiet);
        } else {
            self.emit(Op::IterCloseQuiet);
        }
        self.emit(Op::Throw);
        // Done.
        self.patch(done.0);
        if done.1 {
            self.emit(Op::Pop);
        }
        self.pop_target(None);
        self.free_temp(iter);
        self.free_temp(next);
    }

    // ------------------------------------------------------------------------------------- jumps

    /// Emit a break / continue / return from the current position, unwinding the control stack.
    pub(crate) fn jump_out(&mut self, kind: JumpKind, label: Option<Atom>) {
        self.jump_out_from(self.f.ctl.len(), kind, label);
    }

    fn jump_out_from(&mut self, from: usize, kind: JumpKind, label: Option<Atom>) {
        // For returns the value is on the stack; park it while unwinding.
        let mut ret_tmp: Option<u32> = None;
        let mut i = from;
        let mut cur_scope = self.f.scope;
        while i > 0 {
            i -= 1;
            let found = match &self.f.ctl[i] {
                Ctl::Target { labels, is_loop, labelled_only, .. } => match (kind, &label) {
                    (JumpKind::Break, None) => !*labelled_only,
                    (JumpKind::Continue, None) => *is_loop,
                    (JumpKind::Break, Some(l)) => labels.contains(l),
                    (JumpKind::Continue, Some(l)) => *is_loop && labels.contains(l),
                    (JumpKind::Return, _) => false,
                },
                _ => false,
            };
            if found {
                let target_scope = if let Ctl::Target { scope, .. } = &self.f.ctl[i] { *scope } else { 0 };
                self.pop_envs_until(cur_scope, target_scope);
                let j = self.emit(Op::Jump(0));
                if let Ctl::Target { breaks, continues, .. } = &mut self.f.ctl[i] {
                    if kind == JumpKind::Break {
                        breaks.push(j);
                    } else {
                        continues.push(j);
                    }
                }
                return;
            }
            match &self.f.ctl[i] {
                Ctl::Handler => {
                    self.emit(Op::PopHandler);
                }
                Ctl::Iter { iter, next, is_async } => {
                    let continue_inner = kind == JumpKind::Continue && self.continue_targets_inside(i, &label);
                    if !continue_inner {
                        let (it, nx, asy) = (*iter, *next, *is_async);
                        if kind == JumpKind::Return && ret_tmp.is_none() {
                            let t = self.temp();
                            self.emit(Op::PutLocal(t));
                            ret_tmp = Some(t);
                        }
                        self.emit(Op::GetLocal(it));
                        self.emit(Op::GetLocal(nx));
                        if asy {
                            self.async_iter_close();
                        } else {
                            self.emit(Op::IterClose);
                        }
                    }
                }
                Ctl::Finally { .. } => {
                    // Route through the finally block: record the pending jump and continue after it.
                    if kind == JumpKind::Return && ret_tmp.is_none() {
                        let t = self.temp();
                        self.emit(Op::PutLocal(t));
                        ret_tmp = Some(t);
                    }
                    let (kloc, vloc, fscope) = if let Ctl::Finally { kind: k, value: v, scope, .. } = &self.f.ctl[i] { (*k, *v, *scope) } else { unreachable!() };
                    self.pop_envs_until(cur_scope, fscope);
                    let n = if let Ctl::Finally { pending, .. } = &self.f.ctl[i] { pending.len() } else { 0 };
                    if let Some(t) = ret_tmp {
                        self.emit(Op::GetLocal(t));
                        self.emit(Op::PutLocal(vloc));
                    }
                    self.emit(Op::Int(3 + n as i32));
                    self.emit(Op::PutLocal(kloc));
                    let j = self.emit(Op::Jump(0));
                    if let Ctl::Finally { pending, entry_jumps, .. } = &mut self.f.ctl[i] {
                        pending.push((kind, label.clone(), i));
                        entry_jumps.push(j);
                    }
                    if let Some(t) = ret_tmp {
                        self.free_temp(t);
                    }
                    return;
                }
                Ctl::Scope(s) => {
                    cur_scope = self.tree.scopes[*s as usize].parent.unwrap_or(0);
                    if self.tree.scopes[*s as usize].needs_env {
                        self.emit(Op::PopEnv);
                    }
                }
                Ctl::Target { .. } => {}
            }
        }
        // Return from the function.
        if kind == JumpKind::Return {
            if let Some(t) = ret_tmp {
                self.emit(Op::GetLocal(t));
                self.free_temp(t);
            }
            self.emit_return();
        }
    }

    fn continue_targets_inside(&self, iter_ctl: usize, label: &Option<Atom>) -> bool {
        // Is the continue target a loop nested inside this iterator's loop body? Then no close.
        for c in &self.f.ctl[iter_ctl + 1..] {
            if let Ctl::Target { labels, is_loop: true, .. } = c {
                match label {
                    None => return true,
                    Some(l) if labels.contains(l) => return true,
                    _ => {}
                }
            }
        }
        // The Iter ctl sits just inside its own loop's Target: continue to that loop does not close either.
        if iter_ctl > 0 {
            if let Ctl::Target { labels, is_loop: true, .. } = &self.f.ctl[iter_ctl - 1] {
                match label {
                    None => return true,
                    Some(l) => return labels.contains(l),
                }
            }
        }
        false
    }

    fn pop_envs_until(&mut self, from: u32, to: u32) {
        let _ = (from, to);
    }

    pub(crate) fn emit_return(&mut self) {
        if self.f.derived {
            // Derived constructors: the result is `this` unless an object is returned.
            self.load_this_raw();
            self.emit(Op::CheckDerivedReturn);
        }
        self.emit(Op::Return);
    }

    fn async_iter_close(&mut self) {
        // iter next -> ; calls return() and awaits it
        self.emit(Op::AsyncIterClose);
        self.emit(Op::Await);
        self.emit(Op::RequireObjectCoercibleResult);
    }

    // ------------------------------------------------------------------------------------- try / switch

    fn try_stmt(&mut self, t: &TryStmt, labels: Vec<Atom>) {
        self.cv_undefined();
        let has_labels = !labels.is_empty();
        if has_labels {
            self.push_target(labels, false, false, true);
        }
        let fin = t.finalizer.is_some();
        let (kloc, vloc) = if fin { (self.alloc_local(), self.alloc_local()) } else { (0, 0) };
        let fin_handler = if fin {
            let h = self.emit(Op::PushHandler(0));
            let scope = self.f.scope;
            self.f.ctl.push(Ctl::Finally { kind: kloc, value: vloc, pending: Vec::new(), entry_jumps: Vec::new(), scope });
            Some(h)
        } else {
            None
        };
        // try block (with catch handler)
        if let Some(h) = &t.handler {
            let ch = self.emit(Op::PushHandler(0));
            self.f.ctl.push(Ctl::Handler);
            self.block(t.block.scope, &t.block.body);
            self.f.ctl.pop();
            self.emit(Op::PopHandler);
            let over = self.emit(Op::Jump(0));
            self.patch(ch);
            // catch: exception on stack
            let cidx = self.tree.get(h.scope);
            let saved = self.f.scope;
            self.cv_undefined();
            self.enter_scope(cidx);
            self.f.ctl.push(Ctl::Scope(cidx));
            match &h.param {
                Some(p) => self.bind_pattern(p, BindMode::Init),
                None => {
                    self.emit(Op::Pop);
                }
            }
            self.block(h.body.scope, &h.body.body);
            self.f.ctl.pop();
            self.exit_scope(cidx);
            self.f.scope = saved;
            self.patch(over);
        } else {
            self.block(t.block.scope, &t.block.body);
        }
        if let (Some(fh), Some(fb)) = (fin_handler, &t.finalizer) {
            let ctl = self.f.ctl.pop().unwrap();
            self.emit(Op::PopHandler);
            // normal completion
            self.emit(Op::Int(0));
            self.emit(Op::PutLocal(kloc));
            let to_fin = self.emit(Op::Jump(0));
            // exception: kind 1
            self.patch(fh);
            self.emit(Op::PutLocal(vloc));
            self.emit(Op::Int(1));
            self.emit(Op::PutLocal(kloc));
            self.patch(to_fin);
            let (pending, entry_jumps) = match ctl {
                Ctl::Finally { pending, entry_jumps, .. } => (pending, entry_jumps),
                _ => unreachable!(),
            };
            for j in entry_jumps {
                self.patch(j);
            }
            // The finally block's own completion value does not replace the statement's (unless abrupt).
            let saved_cv = self.f.cv.map(|cv| {
                let t = self.alloc_local();
                self.emit(Op::GetLocal(cv));
                self.emit(Op::PutLocal(t));
                t
            });
            self.block(fb.scope, &fb.body);
            if let (Some(cv), Some(t)) = (self.f.cv, saved_cv) {
                self.emit(Op::GetLocal(t));
                self.emit(Op::PutLocal(cv));
            }
            // dispatch
            self.emit(Op::GetLocal(kloc));
            self.emit(Op::Int(1));
            self.emit(Op::StrictEq);
            let not_throw = self.emit(Op::JumpIfFalse(0));
            self.emit(Op::GetLocal(vloc));
            self.emit(Op::Throw);
            self.patch(not_throw);
            for (n, (kind, label, _)) in pending.into_iter().enumerate() {
                self.emit(Op::GetLocal(kloc));
                self.emit(Op::Int(3 + n as i32));
                self.emit(Op::StrictEq);
                let skip = self.emit(Op::JumpIfFalse(0));
                if kind == JumpKind::Return {
                    self.emit(Op::GetLocal(vloc));
                }
                self.jump_out(kind, label);
                self.patch(skip);
            }
        }
        if has_labels {
            self.pop_target(None);
        }
    }

    fn switch(&mut self, sw: &SwitchStmt, labels: Vec<Atom>) {
        self.cv_undefined();
        self.expr(&sw.disc);
        let d = self.temp();
        self.emit(Op::PutLocal(d));
        let idx = self.tree.get(sw.scope);
        let saved = self.f.scope;
        self.enter_scope(idx);
        self.f.ctl.push(Ctl::Scope(idx));
        let fns: Vec<Rc<Function>> = sw.cases.iter().flat_map(|c| c.body.iter()).filter_map(|s| if let Stmt::Function(f) = s { Some(f.clone()) } else { None }).collect();
        self.hoist_functions(&fns);
        self.push_target(labels, false, true, false);
        let mut jumps = Vec::new();
        for c in &sw.cases {
            if let Some(t) = &c.test {
                self.emit(Op::GetLocal(d));
                self.expr(t);
                self.emit(Op::StrictEq);
                jumps.push(Some(self.emit(Op::JumpIfTrue(0))));
            } else {
                jumps.push(None);
            }
        }
        let default_jump = self.emit(Op::Jump(0));
        let mut has_default = false;
        for (i, c) in sw.cases.iter().enumerate() {
            match jumps[i] {
                Some(j) => self.patch(j),
                None => {
                    self.patch(default_jump);
                    has_default = true;
                }
            }
            self.stmts(&c.body);
        }
        if !has_default {
            self.patch(default_jump);
        }
        self.pop_target(None);
        self.f.ctl.pop();
        self.exit_scope(idx);
        self.f.scope = saved;
        self.free_temp(d);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindMode {
    /// Assignment (var declarations, assignment patterns): store via resolution.
    Assign,
    /// Initialisation of lexical bindings / parameters.
    Init,
}

fn collect_annexb(body: &[Stmt], annexb: &[usize], out: &mut Vec<Atom>) {
    fn walk(s: &Stmt, annexb: &[usize], out: &mut Vec<Atom>, top: bool) {
        match s {
            Stmt::Function(f) => {
                if !top && annexb.contains(&(Rc::as_ptr(f) as usize)) {
                    out.push(f.id.as_ref().unwrap().name.clone());
                }
            }
            Stmt::Block(b) => b.body.iter().for_each(|x| walk(x, annexb, out, false)),
            Stmt::If(_, a, b, _) => {
                walk(a, annexb, out, false);
                if let Some(b) = b {
                    walk(b, annexb, out, false);
                }
            }
            Stmt::For(f) => walk(&f.body, annexb, out, false),
            Stmt::ForIn(f) | Stmt::ForOf(f) => walk(&f.body, annexb, out, false),
            Stmt::While(_, b, _) | Stmt::DoWhile(b, _, _) | Stmt::With(_, b, _, _) => walk(b, annexb, out, false),
            Stmt::Labeled(_, b, _) => walk(b, annexb, out, top),
            Stmt::Try(t) => {
                t.block.body.iter().for_each(|x| walk(x, annexb, out, false));
                if let Some(h) = &t.handler {
                    h.body.body.iter().for_each(|x| walk(x, annexb, out, false));
                }
                if let Some(f) = &t.finalizer {
                    f.body.iter().for_each(|x| walk(x, annexb, out, false));
                }
            }
            Stmt::Switch(sw) => sw.cases.iter().flat_map(|c| c.body.iter()).for_each(|x| walk(x, annexb, out, false)),
            _ => {}
        }
    }
    for s in body {
        walk(s, annexb, out, true);
    }
}

pub(crate) fn _unused(_: Box<()>) {
    let _ = vec![0u8];
}

/// Collect the module's import / export entries (§16.2.1.6.1 ParseModule).
fn module_info(p: &Program) -> crate::vm::module::ModuleInfo {
    use crate::vm::module::{ImportEntry, ModuleInfo};
    let mut info = ModuleInfo::default();
    let req = |info: &mut ModuleInfo, s: &JsStr, a: &[(JsStr, JsStr)]| -> usize {
        if let Some(i) = info.requests.iter().position(|(x, aa)| x == s && aa == a) {
            return i;
        }
        info.requests.push((s.clone(), a.to_vec()));
        info.requests.len() - 1
    };
    // Requests in source order (imports and re-exports).
    for s in &p.body {
        match s {
            Stmt::Import(i) => {
                let r = req(&mut info, &i.source, &i.attributes);
                for sp in &i.specs {
                    let (name, local) = match sp {
                        ImportSpec::Default(l) => (Some(JsStr::from_str("default")), l),
                        ImportSpec::Namespace(l) => (None, l),
                        ImportSpec::Named(n, l) => (Some(n.clone()), l),
                    };
                    info.imports.push(ImportEntry { request: r, import_name: name, local: JsStr::from_str(&local.name) });
                }
            }
            Stmt::Export(e) => match &**e {
                ExportDecl::Named { source: Some(src), attributes, .. } | ExportDecl::All { source: src, attributes, .. } => {
                    req(&mut info, src, attributes);
                }
                _ => {}
            },
            _ => {}
        }
    }
    for s in &p.body {
        if let Stmt::Export(e) = s {
            match &**e {
                ExportDecl::Decl(d) => {
                    let mut names = Vec::new();
                    match d {
                        Stmt::Var(v) => {
                            let mut ids = Vec::new();
                            for dd in &v.decls {
                                dd.target.bound_names(&mut ids);
                            }
                            names.extend(ids.into_iter().map(|i| i.name));
                        }
                        Stmt::Function(f) => names.push(f.id.as_ref().unwrap().name.clone()),
                        Stmt::Class(c) => names.push(c.id.as_ref().unwrap().name.clone()),
                        _ => {}
                    }
                    for n in names {
                        let js = JsStr::from_str(&n);
                        info.local_exports.push((js.clone(), js));
                    }
                }
                ExportDecl::DefaultExpr(..) => info.local_exports.push((JsStr::from_str("default"), JsStr::from_str("*default*"))),
                ExportDecl::DefaultFunction(f) => {
                    let l = f.id.as_ref().map(|i| JsStr::from_str(&i.name)).unwrap_or_else(|| JsStr::from_str("*default*"));
                    info.local_exports.push((JsStr::from_str("default"), l));
                }
                ExportDecl::DefaultClass(c) => {
                    let l = c.id.as_ref().map(|i| JsStr::from_str(&i.name)).unwrap_or_else(|| JsStr::from_str("*default*"));
                    info.local_exports.push((JsStr::from_str("default"), l));
                }
                ExportDecl::Named { specs, source: None, .. } => {
                    for (local, exported) in specs {
                        // A re-export of an imported binding becomes an indirect export.
                        match info.imports.iter().find(|i| i.local == *local) {
                            Some(ie) if ie.import_name.is_some() => {
                                let ie = ie.clone();
                                info.indirect_exports.push((exported.clone(), ie.request, ie.import_name));
                            }
                            _ => info.local_exports.push((exported.clone(), local.clone())),
                        }
                    }
                }
                ExportDecl::Named { specs, source: Some(src), attributes, .. } => {
                    let r = req(&mut info, src, attributes);
                    for (imported, exported) in specs {
                        info.indirect_exports.push((exported.clone(), r, Some(imported.clone())));
                    }
                }
                ExportDecl::All { exported, source, attributes, .. } => {
                    let r = req(&mut info, source, attributes);
                    match exported {
                        Some(n) => info.indirect_exports.push((n.clone(), r, None)),
                        None => info.star_exports.push(r),
                    }
                }
            }
        }
    }
    info
}
