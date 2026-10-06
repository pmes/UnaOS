// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `prefs_core` — Principia's preference core, shared by Ring 0 and Ring 3 (PREFS, rmbp-ledger B300).
//!
//! CHARTER: Principia — shared-core. Principia (`handlers/principia`, CODEX System handler) owns
//! preferences; this crate is the part of it both rings link: the value model, the namespaced tree and a
//! TOML SUBSET codec. Nothing here knows about files, buses or principals — the kernel (`src/prefs.rs`)
//! and the host handler each bring their own I/O.
//!
//! # The model
//!
//! A preference is a **namespace** (`system`, `aether`) plus a **dotted key** (`display.brightness`)
//! holding one of four scalars ([`PrefValue`]) — the four `bandy::PrefValue` carries, the four a TOML
//! scalar carries losslessly. A key is a LEAF: `a` and `a.b` cannot both hold values, because TOML would
//! need one name to be a value and a table; the collision is refused at set time ([`PrefTree::set`]).
//!
//! # The codec: exactly the subset Principia writes
//!
//! Parse ([`PrefTree::parse`]): `[ns]` / `[ns.sub.table]` headers of bare keys; `key = value` lines with
//! bare (optionally dotted) keys; basic, literal and multi-line strings; decimal / `0x` / `0o` / `0b`
//! integers; floats including `inf` and `nan`; `true` / `false`; `#` comments; blank lines. EVERY other
//! construct — an array, an inline table, an array of tables, a date, a quoted key, a scalar above the
//! first table, a duplicate key or header — is REFUSED with its 1-based line number ([`ParseError`]). The
//! parser never drops anything silently: either the whole file is the tree, or the file is refused.
//! Comments are not preserved on re-emit.
//!
//! Emit ([`PrefTree::to_toml`]): byte-identical to Principia's `PrefStore::to_toml` for every tree —
//! Principia's fixed [`HEADER`], then the layout of the `toml` crate's pretty serializer: keys sorted,
//! a table's scalars before its sub-tables, no header for a table holding no scalars of its own, a blank
//! line before every header but the first, `toml_writer`'s choice of string style and its float spelling.
//! TOML arrays are outside the subset, so a list-valued preference is a comma-joined string
//! (the kernel's `system.dock.pins`).

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod cap; // PREFSCAP (B454): who may write what
pub mod declare;
pub mod files;
pub mod appearance;
pub mod login;
pub mod modes;
pub mod notify; // NOTIFYPANE (B435): the Notifications pane's rules
pub mod rules;
pub mod schema;
pub mod trackpad; // TRACKPADPANE (B412): the Trackpad pane's rules
pub mod wire;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write};

// =====================================================================================================
// VALUE
// =====================================================================================================

/// One preference value: the four TOML scalars, nothing retyped by a save/load cycle.
#[derive(Clone, Debug, PartialEq)]
pub enum PrefValue {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl PrefValue {
    /// `"string" | "int" | "float" | "bool"` — the same names `bandy::PrefValue::type_name` answers.
    pub fn type_name(&self) -> &'static str {
        match self {
            PrefValue::Str(_) => "string",
            PrefValue::Int(_) => "int",
            PrefValue::Float(_) => "float",
            PrefValue::Bool(_) => "bool",
        }
    }

    /// The value as a TOML literal, spelled exactly as [`PrefTree::to_toml`] writes it.
    pub fn to_literal(&self) -> String {
        let mut s = String::new();
        write_value(self, &mut s);
        s
    }

    /// Parse ONE TOML scalar literal (`42`, `1.5`, `true`, `"text"`, `'text'`). Trailing blanks and a
    /// trailing `# comment` are allowed; anything else after the literal is refused.
    pub fn from_literal(s: &str) -> Result<PrefValue, &'static str> {
        let mut p = Parser::new(s);
        p.skip_ws();
        let v = p.value().map_err(|e| e.why)?;
        p.end_of_line().map_err(|e| e.why)?;
        p.skip_blank_lines();
        if p.i < p.s.len() {
            return Err("more than one line");
        }
        Ok(v)
    }

    /// Operator input: a TOML scalar literal when it is one, otherwise the text itself as a string —
    /// `pref set system.audio.mute true` sets a Bool, `pref set system.display.wallpaper SKY.PNG` a Str.
    pub fn infer(s: &str) -> PrefValue {
        PrefValue::from_literal(s).unwrap_or_else(|_| PrefValue::Str(String::from(s)))
    }

    pub fn as_int(&self) -> Option<i64> {
        if let PrefValue::Int(i) = self { Some(*i) } else { None }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let PrefValue::Bool(b) = self { Some(*b) } else { None }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let PrefValue::Str(s) = self { Some(s) } else { None }
    }
    pub fn as_float(&self) -> Option<f64> {
        if let PrefValue::Float(f) = self { Some(*f) } else { None }
    }
}

impl fmt::Display for PrefValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_literal())
    }
}

// =====================================================================================================
// VALIDATION — the rules of `handlers/principia/src/prefs.rs` (validate_ns / validate_key)
// =====================================================================================================

/// Why a namespace, key or set was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefError {
    /// Not a non-empty `[A-Za-z0-9_-]` identifier.
    BadNamespace,
    /// Not one or more dot-separated non-empty `[A-Za-z0-9_-]` segments.
    BadKey,
    /// The key is a strict segment-wise prefix of an existing key (or the reverse).
    Collision,
}

impl PrefError {
    pub fn as_str(self) -> &'static str {
        match self {
            PrefError::BadNamespace => "invalid namespace: expected a non-empty [A-Za-z0-9_-] identifier",
            PrefError::BadKey => "invalid key: expected dot-separated non-empty [A-Za-z0-9_-] segments",
            PrefError::Collision => "key collides with another: one name cannot be both a value and a table",
        }
    }
}

impl fmt::Display for PrefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One bare-key segment: non-empty, `[A-Za-z0-9_-]` only.
pub fn valid_segment(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// A namespace is a single bare-key segment: `aether`, `stria`, `system`.
pub fn validate_ns(ns: &str) -> Result<(), PrefError> {
    if valid_segment(ns) { Ok(()) } else { Err(PrefError::BadNamespace) }
}

/// A key is one or more dot-separated bare-key segments: `homepage`, `window.width`.
pub fn validate_key(key: &str) -> Result<(), PrefError> {
    if !key.is_empty() && key.split('.').all(valid_segment) { Ok(()) } else { Err(PrefError::BadKey) }
}

/// Is one of `a`, `b` a strict segment-wise prefix of the other?
pub fn is_prefix_path(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    long.strip_prefix(short).is_some_and(|rest| rest.starts_with('.'))
}

// =====================================================================================================
// TREE
// =====================================================================================================

/// The header every file carries — byte for byte the one Principia's `PrefStore::to_toml` writes.
pub const HEADER: &str = "\
# UnaOS preferences — written by the principia handler.
# One table per namespace; dotted keys are expanded into sub-tables.
# Hand edits are read on next load (live reload is not implemented yet).

";

/// namespace → dotted key → value, both levels sorted (a stable file diff).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrefTree {
    ns: BTreeMap<String, BTreeMap<String, PrefValue>>,
}

impl PrefTree {
    pub const fn new() -> Self {
        PrefTree { ns: BTreeMap::new() }
    }

    /// Total preferences held, across every namespace.
    pub fn len(&self) -> usize {
        self.ns.values().map(|n| n.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.ns.is_empty()
    }

    /// The value of `ns`/`key`, or `None` if unset. The caller owns the default.
    pub fn get(&self, ns: &str, key: &str) -> Option<&PrefValue> {
        self.ns.get(ns)?.get(key)
    }

    /// Set `ns`/`key`; answers the previous value. Refuses a malformed name or a leaf collision, and on
    /// refusal the tree is unchanged.
    pub fn set(&mut self, ns: &str, key: &str, value: PrefValue) -> Result<Option<PrefValue>, PrefError> {
        validate_ns(ns)?;
        validate_key(key)?;
        if let Some(entries) = self.ns.get(ns) {
            if entries.keys().any(|k| k.as_str() != key && is_prefix_path(k, key)) {
                return Err(PrefError::Collision);
            }
        }
        Ok(self.ns.entry(String::from(ns)).or_default().insert(String::from(key), value))
    }

    /// Unset `ns`/`key`; an emptied namespace disappears.
    pub fn remove(&mut self, ns: &str, key: &str) -> Option<PrefValue> {
        let entries = self.ns.get_mut(ns)?;
        let old = entries.remove(key);
        if entries.is_empty() {
            self.ns.remove(ns);
        }
        old
    }

    /// Every `(key, value)` set in `ns`, sorted by key. An unknown namespace lists empty.
    pub fn list(&self, ns: &str) -> Vec<(&str, &PrefValue)> {
        self.ns.get(ns).map(|n| n.iter().map(|(k, v)| (k.as_str(), v)).collect()).unwrap_or_default()
    }

    /// Every namespace holding at least one preference, sorted.
    pub fn namespaces(&self) -> Vec<&str> {
        self.ns.keys().map(|k| k.as_str()).collect()
    }

    /// Every `(ns, key, value)`, sorted.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str, &PrefValue)> {
        self.ns.iter().flat_map(|(n, e)| e.iter().map(move |(k, v)| (n.as_str(), k.as_str(), v)))
    }

    /// The whole tree as TOML text — byte-identical to Principia's `PrefStore::to_toml`.
    pub fn to_toml(&self) -> String {
        let mut out = String::from(HEADER);
        let mut wrote = false;
        for (ns, entries) in &self.ns {
            let mut root = Node::default();
            for (k, v) in entries {
                root.insert(k, v);
            }
            let mut path = String::from(ns.as_str());
            root.emit(&mut path, &mut out, &mut wrote);
        }
        out
    }

    /// Parse a whole file. See the module doc for the subset; anything outside it is refused with its
    /// line number and NOTHING is adopted.
    pub fn parse(text: &str) -> Result<PrefTree, ParseError> {
        Parser::new(text).document()
    }
}

/// A nested table under construction for emission.
#[derive(Default)]
struct Node<'a> {
    vals: BTreeMap<&'a str, &'a PrefValue>,
    subs: BTreeMap<&'a str, Node<'a>>,
}

impl<'a> Node<'a> {
    fn insert(&mut self, key: &'a str, v: &'a PrefValue) {
        match key.split_once('.') {
            None => {
                self.vals.insert(key, v);
            }
            Some((head, rest)) => self.subs.entry(head).or_default().insert(rest, v),
        }
    }

    fn emit(&self, path: &mut String, out: &mut String, wrote: &mut bool) {
        // `toml`'s pretty layout: a table with no scalars of its own but with sub-tables is implicit
        // (no header); an empty leaf table still gets one.
        if !self.vals.is_empty() || self.subs.is_empty() {
            if *wrote {
                out.push('\n');
            }
            out.push('[');
            out.push_str(path);
            out.push_str("]\n");
            for (k, v) in &self.vals {
                out.push_str(k);
                out.push_str(" = ");
                write_value(v, out);
                out.push('\n');
            }
            *wrote = true;
        }
        for (k, sub) in &self.subs {
            let len = path.len();
            path.push('.');
            path.push_str(k);
            sub.emit(path, out, wrote);
            path.truncate(len);
        }
    }
}

// =====================================================================================================
// EMIT — scalars, as `toml_writer` spells them
// =====================================================================================================

fn write_value(v: &PrefValue, out: &mut String) {
    match v {
        PrefValue::Str(s) => write_string(s, out),
        PrefValue::Int(i) => {
            let _ = write!(out, "{}", i);
        }
        PrefValue::Float(f) => write_float(*f, out),
        PrefValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
    }
}

/// `toml_writer`'s f64: `nan`/`-nan`, `0.0`/`-0.0`, an integral value with `.0`, else Rust's `{}`
/// (`inf`/`-inf` included). The integral test avoids `%` (an `fmod` call a no_std kernel may not link):
/// every |f| >= 2^52 is integral, below that the i64 round trip decides.
fn write_float(f: f64, out: &mut String) {
    if f.is_nan() {
        out.push_str(if f.is_sign_negative() { "-nan" } else { "nan" });
    } else if f == 0.0 {
        out.push_str(if f.is_sign_negative() { "-0.0" } else { "0.0" });
    } else if f.is_finite() && (f.abs() >= 4_503_599_627_370_496.0 || f == (f as i64) as f64) {
        let _ = write!(out, "{}.0", f);
    } else {
        let _ = write!(out, "{}", f);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Enc {
    Literal,
    Basic,
    MlLiteral,
    MlBasic,
}

/// `toml_writer::TomlStringBuilder::as_default`: basic if nothing needs escaping, else literal, else
/// multi-line basic, else multi-line literal, else (multi-line) basic with escapes.
fn write_string(s: &str, out: &mut String) {
    let (mut sq, mut dq, mut max_sq, mut max_dq) = (0u8, 0u8, 0u8, 0u8);
    let (mut codes, mut esc, mut nl) = (false, false, false);
    for &b in s.as_bytes() {
        if b == b'\'' { sq = sq.saturating_add(1); max_sq = max_sq.max(sq); } else { sq = 0; }
        if b == b'"' { dq = dq.saturating_add(1); max_dq = max_dq.max(dq); } else { dq = 0; }
        match b {
            b'\\' => esc = true,
            b'\t' => {}
            b'\n' => nl = true,
            c if c <= 0x1f || c == 0x7f => codes = true,
            _ => {}
        }
    }
    let enc = if !codes && !esc && max_dq == 0 && !nl {
        Enc::Basic
    } else if !codes && max_sq == 0 && !nl {
        Enc::Literal
    } else if !codes && !esc && max_dq <= 2 {
        Enc::MlBasic
    } else if !codes && max_sq <= 2 {
        Enc::MlLiteral
    } else if nl {
        Enc::MlBasic
    } else {
        Enc::Basic
    };
    let (delim, escaped, ml) = match enc {
        Enc::Literal => ("'", false, false),
        Enc::Basic => ("\"", true, false),
        Enc::MlLiteral => ("'''", false, true),
        Enc::MlBasic => ("\"\"\"", true, true),
    };
    out.push_str(delim);
    if nl && ml {
        out.push('\n');
    }
    if !escaped {
        out.push_str(s);
    } else {
        let max_dq_run = if ml { 2 } else { 0 };
        let mut stream = s;
        while !stream.is_empty() {
            let mut end = 0usize;
            let mut escape: Option<&str> = None;
            let mut run = 0u32;
            for (i, &b) in stream.as_bytes().iter().enumerate() {
                if b == b'"' {
                    run += 1;
                    if max_dq_run < run {
                        escape = Some("\\\"");
                        break;
                    }
                } else {
                    run = 0;
                }
                match b {
                    0x08 => { escape = Some("\\b"); break; }
                    0x09 => { escape = Some("\\t"); break; }
                    0x0a if !ml => { escape = Some("\\n"); break; }
                    0x0c => { escape = Some("\\f"); break; }
                    0x0d => { escape = Some("\\r"); break; }
                    0x5c => { escape = Some("\\\\"); break; }
                    0x0a | 0x22 => {}
                    c if c <= 0x1f || c == 0x7f => break,
                    _ => {}
                }
                end = i + 1;
            }
            out.push_str(&stream[..end]);
            let skip = end + escape.is_some() as usize;
            if let Some(e) = escape {
                out.push_str(e);
            }
            stream = &stream[skip..];
            if escape.is_none() && !stream.is_empty() {
                let _ = write!(out, "\\u{:04X}", stream.as_bytes()[0] as u32);
                stream = &stream[1..];
            }
        }
    }
    out.push_str(delim);
}

// =====================================================================================================
// PARSE
// =====================================================================================================

/// A refused file: the 1-based line of the construct and why. Nothing of the file was adopted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub why: &'static str,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.why)
    }
}

struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
    line: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Parser { s: src.as_bytes(), src, i: 0, line: 1 }
    }

    fn err<T>(&self, why: &'static str) -> Result<T, ParseError> {
        Err(ParseError { line: self.line, why })
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn at(&self, lit: &str) -> bool {
        self.s[self.i..].starts_with(lit.as_bytes())
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.i += 1;
        }
    }

    /// Consume a newline (`\n` or `\r\n`) if one is next.
    fn newline(&mut self) -> bool {
        if self.at("\r\n") {
            self.i += 2;
        } else if self.peek() == Some(b'\n') {
            self.i += 1;
        } else {
            return false;
        }
        self.line += 1;
        true
    }

    /// After a header or a key/value: blanks, an optional comment, then a newline or the end.
    fn end_of_line(&mut self) -> Result<(), ParseError> {
        self.skip_ws();
        if self.peek() == Some(b'#') {
            while let Some(c) = self.peek() {
                if c == b'\n' || c == b'\r' {
                    break;
                }
                self.i += 1;
            }
        }
        if self.i >= self.s.len() || self.newline() {
            Ok(())
        } else {
            self.err("unexpected text after the value")
        }
    }

    fn skip_blank_lines(&mut self) {
        loop {
            let save = (self.i, self.line);
            self.skip_ws();
            if self.peek() == Some(b'#') {
                while let Some(c) = self.peek() {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.i += 1;
                }
            }
            if !self.newline() {
                if self.i < self.s.len() {
                    (self.i, self.line) = save;
                }
                return;
            }
        }
    }

    /// A bare (possibly dotted, blanks allowed around the dots) key. Quoted keys are outside the subset.
    fn key(&mut self) -> Result<String, ParseError> {
        let mut k = String::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'"' | b'\'') => return self.err("quoted keys are outside the preference subset"),
                _ => {}
            }
            let start = self.i;
            while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
                self.i += 1;
            }
            if start == self.i {
                return self.err("expected a bare key");
            }
            if !k.is_empty() {
                k.push('.');
            }
            k.push_str(&self.src[start..self.i]);
            self.skip_ws();
            if self.peek() == Some(b'.') {
                self.i += 1;
            } else {
                return Ok(k);
            }
        }
    }

    fn document(mut self) -> Result<PrefTree, ParseError> {
        let mut tree = PrefTree::new();
        // The current table: (namespace, key prefix inside it). None = above the first table.
        let mut table: Option<(String, String)> = None;
        let mut headers: Vec<String> = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Ok(tree),
                Some(b'\n' | b'\r') => {
                    if !self.newline() {
                        return self.err("stray carriage return");
                    }
                }
                Some(b'#') => self.end_of_line()?,
                Some(b'[') => {
                    let line = self.line;
                    if self.at("[[") {
                        return self.err("arrays of tables are outside the preference subset");
                    }
                    self.i += 1;
                    let path = self.key()?;
                    if self.peek() != Some(b']') {
                        return self.err("expected `]` closing the table header");
                    }
                    self.i += 1;
                    self.end_of_line()?;
                    if headers.iter().any(|h| *h == path) {
                        return Err(ParseError { line, why: "table defined twice" });
                    }
                    let (ns, rest) = match path.split_once('.') {
                        Some((n, r)) => (n, r),
                        None => (path.as_str(), ""),
                    };
                    // A header naming an existing VALUE (or a path under one) is the leaf collision.
                    if !rest.is_empty() && tree.list(ns).iter().any(|(k, _)| *k == rest || is_prefix_path(k, rest)) {
                        return Err(ParseError { line, why: "table header names a key that holds a value" });
                    }
                    table = Some((String::from(ns), String::from(rest)));
                    headers.push(path);
                }
                Some(_) => {
                    let line = self.line;
                    let k = self.key()?;
                    if self.peek() != Some(b'=') {
                        return self.err("expected `=` after the key");
                    }
                    self.i += 1;
                    self.skip_ws();
                    let v = self.value()?;
                    self.end_of_line()?;
                    let Some((ns, prefix)) = table.as_ref() else {
                        return Err(ParseError { line, why: "a value above the first [namespace] table" });
                    };
                    let full = if prefix.is_empty() { k } else { alloc::format!("{}.{}", prefix, k) };
                    if tree.get(ns, &full).is_some() {
                        return Err(ParseError { line, why: "duplicate key" });
                    }
                    match tree.set(ns, &full, v) {
                        Ok(_) => {}
                        Err(PrefError::Collision) => {
                            return Err(ParseError { line, why: "key is both a value and a table" })
                        }
                        Err(_) => return Err(ParseError { line, why: "invalid namespace or key" }),
                    }
                }
            }
        }
    }

    fn value(&mut self) -> Result<PrefValue, ParseError> {
        match self.peek() {
            None => self.err("expected a value"),
            Some(b'"') => {
                if self.at("\"\"\"") {
                    self.i += 3;
                    self.ml_string(true).map(PrefValue::Str)
                } else {
                    self.i += 1;
                    self.basic_string().map(PrefValue::Str)
                }
            }
            Some(b'\'') => {
                if self.at("'''") {
                    self.i += 3;
                    self.ml_string(false).map(PrefValue::Str)
                } else {
                    self.i += 1;
                    self.literal_string().map(PrefValue::Str)
                }
            }
            Some(b'[') => self.err("arrays are outside the preference subset"),
            Some(b'{') => self.err("inline tables are outside the preference subset"),
            Some(_) => {
                let start = self.i;
                while let Some(c) = self.peek() {
                    if matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'#' | b',' | b']' | b'}') {
                        break;
                    }
                    self.i += 1;
                }
                let tok = &self.src[start..self.i];
                match tok {
                    "true" => Ok(PrefValue::Bool(true)),
                    "false" => Ok(PrefValue::Bool(false)),
                    _ => match number(tok) {
                        Some(v) => Ok(v),
                        None => self.err("not a string, integer, float or bool (dates are outside the subset)"),
                    },
                }
            }
        }
    }

    fn literal_string(&mut self) -> Result<String, ParseError> {
        let start = self.i;
        loop {
            match self.peek() {
                None | Some(b'\n' | b'\r') => return self.err("unterminated string"),
                Some(b'\'') => {
                    let s = String::from(&self.src[start..self.i]);
                    self.i += 1;
                    return Ok(s);
                }
                Some(c) if (c < 0x20 && c != b'\t') || c == 0x7f => return self.err("control character in a string"),
                Some(_) => self.i += 1,
            }
        }
    }

    fn basic_string(&mut self) -> Result<String, ParseError> {
        let mut out = String::new();
        loop {
            let start = self.i;
            while matches!(self.peek(), Some(c) if c != b'"' && c != b'\\' && c >= 0x20 && c != 0x7f || c == b'\t') {
                self.i += 1;
            }
            out.push_str(&self.src[start..self.i]);
            match self.peek() {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => self.escape(&mut out)?,
                None | Some(b'\n' | b'\r') => return self.err("unterminated string"),
                Some(_) => return self.err("control character in a string"),
            }
        }
    }

    /// A multi-line string after its opening delimiter. A newline right after the opener is trimmed;
    /// up to two quotes may sit before the closing delimiter.
    fn ml_string(&mut self, basic: bool) -> Result<String, ParseError> {
        let q = if basic { b'"' } else { b'\'' };
        let mut out = String::new();
        self.newline();
        loop {
            match self.peek() {
                None => return self.err("unterminated multi-line string"),
                Some(c) if c == q => {
                    let mut n = 0;
                    while self.s.get(self.i + n) == Some(&q) {
                        n += 1;
                    }
                    if n >= 3 {
                        if n > 5 {
                            return self.err("too many quotes closing a multi-line string");
                        }
                        for _ in 0..n - 3 {
                            out.push(q as char);
                        }
                        self.i += n;
                        return Ok(out);
                    }
                    for _ in 0..n {
                        out.push(q as char);
                    }
                    self.i += n;
                }
                Some(b'\\') if basic => {
                    // Line-ending backslash: drop it and every blank and newline after it.
                    let mut j = self.i + 1;
                    while matches!(self.s.get(j), Some(b' ' | b'\t')) {
                        j += 1;
                    }
                    if matches!(self.s.get(j), Some(b'\n')) || self.s[j..].starts_with(b"\r\n") {
                        self.i = j;
                        loop {
                            self.skip_ws();
                            if !self.newline() {
                                break;
                            }
                        }
                    } else {
                        self.escape(&mut out)?;
                    }
                }
                Some(b'\n' | b'\r') => {
                    if !self.newline() {
                        return self.err("stray carriage return in a string");
                    }
                    out.push('\n');
                }
                Some(c) if (c < 0x20 && c != b'\t') || c == 0x7f => return self.err("control character in a string"),
                Some(_) => {
                    let start = self.i;
                    while matches!(self.peek(), Some(c) if c != q && c != b'\\' && c != b'\n' && c != b'\r' && (c >= 0x20 || c == b'\t') && c != 0x7f)
                    {
                        self.i += 1;
                    }
                    if !basic {
                        while self.peek() == Some(b'\\') {
                            self.i += 1;
                            while matches!(self.peek(), Some(c) if c != q && c != b'\\' && c != b'\n' && c != b'\r' && (c >= 0x20 || c == b'\t') && c != 0x7f)
                            {
                                self.i += 1;
                            }
                        }
                    }
                    out.push_str(&self.src[start..self.i]);
                }
            }
        }
    }

    /// One escape at `self.i` (the backslash) into `out`. TOML 1.1's set: \b \t \n \f \r \e \" \\
    /// \xHH \uHHHH \UHHHHHHHH.
    fn escape(&mut self, out: &mut String) -> Result<(), ParseError> {
        self.i += 1;
        let c = match self.peek() {
            Some(b'b') => '\u{8}',
            Some(b't') => '\t',
            Some(b'n') => '\n',
            Some(b'f') => '\u{c}',
            Some(b'r') => '\r',
            Some(b'e') => '\u{1b}',
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(k @ (b'x' | b'u' | b'U')) => {
                let n = match k { b'x' => 2, b'u' => 4, _ => 8 };
                let hex = self.s.get(self.i + 1..self.i + 1 + n).ok_or(ParseError { line: self.line, why: "short unicode escape" })?;
                let mut v = 0u32;
                for &h in hex {
                    let d = (h as char).to_digit(16).ok_or(ParseError { line: self.line, why: "bad unicode escape" })?;
                    v = v * 16 + d;
                }
                self.i += n;
                char::from_u32(v).ok_or(ParseError { line: self.line, why: "escape is not a unicode scalar" })?
            }
            _ => return self.err("unknown escape"),
        };
        self.i += 1;
        out.push(c);
        Ok(())
    }
}

/// Underscores only between digits (TOML's rule); returns the token without them.
fn strip_underscores(t: &str, digit: fn(u8) -> bool) -> Option<String> {
    let b = t.as_bytes();
    let mut s = String::with_capacity(t.len());
    for (i, &c) in b.iter().enumerate() {
        if c == b'_' {
            let ok = i > 0 && digit(b[i - 1]) && b.get(i + 1).is_some_and(|&n| digit(n));
            if !ok {
                return None;
            }
        } else {
            s.push(c as char);
        }
    }
    Some(s)
}

/// A TOML integer or float token.
fn number(tok: &str) -> Option<PrefValue> {
    match tok {
        "inf" | "+inf" => return Some(PrefValue::Float(f64::INFINITY)),
        "-inf" => return Some(PrefValue::Float(f64::NEG_INFINITY)),
        "nan" | "+nan" => return Some(PrefValue::Float(f64::NAN)),
        "-nan" => return Some(PrefValue::Float(-f64::NAN)),
        _ => {}
    }
    for (pre, radix) in [("0x", 16u32), ("0o", 8), ("0b", 2)] {
        if let Some(rest) = tok.strip_prefix(pre) {
            let digit = match radix {
                16 => (|c: u8| c.is_ascii_hexdigit()) as fn(u8) -> bool,
                8 => |c: u8| (b'0'..=b'7').contains(&c),
                _ => |c: u8| c == b'0' || c == b'1',
            };
            let d = strip_underscores(rest, digit)?;
            if d.is_empty() || !d.bytes().all(digit) {
                return None;
            }
            return i64::from_str_radix(&d, radix).ok().map(PrefValue::Int);
        }
    }
    let d = strip_underscores(tok, |c| c.is_ascii_digit())?;
    let unsigned = d.strip_prefix(['+', '-']).unwrap_or(&d);
    if unsigned.is_empty() || !unsigned.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    if !unsigned.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return None;
    }
    let int_part_len = unsigned.bytes().take_while(|c| c.is_ascii_digit()).count();
    if int_part_len > 1 && unsigned.starts_with('0') {
        return None; // leading zeros
    }
    if unsigned.bytes().any(|c| matches!(c, b'.' | b'e' | b'E')) {
        // `.` must sit between digits; the exponent needs digits.
        let ub = unsigned.as_bytes();
        for (i, &c) in ub.iter().enumerate() {
            if c == b'.' && !(i > 0 && ub[i - 1].is_ascii_digit() && ub.get(i + 1).is_some_and(|n| n.is_ascii_digit())) {
                return None;
            }
            if (c == b'+' || c == b'-') && !(i > 0 && matches!(ub[i - 1], b'e' | b'E')) {
                return None;
            }
        }
        return d.parse::<f64>().ok().filter(|f| f.is_finite()).map(PrefValue::Float);
    }
    if unsigned.bytes().any(|c| c == b'+' || c == b'-') {
        return None;
    }
    d.parse::<i64>().ok().map(PrefValue::Int)
}

// =====================================================================================================
// TESTS
// =====================================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    /// Produced by `principia::prefs::PrefStore::to_toml` (once the `toml` crate's pretty serializer behind; since PRINCIPIAFILES B445 this crate's own emitter —
    /// Principia's header) for the tree in [`golden_tree`]; the principia test
    /// `prefs_core_accepts_every_to_toml_output` re-derives it live on every run.
    const PRINCIPIA_GOLDEN: &str = include_str!("../tests/principia_golden.toml");

    fn golden_tree() -> PrefTree {
        let mut t = PrefTree::new();
        t.set("aether", "homepage", PrefValue::Str("https://una.os/".into())).unwrap();
        t.set("aether", "window.width", PrefValue::Int(1280)).unwrap();
        t.set("aether", "window.height", PrefValue::Int(-800)).unwrap();
        t.set("aether", "window.deep.on", PrefValue::Bool(true)).unwrap();
        t.set("aether", "quote", PrefValue::Str("say \"hi\" \\ there".into())).unwrap();
        t.set("aether", "lines", PrefValue::Str("a\nb".into())).unwrap();
        t.set("aether", "ctl", PrefValue::Str("a\tb\u{7}".into())).unwrap();
        t.set("aether", "apos", PrefValue::Str("it's".into())).unwrap();
        t.set("aether", "uni", PrefValue::Str("héllo — ok".into())).unwrap();
        t.set("system", "display.brightness", PrefValue::Int(12)).unwrap();
        t.set("system", "display.scale", PrefValue::Float(2.0)).unwrap();
        t.set("system", "display.gamma", PrefValue::Float(1.5e-7)).unwrap();
        t.set("system", "display.big", PrefValue::Float(1e20)).unwrap();
        t.set("system", "audio.mute", PrefValue::Bool(false)).unwrap();
        t.set("system", "dock.pins", PrefValue::Str("console,shell,quarry".into())).unwrap();
        t
    }

    #[test]
    fn a_principia_file_parses_to_the_same_tree_and_reemits_byte_identical() {
        let t = PrefTree::parse(PRINCIPIA_GOLDEN).expect("principia's own output parses");
        assert_eq!(t, golden_tree());
        assert_eq!(t.to_toml(), PRINCIPIA_GOLDEN, "re-emit must be byte-identical");
        assert_eq!(golden_tree().to_toml(), PRINCIPIA_GOLDEN);
    }

    #[test]
    fn the_four_types_survive_a_cycle() {
        let mut t = PrefTree::new();
        t.set("system", "s", PrefValue::Str("x = \"y\" # not a comment".into())).unwrap();
        t.set("system", "i", PrefValue::Int(i64::MIN)).unwrap();
        t.set("system", "j", PrefValue::Int(i64::MAX)).unwrap();
        t.set("system", "f", PrefValue::Float(2.0)).unwrap();
        t.set("system", "g", PrefValue::Float(-0.25)).unwrap();
        t.set("system", "h", PrefValue::Float(f64::INFINITY)).unwrap();
        t.set("system", "b", PrefValue::Bool(true)).unwrap();
        let back = PrefTree::parse(&t.to_toml()).unwrap();
        assert_eq!(back, t);
        assert_eq!(back.get("system", "f"), Some(&PrefValue::Float(2.0)), "a whole float stays a float");
        // nan != nan: check the type survives.
        let mut n = PrefTree::new();
        n.set("system", "n", PrefValue::Float(f64::NAN)).unwrap();
        assert!(matches!(PrefTree::parse(&n.to_toml()).unwrap().get("system", "n"), Some(PrefValue::Float(f)) if f.is_nan()));
    }

    #[test]
    fn a_malformed_line_is_refused_by_number() {
        let cases: &[(&str, usize)] = &[
            ("[system]\na = 1\nb = = 2\n", 3),
            ("[system]\nrecents = [\"a\"]\n", 2),
            ("[system]\nt = { a = 1 }\n", 2),
            ("# c\n\n[[system]]\n", 3),
            ("top = 1\n[system]\n", 1),
            ("[system]\nd = 1979-05-27\n", 2),
            ("[system]\n\"q\" = 1\n", 2),
            ("[system]\na = 1\na = 2\n", 3),
            ("[system]\na = 1\n\n[system]\n", 4),
            ("[system]\na = 1\na.b = 2\n", 3),
            ("[system]\na = 1\n[system.a]\nx = 1\n", 3),
            ("[system]\ns = \"open\n", 2),
            ("[system]\nx = 1 junk\n", 2),
            ("[system]\nx = 01\n", 2),
            ("[system]\nx = 1__0\n", 2),
            ("this is not = = toml", 1),
        ];
        for (text, line) in cases {
            let e = PrefTree::parse(text).expect_err(text);
            assert_eq!(e.line, *line, "{text:?}: {e}");
        }
    }

    #[test]
    fn the_subset_reads_what_hand_edits_write() {
        let t = PrefTree::parse(
            "# mine\n[system]   # the os\nbrightness=9\n  a.b = 0x1F # hex\nc = 1_000\nd = +1.5e3\ne = 'lit\\eral'\n\
             f = \"\"\"\\\n   joined \\\n   up\"\"\"\ng = '''\nraw ''x'' '''\nh = \"\\u00e9\\x41\\e\"\n\n[other.deep]\nk = -inf\n",
        )
        .unwrap();
        assert_eq!(t.get("system", "brightness"), Some(&PrefValue::Int(9)));
        assert_eq!(t.get("system", "a.b"), Some(&PrefValue::Int(31)));
        assert_eq!(t.get("system", "c"), Some(&PrefValue::Int(1000)));
        assert_eq!(t.get("system", "d"), Some(&PrefValue::Float(1500.0)));
        assert_eq!(t.get("system", "e"), Some(&PrefValue::Str("lit\\eral".into())));
        assert_eq!(t.get("system", "f"), Some(&PrefValue::Str("joined up".into())));
        assert_eq!(t.get("system", "g"), Some(&PrefValue::Str("raw ''x'' ".into())));
        assert_eq!(t.get("system", "h"), Some(&PrefValue::Str("éA\u{1b}".into())));
        assert_eq!(t.get("other", "deep.k"), Some(&PrefValue::Float(f64::NEG_INFINITY)));
        assert_eq!(t.len(), 9);
    }

    #[test]
    fn validation_matches_principia() {
        assert!(validate_ns("system").is_ok());
        for bad in ["", "ae ther", "a.b", "é"] {
            assert_eq!(validate_ns(bad), Err(PrefError::BadNamespace), "{bad:?}");
        }
        assert!(validate_key("window.width").is_ok());
        for bad in ["", "window..width", ".width", "width.", "a b"] {
            assert_eq!(validate_key(bad), Err(PrefError::BadKey), "{bad:?}");
        }
        let mut t = PrefTree::new();
        t.set("aether", "window", PrefValue::Int(1)).unwrap();
        assert_eq!(t.set("aether", "window.width", PrefValue::Int(2)), Err(PrefError::Collision));
        assert_eq!(t.list("aether").len(), 1);
        assert_eq!(t.set("aether", "windows", PrefValue::Int(2)), Ok(None), "a shared prefix is not a path prefix");
    }

    #[test]
    fn literals_and_inference() {
        assert_eq!(PrefValue::from_literal("42"), Ok(PrefValue::Int(42)));
        assert_eq!(PrefValue::from_literal(" true "), Ok(PrefValue::Bool(true)));
        assert_eq!(PrefValue::from_literal("\"a b\" # c"), Ok(PrefValue::Str("a b".into())));
        assert!(PrefValue::from_literal("1 2").is_err());
        assert_eq!(PrefValue::infer("SKY.PNG"), PrefValue::Str("SKY.PNG".into()));
        assert_eq!(PrefValue::infer("1.5"), PrefValue::Float(1.5));
        assert_eq!(PrefValue::Float(3.0).to_literal(), "3.0");
        assert_eq!(PrefValue::Str("it's".into()).to_literal(), "\"it's\"");
        assert_eq!(PrefValue::Int(-7).to_string(), "-7");
    }

    #[test]
    fn remove_empties_a_namespace() {
        let mut t = PrefTree::new();
        t.set("system", "a", PrefValue::Int(1)).unwrap();
        assert_eq!(t.remove("system", "a"), Some(PrefValue::Int(1)));
        assert!(t.is_empty());
        assert_eq!(t.to_toml(), HEADER);
        assert_eq!(PrefTree::parse(HEADER), Ok(PrefTree::new()));
    }
}

/// BRIGHTFLOOR (rmbp-ledger B312) — the schema rule for `system.display.*`, here so BOTH rings apply it:
/// a persisted brightness can never be a dark one. Flight 19: a stored level re-applied at login left the
/// session dark and the card had to be rewritten. The backlight's OFF belongs to the idle blank (DIMIDLE),
/// never to a preference, so the stored range is `BRIGHTNESS_MIN..=BRIGHTNESS_MAX` and anything else —
/// a 0, a negative, a value above the top — is CLAMPED on load and on save, not refused (a refused value
/// would fall back to the default and silently drop the operator's choice; a clamped one keeps it lit).
/// PRINCIPIA2 (SR32): the generic form is [`schema::check`]'s `system.display.brightness` row (a test pins
/// them equal); Principia's host `PrefSet` runs it, the kernel's `prefs::set` adopts it at the fold.
pub mod display {
    /// The `system` key (namespace-relative) the rule governs.
    pub const BRIGHTNESS_KEY: &str = "display.brightness";
    /// The lowest stored level: 1/16 of the panel's range, lit.
    pub const BRIGHTNESS_MIN: i64 = 1;
    /// The top level.
    pub const BRIGHTNESS_MAX: i64 = 16;
    /// The level a reset (safe mode) restores.
    pub const BRIGHTNESS_DEFAULT: i64 = 12;
    /// The idle-blank default, minutes (a reset restores it with the brightness).
    pub const IDLE_MIN_DEFAULT: i64 = 10;
    /// Every `system` key under `display.` a reset puts back to its default (wallpaper = removed).
    pub const RESET_KEYS: [&str; 3] = ["display.brightness", "display.idle_min", "display.wallpaper"];

    /// Clamp a stored brightness into the lit range. Pure.
    pub const fn clamp_brightness(v: i64) -> i64 {
        if v < BRIGHTNESS_MIN { BRIGHTNESS_MIN } else if v > BRIGHTNESS_MAX { BRIGHTNESS_MAX } else { v }
    }

    /// The `(key, default)` pairs a reset (safe mode) writes back — wallpaper `""` is "off", the
    /// consumer's default. Pure.
    pub fn defaults() -> [(&'static str, super::PrefValue); 3] {
        use super::PrefValue as P;
        [
            (RESET_KEYS[0], P::Int(BRIGHTNESS_DEFAULT)),
            (RESET_KEYS[1], P::Int(IDLE_MIN_DEFAULT)),
            (RESET_KEYS[2], P::Str(alloc::string::String::new())),
        ]
    }

    /// Reset `system.display.*` in `t` to [`defaults`]; returns how many keys were written. Pure — the
    /// kernel's live reset is the same pairs through its own store's `set`.
    pub fn reset(t: &mut super::PrefTree) -> usize {
        let mut n = 0;
        for (k, v) in defaults() {
            if t.set("system", k, v).is_ok() { n += 1; }
        }
        n
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn reset_restores_a_lit_panel() {
            let mut t = crate::PrefTree::parse("[system]\ndisplay.brightness = 0\ndisplay.idle_min = 0\ndisplay.wallpaper = \"X.PNG\"\n").unwrap();
            assert_eq!(reset(&mut t), 3);
            assert_eq!(t.get("system", BRIGHTNESS_KEY), Some(&crate::PrefValue::Int(BRIGHTNESS_DEFAULT)));
            assert_eq!(t.get("system", "display.idle_min"), Some(&crate::PrefValue::Int(IDLE_MIN_DEFAULT)));
            assert_eq!(t.get("system", "display.wallpaper"), Some(&crate::PrefValue::Str(alloc::string::String::new())));
        }
        #[test]
        fn a_dark_value_never_survives() {
            assert_eq!(clamp_brightness(0), 1);
            assert_eq!(clamp_brightness(-5), 1);
            assert_eq!(clamp_brightness(1), 1);
            assert_eq!(clamp_brightness(11), 11);
            assert_eq!(clamp_brightness(16), 16);
            assert_eq!(clamp_brightness(99), 16);
            assert_eq!(clamp_brightness(BRIGHTNESS_DEFAULT), BRIGHTNESS_DEFAULT);
        }
    }
}
