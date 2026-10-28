// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The content-stream tokenizer (ISO 32000-1 §7.8.2): a sequence of operands followed by an operator. It
//! interprets nothing — PDF2 does. Inline images (§8.9.7, `BI` key/value pairs `ID` data `EI`) become one
//! [`Op`] whose operator is `BI` and whose operands are the image dictionary and the raw data.
//! A malformed token is skipped: a viewer draws what it can.

use alloc::vec::Vec;

use crate::lexer::{Lexer, Token, find, is_white};
use crate::object::{Dict, Object, parse_from};

/// One operator and its operands.
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    pub operands: Vec<Object>,
    pub operator: Vec<u8>,
}

/// Tokenize a content stream.
pub fn ops(data: &[u8]) -> Vec<Op> {
    let mut lx = Lexer::new(data);
    let mut out = Vec::new();
    let mut operands = Vec::new();
    while let Some(tok) = lx.next() {
        match tok {
            Token::Keyword(b"BI") => {
                operands.clear();
                if let Some(op) = inline_image(&mut lx) {
                    out.push(op);
                }
            }
            Token::Keyword(k) if !matches!(k, b"true" | b"false" | b"null") => {
                out.push(Op { operands: core::mem::take(&mut operands), operator: k.to_vec() });
            }
            t => {
                if let Ok(o) = parse_from(t, &mut lx, 0) {
                    operands.push(o);
                }
            }
        }
    }
    out
}

/// After `BI`: the dictionary up to `ID`, one white-space byte, the data, `EI`.
fn inline_image(lx: &mut Lexer<'_>) -> Option<Op> {
    let mut dict = Dict::new();
    loop {
        match lx.next()? {
            Token::Keyword(b"ID") => break,
            Token::Name(k) => {
                let t = lx.next()?;
                if let Ok(v) = parse_from(t, lx, 1) {
                    dict.insert(k, v);
                }
            }
            _ => {}
        }
    }
    let buf = lx.buf;
    let start = (lx.pos + 1).min(buf.len());
    // A declared length (/L, /Length) is used when `EI` follows it.
    let declared = dict.get(b"L").or_else(|| dict.get(b"Length")).and_then(Object::as_int);
    if let Some(len) = declared {
        if len >= 0 && (len as u64) <= (buf.len() - start) as u64 {
            let end = start + len as usize;
            let mut after = Lexer::at(buf, end);
            if after.next() == Some(Token::Keyword(b"EI")) {
                lx.pos = after.pos;
                return Some(image_op(dict, &buf[start..end]));
            }
        }
    }
    // Else: the first `EI` with white space before and after it (or the end of the data), followed by bytes that
    // look like content (ASCII), not more image data.
    let mut from = start;
    while let Some(p) = find(buf, b"EI", from) {
        from = p + 1;
        let before = p > start && is_white(buf[p - 1]);
        let after = buf.get(p + 2).is_none_or(|&b| is_white(b));
        if !(before && after) {
            continue;
        }
        let tail = &buf[(p + 2).min(buf.len())..(p + 10).min(buf.len())];
        if tail.iter().any(|&b| !(b.is_ascii_graphic() || is_white(b))) {
            continue;
        }
        lx.pos = p + 2;
        return Some(image_op(dict, &buf[start..p - 1]));
    }
    lx.pos = buf.len();
    Some(image_op(dict, &buf[start..]))
}

fn image_op(dict: Dict, data: &[u8]) -> Op {
    Op { operands: alloc::vec![Object::Dict(dict), Object::String(data.to_vec())], operator: b"BI".to_vec() }
}
