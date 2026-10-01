// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! The query language (F4, B302).
//!
//! ```text
//!   expr  := and ( OR and )*
//!   and   := unary ( AND unary )*
//!   unary := '(' expr ')' | pred
//!   pred  := key ( == | != | > | < | >= | <= ) value
//!          | key BETWEEN value AND value                 (inclusive)
//!          | value ( < | <= ) key ( < | <= ) value       (two-sided range)
//!          | value ( > | >= ) key ( > | >= ) value       (two-sided, reversed)
//!          | similarity '(' key ',' vector ')' '>' number
//!   value := "string" | int | float | [f, f, …] | bare-word (a string)
//! ```
//!
//! Keywords (`AND`, `OR`, `BETWEEN`) are case-insensitive. Every predicate is
//! TYPED: `==`/`!=` compare the stored `AttributeValue` exactly (an `Int(3)` is
//! not a `Float(3.0)` — equality runs on the value hash); the ordering
//! operators compare `Int`/`Float` numerically across the two types and
//! `String`s bytewise; any other pairing never matches. A predicate only
//! matches inodes that CARRY its key (`k != v` does not match an inode
//! without `k`). `AND` scores multiply and `OR` scores take the maximum, so a
//! lone similarity predicate's score survives any number of `AND`ed filters
//! bit-exactly (`x · 1.0 == x`).

use crate::inode::AttributeValue;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A comparison operator.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryOp {
    Eq,
    Neq,
    Gt,
    Lt,
    /// `>=`
    Ge,
    /// `<=`
    Le,
    /// Two-sided range: `value` is the low bound, `Predicate::value_hi` the
    /// high bound; each side inclusive or strict.
    Range { lo_inclusive: bool, hi_inclusive: bool },
    /// `similarity(key, vec) > threshold` (a scan of the inodes carrying
    /// `key`: there is no vector index).
    SimilarityGt(f32),
}

/// One `key op value` test.
#[derive(Debug, Clone, PartialEq)]
pub struct Predicate {
    pub key: String,
    pub op: QueryOp,
    /// The operand (the LOW bound for [`QueryOp::Range`]; the target vector
    /// for similarity).
    pub value: AttributeValue,
    /// The HIGH bound for [`QueryOp::Range`]; `None` otherwise.
    pub value_hi: Option<AttributeValue>,
}

/// A boolean combination of predicates.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Pred(Predicate),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

/// A parsed query.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub expr: Expr,
}

impl Query {
    pub fn parse(input: &str) -> Result<Self, String> {
        let toks = lex(input)?;
        if toks.is_empty() {
            return Err("Invalid query syntax: empty query".to_string());
        }
        let mut p = Parser { toks, pos: 0, depth: 0 };
        let expr = p.expr()?;
        if p.pos != p.toks.len() {
            return Err(format!("Invalid query syntax: unexpected '{}'", p.toks[p.pos].text()));
        }
        Ok(Query { expr })
    }
}

/// Evaluate one predicate against a value the inode carries. `Some(score)`
/// on a match (1.0 for the boolean ops, the cosine similarity for
/// similarity), `None` otherwise.
pub fn eval_predicate(p: &Predicate, val: &AttributeValue) -> Option<f32> {
    use core::cmp::Ordering::*;
    let ok = |b: bool| if b { Some(1.0) } else { None };
    match &p.op {
        QueryOp::Eq => ok(val == &p.value),
        QueryOp::Neq => ok(val != &p.value),
        QueryOp::Gt => ok(compare_values(val, &p.value) == Some(Greater)),
        QueryOp::Lt => ok(compare_values(val, &p.value) == Some(Less)),
        QueryOp::Ge => ok(matches!(compare_values(val, &p.value), Some(Greater | Equal))),
        QueryOp::Le => ok(matches!(compare_values(val, &p.value), Some(Less | Equal))),
        QueryOp::Range { lo_inclusive, hi_inclusive } => {
            let hi = p.value_hi.as_ref()?;
            let lo_ok = match compare_values(val, &p.value) {
                Some(Greater) => true,
                Some(Equal) => *lo_inclusive,
                _ => false,
            };
            let hi_ok = match compare_values(val, hi) {
                Some(Less) => true,
                Some(Equal) => *hi_inclusive,
                _ => false,
            };
            ok(lo_ok && hi_ok)
        }
        QueryOp::SimilarityGt(threshold) => {
            if let (AttributeValue::Vector(v1), AttributeValue::Vector(v2)) = (val, &p.value) {
                let score = crate::fs::cosine_similarity(v1, v2);
                if score > *threshold { Some(score) } else { None }
            } else {
                None
            }
        }
    }
}

/// The ORDERING the range operators use: Int/Float numerically (across the
/// two types), String bytewise; anything else is unordered (`None`).
pub fn compare_values(a: &AttributeValue, b: &AttributeValue) -> Option<core::cmp::Ordering> {
    use AttributeValue as V;
    match (a, b) {
        (V::Int(x), V::Int(y)) => Some(x.cmp(y)),
        (V::Float(x), V::Float(y)) => x.partial_cmp(y),
        (V::Int(x), V::Float(y)) => (*x as f64).partial_cmp(y),
        (V::Float(x), V::Int(y)) => x.partial_cmp(&(*y as f64)),
        (V::String(x), V::String(y)) => Some(x.as_bytes().cmp(y.as_bytes())),
        _ => None,
    }
}

impl Expr {
    /// Every predicate in the expression, left to right.
    pub fn predicates(&self) -> Vec<&Predicate> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }
    fn collect<'a>(&'a self, out: &mut Vec<&'a Predicate>) {
        match self {
            Expr::Pred(p) => out.push(p),
            Expr::And(v) | Expr::Or(v) => v.iter().for_each(|e| e.collect(out)),
        }
    }

    /// Evaluate with `value_of(key)` supplying the inode's values. AND
    /// multiplies child scores, OR takes the max of the matching children.
    pub fn eval<F: FnMut(&str) -> Option<AttributeValue>>(&self, value_of: &mut F) -> Option<f32> {
        match self {
            Expr::Pred(p) => {
                let v = value_of(&p.key)?;
                eval_predicate(p, &v)
            }
            Expr::And(v) => {
                let mut score = 1.0f32;
                for e in v {
                    score *= e.eval(value_of)?;
                }
                Some(score)
            }
            Expr::Or(v) => {
                let mut best: Option<f32> = None;
                for e in v {
                    if let Some(s) = e.eval(value_of) {
                        best = Some(match best {
                            Some(b) if b >= s => b,
                            _ => s,
                        });
                    }
                }
                best
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// A bare word (key, number, unquoted string, keyword).
    Word(String),
    /// A quoted string literal (unescaped).
    Str(String),
    /// A `[ … ]` vector literal, raw inner text.
    Vec(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

impl Tok {
    fn text(&self) -> String {
        match self {
            Tok::Word(s) => s.clone(),
            Tok::Str(s) => format!("\"{s}\""),
            Tok::Vec(s) => format!("[{s}]"),
            Tok::Op(o) => o.to_string(),
            Tok::LParen => "(".into(),
            Tok::RParen => ")".into(),
            Tok::Comma => ",".into(),
        }
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self, Tok::Word(w) if w.eq_ignore_ascii_case(kw))
    }
}

/// Bound on the query length (the kernel hands this user input).
pub const MAX_QUERY_LEN: usize = 4096;
/// Bound on parenthesis nesting.
pub const MAX_QUERY_DEPTH: usize = 32;

fn lex(input: &str) -> Result<Vec<Tok>, String> {
    if input.len() > MAX_QUERY_LEN {
        return Err("Invalid query syntax: query too long".to_string());
    }
    let b = input.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            b')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            b',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            b'"' => {
                let mut s = String::new();
                i += 1;
                let mut closed = false;
                while i < b.len() {
                    let ch = input[i..].chars().next().unwrap();
                    if ch == '\\' && i + 1 < b.len() && (b[i + 1] == b'"' || b[i + 1] == b'\\') {
                        s.push(b[i + 1] as char);
                        i += 2;
                    } else if ch == '"' {
                        i += 1;
                        closed = true;
                        break;
                    } else {
                        s.push(ch);
                        i += ch.len_utf8();
                    }
                }
                if !closed {
                    return Err("Invalid query syntax: unterminated string".to_string());
                }
                out.push(Tok::Str(s));
            }
            b'[' => {
                let end = input[i..]
                    .find(']')
                    .ok_or_else(|| "Invalid query syntax: unterminated vector".to_string())?;
                out.push(Tok::Vec(input[i + 1..i + end].to_string()));
                i += end + 1;
            }
            b'=' | b'!' | b'<' | b'>' => {
                let two = if i + 1 < b.len() { &input[i..i + 2] } else { "" };
                let op: &'static str = match two {
                    "==" => "==",
                    "!=" => "!=",
                    ">=" => ">=",
                    "<=" => "<=",
                    _ => match c {
                        b'<' => "<",
                        b'>' => ">",
                        _ => return Err(format!("Invalid query syntax: stray '{}'", c as char)),
                    },
                };
                i += op.len();
                out.push(Tok::Op(op));
            }
            _ => {
                let start = i;
                while i < b.len()
                    && !matches!(b[i], b' ' | b'\t' | b'\n' | b'\r' | b'(' | b')' | b',' | b'"' | b'[' | b'=' | b'!' | b'<' | b'>')
                {
                    i += 1;
                }
                out.push(Tok::Word(input[start..i].to_string()));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn peek_at(&self, k: usize) -> Option<&Tok> {
        self.toks.get(self.pos + k)
    }
    fn next(&mut self) -> Result<Tok, String> {
        let t = self
            .toks
            .get(self.pos)
            .cloned()
            .ok_or_else(|| "Invalid query syntax: unexpected end".to_string())?;
        self.pos += 1;
        Ok(t)
    }

    fn expr(&mut self) -> Result<Expr, String> {
        let mut parts = alloc::vec![self.and()?];
        while self.peek().is_some_and(|t| t.is_kw("OR")) {
            self.pos += 1;
            parts.push(self.and()?);
        }
        Ok(if parts.len() == 1 { parts.pop().unwrap() } else { Expr::Or(parts) })
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut parts = alloc::vec![self.unary()?];
        while self.peek().is_some_and(|t| t.is_kw("AND")) {
            self.pos += 1;
            parts.push(self.unary()?);
        }
        Ok(if parts.len() == 1 { parts.pop().unwrap() } else { Expr::And(parts) })
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Tok::LParen) {
            self.depth += 1;
            if self.depth > MAX_QUERY_DEPTH {
                return Err("Invalid query syntax: nesting too deep".to_string());
            }
            self.pos += 1;
            let e = self.expr()?;
            if self.next()? != Tok::RParen {
                return Err("Invalid query syntax: missing ')'".to_string());
            }
            self.depth -= 1;
            return Ok(e);
        }
        Ok(Expr::Pred(self.pred()?))
    }

    fn value(&mut self) -> Result<AttributeValue, String> {
        match self.next()? {
            Tok::Str(s) => Ok(AttributeValue::String(s)),
            Tok::Vec(v) => parse_value(&format!("[{v}]")),
            Tok::Word(w) => parse_value(&w),
            t => Err(format!("Invalid query syntax: expected a value, got '{}'", t.text())),
        }
    }

    fn key(&mut self) -> Result<String, String> {
        match self.next()? {
            Tok::Word(w) => Ok(w),
            Tok::Str(s) => Ok(s),
            t => Err(format!("Invalid query syntax: expected a key, got '{}'", t.text())),
        }
    }

    fn pred(&mut self) -> Result<Predicate, String> {
        // similarity(key, [vec]) > threshold
        if self.peek().is_some_and(|t| t.is_kw("similarity")) && self.peek_at(1) == Some(&Tok::LParen) {
            self.pos += 2;
            let key = self.key()?;
            if self.next()? != Tok::Comma {
                return Err("Missing comma in similarity args".to_string());
            }
            let value = self.value()?;
            if !matches!(value, AttributeValue::Vector(_)) {
                return Err("Second argument to similarity must be a vector".to_string());
            }
            if self.next()? != Tok::RParen {
                return Err("Malformed function call".to_string());
            }
            if self.next()? != Tok::Op(">") {
                return Err("Similarity query must use '>' operator".to_string());
            }
            let threshold = match self.next()? {
                Tok::Word(w) => w.parse::<f32>().map_err(|_| "Invalid threshold".to_string())?,
                _ => return Err("Invalid threshold".to_string()),
            };
            return Ok(Predicate { key, op: QueryOp::SimilarityGt(threshold), value, value_hi: None });
        }

        // key BETWEEN a AND b
        if self.peek_at(1).is_some_and(|t| t.is_kw("BETWEEN")) {
            let key = self.key()?;
            self.pos += 1;
            let lo = self.value()?;
            if !self.next()?.is_kw("AND") {
                return Err("BETWEEN needs 'lo AND hi'".to_string());
            }
            let hi = self.value()?;
            return Ok(Predicate {
                key,
                op: QueryOp::Range { lo_inclusive: true, hi_inclusive: true },
                value: lo,
                value_hi: Some(hi),
            });
        }

        // a OP key OP b (two-sided) — decided by a second comparison after
        // the middle operand.
        let chained = matches!(self.peek_at(1), Some(Tok::Op("<" | "<=" | ">" | ">=")))
            && matches!(self.peek_at(3), Some(Tok::Op("<" | "<=" | ">" | ">=")));
        if chained {
            let a = self.value()?;
            let op1 = self.next()?;
            let key = self.key()?;
            let op2 = self.next()?;
            let b = self.value()?;
            let (o1, o2) = match (&op1, &op2) {
                (Tok::Op(x), Tok::Op(y)) => (*x, *y),
                _ => unreachable!(),
            };
            let ascending = matches!(o1, "<" | "<=") && matches!(o2, "<" | "<=");
            let descending = matches!(o1, ">" | ">=") && matches!(o2, ">" | ">=");
            if ascending {
                return Ok(Predicate {
                    key,
                    op: QueryOp::Range { lo_inclusive: o1 == "<=", hi_inclusive: o2 == "<=" },
                    value: a,
                    value_hi: Some(b),
                });
            }
            if descending {
                return Ok(Predicate {
                    key,
                    op: QueryOp::Range { lo_inclusive: o2 == ">=", hi_inclusive: o1 == ">=" },
                    value: b,
                    value_hi: Some(a),
                });
            }
            return Err("Two-sided range must point one way (a < k < b or a > k > b)".to_string());
        }

        let key = self.key()?;
        let op = match self.next()? {
            Tok::Op("==") => QueryOp::Eq,
            Tok::Op("!=") => QueryOp::Neq,
            Tok::Op(">") => QueryOp::Gt,
            Tok::Op("<") => QueryOp::Lt,
            Tok::Op(">=") => QueryOp::Ge,
            Tok::Op("<=") => QueryOp::Le,
            t => return Err(format!("Invalid query syntax: expected an operator, got '{}'", t.text())),
        };
        let value = self.value()?;
        Ok(Predicate { key, op, value, value_hi: None })
    }
}

pub fn parse_value(input: &str) -> Result<AttributeValue, String> {
    let input = input.trim();
    if input.len() >= 2 && input.starts_with('"') && input.ends_with('"') {
        // String
        let inner = &input[1..input.len() - 1];
        Ok(AttributeValue::String(inner.to_string()))
    } else if input.starts_with('[') && input.ends_with(']') {
        // Vector
        let inner = &input[1..input.len() - 1];
        let parts: Vec<&str> = inner.split(',').collect();
        let mut vec = Vec::new();
        for p in parts {
            let f = p
                .trim()
                .parse::<f32>()
                .map_err(|_| "Invalid number in vector")?;
            vec.push(f);
        }
        Ok(AttributeValue::Vector(vec))
    } else if let Ok(i) = input.parse::<i64>() {
        Ok(AttributeValue::Int(i))
    } else if let Ok(f) = input.parse::<f64>() {
        Ok(AttributeValue::Float(f))
    } else {
        // An unquoted word is a string, for convenience.
        Ok(AttributeValue::String(input.to_string()))
    }
}
