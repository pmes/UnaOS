//! URL Standard §1.3 percent-encoded bytes: the percent-encode sets, UTF-8 percent-encode and percent-decode.

use alloc::string::String;
use alloc::vec::Vec;

/// The percent-encode sets of §1.3, each a superset of the one before it in the standard's chain
/// (C0 control ⊂ fragment, C0 control ⊂ query ⊂ special-query, query ⊂ path ⊂ userinfo ⊂ component ⊂
/// application/x-www-form-urlencoded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeSet {
    C0Control,
    Fragment,
    Query,
    SpecialQuery,
    Path,
    Userinfo,
    Component,
    FormUrlencoded,
}

/// Whether byte `b` is in `set` (every byte above U+007E is in every set: it is a non-ASCII UTF-8 byte or DEL).
pub fn in_set(set: EncodeSet, b: u8) -> bool {
    let c0 = b < 0x20 || b > 0x7E;
    match set {
        EncodeSet::C0Control => c0,
        EncodeSet::Fragment => c0 || matches!(b, b' ' | b'"' | b'<' | b'>' | b'`'),
        EncodeSet::Query => c0 || matches!(b, b' ' | b'"' | b'#' | b'<' | b'>'),
        EncodeSet::SpecialQuery => in_set(EncodeSet::Query, b) || b == b'\'',
        EncodeSet::Path => in_set(EncodeSet::Query, b) || matches!(b, b'?' | b'^' | b'`' | b'{' | b'}'),
        EncodeSet::Userinfo => {
            in_set(EncodeSet::Path, b) || matches!(b, b'/' | b':' | b';' | b'=' | b'@' | b'[' | b'\\' | b']' | b'|')
        }
        EncodeSet::Component => in_set(EncodeSet::Userinfo, b) || matches!(b, b'$' | b'%' | b'&' | b'+' | b','),
        EncodeSet::FormUrlencoded => in_set(EncodeSet::Component, b) || matches!(b, b'!' | b'\'' | b'(' | b')' | b'~'),
    }
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// Percent-encode one byte: `%XX`, uppercase hex (§1.3 "percent-encode a byte").
pub fn push_byte(out: &mut String, b: u8) {
    out.push('%');
    out.push(HEX[(b >> 4) as usize] as char);
    out.push(HEX[(b & 15) as usize] as char);
}

/// UTF-8 percent-encode `s` using `set`, appending to `out` (§1.3 "UTF-8 percent-encode", spaceAsPlus false).
pub fn percent_encode_into(out: &mut String, s: &str, set: EncodeSet) {
    for &b in s.as_bytes() {
        if in_set(set, b) {
            push_byte(out, b);
        } else {
            out.push(b as char);
        }
    }
}

/// UTF-8 percent-encode `s` using `set`.
pub fn percent_encode(s: &str, set: EncodeSet) -> String {
    let mut o = String::with_capacity(s.len());
    percent_encode_into(&mut o, s, set);
    o
}

/// UTF-8 percent-encode one code point using `set`, appending to `out`.
pub fn push_char_encoded(out: &mut String, c: char, set: EncodeSet) {
    let mut b = [0u8; 4];
    percent_encode_into(out, c.encode_utf8(&mut b), set);
}

pub(crate) fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// §1.3 percent-decode a byte sequence: `%` followed by two ASCII hex digits becomes that byte; any other `%`
/// is kept as is.
pub fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            if let (Some(h), Some(l)) = (hexval(input[i + 1]), hexval(input[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

/// §1.3 percent-decode a string: UTF-8 encode, percent-decode, then UTF-8 decode without BOM (invalid
/// sequences become U+FFFD).
pub fn percent_decode_str(input: &str) -> String {
    String::from_utf8_lossy(&percent_decode(input.as_bytes())).into_owned()
}
