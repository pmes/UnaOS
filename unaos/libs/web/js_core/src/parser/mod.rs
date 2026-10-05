//! ECMAScript syntactic grammar (ECMA-262 §13–§16) as a recursive-descent parser producing `ast::Program`,
//! with the static semantics' early errors (redeclarations, strict-mode restrictions, labels, private names,
//! super / new.target placement, cover grammars, regular-expression literal syntax).

mod expr;
mod stmt;

use crate::ast::*;
use crate::lexer::{Atom, LexError, Lexer, Token, P, T};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Debug)]
pub struct ParseError {
    pub pos: u32,
    pub msg: String,
}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> ParseError {
        ParseError { pos: e.pos, msg: e.msg }
    }
}

pub type PResult<T> = Result<T, ParseError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goal {
    Script,
    Module,
}

/// Context for parsing direct-eval code (§19.2.1.1 PerformEval: the early errors that depend on the caller).
#[derive(Clone, Debug, Default)]
pub struct EvalContext {
    pub strict: bool,
    pub in_function: bool,
    pub new_target: bool,
    pub super_prop: bool,
    pub super_call: bool,
    pub in_field_init: bool,
    /// Private names visible at the eval site.
    pub private_names: Vec<Atom>,
}

#[derive(Clone)]
pub(crate) struct Ctx {
    pub strict: bool,
    pub in_function: bool,
    pub yield_kw: bool,
    pub await_kw: bool,
    pub await_reserved: bool,
    pub in_params: bool,
    pub super_prop: bool,
    pub super_call: bool,
    pub new_target: bool,
    pub in_field_init: bool,
    pub in_static_block: bool,
    pub labels: Vec<(Atom, bool)>,
    pub in_iteration: bool,
    pub in_switch: bool,
    pub is_arrow: bool,
}

#[derive(Default, Clone, Copy)]
pub(crate) struct FnFlags {
    pub has_direct_eval: bool,
    pub uses_arguments: bool,
    pub uses_this: bool,
    pub uses_super: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ScopeKind {
    /// Function top level (params + body var scope).
    Function,
    /// Script top level.
    Script,
    Module,
    Block,
    Catch,
    /// The parameter list (while parsing params, before the body scope exists).
    Params,
}

pub(crate) struct DeclScope {
    pub kind: ScopeKind,
    pub lex: Vec<Atom>,
    /// Function declarations directly in this block (sloppy blocks may repeat them).
    pub block_funcs: Vec<Atom>,
    pub vars: Vec<Atom>,
    pub params: Vec<Atom>,
    /// Catch parameter names; `simple_catch` when the parameter is a plain identifier.
    pub catch_params: Vec<Atom>,
    pub simple_catch: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PrivKind {
    Field,
    Method,
    Get,
    Set,
    GetSet,
}

pub(crate) struct ClassPriv {
    pub declared: Vec<(Atom, PrivKind, bool)>,
    pub refs: Vec<(Atom, u32)>,
}

pub struct Parser {
    /// Current syntactic nesting (expressions + statements); bounded so recursion cannot exhaust the stack.
    pub(crate) depth: u32,
    pub(crate) lx: Lexer,
    pub(crate) tok: Token,
    pub(crate) prev_end: u32,
    pub(crate) goal: Goal,
    pub(crate) ctx: Ctx,
    pub(crate) flags: Vec<FnFlags>,
    pub(crate) scopes: Vec<DeclScope>,
    pub(crate) classes: Vec<ClassPriv>,
    pub(crate) scope_counter: u32,
    pub(crate) site_counter: u32,
    /// Positions of the most recent YieldExpression / AwaitExpression / `await` identifier (cover grammar checks).
    pub(crate) yield_pos: Option<u32>,
    pub(crate) await_pos: Option<u32>,
    pub(crate) await_ident_pos: Option<u32>,
    /// First CoverInitializedName (`{a = 1}`) awaiting conversion to a pattern.
    pub(crate) cover_init: Option<u32>,
    pub(crate) eval_privates: Vec<Atom>,
    pub(crate) eval_mode: bool,
    pub(crate) has_top_await: bool,
    pub(crate) array_trailing_comma_after_spread: Vec<u32>,
    pub(crate) object_trailing_comma_after_spread: Vec<u32>,
    pub(crate) async_call_trailing_comma_after_spread: Vec<u32>,
    /// Start positions of `async(` call heads with no line terminator between `async` and `(`.
    pub(crate) async_heads: Vec<u32>,
}

pub fn parse_script(src: &[u16]) -> PResult<Program> {
    Parser::new(Rc::from(src), Goal::Script).parse_program()
}

pub fn parse_module(src: &[u16]) -> PResult<Program> {
    Parser::new(Rc::from(src), Goal::Module).parse_program()
}

pub fn parse_eval(src: &[u16], ec: &EvalContext) -> PResult<Program> {
    let mut p = Parser::new(Rc::from(src), Goal::Script);
    p.ctx.strict = ec.strict;
    p.ctx.in_function = false;
    p.ctx.new_target = ec.new_target;
    p.ctx.super_prop = ec.super_prop;
    p.ctx.super_call = ec.super_call;
    p.ctx.in_field_init = ec.in_field_init;
    p.eval_privates = ec.private_names.clone();
    p.eval_mode = true;
    p.parse_program()
}

impl Parser {
    pub fn new(src: Rc<[u16]>, goal: Goal) -> Parser {
        let module = goal == Goal::Module;
        let lx = Lexer::new(src, module);
        let tok = Token { t: T::Eof, start: 0, end: 0, nl_before: false, escaped: false, legacy_octal: false };
        Parser {
            lx,
            tok,
            prev_end: 0,
            goal,
            ctx: Ctx {
                strict: module,
                in_function: false,
                yield_kw: false,
                await_kw: module,
                await_reserved: module,
                in_params: false,
                super_prop: false,
                super_call: false,
                new_target: false,
                in_field_init: false,
                in_static_block: false,
                labels: Vec::new(),
                in_iteration: false,
                in_switch: false,
                is_arrow: false,
            },
            flags: vec![FnFlags::default()],
            depth: 0,
            scopes: Vec::new(),
            classes: Vec::new(),
            scope_counter: 0,
            site_counter: 0,
            yield_pos: None,
            await_pos: None,
            await_ident_pos: None,
            cover_init: None,
            eval_privates: Vec::new(),
            eval_mode: false,
            has_top_await: false,
            array_trailing_comma_after_spread: Vec::new(),
            object_trailing_comma_after_spread: Vec::new(),
            async_call_trailing_comma_after_spread: Vec::new(),
            async_heads: Vec::new(),
        }
    }

    pub fn parse_program(mut self) -> PResult<Program> {
        self.advance()?;
        let scope = self.new_scope_id();
        let module = self.goal == Goal::Module;
        self.push_scope(if module { ScopeKind::Module } else { ScopeKind::Script });
        let mut body = Vec::new();
        if !module {
            let strict = self.directives(&mut body)?;
            if strict {
                self.ctx.strict = true;
            }
        }
        while self.tok.t != T::Eof {
            let s = if module { self.parse_module_item()? } else { self.parse_statement_list_item()? };
            body.push(s);
        }
        if module {
            self.check_module_exports(&body)?;
        }
        self.pop_scope();
        if !self.classes.is_empty() {
            return self.err_at(0, "unbalanced class");
        }
        Ok(Program {
            body,
            module,
            strict: self.ctx.strict,
            scope,
            scope_count: self.scope_counter,
            source: self.lx.src.clone(),
            has_top_await: self.has_top_await,
        })
    }

    // ------------------------------------------------------------------------------------- token plumbing

    pub(crate) fn advance(&mut self) -> PResult<Token> {
        let next = self.lx.next_token()?;
        self.prev_end = self.tok.end;
        Ok(core::mem::replace(&mut self.tok, next))
    }

    pub(crate) fn err<X>(&self, msg: &str) -> PResult<X> {
        Err(ParseError { pos: self.tok.start, msg: String::from(msg) })
    }
    pub(crate) fn err_at<X>(&self, pos: u32, msg: &str) -> PResult<X> {
        Err(ParseError { pos, msg: String::from(msg) })
    }
    pub(crate) fn unexpected<X>(&self) -> PResult<X> {
        let what = match &self.tok.t {
            T::Eof => String::from("end of input"),
            T::Name(n) => format!("'{}'", n),
            T::Punct(p) => format!("{:?}", p),
            T::Num(_) => String::from("number"),
            T::Str(_) => String::from("string"),
            _ => String::from("token"),
        };
        self.err(&format!("unexpected {}", what))
    }

    pub(crate) fn is(&self, p: P) -> bool {
        self.tok.t == T::Punct(p)
    }
    pub(crate) fn eat(&mut self, p: P) -> PResult<bool> {
        if self.is(p) {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub(crate) fn expect(&mut self, p: P) -> PResult<()> {
        if self.is(p) {
            self.advance()?;
            Ok(())
        } else {
            self.unexpected()
        }
    }
    /// Unescaped IdentifierName equal to `kw` (keywords and contextual keywords).
    pub(crate) fn is_kw(&self, kw: &str) -> bool {
        matches!(&self.tok.t, T::Name(n) if &**n == kw) && !self.tok.escaped
    }
    /// Name equal to `kw`, escaped or not.
    pub(crate) fn is_name_any(&self, kw: &str) -> bool {
        matches!(&self.tok.t, T::Name(n) if &**n == kw)
    }
    pub(crate) fn eat_kw(&mut self, kw: &str) -> PResult<bool> {
        if self.is_kw(kw) {
            self.advance()?;
            Ok(true)
        } else {
            if self.is_name_any(kw) && is_keyword(kw) {
                return self.err("keyword must not contain escapes");
            }
            Ok(false)
        }
    }
    pub(crate) fn expect_kw(&mut self, kw: &str) -> PResult<()> {
        if self.eat_kw(kw)? {
            Ok(())
        } else {
            self.unexpected()
        }
    }

    /// Automatic semicolon insertion (§12.10).
    pub(crate) fn semicolon(&mut self) -> PResult<()> {
        if self.eat(P::Semi)? {
            return Ok(());
        }
        if self.is(P::RBrace) || self.tok.t == T::Eof || self.tok.nl_before {
            return Ok(());
        }
        self.unexpected()
    }

    /// Lookahead one token past the current one (re-lexes; no goal-dependent tokens are involved).
    pub(crate) fn peek(&mut self) -> PResult<Token> {
        let save_pos = self.lx.pos;
        let save_nl = self.lx.nl_before;
        let t = self.lx.next_token();
        self.lx.pos = save_pos;
        self.lx.nl_before = save_nl;
        Ok(t?)
    }

    pub(crate) fn span_from(&self, start: u32) -> Span {
        Span { start, end: self.prev_end }
    }

    pub(crate) fn new_scope_id(&mut self) -> ScopeId {
        let id = self.scope_counter;
        self.scope_counter += 1;
        id
    }

    // ------------------------------------------------------------------------------------- function flags

    pub(crate) fn flag(&mut self) -> &mut FnFlags {
        self.flags.last_mut().unwrap()
    }

    // ------------------------------------------------------------------------------------- declarations

    pub(crate) fn push_scope(&mut self, kind: ScopeKind) {
        self.scopes.push(DeclScope {
            kind,
            lex: Vec::new(),
            block_funcs: Vec::new(),
            vars: Vec::new(),
            params: Vec::new(),
            catch_params: Vec::new(),
            simple_catch: false,
        });
    }
    pub(crate) fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Declare a lexically scoped name (let / const / class, block-level function, module function).
    pub(crate) fn declare_lex(&mut self, name: &Atom, pos: u32, is_func: bool) -> PResult<()> {
        let strict = self.ctx.strict;
        let sc = self.scopes.last_mut().unwrap();
        if sc.lex.contains(name) {
            // Annex B.3.2.4: sloppy duplicate function declarations in a block are allowed.
            let dup_fn_ok = is_func && !strict && sc.block_funcs.contains(name) && sc.kind == ScopeKind::Block;
            if !dup_fn_ok {
                return self.err_at(pos, &format!("redeclaration of '{}'", name));
            }
        }
        if sc.vars.contains(name) || sc.params.contains(name) || sc.catch_params.contains(name) {
            return self.err_at(pos, &format!("redeclaration of '{}'", name));
        }
        if is_func && sc.kind == ScopeKind::Block {
            sc.block_funcs.push(name.clone());
        } else if sc.block_funcs.contains(name) {
            return self.err_at(pos, &format!("redeclaration of '{}'", name));
        }
        sc.lex.push(name.clone());
        Ok(())
    }

    /// Declare a var-scoped name; checks every enclosing block up to the var scope.
    pub(crate) fn declare_var(&mut self, name: &Atom, pos: u32, for_of: bool) -> PResult<()> {
        let n = self.scopes.len();
        for i in (0..n).rev() {
            let sc = &mut self.scopes[i];
            let top = matches!(sc.kind, ScopeKind::Function | ScopeKind::Script | ScopeKind::Module);
            if sc.lex.contains(name) && !(top && sc.kind != ScopeKind::Module && sc.block_funcs.is_empty() && false) {
                // At function/script top level, function declarations are var-scoped (recorded in vars).
                return self.err_at(pos, &format!("redeclaration of '{}'", name));
            }
            let _ = for_of;
            if sc.kind == ScopeKind::Catch && sc.catch_params.contains(name) && !sc.simple_catch {
                return self.err_at(pos, &format!("redeclaration of catch parameter '{}'", name));
            }
            if !sc.vars.contains(name) {
                sc.vars.push(name.clone());
            }
            if top {
                break;
            }
        }
        Ok(())
    }

    /// A function declaration at function / script top level is var-scoped; in a module top level it is lexical.
    pub(crate) fn declare_function(&mut self, name: &Atom, pos: u32) -> PResult<()> {
        let kind = self.scopes.last().unwrap().kind;
        match kind {
            ScopeKind::Function | ScopeKind::Script => {
                let sc = self.scopes.last_mut().unwrap();
                if sc.lex.contains(name) {
                    return self.err_at(pos, &format!("redeclaration of '{}'", name));
                }
                if !sc.vars.contains(name) {
                    sc.vars.push(name.clone());
                }
                Ok(())
            }
            _ => self.declare_lex(name, pos, true),
        }
    }

    pub(crate) fn declare_pattern(&mut self, pat: &Pat, kind: VarKind, for_of: bool) -> PResult<()> {
        let mut names = Vec::new();
        pat.bound_names(&mut names);
        for id in &names {
            self.check_binding_name(&id.name, id.span.start)?;
            match kind {
                VarKind::Var => self.declare_var(&id.name, id.span.start, for_of)?,
                _ => {
                    if &*id.name == "let" {
                        return self.err_at(id.span.start, "let is disallowed as a lexically bound name");
                    }
                    self.declare_lex(&id.name, id.span.start, false)?
                }
            }
        }
        Ok(())
    }

    /// Early errors for a BindingIdentifier.
    pub(crate) fn check_binding_name(&self, name: &str, pos: u32) -> PResult<()> {
        if self.ctx.strict && (name == "eval" || name == "arguments") {
            return self.err_at(pos, "invalid binding name in strict mode");
        }
        self.check_ident_name(name, pos)
    }

    /// Early errors for an IdentifierReference / BindingIdentifier / LabelIdentifier name.
    pub(crate) fn check_ident_name(&self, name: &str, pos: u32) -> PResult<()> {
        if is_reserved(name) {
            return self.err_at(pos, &format!("unexpected reserved word '{}'", name));
        }
        if self.ctx.strict && is_strict_reserved(name) {
            return self.err_at(pos, &format!("unexpected strict mode reserved word '{}'", name));
        }
        if name == "yield" && (self.ctx.yield_kw || self.ctx.strict) {
            return self.err_at(pos, "yield is reserved here");
        }
        if name == "await" && (self.ctx.await_kw || self.ctx.await_reserved || self.ctx.in_static_block) {
            return self.err_at(pos, "await is reserved here");
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------- private names

    pub(crate) fn use_private(&mut self, name: &Atom, pos: u32) -> PResult<()> {
        if let Some(c) = self.classes.last_mut() {
            c.refs.push((name.clone(), pos));
            Ok(())
        } else if self.eval_privates.contains(name) {
            Ok(())
        } else {
            self.err_at(pos, &format!("undeclared private name #{}", name))
        }
    }

    /// Directive prologue; returns true if it contains "use strict". Pushes the directive statements.
    pub(crate) fn directives(&mut self, body: &mut Vec<Stmt>) -> PResult<bool> {
        let mut strict = false;
        let mut octal_pos: Option<u32> = None;
        loop {
            let (s, raw_len) = match &self.tok.t {
                T::Str(s) => (s.clone(), self.tok.end - self.tok.start),
                _ => break,
            };
            let start = self.tok.start;
            let is_octal = self.tok.legacy_octal;
            // The directive must be a complete ExpressionStatement consisting of just the string.
            let save_pos = self.lx.pos;
            let save_tok = self.tok.clone();
            let save_prev = self.prev_end;
            let next = self.peek()?;
            let ends = match &next.t {
                T::Punct(P::Semi) | T::Punct(P::RBrace) | T::Eof => true,
                _ => next.nl_before && !matches!(&next.t, T::Punct(p) if continues_expression(*p)) && !matches!(&next.t, T::Template{..}),
            };
            if !ends {
                let _ = (save_pos, save_tok, save_prev);
                break;
            }
            if is_octal && octal_pos.is_none() {
                octal_pos = Some(start);
            }
            // "use strict" exactly, with no escapes or line continuations (raw length 12).
            if s.eq_str("use strict") && raw_len == 12 {
                strict = true;
                self.ctx.strict = true;
            }
            let st = self.parse_statement_list_item()?;
            body.push(st);
        }
        if strict {
            if let Some(p) = octal_pos {
                return self.err_at(p, "octal escape in strict mode");
            }
        }
        Ok(strict)
    }

    fn check_module_exports(&self, body: &[Stmt]) -> PResult<()> {
        let mut exported: Vec<JsStr> = Vec::new();
        let mut locals: Vec<(JsStr, u32)> = Vec::new();
        let mut declared: Vec<Atom> = Vec::new();
        for s in body {
            collect_top_declared(s, &mut declared);
        }
        for s in body {
            if let Stmt::Export(e) = s {
                let mut names: Vec<(JsStr, u32)> = Vec::new();
                match &**e {
                    ExportDecl::Decl(d) => {
                        let mut ds = Vec::new();
                        collect_top_declared(d, &mut ds);
                        for n in ds {
                            names.push((JsStr::from_str(&n), 0));
                        }
                    }
                    ExportDecl::Named { specs, source, span, .. } => {
                        for (local, exp) in specs {
                            names.push((exp.clone(), span.start));
                            if source.is_none() {
                                locals.push((local.clone(), span.start));
                            }
                        }
                    }
                    ExportDecl::All { exported: Some(n), span, .. } => names.push((n.clone(), span.start)),
                    ExportDecl::All { .. } => {}
                    ExportDecl::DefaultExpr(_, sp) => names.push((JsStr::from_str("default"), sp.start)),
                    ExportDecl::DefaultFunction(f) => names.push((JsStr::from_str("default"), f.span.start)),
                    ExportDecl::DefaultClass(c) => names.push((JsStr::from_str("default"), c.span.start)),
                }
                for (n, p) in names {
                    if exported.contains(&n) {
                        return self.err_at(p, &format!("duplicate export '{}'", n));
                    }
                    exported.push(n);
                }
            }
        }
        for (l, p) in locals {
            let name = l.to_rust();
            if !declared.iter().any(|d| **d == *name) {
                return self.err_at(p, &format!("export of undeclared name '{}'", name));
            }
        }
        Ok(())
    }
}

fn collect_top_declared(s: &Stmt, out: &mut Vec<Atom>) {
    match s {
        Stmt::Var(v) => {
            let mut ids = Vec::new();
            for d in &v.decls {
                d.target.bound_names(&mut ids);
            }
            out.extend(ids.into_iter().map(|i| i.name));
        }
        Stmt::Function(f) => {
            if let Some(id) = &f.id {
                out.push(id.name.clone());
            }
        }
        Stmt::Class(c) => {
            if let Some(id) = &c.id {
                out.push(id.name.clone());
            }
        }
        Stmt::Import(i) => {
            for sp in &i.specs {
                match sp {
                    ImportSpec::Default(id) | ImportSpec::Namespace(id) | ImportSpec::Named(_, id) => out.push(id.name.clone()),
                }
            }
        }
        Stmt::Export(e) => match &**e {
            ExportDecl::Decl(d) => collect_top_declared(d, out),
            ExportDecl::DefaultFunction(f) => {
                if let Some(id) = &f.id {
                    out.push(id.name.clone());
                }
            }
            ExportDecl::DefaultClass(c) => {
                if let Some(id) = &c.id {
                    out.push(id.name.clone());
                }
            }
            _ => {}
        },
        Stmt::Labeled(_, b, _) => collect_top_declared(b, out),
        // var declarations nested in statements are also module-scope declared names
        Stmt::If(_, a, b, _) => {
            collect_nested_vars(a, out);
            if let Some(b) = b {
                collect_nested_vars(b, out);
            }
        }
        _ => collect_nested_vars(s, out),
    }
}

/// VarDeclaredNames of nested statements.
pub(crate) fn collect_nested_vars(s: &Stmt, out: &mut Vec<Atom>) {
    let var_names = |v: &VarDecl, out: &mut Vec<Atom>| {
        if v.kind == VarKind::Var {
            let mut ids = Vec::new();
            for d in &v.decls {
                d.target.bound_names(&mut ids);
            }
            out.extend(ids.into_iter().map(|i| i.name));
        }
    };
    match s {
        Stmt::Var(v) => var_names(v, out),
        Stmt::If(_, a, b, _) => {
            collect_nested_vars(a, out);
            if let Some(b) = b {
                collect_nested_vars(b, out);
            }
        }
        Stmt::Block(b) => {
            for s in &b.body {
                collect_nested_vars(s, out);
            }
        }
        Stmt::For(f) => {
            if let Some(ForInit::Var(v)) = &f.init {
                var_names(v, out);
            }
            collect_nested_vars(&f.body, out);
        }
        Stmt::ForIn(f) | Stmt::ForOf(f) => {
            match &f.left {
                ForHead::Decl(VarKind::Var, p) | ForHead::VarInit(p, _) => {
                    let mut ids = Vec::new();
                    p.bound_names(&mut ids);
                    out.extend(ids.into_iter().map(|i| i.name));
                }
                _ => {}
            }
            collect_nested_vars(&f.body, out);
        }
        Stmt::While(_, b, _) | Stmt::DoWhile(b, _, _) | Stmt::Labeled(_, b, _) | Stmt::With(_, b, _, _) => collect_nested_vars(b, out),
        Stmt::Try(t) => {
            for s in &t.block.body {
                collect_nested_vars(s, out);
            }
            if let Some(h) = &t.handler {
                for s in &h.body.body {
                    collect_nested_vars(s, out);
                }
            }
            if let Some(f) = &t.finalizer {
                for s in &f.body {
                    collect_nested_vars(s, out);
                }
            }
        }
        Stmt::Switch(sw) => {
            for c in &sw.cases {
                for s in &c.body {
                    collect_nested_vars(s, out);
                }
            }
        }
        Stmt::Export(e) => {
            if let ExportDecl::Decl(d) = &**e {
                collect_nested_vars(d, out);
            }
        }
        _ => {}
    }
}

use crate::string::JsStr;

fn continues_expression(p: P) -> bool {
    !matches!(p, P::LBrace | P::Inc | P::Dec | P::Bang | P::Tilde)
}

pub fn is_keyword(s: &str) -> bool {
    is_reserved(s) || matches!(s, "let" | "static" | "yield" | "await" | "implements" | "interface" | "package" | "private" | "protected" | "public")
}

/// ReservedWord minus `await` and `yield` (handled contextually).
pub fn is_reserved(s: &str) -> bool {
    matches!(
        s,
        "break" | "case" | "catch" | "class" | "const" | "continue" | "debugger" | "default" | "delete" | "do"
            | "else" | "enum" | "export" | "extends" | "false" | "finally" | "for" | "function" | "if" | "import"
            | "in" | "instanceof" | "new" | "null" | "return" | "super" | "switch" | "this" | "throw" | "true"
            | "try" | "typeof" | "var" | "void" | "while" | "with"
    )
}

pub fn is_strict_reserved(s: &str) -> bool {
    matches!(s, "implements" | "interface" | "let" | "package" | "private" | "protected" | "public" | "static" | "yield")
}

/// Deepest syntactic nesting the parser accepts (each level costs a few KB of native stack in the parser and the
/// compiler, which recurse over the tree).
pub const MAX_NESTING: u32 = 1500;
