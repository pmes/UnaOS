// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The unified-diff parser. Bounded ([`MAX_FILES`] files, [`MAX_HUNKS`] hunks), no allocator: every hunk
//! borrows its body from the answer text. Accepted: `--- a/<p>` / `+++ b/<p>` file headers (a trailing
//! TAB + timestamp is cut; `a/` `b/` are stripped), `@@ -a[,b] +c[,d] @@` hunk headers, body lines that
//! begin with ` `, `-`, `+`, `\` (`\ No newline at end of file`, ignored) — and an EMPTY body line, read as
//! an empty context line (a model's answer often loses a context line's single space). Refused: file
//! creation or deletion (`/dev/null`), a path that is absolute or climbs (`..`), a hunk whose body does
//! not add up to its header's counts.

pub const MAX_FILES: usize = 8;
pub const MAX_HUNKS: usize = 32;
pub const PATH_MAX: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refuse {
    /// The answer carries no unified diff (or says NO-PATCH).
    NoDiff,
    /// A header or a hunk body does not parse or does not add up.
    Malformed,
    /// More files or hunks than the bounds.
    TooMany,
    /// An absolute, climbing, empty or over-long path.
    BadPath,
    /// File creation or deletion.
    Unsupported,
    /// A hunk's old side matches nowhere within ±FUZZ lines of its stated line (file, hunk index).
    Mismatch(u8, u8),
    /// Two hunks of one file resolve onto overlapping lines, or are out of order.
    Overlap,
    /// A hunk's window did not fit the scratch buffer.
    Window,
    /// The source or sink failed (errno).
    Io(i64),
}

impl Refuse {
    pub fn as_str(&self) -> &'static str {
        match self {
            Refuse::NoDiff => "no-diff",
            Refuse::Malformed => "malformed",
            Refuse::TooMany => "too-many",
            Refuse::BadPath => "bad-path",
            Refuse::Unsupported => "unsupported",
            Refuse::Mismatch(..) => "mismatch",
            Refuse::Overlap => "overlap",
            Refuse::Window => "window",
            Refuse::Io(_) => "io",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk<'a> {
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    /// The body lines, joined by `\n` as they lay in the answer.
    pub body: &'a str,
}

impl<'a> Hunk<'a> {
    const EMPTY: Hunk<'static> = Hunk { old_start: 0, old_len: 0, new_start: 0, new_len: 0, body: "" };

    /// `(kind, text)` per body line: kind is `b' '`, `b'-'` or `b'+'`.
    pub fn lines(&self) -> impl Iterator<Item = (u8, &'a str)> {
        let body = self.body;
        let mut it = if body.is_empty() { None } else { Some(crate::lines(body)) };
        core::iter::from_fn(move || loop {
            let l = it.as_mut()?.next()?;
            match l.as_bytes().first() {
                None => return Some((b' ', "")),
                Some(b'\\') => continue,
                Some(&k @ (b' ' | b'-' | b'+')) => return Some((k, &l[1..])),
                Some(_) => return None,
            }
        })
    }
    pub fn old_lines(&self) -> impl Iterator<Item = &'a str> {
        self.lines().filter(|(k, _)| *k != b'+').map(|(_, t)| t)
    }
    pub fn new_lines(&self) -> impl Iterator<Item = &'a str> {
        self.lines().filter(|(k, _)| *k != b'-').map(|(_, t)| t)
    }
    /// The first original line the old side covers (1-based); for a pure insertion the line it precedes.
    pub fn target(&self) -> u32 {
        if self.old_len == 0 { self.old_start + 1 } else { self.old_start.max(1) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilePatch<'a> {
    /// Repo-relative (the `b/` side).
    pub path: &'a str,
    pub first: usize,
    pub count: usize,
}

pub struct Patch<'a> {
    pub files: [FilePatch<'a>; MAX_FILES],
    pub nfiles: usize,
    pub hunks: [Hunk<'a>; MAX_HUNKS],
    pub nhunks: usize,
}

impl<'a> Patch<'a> {
    pub fn files(&self) -> &[FilePatch<'a>] {
        &self.files[..self.nfiles]
    }
    pub fn hunks_of(&self, f: &FilePatch<'a>) -> &[Hunk<'a>] {
        &self.hunks[f.first..f.first + f.count]
    }
    pub fn total_hunks(&self) -> usize {
        self.nhunks
    }
}

/// The diff inside a model's answer: the first ```` ```diff ```` (or ```` ```patch ````) fence's content, else
/// from the first line that begins `--- ` to the end (or to a closing fence). `None` = no diff.
pub fn extract(answer: &str) -> Option<&str> {
    for fence in ["```diff", "```patch", "```udiff"] {
        if let Some(i) = answer.find(fence) {
            let rest = &answer[i + fence.len()..];
            let rest = match rest.find('\n') {
                Some(j) => &rest[j + 1..],
                None => return None,
            };
            let end = rest.find("\n```").map(|j| j + 1).unwrap_or(rest.len());
            let d = &rest[..end];
            return if d.contains("\n@@") || d.starts_with("@@") { Some(d) } else { None };
        }
    }
    let start = if answer.starts_with("--- ") { 0 } else { answer.find("\n--- ")? + 1 };
    let rest = &answer[start..];
    let end = rest.find("\n```").map(|j| j + 1).unwrap_or(rest.len());
    Some(&rest[..end])
}

fn header_path<'a>(l: &'a str, prefix: &str) -> Option<&'a str> {
    let p = l.strip_prefix(prefix)?;
    let p = p.split('\t').next().unwrap_or(p).trim_end();
    Some(p)
}

fn strip_side<'a>(p: &'a str, side: &str) -> &'a str {
    p.strip_prefix(side).unwrap_or(p)
}

fn path_ok(p: &str) -> bool {
    !p.is_empty() && p.len() <= PATH_MAX && !p.starts_with('/') && !p.split('/').any(|c| c == ".." || c.is_empty()) && !p.contains('\\') && p.bytes().all(|c| c > b' ' && c < 0x7f)
}

/// `-a[,b]` or `+c[,d]` → `(start, len)`; a missing len is 1.
fn range(s: &str) -> Option<(u32, u32)> {
    let mut it = s.splitn(2, ',');
    let a = crate::parse_dec(it.next()?.as_bytes())? as u32;
    let b = match it.next() {
        Some(x) => crate::parse_dec(x.as_bytes())? as u32,
        None => 1,
    };
    Some((a, b))
}

fn hunk_header(l: &str) -> Option<(u32, u32, u32, u32)> {
    let r = l.strip_prefix("@@ -")?;
    let end = r.find(" @@")?;
    let mut parts = r[..end].split(" +");
    let (a, b) = range(parts.next()?)?;
    let (c, d) = range(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some((a, b, c, d))
}

/// Parse the diff text ([`extract`]'s output).
pub fn parse(text: &str) -> Result<Patch<'_>, Refuse> {
    let mut p = Patch { files: [FilePatch { path: "", first: 0, count: 0 }; MAX_FILES], nfiles: 0, hunks: [Hunk::EMPTY; MAX_HUNKS], nhunks: 0 };
    // Walk by byte offsets so a hunk body can borrow a contiguous slice of `text`.
    let bytes = text.as_bytes();
    let mut pos = 0usize;
    let next_line = |pos: &mut usize| -> Option<(usize, usize)> {
        if *pos >= bytes.len() {
            return None;
        }
        let s = *pos;
        let e = text[s..].find('\n').map(|j| s + j).unwrap_or(bytes.len());
        *pos = if e < bytes.len() { e + 1 } else { e };
        Some((s, e))
    };
    let line_at = |s: usize, e: usize| -> &str {
        let l = &text[s..e];
        l.strip_suffix('\r').unwrap_or(l)
    };
    let mut pending_old: Option<&str> = None;
    while let Some((s, e)) = next_line(&mut pos) {
        let l = line_at(s, e);
        if let Some(op) = header_path(l, "--- ") {
            pending_old = Some(op);
            continue;
        }
        if let Some(np) = header_path(l, "+++ ") {
            let Some(op) = pending_old.take() else { return Err(Refuse::Malformed) };
            if op == "/dev/null" || np == "/dev/null" {
                return Err(Refuse::Unsupported);
            }
            let path = strip_side(np, "b/");
            if !path_ok(path) {
                return Err(Refuse::BadPath);
            }
            if p.nfiles == MAX_FILES {
                return Err(Refuse::TooMany);
            }
            p.files[p.nfiles] = FilePatch { path, first: p.nhunks, count: 0 };
            p.nfiles += 1;
            continue;
        }
        if l.starts_with("@@ ") {
            if p.nfiles == 0 {
                return Err(Refuse::Malformed);
            }
            let (a, b, c, d) = hunk_header(l).ok_or(Refuse::Malformed)?;
            // The body: lines until the old and new counts are both met.
            let (mut so, mut sn) = (0u32, 0u32);
            let body_start = pos;
            let mut body_end = pos;
            while so < b || sn < d {
                let save = pos;
                let Some((bs, be)) = next_line(&mut pos) else { break };
                let bl = line_at(bs, be);
                match bl.as_bytes().first() {
                    None | Some(b' ') => {
                        so += 1;
                        sn += 1;
                    }
                    Some(b'-') if !bl.starts_with("--- ") || so < b => so += 1,
                    Some(b'+') if !bl.starts_with("+++ ") || sn < d => sn += 1,
                    Some(b'\\') => {}
                    _ => {
                        pos = save;
                        break;
                    }
                }
                body_end = be;
            }
            if so != b || sn != d {
                return Err(Refuse::Malformed);
            }
            // A trailing `\ No newline at end of file` belongs to the hunk too.
            let save = pos;
            if let Some((bs, be)) = next_line(&mut pos) {
                if text[bs..be].starts_with('\\') {
                    body_end = be;
                } else {
                    pos = save;
                }
            }
            if p.nhunks == MAX_HUNKS {
                return Err(Refuse::TooMany);
            }
            let body = if body_end > body_start { &text[body_start..body_end] } else { "" };
            p.hunks[p.nhunks] = Hunk { old_start: a, old_len: b, new_start: c, new_len: d, body };
            p.nhunks += 1;
            p.files[p.nfiles - 1].count += 1;
            continue;
        }
        // Anything else between files (`diff --git`, `index`, prose) is skipped.
    }
    if p.nhunks == 0 || p.files().iter().any(|f| f.count == 0) {
        return Err(Refuse::NoDiff);
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "Here is the fix:\n\n```diff\n--- a/unaos/x.rs\n+++ b/unaos/x.rs\n@@ -2,3 +2,3 @@\n a\n-b\n+B\n c\n@@ -10,2 +10,3 @@ fn y\n p\n+q\n r\n```\nThat should do it.";

    #[test]
    fn fenced() {
        let d = extract(D).unwrap();
        let p = parse(d).unwrap();
        assert_eq!(p.nfiles, 1);
        assert_eq!(p.files[0].path, "unaos/x.rs");
        let h = p.hunks_of(&p.files[0]);
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].old_start, h[0].old_len, h[0].new_len), (2, 3, 3));
        let old: [&str; 3] = { let mut it = h[0].old_lines(); [it.next().unwrap(), it.next().unwrap(), it.next().unwrap()] };
        assert_eq!(old, ["a", "b", "c"]);
        assert_eq!(h[1].new_lines().count(), 3);
    }

    #[test]
    fn bare_and_refusals() {
        assert!(extract("no patch needed. NO-PATCH").is_none());
        let bare = "--- a/p.rs\n+++ b/p.rs\n@@ -1 +1 @@\n-x\n+y\n";
        let p = parse(extract(bare).unwrap()).unwrap();
        assert_eq!(p.hunks[0].old_len, 1);
        assert_eq!(parse("--- a/../p\n+++ b/../p\n@@ -1 +1 @@\n-x\n+y\n").err(), Some(Refuse::BadPath));
        assert_eq!(parse("--- /dev/null\n+++ b/p\n@@ -0,0 +1 @@\n+y\n").err(), Some(Refuse::Unsupported));
        assert_eq!(parse("--- a/p\n+++ b/p\n@@ -1,3 +1,3 @@\n a\n-b\n").err(), Some(Refuse::Malformed));
        // an empty context line counts as " "
        let p = parse("--- a/p\n+++ b/p\n@@ -1,3 +1,3 @@\n a\n\n-c\n+C\n").unwrap();
        assert_eq!(p.hunks[0].old_lines().nth(1), Some(""));
    }
}
