// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! TOML, from the TOML v1.0.0 specification: comments (`#` to the end of the line), key/value pairs
//! with bare keys (`A-Za-z0-9_-`), quoted keys and dotted keys, tables (`[a.b]`) and arrays of tables
//! (`[[a]]`) as one heading token, basic strings with the spec's escapes (`\b \t \n \f \r \" \\`,
//! `\uXXXX`, `\UXXXXXXXX`), literal strings (no escapes), multi-line basic (`"""`, the line-ending
//! backslash) and multi-line literal (`'''`) strings — up to two quotes may stand next to the closing
//! delimiter — integers (decimal with `_`, `+`/`-`, `0x` `0o` `0b`), floats (fraction, exponent, `inf`,
//! `nan`, signed), booleans, offset/local date-times, dates and times (the space-delimited form
//! `1979-05-27 07:32:00` included), arrays (which may span lines) and inline tables.
//!
//! State: `mode` 1 = multi-line basic string, 2 = multi-line literal string; `stack` = the open arrays
//! (`[`) and inline tables (`{`), so a line inside an array starts expecting a value.

use crate::scan::{at, blank, skip, starts};
use crate::{HighlightKind as K, Language, LexState, LineOut};

pub struct Toml;

const M_ML_BASIC: u8 = 1;
const M_ML_LITERAL: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Key,
    Value,
    After,
}

fn bare(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn escape_end(s: &[u8], i: usize) -> usize {
    match at(s, i + 1) {
        0 => i + 1,
        b'u' => (i + 6).min(s.len()),
        b'U' => (i + 10).min(s.len()),
        _ => (i + 2).min(s.len()),
    }
}

/// A single-line basic string from `from` to its closing `"`.
fn basic(s: &[u8], from: usize, tok: usize, out: &mut LineOut) -> usize {
    let mut seg = tok;
    let mut i = from;
    while i < s.len() {
        match s[i] {
            b'\\' => {
                out.push(K::String, seg, i);
                let e = escape_end(s, i);
                out.push(K::Escape, i, e);
                i = e;
                seg = i;
            }
            b'"' => {
                out.push(K::String, seg, i + 1);
                return i + 1;
            }
            _ => i += 1,
        }
    }
    out.push(K::String, seg, s.len());
    s.len()
}

/// A single-line literal string from `from` to its closing `'`.
fn literal(s: &[u8], from: usize, tok: usize, out: &mut LineOut) -> usize {
    let e = match s[from.min(s.len())..].iter().position(|&b| b == b'\'') {
        Some(p) => from + p + 1,
        None => s.len(),
    };
    out.push(K::String, tok, e);
    e
}

/// The end of a closing triple delimiter at `i`, absorbing up to two more quote bytes before it.
fn close_run(s: &[u8], i: usize, q: u8) -> usize {
    let run = skip(s, i, |b| b == q) - i;
    i + run.min(5)
}

fn ml_basic(s: &[u8], from: usize, tok: usize, st: &mut LexState, out: &mut LineOut) -> usize {
    let mut seg = tok;
    let mut i = from;
    while i < s.len() {
        if s[i] == b'\\' {
            out.push(K::String, seg, i);
            let e = escape_end(s, i);
            out.push(K::Escape, i, e);
            i = e;
            seg = i;
        } else if starts(s, i, b"\"\"\"") {
            let e = close_run(s, i, b'"');
            out.push(K::String, seg, e);
            st.mode = 0;
            return e;
        } else {
            i += 1;
        }
    }
    out.push(K::String, seg, s.len());
    st.mode = M_ML_BASIC;
    s.len()
}

fn ml_literal(s: &[u8], from: usize, tok: usize, st: &mut LexState, out: &mut LineOut) -> usize {
    let mut i = from;
    while i < s.len() {
        if starts(s, i, b"'''") {
            let e = close_run(s, i, b'\'');
            out.push(K::String, tok, e);
            st.mode = 0;
            return e;
        }
        i += 1;
    }
    out.push(K::String, tok, s.len());
    st.mode = M_ML_LITERAL;
    s.len()
}

/// End of a table header starting at `i` (`[` or `[[`): past the closing bracket(s), quoted keys skipped.
fn header_end(s: &[u8], i: usize) -> usize {
    let double = at(s, i + 1) == b'[';
    let mut k = i + if double { 2 } else { 1 };
    while k < s.len() {
        match s[k] {
            b'"' | b'\'' => {
                let q = s[k];
                k += 1;
                while k < s.len() && s[k] != q {
                    if q == b'"' && s[k] == b'\\' {
                        k += 1;
                    }
                    k += 1;
                }
            }
            b']' => {
                return if double && at(s, k + 1) == b']' { k + 2 } else { k + 1 };
            }
            _ => {}
        }
        k += 1;
    }
    s.len()
}

/// A number, date or time from `i`: `[0-9A-Za-z_+\-:.]`, plus the space-delimited date-time.
fn number(s: &[u8], i: usize) -> usize {
    let tok = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b':' | b'.');
    let mut e = skip(s, i, tok);
    let date = e - i == 10 && at(s, i + 4) == b'-' && at(s, i + 7) == b'-';
    if date && at(s, e) == b' ' && at(s, e + 1).is_ascii_digit() && at(s, e + 2).is_ascii_digit() && at(s, e + 3) == b':' {
        e = skip(s, e + 1, tok);
    }
    e
}

impl Language for Toml {
    fn id(&self) -> &'static str {
        "toml"
    }

    fn lex_line(&self, line: &str, st: &mut LexState, out: &mut LineOut) {
        let s = line.as_bytes();
        let len = s.len();
        let resumed = st.mode != 0;
        let mut i = match st.mode {
            M_ML_BASIC => ml_basic(s, 0, 0, st, out),
            M_ML_LITERAL => ml_literal(s, 0, 0, st, out),
            _ => 0,
        };
        let mut ctx = if resumed {
            Ctx::After
        } else if st.stack.last() == Some(&b'[') {
            Ctx::Value
        } else {
            Ctx::Key
        };
        while i < len {
            let b = s[i];
            if blank(b) || b == b'\r' {
                i += 1;
                continue;
            }
            match b {
                b'#' => {
                    out.push(K::Comment, i, len);
                    return;
                }
                b'[' if ctx == Ctx::Key && st.stack.is_empty() => {
                    let e = header_end(s, i);
                    out.push(K::Heading, i, e);
                    i = e;
                    ctx = Ctx::After;
                }
                b'"' => {
                    if ctx == Ctx::Value && starts(s, i, b"\"\"\"") {
                        i = ml_basic(s, i + 3, i, st, out);
                    } else {
                        i = basic(s, i + 1, i, out);
                    }
                    if ctx == Ctx::Value {
                        ctx = Ctx::After;
                    }
                }
                b'\'' => {
                    if ctx == Ctx::Value && starts(s, i, b"'''") {
                        i = ml_literal(s, i + 3, i, st, out);
                    } else {
                        i = literal(s, i + 1, i, out);
                    }
                    if ctx == Ctx::Value {
                        ctx = Ctx::After;
                    }
                }
                b'=' => {
                    out.push(K::Operator, i, i + 1);
                    i += 1;
                    ctx = Ctx::Value;
                }
                b'.' if ctx == Ctx::Key => {
                    out.push(K::Punctuation, i, i + 1);
                    i += 1;
                }
                b'[' => {
                    out.push(K::Punctuation, i, i + 1);
                    st.stack.push(b'[');
                    i += 1;
                    ctx = Ctx::Value;
                }
                b'{' => {
                    out.push(K::Punctuation, i, i + 1);
                    st.stack.push(b'{');
                    i += 1;
                    ctx = Ctx::Key;
                }
                b']' | b'}' => {
                    out.push(K::Punctuation, i, i + 1);
                    st.stack.pop();
                    i += 1;
                    ctx = Ctx::After;
                }
                b',' => {
                    out.push(K::Punctuation, i, i + 1);
                    i += 1;
                    ctx = if st.stack.last() == Some(&b'{') { Ctx::Key } else { Ctx::Value };
                }
                _ if ctx == Ctx::Key && bare(b) => {
                    let e = skip(s, i, bare);
                    out.push(K::Variable, i, e);
                    i = e;
                }
                _ if ctx == Ctx::Value && (b.is_ascii_digit() || b == b'+' || b == b'-') => {
                    let e = number(s, i);
                    out.push(K::Number, i, e);
                    i = e;
                    ctx = Ctx::After;
                }
                _ if ctx == Ctx::Value && b.is_ascii_alphabetic() => {
                    let e = skip(s, i, |c| c.is_ascii_alphanumeric());
                    let kind = match &s[i..e] {
                        b"true" | b"false" => K::Keyword,
                        b"inf" | b"nan" => K::Number,
                        _ => K::Error,
                    };
                    out.push(kind, i, e);
                    i = e;
                    ctx = Ctx::After;
                }
                _ => {
                    let e = skip(s, i + 1, |c| !(blank(c) || matches!(c, b',' | b']' | b'}' | b'#' | b'=')));
                    out.push(K::Error, i, e);
                    i = e;
                }
            }
        }
    }
}
