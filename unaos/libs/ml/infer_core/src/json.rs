// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A small strict JSON reader (RFC 8259) for the three files a model ships: the safetensors
//! header, `config.json` and `tokenizer.json`. Numbers keep their source text so 64-bit offsets
//! are read exactly ([`Value::as_u64`]); nesting is capped at [`MAX_DEPTH`]; any malformation is
//! an [`Error`](crate::Error), never a panic.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{Result, err};

/// Deepest nesting accepted (a hostile header cannot exhaust the stack).
pub const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The number's source text (validated against the RFC 8259 grammar).
    Number(String),
    String(String),
    Array(Vec<Value>),
    /// Members in source order (duplicates kept; [`Value::get`] answers the first).
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }
    /// A non-negative integer written without fraction or exponent, exactly.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Number(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => s.parse().ok(),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(s) => s.parse().ok(),
            _ => None,
        }
    }
}

/// Parse one JSON document (surrounding whitespace allowed, nothing else after it).
pub fn parse(src: &str) -> Result<Value> {
    let mut p = Parser { s: src.as_bytes(), i: 0 };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(err("json: trailing characters"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.s.get(self.i) {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &[u8]) -> bool {
        if self.s[self.i..].starts_with(lit) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(err("json: nested too deeply"));
        }
        self.ws();
        match self.s.get(self.i) {
            None => Err(err("json: unexpected end")),
            Some(b'{') => {
                self.i += 1;
                let mut m = Vec::new();
                self.ws();
                if self.eat(b"}") {
                    return Ok(Value::Object(m));
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return Err(err("json: object key is not a string"));
                    }
                    let k = self.string()?;
                    self.ws();
                    if !self.eat(b":") {
                        return Err(err("json: missing ':'"));
                    }
                    let v = self.value(depth + 1)?;
                    m.push((k, v));
                    self.ws();
                    if self.eat(b",") {
                        continue;
                    }
                    if self.eat(b"}") {
                        return Ok(Value::Object(m));
                    }
                    return Err(err("json: expected ',' or '}'"));
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.eat(b"]") {
                    return Ok(Value::Array(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(b",") {
                        continue;
                    }
                    if self.eat(b"]") {
                        return Ok(Value::Array(a));
                    }
                    return Err(err("json: expected ',' or ']'"));
                }
            }
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') if self.eat(b"true") => Ok(Value::Bool(true)),
            Some(b'f') if self.eat(b"false") => Ok(Value::Bool(false)),
            Some(b'n') if self.eat(b"null") => Ok(Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(err("json: unexpected character")),
        }
    }

    fn number(&mut self) -> Result<Value> {
        let start = self.i;
        let digits = |p: &mut Self| {
            let s = p.i;
            while matches!(p.s.get(p.i), Some(b'0'..=b'9')) {
                p.i += 1;
            }
            p.i - s
        };
        self.eat(b"-");
        match self.s.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                digits(self);
            }
            _ => return Err(err("json: bad number")),
        }
        if self.eat(b".") && digits(self) == 0 {
            return Err(err("json: bad fraction"));
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if digits(self) == 0 {
                return Err(err("json: bad exponent"));
            }
        }
        // The slice is ASCII by construction.
        let text = core::str::from_utf8(&self.s[start..self.i]).map_err(|_| err("json: bad number"))?;
        Ok(Value::Number(String::from(text)))
    }

    fn hex4(&mut self) -> Result<u32> {
        let h = self.s.get(self.i..self.i + 4).ok_or_else(|| err("json: short \\u escape"))?;
        let mut v = 0u32;
        for &b in h {
            v = v * 16 + (b as char).to_digit(16).ok_or_else(|| err("json: bad \\u escape"))?;
        }
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            // Copy a run of plain bytes at once (the input is a &str, so runs are valid UTF-8
            // whenever they end at an ASCII byte).
            let start = self.i;
            while let Some(&b) = self.s.get(self.i) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.i += 1;
            }
            out.push_str(core::str::from_utf8(&self.s[start..self.i]).map_err(|_| err("json: bad UTF-8"))?);
            match self.s.get(self.i) {
                None => return Err(err("json: unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let e = *self.s.get(self.i).ok_or_else(|| err("json: unterminated escape"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                if !self.eat(b"\\u") {
                                    return Err(err("json: lone high surrogate"));
                                }
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(err("json: bad low surrogate"));
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            out.push(char::from_u32(cp).ok_or_else(|| err("json: lone surrogate"))?);
                        }
                        _ => return Err(err("json: bad escape")),
                    }
                }
                Some(_) => return Err(err("json: control character in string")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn parses_documents() {
        let v = parse(r#" {"a": [1, -2.5e3, true, null], "b": "x\u00e9\ud83d\ude00\n", "c": {}} "#).unwrap();
        assert_eq!(v.get("a").unwrap().as_array().unwrap().len(), 4);
        assert_eq!(v.get("a").unwrap().as_array().unwrap()[1].as_f64(), Some(-2500.0));
        assert_eq!(v.get("b").unwrap().as_str(), Some("xé😀\n"));
        assert_eq!(parse("18446744073709551615").unwrap().as_u64(), Some(u64::MAX));
        assert_eq!(parse("1.0").unwrap().as_u64(), None);
        assert_eq!(parse("[]").unwrap(), Value::Array(vec![]));
    }

    #[test]
    fn rejects_malformed() {
        for bad in ["", "{", "[1,]", "{\"a\" 1}", "01", "1.", "1e", "\"\\x\"", "\"\\ud800\"", "\"a", "nul", "[1] x", "\"\u{1}\""] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let deep = "[".repeat(MAX_DEPTH + 2);
        assert!(parse(&deep).is_err());
    }
}
