//! A strict DER reader (ITU-T X.690 §8, §10): definite lengths only, minimal length encoding, no indefinite form.

use crate::error::CertError;

pub mod tag {
    pub const BOOLEAN: u8 = 0x01;
    pub const INTEGER: u8 = 0x02;
    pub const BIT_STRING: u8 = 0x03;
    pub const OCTET_STRING: u8 = 0x04;
    pub const NULL: u8 = 0x05;
    pub const OID: u8 = 0x06;
    pub const UTF8_STRING: u8 = 0x0c;
    pub const PRINTABLE_STRING: u8 = 0x13;
    pub const IA5_STRING: u8 = 0x16;
    pub const UTC_TIME: u8 = 0x17;
    pub const GENERALIZED_TIME: u8 = 0x18;
    pub const SEQUENCE: u8 = 0x30;
    pub const SET: u8 = 0x31;
    /// [n] EXPLICIT / constructed context-specific.
    pub const fn context_constructed(n: u8) -> u8 {
        0xa0 | n
    }
    /// [n] IMPLICIT primitive context-specific.
    pub const fn context_primitive(n: u8) -> u8 {
        0x80 | n
    }
}

const fn bad(m: &'static str) -> CertError {
    CertError::BadDer(m)
}

/// One TLV: tag, contents, and the full encoding (tag+length+contents).
#[derive(Debug, Clone, Copy)]
pub struct Tlv<'a> {
    pub tag: u8,
    pub value: &'a [u8],
    pub raw: &'a [u8],
}

#[derive(Clone)]
pub struct Der<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Der<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Der { buf, pos: 0 }
    }
    pub fn is_empty(&self) -> bool {
        self.pos >= self.buf.len()
    }
    pub fn peek_tag(&self) -> Option<u8> {
        self.buf.get(self.pos).copied()
    }
    pub fn expect_end(&self) -> Result<(), CertError> {
        if self.is_empty() { Ok(()) } else { Err(bad("trailing data")) }
    }

    pub fn tlv(&mut self) -> Result<Tlv<'a>, CertError> {
        let start = self.pos;
        let b = self.buf;
        let tag = *b.get(self.pos).ok_or(bad("truncated tag"))?;
        if tag & 0x1f == 0x1f {
            return Err(bad("high-tag-number form unsupported"));
        }
        self.pos += 1;
        let l0 = *b.get(self.pos).ok_or(bad("truncated length"))?;
        self.pos += 1;
        let len = if l0 < 0x80 {
            l0 as usize
        } else {
            let n = (l0 & 0x7f) as usize;
            if n == 0 {
                return Err(bad("indefinite length"));
            }
            if n > 4 {
                return Err(bad("length too long"));
            }
            let mut v = 0usize;
            for i in 0..n {
                let x = *b.get(self.pos + i).ok_or(bad("truncated length"))?;
                if i == 0 && x == 0 {
                    return Err(bad("non-minimal length"));
                }
                v = (v << 8) | x as usize;
            }
            if v < 0x80 {
                return Err(bad("non-minimal length"));
            }
            self.pos += n;
            v
        };
        let end = self.pos.checked_add(len).ok_or(bad("length overflow"))?;
        if end > b.len() {
            return Err(bad("truncated value"));
        }
        let value = &b[self.pos..end];
        self.pos = end;
        Ok(Tlv { tag, value, raw: &b[start..end] })
    }

    pub fn expect(&mut self, t: u8) -> Result<Tlv<'a>, CertError> {
        let v = self.tlv()?;
        if v.tag != t {
            return Err(bad("unexpected tag"));
        }
        Ok(v)
    }

    /// Reads an element with tag `t` if it is next.
    pub fn optional(&mut self, t: u8) -> Result<Option<Tlv<'a>>, CertError> {
        if self.peek_tag() == Some(t) { Ok(Some(self.tlv()?)) } else { Ok(None) }
    }

    pub fn sequence(&mut self) -> Result<Der<'a>, CertError> {
        Ok(Der::new(self.expect(tag::SEQUENCE)?.value))
    }

    pub fn boolean(&mut self) -> Result<bool, CertError> {
        let v = self.expect(tag::BOOLEAN)?.value;
        match v {
            [0x00] => Ok(false),
            [0xff] => Ok(true),
            _ => Err(bad("BOOLEAN must be 00 or FF")),
        }
    }

    /// A non-negative INTEGER's magnitude bytes (leading zero stripped). Rejects negatives and non-minimal forms.
    pub fn uint(&mut self) -> Result<&'a [u8], CertError> {
        let v = self.expect(tag::INTEGER)?.value;
        integer_magnitude(v)
    }

    /// A small non-negative INTEGER.
    pub fn small_uint(&mut self) -> Result<u64, CertError> {
        let m = self.uint()?;
        if m.len() > 8 {
            return Err(bad("integer too large"));
        }
        Ok(m.iter().fold(0u64, |a, &b| (a << 8) | b as u64))
    }

    pub fn oid(&mut self) -> Result<&'a [u8], CertError> {
        Ok(self.expect(tag::OID)?.value)
    }

    /// BIT STRING contents; requires zero unused bits.
    pub fn bit_string_bytes(&mut self) -> Result<&'a [u8], CertError> {
        let v = self.expect(tag::BIT_STRING)?.value;
        match v.split_first() {
            Some((0, rest)) => Ok(rest),
            _ => Err(bad("BIT STRING with unused bits")),
        }
    }
}

/// INTEGER contents → magnitude; rejects negative and non-minimal encodings.
pub fn integer_magnitude(v: &[u8]) -> Result<&[u8], CertError> {
    if v.is_empty() {
        return Err(bad("empty INTEGER"));
    }
    if v[0] & 0x80 != 0 {
        return Err(bad("negative INTEGER"));
    }
    if v.len() > 1 && v[0] == 0 {
        if v[1] & 0x80 == 0 {
            return Err(bad("non-minimal INTEGER"));
        }
        return Ok(&v[1..]);
    }
    Ok(v)
}

/// Days from 1970-01-01 to y-m-d (proleptic Gregorian; Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn digits(s: &[u8]) -> Result<u32, CertError> {
    let mut v = 0u32;
    for &c in s {
        if !c.is_ascii_digit() {
            return Err(bad("time digit"));
        }
        v = v * 10 + (c - b'0') as u32;
    }
    Ok(v)
}

/// Parses an X.509 Time (RFC 5280 §4.1.2.5): UTCTime YYMMDDHHMMSSZ (YY ≥ 50 → 19YY) or GeneralizedTime
/// YYYYMMDDHHMMSSZ. Returns seconds since the Unix epoch.
pub fn parse_time(t: &Tlv<'_>) -> Result<i64, CertError> {
    let v = t.value;
    let (year, rest) = match t.tag {
        tag::UTC_TIME => {
            if v.len() != 13 {
                return Err(bad("UTCTime must be YYMMDDHHMMSSZ"));
            }
            let yy = digits(&v[0..2])? as i64;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &v[2..])
        }
        tag::GENERALIZED_TIME => {
            if v.len() != 15 {
                return Err(bad("GeneralizedTime must be YYYYMMDDHHMMSSZ"));
            }
            (digits(&v[0..4])? as i64, &v[4..])
        }
        _ => return Err(bad("not a Time")),
    };
    if rest[10] != b'Z' {
        return Err(bad("Time must be Zulu"));
    }
    let mo = digits(&rest[0..2])?;
    let d = digits(&rest[2..4])?;
    let h = digits(&rest[4..6])?;
    let mi = digits(&rest[6..8])?;
    let s = digits(&rest[8..10])?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return Err(bad("Time out of range"));
    }
    Ok(days_from_civil(year, mo, d) * 86400 + (h * 3600 + mi * 60 + s) as i64)
}

/// Encodes one definite-length DER TLV (low tag numbers only).
pub fn encode_tlv(t: u8, content: &[u8]) -> alloc::vec::Vec<u8> {
    let n = content.len();
    let mut out = alloc::vec::Vec::with_capacity(n + 6);
    out.push(t);
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes = (n as u32).to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | (4 - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(content);
    out
}
