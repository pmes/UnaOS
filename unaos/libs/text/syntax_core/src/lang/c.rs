// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! C, from ISO/IEC 9899:2018 (C17) §6.4 "Lexical elements": keywords (§6.4.1, the type specifiers as
//! builtins), identifiers with universal character names (§6.4.2, §6.4.3: `\uXXXX`, `\UXXXXXXXX`),
//! preprocessing numbers (§6.4.8: `[.]digit` then `[A-Za-z0-9_.]`, `e+ e- E+ E- p+ p- P+ P-`), character
//! constants (§6.4.4.4) and string literals (§6.4.5) with their prefixes (`L u U`, `u8` for strings) and
//! escapes (simple, octal 1–3 digits, `\x` hex, UCNs), comments (§6.4.9: `/* */` does NOT nest, `//`),
//! and punctuators (§6.4.6, digraphs included as operators). Translation phase 2 (backslash-newline
//! splicing) is honoured where it changes what a line is: a `//` comment, a string, or a preprocessing
//! directive ending in `\` continues on the next line. Trigraphs (phase 1) are IGNORED — `??=` lexes as
//! `?` `?` `=` — as every current compiler does by default (C23 removes them). A preprocessing directive
//! (a line whose first token is `#` or `%:`) is ONE token of kind macro, its continuations included.
//!
//! State: `mode` 1 = block comment, 2 = string or character constant continued by `\` (`n` = the quote
//! byte, `flags` 1 = character constant), 4 = directive continued, 5 = `//` comment continued.

use crate::lang::one_of;
use crate::scan::{at, blank, ident_cont, ident_start, skip, starts, utf8_len};
use crate::{HighlightKind as K, Language, LexState, LineOut};

pub struct C;

const KEYWORDS: &[&str] = &[
    "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for", "goto", "if",
    "inline", "register", "restrict", "return", "sizeof", "static", "struct", "switch", "typedef", "union",
    "volatile", "while", "_Alignas", "_Alignof", "_Atomic", "_Generic", "_Noreturn", "_Static_assert",
    "_Thread_local",
];

const TYPES: &[&str] =
    &["char", "double", "float", "int", "long", "short", "signed", "unsigned", "void", "_Bool", "_Complex", "_Imaginary"];

const M_COMMENT: u8 = 1;
const M_STR: u8 = 2;
const M_PP: u8 = 4;
const M_LINE: u8 = 5;

/// Does the line end in a backslash (phase-2 splice)?
fn spliced(s: &[u8]) -> bool {
    s.last() == Some(&b'\\')
}

/// End of the escape sequence at `i` (`s[i] == b'\\'`), §6.4.4.4.
fn escape_end(s: &[u8], i: usize) -> usize {
    let n = at(s, i + 1);
    match n {
        0 => i + 1,
        b'0'..=b'7' => {
            let mut k = i + 2;
            while k < i + 4 && matches!(at(s, k), b'0'..=b'7') {
                k += 1;
            }
            k
        }
        b'x' => skip(s, i + 2, |c| c.is_ascii_hexdigit()),
        b'u' => (i + 6).min(s.len()),
        b'U' => (i + 10).min(s.len()),
        b => (i + 1 + utf8_len(b)).min(s.len()),
    }
}

/// A string literal or character constant body from `from` up to the closing `quote`.
fn quoted(s: &[u8], from: usize, tok: usize, quote: u8, st: &mut LexState, out: &mut LineOut) -> usize {
    let kind = if quote == b'\'' { K::Char } else { K::String };
    let mut seg = tok;
    let mut i = from;
    while i < s.len() {
        let b = s[i];
        if b == b'\\' {
            out.push(kind, seg, i);
            let e = escape_end(s, i);
            out.push(K::Escape, i, e);
            i = e;
            seg = i;
        } else if b == quote {
            out.push(kind, seg, i + 1);
            st.mode = 0;
            return i + 1;
        } else {
            i += 1;
        }
    }
    out.push(kind, seg, s.len());
    if spliced(s) {
        st.mode = M_STR;
        st.n = quote as u32;
        st.flags = (quote == b'\'') as u32;
    } else {
        st.mode = 0;
        st.n = 0;
        st.flags = 0;
    }
    s.len()
}

fn block_comment(s: &[u8], from: usize, tok: usize, st: &mut LexState, out: &mut LineOut) -> usize {
    match s[from.min(s.len())..].windows(2).position(|w| w == b"*/") {
        Some(p) => {
            let e = from + p + 2;
            out.push(K::Comment, tok, e);
            st.mode = 0;
            e
        }
        None => {
            out.push(K::Comment, tok, s.len());
            st.mode = M_COMMENT;
            s.len()
        }
    }
}

/// End of an identifier from `i`, universal character names included.
fn ident_end(s: &[u8], mut i: usize) -> usize {
    loop {
        let b = at(s, i);
        if ident_cont(b) {
            i += 1;
        } else if b == b'\\' && at(s, i + 1) == b'u' {
            i = (i + 6).min(s.len());
        } else if b == b'\\' && at(s, i + 1) == b'U' {
            i = (i + 10).min(s.len());
        } else {
            return i;
        }
    }
}

fn pp_number(s: &[u8], mut i: usize) -> usize {
    i += 1;
    loop {
        let c = at(s, i);
        if matches!(c, b'e' | b'E' | b'p' | b'P') && matches!(at(s, i + 1), b'+' | b'-') {
            i += 2;
        } else if ident_cont(c) || c == b'.' {
            i += 1;
        } else {
            return i;
        }
    }
}

fn is_op(b: u8) -> bool {
    matches!(b, b'+' | b'-' | b'*' | b'/' | b'%' | b'^' | b'!' | b'&' | b'|' | b'=' | b'<' | b'>' | b'.' | b'~' | b'?' | b'#')
}

impl Language for C {
    fn id(&self) -> &'static str {
        "c"
    }

    fn lex_line(&self, line: &str, st: &mut LexState, out: &mut LineOut) {
        let s = line.as_bytes();
        let len = s.len();
        let mut i = 0;
        match st.mode {
            M_PP => {
                out.push(K::Macro, 0, len);
                if !spliced(s) {
                    st.mode = 0;
                }
                return;
            }
            M_LINE => {
                out.push(K::Comment, 0, len);
                if !spliced(s) {
                    st.mode = 0;
                }
                return;
            }
            M_COMMENT => i = block_comment(s, 0, 0, st, out),
            M_STR => {
                let q = st.n as u8;
                i = quoted(s, 0, 0, if q == 0 { b'"' } else { q }, st, out);
            }
            _ => {
                let j = skip(s, 0, blank);
                if at(s, j) == b'#' || starts(s, j, b"%:") {
                    out.push(K::Macro, j, len);
                    if spliced(s) {
                        st.mode = M_PP;
                    }
                    return;
                }
            }
        }
        if st.mode == 0 {
            st.n = 0;
            st.flags = 0;
        }
        while i < len {
            let b = s[i];
            if blank(b) || b == b'\r' || b == 0x0c || b == 0x0b {
                i += 1;
                continue;
            }
            if starts(s, i, b"//") {
                out.push(K::Comment, i, len);
                if spliced(s) {
                    st.mode = M_LINE;
                }
                return;
            }
            if starts(s, i, b"/*") {
                i = block_comment(s, i + 2, i, st, out);
                continue;
            }
            // Encoding prefixes: u8"…", u"…", U"…", L"…", and u'…', U'…', L'…'.
            let pfx = if starts(s, i, b"u8\"") {
                2
            } else if matches!(b, b'u' | b'U' | b'L') && matches!(at(s, i + 1), b'"' | b'\'') {
                1
            } else {
                0
            };
            if pfx > 0 {
                let q = s[i + pfx];
                i = quoted(s, i + pfx + 1, i, q, st, out);
                continue;
            }
            if b == b'"' || b == b'\'' {
                i = quoted(s, i + 1, i, b, st, out);
                continue;
            }
            if ident_start(b) || (b == b'\\' && matches!(at(s, i + 1), b'u' | b'U')) {
                let e = ident_end(s, i);
                let w = &s[i..e];
                let kind = if one_of(w, KEYWORDS) {
                    K::Keyword
                } else if one_of(w, TYPES) {
                    K::Builtin
                } else if w.len() > 2 && w.ends_with(b"_t") {
                    K::Type
                } else if at(s, e) == b'(' {
                    K::Function
                } else {
                    K::Identifier
                };
                out.push(kind, i, e);
                i = e;
                continue;
            }
            if b.is_ascii_digit() || (b == b'.' && at(s, i + 1).is_ascii_digit()) {
                let e = pp_number(s, i);
                out.push(K::Number, i, e);
                i = e;
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
