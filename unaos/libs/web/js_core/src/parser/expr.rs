//! Expressions (§13), including the cover grammars for arrow parameters and assignment patterns.

use super::*;
use alloc::boxed::Box;

fn binop(p: P, allow_in: bool, name: Option<&str>) -> Option<(BinOrLogical, u8)> {
    use BinOrLogical::*;
    Some(match p {
        P::Nullish => (L(LogicalOp::Nullish), 1),
        P::Or => (L(LogicalOp::Or), 1),
        P::And => (L(LogicalOp::And), 2),
        P::Pipe => (B(BinOp::BitOr), 3),
        P::Caret => (B(BinOp::BitXor), 4),
        P::Amp => (B(BinOp::BitAnd), 5),
        P::Eq2 => (B(BinOp::Eq), 6),
        P::Ne => (B(BinOp::Ne), 6),
        P::Eq3 => (B(BinOp::StrictEq), 6),
        P::Ne2 => (B(BinOp::StrictNe), 6),
        P::Lt => (B(BinOp::Lt), 7),
        P::Gt => (B(BinOp::Gt), 7),
        P::Le => (B(BinOp::Le), 7),
        P::Ge => (B(BinOp::Ge), 7),
        P::Shl => (B(BinOp::Shl), 8),
        P::Shr => (B(BinOp::Shr), 8),
        P::UShr => (B(BinOp::UShr), 8),
        P::Plus => (B(BinOp::Add), 9),
        P::Minus => (B(BinOp::Sub), 9),
        P::Star => (B(BinOp::Mul), 10),
        P::Slash => (B(BinOp::Div), 10),
        P::Percent => (B(BinOp::Mod), 10),
        P::Star2 => (B(BinOp::Exp), 11),
        _ => {
            let _ = (allow_in, name);
            return None;
        }
    })
}

#[derive(Clone, Copy)]
enum BinOrLogical {
    B(BinOp),
    L(LogicalOp),
}

fn assign_op(p: P) -> Option<AssignOp> {
    Some(match p {
        P::Assign => AssignOp::Assign,
        P::PlusEq => AssignOp::Bin(BinOp::Add),
        P::MinusEq => AssignOp::Bin(BinOp::Sub),
        P::StarEq => AssignOp::Bin(BinOp::Mul),
        P::SlashEq => AssignOp::Bin(BinOp::Div),
        P::PercentEq => AssignOp::Bin(BinOp::Mod),
        P::Star2Eq => AssignOp::Bin(BinOp::Exp),
        P::ShlEq => AssignOp::Bin(BinOp::Shl),
        P::ShrEq => AssignOp::Bin(BinOp::Shr),
        P::UShrEq => AssignOp::Bin(BinOp::UShr),
        P::AmpEq => AssignOp::Bin(BinOp::BitAnd),
        P::PipeEq => AssignOp::Bin(BinOp::BitOr),
        P::CaretEq => AssignOp::Bin(BinOp::BitXor),
        P::AndEq => AssignOp::Logical(LogicalOp::And),
        P::OrEq => AssignOp::Logical(LogicalOp::Or),
        P::NullishEq => AssignOp::Logical(LogicalOp::Nullish),
        _ => return None,
    })
}

impl Parser {
    /// Expression[In] — a final expression (cover grammars resolved).
    pub(crate) fn parse_expression(&mut self, allow_in: bool) -> PResult<Expr> {
        let saved = self.cover_init.take();
        let e = self.parse_expression_inner(allow_in)?;
        if let Some(p) = self.cover_init {
            return self.err_at(p, "invalid shorthand property initializer");
        }
        self.cover_init = saved;
        Ok(e)
    }

    pub(crate) fn parse_expression_inner(&mut self, allow_in: bool) -> PResult<Expr> {
        let start = self.tok.start;
        let e = self.parse_assign(allow_in)?;
        if self.is(P::Comma) {
            let mut v = vec![e];
            while self.eat(P::Comma)? {
                v.push(self.parse_assign(allow_in)?);
            }
            return Ok(Expr::Seq(v, self.span_from(start)));
        }
        Ok(e)
    }

    pub(crate) fn final_check(&mut self) -> PResult<()> {
        if let Some(p) = self.cover_init.take() {
            return self.err_at(p, "invalid shorthand property initializer");
        }
        Ok(())
    }

    /// AssignmentExpression.
    pub(crate) fn parse_assign(&mut self, allow_in: bool) -> PResult<Expr> {
        self.depth += 1;
        if self.depth > super::MAX_NESTING {
            return self.err("expression nested too deeply");
        }
        let r = self.parse_assign_inner(allow_in);
        self.depth -= 1;
        r
    }

    fn parse_assign_inner(&mut self, allow_in: bool) -> PResult<Expr> {
        let start = self.tok.start;
        // YieldExpression
        if self.is_kw("yield") && self.ctx.yield_kw {
            return self.parse_yield(allow_in);
        }
        if self.is_name_any("yield") && self.tok.escaped && self.ctx.yield_kw {
            return self.err("keyword must not contain escapes");
        }
        // Arrow functions with a single identifier parameter, and async arrows.
        if let T::Name(n) = &self.tok.t {
            let n = n.clone();
            let escaped = self.tok.escaped;
            let nx = self.peek()?;
            if nx.t == T::Punct(P::Arrow) && !nx.nl_before && !is_reserved(&n) {
                let pos = self.tok.start;
                if &*n == "await" {
                    self.await_ident_pos = Some(pos);
                }
                self.check_binding_name(&n, pos)?;
                if &*n == "await" && self.goal == Goal::Module {
                    return self.err("await is reserved");
                }
                self.advance()?;
                let id = Ident { name: n, span: self.span_from(pos) };
                return self.parse_arrow_body(start, vec![Pat::Ident(id)], None, false, true);
            }
            if &*n == "async" && !escaped && !nx.nl_before {
                if let T::Name(pn) = &nx.t {
                    // async x => …
                    let pn = pn.clone();
                    let save_lx = self.lx.pos;
                    let save_tok = self.tok.clone();
                    let save_prev = self.prev_end;
                    self.advance()?; // async
                    let ppos = self.tok.start;
                    let nx2 = self.peek()?;
                    if nx2.t == T::Punct(P::Arrow) && !nx2.nl_before {
                        if &*pn == "await" || is_reserved(&pn) {
                            return self.err_at(ppos, "invalid async arrow parameter");
                        }
                        let saved_await = self.ctx.await_kw;
                        self.ctx.await_kw = true;
                        let r = self.check_binding_name(&pn, ppos);
                        self.ctx.await_kw = saved_await;
                        r?;
                        self.advance()?;
                        let id = Ident { name: pn, span: self.span_from(ppos) };
                        return self.parse_arrow_body(start, vec![Pat::Ident(id)], None, true, true);
                    }
                    // Not an arrow: restore.
                    self.lx.pos = save_lx;
                    self.tok = save_tok;
                    self.prev_end = save_prev;
                    self.lx.nl_before = false;
                }
            }
        }
        let outer_cover = self.cover_init.take();
        let yield_before = self.yield_pos;
        let await_before = self.await_pos;
        let await_ident_before = self.await_ident_pos;
        let e = self.parse_conditional(allow_in)?;
        let inner_cover = self.cover_init;
        // Arrow function from a parenthesised cover / async call.
        if self.is(P::Arrow) {
            if self.tok.nl_before {
                return self.err("line terminator before =>");
            }
            return self.arrow_from_cover(start, e, yield_before, await_before, await_ident_before, outer_cover);
        }
        if let T::Punct(p) = self.tok.t {
            if let Some(op) = assign_op(p) {
                let target = if op == AssignOp::Assign {
                    match e {
                        Expr::Object(..) | Expr::Array(..) => self.expr_to_assign_target(e, false)?,
                        _ => self.simple_target(e)?,
                    }
                } else {
                    if let Some(c) = inner_cover {
                        return self.err_at(c, "invalid shorthand property initializer");
                    }
                    if matches!(op, AssignOp::Logical(_)) && matches!(e.unparen(), Expr::Call(..)) {
                        return self.err("invalid assignment target");
                    }
                    self.simple_target(e)?
                };
                self.cover_init = outer_cover;
                self.advance()?;
                let rhs_outer = self.cover_init.take();
                let rhs = self.parse_assign(allow_in)?;
                if let Some(c) = self.cover_init {
                    return self.err_at(c, "invalid shorthand property initializer");
                }
                self.cover_init = rhs_outer;
                return Ok(Expr::Assign(op, Box::new(target), Box::new(rhs), self.span_from(start)));
            }
        }
        if let Some(c) = inner_cover {
            if !matches!(e, Expr::Object(..) | Expr::Array(..)) {
                return self.err_at(c, "invalid shorthand property initializer");
            }
        }
        self.cover_init = outer_cover.or(inner_cover);
        Ok(e)
    }

    fn parse_yield(&mut self, allow_in: bool) -> PResult<Expr> {
        let start = self.tok.start;
        if self.ctx.in_params {
            return self.err("yield expression in formal parameters");
        }
        self.advance()?;
        self.yield_pos = Some(start);
        let mut delegate = false;
        let arg = if self.tok.nl_before {
            None
        } else {
            if self.eat(P::Star)? {
                delegate = true;
            }
            let can_start = !matches!(
                self.tok.t,
                T::Eof | T::Punct(P::RParen) | T::Punct(P::RBracket) | T::Punct(P::RBrace) | T::Punct(P::Comma)
                    | T::Punct(P::Semi) | T::Punct(P::Colon) | T::Punct(P::Question)
            ) && !(self.is_kw("in") && false);
            if delegate || can_start && !self.is_kw("in") {
                Some(Box::new(self.parse_assign(allow_in)?))
            } else {
                None
            }
        };
        Ok(Expr::Yield(arg, delegate, self.span_from(start)))
    }

    fn arrow_from_cover(
        &mut self,
        start: u32,
        e: Expr,
        yield_before: Option<u32>,
        await_before: Option<u32>,
        await_ident_before: Option<u32>,
        outer_cover: Option<u32>,
    ) -> PResult<Expr> {
        let in_range = |p: Option<u32>, before: Option<u32>| matches!(p, Some(x) if x >= start && p != before);
        if in_range(self.yield_pos, yield_before) {
            return self.err("yield expression in arrow parameters");
        }
        if in_range(self.await_pos, await_before) {
            return self.err("await expression in arrow parameters");
        }
        let (params, rest, is_async) = match e {
            Expr::Paren(inner, sp) if sp.start == start => {
                let items = match *inner {
                    Expr::Seq(v, _) => v,
                    other => vec![other],
                };
                let (ps, r) = self.cover_items_to_params(items)?;
                (ps, r, false)
            }
            Expr::CoverParams(items, rest, sp) if sp.start == start => {
                let (mut ps, _) = self.cover_items_to_params(items)?;
                let r = match rest {
                    Some(r) => Some(self.expr_to_binding_pat(*r)?),
                    None => None,
                };
                let _ = &mut ps;
                (ps, r, false)
            }
            Expr::Call(callee, args, false, sp)
                if sp.start == start
                    && self.async_heads.contains(&sp.start)
                    && matches!(&*callee, Expr::Ident(i) if &*i.name == "async" && i.span.end - i.span.start == 5) =>
            {
                if in_range(self.await_ident_pos, await_ident_before) {
                    return self.err("await in async arrow parameters");
                }
                let mut items = Vec::new();
                let mut rest = None;
                let n = args.len();
                for (i, a) in args.into_iter().enumerate() {
                    match a {
                        Arg::Expr(x) => items.push(x),
                        Arg::Spread(x) => {
                            if i != n - 1 {
                                return self.err("rest parameter must be last");
                            }
                            rest = Some(x);
                        }
                    }
                }
                if self.async_call_trailing_comma_after_spread.contains(&sp.start) && rest.is_some() {
                    return self.err("trailing comma after rest parameter");
                }
                let saved = self.ctx.await_kw;
                self.ctx.await_kw = true;
                let r = (|| -> PResult<(Vec<Pat>, Option<Pat>)> {
                    let (ps, _) = self.cover_items_to_params(items)?;
                    let r = match rest {
                        Some(r) => {
                            let p = self.expr_to_binding_pat(r)?;
                            if matches!(p, Pat::Assign(..)) {
                                return self.err("rest parameter may not have a default");
                            }
                            Some(p)
                        }
                        None => None,
                    };
                    Ok((ps, r))
                })();
                self.ctx.await_kw = saved;
                let (ps, r) = r?;
                (ps, r, true)
            }
            _ => return self.err("invalid arrow function parameters"),
        };
        self.cover_init = outer_cover;
        self.parse_arrow_body(start, params, rest, is_async, false)
    }

    fn cover_items_to_params(&mut self, items: Vec<Expr>) -> PResult<(Vec<Pat>, Option<Pat>)> {
        let mut ps = Vec::new();
        for it in items {
            ps.push(self.expr_to_binding_pat(it)?);
        }
        Ok((ps, None))
    }

    /// Parse `=> body` (the current token is `=>`).
    pub(crate) fn parse_arrow_body(&mut self, start: u32, params: Vec<Pat>, rest: Option<Pat>, is_async: bool, _simple_form: bool) -> PResult<Expr> {
        self.expect(P::Arrow)?;
        let saved_ctx = self.ctx.clone();
        let saved_yield = self.yield_pos.take();
        let saved_await = self.await_pos.take();
        let saved_await_ident = self.await_ident_pos.take();
        self.ctx = Ctx {
            strict: saved_ctx.strict,
            in_function: true,
            yield_kw: false,
            await_kw: is_async,
            await_reserved: self.goal == Goal::Module,
            in_params: false,
            super_prop: saved_ctx.super_prop,
            super_call: saved_ctx.super_call,
            new_target: saved_ctx.new_target,
            in_field_init: saved_ctx.in_field_init,
            in_static_block: false,
            labels: Vec::new(),
            in_iteration: false,
            in_switch: false,
            is_arrow: true,
        };
        self.flags.push(FnFlags::default());
        let scope = self.new_scope_id();
        self.push_scope(ScopeKind::Function);
        let r = self.parse_arrow_body_inner(start, params, rest, is_async, scope);
        self.pop_scope();
        let fl = self.flags.pop().unwrap();
        {
            let parent = self.flag();
            parent.uses_this |= fl.uses_this;
            parent.uses_arguments |= fl.uses_arguments;
            parent.has_direct_eval |= fl.has_direct_eval;
            parent.uses_super |= fl.uses_super;
        }
        self.ctx = saved_ctx;
        self.yield_pos = saved_yield;
        self.await_pos = saved_await;
        self.await_ident_pos = saved_await_ident;
        r
    }

    fn parse_arrow_body_inner(&mut self, start: u32, params: Vec<Pat>, rest: Option<Pat>, is_async: bool, scope: ScopeId) -> PResult<Expr> {
        let mut names = Vec::new();
        for p in &params {
            p.bound_names(&mut names);
        }
        if let Some(r) = &rest {
            r.bound_names(&mut names);
        }
        for (i, n) in names.iter().enumerate() {
            self.check_binding_name(&n.name, n.span.start)?;
            if names[..i].iter().any(|m| m.name == n.name) {
                return self.err_at(n.span.start, "duplicate parameter name");
            }
        }
        let simple = rest.is_none() && params.iter().all(|p| matches!(p, Pat::Ident(_)));
        let mut length = 0;
        for p in &params {
            if matches!(p, Pat::Assign(..)) {
                break;
            }
            length += 1;
        }
        self.scopes.last_mut().unwrap().params = names.iter().map(|n| n.name.clone()).collect();
        let body_scope = self.new_scope_id();
        let was_strict = self.ctx.strict;
        let (body, expr_body) = if self.is(P::LBrace) {
            let bstart = self.tok.start;
            self.advance()?;
            let mut body = Vec::new();
            let us = self.directives(&mut body)?;
            if us && !simple {
                return self.err_at(bstart, "\"use strict\" not allowed in function with non-simple parameters");
            }
            while !self.is(P::RBrace) {
                if self.tok.t == T::Eof {
                    return self.unexpected();
                }
                body.push(self.parse_statement_list_item()?);
            }
            self.advance()?;
            (body, None)
        } else {
            let saved = self.cover_init.take();
            let e = self.parse_assign(true)?;
            self.final_check()?;
            self.cover_init = saved;
            (Vec::new(), Some(Box::new(e)))
        };
        let strict = self.ctx.strict;
        if strict && !was_strict {
            for n in &names {
                if &*n.name == "eval" || &*n.name == "arguments" || is_strict_reserved(&n.name) {
                    return self.err_at(n.span.start, "invalid parameter name in strict mode");
                }
            }
        }
        let fl = *self.flags.last().unwrap();
        Ok(Expr::Arrow(Rc::new(Function {
            id: None,
            params,
            rest,
            body,
            expr_body,
            kind: FnKind::Arrow,
            is_async,
            is_generator: false,
            strict,
            simple_params: simple,
            derived: false,
            span: self.span_from(start),
            scope,
            body_scope,
            length,
            has_direct_eval: fl.has_direct_eval,
            uses_arguments: fl.uses_arguments,
            uses_this: fl.uses_this,
            uses_super: fl.uses_super,
            class_fields: false,
            name_scope: body_scope,
        })))
    }

    fn parse_conditional(&mut self, allow_in: bool) -> PResult<Expr> {
        let start = self.tok.start;
        let test = self.parse_binary(0, allow_in)?;
        if !self.is(P::Question) {
            return Ok(test);
        }
        if let Some(c) = self.cover_init {
            return self.err_at(c, "invalid shorthand property initializer");
        }
        self.advance()?;
        let cons = self.parse_assign(true)?;
        self.final_check()?;
        self.expect(P::Colon)?;
        let alt = self.parse_assign(allow_in)?;
        self.final_check()?;
        Ok(Expr::Cond(Box::new(test), Box::new(cons), Box::new(alt), self.span_from(start)))
    }

    fn parse_binary(&mut self, min_prec: u8, allow_in: bool) -> PResult<Expr> {
        let start = self.tok.start;
        // `#x in obj`
        let mut left = if let T::Private(n) = &self.tok.t {
            let n = n.clone();
            let pos = self.tok.start;
            self.advance()?;
            if !self.is_kw("in") || !allow_in || min_prec > 7 {
                return self.err_at(pos, "unexpected private name");
            }
            self.use_private(&n, pos)?;
            self.advance()?;
            let rhs = self.parse_binary(8, allow_in)?;
            Expr::PrivateIn(n, Box::new(rhs), self.span_from(start))
        } else {
            self.parse_unary()?
        };
        loop {
            let (op, prec) = match &self.tok.t {
                T::Punct(p) => match binop(*p, allow_in, None) {
                    Some(x) => x,
                    None => break,
                },
                T::Name(n) if !self.tok.escaped && &**n == "instanceof" => (BinOrLogical::B(BinOp::InstanceOf), 7),
                T::Name(n) if !self.tok.escaped && &**n == "in" && allow_in => (BinOrLogical::B(BinOp::In), 7),
                T::Name(n) if self.tok.escaped && (&**n == "in" || &**n == "instanceof") => return self.err("keyword must not contain escapes"),
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            if prec == 11 {
                // `**`: the left operand may not be an unparenthesised UnaryExpression.
                if matches!(left, Expr::Unary(..) | Expr::Await(..)) {
                    return self.err("unparenthesized unary expression before **");
                }
            }
            if let Some(c) = self.cover_init {
                return self.err_at(c, "invalid shorthand property initializer");
            }
            self.advance()?;
            let right = if prec == 11 { self.parse_binary(11, allow_in)? } else { self.parse_binary(prec + 1, allow_in)? };
            if let Some(c) = self.cover_init {
                return self.err_at(c, "invalid shorthand property initializer");
            }
            left = match op {
                BinOrLogical::B(b) => Expr::Binary(b, Box::new(left), Box::new(right), self.span_from(start)),
                BinOrLogical::L(l) => {
                    // `??` may not be mixed with `||` / `&&` without parentheses.
                    let mixes = |e: &Expr, l: LogicalOp| -> bool {
                        match e {
                            Expr::Logical(o, ..) => (l == LogicalOp::Nullish) != (*o == LogicalOp::Nullish),
                            _ => false,
                        }
                    };
                    if mixes(&left, l) || mixes(&right, l) {
                        return self.err("cannot mix ?? with || or && without parentheses");
                    }
                    Expr::Logical(l, Box::new(left), Box::new(right), self.span_from(start))
                }
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        self.depth += 1;
        if self.depth > super::MAX_NESTING {
            return self.err("expression nested too deeply");
        }
        let r = self.parse_unary_inner();
        self.depth -= 1;
        r
    }

    fn parse_unary_inner(&mut self) -> PResult<Expr> {
        let start = self.tok.start;
        let op = match &self.tok.t {
            T::Punct(P::Bang) => Some(UnaryOp::Not),
            T::Punct(P::Tilde) => Some(UnaryOp::BitNot),
            T::Punct(P::Plus) => Some(UnaryOp::Plus),
            T::Punct(P::Minus) => Some(UnaryOp::Minus),
            T::Name(n) if !self.tok.escaped => match &**n {
                "typeof" => Some(UnaryOp::Typeof),
                "void" => Some(UnaryOp::Void),
                "delete" => Some(UnaryOp::Delete),
                _ => None,
            },
            _ => None,
        };
        if let Some(op) = op {
            self.advance()?;
            let arg = self.parse_unary()?;
            if let Some(c) = self.cover_init {
                return self.err_at(c, "invalid shorthand property initializer");
            }
            if op == UnaryOp::Delete {
                match arg.unparen() {
                    Expr::Ident(_) if self.ctx.strict => return self.err_at(start, "delete of an unqualified identifier in strict mode"),
                    Expr::Member(_, p, _, _) if matches!(**p, MemberProp::Private(_)) => return self.err_at(start, "private fields cannot be deleted"),
                    Expr::Chain(inner, _) => {
                        if let Expr::Member(_, p, _, _) = &**inner {
                            if matches!(**p, MemberProp::Private(_)) {
                                return self.err_at(start, "private fields cannot be deleted");
                            }
                        }
                    }
                    _ => {}
                }
            }
            return Ok(Expr::Unary(op, Box::new(arg), self.span_from(start)));
        }
        if self.is(P::Inc) || self.is(P::Dec) {
            let inc = self.is(P::Inc);
            self.advance()?;
            let arg = self.parse_unary()?;
            let t = self.simple_target(arg)?;
            let arg = match t {
                Pat::Ident(i) => Expr::Ident(i),
                Pat::Expr(e) => *e,
                _ => unreachable!(),
            };
            return Ok(Expr::Update(inc, true, Box::new(arg), self.span_from(start)));
        }
        if self.is_kw("await") && (self.ctx.await_kw || (self.goal == Goal::Module && !self.ctx.in_function && !self.ctx.in_field_init)) {
            if self.ctx.in_params {
                return self.err("await expression in formal parameters");
            }
            if self.ctx.in_field_init || self.ctx.in_static_block {
                return self.err("await is not allowed here");
            }
            self.advance()?;
            self.await_pos = Some(start);
            if !self.ctx.in_function {
                self.has_top_await = true;
            }
            let arg = self.parse_unary()?;
            return Ok(Expr::Await(Box::new(arg), self.span_from(start)));
        }
        if self.is_name_any("await") && self.tok.escaped && (self.ctx.await_kw || self.ctx.await_reserved) {
            return self.err("keyword must not contain escapes");
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let start = self.tok.start;
        let e = self.parse_lhs_expression()?;
        if (self.is(P::Inc) || self.is(P::Dec)) && !self.tok.nl_before {
            let inc = self.is(P::Inc);
            let t = self.simple_target(e)?;
            self.advance()?;
            let arg = match t {
                Pat::Ident(i) => Expr::Ident(i),
                Pat::Expr(e) => *e,
                _ => unreachable!(),
            };
            return Ok(Expr::Update(inc, false, Box::new(arg), self.span_from(start)));
        }
        Ok(e)
    }

    /// A simple assignment target (§13.15.1 AssignmentTargetType "simple").
    pub(crate) fn simple_target(&mut self, e: Expr) -> PResult<Pat> {
        let sp = e.span();
        match e {
            Expr::Ident(i) => {
                if self.ctx.strict && (&*i.name == "eval" || &*i.name == "arguments") {
                    return self.err_at(sp.start, "invalid assignment target in strict mode");
                }
                Ok(Pat::Ident(i))
            }
            Expr::Member(_, _, false, _) | Expr::SuperMember(..) => Ok(Pat::Expr(Box::new(e))),
            // Annex B: a function call as an assignment target in sloppy code (a runtime ReferenceError).
            Expr::Call(_, _, false, _) if !self.ctx.strict => Ok(Pat::Expr(Box::new(e))),
            Expr::Paren(inner, _) => match *inner {
                x @ (Expr::Ident(_) | Expr::Member(_, _, false, _) | Expr::SuperMember(..) | Expr::Paren(..)) => self.simple_target(x),
                x @ Expr::Call(_, _, false, _) if !self.ctx.strict => self.simple_target(x),
                _ => self.err_at(sp.start, "invalid assignment target"),
            },
            _ => self.err_at(sp.start, "invalid assignment target"),
        }
    }

    /// Convert an expression to an assignment pattern (`[a, b] = …`, `for ([a] of …)`).
    pub(crate) fn expr_to_assign_target(&mut self, e: Expr, for_in_of: bool) -> PResult<Pat> {
        let sp = e.span();
        match e {
            Expr::Object(props, sp) => {
                let mut out = Vec::new();
                let mut rest = None;
                let n = props.len();
                for (i, p) in props.into_iter().enumerate() {
                    match p {
                        Prop::KeyValue(k, v) => out.push(PatProp { key: k, value: self.assign_elem(v)? }),
                        Prop::Proto(v, _) => out.push(PatProp { key: PropKey::Name(Rc::from("__proto__")), value: self.assign_elem(v)? }),
                        Prop::Shorthand(id) => {
                            self.check_assign_ident(&id)?;
                            out.push(PatProp { key: PropKey::Name(id.name.clone()), value: Pat::Ident(id) })
                        }
                        Prop::CoverInit(id, d) => {
                            self.check_assign_ident(&id)?;
                            let isp = id.span;
                            out.push(PatProp { key: PropKey::Name(id.name.clone()), value: Pat::Assign(Box::new(Pat::Ident(id)), Box::new(d), isp) })
                        }
                        Prop::Spread(x) => {
                            if i != n - 1 || self.object_trailing_comma_after_spread.contains(&sp.start) {
                                return self.err_at(sp.start, "rest element must be last");
                            }
                            let t = self.simple_target(x)?;
                            rest = Some(Box::new(t));
                        }
                        Prop::Method(..) => return self.err_at(sp.start, "invalid destructuring target"),
                    }
                }
                Ok(Pat::Object(out, rest, sp))
            }
            Expr::Array(elems, sp) => {
                let mut out = Vec::new();
                let mut rest = None;
                let n = elems.len();
                for (i, el) in elems.into_iter().enumerate() {
                    match el {
                        ArrayElem::Hole => out.push(None),
                        ArrayElem::Expr(x) => out.push(Some(self.assign_elem(x)?)),
                        ArrayElem::Spread(x) => {
                            if i != n - 1 || self.array_trailing_comma_after_spread.contains(&sp.start) {
                                return self.err_at(sp.start, "rest element must be last");
                            }
                            if matches!(x, Expr::Assign(..)) {
                                return self.err_at(sp.start, "rest element may not have a default");
                            }
                            rest = Some(Box::new(self.assign_elem(x)?));
                        }
                    }
                }
                Ok(Pat::Array(out, rest, sp))
            }
            _ => {
                let _ = for_in_of;
                let _ = sp;
                self.simple_target(e)
            }
        }
    }

    fn check_assign_ident(&self, id: &Ident) -> PResult<()> {
        if self.ctx.strict && (&*id.name == "eval" || &*id.name == "arguments") {
            return self.err_at(id.span.start, "invalid assignment target in strict mode");
        }
        self.check_ident_name(&id.name, id.span.start)
    }

    /// An element of an assignment pattern: nested pattern, simple target, or target = default.
    fn assign_elem(&mut self, e: Expr) -> PResult<Pat> {
        match e {
            Expr::Assign(AssignOp::Assign, target, value, sp) => Ok(Pat::Assign(target, value, sp)),
            Expr::Object(..) | Expr::Array(..) => self.expr_to_assign_target(e, false),
            _ => self.simple_target(e),
        }
    }

    /// Convert a cover expression to a binding pattern (arrow parameters).
    pub(crate) fn expr_to_binding_pat(&mut self, e: Expr) -> PResult<Pat> {
        let sp = e.span();
        match e {
            Expr::Ident(i) => Ok(Pat::Ident(i)),
            Expr::Assign(AssignOp::Assign, target, value, sp) => {
                let t = self.pat_to_binding(*target)?;
                Ok(Pat::Assign(Box::new(t), value, sp))
            }
            Expr::Object(..) | Expr::Array(..) => {
                let p = self.expr_to_assign_target(e, false)?;
                self.pat_to_binding(p)
            }
            Expr::CoverPat(p) => self.pat_to_binding(*p),
            _ => self.err_at(sp.start, "invalid parameter"),
        }
    }

    /// Re-check an assignment pattern as a binding pattern (no member targets).
    fn pat_to_binding(&mut self, p: Pat) -> PResult<Pat> {
        match p {
            Pat::Ident(i) => Ok(Pat::Ident(i)),
            Pat::Expr(e) => self.err_at(e.span().start, "invalid binding pattern"),
            Pat::Object(props, rest, sp) => {
                let mut out = Vec::new();
                for pp in props {
                    out.push(PatProp { key: pp.key, value: self.pat_to_binding(pp.value)? });
                }
                let rest = match rest {
                    Some(r) => match *r {
                        Pat::Ident(i) => Some(Box::new(Pat::Ident(i))),
                        other => return self.err_at(other.span().start, "invalid rest binding"),
                    },
                    None => None,
                };
                Ok(Pat::Object(out, rest, sp))
            }
            Pat::Array(elems, rest, sp) => {
                let mut out = Vec::new();
                for el in elems {
                    out.push(match el {
                        Some(p) => Some(self.pat_to_binding(p)?),
                        None => None,
                    });
                }
                let rest = match rest {
                    Some(r) => Some(Box::new(self.pat_to_binding(*r)?)),
                    None => None,
                };
                Ok(Pat::Array(out, rest, sp))
            }
            Pat::Assign(t, d, sp) => {
                let t = self.pat_to_binding(*t)?;
                Ok(Pat::Assign(Box::new(t), d, sp))
            }
        }
    }

    // ------------------------------------------------------------------------------------- LHS / calls

    /// LeftHandSideExpression (calls, members, new, optional chains).
    pub(crate) fn parse_lhs_expression(&mut self) -> PResult<Expr> {
        let start = self.tok.start;
        let mut e = if self.is_kw("new") {
            self.parse_new()?
        } else if self.is_kw("super") {
            self.parse_super()?
        } else if self.is_kw("import") {
            self.parse_import_expr()?
        } else {
            self.parse_primary()?
        };
        if matches!(e, Expr::Arrow(_) | Expr::CoverParams(..)) {
            return Ok(e);
        }
        let mut in_chain = false;
        loop {
            match &self.tok.t {
                T::Punct(P::Dot) => {
                    self.advance()?;
                    let prop = self.parse_member_name()?;
                    e = Expr::Member(Box::new(e), Box::new(prop), false, self.span_from(start));
                }
                T::Punct(P::QDot) => {
                    self.advance()?;
                    in_chain = true;
                    match &self.tok.t {
                        T::Punct(P::LParen) => {
                            let args = self.parse_arguments()?;
                            self.final_check()?;
                            e = Expr::Call(Box::new(e), args, true, self.span_from(start));
                        }
                        T::Punct(P::LBracket) => {
                            self.advance()?;
                            let p = self.parse_expression(true)?;
                            self.expect(P::RBracket)?;
                            e = Expr::Member(Box::new(e), Box::new(MemberProp::Computed(p)), true, self.span_from(start));
                        }
                        T::Template { .. } => return self.err("tagged template in optional chain"),
                        _ => {
                            let prop = self.parse_member_name()?;
                            e = Expr::Member(Box::new(e), Box::new(prop), true, self.span_from(start));
                        }
                    }
                }
                T::Punct(P::LBracket) => {
                    self.advance()?;
                    let p = self.parse_expression(true)?;
                    self.expect(P::RBracket)?;
                    e = Expr::Member(Box::new(e), Box::new(MemberProp::Computed(p)), false, self.span_from(start));
                }
                T::Punct(P::LParen) => {
                    let is_async_head = matches!(&e, Expr::Ident(i) if &*i.name == "async" && i.span.end - i.span.start == 5) && !self.tok.nl_before && !in_chain;
                    let direct_eval = matches!(&e, Expr::Ident(i) if &*i.name == "eval");
                    let args = if is_async_head { self.parse_arguments_cover(start)? } else { self.parse_arguments()? };
                    if !is_async_head || !self.is(P::Arrow) {
                        self.final_check()?;
                    }
                    if direct_eval {
                        self.flag().has_direct_eval = true;
                    }
                    e = Expr::Call(Box::new(e), args, false, self.span_from(start));
                    if is_async_head && self.is(P::Arrow) {
                        return Ok(e);
                    }
                }
                T::Template { .. } => {
                    if in_chain {
                        return self.err("tagged template in optional chain");
                    }
                    let t = self.parse_template(true)?;
                    let site = self.site_counter;
                    self.site_counter += 1;
                    e = Expr::TaggedTemplate(Box::new(e), Rc::new(t), site, self.span_from(start));
                }
                _ => break,
            }
        }
        if in_chain {
            e = Expr::Chain(Box::new(e), self.span_from(start));
        }
        Ok(e)
    }

    fn parse_member_name(&mut self) -> PResult<MemberProp> {
        match &self.tok.t {
            T::Name(n) => {
                let n = n.clone();
                self.advance()?;
                Ok(MemberProp::Name(n))
            }
            T::Private(n) => {
                let n = n.clone();
                let pos = self.tok.start;
                self.use_private(&n, pos)?;
                self.advance()?;
                Ok(MemberProp::Private(n))
            }
            _ => self.unexpected(),
        }
    }

    fn parse_new(&mut self) -> PResult<Expr> {
        let start = self.tok.start;
        self.advance()?; // new
        if self.is(P::Dot) {
            self.advance()?;
            if self.is_kw("target") {
                if !self.ctx.new_target {
                    return self.err("new.target outside of function");
                }
                self.advance()?;
                return Ok(Expr::NewTarget(self.span_from(start)));
            }
            return self.unexpected();
        }
        let cstart = self.tok.start;
        let mut callee = if self.is_kw("new") {
            self.parse_new()?
        } else if self.is_kw("super") {
            self.parse_super()?
        } else if self.is_kw("import") {
            let nx = self.peek()?;
            if nx.t == T::Punct(P::LParen) {
                return self.err("cannot use new with import()");
            }
            self.parse_import_expr()?
        } else {
            self.parse_primary()?
        };
        if matches!(callee, Expr::Arrow(_) | Expr::CoverParams(..)) {
            return self.err("invalid new expression");
        }
        if let Expr::SuperCall(..) = callee {
            return self.err("invalid new expression");
        }
        loop {
            match &self.tok.t {
                T::Punct(P::Dot) => {
                    self.advance()?;
                    let prop = self.parse_member_name()?;
                    callee = Expr::Member(Box::new(callee), Box::new(prop), false, self.span_from(cstart));
                }
                T::Punct(P::LBracket) => {
                    self.advance()?;
                    let p = self.parse_expression(true)?;
                    self.expect(P::RBracket)?;
                    callee = Expr::Member(Box::new(callee), Box::new(MemberProp::Computed(p)), false, self.span_from(cstart));
                }
                T::Template { .. } => {
                    let t = self.parse_template(true)?;
                    let site = self.site_counter;
                    self.site_counter += 1;
                    callee = Expr::TaggedTemplate(Box::new(callee), Rc::new(t), site, self.span_from(cstart));
                }
                T::Punct(P::QDot) => return self.err("optional chain in new expression"),
                _ => break,
            }
        }
        let args = if self.is(P::LParen) {
            let a = self.parse_arguments()?;
            self.final_check()?;
            a
        } else {
            Vec::new()
        };
        Ok(Expr::New(Box::new(callee), args, self.span_from(start)))
    }

    fn parse_super(&mut self) -> PResult<Expr> {
        let start = self.tok.start;
        self.advance()?;
        match &self.tok.t {
            T::Punct(P::LParen) => {
                if !self.ctx.super_call {
                    return self.err_at(start, "'super' call outside of a derived constructor");
                }
                let args = self.parse_arguments()?;
                self.final_check()?;
                self.flag().uses_super = true;
                self.flag().uses_this = true;
                Ok(Expr::SuperCall(args, self.span_from(start)))
            }
            T::Punct(P::Dot) | T::Punct(P::LBracket) => {
                if !self.ctx.super_prop {
                    return self.err_at(start, "'super' keyword unexpected here");
                }
                self.flag().uses_super = true;
                self.flag().uses_this = true;
                let prop = if self.eat(P::Dot)? {
                    match &self.tok.t {
                        T::Name(n) => {
                            let n = n.clone();
                            self.advance()?;
                            MemberProp::Name(n)
                        }
                        _ => return self.unexpected(),
                    }
                } else {
                    self.advance()?;
                    let p = self.parse_expression(true)?;
                    self.expect(P::RBracket)?;
                    MemberProp::Computed(p)
                };
                Ok(Expr::SuperMember(Box::new(prop), self.span_from(start)))
            }
            _ => self.err_at(start, "'super' keyword unexpected here"),
        }
    }

    fn parse_import_expr(&mut self) -> PResult<Expr> {
        let start = self.tok.start;
        self.advance()?; // import
        if self.eat(P::Dot)? {
            if self.is_kw("meta") {
                if self.goal != Goal::Module {
                    return self.err("import.meta is only valid in modules");
                }
                self.advance()?;
                return Ok(Expr::ImportMeta(self.span_from(start)));
            }
            return self.unexpected();
        }
        self.expect(P::LParen)?;
        if self.is(P::RParen) || self.is(P::Ellipsis) {
            return self.unexpected();
        }
        let spec = self.parse_assign(true)?;
        self.final_check()?;
        let mut opts = None;
        if self.eat(P::Comma)? && !self.is(P::RParen) {
            let o = self.parse_assign(true)?;
            self.final_check()?;
            opts = Some(Box::new(o));
            self.eat(P::Comma)?;
        }
        self.expect(P::RParen)?;
        Ok(Expr::ImportCall(Box::new(spec), opts, self.span_from(start)))
    }

    pub(crate) fn parse_arguments(&mut self) -> PResult<Vec<Arg>> {
        self.expect(P::LParen)?;
        let mut args = Vec::new();
        while !self.is(P::RParen) {
            if self.eat(P::Ellipsis)? {
                args.push(Arg::Spread(self.parse_assign(true)?));
            } else {
                args.push(Arg::Expr(self.parse_assign(true)?));
            }
            if !self.is(P::RParen) {
                self.expect(P::Comma)?;
            }
        }
        self.advance()?;
        Ok(args)
    }

    /// Arguments of a possible async arrow head `async( … )`: like arguments, but records a trailing comma
    /// after a spread (invalid as parameters).
    fn parse_arguments_cover(&mut self, start: u32) -> PResult<Vec<Arg>> {
        self.async_heads.push(start);
        self.expect(P::LParen)?;
        let mut args = Vec::new();
        while !self.is(P::RParen) {
            let spread = self.eat(P::Ellipsis)?;
            let e = self.parse_assign(true)?;
            args.push(if spread { Arg::Spread(e) } else { Arg::Expr(e) });
            if !self.is(P::RParen) {
                self.expect(P::Comma)?;
                if spread && self.is(P::RParen) {
                    self.async_call_trailing_comma_after_spread.push(start);
                }
            }
        }
        self.advance()?;
        Ok(args)
    }

    // ------------------------------------------------------------------------------------- primary

    fn parse_primary(&mut self) -> PResult<Expr> {
        let start = self.tok.start;
        match self.tok.t.clone() {
            T::Num(v) => {
                if self.tok.legacy_octal && self.ctx.strict {
                    return self.err("octal literals are not allowed in strict mode");
                }
                self.advance()?;
                Ok(Expr::Num(v, self.span_from(start)))
            }
            T::BigInt(d) => {
                self.advance()?;
                Ok(Expr::BigInt(d, self.span_from(start)))
            }
            T::Str(s) => {
                if self.tok.legacy_octal && self.ctx.strict {
                    return self.err("octal escape sequences are not allowed in strict mode");
                }
                self.advance()?;
                Ok(Expr::Str(s, self.span_from(start)))
            }
            T::Template { .. } => {
                let t = self.parse_template(false)?;
                Ok(Expr::Template(Box::new(t)))
            }
            T::Punct(P::Slash) | T::Punct(P::SlashEq) => {
                let t = self.lx.rescan_regex(start)?;
                let nl = self.tok.nl_before;
                self.tok = t;
                self.tok.nl_before = nl;
                let (body, flags) = match &self.tok.t {
                    T::Regex { body, flags } => (body.clone(), flags.clone()),
                    _ => unreachable!(),
                };
                if let Err(m) = crate::regexp::validate(body.units(), flags.units()) {
                    return self.err_at(start, &format!("invalid regular expression: {}", m));
                }
                self.advance()?;
                Ok(Expr::Regex(body, flags, self.span_from(start)))
            }
            T::Punct(P::LParen) => self.parse_paren(),
            T::Punct(P::LBracket) => self.parse_array_literal(),
            T::Punct(P::LBrace) => self.parse_object_literal(),
            T::Private(_) => self.unexpected(),
            T::Name(n) => {
                let escaped = self.tok.escaped;
                if !escaped {
                    match &*n {
                        "this" => {
                            self.advance()?;
                            self.flag().uses_this = true;
                            return Ok(Expr::This(self.span_from(start)));
                        }
                        "null" => {
                            self.advance()?;
                            return Ok(Expr::Null(self.span_from(start)));
                        }
                        "true" | "false" => {
                            self.advance()?;
                            return Ok(Expr::Bool(&*n == "true", self.span_from(start)));
                        }
                        "function" => {
                            self.advance()?;
                            let f = self.parse_function(start, false, false, false)?;
                            return Ok(Expr::Function(f));
                        }
                        "class" => {
                            let c = self.parse_class(false, false)?;
                            return Ok(Expr::Class(c));
                        }
                        "async" => {
                            let nx = self.peek()?;
                            if !nx.nl_before && matches!(&nx.t, T::Name(f) if &**f == "function") && !nx.escaped {
                                self.advance()?;
                                self.advance()?;
                                let f = self.parse_function(start, true, false, false)?;
                                return Ok(Expr::Function(f));
                            }
                        }
                        "new" | "super" | "import" => return self.parse_lhs_expression(),
                        _ => {}
                    }
                }
                // IdentifierReference
                if escaped && is_reserved(&n) {
                    return self.err("keyword must not contain escapes");
                }
                self.check_ident_name(&n, start)?;
                if &*n == "await" {
                    self.await_ident_pos = Some(start);
                }
                if &*n == "arguments" {
                    if self.ctx.in_field_init {
                        return self.err("'arguments' is not allowed in class field initializer or static block");
                    }
                    self.flag().uses_arguments = true;
                }
                self.advance()?;
                Ok(Expr::Ident(Ident { name: n, span: self.span_from(start) }))
            }
            _ => self.unexpected(),
        }
    }

    /// `( … )`: a parenthesised expression or the cover of arrow parameters.
    fn parse_paren(&mut self) -> PResult<Expr> {
        let start = self.tok.start;
        self.advance()?;
        if self.is(P::RParen) {
            self.advance()?;
            if !self.is(P::Arrow) || self.tok.nl_before {
                return self.unexpected();
            }
            return Ok(Expr::CoverParams(Vec::new(), None, self.span_from(start)));
        }
        let mut items = Vec::new();
        let mut rest = None;
        let mut trailing_comma = false;
        loop {
            if self.is(P::Ellipsis) {
                self.advance()?;
                let t = self.parse_binding_target()?;
                // Represent the rest target as an expression-like pattern holder.
                rest = Some(Box::new(pat_to_cover_expr(t)));
                if !self.is(P::RParen) {
                    return self.err("rest parameter must be last");
                }
                break;
            }
            items.push(self.parse_assign(true)?);
            if self.is(P::Comma) {
                self.advance()?;
                if self.is(P::RParen) {
                    trailing_comma = true;
                    break;
                }
                continue;
            }
            break;
        }
        self.expect(P::RParen)?;
        if rest.is_some() || trailing_comma {
            if !self.is(P::Arrow) || self.tok.nl_before {
                return self.unexpected();
            }
            return Ok(Expr::CoverParams(items, rest, self.span_from(start)));
        }
        if !self.is(P::Arrow) {
            // A parenthesised expression: cover initialisers inside are errors now.
            self.final_check()?;
        }
        let sp = self.span_from(start);
        let inner = if items.len() == 1 { items.pop().unwrap() } else { Expr::Seq(items, Span { start: start + 1, end: sp.end - 1 }) };
        Ok(Expr::Paren(Box::new(inner), sp))
    }

    fn parse_array_literal(&mut self) -> PResult<Expr> {
        let start = self.tok.start;
        self.advance()?;
        let mut elems = Vec::new();
        loop {
            if self.eat(P::RBracket)? {
                break;
            }
            if self.is(P::Comma) {
                self.advance()?;
                elems.push(ArrayElem::Hole);
                continue;
            }
            let el = if self.eat(P::Ellipsis)? {
                let e = self.parse_assign(true)?;
                if self.is(P::Comma) {
                    // A trailing comma after a spread makes the literal invalid as a pattern.
                    let nx = self.peek()?;
                    let _ = nx;
                    self.array_trailing_comma_after_spread.push(start);
                }
                ArrayElem::Spread(e)
            } else {
                ArrayElem::Expr(self.parse_assign(true)?)
            };
            elems.push(el);
            if !self.is(P::RBracket) {
                self.expect(P::Comma)?;
            }
        }
        Ok(Expr::Array(elems, self.span_from(start)))
    }

    pub(crate) fn parse_prop_key(&mut self) -> PResult<PropKey> {
        let k = match self.tok.t.clone() {
            T::Name(n) => PropKey::Name(n),
            T::Str(s) => {
                if self.tok.legacy_octal && self.ctx.strict {
                    return self.err("octal escape sequences are not allowed in strict mode");
                }
                PropKey::Str(s)
            }
            T::Num(v) => {
                if self.tok.legacy_octal && self.ctx.strict {
                    return self.err("octal literals are not allowed in strict mode");
                }
                PropKey::Num(v)
            }
            T::BigInt(d) => PropKey::BigInt(d),
            T::Private(n) => {
                self.advance()?;
                return Ok(PropKey::Private(n));
            }
            T::Punct(P::LBracket) => {
                self.advance()?;
                let saved = self.cover_init.take();
                let e = self.parse_assign(true)?;
                self.final_check()?;
                self.cover_init = saved;
                self.expect(P::RBracket)?;
                return Ok(PropKey::Computed(Rc::new(e)));
            }
            _ => return self.unexpected(),
        };
        self.advance()?;
        Ok(k)
    }

    fn parse_object_literal(&mut self) -> PResult<Expr> {
        let start = self.tok.start;
        self.advance()?;
        let mut props = Vec::new();
        let mut proto_seen: Option<u32> = None;
        while !self.is(P::RBrace) {
            let pstart = self.tok.start;
            if self.eat(P::Ellipsis)? {
                let e = self.parse_assign(true)?;
                props.push(Prop::Spread(e));
                if self.is(P::Comma) {
                    self.object_trailing_comma_after_spread.push(start);
                }
                if !self.is(P::RBrace) {
                    self.expect(P::Comma)?;
                }
                continue;
            }
            let mut is_async = false;
            let mut is_gen = false;
            let mut kind = MethodKind::Method;
            if self.is_kw("async") {
                let nx = self.peek()?;
                if !nx.nl_before && !matches!(nx.t, T::Punct(P::LParen) | T::Punct(P::Colon) | T::Punct(P::Comma) | T::Punct(P::RBrace) | T::Punct(P::Assign)) {
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
                if !matches!(nx.t, T::Punct(P::LParen) | T::Punct(P::Colon) | T::Punct(P::Comma) | T::Punct(P::RBrace) | T::Punct(P::Assign)) {
                    kind = if self.is_kw("get") { MethodKind::Get } else { MethodKind::Set };
                    self.advance()?;
                }
            }
            let key_tok = self.tok.clone();
            let key = self.parse_prop_key()?;
            if let PropKey::Private(_) = key {
                return self.err_at(pstart, "unexpected private name");
            }
            if self.is(P::LParen) {
                let fkind = match kind {
                    MethodKind::Method => FnKind::Method,
                    MethodKind::Get => FnKind::Getter,
                    MethodKind::Set => FnKind::Setter,
                };
                let ns = self.new_scope_id();
                let f = self.parse_function_rest(pstart, None, fkind, is_async, is_gen, false, ns)?;
                props.push(Prop::Method(key, Rc::new(f), kind));
            } else {
                if is_async || is_gen || kind != MethodKind::Method {
                    return self.unexpected();
                }
                if self.eat(P::Colon)? {
                    let v = self.parse_assign(true)?;
                    let is_proto = match &key {
                        PropKey::Name(n) => &**n == "__proto__",
                        PropKey::Str(s) => s.eq_str("__proto__"),
                        _ => false,
                    };
                    if is_proto {
                        if proto_seen.is_some() && self.cover_init.is_none() {
                            // Duplicate __proto__ is valid only in an assignment pattern.
                            self.cover_init = Some(pstart);
                        }
                        proto_seen = Some(pstart);
                        props.push(Prop::Proto(v, self.span_from(pstart)));
                    } else {
                        props.push(Prop::KeyValue(key, v));
                    }
                } else {
                    // Shorthand / CoverInitializedName
                    let name = match (&key, &key_tok.t) {
                        (PropKey::Name(n), T::Name(_)) => n.clone(),
                        _ => return self.unexpected(),
                    };
                    if key_tok.escaped && is_reserved(&name) {
                        return self.err_at(pstart, "keyword must not contain escapes");
                    }
                    self.check_ident_name(&name, pstart)?;
                    if &*name == "await" {
                        self.await_ident_pos = Some(pstart);
                    }
                    if &*name == "arguments" {
                        if self.ctx.in_field_init {
                            return self.err("'arguments' is not allowed in class field initializer or static block");
                        }
                        self.flag().uses_arguments = true;
                    }
                    let id = Ident { name, span: Span { start: key_tok.start, end: key_tok.end } };
                    if self.is(P::Assign) {
                        self.advance()?;
                        let d = self.parse_assign(true)?;
                        if self.cover_init.is_none() {
                            self.cover_init = Some(pstart);
                        }
                        props.push(Prop::CoverInit(id, d));
                    } else {
                        props.push(Prop::Shorthand(id));
                    }
                }
            }
            if !self.is(P::RBrace) {
                self.expect(P::Comma)?;
            }
        }
        self.advance()?;
        Ok(Expr::Object(props, self.span_from(start)))
    }

    /// Template literal; the current token is the first template piece.
    fn parse_template(&mut self, tagged: bool) -> PResult<TemplateLit> {
        let start = self.tok.start;
        let mut quasis = Vec::new();
        let mut exprs = Vec::new();
        loop {
            let (cooked, raw, tail) = match &self.tok.t {
                T::Template { cooked, raw, tail } => (cooked.clone(), raw.clone(), *tail),
                _ => return self.unexpected(),
            };
            if cooked.is_none() && !tagged {
                return self.err("invalid escape sequence in template literal");
            }
            quasis.push((cooked, raw));
            if tail {
                self.advance()?;
                break;
            }
            self.advance()?;
            let e = self.parse_expression(true)?;
            exprs.push(e);
            if !self.is(P::RBrace) {
                return self.unexpected();
            }
            let t = self.lx.rescan_template(self.tok.start)?;
            self.tok = t;
        }
        Ok(TemplateLit { quasis, exprs, span: self.span_from(start) })
    }
}

/// Wrap a binding pattern (an arrow rest parameter parsed directly as a pattern) for the cover AST.
fn pat_to_cover_expr(p: Pat) -> Expr {
    Expr::CoverPat(Box::new(p))
}
