//! A small strict JSON reader (RFC 8259) — enough for CT log lists (Google's v3 schema, Apple's). `no_std` +
//! `alloc`; numbers are kept as their source text (the lists carry integers only, e.g. `mmd`).

use alloc::string::String;
use alloc::vec::Vec;

/// A JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The number's text, validated against the RFC 8259 §6 grammar.
    Num(String),
    Str(String),
    Arr(Vec<Value>),
    /// Members in document order (duplicate names are refused).
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> &[Value] {
        match self {
            Value::Arr(a) => a,
            _ => &[],
        }
    }
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Num(n) => n.parse().ok(),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Obj(m) => Some(m),
            _ => None,
        }
    }
}

const MAX_DEPTH: usize = 64;

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

/// Parses one JSON text (whitespace around it allowed, nothing else).
pub fn parse(text: &[u8]) -> Option<Value> {
    let text = text.strip_prefix(b"\xef\xbb\xbf").unwrap_or(text);
    let mut p = P { b: text, i: 0 };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return None;
    }
    Some(v)
}

impl P<'_> {
    fn ws(&mut self) {
        while let Some(&c) = self.b.get(self.i) {
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                self.i += 1;
            } else {
                break;
            }
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.b.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn lit(&mut self, s: &[u8]) -> bool {
        if self.b[self.i..].starts_with(s) {
            self.i += s.len();
            true
        } else {
            false
        }
    }
    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut m: Vec<(String, Value)> = Vec::new();
                self.ws();
                if self.eat(b'}') {
                    return Some(Value::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    if m.iter().any(|(x, _)| *x == k) {
                        return None;
                    }
                    self.ws();
                    if !self.eat(b':') {
                        return None;
                    }
                    let v = self.value(depth + 1)?;
                    m.push((k, v));
                    self.ws();
                    if self.eat(b',') {
                        continue;
                    }
                    if self.eat(b'}') {
                        return Some(Value::Obj(m));
                    }
                    return None;
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.eat(b']') {
                    return Some(Value::Arr(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(b',') {
                        continue;
                    }
                    if self.eat(b']') {
                        return Some(Value::Arr(a));
                    }
                    return None;
                }
            }
            b'"' => Some(Value::Str(self.string()?)),
            b't' => self.lit(b"true").then_some(Value::Bool(true)),
            b'f' => self.lit(b"false").then_some(Value::Bool(false)),
            b'n' => self.lit(b"null").then_some(Value::Null),
            _ => self.number(),
        }
    }
    fn number(&mut self) -> Option<Value> {
        let s = self.i;
        self.eat(b'-');
        let digits = |p: &mut Self| {
            let st = p.i;
            while p.b.get(p.i).is_some_and(|c| c.is_ascii_digit()) {
                p.i += 1;
            }
            p.i - st
        };
        if self.b.get(self.i) == Some(&b'0') {
            self.i += 1;
        } else if digits(self) == 0 {
            return None;
        }
        if self.eat(b'.') && digits(self) == 0 {
            return None;
        }
        if self.eat(b'e') || self.eat(b'E') {
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if digits(self) == 0 {
                return None;
            }
        }
        Some(Value::Num(String::from(core::str::from_utf8(&self.b[s..self.i]).ok()?)))
    }
    fn hex4(&mut self) -> Option<u32> {
        let h = self.b.get(self.i..self.i + 4)?;
        self.i += 4;
        u32::from_str_radix(core::str::from_utf8(h).ok()?, 16).ok()
    }
    fn string(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xd800..0xdc00).contains(&hi) {
                                if !self.lit(b"\\u") {
                                    return None;
                                }
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return None;
                                }
                                0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                            } else {
                                hi
                            };
                            char::from_u32(cp)?
                        }
                        _ => return None,
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                0x00..=0x1f => return None,
                _ => out.push(c),
            }
        }
    }
}

/// RFC 3339 date-time (`2026-10-04T13:39:14Z`, fractional seconds and ±hh:mm offsets accepted) → Unix seconds.
pub fn rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b't') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: core::ops::Range<usize>| -> Option<i64> {
        let t = &b[r];
        if !t.iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        core::str::from_utf8(t).ok()?.parse().ok()
    };
    let (y, mo, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let st = i;
        while b.get(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
        if i == st {
            return None;
        }
    }
    let off = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        c @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let o = num(i + 1..i + 3)? * 3600 + num(i + 4..i + 6)? * 60;
            if *c == b'+' { o } else { -o }
        }
        _ => return None,
    };
    // days_from_civil (H. Hinnant).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec - off)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_refuses() {
        let v = parse(br#" {"a": [1, -2.5e3, true, null, "x\u00e9\ud83d\ude00\n"], "b": {}} "#).unwrap();
        assert_eq!(v.get("a").unwrap().as_arr().len(), 5);
        assert_eq!(v.get("a").unwrap().as_arr()[4].as_str(), Some("xé😀\n"));
        for bad in [&b"{\"a\":1,\"a\":2}"[..], b"[1,]", b"01", b"\"\x01\"", b"{} x", b"[\"\\ud800\"]", b"1.", b"-"] {
            assert!(parse(bad).is_none(), "{:?}", core::str::from_utf8(bad));
        }
        assert_eq!(rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339("2026-10-04T13:39:14Z"), Some(1_791_121_154));
        assert_eq!(rfc3339("2026-10-04T15:39:14.250+02:00"), Some(1_791_121_154));
        assert_eq!(rfc3339("2026-13-04T13:39:14Z"), None);
    }
}
