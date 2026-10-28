// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The PDF object model (ISO 32000-1 §7.3) and its parser.

use alloc::string::String;
use alloc::vec::Vec;

use crate::lexer::{Lexer, Token, find};
use crate::{Error, MAX_DEPTH};

/// An indirect reference `num gen R` (§7.3.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ref {
    pub num: u32,
    pub generation: u16,
}

/// A dictionary (§7.3.7): keys without their `/`, in file order. A repeated key keeps its last value.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dict(pub Vec<(Vec<u8>, Object)>);

impl Dict {
    pub fn new() -> Self {
        Dict(Vec::new())
    }
    pub fn get(&self, key: &[u8]) -> Option<&Object> {
        self.0.iter().find(|(k, _)| k.as_slice() == key).map(|(_, v)| v)
    }
    pub fn insert(&mut self, key: Vec<u8>, value: Object) {
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }
    pub fn contains(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &Object)> {
        self.0.iter().map(|(k, v)| (k.as_slice(), v))
    }
    /// The value at `key` when it is a direct name.
    pub fn name(&self, key: &[u8]) -> Option<&[u8]> {
        self.get(key).and_then(Object::as_name)
    }
    /// The value at `key` when it is a direct number, as an integer.
    pub fn int(&self, key: &[u8]) -> Option<i64> {
        self.get(key).and_then(Object::as_int)
    }
}

/// A stream (§7.3.8): its dictionary and its bytes, still encoded (`filters::decode` undoes /Filter).
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub dict: Dict,
    pub data: Vec<u8>,
}

/// A PDF object.
#[derive(Debug, Clone, PartialEq)]
pub enum Object {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    String(Vec<u8>),
    Name(Vec<u8>),
    Array(Vec<Object>),
    Dict(Dict),
    Stream(Stream),
    Ref(Ref),
}

impl Object {
    /// An integer, or a real truncated toward zero.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Object::Int(i) => Some(*i),
            Object::Real(r) => Some(*r as i64),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Object::Int(i) => Some(*i as f64),
            Object::Real(r) => Some(*r),
            _ => None,
        }
    }
    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Object::Name(n) => Some(n),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&[u8]> {
        match self {
            Object::String(s) => Some(s),
            _ => None,
        }
    }
    /// A dictionary, or a stream's dictionary.
    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Object]> {
        match self {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_ref(&self) -> Option<Ref> {
        match self {
            Object::Ref(r) => Some(*r),
            _ => None,
        }
    }
    pub fn as_stream(&self) -> Option<&Stream> {
        match self {
            Object::Stream(s) => Some(s),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Object::Null)
    }
}

/// Parse one direct object at the lexer's position (no `stream` body: that needs the indirect wrapper).
pub fn parse_object(lx: &mut Lexer<'_>, depth: usize) -> Result<Object, Error> {
    let start = lx.pos;
    let tok = lx.next().ok_or(Error::Syntax("end of data where an object was expected", start))?;
    parse_from(tok, lx, depth)
}

/// Parse one direct object whose first token has already been read.
pub fn parse_from(tok: Token<'_>, lx: &mut Lexer<'_>, depth: usize) -> Result<Object, Error> {
    if depth > MAX_DEPTH {
        return Err(Error::Depth);
    }
    match tok {
        Token::Int(n) => {
            // `num gen R`: two more tokens of lookahead.
            if (0..=u32::MAX as i64).contains(&n) {
                let mut look = lx.clone();
                if let Some(Token::Int(g)) = look.next() {
                    if (0..=u16::MAX as i64).contains(&g) {
                        if let Some(Token::Keyword(b"R")) = look.next() {
                            *lx = look;
                            return Ok(Object::Ref(Ref { num: n as u32, generation: g as u16 }));
                        }
                    }
                }
            }
            Ok(Object::Int(n))
        }
        Token::Real(r) => Ok(Object::Real(r)),
        Token::Str(s) => Ok(Object::String(s)),
        Token::Name(n) => Ok(Object::Name(n)),
        Token::ArrayOpen => {
            let mut items = Vec::new();
            loop {
                match lx.next() {
                    None | Some(Token::ArrayClose) => break,
                    Some(t) => items.push(parse_from(t, lx, depth + 1)?),
                }
            }
            Ok(Object::Array(items))
        }
        Token::DictOpen => {
            let mut d = Dict::new();
            loop {
                match lx.next() {
                    None | Some(Token::DictClose) => break,
                    Some(Token::Name(k)) => match lx.next() {
                        None => break,
                        Some(Token::DictClose) => {
                            d.insert(k, Object::Null);
                            break;
                        }
                        Some(t) => {
                            let v = parse_from(t, lx, depth + 1)?;
                            d.insert(k, v);
                        }
                    },
                    // A value with no key: skipped, as other readers do.
                    Some(_) => continue,
                }
            }
            Ok(Object::Dict(d))
        }
        Token::Keyword(k) => match k {
            b"true" => Ok(Object::Bool(true)),
            b"false" => Ok(Object::Bool(false)),
            b"null" => Ok(Object::Null),
            _ => Err(Error::Syntax("unexpected keyword", lx.pos)),
        },
        Token::ArrayClose | Token::DictClose => Err(Error::Syntax("unbalanced close", lx.pos)),
    }
}

/// Parse the indirect object `num gen obj ... endobj` at `pos`. A dictionary followed by `stream` becomes a
/// [`Stream`]: its length is /Length when that is right (`len_of` resolves an indirect /Length), else the bytes up to
/// `endstream`.
pub fn parse_indirect(
    buf: &[u8],
    pos: usize,
    len_of: &mut dyn FnMut(Ref) -> Option<i64>,
) -> Result<(Ref, Object), Error> {
    let mut lx = Lexer::at(buf, pos);
    let num = match lx.next() {
        Some(Token::Int(n)) if (0..=u32::MAX as i64).contains(&n) => n as u32,
        _ => return Err(Error::Syntax("object number expected", pos)),
    };
    let generation = match lx.next() {
        Some(Token::Int(g)) if (0..=u16::MAX as i64).contains(&g) => g as u16,
        _ => return Err(Error::Syntax("generation number expected", pos)),
    };
    match lx.next() {
        Some(Token::Keyword(b"obj")) => {}
        _ => return Err(Error::Syntax("`obj` expected", pos)),
    }
    let r = Ref { num, generation };
    let at = lx.pos;
    let tok = match lx.next() {
        None => return Err(Error::Syntax("end of data inside an object", at)),
        Some(Token::Keyword(b"endobj")) => return Ok((r, Object::Null)),
        Some(t) => t,
    };
    let obj = parse_from(tok, &mut lx, 0)?;
    let Object::Dict(dict) = obj else { return Ok((r, obj)) };
    let mut look = lx.clone();
    if look.next() != Some(Token::Keyword(b"stream")) {
        return Ok((r, Object::Dict(dict)));
    }
    // §7.3.8.1: `stream` is followed by CR LF or LF (a bare CR is tolerated).
    let mut start = look.pos;
    if buf.get(start) == Some(&b'\r') {
        start += 1;
    }
    if buf.get(start) == Some(&b'\n') {
        start += 1;
    }
    let declared = match dict.get(b"Length") {
        Some(Object::Ref(lr)) => len_of(*lr),
        Some(o) => o.as_int(),
        None => None,
    };
    let mut end = None;
    if let Some(len) = declared {
        if len >= 0 && (len as u64) <= (buf.len() - start) as u64 {
            let e = start + len as usize;
            let mut after = Lexer::at(buf, e);
            if after.next() == Some(Token::Keyword(b"endstream")) {
                end = Some(e);
            }
        }
    }
    let end = match end {
        Some(e) => e,
        None => {
            let mut e = find(buf, b"endstream", start).unwrap_or(buf.len());
            // The end-of-line before `endstream` is not data.
            if e > start && buf[e - 1] == b'\n' {
                e -= 1;
            }
            if e > start && buf[e - 1] == b'\r' {
                e -= 1;
            }
            e
        }
    };
    Ok((r, Object::Stream(Stream { dict, data: buf[start..end].to_vec() })))
}

/// PDFDocEncoding (§D.2) bytes 0x18..=0x1F.
const PDFDOC_18: [u16; 8] = [0x02D8, 0x02C7, 0x02C6, 0x02D9, 0x02DD, 0x02DB, 0x02DA, 0x02DC];
/// PDFDocEncoding bytes 0x80..=0xA0 (0x9F is undefined).
const PDFDOC_80: [u16; 33] = [
    0x2022, 0x2020, 0x2021, 0x2026, 0x2014, 0x2013, 0x0192, 0x2044, 0x2039, 0x203A, 0x2212, 0x2030, 0x201E, 0x201C,
    0x201D, 0x2018, 0x2019, 0x201A, 0x2122, 0xFB01, 0xFB02, 0x0141, 0x0152, 0x0160, 0x0178, 0x017D, 0x0131, 0x0142,
    0x0153, 0x0161, 0x017E, 0xFFFD, 0x20AC,
];

/// A text string (§7.9.2.2) as Rust text: UTF-16BE after a `FE FF` byte-order mark (surrogate pairs joined, a lone
/// surrogate becomes U+FFFD), UTF-8 after `EF BB BF` (PDF 2.0), else PDFDocEncoding.
pub fn text_string(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let units = b[2..].chunks(2).map(|c| (c[0] as u16) << 8 | *c.get(1).unwrap_or(&0) as u16);
        return core::char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')).collect();
    }
    if b.len() >= 3 && b[..3] == [0xEF, 0xBB, 0xBF] {
        return String::from_utf8_lossy(&b[3..]).into_owned();
    }
    b.iter()
        .map(|&c| {
            let u = match c {
                0x18..=0x1F => PDFDOC_18[(c - 0x18) as usize] as u32,
                0x80..=0xA0 => PDFDOC_80[(c - 0x80) as usize] as u32,
                0x7F | 0xAD => 0xFFFD,
                _ => c as u32,
            };
            char::from_u32(u).unwrap_or('\u{FFFD}')
        })
        .collect()
}
