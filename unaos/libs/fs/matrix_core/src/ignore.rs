// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The ignore rules — what the tree, the Finder and the indexer never show or walk.
//!
//! Three layers, checked in order on every entry (`path` in the volume's namespace, `name` its last
//! component):
//!
//! 1. **Build noise** — `target`, `.git`, `node_modules` by NAME, at any depth, file or directory. This is the
//!    set the host handler has always excluded (genesis, the Finder's `EXCLUDED`, the crate indexer), so
//!    [`Ignore::build_noise`] reproduces today's host topology byte for byte.
//! 2. **Hidden files** — a name starting with `.` (opt-in, [`Ignore::hidden`]).
//! 3. **A `.gitignore` subset** ([`Ignore::gitignore`]): blank lines and `#` comments skipped; `\` escapes the
//!    next character; `!` negates (the last matching rule wins); a trailing `/` matches directories only; a
//!    pattern with a `/` anywhere but the end is anchored at the ignore file's base, otherwise it matches the
//!    NAME at any depth; `*` matches a run within one component, `?` one character, `**` any run across
//!    components (`**/x` = `x` at any depth, `x/**` = everything under `x`). Character classes (`[abc]`) are
//!    not in the subset: a `[` is literal. A directory the rules exclude is never descended, so a child of an
//!    excluded directory cannot be re-included (the gitignore rule, by construction).

use alloc::string::String;
use alloc::vec::Vec;

/// The names the host has always excluded at every depth.
pub const BUILD_NOISE: &[&str] = &["target", ".git", "node_modules"];

#[derive(Clone, Debug)]
struct Rule {
    /// The glob, with the leading `/`, the trailing `/` and the `!` removed.
    glob: String,
    dir_only: bool,
    anchored: bool,
    negate: bool,
}

/// An ignore policy. `Ignore::build_noise()` is the host's present policy; layers are added by builder calls.
#[derive(Clone, Debug)]
pub struct Ignore {
    noise: bool,
    hidden: bool,
    /// The path the `.gitignore` rules are relative to (in the volume's namespace).
    base: String,
    rules: Vec<Rule>,
}

impl Ignore {
    /// Nothing ignored at all.
    pub fn none() -> Ignore {
        Ignore { noise: false, hidden: false, base: String::new(), rules: Vec::new() }
    }

    /// The build-noise set only — the host's genesis / Finder policy since MATRIX-REOPEN M1.
    pub fn build_noise() -> Ignore {
        Ignore { noise: true, ..Ignore::none() }
    }

    /// Also hide every name that starts with `.`.
    pub fn hidden(mut self, on: bool) -> Ignore {
        self.hidden = on;
        self
    }

    /// Add the rules of a `.gitignore` found at directory `base`.
    pub fn gitignore(mut self, base: &str, text: &str) -> Ignore {
        self.base = String::from(base.trim_end_matches('/'));
        for raw in text.lines() {
            let line = trim_unescaped_trailing_spaces(raw.trim_end_matches('\r'));
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (negate, line) = match line.strip_prefix('!') {
                Some(r) => (true, r),
                None => (false, line),
            };
            // `\!` and `\#` are a literal `!` / `#` (the backslash is dropped, the character kept).
            let line = if line.starts_with("\\!") || line.starts_with("\\#") { &line[1..] } else { line };
            let (dir_only, line) = match line.strip_suffix('/') {
                Some(r) => (true, r),
                None => (false, line),
            };
            let (lead_slash, line) = match line.strip_prefix('/') {
                Some(r) => (true, r),
                None => (false, line),
            };
            if line.is_empty() {
                continue;
            }
            // `**/x` = `x` at any depth (unanchored), unless more `/` follow.
            let (glob, anchored) = match line.strip_prefix("**/") {
                Some(r) if !r.contains('/') && !lead_slash => (String::from(r), false),
                _ => (String::from(line), lead_slash || line.contains('/')),
            };
            self.rules.push(Rule { glob, dir_only, anchored, negate });
        }
        self
    }

    /// Is the entry at `path` (named `name`, a directory when `is_dir`) ignored?
    pub fn ignored(&self, path: &str, name: &str, is_dir: bool) -> bool {
        if self.noise && BUILD_NOISE.contains(&name) {
            return true;
        }
        if self.hidden && name.starts_with('.') && name != "." && name != ".." {
            return true;
        }
        if self.rules.is_empty() {
            return false;
        }
        let rel = self.relative(path);
        let mut out = false;
        for r in &self.rules {
            if r.dir_only && !is_dir {
                continue;
            }
            let hit = if r.anchored { glob(r.glob.as_bytes(), rel.as_bytes()) } else { glob(r.glob.as_bytes(), name.as_bytes()) };
            if hit {
                out = !r.negate;
            }
        }
        out
    }

    /// `path` relative to the rules' base (the path itself when it is not under the base).
    fn relative<'a>(&self, path: &'a str) -> &'a str {
        let p = path.trim_matches('/');
        let b = self.base.trim_matches('/');
        if b.is_empty() {
            return p;
        }
        match p.strip_prefix(b) {
            Some(r) if r.is_empty() => r,
            Some(r) if r.starts_with('/') => &r[1..],
            _ => p,
        }
    }
}

/// gitignore: trailing spaces are dropped unless escaped with `\`.
fn trim_unescaped_trailing_spaces(s: &str) -> &str {
    let mut end = s.len();
    let b = s.as_bytes();
    while end > 0 && b[end - 1] == b' ' {
        if end >= 2 && b[end - 2] == b'\\' {
            break;
        }
        end -= 1;
    }
    &s[..end]
}

/// The subset's glob: `*` (no `/`), `**` (anything), `?` (one byte, not `/`), `\x` (literal `x`).
pub fn glob(pat: &[u8], text: &[u8]) -> bool {
    match pat.first() {
        None => text.is_empty(),
        Some(b'*') => {
            if pat.get(1) == Some(&b'*') {
                // `**`: a following `/` is optional, so `x/**/y` matches `x/y` too.
                let mut rest = &pat[2..];
                if rest.first() == Some(&b'/') {
                    if glob(&rest[1..], text) {
                        return true;
                    }
                    rest = &rest[1..];
                }
                (0..=text.len()).any(|i| glob(rest, &text[i..]))
            } else {
                let rest = &pat[1..];
                let mut i = 0;
                loop {
                    if glob(rest, &text[i..]) {
                        return true;
                    }
                    if i == text.len() || text[i] == b'/' {
                        return false;
                    }
                    i += 1;
                }
            }
        }
        Some(b'?') => !text.is_empty() && text[0] != b'/' && glob(&pat[1..], &text[1..]),
        Some(b'\\') if pat.len() >= 2 => !text.is_empty() && text[0] == pat[1] && glob(&pat[2..], &text[1..]),
        Some(&c) => !text.is_empty() && text[0] == c && glob(&pat[1..], &text[1..]),
    }
}
