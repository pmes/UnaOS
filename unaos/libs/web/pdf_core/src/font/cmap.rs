// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! ONE CMap reader (ISO 32000-1 §9.7.5, §9.10.3; Adobe TN #5014 / #5411) for both uses: a Type 0 font's encoding
//! CMap (`begincodespacerange`, `begincidrange`, `begincidchar`, `beginnotdefrange`) and a /ToUnicode CMap
//! (`beginbfchar`, `beginbfrange` in both the base-string and the array form, UTF-16BE destinations with surrogate
//! pairs and multi-character ligatures). Tokens come from pdf_core's own lexer. `usecmap` is honoured for the
//! predefined Identity-H / Identity-V; the other predefined CJK CMaps are owed (read as Identity).

use alloc::string::String;
use alloc::vec::Vec;

use crate::lexer::{Lexer, Token};

/// A parsed CMap.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CMap {
    /// (low, high, byte length).
    pub codespace: Vec<(u32, u32, u8)>,
    /// (low, high, first CID) — cidchar entries are one-code ranges.
    cids: Vec<(u32, u32, u32)>,
    /// (low, high, notdef CID).
    notdef: Vec<(u32, u32, u32)>,
    /// (low, high, destination).
    bf: Vec<(u32, u32, Dest)>,
    /// /WMode 1 (vertical writing: metrics owed, codes read the same).
    pub vertical: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Dest {
    /// UTF-16BE units; the last unit is incremented by (code - low).
    Base(Vec<u16>),
    /// One string per code from low.
    List(Vec<String>),
}

fn code_of(b: &[u8]) -> Option<(u32, u8)> {
    if b.is_empty() || b.len() > 4 {
        return None;
    }
    Some((b.iter().fold(0u32, |a, &x| a << 8 | x as u32), b.len() as u8))
}

/// The next token if it is a string (consumed), else `None` with nothing consumed: a section ends at its `end...`
/// keyword without eating the tokens after it.
fn take_str(lx: &mut Lexer<'_>) -> Option<Vec<u8>> {
    match lx.peek() {
        Some(Token::Str(_)) => match lx.next() {
            Some(Token::Str(s)) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

fn take_int(lx: &mut Lexer<'_>) -> Option<u32> {
    match lx.peek() {
        Some(Token::Int(v)) => {
            lx.next();
            u32::try_from(v).ok()
        }
        _ => None,
    }
}

fn utf16(b: &[u8]) -> Vec<u16> {
    b.chunks(2).map(|c| if c.len() == 2 { u16::from_be_bytes([c[0], c[1]]) } else { c[0] as u16 }).collect()
}

fn utf16_string(u: &[u16]) -> String {
    char::decode_utf16(u.iter().copied()).map(|r| r.unwrap_or('\u{FFFD}')).collect()
}

impl CMap {
    /// The predefined Identity-H (`vertical` false) / Identity-V: two-byte codes, CID = code.
    pub fn identity(vertical: bool) -> CMap {
        CMap { codespace: alloc::vec![(0, 0xFFFF, 2)], cids: alloc::vec![(0, 0xFFFF, 0)], vertical, ..CMap::default() }
    }

    /// A predefined CMap by name: Identity-H / Identity-V; any other name is owed and read as Identity (two-byte).
    pub fn predefined(name: &[u8]) -> CMap {
        CMap::identity(name.ends_with(b"-V"))
    }

    /// Parse an embedded CMap stream (already decoded). Malformed entries are skipped.
    pub fn parse(data: &[u8]) -> CMap {
        let mut m = CMap::default();
        let mut lx = Lexer::new(data);
        let mut prev: Option<Token<'_>> = None;
        let mut last_name: Option<Vec<u8>> = None;
        while let Some(t) = lx.next() {
            match &t {
                Token::Name(n) => last_name = Some(n.clone()),
                Token::Int(v) if prev.as_ref().is_some_and(|p| matches!(p, Token::Name(k) if k == b"WMode")) => {
                    m.vertical = *v == 1;
                }
                Token::Keyword(b"usecmap") => {
                    if let Some(n) = last_name.take() {
                        let base = CMap::predefined(&n);
                        m.codespace.extend(base.codespace);
                        m.cids.extend(base.cids);
                    }
                }
                Token::Keyword(b"begincodespacerange") => {
                    while let Some(lo) = take_str(&mut lx) {
                        let Some(hi) = take_str(&mut lx) else { break };
                        if let (Some((l, n)), Some((h, _))) = (code_of(&lo), code_of(&hi)) {
                            m.codespace.push((l, h, n));
                        }
                    }
                }
                Token::Keyword(k @ (b"begincidrange" | b"beginnotdefrange")) => {
                    let notdef = *k == b"beginnotdefrange";
                    while let Some(lo) = take_str(&mut lx) {
                        let (Some(hi), Some(cid)) = (take_str(&mut lx), take_int(&mut lx)) else { break };
                        if let (Some((l, _)), Some((h, _))) = (code_of(&lo), code_of(&hi)) {
                            if notdef { m.notdef.push((l, h, cid)) } else { m.cids.push((l, h, cid)) }
                        }
                    }
                }
                Token::Keyword(b"begincidchar") => {
                    while let Some(c) = take_str(&mut lx) {
                        let Some(cid) = take_int(&mut lx) else { break };
                        if let Some((l, _)) = code_of(&c) {
                            m.cids.push((l, l, cid));
                        }
                    }
                }
                Token::Keyword(b"beginbfchar") => {
                    while let Some(c) = take_str(&mut lx) {
                        let Some(dst) = lx.next() else { break };
                        let Some((l, _)) = code_of(&c) else { continue };
                        match dst {
                            Token::Str(d) => m.bf.push((l, l, Dest::Base(utf16(&d)))),
                            // `/name` destinations (glyph names) are rare; read them through the AGL.
                            Token::Name(n) => {
                                if let Some(s) = super::encoding::agl(&String::from_utf8_lossy(&n)) {
                                    m.bf.push((l, l, Dest::List(alloc::vec![s])));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Token::Keyword(b"beginbfrange") => {
                    while let Some(lo) = take_str(&mut lx) {
                        let Some(hi) = take_str(&mut lx) else { break };
                        let (Some((l, _)), Some((h, _))) = (code_of(&lo), code_of(&hi)) else { continue };
                        match lx.next() {
                            Some(Token::Str(d)) => m.bf.push((l, h, Dest::Base(utf16(&d)))),
                            Some(Token::ArrayOpen) => {
                                let mut list = Vec::new();
                                while let Some(d) = take_str(&mut lx) {
                                    list.push(utf16_string(&utf16(&d)));
                                }
                                if matches!(lx.peek(), Some(Token::ArrayClose)) {
                                    lx.next();
                                }
                                m.bf.push((l, h, Dest::List(list)));
                            }
                            _ => break,
                        }
                    }
                }
                _ => {}
            }
            prev = Some(t);
        }
        m
    }

    /// Split the next code off `bytes` (§9.7.6.2): the shortest codespace range that matches; with none, the
    /// shortest partial match's length (one byte when nothing matches at all). Returns (code, length).
    pub fn next_code(&self, bytes: &[u8]) -> (u32, usize) {
        if self.codespace.is_empty() {
            let n = bytes.len().min(2).max(1);
            return (code_of(&bytes[..n.min(bytes.len())]).map(|c| c.0).unwrap_or(0), n);
        }
        let mut fallback = 0usize;
        for n in 1..=4usize {
            if n > bytes.len() {
                break;
            }
            let Some((code, _)) = code_of(&bytes[..n]) else { break };
            for &(lo, hi, len) in &self.codespace {
                if len as usize != n {
                    continue;
                }
                // Byte-wise range check (§9.7.6.2: each byte within its own lo/hi byte).
                let ok = (0..n).all(|i| {
                    let sh = 8 * (n - 1 - i);
                    let (b, l, h) = ((code >> sh) & 0xFF, (lo >> sh) & 0xFF, (hi >> sh) & 0xFF);
                    l <= b && b <= h
                });
                if ok {
                    return (code, n);
                }
                if fallback == 0 {
                    fallback = n;
                }
            }
        }
        let n = fallback.max(1).min(bytes.len().max(1));
        (code_of(&bytes[..n.min(bytes.len())]).map(|c| c.0).unwrap_or(0), n)
    }

    /// Every code of `bytes`, in order.
    pub fn codes(&self, bytes: &[u8]) -> Vec<(u32, usize)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let (c, n) = self.next_code(&bytes[i..]);
            out.push((c, n));
            i += n.max(1);
        }
        out
    }

    /// The CID of `code` (the last matching range wins, as later definitions override); notdef ranges, else 0.
    pub fn cid(&self, code: u32) -> u32 {
        if let Some(&(lo, _, c)) = self.cids.iter().rev().find(|r| r.0 <= code && code <= r.1) {
            return c + (code - lo);
        }
        self.notdef.iter().rev().find(|r| r.0 <= code && code <= r.1).map(|r| r.2).unwrap_or(0)
    }

    /// The Unicode text a ToUnicode CMap gives `code`.
    pub fn unicode(&self, code: u32) -> Option<String> {
        let (lo, _, d) = self.bf.iter().rev().find(|r| r.0 <= code && code <= r.1)?;
        let off = code - lo;
        match d {
            Dest::Base(u) => {
                let mut u = u.clone();
                let last = u.last_mut()?;
                *last = last.wrapping_add(off as u16);
                Some(utf16_string(&u))
            }
            Dest::List(l) => l.get(off as usize).cloned(),
        }
    }

    /// True when the CMap has bf entries (a ToUnicode map).
    pub fn has_unicode(&self) -> bool {
        !self.bf.is_empty()
    }
}
