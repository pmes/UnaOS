// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The PDF lexer (ISO 32000-1 §7.2 lexical conventions, §7.3 the token forms of the basic objects).
//!
//! A [`Lexer`] is a cursor over a byte slice; it is `Clone`, so a parser looks ahead by cloning it. Malformed input
//! never panics: an unterminated string ends at the end of the buffer, a stray delimiter is a one-byte keyword.

use alloc::vec::Vec;

/// One token.
#[derive(Debug, Clone, PartialEq)]
pub enum Token<'a> {
    Int(i64),
    Real(f64),
    /// A literal or hex string, its escapes already undone.
    Str(Vec<u8>),
    /// A name without its `/`, `#xx` already undone.
    Name(Vec<u8>),
    ArrayOpen,
    ArrayClose,
    DictOpen,
    DictClose,
    /// Any other run of regular characters (`obj`, `R`, `true`, an operator), or a stray delimiter byte.
    Keyword(&'a [u8]),
}

/// §7.2.2 Table 1: the white-space characters.
pub fn is_white(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

/// §7.2.2 Table 2: the delimiter characters.
pub fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The first position at or after `from` where `needle` occurs in `hay`.
pub fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() || hay.len() - from < needle.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// The last position where `needle` occurs in `hay`.
pub fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// A cursor over PDF bytes.
#[derive(Debug, Clone)]
pub struct Lexer<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Lexer { buf, pos: 0 }
    }

    pub fn at(buf: &'a [u8], pos: usize) -> Self {
        Lexer { buf, pos }
    }

    /// Skip white space and `%` comments (a comment runs to the end of its line).
    pub fn skip_ws(&mut self) {
        while let Some(&b) = self.buf.get(self.pos) {
            if is_white(b) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(&c) = self.buf.get(self.pos) {
                    if c == b'\r' || c == b'\n' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    /// The next token without consuming it.
    pub fn peek(&self) -> Option<Token<'a>> {
        self.clone().next()
    }

    /// The next token, or `None` at the end of the buffer.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Token<'a>> {
        self.skip_ws();
        let b = *self.buf.get(self.pos)?;
        match b {
            b'/' => {
                self.pos += 1;
                Some(Token::Name(self.name()))
            }
            b'(' => {
                self.pos += 1;
                Some(Token::Str(self.literal()))
            }
            b'<' => {
                if self.buf.get(self.pos + 1) == Some(&b'<') {
                    self.pos += 2;
                    Some(Token::DictOpen)
                } else {
                    self.pos += 1;
                    Some(Token::Str(self.hex()))
                }
            }
            b'>' => {
                if self.buf.get(self.pos + 1) == Some(&b'>') {
                    self.pos += 2;
                    Some(Token::DictClose)
                } else {
                    self.pos += 1;
                    Some(Token::Keyword(&self.buf[self.pos - 1..self.pos]))
                }
            }
            b'[' => {
                self.pos += 1;
                Some(Token::ArrayOpen)
            }
            b']' => {
                self.pos += 1;
                Some(Token::ArrayClose)
            }
            b'{' | b'}' | b')' => {
                self.pos += 1;
                Some(Token::Keyword(&self.buf[self.pos - 1..self.pos]))
            }
            b'0'..=b'9' | b'+' | b'-' | b'.' => Some(self.number()),
            _ => {
                let start = self.pos;
                while let Some(&c) = self.buf.get(self.pos) {
                    if is_white(c) || is_delim(c) {
                        break;
                    }
                    self.pos += 1;
                }
                Some(Token::Keyword(&self.buf[start..self.pos]))
            }
        }
    }

    /// §7.3.5: a name runs to the next white space or delimiter; `#xx` is one byte.
    fn name(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(&c) = self.buf.get(self.pos) {
            if is_white(c) || is_delim(c) {
                break;
            }
            if c == b'#' {
                let h = self.buf.get(self.pos + 1).copied().and_then(hex_val);
                let l = self.buf.get(self.pos + 2).copied().and_then(hex_val);
                if let (Some(h), Some(l)) = (h, l) {
                    out.push(h << 4 | l);
                    self.pos += 3;
                    continue;
                }
            }
            out.push(c);
            self.pos += 1;
        }
        out
    }

    /// §7.3.4.2: a literal string (the opening `(` already consumed).
    fn literal(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut depth = 1usize;
        while let Some(&c) = self.buf.get(self.pos) {
            self.pos += 1;
            match c {
                b'\\' => {
                    let Some(&e) = self.buf.get(self.pos) else { break };
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.buf.get(self.pos) {
                                    Some(&d @ b'0'..=b'7') => {
                                        v = v * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((v & 0xFF) as u8);
                        }
                        // A backslash before an end-of-line continues the string on the next line.
                        b'\r' => {
                            if self.buf.get(self.pos) == Some(&b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        // `\(`, `\)`, `\\`, and (§7.3.4.2) any other byte: the backslash is ignored.
                        other => out.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    out.push(c);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(c);
                }
                // An unescaped end-of-line (CR, CR LF, LF) reads as one LF.
                b'\r' => {
                    if self.buf.get(self.pos) == Some(&b'\n') {
                        self.pos += 1;
                    }
                    out.push(b'\n');
                }
                _ => out.push(c),
            }
        }
        out
    }

    /// §7.3.4.3: a hex string (the opening `<` already consumed). White space is skipped; an odd final digit is
    /// followed by an implied 0.
    fn hex(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;
        while let Some(&c) = self.buf.get(self.pos) {
            self.pos += 1;
            if c == b'>' {
                break;
            }
            if let Some(v) = hex_val(c) {
                match hi.take() {
                    Some(h) => out.push(h << 4 | v),
                    None => hi = Some(v),
                }
            }
        }
        if let Some(h) = hi {
            out.push(h << 4);
        }
        out
    }

    /// §7.3.3: an integer or a real. Repeated leading signs (`--5`) read as one, as other readers do.
    fn number(&mut self) -> Token<'a> {
        let mut neg = false;
        while let Some(&c) = self.buf.get(self.pos) {
            match c {
                b'-' => neg = true,
                b'+' => {}
                _ => break,
            }
            self.pos += 1;
        }
        let mut int: i64 = 0;
        let mut real: f64 = 0.0;
        let mut overflow = false;
        let mut digits = 0usize;
        // The digits as one integer mantissa while it fits; the value is mantissa / 10^frac, one correctly rounded
        // division (595.276 reads as 595.276, not as a sum of rounded tenths).
        let mut mant: u64 = 0;
        let mut mant_ok = true;
        let mut frac = 0i32;
        let push = |d: u8, mant: &mut u64, ok: &mut bool| match mant.checked_mul(10).and_then(|v| v.checked_add(d as u64)) {
            Some(v) if *ok && v < (1u64 << 53) => *mant = v,
            _ => *ok = false,
        };
        while let Some(&c @ b'0'..=b'9') = self.buf.get(self.pos) {
            let d = (c - b'0') as i64;
            match int.checked_mul(10).and_then(|v| v.checked_add(d)) {
                Some(v) if !overflow => int = v,
                _ => overflow = true,
            }
            real = real * 10.0 + d as f64;
            push(c - b'0', &mut mant, &mut mant_ok);
            digits += 1;
            self.pos += 1;
        }
        let mut is_real = overflow;
        if self.buf.get(self.pos) == Some(&b'.') {
            is_real = true;
            self.pos += 1;
            let mut scale = 0.1;
            while let Some(&c @ b'0'..=b'9') = self.buf.get(self.pos) {
                real += (c - b'0') as f64 * scale;
                scale *= 0.1;
                if frac < 22 {
                    push(c - b'0', &mut mant, &mut mant_ok);
                    frac += 1;
                }
                digits += 1;
                self.pos += 1;
            }
        }
        if mant_ok {
            let mut p10 = 1.0f64;
            for _ in 0..frac {
                p10 *= 10.0;
            }
            real = mant as f64 / p10;
        }
        // Junk glued to a number (`12a`) is skipped up to the next separator, as other readers do.
        while let Some(&c) = self.buf.get(self.pos) {
            if is_white(c) || is_delim(c) {
                break;
            }
            self.pos += 1;
        }
        if digits == 0 {
            return Token::Int(0);
        }
        if is_real {
            Token::Real(if neg { -real } else { real })
        } else {
            Token::Int(if neg { -int } else { int })
        }
    }
}
