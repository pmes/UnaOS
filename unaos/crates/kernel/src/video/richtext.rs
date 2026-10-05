// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Tabula — owed
//!
//! QUARRY2 (rmbp-ledger B336) — the text viewer's two RENDERERS: Markdown and JSON. Text is Tabula's
//! domain (CODEX §2); Tabula's portable document is `std`-only and not on the bus, so the renderer the
//! kernel viewer needs is written here as PURE functions — `&[u8]` in, [`Rendered`] (display bytes plus
//! styled byte ranges) out, `alloc` only, no kernel import — so it moves into a `no_std` Tabula core
//! unchanged the day one exists (the seam is owed, said in the CHARTER line).
//!
//! * [`markdown`] — NOT a CommonMark engine; the small subset a README uses: ATX headings (`#`..`######`,
//!   drawn BOLD without their marks), bullet and ordered lists (indented two columns per level, the
//!   marker kept), block quotes, fenced code and YAML front matter (tinted), a thematic break, and
//!   inline `**bold**` / `` `code` ``. Anything else is the line as written.
//! * [`json`] — a validating pretty-printer: 2-space indent, keys / strings / numbers / literals /
//!   punctuation tinted. A document that does not parse is shown AS WRITTEN with [`Rendered::note`]
//!   naming why (never a half-printed tree).
//!
//! Input is the viewer's sanitised ASCII (tabs already spaces); spans are byte ranges into
//! [`Rendered::text`], in order and non-overlapping. The viewer maps a [`Tint`] to a colour.

use alloc::vec::Vec;

/// What a styled range IS; the viewer picks the colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    Plain,
    Heading,
    Dim,
    Code,
    Quote,
    Key,
    Str,
    Num,
    Lit,
    Punct,
}

/// One styled range of [`Rendered::text`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub tint: Tint,
    pub bold: bool,
}

/// A renderer's output.
#[derive(Clone, Debug, Default)]
pub struct Rendered {
    pub text: Vec<u8>,
    pub spans: Vec<Span>,
    /// Why the document was shown raw (JSON that does not parse), else `None`.
    pub note: Option<&'static str>,
}

impl Rendered {
    fn push(&mut self, s: &[u8], tint: Tint, bold: bool) {
        let a = self.text.len() as u32;
        self.text.extend_from_slice(s);
        let b = self.text.len() as u32;
        if b > a && (tint != Tint::Plain || bold) {
            self.spans.push(Span { start: a, end: b, tint, bold });
        }
    }
    fn plain(&mut self, s: &[u8]) {
        self.text.extend_from_slice(s);
    }
    fn spaces(&mut self, n: usize) {
        for _ in 0..n {
            self.text.push(b' ');
        }
    }
}

/// Render `kind` (`"markdown"` or `"json"`; anything else is plain).
pub fn render(kind: &str, src: &[u8]) -> Rendered {
    match kind {
        "markdown" => markdown(src),
        "json" => json(src),
        _ => Rendered { text: Vec::from(src), spans: Vec::new(), note: None },
    }
}

// ── Markdown ────────────────────────────────────────────────────────────────────────────────────

/// `**bold**` and `` `code` `` inside one line, the marks dropped; the rest in `tint`/`bold`.
fn inline(o: &mut Rendered, s: &[u8], tint: Tint, bold: bool) {
    let mut i = 0usize;
    let mut run = 0usize;
    while i < s.len() {
        let close = |mark: &[u8], from: usize| s[from..].windows(mark.len()).position(|w| w == mark).map(|p| from + p);
        if s[i..].starts_with(b"**") {
            if let Some(j) = close(b"**", i + 2) {
                o.push(&s[run..i], tint, bold);
                o.push(&s[i + 2..j], tint, true);
                i = j + 2;
                run = i;
                continue;
            }
        } else if s[i] == b'`' {
            if let Some(j) = close(b"`", i + 1) {
                o.push(&s[run..i], tint, bold);
                o.push(&s[i + 1..j], Tint::Code, bold);
                i = j + 1;
                run = i;
                continue;
            }
        }
        i += 1;
    }
    o.push(&s[run..], tint, bold);
}

/// The Markdown subset (module header). Pure.
pub fn markdown(src: &[u8]) -> Rendered {
    let mut o = Rendered::default();
    let body = src.strip_suffix(b"\n").unwrap_or(src);
    let (mut front, mut fence) = (false, false);
    for (n, raw) in body.split(|&c| c == b'\n').enumerate() {
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        let ind = line.iter().take_while(|&&c| c == b' ').count();
        let rest = &line[ind..];
        if n == 0 && line == b"---" {
            front = true;
            o.push(line, Tint::Dim, false);
        } else if front {
            o.push(line, Tint::Dim, false);
            front = line != b"---";
        } else if rest.starts_with(b"```") || rest.starts_with(b"~~~") {
            fence = !fence;
            o.push(line, Tint::Dim, false);
        } else if fence {
            o.push(line, Tint::Code, false);
        } else if let Some(h) = Some(rest.iter().take_while(|&&c| c == b'#').count()).filter(|h| (1..=6).contains(h) && rest.get(*h) == Some(&b' ')) {
            let t = &rest[h + 1..];
            let t = &t[..t.len() - t.iter().rev().take_while(|&&c| c == b'#' || c == b' ').count()];
            inline(&mut o, t, Tint::Heading, true);
        } else if rest.len() >= 3 && rest.iter().all(|&c| c == rest[0] || c == b' ') && matches!(rest[0], b'-' | b'*' | b'_') && rest.iter().filter(|&&c| c != b' ').count() >= 3 {
            o.push(&[b'-'; 40], Tint::Dim, false);
        } else if rest.len() >= 2 && matches!(rest[0], b'-' | b'*' | b'+') && rest[1] == b' ' {
            o.spaces((ind / 2 + 1) * 2);
            o.push(b"- ", Tint::Punct, false);
            inline(&mut o, &rest[2..], Tint::Plain, false);
        } else if let Some(d) = Some(rest.iter().take_while(|c| c.is_ascii_digit()).count()).filter(|&d| d > 0 && d < 10 && matches!(rest.get(d), Some(b'.') | Some(b')')) && rest.get(d + 1) == Some(&b' ')) {
            o.spaces((ind / 2 + 1) * 2);
            o.push(&rest[..d + 2], Tint::Punct, false);
            inline(&mut o, &rest[d + 2..], Tint::Plain, false);
        } else if rest.first() == Some(&b'>') {
            o.push(b"| ", Tint::Dim, false);
            let q = &rest[1..];
            inline(&mut o, q.strip_prefix(b" ").unwrap_or(q), Tint::Quote, false);
        } else {
            inline(&mut o, line, Tint::Plain, false);
        }
        o.plain(b"\n");
    }
    o
}

// ── JSON ────────────────────────────────────────────────────────────────────────────────────────

const MAX_DEPTH: usize = 64;

struct P<'a> {
    s: &'a [u8],
    i: usize,
    o: Rendered,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\r' | b'\n') {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn string(&mut self, tint: Tint) -> Result<(), &'static str> {
        let a = self.i;
        self.i += 1;
        loop {
            match self.peek() {
                None => return Err("unterminated string"),
                Some(b'"') => break,
                Some(b'\\') => self.i += 2,
                Some(c) if c < 0x20 => return Err("control byte in a string"),
                Some(_) => self.i += 1,
            }
        }
        self.i += 1;
        if self.i > self.s.len() {
            return Err("unterminated string");
        }
        let s = self.s;
        self.o.push(&s[a..self.i], tint, false);
        Ok(())
    }
    fn number(&mut self) -> Result<(), &'static str> {
        let a = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')) {
            self.i += 1;
        }
        let s = self.s;
        if !s[a..self.i].iter().any(|c| c.is_ascii_digit()) {
            return Err("bad number");
        }
        self.o.push(&s[a..self.i], Tint::Num, false);
        Ok(())
    }
    fn value(&mut self, ind: usize, depth: usize) -> Result<(), &'static str> {
        if depth > MAX_DEPTH {
            return Err("nested too deep");
        }
        self.ws();
        match self.peek() {
            Some(open @ (b'{' | b'[')) => {
                let close = if open == b'{' { b'}' } else { b']' };
                self.i += 1;
                self.o.push(&[open], Tint::Punct, false);
                self.ws();
                if self.peek() == Some(close) {
                    self.i += 1;
                    self.o.push(&[close], Tint::Punct, false);
                    return Ok(());
                }
                self.o.plain(b"\n");
                loop {
                    self.o.spaces((ind + 1) * 2);
                    if open == b'{' {
                        self.ws();
                        if self.peek() != Some(b'"') {
                            return Err("expected a key");
                        }
                        self.string(Tint::Key)?;
                        self.ws();
                        if self.peek() != Some(b':') {
                            return Err("expected ':'");
                        }
                        self.i += 1;
                        self.o.push(b":", Tint::Punct, false);
                        self.o.plain(b" ");
                    }
                    self.value(ind + 1, depth + 1)?;
                    self.ws();
                    match self.peek() {
                        Some(b',') => {
                            self.i += 1;
                            self.o.push(b",", Tint::Punct, false);
                            self.o.plain(b"\n");
                        }
                        Some(c) if c == close => {
                            self.i += 1;
                            self.o.plain(b"\n");
                            self.o.spaces(ind * 2);
                            self.o.push(&[close], Tint::Punct, false);
                            return Ok(());
                        }
                        _ => return Err("expected ',' or a close"),
                    }
                }
            }
            Some(b'"') => self.string(Tint::Str),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b't' | b'f' | b'n') => {
                for lit in [&b"true"[..], b"false", b"null"] {
                    if self.s[self.i..].starts_with(lit) {
                        self.i += lit.len();
                        self.o.push(lit, Tint::Lit, false);
                        return Ok(());
                    }
                }
                Err("bad literal")
            }
            None => Err("unexpected end"),
            Some(_) => Err("unexpected byte"),
        }
    }
}

/// Pretty-print `src` (module header). Pure.
pub fn json(src: &[u8]) -> Rendered {
    let mut p = P { s: src, i: 0, o: Rendered::default() };
    let r = p.value(0, 0).and_then(|_| {
        p.ws();
        if p.i == src.len() { Ok(()) } else { Err("bytes after the document") }
    });
    match r {
        Ok(()) => {
            p.o.plain(b"\n");
            p.o
        }
        Err(why) => Rendered { text: Vec::from(src), spans: Vec::new(), note: Some(why) },
    }
}
