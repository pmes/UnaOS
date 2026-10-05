//! Statements, declarations, functions, classes and module items.

use super::*;
use alloc::boxed::Box;

impl Parser {
    // ------------------------------------------------------------------------------------- statements

    pub(crate) fn parse_statement_list_item(&mut self) -> PResult<Stmt> {
        let start = self.tok.start;
        if let T::Name(n) = self.tok.t.clone() {
            if !self.tok.escaped {
                match &*n {
                    "function" => {
                        self.advance()?;
                        let f = self.parse_function(start, false, true, false)?;
                        return Ok(Stmt::Function(f));
                    }
                    "class" => {
                        let c = self.parse_class(true, false)?;
                        return Ok(Stmt::Class(c));
                    }
                    "const" => {
                        let d = self.parse_var_decl(VarKind::Const, true)?;
                        self.semicolon()?;
                        return Ok(Stmt::Var(Box::new(d)));
                    }
                    "let" if self.let_is_declaration(true)? => {
                        let d = self.parse_var_decl(VarKind::Let, true)?;
                        self.semicolon()?;
                        return Ok(Stmt::Var(Box::new(d)));
                    }
                    "async" => {
                        let nx = self.peek()?;
                        if !nx.nl_before && matches!(&nx.t, T::Name(f) if &**f == "function") && !nx.escaped {
                            self.advance()?;
                            self.advance()?;
                            let f = self.parse_function(start, true, true, false)?;
                            return Ok(Stmt::Function(f));
                        }
                    }
                    "import" if self.goal == Goal::Script => {}
                    _ => {}
                }
            }
        }
        self.parse_statement()
    }

    /// `let` starts a LexicalDeclaration when followed by an identifier, `[` or `{`.
    pub(crate) fn let_is_declaration(&mut self, list_item: bool) -> PResult<bool> {
        if self.ctx.strict {
            return Ok(true);
        }
        let nx = self.peek()?;
        Ok(match &nx.t {
            T::Punct(P::LBracket) => true,
            T::Punct(P::LBrace) => list_item || !nx.nl_before,
            T::Name(n) => {
                if !list_item && nx.nl_before {
                    false
                } else {
                    !(matches!(&**n, "in" | "instanceof") && !nx.escaped)
                }
            }
            _ => false,
        })
    }

    pub(crate) fn parse_statement(&mut self) -> PResult<Stmt> {
        let start = self.tok.start;
        match &self.tok.t {
            T::Punct(P::LBrace) => {
                let b = self.parse_block()?;
                return Ok(Stmt::Block(Box::new(b)));
            }
            T::Punct(P::Semi) => {
                self.advance()?;
                return Ok(Stmt::Empty(self.span_from(start)));
            }
            T::Name(n) if !self.tok.escaped => {
                let n = n.clone();
                match &*n {
                    "var" => {
                        self.advance()?;
                        let d = self.parse_var_decl_rest(VarKind::Var, true, start)?;
                        self.semicolon()?;
                        return Ok(Stmt::Var(Box::new(d)));
                    }
                    "if" => return self.parse_if(),
                    "for" => return self.parse_for(),
                    "while" => {
                        self.advance()?;
                        self.expect(P::LParen)?;
                        let test = self.parse_expression(true)?;
                        self.expect(P::RParen)?;
                        let body = self.parse_loop_body()?;
                        return Ok(Stmt::While(Box::new(test), Box::new(body), self.span_from(start)));
                    }
                    "do" => {
                        self.advance()?;
                        let body = self.parse_loop_body()?;
                        self.expect_kw("while")?;
                        self.expect(P::LParen)?;
                        let test = self.parse_expression(true)?;
                        self.expect(P::RParen)?;
                        // ASI: a semicolon is always inserted after do-while's `)` if needed.
                        self.eat(P::Semi)?;
                        return Ok(Stmt::DoWhile(Box::new(body), Box::new(test), self.span_from(start)));
                    }
                    "continue" | "break" => {
                        let is_break = &*n == "break";
                        self.advance()?;
                        let mut label = None;
                        if let T::Name(l) = &self.tok.t {
                            if !self.tok.nl_before {
                                let l = l.clone();
                                let pos = self.tok.start;
                                self.check_ident_name(&l, pos)?;
                                self.advance()?;
                                match self.ctx.labels.iter().rev().find(|(x, _)| *x == l) {
                                    None => return self.err_at(pos, &format!("undefined label '{}'", l)),
                                    Some((_, is_loop)) => {
                                        if !is_break && !*is_loop {
                                            return self.err_at(pos, "continue target is not a loop");
                                        }
                                    }
                                }
                                label = Some(l);
                            }
                        }
                        if label.is_none() {
                            if is_break && !self.ctx.in_iteration && !self.ctx.in_switch {
                                return self.err_at(start, "illegal break");
                            }
                            if !is_break && !self.ctx.in_iteration {
                                return self.err_at(start, "illegal continue");
                            }
                        }
                        self.semicolon()?;
                        let sp = self.span_from(start);
                        return Ok(if is_break { Stmt::Break(label, sp) } else { Stmt::Continue(label, sp) });
                    }
                    "return" => {
                        if !self.ctx.in_function {
                            return self.err("return outside of function");
                        }
                        self.advance()?;
                        let arg = if self.is(P::Semi) || self.is(P::RBrace) || self.tok.t == T::Eof || self.tok.nl_before {
                            None
                        } else {
                            Some(Box::new(self.parse_expression(true)?))
                        };
                        self.semicolon()?;
                        return Ok(Stmt::Return(arg, self.span_from(start)));
                    }
                    "with" => {
                        if self.ctx.strict {
                            return self.err("with is not allowed in strict mode");
                        }
                        self.advance()?;
                        self.expect(P::LParen)?;
                        let obj = self.parse_expression(true)?;
                        self.expect(P::RParen)?;
                        let id = self.new_scope_id();
                        let body = self.parse_substatement()?;
                        return Ok(Stmt::With(Box::new(obj), Box::new(body), id, self.span_from(start)));
                    }
                    "switch" => return self.parse_switch(),
                    "throw" => {
                        self.advance()?;
                        if self.tok.nl_before {
                            return self.err("line break after throw");
                        }
                        let e = self.parse_expression(true)?;
                        self.semicolon()?;
                        return Ok(Stmt::Throw(Box::new(e), self.span_from(start)));
                    }
                    "try" => return self.parse_try(),
                    "debugger" => {
                        self.advance()?;
                        self.semicolon()?;
                        return Ok(Stmt::Debugger(self.span_from(start)));
                    }
                    "function" => return self.err("function declaration not allowed in statement position"),
                    "class" => return self.err("class declaration not allowed in statement position"),
                    "const" => return self.err("lexical declaration not allowed in statement position"),
                    "let" => {
                        let nx = self.peek()?;
                        if matches!(nx.t, T::Punct(P::LBracket)) {
                            return self.err("lexical declaration not allowed in statement position");
                        }
                        if self.ctx.strict {
                            return self.err("lexical declaration not allowed in statement position");
                        }
                        if !nx.nl_before {
                            if let T::Name(_) | T::Punct(P::LBrace) = nx.t {
                                if !matches!(&nx.t, T::Name(k) if &**k == "in" || &**k == "instanceof") {
                                    return self.err("lexical declaration not allowed in statement position");
                                }
                            }
                        }
                    }
                    "import" if self.goal == Goal::Module => {
                        let nx = self.peek()?;
                        if !matches!(nx.t, T::Punct(P::LParen) | T::Punct(P::Dot)) {
                            return self.err("import declaration only at module top level");
                        }
                    }
                    "export" => return self.err("export declaration only at module top level"),
                    "async" => {
                        let nx = self.peek()?;
                        if !nx.nl_before && matches!(&nx.t, T::Name(f) if &**f == "function") && !nx.escaped {
                            return self.err("async function declaration not allowed in statement position");
                        }
                    }
                    _ => {}
                }
                // Labelled statement?
                if !matches!(&*n, "yield" | "await") || !self.reserved_here(&n) {
                    let nx = self.peek()?;
                    if nx.t == T::Punct(P::Colon) && !is_reserved(&n) {
                        return self.parse_labeled();
                    }
                }
            }
            T::Name(n) => {
                let n = n.clone();
                let nx = self.peek()?;
                if nx.t == T::Punct(P::Colon) {
                    if is_reserved(&n) {
                        return self.err("keyword must not contain escapes");
                    }
                    return self.parse_labeled();
                }
            }
            _ => {}
        }
        // ExpressionStatement
        let e = self.parse_expression(true)?;
        self.semicolon()?;
        Ok(Stmt::Expr(Box::new(e), self.span_from(start)))
    }

    fn reserved_here(&self, n: &str) -> bool {
        match n {
            "yield" => self.ctx.yield_kw || self.ctx.strict,
            "await" => self.ctx.await_kw || self.ctx.await_reserved || self.ctx.in_static_block,
            _ => false,
        }
    }

    fn parse_labeled(&mut self) -> PResult<Stmt> {
        let start = self.tok.start;
        let name = match &self.tok.t {
            T::Name(n) => n.clone(),
            _ => unreachable!(),
        };
        self.check_ident_name(&name, start)?;
        self.advance()?;
        self.expect(P::Colon)?;
        if self.ctx.labels.iter().any(|(l, _)| *l == name) {
            return self.err_at(start, &format!("duplicate label '{}'", name));
        }
        // Is the labelled item an iteration statement (possibly through more labels)?
        let is_loop = self.label_target_is_loop()?;
        self.ctx.labels.push((name.clone(), is_loop));
        let body = if self.is_kw("function") {
            if self.ctx.strict {
                return self.err("labelled function declaration in strict mode");
            }
            let fs = self.tok.start;
            self.advance()?;
            if self.is(P::Star) {
                return self.err("labelled generator declaration");
            }
            let f = self.parse_function(fs, false, true, false)?;
            Stmt::Function(f)
        } else {
            self.parse_statement()?
        };
        self.ctx.labels.pop();
        Ok(Stmt::Labeled(name, Box::new(body), self.span_from(start)))
    }

    fn label_target_is_loop(&mut self) -> PResult<bool> {
        // Look through further `label:` prefixes.
        let save_pos = self.lx.pos;
        let save_nl = self.lx.nl_before;
        let mut cur = self.tok.clone();
        let mut res = false;
        loop {
            match &cur.t {
                T::Name(n) if !cur.escaped && matches!(&**n, "for" | "while" | "do") => {
                    res = true;
                    break;
                }
                T::Name(_) => {
                    let nx = match self.lx.next_token() {
                        Ok(t) => t,
                        Err(_) => break,
                    };
                    if nx.t == T::Punct(P::Colon) {
                        cur = match self.lx.next_token() {
                            Ok(t) => t,
                            Err(_) => break,
                        };
                        continue;
                    }
                    break;
                }
                _ => break,
            }
        }
        self.lx.pos = save_pos;
        self.lx.nl_before = save_nl;
        Ok(res)
    }

    pub(crate) fn parse_block(&mut self) -> PResult<Block> {
        let start = self.tok.start;
        self.expect(P::LBrace)?;
        let scope = self.new_scope_id();
        self.push_scope(ScopeKind::Block);
        let mut body = Vec::new();
        while !self.is(P::RBrace) {
            if self.tok.t == T::Eof {
                return self.unexpected();
            }
            body.push(self.parse_statement_list_item()?);
        }
        self.advance()?;
        self.pop_scope();
        Ok(Block { body, scope, span: self.span_from(start) })
    }

    /// The body of if / with / labelled statement (Annex B: sloppy `if (x) function f(){}` allowed in if only).
    fn parse_substatement(&mut self) -> PResult<Stmt> {
        let pos = self.tok.start;
        let s = self.parse_statement()?;
        if is_labelled_function(&s) {
            return self.err_at(pos, "labelled function declaration in statement position");
        }
        Ok(s)
    }

    fn parse_loop_body(&mut self) -> PResult<Stmt> {
        let saved = self.ctx.in_iteration;
        self.ctx.in_iteration = true;
        // Labels in the label set apply to this loop; continue may target them.
        let r = self.parse_substatement();
        self.ctx.in_iteration = saved;
        r
    }

    fn parse_if(&mut self) -> PResult<Stmt> {
        let start = self.tok.start;
        self.advance()?;
        self.expect(P::LParen)?;
        let test = self.parse_expression(true)?;
        self.expect(P::RParen)?;
        let cons = self.parse_if_branch()?;
        let alt = if self.eat_kw("else")? { Some(Box::new(self.parse_if_branch()?)) } else { None };
        Ok(Stmt::If(Box::new(test), Box::new(cons), alt, self.span_from(start)))
    }

    fn parse_if_branch(&mut self) -> PResult<Stmt> {
        if self.is_kw("function") && !self.ctx.strict {
            // Annex B.3.3: FunctionDeclaration in an IfStatement clause behaves as if wrapped in a block.
            let start = self.tok.start;
            self.advance()?;
            if self.is(P::Star) {
                return self.err("generator declaration in if statement");
            }
            let scope = self.new_scope_id();
            self.push_scope(ScopeKind::Block);
            let f = self.parse_function(start, false, true, false)?;
            self.pop_scope();
            return Ok(Stmt::Block(Box::new(Block { body: vec![Stmt::Function(f)], scope, span: self.span_from(start) })));
        }
        self.parse_substatement()
    }

    pub(crate) fn parse_var_decl(&mut self, kind: VarKind, allow_in: bool) -> PResult<VarDecl> {
        let start = self.tok.start;
        self.advance()?;
        self.parse_var_decl_rest(kind, allow_in, start)
    }

    fn parse_var_decl_rest(&mut self, kind: VarKind, allow_in: bool, start: u32) -> PResult<VarDecl> {
        let mut decls = Vec::new();
        loop {
            let target = self.parse_binding_target()?;
            self.declare_pattern(&target, kind, false)?;
            let init = if self.eat(P::Assign)? {
                Some(self.parse_assign(allow_in)?)
            } else {
                None
            };
            if init.is_none() && allow_in {
                if kind == VarKind::Const {
                    return self.err("missing initializer in const declaration");
                }
                if !matches!(target, Pat::Ident(_)) {
                    return self.err("missing initializer in destructuring declaration");
                }
            }
            decls.push(Declarator { target, init });
            if !self.eat(P::Comma)? {
                break;
            }
        }
        Ok(VarDecl { kind, decls, span: self.span_from(start) })
    }

    fn parse_for(&mut self) -> PResult<Stmt> {
        let start = self.tok.start;
        self.advance()?;
        let mut is_await = false;
        if self.is_kw("await") {
            if !self.ctx.await_kw {
                return self.err("for await is only valid in async functions and modules");
            }
            if !self.ctx.in_function {
                self.has_top_await = true;
            }
            is_await = true;
            self.advance()?;
        }
        self.expect(P::LParen)?;
        let scope = self.new_scope_id();
        self.push_scope(ScopeKind::Block);
        let r = self.parse_for_rest(start, is_await, scope);
        self.pop_scope();
        r
    }

    fn parse_for_rest(&mut self, start: u32, is_await: bool, scope: ScopeId) -> PResult<Stmt> {
        // for ( [var|let|const] … )
        let mut init: Option<ForInit> = None;
        if self.is(P::Semi) {
            if is_await {
                return self.unexpected();
            }
        } else {
            let decl_kind = if self.is_kw("var") {
                Some(VarKind::Var)
            } else if self.is_kw("const") {
                Some(VarKind::Const)
            } else if self.is_kw("let") {
                let nx = self.peek()?;
                let is_decl = self.ctx.strict
                    || match &nx.t {
                        T::Punct(P::LBracket) | T::Punct(P::LBrace) => true,
                        T::Name(n) => !(matches!(&**n, "in" | "instanceof") && !nx.escaped),
                        _ => false,
                    };
                if is_decl {
                    Some(VarKind::Let)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(kind) = decl_kind {
                let dstart = self.tok.start;
                self.advance()?;
                let target = self.parse_binding_target()?;
                // for-in / for-of with a single binding?
                if self.is_kw("of") || (self.is_kw("in") && !is_await) {
                    let is_of = self.is_kw("of");
                    self.advance()?;
                    self.declare_pattern(&target, kind, is_of)?;
                    let right = if is_of { self.parse_assign(true)? } else { self.parse_expression(true)? };
                    self.expect(P::RParen)?;
                    self.check_for_decl_dups(&target, kind)?;
                    let body = self.parse_loop_body()?;
                    self.check_for_body_lex(&target, kind, &body)?;
                    let f = ForInStmt { left: ForHead::Decl(kind, target), right, body, is_await, scope, span: self.span_from(start) };
                    return Ok(if is_of { Stmt::ForOf(Box::new(f)) } else { Stmt::ForIn(Box::new(f)) });
                }
                if is_await {
                    return self.unexpected();
                }
                // Annex B.3.5: `for (var x = init in obj)` in sloppy mode with a simple binding.
                if kind == VarKind::Var && self.is(P::Assign) && !self.ctx.strict {
                    if let Pat::Ident(_) = &target {
                        let save_pos = self.lx.pos;
                        let _ = save_pos;
                        self.advance()?;
                        let e = self.parse_assign(false)?;
                        if self.is_kw("in") {
                            self.advance()?;
                            self.declare_pattern(&target, kind, false)?;
                            let right = self.parse_expression(true)?;
                            self.expect(P::RParen)?;
                            let body = self.parse_loop_body()?;
                            let f = ForInStmt { left: ForHead::VarInit(target, Box::new(e)), right, body, is_await, scope, span: self.span_from(start) };
                            return Ok(Stmt::ForIn(Box::new(f)));
                        }
                        // Regular for with an initialised first declarator.
                        self.declare_pattern(&target, kind, false)?;
                        let mut decls = vec![Declarator { target, init: Some(e) }];
                        while self.eat(P::Comma)? {
                            let t = self.parse_binding_target()?;
                            self.declare_pattern(&t, kind, false)?;
                            let i = if self.eat(P::Assign)? { Some(self.parse_assign(false)?) } else { None };
                            if i.is_none() && !matches!(t, Pat::Ident(_)) {
                                return self.err("missing initializer in destructuring declaration");
                            }
                            decls.push(Declarator { target: t, init: i });
                        }
                        init = Some(ForInit::Var(Box::new(VarDecl { kind, decls, span: self.span_from(dstart) })));
                        return self.parse_for_classic(start, init, scope);
                    }
                }
                // Regular declaration list (no `in` allowed in initialisers).
                self.declare_pattern(&target, kind, false)?;
                let first_init = if self.eat(P::Assign)? { Some(self.parse_assign(false)?) } else { None };
                if first_init.is_none() && (kind == VarKind::Const || !matches!(target, Pat::Ident(_))) {
                    return self.err("missing initializer in declaration");
                }
                let mut decls = vec![Declarator { target, init: first_init }];
                while self.eat(P::Comma)? {
                    let t = self.parse_binding_target()?;
                    self.declare_pattern(&t, kind, false)?;
                    let i = if self.eat(P::Assign)? { Some(self.parse_assign(false)?) } else { None };
                    if i.is_none() && (kind == VarKind::Const || !matches!(t, Pat::Ident(_))) {
                        return self.err("missing initializer in declaration");
                    }
                    decls.push(Declarator { target: t, init: i });
                }
                init = Some(ForInit::Var(Box::new(VarDecl { kind, decls, span: self.span_from(dstart) })));
            } else {
                // Expression or assignment target.
                let estart = self.tok.start;
                let starts_with_let = self.is_kw("let");
                let starts_with_async = self.is_kw("async") && !self.tok.escaped;
                let cover_before = self.cover_init;
                self.cover_init = None;
                let e = self.parse_expression_inner(false)?;
                if self.is_kw("of") || (self.is_kw("in") && !is_await) {
                    let is_of = self.is_kw("of");
                    if is_of && starts_with_let {
                        return self.err_at(estart, "for-of left side may not start with let");
                    }
                    if is_of && starts_with_async && !is_await && matches!(&e, Expr::Ident(i) if &*i.name == "async") {
                        return self.err_at(estart, "for (async of …) is not allowed");
                    }
                    self.advance()?;
                    let target = self.expr_to_assign_target(e, true)?;
                    self.cover_init = cover_before;
                    let right = if is_of { self.parse_assign(true)? } else { self.parse_expression(true)? };
                    self.expect(P::RParen)?;
                    let body = self.parse_loop_body()?;
                    let f = ForInStmt { left: ForHead::Target(target), right, body, is_await, scope, span: self.span_from(start) };
                    return Ok(if is_of { Stmt::ForOf(Box::new(f)) } else { Stmt::ForIn(Box::new(f)) });
                }
                if let Some(p) = self.cover_init {
                    return self.err_at(p, "invalid shorthand property initializer");
                }
                self.cover_init = cover_before;
                if is_await {
                    return self.unexpected();
                }
                init = Some(ForInit::Expr(Box::new(e)));
            }
        }
        self.parse_for_classic(start, init, scope)
    }

    fn parse_for_classic(&mut self, start: u32, init: Option<ForInit>, scope: ScopeId) -> PResult<Stmt> {
        self.expect(P::Semi)?;
        let test = if self.is(P::Semi) { None } else { Some(self.parse_expression(true)?) };
        self.expect(P::Semi)?;
        let update = if self.is(P::RParen) { None } else { Some(self.parse_expression(true)?) };
        self.expect(P::RParen)?;
        let body = self.parse_loop_body()?;
        if let Some(ForInit::Var(v)) = &init {
            if v.kind != VarKind::Var {
                let mut names = Vec::new();
                for d in &v.decls {
                    d.target.bound_names(&mut names);
                }
                self.check_body_var_conflict(&names, &body)?;
            }
        }
        Ok(Stmt::For(Box::new(ForStmt { init, test, update, body, scope, span: self.span_from(start) })))
    }

    fn check_for_decl_dups(&self, target: &Pat, kind: VarKind) -> PResult<()> {
        if kind == VarKind::Var {
            return Ok(());
        }
        let mut names = Vec::new();
        target.bound_names(&mut names);
        for (i, a) in names.iter().enumerate() {
            if names[..i].iter().any(|b| b.name == a.name) {
                return self.err_at(a.span.start, "duplicate binding");
            }
        }
        Ok(())
    }

    fn check_for_body_lex(&self, target: &Pat, kind: VarKind, body: &Stmt) -> PResult<()> {
        if kind == VarKind::Var {
            return Ok(());
        }
        let mut names = Vec::new();
        target.bound_names(&mut names);
        self.check_body_var_conflict(&names, body)
    }

    /// `for (let x …) { var x }` — the body's VarDeclaredNames must not include the loop's lexical names.
    fn check_body_var_conflict(&self, names: &[Ident], body: &Stmt) -> PResult<()> {
        let mut vars = Vec::new();
        collect_nested_vars(body, &mut vars);
        for n in names {
            if vars.contains(&n.name) {
                return self.err_at(n.span.start, &format!("redeclaration of '{}'", n.name));
            }
        }
        Ok(())
    }

    fn parse_switch(&mut self) -> PResult<Stmt> {
        let start = self.tok.start;
        self.advance()?;
        self.expect(P::LParen)?;
        let disc = self.parse_expression(true)?;
        self.expect(P::RParen)?;
        self.expect(P::LBrace)?;
        let scope = self.new_scope_id();
        self.push_scope(ScopeKind::Block);
        let saved = self.ctx.in_switch;
        self.ctx.in_switch = true;
        let mut cases = Vec::new();
        let mut has_default = false;
        while !self.is(P::RBrace) {
            let test = if self.eat_kw("case")? {
                Some(self.parse_expression(true)?)
            } else if self.eat_kw("default")? {
                if has_default {
                    return self.err("multiple default clauses");
                }
                has_default = true;
                None
            } else {
                return self.unexpected();
            };
            self.expect(P::Colon)?;
            let mut body = Vec::new();
            while !self.is(P::RBrace) && !self.is_kw("case") && !self.is_kw("default") {
                if self.tok.t == T::Eof {
                    return self.unexpected();
                }
                body.push(self.parse_statement_list_item()?);
            }
            cases.push(SwitchCase { test, body });
        }
        self.advance()?;
        self.ctx.in_switch = saved;
        self.pop_scope();
        Ok(Stmt::Switch(Box::new(SwitchStmt { disc, cases, scope, span: self.span_from(start) })))
    }

    fn parse_try(&mut self) -> PResult<Stmt> {
        let start = self.tok.start;
        self.advance()?;
        let block = self.parse_block()?;
        let mut handler = None;
        if self.eat_kw("catch")? {
            let scope = self.new_scope_id();
            self.push_scope(ScopeKind::Catch);
            let mut param = None;
            if self.eat(P::LParen)? {
                let p = self.parse_binding_target()?;
                let mut names = Vec::new();
                p.bound_names(&mut names);
                for (i, n) in names.iter().enumerate() {
                    self.check_binding_name(&n.name, n.span.start)?;
                    if names[..i].iter().any(|m| m.name == n.name) {
                        return self.err_at(n.span.start, "duplicate catch parameter");
                    }
                }
                let simple = matches!(p, Pat::Ident(_));
                {
                    let sc = self.scopes.last_mut().unwrap();
                    sc.catch_params = names.iter().map(|n| n.name.clone()).collect();
                    sc.simple_catch = simple;
                }
                param = Some(p);
                self.expect(P::RParen)?;
            }
            // The catch block's lexical declarations live in the catch scope (they conflict with the param).
            let bstart = self.tok.start;
            self.expect(P::LBrace)?;
            let bscope = self.new_scope_id();
            let mut body = Vec::new();
            while !self.is(P::RBrace) {
                if self.tok.t == T::Eof {
                    return self.unexpected();
                }
                body.push(self.parse_statement_list_item()?);
            }
            self.advance()?;
            self.pop_scope();
            handler = Some(Catch { param, body: Block { body, scope: bscope, span: self.span_from(bstart) }, scope });
        }
        let finalizer = if self.eat_kw("finally")? { Some(self.parse_block()?) } else { None };
        if handler.is_none() && finalizer.is_none() {
            return self.err("missing catch or finally after try");
        }
        Ok(Stmt::Try(Box::new(TryStmt { block, handler, finalizer, span: self.span_from(start) })))
    }

    // ------------------------------------------------------------------------------------- binding patterns

    pub(crate) fn parse_binding_target(&mut self) -> PResult<Pat> {
        let start = self.tok.start;
        match &self.tok.t {
            T::Punct(P::LBracket) => {
                self.advance()?;
                let mut elems = Vec::new();
                let mut rest = None;
                loop {
                    if self.eat(P::RBracket)? {
                        break;
                    }
                    if self.is(P::Comma) {
                        self.advance()?;
                        elems.push(None);
                        continue;
                    }
                    if self.eat(P::Ellipsis)? {
                        let t = self.parse_binding_target()?;
                        rest = Some(Box::new(t));
                        if self.is(P::Comma) {
                            return self.err("rest element must be last");
                        }
                        self.expect(P::RBracket)?;
                        break;
                    }
                    let el = self.parse_binding_element()?;
                    elems.push(Some(el));
                    if !self.is(P::RBracket) {
                        self.expect(P::Comma)?;
                    }
                }
                Ok(Pat::Array(elems, rest, self.span_from(start)))
            }
            T::Punct(P::LBrace) => {
                self.advance()?;
                let mut props = Vec::new();
                let mut rest = None;
                loop {
                    if self.eat(P::RBrace)? {
                        break;
                    }
                    if self.eat(P::Ellipsis)? {
                        let id = self.parse_binding_ident()?;
                        rest = Some(Box::new(Pat::Ident(id)));
                        self.expect(P::RBrace)?;
                        break;
                    }
                    let kstart = self.tok.start;
                    let is_ident = matches!(self.tok.t, T::Name(_));
                    let key = self.parse_prop_key()?;
                    if let PropKey::Private(_) = key {
                        return self.err_at(kstart, "unexpected private name");
                    }
                    let value = if self.eat(P::Colon)? {
                        self.parse_binding_element()?
                    } else {
                        // Shorthand: key must be an identifier.
                        let name = match (&key, is_ident) {
                            (PropKey::Name(n), true) => n.clone(),
                            _ => return self.unexpected(),
                        };
                        let sp = self.span_from(kstart);
                        self.check_ident_name(&name, kstart)?;
                        let id = Ident { name, span: sp };
                        if self.eat(P::Assign)? {
                            let d = self.parse_assign(true)?;
                            Pat::Assign(Box::new(Pat::Ident(id)), Box::new(d), self.span_from(kstart))
                        } else {
                            Pat::Ident(id)
                        }
                    };
                    props.push(PatProp { key, value });
                    if !self.is(P::RBrace) {
                        self.expect(P::Comma)?;
                    }
                }
                Ok(Pat::Object(props, rest, self.span_from(start)))
            }
            _ => Ok(Pat::Ident(self.parse_binding_ident()?)),
        }
    }

    fn parse_binding_element(&mut self) -> PResult<Pat> {
        let start = self.tok.start;
        let t = self.parse_binding_target()?;
        if self.eat(P::Assign)? {
            let d = self.parse_assign(true)?;
            return Ok(Pat::Assign(Box::new(t), Box::new(d), self.span_from(start)));
        }
        Ok(t)
    }

    pub(crate) fn parse_binding_ident(&mut self) -> PResult<Ident> {
        let start = self.tok.start;
        match &self.tok.t {
            T::Name(n) => {
                let n = n.clone();
                self.check_binding_name(&n, start)?;
                if &*n == "await" && !self.ctx.in_function && self.goal == Goal::Module {
                    return self.err("await is reserved in modules");
                }
                if &*n == "await" {
                    self.await_ident_pos = Some(start);
                }
                self.advance()?;
                Ok(Ident { name: n, span: self.span_from(start) })
            }
            _ => self.unexpected(),
        }
    }

    // ------------------------------------------------------------------------------------- functions

    /// After `function` (and `async`): parse `[*] [name] (params) { body }`.
    pub(crate) fn parse_function(&mut self, start: u32, is_async: bool, is_decl: bool, default_export: bool) -> PResult<Rc<Function>> {
        let is_gen = self.eat(P::Star)?;
        let mut id = None;
        if let T::Name(_) = &self.tok.t {
            {
                let pos = self.tok.start;
                let name = match &self.tok.t {
                    T::Name(n) => n.clone(),
                    _ => unreachable!(),
                };
                if is_decl {
                    self.check_ident_name(&name, pos)?;
                    if self.ctx.strict && (&*name == "eval" || &*name == "arguments") {
                        return self.err_at(pos, "invalid function name in strict mode");
                    }
                } else {
                    // A function expression's name is checked in its own yield/await context.
                    if is_reserved(&name) || (self.ctx.strict && is_strict_reserved(&name)) {
                        return self.err_at(pos, "unexpected reserved word");
                    }
                    if &*name == "yield" && (is_gen || self.ctx.strict) {
                        return self.err_at(pos, "yield is not a valid function name here");
                    }
                    if &*name == "await" && (is_async || self.goal == Goal::Module) {
                        return self.err_at(pos, "await is not a valid function name here");
                    }
                    if self.ctx.strict && (&*name == "eval" || &*name == "arguments") {
                        return self.err_at(pos, "invalid function name in strict mode");
                    }
                }
                self.advance()?;
                id = Some(Ident { name, span: self.span_from(pos) });
            }
        }
        if id.is_none() && is_decl && !default_export {
            return self.unexpected();
        }
        if is_decl {
            if let Some(i) = &id {
                if !self.scopes.is_empty() {
                    let in_block = self.scopes.last().unwrap().kind == ScopeKind::Block;
                    if in_block && (is_async || is_gen) {
                        // Async / generator declarations in blocks are lexical and never duplicable.
                        let sc = self.scopes.last().unwrap();
                        if sc.lex.contains(&i.name) {
                            return self.err_at(i.span.start, "redeclaration");
                        }
                        self.declare_lex(&i.name, i.span.start, false)?;
                    } else {
                        self.declare_function(&i.name, i.span.start)?;
                    }
                }
            }
        }
        let name_scope = self.new_scope_id();
        let f = self.parse_function_rest(start, id, FnKind::Normal, is_async, is_gen, false, name_scope)?;
        Ok(Rc::new(f))
    }

    /// Parameters and body of a function / method. The current token is `(`.
    pub(crate) fn parse_function_rest(
        &mut self,
        start: u32,
        id: Option<Ident>,
        kind: FnKind,
        is_async: bool,
        is_gen: bool,
        derived: bool,
        name_scope: ScopeId,
    ) -> PResult<Function> {
        let saved_ctx = self.ctx.clone();
        let saved_yield = self.yield_pos.take();
        let saved_await = self.await_pos.take();
        let saved_await_ident = self.await_ident_pos.take();
        let saved_cover = self.cover_init.take();
        let is_method = matches!(kind, FnKind::Method | FnKind::Getter | FnKind::Setter | FnKind::ClassConstructor);
        self.ctx = Ctx {
            strict: saved_ctx.strict,
            in_function: true,
            yield_kw: is_gen,
            await_kw: is_async,
            await_reserved: self.goal == Goal::Module,
            in_params: true,
            super_prop: is_method,
            super_call: kind == FnKind::ClassConstructor && derived,
            new_target: true,
            in_field_init: false,
            in_static_block: false,
            labels: Vec::new(),
            in_iteration: false,
            in_switch: false,
            is_arrow: false,
        };
        self.flags.push(FnFlags::default());
        let scope = self.new_scope_id();
        self.push_scope(ScopeKind::Function);
        let r = self.parse_function_inner(start, id, kind, is_async, is_gen, derived, scope, name_scope);
        self.pop_scope();
        let fl = self.flags.pop().unwrap();
        let _ = fl;
        self.ctx = saved_ctx;
        self.yield_pos = saved_yield;
        self.await_pos = saved_await;
        self.await_ident_pos = saved_await_ident;
        self.cover_init = saved_cover;
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn parse_function_inner(
        &mut self,
        start: u32,
        id: Option<Ident>,
        kind: FnKind,
        is_async: bool,
        is_gen: bool,
        derived: bool,
        scope: ScopeId,
        name_scope: ScopeId,
    ) -> PResult<Function> {
        self.expect(P::LParen)?;
        let (params, rest, simple, length) = self.parse_formal_params()?;
        self.expect(P::RParen)?;
        match kind {
            FnKind::Getter if !params.is_empty() || rest.is_some() => return self.err("getter must have no parameters"),
            FnKind::Setter if params.len() != 1 || rest.is_some() => return self.err("setter must have exactly one parameter"),
            _ => {}
        }
        self.ctx.in_params = false;
        let mut names = Vec::new();
        for p in &params {
            p.bound_names(&mut names);
        }
        if let Some(r) = &rest {
            r.bound_names(&mut names);
        }
        {
            let sc = self.scopes.last_mut().unwrap();
            sc.params = names.iter().map(|n| n.name.clone()).collect();
        }
        let was_strict = self.ctx.strict;
        let body_scope = self.new_scope_id();
        let bstart = self.tok.start;
        self.expect(P::LBrace)?;
        let mut body = Vec::new();
        let use_strict = self.directives(&mut body)?;
        if use_strict && !simple {
            return self.err_at(bstart, "\"use strict\" not allowed in function with non-simple parameters");
        }
        while !self.is(P::RBrace) {
            if self.tok.t == T::Eof {
                return self.unexpected();
            }
            body.push(self.parse_statement_list_item()?);
        }
        let strict = self.ctx.strict;
        self.advance_after_body()?;
        // Retroactive checks now that strictness is known.
        if strict && !was_strict {
            if let Some(i) = &id {
                if &*i.name == "eval" || &*i.name == "arguments" || is_strict_reserved(&i.name) {
                    return self.err_at(i.span.start, "invalid function name in strict mode");
                }
            }
        }
        let is_method = matches!(kind, FnKind::Method | FnKind::Getter | FnKind::Setter | FnKind::ClassConstructor);
        for (i, n) in names.iter().enumerate() {
            if strict && (&*n.name == "eval" || &*n.name == "arguments" || is_strict_reserved(&n.name)) {
                return self.err_at(n.span.start, "invalid parameter name in strict mode");
            }
            if (strict || !simple || is_method) && names[..i].iter().any(|m| m.name == n.name) {
                return self.err_at(n.span.start, "duplicate parameter name");
            }
        }
        let fl = *self.flags.last().unwrap();
        Ok(Function {
            id,
            params,
            rest,
            body,
            expr_body: None,
            kind,
            is_async,
            is_generator: is_gen,
            strict,
            simple_params: simple,
            derived,
            span: self.span_from(start),
            scope,
            body_scope,
            length,
            has_direct_eval: fl.has_direct_eval,
            uses_arguments: fl.uses_arguments,
            uses_this: fl.uses_this,
            uses_super: fl.uses_super,
            class_fields: false,
            name_scope,
        })
    }

    /// Consume the closing `}` of a function body. The token after it is lexed in the enclosing context.
    fn advance_after_body(&mut self) -> PResult<()> {
        if !self.is(P::RBrace) {
            return self.unexpected();
        }
        // The next token must be lexed with the outer strictness; tokens carry flags checked on use.
        self.advance()?;
        Ok(())
    }

    /// FormalParameters: returns (params, rest, is_simple, ExpectedArgumentCount).
    pub(crate) fn parse_formal_params(&mut self) -> PResult<(Vec<Pat>, Option<Pat>, bool, u32)> {
        let mut params = Vec::new();
        let mut rest = None;
        let mut simple = true;
        let mut length = 0u32;
        let mut seen_default = false;
        while !self.is(P::RParen) {
            if self.eat(P::Ellipsis)? {
                let t = self.parse_binding_target()?;
                if self.is(P::Assign) {
                    return self.err("rest parameter may not have a default");
                }
                rest = Some(t);
                simple = false;
                if !self.is(P::RParen) {
                    return self.err("rest parameter must be last");
                }
                break;
            }
            let p = self.parse_binding_element()?;
            match &p {
                Pat::Ident(_) => {
                    if !seen_default {
                        length += 1;
                    }
                }
                Pat::Assign(..) => {
                    simple = false;
                    seen_default = true;
                }
                _ => {
                    simple = false;
                    if !seen_default {
                        length += 1;
                    }
                }
            }
            params.push(p);
            if !self.is(P::RParen) {
                self.expect(P::Comma)?;
            }
        }
        Ok((params, rest, simple, length))
    }

    // ------------------------------------------------------------------------------------- classes

    pub(crate) fn parse_class(&mut self, is_decl: bool, default_export: bool) -> PResult<Rc<Class>> {
        let start = self.tok.start;
        self.expect_kw("class")?;
        let saved_strict = self.ctx.strict;
        self.ctx.strict = true;
        let mut id = None;
        if let T::Name(n) = &self.tok.t {
            if !(self.is_kw("extends")) {
                let n = n.clone();
                let pos = self.tok.start;
                self.check_binding_name(&n, pos)?;
                self.advance()?;
                id = Some(Ident { name: n, span: self.span_from(pos) });
            }
        }
        if id.is_none() && is_decl && !default_export {
            return self.unexpected();
        }
        if is_decl {
            if let Some(i) = &id {
                self.declare_lex(&i.name, i.span.start, false)?;
            }
        }
        let scope = self.new_scope_id();
        let super_class = if self.eat_kw("extends")? { Some(Box::new(self.parse_lhs_expression()?)) } else { None };
        let derived = super_class.is_some();
        self.expect(P::LBrace)?;
        self.classes.push(ClassPriv { declared: Vec::new(), refs: Vec::new() });
        let mut members = Vec::new();
        let mut constructor = None;
        while !self.is(P::RBrace) {
            if self.eat(P::Semi)? {
                continue;
            }
            if self.tok.t == T::Eof {
                return self.unexpected();
            }
            self.parse_class_element(&mut members, &mut constructor, derived)?;
        }
        self.advance()?;
        // AllPrivateIdentifiersValid
        let cp = self.classes.pop().unwrap();
        for (name, pos) in cp.refs {
            if cp.declared.iter().any(|(n, _, _)| *n == name) {
                continue;
            }
            if let Some(outer) = self.classes.last_mut() {
                outer.refs.push((name, pos));
            } else if !self.eval_privates.contains(&name) {
                return self.err_at(pos, &format!("undeclared private name #{}", name));
            }
        }
        self.ctx.strict = saved_strict;
        Ok(Rc::new(Class { id, super_class, constructor, members, scope, span: self.span_from(start) }))
    }

    fn parse_class_element(&mut self, members: &mut Vec<ClassMember>, constructor: &mut Option<Rc<Function>>, derived: bool) -> PResult<()> {
        let start = self.tok.start;
        // A method's source text starts after `static` (§15.7.1 ClassElement : static MethodDefinition).
        let mut mstart = start;
        let mut is_static = false;
        let mut is_async = false;
        let mut is_gen = false;
        let mut kind = MethodKind::Method;
        // `static`
        if self.is_kw("static") {
            let nx = self.peek()?;
            let modifier = starts_element_name(&nx.t) || matches!(nx.t, T::Punct(P::Star) | T::Punct(P::LBrace));
            if modifier {
                self.advance()?;
                is_static = true;
                mstart = self.tok.start;
                if self.is(P::LBrace) {
                    let f = self.parse_static_block(start)?;
                    members.push(ClassMember::StaticBlock(Rc::new(f)));
                    return Ok(());
                }
            }
        }
        if self.is_kw("async") {
            let nx = self.peek()?;
            let modifier = !nx.nl_before && (starts_element_name(&nx.t) || nx.t == T::Punct(P::Star));
            if modifier {
                self.advance()?;
                is_async = true;
            }
        }
        if self.is(P::Star) {
            self.advance()?;
            is_gen = true;
        }
        if !is_async && !is_gen && (self.is_kw("get") || self.is_kw("set")) {
            let nx = self.peek()?;
            if starts_element_name(&nx.t) {
                kind = if self.is_kw("get") { MethodKind::Get } else { MethodKind::Set };
                self.advance()?;
            }
        }
        let kpos = self.tok.start;
        let key = self.parse_class_key()?;
        let key_name: Option<String> = match &key {
            PropKey::Name(n) => Some(String::from(&**n)),
            PropKey::Str(s) => Some(s.to_rust()),
            _ => None,
        };
        if let PropKey::Private(n) = &key {
            if &**n == "constructor" {
                return self.err_at(kpos, "#constructor is not allowed");
            }
        }
        if self.is(P::LParen) {
            // Method
            let is_ctor = !is_static && key_name.as_deref() == Some("constructor");
            if is_ctor {
                if kind != MethodKind::Method || is_async || is_gen {
                    return self.err_at(kpos, "class constructor may not be an accessor, generator or async");
                }
                if constructor.is_some() {
                    return self.err_at(kpos, "duplicate constructor");
                }
            }
            if is_static && key_name.as_deref() == Some("prototype") {
                return self.err_at(kpos, "static method named prototype");
            }
            let fkind = if is_ctor {
                FnKind::ClassConstructor
            } else {
                match kind {
                    MethodKind::Method => FnKind::Method,
                    MethodKind::Get => FnKind::Getter,
                    MethodKind::Set => FnKind::Setter,
                }
            };
            let ns = self.new_scope_id();
            let f = Rc::new(self.parse_function_rest(mstart, None, fkind, is_async, is_gen, derived && is_ctor, ns)?);
            if let PropKey::Private(n) = &key {
                let pk = match kind {
                    MethodKind::Method => PrivKind::Method,
                    MethodKind::Get => PrivKind::Get,
                    MethodKind::Set => PrivKind::Set,
                };
                self.declare_private(n, pk, is_static, kpos)?;
            }
            if is_ctor {
                *constructor = Some(f);
            } else {
                members.push(ClassMember::Method { key, func: f, kind, is_static });
            }
            return Ok(());
        }
        if is_async || is_gen || kind != MethodKind::Method {
            return self.unexpected();
        }
        // Field
        if key_name.as_deref() == Some("constructor") {
            return self.err_at(kpos, "class field named constructor");
        }
        if is_static && key_name.as_deref() == Some("prototype") {
            return self.err_at(kpos, "static field named prototype");
        }
        if let PropKey::Private(n) = &key {
            self.declare_private(n, PrivKind::Field, is_static, kpos)?;
        }
        let init = if self.eat(P::Assign)? {
            let istart = self.tok.start;
            Some(Rc::new(self.parse_field_initializer(istart)?))
        } else {
            None
        };
        self.semicolon()?;
        members.push(ClassMember::Field { key, init, is_static, span: self.span_from(start) });
        Ok(())
    }

    fn declare_private(&mut self, name: &Atom, kind: PrivKind, is_static: bool, pos: u32) -> PResult<()> {
        let c = self.classes.last_mut().unwrap();
        if let Some(existing) = c.declared.iter_mut().find(|(n, _, _)| n == name) {
            let pair = matches!((existing.1, kind), (PrivKind::Get, PrivKind::Set) | (PrivKind::Set, PrivKind::Get));
            if pair && existing.2 == is_static {
                existing.1 = PrivKind::GetSet;
                return Ok(());
            }
            return Err(ParseError { pos, msg: format!("duplicate private name #{}", name) });
        }
        c.declared.push((name.clone(), kind, is_static));
        Ok(())
    }

    fn parse_class_key(&mut self) -> PResult<PropKey> {
        if let T::Private(n) = &self.tok.t {
            let n = n.clone();
            self.advance()?;
            return Ok(PropKey::Private(n));
        }
        self.parse_prop_key()
    }

    fn parse_field_initializer(&mut self, start: u32) -> PResult<Function> {
        let saved_ctx = self.ctx.clone();
        let saved_yield = self.yield_pos.take();
        let saved_await = self.await_pos.take();
        self.ctx = Ctx {
            strict: true,
            in_function: false,
            yield_kw: false,
            await_kw: false,
            await_reserved: saved_ctx.await_kw || saved_ctx.await_reserved,
            in_params: false,
            super_prop: true,
            super_call: false,
            new_target: true,
            in_field_init: true,
            in_static_block: false,
            labels: Vec::new(),
            in_iteration: false,
            in_switch: false,
            is_arrow: false,
        };
        self.flags.push(FnFlags::default());
        let scope = self.new_scope_id();
        self.push_scope(ScopeKind::Function);
        let r = self.parse_assign(true);
        self.pop_scope();
        let fl = self.flags.pop().unwrap();
        self.ctx = saved_ctx;
        self.yield_pos = saved_yield;
        self.await_pos = saved_await;
        let e = r?;
        let body_scope = self.new_scope_id();
        Ok(Function {
            id: None,
            params: Vec::new(),
            rest: None,
            body: Vec::new(),
            expr_body: Some(Box::new(e)),
            kind: FnKind::FieldInit,
            is_async: false,
            is_generator: false,
            strict: true,
            simple_params: true,
            derived: false,
            span: self.span_from(start),
            scope,
            body_scope,
            length: 0,
            has_direct_eval: fl.has_direct_eval,
            uses_arguments: false,
            uses_this: fl.uses_this,
            uses_super: fl.uses_super,
            class_fields: false,
            name_scope: body_scope,
        })
    }

    fn parse_static_block(&mut self, start: u32) -> PResult<Function> {
        let saved_ctx = self.ctx.clone();
        self.ctx = Ctx {
            strict: true,
            in_function: false,
            yield_kw: false,
            await_kw: false,
            await_reserved: true,
            in_params: false,
            super_prop: true,
            super_call: false,
            new_target: true,
            in_field_init: true,
            in_static_block: true,
            labels: Vec::new(),
            in_iteration: false,
            in_switch: false,
            is_arrow: false,
        };
        self.flags.push(FnFlags::default());
        let scope = self.new_scope_id();
        self.push_scope(ScopeKind::Function);
        let r = (|| -> PResult<Vec<Stmt>> {
            self.expect(P::LBrace)?;
            let mut body = Vec::new();
            while !self.is(P::RBrace) {
                if self.tok.t == T::Eof {
                    return self.unexpected();
                }
                body.push(self.parse_statement_list_item()?);
            }
            self.advance()?;
            Ok(body)
        })();
        self.pop_scope();
        let fl = self.flags.pop().unwrap();
        self.ctx = saved_ctx;
        let body = r?;
        let body_scope = self.new_scope_id();
        Ok(Function {
            id: None,
            params: Vec::new(),
            rest: None,
            body,
            expr_body: None,
            kind: FnKind::StaticBlock,
            is_async: false,
            is_generator: false,
            strict: true,
            simple_params: true,
            derived: false,
            span: self.span_from(start),
            scope,
            body_scope,
            length: 0,
            has_direct_eval: fl.has_direct_eval,
            uses_arguments: false,
            uses_this: fl.uses_this,
            uses_super: fl.uses_super,
            class_fields: false,
            name_scope: body_scope,
        })
    }

    // ------------------------------------------------------------------------------------- modules

    pub(crate) fn parse_module_item(&mut self) -> PResult<Stmt> {
        if self.is_kw("import") {
            let nx = self.peek()?;
            if !matches!(nx.t, T::Punct(P::LParen) | T::Punct(P::Dot)) {
                return self.parse_import();
            }
        }
        if self.is_kw("export") {
            return self.parse_export();
        }
        self.parse_statement_list_item()
    }

    fn parse_module_export_name(&mut self) -> PResult<JsStr> {
        match &self.tok.t {
            T::Str(s) => {
                let s = s.clone();
                // Must be a well-formed Unicode string (no lone surrogates).
                if char::decode_utf16(s.units().iter().copied()).any(|r| r.is_err()) {
                    return self.err("module export name must be well-formed");
                }
                self.advance()?;
                Ok(s)
            }
            T::Name(n) => {
                let s = JsStr::from_str(n);
                self.advance()?;
                Ok(s)
            }
            _ => self.unexpected(),
        }
    }

    fn parse_from_clause(&mut self) -> PResult<(JsStr, Vec<(JsStr, JsStr)>)> {
        self.expect_kw("from")?;
        self.parse_module_specifier()
    }

    fn parse_module_specifier(&mut self) -> PResult<(JsStr, Vec<(JsStr, JsStr)>)> {
        let src = match &self.tok.t {
            T::Str(s) => s.clone(),
            _ => return self.unexpected(),
        };
        self.advance()?;
        let mut attrs = Vec::new();
        if self.is_kw("with") {
            self.advance()?;
            self.expect(P::LBrace)?;
            while !self.is(P::RBrace) {
                let key = match &self.tok.t {
                    T::Str(s) => s.clone(),
                    T::Name(n) => JsStr::from_str(n),
                    _ => return self.unexpected(),
                };
                let kpos = self.tok.start;
                self.advance()?;
                self.expect(P::Colon)?;
                let val = match &self.tok.t {
                    T::Str(s) => s.clone(),
                    _ => return self.unexpected(),
                };
                self.advance()?;
                if attrs.iter().any(|(k, _): &(JsStr, JsStr)| *k == key) {
                    return self.err_at(kpos, "duplicate import attribute");
                }
                attrs.push((key, val));
                if !self.is(P::RBrace) {
                    self.expect(P::Comma)?;
                }
            }
            self.advance()?;
        }
        Ok((src, attrs))
    }

    fn parse_import(&mut self) -> PResult<Stmt> {
        let start = self.tok.start;
        self.advance()?;
        let mut specs = Vec::new();
        if let T::Str(_) = self.tok.t {
            let (source, attributes) = self.parse_module_specifier()?;
            self.semicolon()?;
            return Ok(Stmt::Import(Box::new(ImportDecl { specs, source, attributes, span: self.span_from(start) })));
        }
        let mut need_named = true;
        if let T::Name(_) = &self.tok.t {
            // default binding
            let id = self.parse_binding_ident()?;
            specs.push(ImportSpec::Default(id));
            if !self.eat(P::Comma)? {
                need_named = false;
            }
        }
        if need_named {
            if self.eat(P::Star)? {
                self.expect_kw("as")?;
                let id = self.parse_binding_ident()?;
                specs.push(ImportSpec::Namespace(id));
            } else if self.eat(P::LBrace)? {
                while !self.is(P::RBrace) {
                    let is_str = matches!(self.tok.t, T::Str(_));
                    let npos = self.tok.start;
                    let nm_tok = self.tok.t.clone();
                    let imported = self.parse_module_export_name()?;
                    let local = if self.eat_kw("as")? {
                        self.parse_binding_ident()?
                    } else {
                        if is_str {
                            return self.unexpected();
                        }
                        let name = match nm_tok {
                            T::Name(n) => n,
                            _ => unreachable!(),
                        };
                        self.check_binding_name(&name, npos)?;
                        Ident { name, span: self.span_from(npos) }
                    };
                    specs.push(ImportSpec::Named(imported, local));
                    if !self.is(P::RBrace) {
                        self.expect(P::Comma)?;
                    }
                }
                self.advance()?;
            } else {
                return self.unexpected();
            }
        }
        let (source, attributes) = self.parse_from_clause()?;
        self.semicolon()?;
        for sp in &specs {
            let id = match sp {
                ImportSpec::Default(i) | ImportSpec::Namespace(i) | ImportSpec::Named(_, i) => i,
            };
            self.declare_lex(&id.name, id.span.start, false)?;
        }
        Ok(Stmt::Import(Box::new(ImportDecl { specs, source, attributes, span: self.span_from(start) })))
    }

    fn parse_export(&mut self) -> PResult<Stmt> {
        let start = self.tok.start;
        self.advance()?;
        if self.eat(P::Star)? {
            let exported = if self.eat_kw("as")? { Some(self.parse_module_export_name()?) } else { None };
            let (source, attributes) = self.parse_from_clause()?;
            self.semicolon()?;
            return Ok(Stmt::Export(Box::new(ExportDecl::All { exported, source, attributes, span: self.span_from(start) })));
        }
        if self.eat_kw("default")? {
            let dstart = self.tok.start;
            if self.is_kw("function") {
                self.advance()?;
                let f = self.parse_function(dstart, false, true, true)?;
                return Ok(Stmt::Export(Box::new(ExportDecl::DefaultFunction(f))));
            }
            if self.is_kw("async") {
                let nx = self.peek()?;
                if !nx.nl_before && matches!(&nx.t, T::Name(f) if &**f == "function") && !nx.escaped {
                    self.advance()?;
                    self.advance()?;
                    let f = self.parse_function(dstart, true, true, true)?;
                    return Ok(Stmt::Export(Box::new(ExportDecl::DefaultFunction(f))));
                }
            }
            if self.is_kw("class") {
                let c = self.parse_class(true, true)?;
                return Ok(Stmt::Export(Box::new(ExportDecl::DefaultClass(c))));
            }
            let e = self.parse_assign(true)?;
            self.semicolon()?;
            // `*default*` is a lexical binding of the module.
            let dname: Atom = Rc::from("*default*");
            self.declare_lex(&dname, dstart, false)?;
            return Ok(Stmt::Export(Box::new(ExportDecl::DefaultExpr(e, self.span_from(start)))));
        }
        if self.eat(P::LBrace)? {
            let mut specs = Vec::new();
            let mut string_locals = Vec::new();
            while !self.is(P::RBrace) {
                let lpos = self.tok.start;
                let was_str = matches!(self.tok.t, T::Str(_));
                let was_reserved = matches!(&self.tok.t, T::Name(n) if is_reserved(n));
                let local = self.parse_module_export_name()?;
                let exported = if self.eat_kw("as")? { self.parse_module_export_name()? } else { local.clone() };
                if was_str || was_reserved {
                    string_locals.push(lpos);
                }
                specs.push((local, exported));
                if !self.is(P::RBrace) {
                    self.expect(P::Comma)?;
                }
            }
            self.advance()?;
            let (source, attributes) = if self.is_kw("from") {
                let (s, a) = self.parse_from_clause()?;
                (Some(s), a)
            } else {
                if let Some(p) = string_locals.first() {
                    return self.err_at(*p, "string or reserved word as local export name");
                }
                (None, Vec::new())
            };
            self.semicolon()?;
            return Ok(Stmt::Export(Box::new(ExportDecl::Named { specs, source, attributes, span: self.span_from(start) })));
        }
        // export declaration
        let d = if self.is_kw("var") {
            let s = self.tok.start;
            self.advance()?;
            let d = self.parse_var_decl_rest(VarKind::Var, true, s)?;
            self.semicolon()?;
            Stmt::Var(Box::new(d))
        } else if self.is_kw("let") || self.is_kw("const") {
            let k = if self.is_kw("let") { VarKind::Let } else { VarKind::Const };
            let d = self.parse_var_decl(k, true)?;
            self.semicolon()?;
            Stmt::Var(Box::new(d))
        } else if self.is_kw("function") || self.is_kw("async") || self.is_kw("class") {
            let s = self.parse_statement_list_item()?;
            match s {
                Stmt::Function(_) | Stmt::Class(_) => s,
                _ => return self.err_at(start, "invalid export"),
            }
        } else {
            return self.unexpected();
        };
        Ok(Stmt::Export(Box::new(ExportDecl::Decl(d))))
    }
}

fn starts_element_name(t: &T) -> bool {
    matches!(t, T::Name(_) | T::Str(_) | T::Num(_) | T::BigInt(_) | T::Private(_) | T::Punct(P::LBracket))
}

fn is_labelled_function(s: &Stmt) -> bool {
    match s {
        Stmt::Labeled(_, b, _) => matches!(**b, Stmt::Function(_)) || is_labelled_function(b),
        _ => false,
    }
}
