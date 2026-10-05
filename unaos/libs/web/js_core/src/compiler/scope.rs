//! Pass 1 of compilation: build the scope tree (one entry per scope-bearing AST node), declare every binding,
//! resolve every identifier reference to find bindings captured by inner functions, and find the scopes a
//! direct `eval` or `with` can observe. Pass 2 (codegen) consults the finished tree.

use crate::ast::*;
use crate::bytecode::BindKind;
use crate::lexer::Atom;
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SKind {
    Script,
    Module,
    Eval,
    FnName,
    Function,
    /// Separate variable scope of a function with parameter expressions.
    Var,
    Block,
    Catch,
    Class,
    With,
}

#[derive(Debug)]
pub struct Binding {
    pub name: Atom,
    pub kind: BindKind,
    pub captured: bool,
    pub used: bool,
    /// Allocated in an environment record (else a frame register).
    pub env: bool,
    pub slot: u32,
}

#[derive(Debug)]
pub struct Scope {
    pub kind: SKind,
    pub parent: Option<u32>,
    /// The scope that owns the function this scope belongs to (Function / Script / Module / Eval / FnName…).
    pub func: u32,
    pub bindings: Vec<Binding>,
    /// Names may be added at run time (sloppy direct eval var scope) or come from an object (with).
    pub dynamic: bool,
    pub eval_visible: bool,
    pub needs_env: bool,
    pub strict: bool,
    pub is_arrow_fn: bool,
    /// Sloppy function with simple parameters: a mapped arguments object aliases the parameters.
    pub mapped_args_possible: bool,
    /// Parameters are in TDZ until initialised (functions with parameter expressions).
    pub param_tdz: bool,
    /// Derived constructor: `this` is a real binding, uninitialised until super().
    pub derived_this: bool,
    /// Function scope whose variables live in a separate Var scope.
    pub has_var_child: bool,
}

impl Scope {
    pub fn find(&self, name: &str) -> Option<usize> {
        self.bindings.iter().position(|b| &*b.name == name)
    }
}

pub struct ScopeTree {
    pub scopes: Vec<Scope>,
    pub by_id: Vec<u32>,
    /// Function scope id -> (variable scope index, body lexical scope index).
    pub fn_scopes: alloc::collections::BTreeMap<ScopeId, (u32, u32)>,
    /// Annex B.3.3 hoisted block functions (by AST node address).
    pub annexb: Vec<usize>,
}

impl ScopeTree {
    pub fn get(&self, id: ScopeId) -> u32 {
        self.by_id[id as usize]
    }
}

pub struct Analyzer {
    pub t: ScopeTree,
    cur: u32,
    strict: bool,
}

pub fn is_lexical_decl(s: &Stmt) -> bool {
    matches!(s, Stmt::Var(v) if v.kind != VarKind::Var) || matches!(s, Stmt::Class(_))
}

/// VarDeclaredNames of a statement list (not descending into functions).
pub fn var_names(stmts: &[Stmt], out: &mut Vec<Atom>) {
    for s in stmts {
        var_names_stmt(s, out);
    }
}

fn push_unique(out: &mut Vec<Atom>, n: Atom) {
    if !out.iter().any(|x| *x == n) {
        out.push(n);
    }
}

pub fn var_names_stmt(s: &Stmt, out: &mut Vec<Atom>) {
    let pat = |p: &Pat, out: &mut Vec<Atom>| {
        let mut ids = Vec::new();
        p.bound_names(&mut ids);
        for i in ids {
            push_unique(out, i.name);
        }
    };
    match s {
        Stmt::Var(v) if v.kind == VarKind::Var => {
            for d in &v.decls {
                pat(&d.target, out);
            }
        }
        Stmt::If(_, a, b, _) => {
            var_names_stmt(a, out);
            if let Some(b) = b {
                var_names_stmt(b, out);
            }
        }
        Stmt::Block(b) => var_names(&b.body, out),
        Stmt::For(f) => {
            if let Some(ForInit::Var(v)) = &f.init {
                if v.kind == VarKind::Var {
                    for d in &v.decls {
                        pat(&d.target, out);
                    }
                }
            }
            var_names_stmt(&f.body, out);
        }
        Stmt::ForIn(f) | Stmt::ForOf(f) => {
            match &f.left {
                ForHead::Decl(VarKind::Var, p) | ForHead::VarInit(p, _) => pat(p, out),
                _ => {}
            }
            var_names_stmt(&f.body, out);
        }
        Stmt::While(_, b, _) | Stmt::DoWhile(b, _, _) | Stmt::With(_, b, _, _) => var_names_stmt(b, out),
        Stmt::Labeled(_, b, _) => {
            if !matches!(**b, Stmt::Function(_)) {
                var_names_stmt(b, out)
            }
        }
        Stmt::Try(t) => {
            var_names(&t.block.body, out);
            if let Some(h) = &t.handler {
                var_names(&h.body.body, out);
            }
            if let Some(f) = &t.finalizer {
                var_names(&f.body, out);
            }
        }
        Stmt::Switch(sw) => {
            for c in &sw.cases {
                var_names(&c.body, out);
            }
        }
        Stmt::Export(e) => {
            if let ExportDecl::Decl(d) = &**e {
                var_names_stmt(d, out);
            }
        }
        _ => {}
    }
}

/// Top-level function declarations of a function body / script (VarScopedDeclarations that are functions).
pub fn top_functions(stmts: &[Stmt]) -> Vec<Rc<Function>> {
    let mut out: Vec<Rc<Function>> = Vec::new();
    for s in stmts {
        let f = match s {
            Stmt::Function(f) => Some(f),
            Stmt::Export(e) => match &**e {
                ExportDecl::Decl(d) => match d {
                    Stmt::Function(f) => Some(f),
                    _ => None,
                },
                _ => None,
            },
            Stmt::Labeled(_, b, _) => {
                let mut b = &**b;
                while let Stmt::Labeled(_, x, _) = b {
                    b = x;
                }
                if let Stmt::Function(f) = b {
                    Some(f)
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(f) = f {
            out.push(f.clone());
        }
    }
    out
}

/// Lexically scoped declarations directly in a block: (name, kind, is_function).
pub fn lexical_names(stmts: &[Stmt], include_functions: bool) -> Vec<(Atom, BindKind)> {
    let mut out = Vec::new();
    for s in stmts {
        lexical_names_stmt(s, include_functions, &mut out);
    }
    out
}

fn lexical_names_stmt(s: &Stmt, include_functions: bool, out: &mut Vec<(Atom, BindKind)>) {
    match s {
        Stmt::Var(v) if v.kind != VarKind::Var => {
            let mut ids = Vec::new();
            for d in &v.decls {
                d.target.bound_names(&mut ids);
            }
            for i in ids {
                out.push((i.name, if v.kind == VarKind::Const { BindKind::Const } else { BindKind::Let }));
            }
        }
        Stmt::Class(c) => {
            if let Some(id) = &c.id {
                out.push((id.name.clone(), BindKind::Class));
            }
        }
        Stmt::Function(f) if include_functions => {
            if let Some(id) = &f.id {
                if !out.iter().any(|(n, _)| *n == id.name) {
                    out.push((id.name.clone(), BindKind::Func));
                }
            }
        }
        Stmt::Labeled(_, b, _) if include_functions => lexical_names_stmt(b, include_functions, out),
        Stmt::Export(e) => match &**e {
            ExportDecl::Decl(d) => lexical_names_stmt(d, include_functions, out),
            ExportDecl::DefaultClass(c) => {
                let n: Atom = c.id.as_ref().map(|i| i.name.clone()).unwrap_or_else(|| Rc::from("*default*"));
                out.push((n, BindKind::Class));
            }
            ExportDecl::DefaultFunction(f) if include_functions => {
                let n: Atom = f.id.as_ref().map(|i| i.name.clone()).unwrap_or_else(|| Rc::from("*default*"));
                out.push((n, BindKind::Func));
            }
            ExportDecl::DefaultExpr(..) => out.push((Rc::from("*default*"), BindKind::Const)),
            _ => {}
        },
        _ => {}
    }
}

/// Block-level function declarations (for Annex B.3.3 candidates): functions directly in a block.
fn block_functions(stmts: &[Stmt]) -> Vec<&Rc<Function>> {
    let mut v = Vec::new();
    for s in stmts {
        if let Stmt::Function(f) = s {
            v.push(f);
        }
    }
    v
}

pub fn has_param_expressions(f: &Function) -> bool {
    !f.simple_params
}

impl Analyzer {
    pub fn new(scope_count: u32) -> Analyzer {
        Analyzer {
            t: ScopeTree { scopes: Vec::new(), by_id: vec![u32::MAX; scope_count as usize + 1], fn_scopes: alloc::collections::BTreeMap::new(), annexb: Vec::new() },
            cur: 0,
            strict: false,
        }
    }

    fn push(&mut self, id: Option<ScopeId>, kind: SKind, is_func: bool) -> u32 {
        let idx = self.t.scopes.len() as u32;
        let parent = if self.t.scopes.is_empty() { None } else { Some(self.cur) };
        let func = if is_func || parent.is_none() { idx } else { self.t.scopes[self.cur as usize].func };
        self.t.scopes.push(Scope {
            kind,
            parent,
            func,
            bindings: Vec::new(),
            dynamic: kind == SKind::With,
            eval_visible: false,
            needs_env: false,
            strict: self.strict,
            is_arrow_fn: false,
            mapped_args_possible: false,
            param_tdz: false,
            derived_this: false,
            has_var_child: false,
        });
        if let Some(id) = id {
            self.t.by_id[id as usize] = idx;
        }
        self.cur = idx;
        idx
    }
    fn pop(&mut self) {
        self.cur = self.t.scopes[self.cur as usize].parent.unwrap_or(0);
    }

    pub fn declare(&mut self, scope: u32, name: &Atom, kind: BindKind) {
        let s = &mut self.t.scopes[scope as usize];
        if s.find(name).is_some() {
            return;
        }
        s.bindings.push(Binding { name: name.clone(), kind, captured: false, used: false, env: false, slot: 0 });
    }

    /// Resolve a reference from the current scope (marks captures).
    pub fn reference(&mut self, name: &str) {
        let from_func = self.t.scopes[self.cur as usize].func;
        let mut s = Some(self.cur);
        while let Some(i) = s {
            let sc = &mut self.t.scopes[i as usize];
            if let Some(b) = sc.find(name) {
                if sc.func != from_func {
                    sc.bindings[b].captured = true;
                }
                sc.bindings[b].used = true;
                return;
            }
            s = sc.parent;
        }
    }

    /// A direct eval at the current scope: every enclosing binding becomes visible by name.
    fn direct_eval(&mut self) {
        let strict = self.strict;
        let mut s = Some(self.cur);
        let mut var_scope_marked = false;
        while let Some(i) = s {
            let sc = &mut self.t.scopes[i as usize];
            sc.eval_visible = true;
            if !var_scope_marked && matches!(sc.kind, SKind::Function | SKind::Var | SKind::Script | SKind::Eval) {
                // Sloppy eval may declare `var`s in the nearest variable scope (of a function with parameter
                // expressions, that is the separate Var scope, the first one met walking outwards).
                if !strict {
                    sc.dynamic = true;
                }
                var_scope_marked = true;
            }
            s = sc.parent;
        }
    }

    // ------------------------------------------------------------------------------------- programs

    pub fn program(&mut self, p: &Program, eval: bool) {
        self.strict = p.strict;
        let kind = if p.module {
            SKind::Module
        } else if eval {
            SKind::Eval
        } else {
            SKind::Script
        };
        let root = self.push(Some(p.scope), kind, true);
        match kind {
            SKind::Module => {
                // All module-level declarations live in the module environment.
                let mut vars = Vec::new();
                var_names(&p.body, &mut vars);
                for v in vars {
                    self.declare(root, &v, BindKind::Var);
                }
                for (n, k) in lexical_names(&p.body, true) {
                    self.declare(root, &n, k);
                }
                for s in &p.body {
                    if let Stmt::Import(i) = s {
                        for sp in &i.specs {
                            let id = match sp {
                                ImportSpec::Default(i) | ImportSpec::Namespace(i) | ImportSpec::Named(_, i) => i,
                            };
                            let k = if matches!(sp, ImportSpec::Namespace(_)) { BindKind::Const } else { BindKind::Import };
                            self.declare(root, &id.name, k);
                        }
                    }
                }
                let s = &mut self.t.scopes[root as usize];
                for b in s.bindings.iter_mut() {
                    b.captured = true;
                }
            }
            SKind::Eval => {
                // Strict eval: vars and functions are local to the eval. Sloppy: they go to the caller's var scope.
                if p.strict {
                    let mut vars = Vec::new();
                    var_names(&p.body, &mut vars);
                    for v in vars {
                        self.declare(root, &v, BindKind::Var);
                    }
                    for f in top_functions(&p.body) {
                        if let Some(id) = &f.id {
                            self.declare(root, &id.name, BindKind::Func);
                        }
                    }
                } else {
                    self.t.scopes[root as usize].dynamic = true;
                }
                for (n, k) in lexical_names(&p.body, false) {
                    self.declare(root, &n, k);
                }
                // Eval code is reachable by name from nested evals.
                self.t.scopes[root as usize].eval_visible = true;
            }
            _ => {
                // Script: declarations are global (object / global lexical); nothing is a slot.
            }
        }
        if kind != SKind::Script {
            self.annexb_scan(&p.body, root, p.strict);
        } else if !p.strict {
            self.annexb_scan(&p.body, root, false);
        }
        self.stmts(&p.body);
        self.pop();
    }

    /// Annex B.3.3: sloppy block-level function declarations also get a var binding in the enclosing function
    /// (when that would not conflict with a lexical declaration). Records the decision per function node.
    fn annexb_scan(&mut self, body: &[Stmt], var_scope: u32, strict: bool) {
        if strict {
            return;
        }
        let mut found: Vec<(Atom, usize)> = Vec::new();
        let top_lex: Vec<Atom> = lexical_names(body, false).into_iter().map(|(n, _)| n).collect();
        for s in body {
            annexb_collect(s, &top_lex, &mut Vec::new(), &mut found, true);
        }
        let kind = self.t.scopes[var_scope as usize].kind;
        for (name, ptr) in found {
            if kind != SKind::Script && kind != SKind::Eval {
                // B.3.2.1: parameter names and `arguments` are never hoisted in function code.
                if &*name == "arguments" {
                    continue;
                }
                if let Some(bi) = self.t.scopes[var_scope as usize].find(&name) {
                    if matches!(self.t.scopes[var_scope as usize].bindings[bi].kind, BindKind::Param) {
                        continue;
                    }
                }
                let mut ps = self.t.scopes[var_scope as usize].parent;
                let mut is_param = false;
                if kind == SKind::Var {
                    while let Some(p) = ps {
                        let sc = &self.t.scopes[p as usize];
                        if let Some(bi) = sc.find(&name) {
                            is_param = matches!(sc.bindings[bi].kind, BindKind::Param);
                        }
                        if sc.kind == SKind::Function {
                            break;
                        }
                        ps = sc.parent;
                    }
                }
                if is_param {
                    continue;
                }
                self.declare(var_scope, &name, BindKind::Var);
            }
            self.t.annexb.push(ptr);
        }
    }

    // ------------------------------------------------------------------------------------- statements

    fn stmts(&mut self, list: &[Stmt]) {
        for s in list {
            self.stmt(s);
        }
    }

    fn block_scope(&mut self, id: ScopeId, body: &[Stmt]) {
        let sc = self.push(Some(id), SKind::Block, false);
        for (n, k) in lexical_names(body, true) {
            self.declare(sc, &n, k);
        }
        self.stmts(body);
        self.pop();
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Expr(e, _) => self.expr(e),
            Stmt::Var(v) => {
                for d in &v.decls {
                    self.pat(&d.target);
                    if let Some(i) = &d.init {
                        self.expr(i);
                    }
                }
            }
            Stmt::Function(f) => {
                if let Some(id) = &f.id {
                    self.reference(&id.name);
                }
                self.function(f, false);
            }
            Stmt::Class(c) => {
                if let Some(id) = &c.id {
                    self.reference(&id.name);
                }
                self.class(c);
            }
            Stmt::Return(e, _) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            Stmt::If(t, a, b, _) => {
                self.expr(t);
                self.stmt(a);
                if let Some(b) = b {
                    self.stmt(b);
                }
            }
            Stmt::Block(b) => self.block_scope(b.scope, &b.body),
            Stmt::For(f) => {
                let sc = self.push(Some(f.scope), SKind::Block, false);
                if let Some(ForInit::Var(v)) = &f.init {
                    if v.kind != VarKind::Var {
                        let mut ids = Vec::new();
                        for d in &v.decls {
                            d.target.bound_names(&mut ids);
                        }
                        for i in ids {
                            self.declare(sc, &i.name, if v.kind == VarKind::Const { BindKind::Const } else { BindKind::Let });
                        }
                    }
                }
                match &f.init {
                    Some(ForInit::Var(v)) => {
                        for d in &v.decls {
                            self.pat(&d.target);
                            if let Some(i) = &d.init {
                                self.expr(i);
                            }
                        }
                    }
                    Some(ForInit::Expr(e)) => self.expr(e),
                    None => {}
                }
                if let Some(t) = &f.test {
                    self.expr(t);
                }
                if let Some(u) = &f.update {
                    self.expr(u);
                }
                self.stmt(&f.body);
                self.pop();
            }
            Stmt::ForIn(f) | Stmt::ForOf(f) => {
                // The head expression is evaluated in a TDZ scope holding the loop's lexical names.
                let sc = self.push(Some(f.scope), SKind::Block, false);
                if let ForHead::Decl(k, p) = &f.left {
                    if *k != VarKind::Var {
                        let mut ids = Vec::new();
                        p.bound_names(&mut ids);
                        for i in ids {
                            self.declare(sc, &i.name, if *k == VarKind::Const { BindKind::Const } else { BindKind::Let });
                        }
                    }
                }
                self.expr(&f.right);
                match &f.left {
                    ForHead::Decl(_, p) | ForHead::Target(p) => self.pat(p),
                    ForHead::VarInit(p, e) => {
                        self.pat(p);
                        self.expr(e);
                    }
                }
                self.stmt(&f.body);
                self.pop();
            }
            Stmt::While(t, b, _) => {
                self.expr(t);
                self.stmt(b);
            }
            Stmt::DoWhile(b, t, _) => {
                self.stmt(b);
                self.expr(t);
            }
            Stmt::Throw(e, _) => self.expr(e),
            Stmt::Try(t) => {
                self.block_scope(t.block.scope, &t.block.body);
                if let Some(h) = &t.handler {
                    let sc = self.push(Some(h.scope), SKind::Catch, false);
                    if let Some(p) = &h.param {
                        let mut ids = Vec::new();
                        p.bound_names(&mut ids);
                        for i in ids {
                            self.declare(sc, &i.name, BindKind::CatchParam);
                        }
                        self.pat(p);
                    }
                    self.block_scope(h.body.scope, &h.body.body);
                    self.pop();
                }
                if let Some(f) = &t.finalizer {
                    self.block_scope(f.scope, &f.body);
                }
            }
            Stmt::Switch(sw) => {
                self.expr(&sw.disc);
                let sc = self.push(Some(sw.scope), SKind::Block, false);
                for c in &sw.cases {
                    for (n, k) in lexical_names(&c.body, true) {
                        self.declare(sc, &n, k);
                    }
                }
                for c in &sw.cases {
                    if let Some(t) = &c.test {
                        self.expr(t);
                    }
                    self.stmts(&c.body);
                }
                self.pop();
            }
            Stmt::Labeled(_, b, _) => self.stmt(b),
            Stmt::With(o, b, id, _) => {
                self.expr(o);
                // Names under `with` resolve dynamically, so every enclosing binding must live in an environment.
                let mut s = Some(self.cur);
                while let Some(i) = s {
                    self.t.scopes[i as usize].eval_visible = true;
                    s = self.t.scopes[i as usize].parent;
                }
                self.push(Some(*id), SKind::With, false);
                self.stmt(b);
                self.pop();
            }
            Stmt::Export(e) => match &**e {
                ExportDecl::Decl(d) => self.stmt(d),
                ExportDecl::Named { specs, source: None, .. } => {
                    for (l, _) in specs {
                        self.reference(&l.to_rust());
                    }
                }
                ExportDecl::DefaultExpr(e, _) => self.expr(e),
                ExportDecl::DefaultFunction(f) => self.function(f, false),
                ExportDecl::DefaultClass(c) => self.class(c),
                _ => {}
            },
            Stmt::Import(_) | Stmt::Empty(_) | Stmt::Debugger(_) | Stmt::Break(..) | Stmt::Continue(..) => {}
        }
    }

    fn pat(&mut self, p: &Pat) {
        match p {
            Pat::Ident(i) => self.reference(&i.name),
            Pat::Expr(e) => self.expr(e),
            Pat::Object(props, rest, _) => {
                for pp in props {
                    if let PropKey::Computed(e) = &pp.key {
                        self.expr(e);
                    }
                    self.pat(&pp.value);
                }
                if let Some(r) = rest {
                    self.pat(r);
                }
            }
            Pat::Array(elems, rest, _) => {
                for e in elems.iter().flatten() {
                    self.pat(e);
                }
                if let Some(r) = rest {
                    self.pat(r);
                }
            }
            Pat::Assign(t, d, _) => {
                self.pat(t);
                self.expr(d);
            }
        }
    }

    fn key(&mut self, k: &PropKey) {
        if let PropKey::Computed(e) = k {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Ident(i) => self.reference(&i.name),
            Expr::This(_) => self.reference("this"),
            Expr::Template(t) => t.exprs.iter().for_each(|x| self.expr(x)),
            Expr::TaggedTemplate(tag, t, _, _) => {
                self.expr(tag);
                t.exprs.iter().for_each(|x| self.expr(x));
            }
            Expr::Array(els, _) => {
                for el in els {
                    match el {
                        ArrayElem::Expr(x) | ArrayElem::Spread(x) => self.expr(x),
                        ArrayElem::Hole => {}
                    }
                }
            }
            Expr::Object(props, _) => {
                for p in props {
                    match p {
                        Prop::KeyValue(k, v) => {
                            self.key(k);
                            self.expr(v);
                        }
                        Prop::Shorthand(i) => self.reference(&i.name),
                        Prop::Method(k, f, _) => {
                            self.key(k);
                            self.function(f, false);
                        }
                        Prop::Spread(x) | Prop::Proto(x, _) => self.expr(x),
                        Prop::CoverInit(i, d) => {
                            self.reference(&i.name);
                            self.expr(d);
                        }
                    }
                }
            }
            Expr::Function(f) | Expr::Arrow(f) => self.function(f, true),
            Expr::Class(c) => self.class(c),
            Expr::Unary(_, a, _) | Expr::Update(_, _, a, _) | Expr::Await(a, _) | Expr::Paren(a, _) | Expr::Chain(a, _) => self.expr(a),
            Expr::Binary(_, a, b, _) | Expr::Logical(_, a, b, _) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Assign(_, t, v, _) => {
                self.pat(t);
                self.expr(v);
            }
            Expr::Cond(a, b, c, _) => {
                self.expr(a);
                self.expr(b);
                self.expr(c);
            }
            Expr::Call(callee, args, optional, _) => {
                self.expr(callee);
                for a in args {
                    match a {
                        Arg::Expr(x) | Arg::Spread(x) => self.expr(x),
                    }
                }
                if !*optional {
                    if let Expr::Ident(i) = &**callee {
                        if &*i.name == "eval" {
                            self.direct_eval();
                            // Eval code may refer to the function's special bindings.
                            for n in ["this", "new.target", "%fn", "arguments"] {
                                self.reference(n);
                            }
                        }
                    }
                }
            }
            Expr::New(c, args, _) => {
                self.expr(c);
                for a in args {
                    match a {
                        Arg::Expr(x) | Arg::Spread(x) => self.expr(x),
                    }
                }
            }
            Expr::Member(o, p, _, _) => {
                self.expr(o);
                match &**p {
                    MemberProp::Computed(x) => self.expr(x),
                    MemberProp::Private(n) => self.reference(&alloc::format!("#{}", n)),
                    _ => {}
                }
            }
            Expr::SuperMember(p, _) => {
                self.reference("this");
                self.reference("%fn");
                if let MemberProp::Computed(x) = &**p {
                    self.expr(x);
                }
            }
            Expr::SuperCall(args, _) => {
                self.reference("this");
                self.reference("%fn");
                self.reference("new.target");
                for a in args {
                    match a {
                        Arg::Expr(x) | Arg::Spread(x) => self.expr(x),
                    }
                }
            }
            Expr::Seq(v, _) => v.iter().for_each(|x| self.expr(x)),
            Expr::Yield(a, _, _) => {
                if let Some(a) = a {
                    self.expr(a);
                }
            }
            Expr::NewTarget(_) => self.reference("new.target"),
            Expr::ImportCall(a, b, _) => {
                self.expr(a);
                if let Some(b) = b {
                    self.expr(b);
                }
            }
            Expr::PrivateIn(n, a, _) => {
                self.reference(&alloc::format!("#{}", n));
                self.expr(a);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------------- functions

    pub fn function(&mut self, f: &Function, is_expr: bool) {
        let saved_strict = self.strict;
        self.strict = f.strict;
        let outer = self.cur;
        // Named function expressions bind their own name in an intermediate scope.
        let named_expr = f.id.is_some() && f.kind == FnKind::Normal && is_expr;
        if named_expr {
            let sc = self.push(Some(f.name_scope), SKind::FnName, false);
            self.declare(sc, &f.id.as_ref().unwrap().name, BindKind::FnName);
        }
        let fs = self.push(Some(f.scope), SKind::Function, true);
        let arrow = f.kind == FnKind::Arrow;
        {
            let sc = &mut self.t.scopes[fs as usize];
            sc.is_arrow_fn = arrow;
            sc.mapped_args_possible = !f.strict && f.simple_params && !arrow;
            sc.param_tdz = !f.simple_params;
            sc.derived_this = f.kind == FnKind::ClassConstructor && f.derived;
            sc.has_var_child = has_param_expressions(f);
        }
        if !arrow {
            self.declare(fs, &Rc::from("this"), BindKind::Internal);
            self.declare(fs, &Rc::from("new.target"), BindKind::Internal);
            if matches!(f.kind, FnKind::Method | FnKind::Getter | FnKind::Setter | FnKind::ClassConstructor | FnKind::FieldInit | FnKind::StaticBlock) {
                self.declare(fs, &Rc::from("%fn"), BindKind::Internal);
            }
        }
        let mut pnames = Vec::new();
        for p in &f.params {
            p.bound_names(&mut pnames);
        }
        if let Some(r) = &f.rest {
            r.bound_names(&mut pnames);
        }
        for p in &pnames {
            self.declare(fs, &p.name, BindKind::Param);
        }
        let lex = lexical_names(&f.body, false);
        let top_fns = top_functions(&f.body);
        // arguments object (§10.2.11 step 15–18)
        if !arrow && !matches!(f.kind, FnKind::FieldInit | FnKind::StaticBlock) {
            let shadowed = pnames.iter().any(|p| &*p.name == "arguments")
                || (!has_param_expressions(f) && (top_fns.iter().any(|x| x.id.as_ref().map(|i| &*i.name == "arguments").unwrap_or(false))
                    || lex.iter().any(|(n, _)| &**n == "arguments")));
            if !shadowed {
                self.declare(fs, &Rc::from("arguments"), BindKind::Internal);
            }
        }
        if has_param_expressions(f) {
            self.push(None, SKind::Var, false);
        }
        // Vars and top-level functions.
        let mut vars = Vec::new();
        var_names(&f.body, &mut vars);
        let vs = self.cur;
        for v in &vars {
            self.declare(vs, v, BindKind::Var);
        }
        for tf in &top_fns {
            if let Some(id) = &tf.id {
                self.declare(vs, &id.name, BindKind::Func);
            }
        }
        self.annexb_scan(&f.body, vs, f.strict);
        // Parameters (default values see the parameter scope).
        if vs != fs {
            // Evaluate params in the function scope.
            self.cur = fs;
        }
        for p in &f.params {
            self.pat(p);
        }
        if let Some(r) = &f.rest {
            self.pat(r);
        }
        self.cur = vs;
        // Body lexical scope.
        let bs = self.push(None, SKind::Block, false);
        for (n, k) in &lex {
            self.declare(bs, n, match k {
                BindKind::Const => BindKind::Const,
                BindKind::Class => BindKind::Class,
                _ => BindKind::Let,
            });
        }
        self.t.fn_scopes.insert(f.scope, (vs, bs));
        if let Some(e) = &f.expr_body {
            self.expr(e);
        }
        self.stmts(&f.body);
        self.pop(); // body
        if vs != fs {
            self.pop();
        }
        self.pop(); // function
        if named_expr {
            self.pop();
        }
        self.cur = outer;
        self.strict = saved_strict;
    }

    fn class(&mut self, c: &Class) {
        let saved = self.strict;
        self.strict = true;
        let sc = self.push(Some(c.scope), SKind::Class, false);
        if let Some(id) = &c.id {
            self.declare(sc, &id.name, BindKind::Const);
        }
        // Private names.
        for m in &c.members {
            let key = match m {
                ClassMember::Method { key, .. } | ClassMember::Field { key, .. } => Some(key),
                _ => None,
            };
            if let Some(PropKey::Private(n)) = key {
                self.declare(sc, &Rc::from(alloc::format!("#{}", n).as_str()), BindKind::Internal);
            }
        }
        if let Some(h) = &c.super_class {
            self.expr(h);
        }
        if let Some(ctor) = &c.constructor {
            self.function(ctor, false);
        }
        for m in &c.members {
            match m {
                ClassMember::Method { key, func, .. } => {
                    self.key(key);
                    self.function(func, false);
                }
                ClassMember::Field { key, init, .. } => {
                    self.key(key);
                    if let Some(i) = init {
                        self.function(i, false);
                    }
                }
                ClassMember::StaticBlock(f) => self.function(f, false),
            }
        }
        self.pop();
        self.strict = saved;
    }
}

/// Find Annex B.3.3 candidates: function declarations in blocks (incl. switch cases) whose name, replaced by a
/// `var`, would not conflict with any enclosing lexical declaration below the variable scope.
fn annexb_collect(s: &Stmt, top_lex: &[Atom], enclosing: &mut Vec<Vec<Atom>>, found: &mut Vec<(Atom, usize)>, top: bool) {
    let consider_block = |body: &[Stmt], enclosing: &mut Vec<Vec<Atom>>, found: &mut Vec<(Atom, usize)>| {
        // Lexical names of this block other than its function declarations.
        let lex: Vec<Atom> = lexical_names(body, false).into_iter().map(|(n, _)| n).collect();
        for f in block_functions(body) {
            if f.is_async || f.is_generator {
                continue;
            }
            let name = f.id.as_ref().unwrap().name.clone();
            let conflict = top_lex.contains(&name) || enclosing.iter().any(|l| l.contains(&name)) || lex.contains(&name);
            if !conflict {
                found.push((name, Rc::as_ptr(f) as usize));
            }
        }
        enclosing.push(lex.clone());
        // Also names of block-level functions in enclosing blocks conflict (they are lexical there).
        let fnames: Vec<Atom> = block_functions(body).iter().filter_map(|f| f.id.as_ref().map(|i| i.name.clone())).collect();
        enclosing.last_mut().unwrap().extend(fnames);
        for st in body {
            annexb_collect(st, top_lex, enclosing, found, false);
        }
        enclosing.pop();
    };
    match s {
        Stmt::Block(b) => consider_block(&b.body, enclosing, found),
        Stmt::If(_, a, b, _) => {
            annexb_collect(a, top_lex, enclosing, found, false);
            if let Some(b) = b {
                annexb_collect(b, top_lex, enclosing, found, false);
            }
        }
        Stmt::For(f) => {
            let mut names = Vec::new();
            if let Some(ForInit::Var(v)) = &f.init {
                if v.kind != VarKind::Var {
                    let mut ids = Vec::new();
                    for d in &v.decls {
                        d.target.bound_names(&mut ids);
                    }
                    names = ids.into_iter().map(|i| i.name).collect();
                }
            }
            enclosing.push(names);
            annexb_collect(&f.body, top_lex, enclosing, found, false);
            enclosing.pop();
        }
        Stmt::ForIn(f) | Stmt::ForOf(f) => {
            let mut names = Vec::new();
            if let ForHead::Decl(k, p) = &f.left {
                if *k != VarKind::Var {
                    let mut ids = Vec::new();
                    p.bound_names(&mut ids);
                    names = ids.into_iter().map(|i| i.name).collect();
                }
            }
            enclosing.push(names);
            annexb_collect(&f.body, top_lex, enclosing, found, false);
            enclosing.pop();
        }
        Stmt::While(_, b, _) | Stmt::DoWhile(b, _, _) | Stmt::With(_, b, _, _) | Stmt::Labeled(_, b, _) => annexb_collect(b, top_lex, enclosing, found, false),
        Stmt::Try(t) => {
            consider_block(&t.block.body, enclosing, found);
            if let Some(h) = &t.handler {
                // A catch parameter that is a pattern conflicts; a simple one does not (B.3.4).
                let mut names = Vec::new();
                if let Some(p) = &h.param {
                    if !matches!(p, Pat::Ident(_)) {
                        let mut ids = Vec::new();
                        p.bound_names(&mut ids);
                        names = ids.into_iter().map(|i| i.name).collect();
                    }
                }
                enclosing.push(names);
                consider_block(&h.body.body, enclosing, found);
                enclosing.pop();
            }
            if let Some(f) = &t.finalizer {
                consider_block(&f.body, enclosing, found);
            }
        }
        Stmt::Switch(sw) => {
            let all: Vec<Stmt> = Vec::new();
            let _ = all;
            // The case block is one scope.
            let mut lex: Vec<Atom> = Vec::new();
            for c in &sw.cases {
                lex.extend(lexical_names(&c.body, false).into_iter().map(|(n, _)| n));
            }
            let mut fns: Vec<&Rc<Function>> = Vec::new();
            for c in &sw.cases {
                fns.extend(block_functions(&c.body));
            }
            for f in &fns {
                if f.is_async || f.is_generator {
                    continue;
                }
                let name = f.id.as_ref().unwrap().name.clone();
                let conflict = top_lex.contains(&name) || enclosing.iter().any(|l| l.contains(&name)) || lex.contains(&name);
                if !conflict {
                    found.push((name, Rc::as_ptr(f) as usize));
                }
            }
            let mut l2 = lex.clone();
            l2.extend(fns.iter().filter_map(|f| f.id.as_ref().map(|i| i.name.clone())));
            enclosing.push(l2);
            for c in &sw.cases {
                for st in &c.body {
                    annexb_collect(st, top_lex, enclosing, found, false);
                }
            }
            enclosing.pop();
        }
        _ => {
            let _ = top;
        }
    }
}
