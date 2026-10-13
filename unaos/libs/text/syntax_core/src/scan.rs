// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Byte-level helpers the grammars share. Lexing is over bytes: every construct the five grammars
//! delimit is ASCII, and a byte >= 0x80 (part of a UTF-8 sequence) is treated as an identifier byte,
//! so a token boundary never falls inside a code point.

/// `[A-Za-z_]` or a non-ASCII byte.
#[inline]
pub fn ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

/// `[A-Za-z0-9_]` or a non-ASCII byte.
#[inline]
pub fn ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// The byte at `i`, or 0 past the end.
#[inline]
pub fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

/// Does `s[i..]` start with `p`?
#[inline]
pub fn starts(s: &[u8], i: usize, p: &[u8]) -> bool {
    s.len() >= i + p.len() && &s[i..i + p.len()] == p
}

/// Advance from `i` while `f` holds; returns the first index where it does not.
#[inline]
pub fn skip(s: &[u8], mut i: usize, f: impl Fn(u8) -> bool) -> usize {
    while i < s.len() && f(s[i]) {
        i += 1;
    }
    i
}

/// Space or tab.
#[inline]
pub fn blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Length in bytes of the UTF-8 sequence led by `b` (1 for ASCII or a stray continuation byte).
#[inline]
pub fn utf8_len(b: u8) -> usize {
    if b < 0xC0 {
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    }
}
