//! A recursive-descent parser for the GLSL ES 1.00 grammar (GLSL ES §9), with the expression precedence of
//! §5.1. The reserved operators (%, <<, >>, &, |, ^, ~ and their assignment forms, §5.1) and reserved
//! keywords (§3.6) are errors here. Struct type names are scoped as the grammar's TYPE_NAME requires: the
//! parser keeps its own scope stack of struct names so `S s;` (a declaration) and `S(…)` (a constructor)
//! parse as the specification says.

use super::ast::*;
use super::pp::{Tok, Token};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

pub type PResult<T> = Result<T, (u32, String)>;

/// GLSL ES 1.00 §3.6 reserved keywords (any use is an error).
const RESERVED: &[&str] = &[
    "asm", "class", "union", "enum", "typedef", "template", "this", "packed", "goto", "switch", "default", "inline", "noinline", "volatile", "public",
    "static", "extern", "external", "interface", "flat", "long", "short", "double", "half", "fixed", "unsigned", "superp", "input", "output", "hvec2",
    "hvec3", "hvec4", "dvec2", "dvec3", "dvec4", "fvec2", "fvec3", "fvec4", "sampler1D", "sampler3D", "sampler1DShadow", "sampler2DShadow",
    "sampler2DRect", "sampler3DRect", "sampler2DRectShadow", "sizeof", "cast", "namespace", "using",
];

/// Keywords of §3.6 that are not types (an identifier may not be one).
const KEYWORDS: &[&str] = &[
    "attribute", "const", "uniform", "varying", "break", "continue", "do", "for", "while", "if", "else", "in", "out", "inout", "true", "false",
    "lowp", "mediump", "highp", "precision", "invariant", "discard", "return", "struct",
];

pub fn builtin_type(name: &str) -> Option<Ty> {
    Some(match name {
        "void" => Ty::VOID,
        "float" => Ty::FLOAT,
        "int" => Ty::INT,
        "bool" => Ty::BOOL,
        "vec2" => Ty::vec(Base::Float, 2),
        "vec3" => Ty::vec(Base::Float, 3),
        "vec4" => Ty::vec(Base::Float, 4),
        "ivec2" => Ty::vec(Base::Int, 2),
        "ivec3" => Ty::vec(Base::Int, 3),
        "ivec4" => Ty::vec(Base::Int, 4),
        "bvec2" => Ty::vec(Base::Bool, 2),
        "bvec3" => Ty::vec(Base::Bool, 3),
        "bvec4" => Ty::vec(Base::Bool, 4),
        "mat2" => Ty::mat(2),
        "mat3" => Ty::mat(3),
        "mat4" => Ty::mat(4),
        "sampler2D" => Ty { base: Base::Sampler2D, rows: 1, cols: 1, arr: 0 },
        "samplerCube" => Ty { base: Base::SamplerCube, rows: 1, cols: 1, arr: 0 },
        _ => return None,
    })
}

pub struct Parser<'a> {
    t: &'a [Token],
    i: usize,
    pub structs: Vec<StructDef>,
    scopes: Vec<BTreeMap<String, u16>>,
}

pub fn parse(tokens: &[Token]) -> PResult<Unit> {
    let mut p = Parser { t: tokens, i: 0, structs: Vec::new(), scopes: vec![BTreeMap::new()] };
    let mut items = Vec::new();
    while p.i < p.t.len() {
        p.external(&mut items)?;
    }
    Ok(Unit { items, structs: p.structs })
}

impl Parser<'_> {
    fn line(&self) -> u32 {
        self.t.get(self.i).or(self.t.last()).map(|t| t.line).unwrap_or(1)
    }

    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i).map(|t| &t.t)
    }

    fn peek_at(&self, k: usize) -> Option<&Tok> {
        self.t.get(self.i + k).map(|t| &t.t)
    }

    fn is_p(&self, p: &str) -> bool {
        matches!(self.peek(), Some(Tok::P(x)) if *x == p)
    }

    fn is_kw(&self, k: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(x)) if x == k)
    }

    fn eat_p(&mut self, p: &str) -> bool {
        if self.is_p(p) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, k: &str) -> bool {
        if self.is_kw(k) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn err<T>(&self, msg: impl Into<String>) -> PResult<T> {
        Err((self.line(), msg.into()))
    }

    fn expect_p(&mut self, p: &str) -> PResult<()> {
        if self.eat_p(p) {
            Ok(())
        } else {
            let got = self.describe();
            self.err(format!("'{got}' : syntax error, expected '{p}'"))
        }
    }

    fn describe(&self) -> String {
        match self.peek() {
            Some(Tok::Ident(s)) => s.clone(),
            Some(Tok::Int(v)) => format!("{v}"),
            Some(Tok::Float(v)) => format!("{v}"),
            Some(Tok::P(p)) => p.to_string(),
            None => "end of file".into(),
        }
    }

    /// An identifier that may name a variable or function: not a keyword, type or reserved word, WebGL's
    /// restrictions applied (WebGL 1.0 "Reserved Identifiers" `webgl_` / `_webgl_` prefixes, "Maximum GLSL Token Size" 256 characters).
    fn ident(&mut self) -> PResult<String> {
        let line = self.line();
        let Some(Tok::Ident(s)) = self.peek() else {
            let got = self.describe();
            return Err((line, format!("'{got}' : syntax error, expected an identifier")));
        };
        let s = s.clone();
        check_ident(&s, line)?;
        if builtin_type(&s).is_some() || KEYWORDS.contains(&s.as_str()) {
            return Err((line, format!("'{s}' : syntax error, unexpected keyword")));
        }
        self.i += 1;
        Ok(s)
    }

    fn struct_named(&self, name: &str) -> Option<u16> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn push_scope(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn precision(&mut self) -> Prec {
        if self.eat_kw("lowp") {
            Prec::Low
        } else if self.eat_kw("mediump") {
            Prec::Medium
        } else if self.eat_kw("highp") {
            Prec::High
        } else {
            Prec::None
        }
    }

    /// A type specifier (§4.1); a struct specifier defines its struct and records a definition point.
    fn type_spec(&mut self, defs: &mut Vec<u16>) -> PResult<Ty> {
        let line = self.line();
        let Some(Tok::Ident(s)) = self.peek().cloned() else { return self.err("syntax error, expected a type") };
        if let Some(t) = builtin_type(&s) {
            self.i += 1;
            return Ok(t);
        }
        if s == "struct" {
            self.i += 1;
            let name = if matches!(self.peek(), Some(Tok::Ident(_))) { self.ident()? } else { String::new() };
            self.expect_p("{")?;
            let mut fields = Vec::new();
            let mut sizes = Vec::new();
            while !self.eat_p("}") {
                if self.peek().is_none() {
                    return self.err("unexpected end of file in struct");
                }
                let prec = self.precision();
                if self.is_kw("struct") {
                    return self.err("embedded struct definitions are not supported (GLSL ES §4.1.8)");
                }
                let mut d2 = Vec::new();
                let ft = self.type_spec(&mut d2)?;
                if ft.base == Base::Void {
                    return self.err("illegal use of type 'void'");
                }
                loop {
                    let fname = self.ident()?;
                    let size = self.array_suffix()?;
                    if fields.iter().any(|(n, _, _): &(String, Ty, Prec)| *n == fname) {
                        return self.err(format!("'{fname}' : duplicate field name in structure"));
                    }
                    fields.push((fname, ft, prec));
                    sizes.push(size);
                    if !self.eat_p(",") {
                        break;
                    }
                }
                self.expect_p(";")?;
            }
            if fields.is_empty() {
                return Err((line, "structure must have at least one field".into()));
            }
            let id = self.structs.len() as u16;
            self.structs.push(StructDef { name: name.clone(), fields, sizes, line });
            if !name.is_empty() {
                if self.scopes.last().unwrap().contains_key(&name) {
                    return Err((line, format!("'{name}' : redefinition of struct")));
                }
                self.scopes.last_mut().unwrap().insert(name, id);
            }
            defs.push(id);
            return Ok(Ty { base: Base::Struct(id), rows: 1, cols: 1, arr: 0 });
        }
        if let Some(id) = self.struct_named(&s) {
            self.i += 1;
            return Ok(Ty { base: Base::Struct(id), rows: 1, cols: 1, arr: 0 });
        }
        if RESERVED.contains(&s.as_str()) {
            return self.err(format!("'{s}' : reserved keyword"));
        }
        self.err(format!("'{s}' : syntax error, expected a type"))
    }

    /// `[ size ]` after a declarator: Some(Some(expr)), Some(None) for `[]`, None without brackets.
    fn array_suffix(&mut self) -> PResult<Option<Option<Expr>>> {
        if !self.eat_p("[") {
            return Ok(None);
        }
        if self.eat_p("]") {
            return Ok(Some(None));
        }
        let e = self.conditional()?;
        self.expect_p("]")?;
        Ok(Some(Some(e)))
    }

    fn qualifier(&mut self) -> Qual {
        if self.eat_kw("const") {
            Qual::Const
        } else if self.eat_kw("attribute") {
            Qual::Attribute
        } else if self.eat_kw("uniform") {
            Qual::Uniform
        } else if self.eat_kw("varying") {
            Qual::Varying
        } else {
            Qual::None
        }
    }

    fn external(&mut self, items: &mut Vec<Ext>) -> PResult<()> {
        let line = self.line();
        if self.eat_p(";") {
            return Ok(());
        }
        if let Some(Tok::Ident(s)) = self.peek() {
            if RESERVED.contains(&s.as_str()) {
                return self.err(format!("'{s}' : reserved keyword"));
            }
        }
        if self.eat_kw("precision") {
            let p = self.precision();
            if p == Prec::None {
                return self.err("syntax error, expected a precision qualifier");
            }
            let mut defs = Vec::new();
            let t = self.type_spec(&mut defs)?;
            self.expect_p(";")?;
            items.push(Ext::Precision(p, t, line));
            return Ok(());
        }
        // `invariant name, name;` re-declares outputs as invariant (§4.6.1).
        if self.is_kw("invariant") && matches!(self.peek_at(1), Some(Tok::Ident(n)) if !matches!(n.as_str(), "varying" | "attribute" | "uniform" | "const" | "lowp" | "mediump" | "highp") && builtin_type(n).is_none())
        {
            self.i += 1;
            let mut names = vec![self.any_ident()?];
            while self.eat_p(",") {
                names.push(self.any_ident()?);
            }
            self.expect_p(";")?;
            items.push(Ext::Invariant(names, line));
            return Ok(());
        }
        let invariant = self.eat_kw("invariant");
        let qual = self.qualifier();
        if invariant && qual != Qual::Varying {
            return self.err("'invariant' : can only qualify varyings and outputs (GLSL ES §4.6.1)");
        }
        let prec = self.precision();
        let mut defs = Vec::new();
        let ty = self.type_spec(&mut defs)?;
        for d in &defs {
            items.push(Ext::Struct(*d));
        }
        // A function definition or prototype: type name '('.
        if qual == Qual::None && !invariant && matches!(self.peek(), Some(Tok::Ident(_))) && matches!(self.peek_at(1), Some(Tok::P("("))) {
            let f = self.function(ty, prec, line)?;
            items.push(Ext::Func(f));
            return Ok(());
        }
        let g = self.declarators(qual, invariant, prec, ty, line)?;
        items.push(Ext::Decl(g));
        Ok(())
    }

    /// An identifier for `invariant` (may name a builtin like gl_Position).
    fn any_ident(&mut self) -> PResult<String> {
        match self.peek() {
            Some(Tok::Ident(s)) => {
                let s = s.clone();
                self.i += 1;
                Ok(s)
            }
            _ => self.err("syntax error, expected an identifier"),
        }
    }

    fn declarators(&mut self, qual: Qual, invariant: bool, prec: Prec, ty: Ty, line: u32) -> PResult<DeclGroup> {
        let mut decls = Vec::new();
        if !self.eat_p(";") {
            loop {
                let dl = self.line();
                let name = self.ident()?;
                let size = self.array_suffix()?;
                let init = if self.eat_p("=") { Some(self.assignment()?) } else { None };
                decls.push(VarDecl { name, ty, size, init, line: dl, id: 0 });
                if !self.eat_p(",") {
                    break;
                }
            }
            self.expect_p(";")?;
        }
        Ok(DeclGroup { qual, invariant, prec, decls, line })
    }

    fn function(&mut self, ret: Ty, ret_prec: Prec, line: u32) -> PResult<FuncDef> {
        let name = self.ident()?;
        self.expect_p("(")?;
        let mut params = Vec::new();
        if self.is_kw("void") && matches!(self.peek_at(1), Some(Tok::P(")"))) {
            self.i += 1;
        }
        if !self.eat_p(")") {
            loop {
                let pl = self.line();
                let cnst = self.eat_kw("const");
                let qual = if self.eat_kw("in") {
                    ParamQual::In
                } else if self.eat_kw("out") {
                    ParamQual::Out
                } else if self.eat_kw("inout") {
                    ParamQual::InOut
                } else {
                    ParamQual::In
                };
                let prec = self.precision();
                let mut defs = Vec::new();
                let ty = self.type_spec(&mut defs)?;
                if !defs.is_empty() {
                    return self.err("structure definitions are not allowed in parameter lists");
                }
                let pname = if matches!(self.peek(), Some(Tok::Ident(_))) { Some(self.ident()?) } else { None };
                let size = self.array_suffix()?;
                params.push(Param { name: pname, ty, size, qual, cnst, prec, line: pl, id: 0 });
                if !self.eat_p(",") {
                    break;
                }
            }
            self.expect_p(")")?;
        }
        let body = if self.eat_p(";") {
            None
        } else {
            if !self.is_p("{") {
                return self.err("syntax error, expected '{' or ';' after a function prototype");
            }
            self.i += 1;
            self.push_scope();
            let b = self.block_body()?;
            self.pop_scope();
            Some(b)
        };
        Ok(FuncDef { name, ret, ret_prec, params, body, line })
    }

    /// Statements up to the closing `}` (already past the opening `{`).
    fn block_body(&mut self) -> PResult<Vec<Stmt>> {
        let mut v = Vec::new();
        while !self.eat_p("}") {
            if self.peek().is_none() {
                return self.err("unexpected end of file, expected '}'");
            }
            self.statement(&mut v)?;
        }
        Ok(v)
    }

    fn sub_statement(&mut self) -> PResult<Stmt> {
        // A sub-statement (if/loop body) that is not a compound statement has its own scope (§6).
        let mut v = Vec::new();
        self.push_scope();
        let r = self.statement(&mut v);
        self.pop_scope();
        r?;
        Ok(if v.len() == 1 { v.pop().unwrap() } else { Stmt::Block(v) })
    }

    /// Is this a declaration statement (vs an expression statement)?
    fn at_declaration(&self) -> bool {
        match self.peek() {
            Some(Tok::Ident(s)) => {
                if matches!(s.as_str(), "const" | "lowp" | "mediump" | "highp" | "struct" | "attribute" | "uniform" | "varying" | "invariant") {
                    return true;
                }
                if builtin_type(s).is_some() || self.struct_named(s).is_some() {
                    // `vec3(…)` / `S(…)` is a constructor expression.
                    return !matches!(self.peek_at(1), Some(Tok::P("(")) | Some(Tok::P(".")) | Some(Tok::P("[")));
                }
                false
            }
            _ => false,
        }
    }

    fn statement(&mut self, out: &mut Vec<Stmt>) -> PResult<()> {
        let line = self.line();
        if let Some(Tok::Ident(s)) = self.peek() {
            if RESERVED.contains(&s.as_str()) {
                return self.err(format!("'{s}' : reserved keyword"));
            }
        }
        if self.eat_p("{") {
            self.push_scope();
            let b = self.block_body();
            self.pop_scope();
            out.push(Stmt::Block(b?));
            return Ok(());
        }
        if self.eat_p(";") {
            out.push(Stmt::Empty);
            return Ok(());
        }
        if self.eat_kw("precision") {
            let p = self.precision();
            if p == Prec::None {
                return self.err("syntax error, expected a precision qualifier");
            }
            let mut defs = Vec::new();
            let t = self.type_spec(&mut defs)?;
            self.expect_p(";")?;
            out.push(Stmt::Precision(p, t, line));
            return Ok(());
        }
        if self.eat_kw("if") {
            self.expect_p("(")?;
            let c = self.expression()?;
            self.expect_p(")")?;
            let a = self.sub_statement()?;
            let b = if self.eat_kw("else") { Some(Box::new(self.sub_statement()?)) } else { None };
            out.push(Stmt::If(c, Box::new(a), b));
            return Ok(());
        }
        if self.eat_kw("while") {
            self.expect_p("(")?;
            self.push_scope();
            let r = (|| -> PResult<Stmt> {
                let (c, d) = self.condition()?;
                self.expect_p(")")?;
                let body = self.loop_body()?;
                Ok(Stmt::While(c, d, Box::new(body)))
            })();
            self.pop_scope();
            out.push(r?);
            return Ok(());
        }
        if self.eat_kw("do") {
            let body = self.sub_statement()?;
            if !self.eat_kw("while") {
                return self.err("syntax error, expected 'while'");
            }
            self.expect_p("(")?;
            let c = self.expression()?;
            self.expect_p(")")?;
            self.expect_p(";")?;
            out.push(Stmt::DoWhile(Box::new(body), c));
            return Ok(());
        }
        if self.eat_kw("for") {
            self.expect_p("(")?;
            self.push_scope();
            let r = (|| -> PResult<Stmt> {
                let mut init = Vec::new();
                if self.at_declaration() {
                    self.declaration_statement(&mut init)?;
                } else if !self.eat_p(";") {
                    let e = self.expression()?;
                    self.expect_p(";")?;
                    init.push(Stmt::Expr(e));
                }
                let (c, d) = if self.is_p(";") { (None, None) } else { self.condition()? };
                self.expect_p(";")?;
                let step = if self.is_p(")") { None } else { Some(self.expression()?) };
                self.expect_p(")")?;
                let body = self.loop_body()?;
                let init = if init.len() == 1 { init.pop().unwrap() } else { Stmt::Block(init) };
                Ok(Stmt::For(Box::new(init), c, d, step, Box::new(body)))
            })();
            self.pop_scope();
            out.push(r?);
            return Ok(());
        }
        if self.eat_kw("return") {
            let e = if self.is_p(";") { None } else { Some(self.expression()?) };
            self.expect_p(";")?;
            out.push(Stmt::Return(e, line));
            return Ok(());
        }
        if self.eat_kw("break") {
            self.expect_p(";")?;
            out.push(Stmt::Break(line));
            return Ok(());
        }
        if self.eat_kw("continue") {
            self.expect_p(";")?;
            out.push(Stmt::Continue(line));
            return Ok(());
        }
        if self.eat_kw("discard") {
            self.expect_p(";")?;
            out.push(Stmt::Discard(line));
            return Ok(());
        }
        if self.at_declaration() {
            return self.declaration_statement(out);
        }
        let e = self.expression()?;
        self.expect_p(";")?;
        out.push(Stmt::Expr(e));
        Ok(())
    }

    /// A loop body: §6.3 — the body of a for/while does not start a new scope beyond the loop's own when it is
    /// a compound statement (a redeclaration of the loop variable in it is an error, caught by sema), so the
    /// body's block shares nothing special here.
    fn loop_body(&mut self) -> PResult<Stmt> {
        self.sub_statement()
    }

    /// A loop condition: an expression or `type name = initializer` (§6.3 condition).
    fn condition(&mut self) -> PResult<(Option<Expr>, Option<DeclGroup>)> {
        if self.at_declaration() {
            let line = self.line();
            let prec = self.precision();
            let mut defs = Vec::new();
            let ty = self.type_spec(&mut defs)?;
            let dl = self.line();
            let name = self.ident()?;
            if !self.eat_p("=") {
                return self.err("syntax error, a condition declaration needs an initializer");
            }
            let init = self.assignment()?;
            return Ok((None, Some(DeclGroup { qual: Qual::None, invariant: false, prec, decls: vec![VarDecl { name, ty, size: None, init: Some(init), line: dl, id: 0 }], line })));
        }
        Ok((Some(self.expression()?), None))
    }

    fn declaration_statement(&mut self, out: &mut Vec<Stmt>) -> PResult<()> {
        let line = self.line();
        let invariant = self.eat_kw("invariant");
        let qual = self.qualifier();
        if qual != Qual::None && qual != Qual::Const {
            return self.err("'attribute', 'uniform' and 'varying' are only allowed at global scope");
        }
        if invariant {
            return self.err("'invariant' is only allowed at global scope");
        }
        let prec = self.precision();
        let mut defs = Vec::new();
        let ty = self.type_spec(&mut defs)?;
        for d in defs {
            out.push(Stmt::Struct(d));
        }
        if matches!(self.peek(), Some(Tok::Ident(_))) && matches!(self.peek_at(1), Some(Tok::P("("))) {
            return self.err("function declarations are only allowed at global scope");
        }
        let g = self.declarators(qual, false, prec, ty, line)?;
        out.push(Stmt::Decl(g));
        Ok(())
    }

    // ---------------------------------------------------------------------------------------- expressions

    pub fn expression(&mut self) -> PResult<Expr> {
        let mut e = self.assignment()?;
        while self.is_p(",") {
            let line = self.line();
            self.i += 1;
            let r = self.assignment()?;
            e = Expr::new(EK::Comma(Box::new(e), Box::new(r)), line);
        }
        Ok(e)
    }

    fn assignment(&mut self) -> PResult<Expr> {
        let lhs = self.conditional()?;
        let line = self.line();
        let op = match self.peek() {
            Some(Tok::P("=")) => AsgOp::Set,
            Some(Tok::P("+=")) => AsgOp::Add,
            Some(Tok::P("-=")) => AsgOp::Sub,
            Some(Tok::P("*=")) => AsgOp::Mul,
            Some(Tok::P("/=")) => AsgOp::Div,
            Some(Tok::P(p @ ("%=" | "<<=" | ">>=" | "&=" | "^=" | "|="))) => {
                let p = *p;
                return self.err(format!("'{p}' : reserved operator (GLSL ES §5.1)"));
            }
            _ => return Ok(lhs),
        };
        self.i += 1;
        let rhs = self.assignment()?;
        Ok(Expr::new(EK::Assign(op, Box::new(lhs), Box::new(rhs)), line))
    }

    fn conditional(&mut self) -> PResult<Expr> {
        let c = self.binary(0)?;
        if !self.is_p("?") {
            return Ok(c);
        }
        let line = self.line();
        self.i += 1;
        let a = self.expression()?;
        self.expect_p(":")?;
        let b = self.assignment()?;
        Ok(Expr::new(EK::Cond(Box::new(c), Box::new(a), Box::new(b)), line))
    }

    fn binop(&self) -> Option<(u8, BinOp)> {
        let Some(Tok::P(p)) = self.peek() else { return None };
        Some(match *p {
            "||" => (1, BinOp::Or),
            "^^" => (2, BinOp::Xor),
            "&&" => (3, BinOp::And),
            "==" => (4, BinOp::Eq),
            "!=" => (4, BinOp::Ne),
            "<" => (5, BinOp::Lt),
            ">" => (5, BinOp::Gt),
            "<=" => (5, BinOp::Le),
            ">=" => (5, BinOp::Ge),
            "+" => (6, BinOp::Add),
            "-" => (6, BinOp::Sub),
            "*" => (7, BinOp::Mul),
            "/" => (7, BinOp::Div),
            _ => return None,
        })
    }

    fn binary(&mut self, min: u8) -> PResult<Expr> {
        let mut lhs = self.unary()?;
        loop {
            if let Some(Tok::P(p @ ("%" | "<<" | ">>" | "&" | "|" | "^"))) = self.peek() {
                let p = *p;
                return self.err(format!("'{p}' : reserved operator (GLSL ES §5.1)"));
            }
            let Some((prec, op)) = self.binop() else { break };
            if prec <= min {
                break;
            }
            let line = self.line();
            self.i += 1;
            let rhs = self.binary(prec)?;
            lhs = Expr::new(EK::Bin(op, Box::new(lhs), Box::new(rhs)), line);
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let line = self.line();
        let op = match self.peek() {
            Some(Tok::P("++")) => UnOp::PreInc,
            Some(Tok::P("--")) => UnOp::PreDec,
            Some(Tok::P("-")) => UnOp::Neg,
            Some(Tok::P("+")) => UnOp::Pos,
            Some(Tok::P("!")) => UnOp::Not,
            Some(Tok::P("~")) => return self.err("'~' : reserved operator (GLSL ES §5.1)"),
            _ => return self.postfix(),
        };
        self.i += 1;
        let e = self.unary()?;
        Ok(Expr::new(EK::Un(op, Box::new(e)), line))
    }

    fn args(&mut self) -> PResult<Vec<Expr>> {
        // Past '('.
        let mut v = Vec::new();
        if self.is_kw("void") && matches!(self.peek_at(1), Some(Tok::P(")"))) {
            self.i += 1;
        }
        if self.eat_p(")") {
            return Ok(v);
        }
        loop {
            v.push(self.assignment()?);
            if self.eat_p(")") {
                return Ok(v);
            }
            self.expect_p(",")?;
        }
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        loop {
            let line = self.line();
            if self.eat_p("[") {
                let ix = self.expression()?;
                self.expect_p("]")?;
                e = Expr::new(EK::Index(Box::new(e), Box::new(ix)), line);
            } else if self.eat_p(".") {
                let Some(Tok::Ident(f)) = self.peek().cloned() else { return self.err("syntax error, expected a field name") };
                self.i += 1;
                e = Expr::new(EK::Field(Box::new(e), f), line);
            } else if self.eat_p("++") {
                e = Expr::new(EK::Un(UnOp::PostInc, Box::new(e)), line);
            } else if self.eat_p("--") {
                e = Expr::new(EK::Un(UnOp::PostDec, Box::new(e)), line);
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> PResult<Expr> {
        let line = self.line();
        let Some(tk) = self.peek().cloned() else { return self.err("unexpected end of file") };
        match tk {
            Tok::Int(v) => {
                self.i += 1;
                if v > i32::MAX as i64 + 1 {
                    return Err((line, format!("'{v}' : integer constant overflow")));
                }
                Ok(Expr::new(EK::Int(v), line))
            }
            Tok::Float(v) => {
                self.i += 1;
                Ok(Expr::new(EK::Float(v), line))
            }
            Tok::P("(") => {
                self.i += 1;
                let e = self.expression()?;
                self.expect_p(")")?;
                Ok(e)
            }
            Tok::Ident(s) => {
                if s == "true" || s == "false" {
                    self.i += 1;
                    return Ok(Expr::new(EK::Bool(s == "true"), line));
                }
                if let Some(t) = builtin_type(&s) {
                    self.i += 1;
                    if !self.eat_p("(") {
                        return Err((line, format!("'{s}' : syntax error, a type name needs '(' here")));
                    }
                    let a = self.args()?;
                    return Ok(Expr::new(EK::Ctor(t, a), line));
                }
                if let Some(id) = self.struct_named(&s) {
                    self.i += 1;
                    self.expect_p("(")?;
                    let a = self.args()?;
                    return Ok(Expr::new(EK::Ctor(Ty { base: Base::Struct(id), rows: 1, cols: 1, arr: 0 }, a), line));
                }
                if RESERVED.contains(&s.as_str()) {
                    return Err((line, format!("'{s}' : reserved keyword")));
                }
                if KEYWORDS.contains(&s.as_str()) {
                    return Err((line, format!("'{s}' : syntax error")));
                }
                check_ident(&s, line)?;
                self.i += 1;
                if self.eat_p("(") {
                    let a = self.args()?;
                    return Ok(Expr::new(EK::Call(s, a), line));
                }
                Ok(Expr::new(EK::Name(s), line))
            }
            Tok::P(p) => Err((line, format!("'{p}' : syntax error"))),
        }
    }
}

/// WebGL 1.0 "Reserved Identifiers"/§6.25 identifier rules.
pub fn check_ident(s: &str, line: u32) -> PResult<()> {
    if s.starts_with("webgl_") || s.starts_with("_webgl_") {
        return Err((line, format!("'{s}' : identifiers starting with webgl_ or _webgl_ are reserved (WebGL 1.0 'Reserved Identifiers')")));
    }
    if s.len() > 256 {
        return Err((line, "identifier longer than 256 characters (WebGL 1.0 'Maximum GLSL Token Size')".into()));
    }
    if RESERVED.contains(&s) {
        return Err((line, format!("'{s}' : reserved keyword")));
    }
    Ok(())
}
