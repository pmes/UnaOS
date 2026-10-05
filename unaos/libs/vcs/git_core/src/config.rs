// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! git-config files (`git-config(1)`, "CONFIGURATION FILE"): sections, quoted subsections, the
//! legacy `[section.sub]` form, keys with and without values, quoted values with `\n \t \b \" \\`
//! escapes, backslash-newline continuation, `;`/`#` comments, whitespace folding exactly as git's
//! parser folds it; `include.path` / `includeIf.<cond>.path` through a caller-supplied loader; typed
//! getters (bool, int with k/m/g); and a minimal-diff editor for `set`/`unset` that leaves every
//! other byte of the file untouched.
//!
//! Oracle (tests/config.rs): `git config --list [--includes]` over adversarial files, and `git
//! config --get` reading files this module edited.

use alloc::vec::Vec;

use crate::{Error, Result};

/// One `key[=value]` occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Section name, lower-cased.
    pub section: Vec<u8>,
    /// Subsection: case-sensitive for `[s "sub"]`, lower-cased for legacy `[s.sub]`.
    pub subsection: Option<Vec<u8>>,
    /// Key name, lower-cased.
    pub name: Vec<u8>,
    /// The value; `None` for a bare key (boolean true).
    pub value: Option<Vec<u8>>,
    /// Index of the file it came from in [`Config::files`].
    pub file: usize,
    /// Byte span of its line(s) in that file.
    pub span: (usize, usize),
}

impl Entry {
    /// `section[.subsection].name`, as `git config --list` prints the key.
    pub fn key(&self) -> Vec<u8> {
        let mut k = self.section.clone();
        if let Some(s) = &self.subsection {
            k.push(b'.');
            k.extend_from_slice(s);
        }
        k.push(b'.');
        k.extend_from_slice(&self.name);
        k
    }
}

/// A section header seen while parsing (for the editor).
#[derive(Debug, Clone)]
struct SectionSpan {
    section: Vec<u8>,
    subsection: Option<Vec<u8>>,
    /// End of the last line belonging to this section.
    end: usize,
}

/// The result of parsing one file.
#[derive(Debug, Clone, Default)]
pub struct Parsed {
    /// Entries in order.
    pub entries: Vec<Entry>,
    sections: Vec<SectionSpan>,
}

struct Rd<'a> {
    d: &'a [u8],
    p: usize,
    eof: bool,
}

impl Rd<'_> {
    /// git's `get_next_char`: CRLF folds to LF, end of input reads as LF and sets `eof`.
    fn next(&mut self) -> u8 {
        if self.p >= self.d.len() {
            self.eof = true;
            return b'\n';
        }
        let mut c = self.d[self.p];
        self.p += 1;
        if c == b'\r' && self.d.get(self.p) == Some(&b'\n') {
            self.p += 1;
            c = b'\n';
        }
        c
    }
}

fn iskeychar(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-'
}

fn isspace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Parse one config file.
pub fn parse(data: &[u8], file: usize) -> Result<Parsed> {
    let mut r = Rd { d: data, p: 0, eof: false };
    if data.starts_with(&[0xef, 0xbb, 0xbf]) {
        r.p = 3;
    }
    let mut out = Parsed::default();
    let mut section: Vec<u8> = Vec::new();
    let mut subsection: Option<Vec<u8>> = None;
    let mut comment = false;
    loop {
        let line_start = r.p;
        let c = r.next();
        if c == b'\n' {
            if r.eof {
                return Ok(out);
            }
            comment = false;
            continue;
        }
        if comment || isspace(c) {
            continue;
        }
        if c == b'#' || c == b';' {
            comment = true;
            continue;
        }
        if c == b'[' {
            // get_base_var
            let mut name = Vec::new();
            let mut sub: Option<Vec<u8>> = None;
            loop {
                let c = r.next();
                if r.eof {
                    return Err(Error::Corrupt("config: unterminated section header"));
                }
                if c == b']' {
                    break;
                }
                if isspace(c) {
                    // get_extended_base_var
                    let mut c = c;
                    loop {
                        if c == b'\n' {
                            return Err(Error::Corrupt("config: incomplete section header"));
                        }
                        c = r.next();
                        if !isspace(c) {
                            break;
                        }
                    }
                    if c != b'"' {
                        return Err(Error::Corrupt("config: subsection must be quoted"));
                    }
                    let mut s = Vec::new();
                    loop {
                        let mut c = r.next();
                        if c == b'\n' {
                            return Err(Error::Corrupt("config: incomplete subsection"));
                        }
                        if c == b'"' {
                            break;
                        }
                        if c == b'\\' {
                            c = r.next();
                            if c == b'\n' {
                                return Err(Error::Corrupt("config: incomplete subsection"));
                            }
                        }
                        s.push(c);
                    }
                    if r.next() != b']' {
                        return Err(Error::Corrupt("config: section header: ] expected"));
                    }
                    sub = Some(s);
                    break;
                }
                if !iskeychar(c) && c != b'.' {
                    return Err(Error::Corrupt("config: bad section name"));
                }
                name.push(c.to_ascii_lowercase());
            }
            if name.is_empty() {
                return Err(Error::Corrupt("config: empty section name"));
            }
            if sub.is_some() && name.contains(&b'.') {
                // `[a.b "c"]`: git keeps the whole dotted base; model it as section a, subsection b.c
                let dot = name.iter().position(|&b| b == b'.').unwrap();
                let mut s = name[dot + 1..].to_vec();
                s.push(b'.');
                s.extend_from_slice(sub.as_ref().unwrap());
                sub = Some(s);
                name.truncate(dot);
            } else if sub.is_none() {
                if let Some(dot) = name.iter().position(|&b| b == b'.') {
                    sub = Some(name[dot + 1..].to_vec());
                    name.truncate(dot);
                }
            }
            section = name;
            subsection = sub;
            out.sections.push(SectionSpan { section: section.clone(), subsection: subsection.clone(), end: r.p });
            continue;
        }
        if !c.is_ascii_alphabetic() {
            return Err(Error::Corrupt("config: key must start with a letter"));
        }
        if section.is_empty() {
            return Err(Error::Corrupt("config: key outside any section"));
        }
        let start = line_start;
        let mut name = alloc::vec![c.to_ascii_lowercase()];
        let mut c;
        loop {
            c = r.next();
            if r.eof || !iskeychar(c) {
                break;
            }
            name.push(c.to_ascii_lowercase());
        }
        while c == b' ' || c == b'\t' {
            c = r.next();
        }
        let value = if c != b'\n' {
            if c != b'=' {
                return Err(Error::Corrupt("config: = expected after key"));
            }
            Some(parse_value(&mut r)?)
        } else {
            None
        };
        let end = r.p;
        out.entries.push(Entry { section: section.clone(), subsection: subsection.clone(), name, value, file, span: (start, end) });
        if let Some(s) = out.sections.last_mut() {
            s.end = end;
        }
        if r.eof {
            return Ok(out);
        }
    }
}

fn parse_value(r: &mut Rd<'_>) -> Result<Vec<u8>> {
    let mut v = Vec::new();
    let (mut quote, mut comment, mut space) = (false, false, 0usize);
    loop {
        let c = r.next();
        if c == b'\n' {
            if quote {
                return Err(Error::Corrupt("config: unterminated quote"));
            }
            return Ok(v);
        }
        if comment {
            continue;
        }
        if isspace(c) && !quote {
            if !v.is_empty() {
                space += 1;
            }
            continue;
        }
        if !quote && (c == b';' || c == b'#') {
            comment = true;
            continue;
        }
        for _ in 0..space {
            v.push(b' ');
        }
        space = 0;
        if c == b'\\' {
            let e = r.next();
            let out = match e {
                b'\n' => {
                    if r.eof {
                        return Err(Error::Corrupt("config: bad escape at end of file"));
                    }
                    continue;
                }
                b't' => b'\t',
                b'b' => 0x08,
                b'n' => b'\n',
                b'\\' | b'"' => e,
                _ => return Err(Error::Corrupt("config: bad escape")),
            };
            v.push(out);
            continue;
        }
        if c == b'"' {
            quote = !quote;
            continue;
        }
        v.push(c);
    }
}

/// Resolves includes for [`Config::load`].
pub trait Includer {
    /// Load the file `path` (as written in the including file `from`): (canonical path, bytes).
    fn load(&mut self, from: &[u8], path: &[u8]) -> Option<(Vec<u8>, Vec<u8>)>;
    /// Evaluate an `includeIf` condition (`gitdir:…`, `gitdir/i:…`, `onbranch:…`, …) for `from`.
    fn condition(&mut self, from: &[u8], cond: &[u8]) -> bool;
}

/// An includer that refuses every include (pure parsing).
pub struct NoIncludes;

impl Includer for NoIncludes {
    fn load(&mut self, _: &[u8], _: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        None
    }
    fn condition(&mut self, _: &[u8], _: &[u8]) -> bool {
        false
    }
}

/// A layered configuration (system, global, local, …, each with its includes expanded in place).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Every entry in precedence order (later wins).
    pub entries: Vec<Entry>,
    /// The file paths, indexed by [`Entry::file`].
    pub files: Vec<Vec<u8>>,
}

const MAX_INCLUDE_DEPTH: usize = 10;

impl Config {
    /// An empty configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse `data` (the file at `path`) and append it, expanding includes through `inc`.
    pub fn load(&mut self, path: &[u8], data: &[u8], inc: &mut dyn Includer) -> Result<()> {
        self.load_depth(path, data, inc, 0)
    }

    fn load_depth(&mut self, path: &[u8], data: &[u8], inc: &mut dyn Includer, depth: usize) -> Result<()> {
        if depth > MAX_INCLUDE_DEPTH {
            return Err(Error::Corrupt("config: include depth exceeded"));
        }
        let fi = self.files.len();
        self.files.push(path.to_vec());
        let p = parse(data, fi)?;
        for e in p.entries {
            let include = e.name == b"path"
                && e.value.is_some()
                && ((e.section == b"include" && e.subsection.is_none())
                    || (e.section == b"includeif" && e.subsection.is_some()));
            let target = if include {
                let go = e.section == b"include" || inc.condition(path, e.subsection.as_deref().unwrap());
                if go { e.value.clone() } else { None }
            } else {
                None
            };
            self.entries.push(e);
            if let Some(t) = target {
                if let Some((cp, bytes)) = inc.load(path, &t) {
                    self.load_depth(&cp, &bytes, inc, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    /// Entries matching `key` (`section.name` or `section.sub.name`).
    pub fn matching<'a>(&'a self, key: &'a [u8]) -> impl Iterator<Item = &'a Entry> + 'a {
        let (s, sub, n) = split_key(key);
        self.entries.iter().filter(move |e| {
            e.section.eq_ignore_ascii_case(s) && e.name.eq_ignore_ascii_case(n) && e.subsection.as_deref() == sub
        })
    }

    /// The last value for `key`: `None` when absent, `Some(None)` for a bare key.
    pub fn get_raw<'a>(&'a self, key: &[u8]) -> Option<Option<&'a [u8]>> {
        let (s, sub, n) = split_key(key);
        self.entries
            .iter()
            .rev()
            .find(|e| e.section.eq_ignore_ascii_case(s) && e.name.eq_ignore_ascii_case(n) && e.subsection.as_deref() == sub)
            .map(|e| e.value.as_deref())
    }

    /// The last value as bytes (a bare key reads as empty).
    pub fn get<'a>(&'a self, key: &[u8]) -> Option<&'a [u8]> {
        self.get_raw(key).map(|v| v.unwrap_or(b""))
    }

    /// Every value.
    pub fn get_all<'a>(&'a self, key: &'a [u8]) -> Vec<&'a [u8]> {
        self.matching(key).map(|e| e.value.as_deref().unwrap_or(b"")).collect()
    }

    /// Typed boolean (`git config --type=bool`).
    pub fn get_bool(&self, key: &[u8]) -> Option<Result<bool>> {
        self.get_raw(key).map(parse_bool)
    }

    /// Typed integer with k/m/g suffixes (`--type=int`).
    pub fn get_int(&self, key: &[u8]) -> Option<Result<i64>> {
        self.get_raw(key).map(|v| parse_int(v.unwrap_or(b"")))
    }

    /// Lines as `git config --list` prints them.
    pub fn list(&self) -> Vec<u8> {
        let mut o = Vec::new();
        for e in &self.entries {
            o.extend_from_slice(&e.key());
            if let Some(v) = &e.value {
                o.push(b'=');
                o.extend_from_slice(v);
            }
            o.push(b'\n');
        }
        o
    }
}

/// Split `section[.sub].name`.
pub fn split_key(key: &[u8]) -> (&[u8], Option<&[u8]>, &[u8]) {
    let first = key.iter().position(|&b| b == b'.').unwrap_or(key.len());
    let last = key.iter().rposition(|&b| b == b'.').unwrap_or(key.len());
    let section = &key[..first];
    if first == last {
        (section, None, key.get(first + 1..).unwrap_or(b""))
    } else {
        (section, Some(&key[first + 1..last]), &key[last + 1..])
    }
}

/// git's boolean rule.
pub fn parse_bool(v: Option<&[u8]>) -> Result<bool> {
    let Some(v) = v else { return Ok(true) };
    let l: Vec<u8> = v.to_ascii_lowercase();
    match l.as_slice() {
        b"true" | b"yes" | b"on" => Ok(true),
        b"false" | b"no" | b"off" | b"" => Ok(false),
        _ => parse_int(v).map(|i| i != 0).map_err(|_| Error::Corrupt("config: bad boolean")),
    }
}

/// git's integer rule (decimal, optional sign, k/m/g suffix).
pub fn parse_int(v: &[u8]) -> Result<i64> {
    let (neg, mut d) = match v.first() {
        Some(b'-') => (true, &v[1..]),
        Some(b'+') => (false, &v[1..]),
        _ => (false, v),
    };
    let mut mul: i64 = 1;
    if let Some((&last, rest)) = d.split_last() {
        match last.to_ascii_lowercase() {
            b'k' => (mul, d) = (1 << 10, rest),
            b'm' => (mul, d) = (1 << 20, rest),
            b'g' => (mul, d) = (1 << 30, rest),
            _ => {}
        }
    }
    if d.is_empty() {
        return Err(Error::Corrupt("config: bad integer"));
    }
    let mut n: i64 = 0;
    for &c in d {
        if !c.is_ascii_digit() {
            return Err(Error::Corrupt("config: bad integer"));
        }
        n = n.checked_mul(10).and_then(|n| n.checked_add((c - b'0') as i64)).ok_or(Error::Corrupt("config: integer overflow"))?;
    }
    let n = n.checked_mul(mul).ok_or(Error::Corrupt("config: integer overflow"))?;
    Ok(if neg { -n } else { n })
}

/// Quote a value the way `git config` writes it.
pub fn quote_value(v: &[u8]) -> Vec<u8> {
    let needs = v.first().is_some_and(|&c| isspace(c)) || v.last().is_some_and(|&c| isspace(c)) || v.iter().any(|&c| c == b';' || c == b'#');
    let mut o = Vec::new();
    if needs {
        o.push(b'"');
    }
    for &c in v {
        match c {
            b'\n' => o.extend_from_slice(b"\\n"),
            b'\t' => o.extend_from_slice(b"\\t"),
            b'"' => o.extend_from_slice(b"\\\""),
            b'\\' => o.extend_from_slice(b"\\\\"),
            _ => o.push(c),
        }
    }
    if needs {
        o.push(b'"');
    }
    o
}

/// Set `key = value` in the file text, changing nothing else: replace the last occurrence, else
/// append to the last matching section, else add the section at the end.
pub fn set(text: &[u8], key: &[u8], value: &[u8]) -> Result<Vec<u8>> {
    let p = parse(text, 0)?;
    let (s, sub, n) = split_key(key);
    if s.is_empty() || n.is_empty() {
        return Err(Error::Corrupt("config: bad key"));
    }
    let mut line = Vec::new();
    line.push(b'\t');
    line.extend_from_slice(n);
    line.extend_from_slice(b" = ");
    line.extend_from_slice(&quote_value(value));
    line.push(b'\n');
    let hit = p.entries.iter().rev().find(|e| {
        e.section.eq_ignore_ascii_case(s) && e.name.eq_ignore_ascii_case(n) && e.subsection.as_deref() == sub
    });
    let mut out = Vec::with_capacity(text.len() + line.len() + 32);
    if let Some(e) = hit {
        out.extend_from_slice(&text[..e.span.0]);
        // keep the original line's indentation-free start: the span begins at the key itself
        out.extend_from_slice(&line[1..]);
        out.extend_from_slice(&text[e.span.1..]);
        return Ok(out);
    }
    let lower: Vec<u8> = s.to_ascii_lowercase();
    if let Some(sec) = p.sections.iter().rev().find(|x| x.section == lower && x.subsection.as_deref() == sub) {
        out.extend_from_slice(&text[..sec.end]);
        if sec.end > 0 && text[sec.end - 1] != b'\n' {
            out.push(b'\n');
        }
        out.extend_from_slice(&line);
        out.extend_from_slice(&text[sec.end..]);
        return Ok(out);
    }
    out.extend_from_slice(text);
    if !out.is_empty() && *out.last().unwrap() != b'\n' {
        out.push(b'\n');
    }
    out.push(b'[');
    out.extend_from_slice(s);
    if let Some(sub) = sub {
        out.extend_from_slice(b" \"");
        for &c in sub {
            if c == b'"' || c == b'\\' {
                out.push(b'\\');
            }
            out.push(c);
        }
        out.push(b'"');
    }
    out.extend_from_slice(b"]\n");
    out.extend_from_slice(&line);
    Ok(out)
}

/// Remove every occurrence of `key`.
pub fn unset_all(text: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    let p = parse(text, 0)?;
    let (s, sub, n) = split_key(key);
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    for e in p.entries.iter().filter(|e| e.section.eq_ignore_ascii_case(s) && e.name.eq_ignore_ascii_case(n) && e.subsection.as_deref() == sub) {
        // remove from the start of the line holding the key
        let mut ls = e.span.0;
        while ls > 0 && text[ls - 1] != b'\n' && (text[ls - 1] == b' ' || text[ls - 1] == b'\t') {
            ls -= 1;
        }
        out.extend_from_slice(&text[at..ls]);
        at = e.span.1;
    }
    out.extend_from_slice(&text[at..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn values() {
        let t = b"[core]\n\tbare = false ; c\n\tx = \" a \"b\\\\  c  \n[remote \"Or\\\"ig\"]\n\turl = u\\\n v\n[A.B]k\n";
        let mut c = Config::new();
        c.load(b"x", t, &mut NoIncludes).unwrap();
        assert_eq!(c.list(), b"core.bare=false\ncore.x= a b\\  c\nremote.Or\"ig.url=u v\na.b.k\n".to_vec());
        assert_eq!(c.get_bool(b"a.b.k"), Some(Ok(true)));
        assert_eq!(parse_int(b"2k"), Ok(2048));
    }
}
