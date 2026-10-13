// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Rust, from the Rust Reference's "Lexical structure" chapter: keywords (strict, reserved, and the
//! 2018+/2024 additions), identifiers and raw identifiers (`r#x`), line and block comments — block
//! comments NEST, and `///`, `//!`, `/** */`, `/*! */` are doc comments while `////`, `/***` and `/**/`
//! are not — character and string literals with their escapes (`\n \r \t \\ \0 \' \"`, `\x7F`,
//! `\u{…}`, and the string line continuation), byte (`b'…'`, `b"…"`), raw (`r#"…"#`, `br…`, `cr…`) and
//! C (`c"…"`) strings, integer and float literals with suffixes, lifetimes versus character literals
//! (`'a` against `'a'`), and attributes (`#[…]`, `#![…]`) as one token. A shebang `#!` that is not
//! `#![` on the first line is a comment.
//!
//! State: `mode` 1 = block comment, 2 = doc block comment (`n` = nesting depth), 3 = string (escapes
//! live), 4 = raw string (`n` = the `#` count).

use crate::lang::one_of;
use crate::scan::{at, blank, ident_cont, ident_start, skip, starts, utf8_len};
use crate::{HighlightKind as K, Language, LexState, LineOut};

pub struct Rust;

const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in",
    "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
    "super", "trait", "true", "type", "unsafe", "use", "where", "while", "async", "await", "dyn", "abstract",
    "become", "box", "do", "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
    "gen",
];

const PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
    "f16", "f32", "f64", "f128",
];

const M_COMMENT: u8 = 1;
const M_DOC: u8 = 2;
const M_STR: u8 = 3;
const M_RAW: u8 = 4;

/// End of the escape that starts at `i` (`s[i] == b'\\'`).
fn escape_end(s: &[u8], i: usize) -> usize {
    match at(s, i + 1) {
        0 => i + 1,
        b'x' => (i + 4).min(s.len()),
        b'u' if at(s, i + 2) == b'{' => match s[i + 2..].iter().position(|&b| b == b'}') {
            Some(p) => i + 2 + p + 1,
            None => s.len(),
        },
        b => (i + 1 + utf8_len(b)).min(s.len()),
    }
}

/// Scan a block comment body from `from`; the token began at `tok`.
fn block_comment(s: &[u8], from: usize, tok: usize, st: &mut LexState, out: &mut LineOut) -> usize {
    let kind = if st.mode == M_DOC { K::DocComment } else { K::Comment };
    let mut depth = st.n;
    let mut i = from;
    while i < s.len() {
        if starts(s, i, b"/*") {
            depth += 1;
            i += 2;
        } else if starts(s, i, b"*/") {
            depth = depth.saturating_sub(1);
            i += 2;
            if depth == 0 {
                out.push(kind, tok, i);
                st.mode = 0;
                st.n = 0;
                return i;
            }
        } else {
            i += 1;
        }
    }
    out.push(kind, tok, s.len());
    st.n = depth;
    s.len()
}

/// Scan a (byte / C) string body with escapes from `from`; the token began at `tok`.
fn string_body(s: &[u8], from: usize, tok: usize, st: &mut LexState, out: &mut LineOut) -> usize {
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
                i += 1;
                out.push(K::String, seg, i);
                st.mode = 0;
                return i;
            }
            _ => i += 1,
        }
    }
    out.push(K::String, seg, s.len());
    st.mode = M_STR;
    s.len()
}

/// Scan a raw string body from `from` (closing `"` + `st.n` hashes); the token began at `tok`.
fn raw_body(s: &[u8], from: usize, tok: usize, st: &mut LexState, out: &mut LineOut) -> usize {
    let n = st.n as usize;
    let mut i = from;
    while i < s.len() {
        if s[i] == b'"' && (0..n).all(|k| at(s, i + 1 + k) == b'#') {
            i += 1 + n;
            out.push(K::String, tok, i);
            st.mode = 0;
            st.n = 0;
            return i;
        }
        i += 1;
    }
    out.push(K::String, tok, s.len());
    st.mode = M_RAW;
    s.len()
}

/// A character literal whose opening quote is at `q`; the token began at `tok` (`b'…'` starts one
/// earlier). Returns `None` when this quote does not open a character literal.
fn char_lit(s: &[u8], q: usize, tok: usize, out: &mut LineOut) -> Option<usize> {
    if at(s, q + 1) == b'\\' {
        let e = escape_end(s, q + 1);
        out.push(K::Char, tok, q + 1);
        out.push(K::Escape, q + 1, e);
        if at(s, e) == b'\'' {
            out.push(K::Char, e, e + 1);
            return Some(e + 1);
        }
        return Some(e);
    }
    let c = at(s, q + 1);
    if c == 0 || c == b'\'' {
        return None;
    }
    let cl = utf8_len(c);
    if at(s, q + 1 + cl) == b'\'' {
        out.push(K::Char, tok, q + 2 + cl);
        return Some(q + 2 + cl);
    }
    None
}

/// End of an attribute starting at `i` (`#[` or `#![`): the matching `]`, strings skipped.
fn attr_end(s: &[u8], i: usize) -> usize {
    let mut depth = 0usize;
    let mut k = i;
    while k < s.len() {
        match s[k] {
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return k + 1;
                }
            }
            b'"' => {
                k += 1;
                while k < s.len() && s[k] != b'"' {
                    if s[k] == b'\\' {
                        k += 1;
                    }
                    k += 1;
                }
            }
            _ => {}
        }
        k += 1;
    }
    s.len()
}

fn is_op(b: u8) -> bool {
    matches!(b, b'+' | b'-' | b'*' | b'/' | b'%' | b'^' | b'!' | b'&' | b'|' | b'=' | b'<' | b'>' | b'@' | b'.' | b'~' | b'?' | b'$' | b'#')
}

fn number(s: &[u8], st: usize) -> usize {
    let digit = |c: u8| c.is_ascii_digit() || c == b'_';
    let mut i = st;
    if s[i] == b'0' && matches!(at(s, i + 1), b'x' | b'o' | b'b') {
        i = skip(s, i + 2, |c| c.is_ascii_hexdigit() || c == b'_');
    } else {
        i = skip(s, i, digit);
        if at(s, i) == b'.' && at(s, i + 1) != b'.' && !ident_start(at(s, i + 1)) {
            i = skip(s, i + 1, digit);
        }
        if matches!(at(s, i), b'e' | b'E') {
            let mut j = i + 1;
            if matches!(at(s, j), b'+' | b'-') {
                j += 1;
            }
            if digit(at(s, j)) {
                i = skip(s, j, digit);
            }
        }
    }
    skip(s, i, ident_cont)
}

impl Language for Rust {
    fn id(&self) -> &'static str {
        "rust"
    }

    fn lex_line(&self, line: &str, st: &mut LexState, out: &mut LineOut) {
        let s = line.as_bytes();
        let len = s.len();
        let mut i = match st.mode {
            M_COMMENT | M_DOC => block_comment(s, 0, 0, st, out),
            M_STR => string_body(s, 0, 0, st, out),
            M_RAW => raw_body(s, 0, 0, st, out),
            _ => 0,
        };
        // A shebang line (`#!` not followed by `[`) on the first line of a file is a comment; only a line
        // that starts in the ground state can be one.
        if i == 0 && st.mode == 0 && starts(s, 0, b"#!") && at(s, 2) != b'[' && out.tokens.is_empty() {
            let j = skip(s, 2, blank);
            if at(s, j) != b'[' {
                out.push(K::Comment, 0, len);
                return;
            }
        }
        while i < len {
            let b = s[i];
            if blank(b) || b == b'\r' {
                i += 1;
                continue;
            }
            if starts(s, i, b"//") {
                let doc = (at(s, i + 2) == b'/' && at(s, i + 3) != b'/') || at(s, i + 2) == b'!';
                out.push(if doc { K::DocComment } else { K::Comment }, i, len);
                return;
            }
            if starts(s, i, b"/*") {
                let doc = (at(s, i + 2) == b'*' && !matches!(at(s, i + 3), b'*' | b'/')) || at(s, i + 2) == b'!';
                st.mode = if doc { M_DOC } else { M_COMMENT };
                st.n = 1;
                i = block_comment(s, i + 2, i, st, out);
                continue;
            }
            if b == b'#' && (at(s, i + 1) == b'[' || (at(s, i + 1) == b'!' && at(s, i + 2) == b'[')) {
                let e = attr_end(s, i);
                out.push(K::Attribute, i, e);
                i = e;
                continue;
            }
            if ident_start(b) {
                // Literal prefixes: b'…', b"…", c"…", and the raw forms r / br / cr with hashes.
                if b == b'b' && at(s, i + 1) == b'\'' {
                    if let Some(e) = char_lit(s, i + 1, i, out) {
                        i = e;
                        continue;
                    }
                }
                if (b == b'b' || b == b'c') && at(s, i + 1) == b'"' {
                    st.mode = M_STR;
                    i = string_body(s, i + 2, i, st, out);
                    continue;
                }
                let p = if starts(s, i, b"br") || starts(s, i, b"cr") {
                    2
                } else if b == b'r' {
                    1
                } else {
                    0
                };
                if p > 0 {
                    let h = skip(s, i + p, |c| c == b'#') - (i + p);
                    if at(s, i + p + h) == b'"' {
                        st.mode = M_RAW;
                        st.n = h as u32;
                        i = raw_body(s, i + p + h + 1, i, st, out);
                        continue;
                    }
                }
                // Raw identifier `r#name`.
                let (ws, we) = if b == b'r' && at(s, i + 1) == b'#' && ident_start(at(s, i + 2)) {
                    (i + 2, skip(s, i + 2, ident_cont))
                } else {
                    (i, skip(s, i, ident_cont))
                };
                let w = &s[ws..we];
                let raw = ws != i;
                let kind = if !raw && at(s, we) == b'!' && at(s, we + 1) != b'=' {
                    out.push(K::Macro, i, we + 1);
                    i = we + 1;
                    continue;
                } else if !raw && one_of(w, KEYWORDS) {
                    K::Keyword
                } else if !raw && one_of(w, PRIMITIVES) {
                    K::Builtin
                } else if w[0].is_ascii_uppercase() {
                    K::Type
                } else if at(s, we) == b'(' {
                    K::Function
                } else {
                    K::Identifier
                };
                out.push(kind, i, we);
                i = we;
                continue;
            }
            if b == b'\'' {
                if let Some(e) = char_lit(s, i, i, out) {
                    i = e;
                    continue;
                }
                let c = at(s, i + 1);
                if ident_start(c) {
                    let from = if c == b'r' && at(s, i + 2) == b'#' && ident_start(at(s, i + 3)) { i + 3 } else { i + 1 };
                    let e = skip(s, from, ident_cont);
                    out.push(K::Lifetime, i, e);
                    i = e;
                    continue;
                }
                out.push(K::Error, i, i + 1);
                i += 1;
                continue;
            }
            if b.is_ascii_digit() {
                let e = number(s, i);
                out.push(K::Number, i, e);
                i = e;
                continue;
            }
            if b == b'"' {
                st.mode = M_STR;
                i = string_body(s, i + 1, i, st, out);
                continue;
            }
            if b == b':' && at(s, i + 1) == b':' {
                out.push(K::Punctuation, i, i + 2);
                i += 2;
                continue;
            }
            if matches!(b, b'(' | b')' | b'[' | b']' | b'{' | b'}' | b',' | b';' | b':') {
                out.push(K::Punctuation, i, i + 1);
                i += 1;
                continue;
            }
            if is_op(b) {
                let st0 = i;
                while i < len && is_op(s[i]) {
                    if i > st0 && s[i] == b'/' && matches!(at(s, i + 1), b'/' | b'*') {
                        break;
                    }
                    i += 1;
                }
                out.push(K::Operator, st0, i);
                continue;
            }
            let e = (i + utf8_len(b)).min(len);
            out.push(K::Error, i, e);
            i = e;
        }
    }
}
