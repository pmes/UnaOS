// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The POSIX shell, from POSIX.1-2024 XCU §2 "Shell Command Language", with the bash extensions an
//! editor meets. §2.2 quoting: backslash, single quotes (no escapes, may span lines), double quotes
//! (where `$`, `` ` `` and `\` keep their meaning — `\` escapes only `$ ` " \` and the newline);
//! bash's `$'…'` (ANSI-C escapes) and `$"…"`. §2.3 token recognition: operators (`| & ; < > ( )` and
//! their two-/three-byte forms, `;;`, `&&`, `||`, `>>`, `<&`, `>&`, `<>`, `>|`, bash `|&`, `&>`,
//! `<<<`), comments (`#` at the start of a word), words. §2.4 reserved words, recognised only in
//! command position (`in` after `for`/`case`/`select`), plus bash's `[[ ]]`, `function`, `select`,
//! `time`, `coproc`. §2.6 expansions: `$name`, the special parameters (`$@ $* $# $? $- $$ $! $0`–`$9`),
//! `${…}` parameter expansion (its operators `:- := :? :+ # ## % %% /` and bash's substring and case
//! forms are operators inside it), `$(…)` and `` `…` `` command substitution — NESTED to any depth, in
//! and out of double quotes — and `$((…))` / `((…))` arithmetic. §2.7.4 here-documents: `<<WORD` and
//! `<<-WORD` (leading tabs stripped when matching the delimiter); a quoted delimiter (`'EOF'`, `"EOF"`,
//! `\EOF`) makes the body literal, otherwise the body expands like a double-quoted string; several
//! here-documents on one line are read in order. Command names are builtins (the POSIX special and
//! regular built-ins plus bash's) or functions; `NAME=value` prefixes are variables.
//!
//! State: `mode` 1 = inside here-document bodies, `tag` = the pending delimiters, each `"<f><word>\n"`
//! with `f` = `'0' + (strip-tabs | quoted << 1)`; `stack` = the open contexts: `'` single quote, `"`
//! double quote, `E` `$'…'`, `(` `$(…)`, `p` a `( … )` subshell, `{` `${…}`, `` ` `` backquotes, `A`
//! arithmetic (`H`, a here-document body line, never outlives its line).

use alloc::string::String;

use crate::lang::one_of;
use crate::scan::{at, blank, skip, utf8_len};
use crate::{HighlightKind as K, Language, LexState, LineOut};

pub struct Shell;

const M_HEREDOC: u8 = 1;

const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until", "do", "done", "in",
    "function", "time", "coproc", "!", "[[", "]]",
];

/// Reserved words after which the next word is again in command position.
const KEYWORDS_CMD: &[&str] = &["if", "then", "else", "elif", "do", "while", "until", "!", "time", "coproc"];

const BUILTINS: &[&str] = &[
    ":", ".", "[", "alias", "bg", "bind", "break", "builtin", "caller", "cd", "command", "compgen", "complete",
    "continue", "declare", "dirs", "disown", "echo", "enable", "eval", "exec", "exit", "export", "false", "fc",
    "fg", "getopts", "hash", "help", "history", "jobs", "kill", "let", "local", "logout", "mapfile", "popd",
    "printf", "pushd", "pwd", "read", "readarray", "readonly", "return", "set", "shift", "shopt", "source",
    "suspend", "test", "times", "trap", "true", "type", "typeset", "ulimit", "umask", "unalias", "unset", "wait",
];

/// Bytes that end an unquoted word.
fn meta(b: u8) -> bool {
    blank(b) || matches!(b, b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')' | b'\'' | b'"' | b'$' | b'`' | b'\\' | b'\r')
}

/// A byte after which a new word starts.
fn word_break(b: u8) -> bool {
    blank(b) || matches!(b, b';' | b'&' | b'|' | b'(' | b')' | b'`')
}

fn heredoc_first(tag: &str) -> Option<(u8, &str)> {
    let line = tag.split('\n').next()?;
    let f = *line.as_bytes().first()?;
    Some((f.wrapping_sub(b'0'), &line[1..]))
}

fn heredoc_pop(tag: &mut String) {
    match tag.find('\n') {
        Some(p) => {
            tag.drain(..=p);
        }
        None => tag.clear(),
    }
}

/// A `$` expansion at `i`. Pushes a context for `$(`, `$((`, `${`, `$'`, `$"`.
fn dollar(s: &[u8], i: usize, st: &mut LexState, out: &mut LineOut) -> usize {
    let n1 = at(s, i + 1);
    if n1 == b'(' && at(s, i + 2) == b'(' {
        out.push(K::Operator, i, i + 1);
        out.push(K::Operator, i + 1, i + 3);
        st.stack.push(b'A');
        return i + 3;
    }
    if n1 == b'(' || n1 == b'{' {
        out.push(K::Operator, i, i + 1);
        out.push(K::Punctuation, i + 1, i + 2);
        st.stack.push(n1);
        return i + 2;
    }
    if n1 == b'\'' {
        out.push(K::String, i, i + 2);
        st.stack.push(b'E');
        return i + 2;
    }
    if n1 == b'"' {
        out.push(K::String, i, i + 2);
        st.stack.push(b'"');
        return i + 2;
    }
    if n1.is_ascii_alphabetic() || n1 == b'_' {
        let e = skip(s, i + 1, |c| c.is_ascii_alphanumeric() || c == b'_');
        out.push(K::Variable, i, e);
        return e;
    }
    if n1.is_ascii_digit() || matches!(n1, b'@' | b'*' | b'#' | b'?' | b'-' | b'$' | b'!') {
        out.push(K::Variable, i, i + 2);
        return i + 2;
    }
    out.push(K::Operator, i, i + 1);
    i + 1
}

/// Double-quoted text (or, `heredoc`, an unquoted here-document body) from `i`. Returns at the closing
/// quote (popping the context), at a pushed substitution, or at the end of the line.
fn dq(s: &[u8], mut i: usize, st: &mut LexState, out: &mut LineOut, heredoc: bool) -> usize {
    let mut seg = i;
    while i < s.len() {
        match s[i] {
            b'"' if !heredoc => {
                out.push(K::String, seg, i + 1);
                st.stack.pop();
                return i + 1;
            }
            b'\\' => {
                let n = at(s, i + 1);
                if i + 1 == s.len() || matches!(n, b'$' | b'`' | b'"' | b'\\') {
                    out.push(K::String, seg, i);
                    let e = (i + 2).min(s.len());
                    out.push(K::Escape, i, e);
                    i = e;
                    seg = i;
                } else {
                    i += 1;
                }
            }
            b'$' if !matches!(at(s, i + 1), b'\'' | b'"') => {
                out.push(K::String, seg, i);
                let depth = st.stack.len();
                i = dollar(s, i, st, out);
                seg = i;
                if st.stack.len() != depth {
                    return i;
                }
            }
            b'`' => {
                out.push(K::String, seg, i);
                out.push(K::Operator, i, i + 1);
                st.stack.push(b'`');
                return i + 1;
            }
            _ => i += 1,
        }
    }
    out.push(K::String, seg, s.len());
    s.len()
}

/// `$'…'` from `i`: every backslash starts an escape.
fn ansi_c(s: &[u8], mut i: usize, st: &mut LexState, out: &mut LineOut) -> usize {
    let mut seg = i;
    while i < s.len() {
        match s[i] {
            b'\\' => {
                out.push(K::String, seg, i);
                let e = (i + 2).min(s.len());
                out.push(K::Escape, i, e);
                i = e;
                seg = i;
            }
            b'\'' => {
                out.push(K::String, seg, i + 1);
                st.stack.pop();
                return i + 1;
            }
            _ => i += 1,
        }
    }
    out.push(K::String, seg, s.len());
    s.len()
}

/// `<<WORD` / `<<-WORD` at `i`: records the delimiter for the body that starts on the next line.
fn heredoc_op(s: &[u8], i: usize, st: &mut LexState, out: &mut LineOut) -> usize {
    let mut k = i + 2;
    let strip = at(s, k) == b'-';
    if strip {
        k += 1;
    }
    out.push(K::Operator, i, k);
    let ws = skip(s, k, blank);
    let we = skip(s, ws, |b| !(blank(b) || matches!(b, b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')' | b'\r')));
    if we == ws {
        return ws;
    }
    let raw = core::str::from_utf8(&s[ws..we]).unwrap_or("");
    let quoted = raw.bytes().any(|b| matches!(b, b'\'' | b'"' | b'\\'));
    let f = (strip as u8) | ((quoted as u8) << 1);
    st.tag.push((b'0' + f) as char);
    for ch in raw.chars().filter(|c| !matches!(c, '\'' | '"' | '\\')) {
        st.tag.push(ch);
    }
    st.tag.push('\n');
    out.push(K::Keyword, ws, we);
    we
}

fn lex(s: &[u8], mut i: usize, st: &mut LexState, out: &mut LineOut) {
    let len = s.len();
    let mut cmd = true;
    let mut last_kw: &[u8] = b"";
    while i < len {
        let top = st.stack.last().copied().unwrap_or(0);
        match top {
            b'\'' => {
                match s[i..].iter().position(|&b| b == b'\'') {
                    Some(p) => {
                        out.push(K::String, i, i + p + 1);
                        st.stack.pop();
                        i += p + 1;
                    }
                    None => {
                        out.push(K::String, i, len);
                        i = len;
                    }
                }
                continue;
            }
            b'"' | b'H' => {
                let d = st.stack.len();
                i = dq(s, i, st, out, top == b'H');
                // A command substitution opened inside the quotes starts a new command.
                if st.stack.len() > d && matches!(st.stack.last(), Some(b'(') | Some(b'`')) {
                    cmd = true;
                }
                continue;
            }
            b'E' => {
                i = ansi_c(s, i, st, out);
                continue;
            }
            _ => {}
        }
        let c = s[i];
        if blank(c) || c == b'\r' {
            i += 1;
            continue;
        }
        let word_start = i == 0 || word_break(s[i - 1]);
        if top == b'{' && !matches!(c, b'"' | b'\'' | b'$' | b'`') {
            if c == b'}' {
                out.push(K::Punctuation, i, i + 1);
                st.stack.pop();
                i += 1;
            } else if c.is_ascii_alphanumeric() || c == b'_' {
                let e = skip(s, i, |b| b.is_ascii_alphanumeric() || b == b'_');
                out.push(K::Variable, i, e);
                i = e;
            } else {
                let e = (i + utf8_len(c)).min(len);
                out.push(K::Operator, i, e);
                i = e;
            }
            continue;
        }
        if top == b'A' && !matches!(c, b'"' | b'\'' | b'$' | b'`' | b'(' | b')') {
            if c.is_ascii_digit() {
                let e = skip(s, i, |b| b.is_ascii_alphanumeric() || b == b'#');
                out.push(K::Number, i, e);
                i = e;
            } else if c.is_ascii_alphabetic() || c == b'_' {
                let e = skip(s, i, |b| b.is_ascii_alphanumeric() || b == b'_');
                out.push(K::Variable, i, e);
                i = e;
            } else {
                let e = (i + utf8_len(c)).min(len);
                out.push(K::Operator, i, e);
                i = e;
            }
            continue;
        }
        match c {
            b'#' if word_start => {
                out.push(K::Comment, i, len);
                return;
            }
            b'\'' | b'"' => {
                out.push(K::String, i, i + 1);
                st.stack.push(c);
                i += 1;
                if word_start {
                    cmd = false;
                }
            }
            b'$' => {
                let d = st.stack.len();
                i = dollar(s, i, st, out);
                if st.stack.len() > d && st.stack.last() == Some(&b'(') {
                    cmd = true;
                } else if word_start {
                    cmd = false;
                }
            }
            b'`' => {
                out.push(K::Operator, i, i + 1);
                if top == b'`' {
                    st.stack.pop();
                    cmd = false;
                } else {
                    st.stack.push(b'`');
                    cmd = true;
                }
                i += 1;
            }
            b'(' => {
                if at(s, i + 1) == b'(' {
                    out.push(K::Operator, i, i + 2);
                    st.stack.push(b'A');
                    i += 2;
                } else {
                    out.push(K::Punctuation, i, i + 1);
                    st.stack.push(b'p');
                    i += 1;
                    cmd = true;
                }
            }
            b')' => {
                if top == b'A' && at(s, i + 1) == b')' {
                    out.push(K::Operator, i, i + 2);
                    st.stack.pop();
                    i += 2;
                } else {
                    if top == b'(' || top == b'p' {
                        st.stack.pop();
                    }
                    out.push(K::Punctuation, i, i + 1);
                    i += 1;
                }
                cmd = false;
            }
            b'<' if at(s, i + 1) == b'<' && at(s, i + 2) != b'<' => {
                i = heredoc_op(s, i, st, out);
            }
            b'|' | b'&' | b';' | b'<' | b'>' => {
                let e = skip(s, i, |b| matches!(b, b'|' | b'&' | b';' | b'<' | b'>')).min(i + 3);
                out.push(K::Operator, i, e);
                if !s[i..e].iter().any(|&b| b == b'<' || b == b'>') {
                    cmd = true;
                    last_kw = b"";
                }
                i = e;
            }
            b'\\' => {
                let e = if i + 1 < len { (i + 1 + utf8_len(s[i + 1])).min(len) } else { len };
                out.push(K::Escape, i, e);
                i = e;
                if word_start {
                    cmd = false;
                }
            }
            b'{' if word_start && (i + 1 == len || blank(s[i + 1])) => {
                out.push(K::Punctuation, i, i + 1);
                i += 1;
                cmd = true;
            }
            b'}' if word_start && (i + 1 == len || meta(s[i + 1])) => {
                out.push(K::Punctuation, i, i + 1);
                i += 1;
                cmd = false;
            }
            _ => {
                let e = skip(s, i, |b| !meta(b)).max(i + 1);
                let w = &s[i..e];
                if cmd {
                    let ne = skip(s, i, |b| b.is_ascii_alphanumeric() || b == b'_');
                    if ne > i && !c.is_ascii_digit() && at(s, ne) == b'=' && ne < e {
                        out.push(K::Variable, i, ne);
                        out.push(K::Operator, ne, ne + 1);
                        out.push(K::Identifier, ne + 1, e);
                        i = e;
                        continue;
                    }
                    let kind = if one_of(w, KEYWORDS) {
                        K::Keyword
                    } else if one_of(w, BUILTINS) {
                        K::Builtin
                    } else if w.iter().all(|b| b.is_ascii_digit()) {
                        K::Number
                    } else {
                        K::Function
                    };
                    out.push(kind, i, e);
                    cmd = kind == K::Keyword && one_of(w, KEYWORDS_CMD);
                    last_kw = if kind == K::Keyword { w } else { b"" };
                } else {
                    let kind = if (w == b"in" && one_of(last_kw, &["for", "case", "select"])) || w == b"]]" {
                        K::Keyword
                    } else if w.iter().all(|b| b.is_ascii_digit()) {
                        K::Number
                    } else {
                        K::Identifier
                    };
                    if kind == K::Keyword {
                        last_kw = b"";
                    }
                    out.push(kind, i, e);
                }
                i = e;
            }
        }
    }
}

impl Language for Shell {
    fn id(&self) -> &'static str {
        "sh"
    }

    fn lex_line(&self, line: &str, st: &mut LexState, out: &mut LineOut) {
        let s = line.as_bytes();
        let len = s.len();
        let base = st.stack.len();
        let mut body = false;
        if st.mode == M_HEREDOC {
            match heredoc_first(&st.tag) {
                Some((f, word)) => {
                    let strip = f & 1 != 0;
                    let quoted = f & 2 != 0;
                    let cs = if strip { skip(s, 0, |b| b == b'\t') } else { 0 };
                    if &s[cs..] == word.as_bytes() {
                        out.push(K::Keyword, cs, len);
                        heredoc_pop(&mut st.tag);
                        if st.tag.is_empty() {
                            st.mode = 0;
                        }
                        return;
                    }
                    if quoted {
                        out.push(K::String, 0, len);
                        return;
                    }
                    st.stack.push(b'H');
                    body = true;
                }
                None => st.mode = 0,
            }
        }
        lex(s, 0, st, out);
        if body {
            st.stack.truncate(base);
            return;
        }
        if !st.tag.is_empty() {
            st.mode = M_HEREDOC;
        }
    }
}
