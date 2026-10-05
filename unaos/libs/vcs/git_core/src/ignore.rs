// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Path patterns: git's `wildmatch` (the glob dialect of `gitignore(5)`, `gitattributes(5)` and
//! pathspecs — `*`, `?`, `[...]` with ranges, `!`/`^` negation and POSIX classes, `\` escapes,
//! and `**` that crosses directories only when it stands between slashes), `.gitignore` lists with
//! git's precedence rules, and `.gitattributes` with macros (`binary` built in).
//!
//! Oracle (tests/ignore.rs): `git ls-files -o -i --exclude-standard` and `git check-attr -a` over a
//! fixture tree with nested lists, negations, anchored/dir-only/`**` patterns.

use alloc::vec::Vec;

/// `WM_PATHNAME`: wildcards do not match `/`.
pub const PATHNAME: u32 = 1;
/// `WM_CASEFOLD`.
pub const CASEFOLD: u32 = 2;

#[derive(PartialEq, Eq, Clone, Copy)]
enum M {
    Match,
    NoMatch,
    AbortAll,
    AbortToStarStar,
}

/// Does `text` match glob `pat` (git's `wildmatch`)?
pub fn wildmatch(pat: &[u8], text: &[u8], flags: u32) -> bool {
    dowild(pat, text, flags) == M::Match
}

fn fold(c: u8, flags: u32) -> u8 {
    if flags & CASEFOLD != 0 { c.to_ascii_lowercase() } else { c }
}

fn dowild(p: &[u8], t: &[u8], flags: u32) -> M {
    let (mut pi, mut ti) = (0usize, 0usize);
    let pathname = flags & PATHNAME != 0;
    while pi < p.len() {
        let mut pc = p[pi];
        let tc = t.get(ti).copied();
        if tc.is_none() && pc != b'*' {
            return M::AbortAll;
        }
        let tc = fold(tc.unwrap_or(0), flags);
        match pc {
            b'\\' => {
                pi += 1;
                pc = match p.get(pi) {
                    Some(&c) => c,
                    None => return M::AbortAll, // a trailing backslash matches nothing
                };
                if fold(pc, flags) != tc {
                    return M::NoMatch;
                }
            }
            b'?' => {
                if pathname && tc == b'/' {
                    return M::NoMatch;
                }
            }
            b'*' => {
                let match_slash;
                pi += 1;
                if p.get(pi) == Some(&b'*') {
                    let prev_ok = pi < 2 || p[pi - 2] == b'/';
                    while p.get(pi) == Some(&b'*') {
                        pi += 1;
                    }
                    if !pathname {
                        match_slash = true;
                    } else if prev_ok && (pi == p.len() || p[pi] == b'/') {
                        // "**/" also matches zero directories.
                        if pi < p.len() && p[pi] == b'/' && dowild(&p[pi + 1..], &t[ti..], flags) == M::Match {
                            return M::Match;
                        }
                        match_slash = true;
                    } else {
                        match_slash = false;
                    }
                } else {
                    match_slash = !pathname;
                }
                if pi == p.len() {
                    if !match_slash && t[ti..].contains(&b'/') {
                        return M::AbortToStarStar;
                    }
                    return M::Match;
                }
                if !match_slash && p[pi] == b'/' {
                    match t[ti..].iter().position(|&c| c == b'/') {
                        Some(s) => {
                            ti += s;
                            // the slash itself is consumed by the main loop below
                            pi += 1;
                            ti += 1;
                            continue;
                        }
                        None => return M::AbortAll,
                    }
                }
                loop {
                    if ti >= t.len() {
                        break;
                    }
                    let r = dowild(&p[pi..], &t[ti..], flags);
                    if r != M::NoMatch {
                        if !match_slash || r != M::AbortToStarStar {
                            return r;
                        }
                    } else if !match_slash && t[ti] == b'/' {
                        return M::AbortToStarStar;
                    }
                    ti += 1;
                }
                return M::AbortAll;
            }
            b'[' => {
                pi += 1;
                let mut pch = match p.get(pi) {
                    Some(&c) => c,
                    None => return M::AbortAll,
                };
                if pch == b'^' {
                    pch = b'!';
                }
                let negated = pch == b'!';
                if negated {
                    pi += 1;
                    pch = match p.get(pi) {
                        Some(&c) => c,
                        None => return M::AbortAll,
                    };
                }
                let mut prev: u8 = 0;
                let mut matched = false;
                loop {
                    if pi >= p.len() {
                        return M::AbortAll;
                    }
                    if pch == b'\\' {
                        pi += 1;
                        pch = match p.get(pi) {
                            Some(&c) => c,
                            None => return M::AbortAll,
                        };
                        if tc == fold(pch, flags) {
                            matched = true;
                        }
                    } else if pch == b'-' && prev != 0 && pi + 1 < p.len() && p[pi + 1] != b']' {
                        pi += 1;
                        pch = p[pi];
                        if pch == b'\\' {
                            pi += 1;
                            pch = match p.get(pi) {
                                Some(&c) => c,
                                None => return M::AbortAll,
                            };
                        }
                        let raw_t = t[ti];
                        if raw_t <= pch && raw_t >= prev {
                            matched = true;
                        } else if flags & CASEFOLD != 0 && raw_t.is_ascii_lowercase() {
                            let u = raw_t.to_ascii_uppercase();
                            if u <= pch && u >= prev {
                                matched = true;
                            }
                        }
                        pch = 0;
                    } else if pch == b'[' && p.get(pi + 1) == Some(&b':') {
                        let s = pi + 2;
                        let mut e = s;
                        while e < p.len() && p[e] != b']' {
                            e += 1;
                        }
                        if e >= p.len() {
                            return M::AbortAll;
                        }
                        if e == s || p[e - 1] != b':' {
                            // no ":]": a literal '['
                            if tc == b'[' {
                                matched = true;
                            }
                        } else {
                            let name = &p[s..e - 1];
                            let c = t[ti];
                            let ok = match name {
                                b"alnum" => c.is_ascii_alphanumeric(),
                                b"alpha" => c.is_ascii_alphabetic(),
                                b"blank" => c == b' ' || c == b'\t',
                                b"cntrl" => c.is_ascii_control(),
                                b"digit" => c.is_ascii_digit(),
                                b"graph" => c.is_ascii_graphic(),
                                b"lower" => c.is_ascii_lowercase() || (flags & CASEFOLD != 0 && c.is_ascii_uppercase()),
                                b"print" => c.is_ascii_graphic() || c == b' ',
                                b"punct" => c.is_ascii_punctuation(),
                                b"space" => matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c),
                                b"upper" => c.is_ascii_uppercase() || (flags & CASEFOLD != 0 && c.is_ascii_lowercase()),
                                b"xdigit" => c.is_ascii_hexdigit(),
                                _ => return M::AbortAll,
                            };
                            if ok {
                                matched = true;
                            }
                            pi = e;
                            pch = 0;
                        }
                    } else if tc == fold(pch, flags) {
                        matched = true;
                    }
                    prev = pch;
                    pi += 1;
                    match p.get(pi) {
                        Some(&b']') => break,
                        Some(&c) => pch = c,
                        None => return M::AbortAll,
                    }
                }
                if matched == negated || (pathname && tc == b'/') {
                    return M::NoMatch;
                }
            }
            _ => {
                if fold(pc, flags) != tc {
                    return M::NoMatch;
                }
            }
        }
        pi += 1;
        ti += 1;
    }
    if ti < t.len() { M::NoMatch } else { M::Match }
}

// ---------------------------------------------------------------------------------------------
// gitignore
// ---------------------------------------------------------------------------------------------

/// One parsed pattern line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// The glob (leading `!`, trailing `/` and a leading `/` removed).
    pub glob: Vec<u8>,
    /// `!pattern`.
    pub negative: bool,
    /// `pattern/`: directories only.
    pub dir_only: bool,
    /// No slash in the pattern: matched against the basename at any depth.
    pub basename: bool,
}

impl Pattern {
    /// Parse one line (`None` for blank lines and comments).
    pub fn parse(line: &[u8]) -> Option<Self> {
        let mut l = line;
        if let [rest @ .., b'\r'] = l {
            l = rest;
        }
        if l.is_empty() || l[0] == b'#' {
            return None;
        }
        // trailing spaces unless backslash-escaped
        let mut end = l.len();
        while end > 0 && l[end - 1] == b' ' {
            if end >= 2 && l[end - 2] == b'\\' {
                break;
            }
            end -= 1;
        }
        l = &l[..end];
        let mut negative = false;
        if l.first() == Some(&b'!') {
            negative = true;
            l = &l[1..];
        }
        if l.is_empty() {
            return None;
        }
        let mut dir_only = false;
        if l.len() > 1 && l.last() == Some(&b'/') {
            dir_only = true;
            l = &l[..l.len() - 1];
        }
        let basename = !l.contains(&b'/');
        let glob = if !basename && l[0] == b'/' { l[1..].to_vec() } else { l.to_vec() };
        Some(Pattern { glob, negative, dir_only, basename })
    }

    /// Does this pattern match `path` (relative to the repository root) given the list's `base`
    /// (`""` or `"dir/sub/"`)?
    pub fn matches(&self, base: &[u8], path: &[u8], is_dir: bool, flags: u32) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        if self.basename {
            let b = path.iter().rposition(|&c| c == b'/').map(|i| &path[i + 1..]).unwrap_or(path);
            return wildmatch(&self.glob, b, flags);
        }
        let Some(rest) = path.strip_prefix(base) else { return false };
        wildmatch(&self.glob, rest, flags | PATHNAME)
    }
}

/// The patterns of one source (a `.gitignore` at `base`, `info/exclude`, `core.excludesFile`).
#[derive(Debug, Clone, Default)]
pub struct PatternList {
    /// Directory of the source relative to the root, with a trailing `/` (or empty).
    pub base: Vec<u8>,
    /// The patterns in file order.
    pub patterns: Vec<Pattern>,
}

impl PatternList {
    /// Parse a whole file.
    pub fn parse(text: &[u8], base: &[u8]) -> Self {
        let text = text.strip_prefix(&[0xef, 0xbb, 0xbf][..]).unwrap_or(text);
        PatternList { base: base.to_vec(), patterns: text.split(|&b| b == b'\n').filter_map(Pattern::parse).collect() }
    }

    /// The last matching pattern's verdict: `Some(true)` excluded, `Some(false)` re-included.
    pub fn verdict(&self, path: &[u8], is_dir: bool, flags: u32) -> Option<bool> {
        self.patterns.iter().rev().find(|p| p.matches(&self.base, path, is_dir, flags)).map(|p| !p.negative)
    }
}

/// An ordered stack of lists, lowest precedence first: core.excludesFile, info/exclude, then the
/// `.gitignore` files from the root down to the path's own directory.
#[derive(Debug, Clone, Default)]
pub struct Ignore {
    /// Lists in increasing precedence.
    pub lists: Vec<PatternList>,
    /// `core.ignoreCase`.
    pub flags: u32,
}

impl Ignore {
    /// Is `path` excluded by its own patterns (parents are the walker's business)?
    pub fn is_excluded(&self, path: &[u8], is_dir: bool) -> bool {
        for l in self.lists.iter().rev() {
            // A .gitignore only applies beneath its own directory.
            if !path.starts_with(&l.base) {
                continue;
            }
            if let Some(v) = l.verdict(path, is_dir, self.flags) {
                return v;
            }
        }
        false
    }

    /// Is `path` excluded, counting an excluded parent directory (git cannot re-include beneath one)?
    pub fn is_excluded_with_parents(&self, path: &[u8], is_dir: bool) -> bool {
        let mut i = 0;
        while let Some(s) = path[i..].iter().position(|&c| c == b'/') {
            if self.is_excluded(&path[..i + s], true) {
                return true;
            }
            i += s + 1;
        }
        self.is_excluded(path, is_dir)
    }
}

// ---------------------------------------------------------------------------------------------
// gitattributes
// ---------------------------------------------------------------------------------------------

/// The state of one attribute for a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrValue {
    /// `attr`.
    Set,
    /// `-attr`.
    Unset,
    /// `attr=value`.
    Value(Vec<u8>),
    /// `!attr` (explicitly unspecified).
    Unspecified,
}

/// One `.gitattributes` line.
#[derive(Debug, Clone)]
pub struct AttrRule {
    pattern: Pattern,
    attrs: Vec<(Vec<u8>, AttrValue)>,
}

/// One attributes file.
#[derive(Debug, Clone, Default)]
pub struct AttrFile {
    /// Directory relative to the root with trailing `/`, or empty.
    pub base: Vec<u8>,
    rules: Vec<AttrRule>,
    macros: Vec<(Vec<u8>, Vec<(Vec<u8>, AttrValue)>)>,
}

fn parse_attr_states(words: &[&[u8]]) -> Vec<(Vec<u8>, AttrValue)> {
    words
        .iter()
        .filter(|w| !w.is_empty())
        .map(|w| {
            if let Some(n) = w.strip_prefix(b"-") {
                (n.to_vec(), AttrValue::Unset)
            } else if let Some(n) = w.strip_prefix(b"!") {
                (n.to_vec(), AttrValue::Unspecified)
            } else if let Some(eq) = w.iter().position(|&c| c == b'=') {
                (w[..eq].to_vec(), AttrValue::Value(w[eq + 1..].to_vec()))
            } else {
                (w.to_vec(), AttrValue::Set)
            }
        })
        .collect()
}

impl AttrFile {
    /// Parse a file (`[attr]` macro lines are honoured only when `allow_macros`, i.e. the root file
    /// or `info/attributes`).
    pub fn parse(text: &[u8], base: &[u8], allow_macros: bool) -> Self {
        let mut f = AttrFile { base: base.to_vec(), ..Default::default() };
        for line in text.split(|&b| b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let words: Vec<&[u8]> = line.split(|&b| b == b' ' || b == b'\t').filter(|w| !w.is_empty()).collect();
            let Some(first) = words.first() else { continue };
            if first[0] == b'#' {
                continue;
            }
            if let Some(m) = first.strip_prefix(b"[attr]") {
                if allow_macros {
                    f.macros.push((m.to_vec(), parse_attr_states(&words[1..])));
                }
                continue;
            }
            let Some(p) = Pattern::parse(first) else { continue };
            if p.negative {
                continue; // negative patterns are forbidden in attributes files
            }
            f.rules.push(AttrRule { pattern: p, attrs: parse_attr_states(&words[1..]) });
        }
        f
    }
}

/// A stack of attribute files, lowest precedence first (root `.gitattributes` … deepest, then
/// `info/attributes`).
#[derive(Debug, Clone, Default)]
pub struct Attributes {
    /// Files in increasing precedence.
    pub files: Vec<AttrFile>,
}

impl Attributes {
    /// Every specified attribute of `path` (a file), with macros expanded; sorted by name.
    pub fn check_all(&self, path: &[u8]) -> Vec<(Vec<u8>, AttrValue)> {
        let mut macros: Vec<(Vec<u8>, Vec<(Vec<u8>, AttrValue)>)> =
            alloc::vec![(b"binary".to_vec(), parse_attr_states(&[b"-diff", b"-merge", b"-text"]))];
        for f in &self.files {
            for m in &f.macros {
                macros.push(m.clone());
            }
        }
        let mut decided: Vec<(Vec<u8>, AttrValue)> = Vec::new();
        fn apply(
            decided: &mut Vec<(Vec<u8>, AttrValue)>,
            macros: &[(Vec<u8>, Vec<(Vec<u8>, AttrValue)>)],
            name: &[u8],
            v: &AttrValue,
            depth: usize,
        ) {
            if decided.iter().any(|(n, _)| n == name) {
                return;
            }
            decided.push((name.to_vec(), v.clone()));
            if *v == AttrValue::Set && depth < 8 {
                if let Some((_, exp)) = macros.iter().rev().find(|(n, _)| n == name) {
                    for (n2, v2) in exp.iter().rev() {
                        apply(decided, macros, n2, v2, depth + 1);
                    }
                }
            }
        }
        for f in self.files.iter().rev() {
            if !path.starts_with(&f.base) {
                continue;
            }
            for r in f.rules.iter().rev() {
                if r.pattern.matches(&f.base, path, false, 0) {
                    for (n, v) in r.attrs.iter().rev() {
                        apply(&mut decided, &macros, n, v, 0);
                    }
                }
            }
        }
        decided.retain(|(_, v)| *v != AttrValue::Unspecified);
        decided.sort_by(|a, b| a.0.cmp(&b.0));
        decided
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wildmatch_basics() {
        let cases: &[(&[u8], &[u8], bool)] = &[
            (b"foo", b"foo", true),
            (b"*", b"foo/bar", false),
            (b"**", b"foo/bar", true),
            (b"**/foo", b"foo", true),
            (b"**/foo", b"a/b/foo", true),
            (b"a/**/b", b"a/b", true),
            (b"a/**/b", b"a/x/y/b", true),
            (b"a/*/b", b"a/x/y/b", false),
            (b"[a-c]x", b"bx", true),
            (b"[!a-c]x", b"bx", false),
            (b"[]]", b"]", true),
            (b"[[:digit:]]*", b"7up", true),
            (b"\\*", b"*", true),
            (b"foo**bar", b"foo/x/bar", false),
            (b"*.c", b"x.c", true),
        ];
        for (p, t, want) in cases {
            assert_eq!(wildmatch(p, t, PATHNAME), *want, "{} vs {}", core::str::from_utf8(p).unwrap(), core::str::from_utf8(t).unwrap());
        }
    }
}
