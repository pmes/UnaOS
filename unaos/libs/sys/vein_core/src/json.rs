// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The JSON the Messages API needs, without an allocator: a string escaper for the request and a
//! structural scanner + in-place unescaper for the stream events. Not a general parser: it finds a key's
//! raw value in an object (skipping nested values correctly) and decodes a string value.

use crate::Out;

/// Write `s` as the INSIDE of a JSON string (no quotes): `"` `\` and controls escaped.
pub fn escape_into(s: &[u8], o: &mut Out<'_>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut start = 0;
    for (i, &c) in s.iter().enumerate() {
        let esc: Option<&[u8]> = match c {
            b'"' => Some(b"\\\""),
            b'\\' => Some(b"\\\\"),
            b'\n' => Some(b"\\n"),
            b'\r' => Some(b"\\r"),
            b'\t' => Some(b"\\t"),
            0..=0x1f => None,
            _ => continue,
        };
        o.put(&s[start..i]);
        match esc {
            Some(e) => o.put(e),
            None => o.put(&[b'\\', b'u', b'0', b'0', HEX[(c >> 4) as usize], HEX[(c & 15) as usize]]),
        }
        start = i + 1;
    }
    o.put(&s[start..]);
}

fn ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    i
}

/// End (exclusive) of the string starting at the `"` at `i`.
fn skip_str(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// End (exclusive) of the value starting at `i` (after whitespace).
fn skip_value(b: &[u8], i: usize) -> Option<usize> {
    let i = ws(b, i);
    match *b.get(i)? {
        b'"' => skip_str(b, i),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = skip_str(b, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
                j += 1;
            }
            if j == i { None } else { Some(j) }
        }
    }
}

/// The raw value of top-level `key` in the object `obj` (e.g. `"text"` gives `"\"hi\""` with quotes).
pub fn get<'a>(obj: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let mut i = ws(obj, 0);
    if obj.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;
    loop {
        i = ws(obj, i);
        match *obj.get(i)? {
            b'}' => return None,
            b',' => {
                i += 1;
                continue;
            }
            b'"' => {}
            _ => return None,
        }
        let ke = skip_str(obj, i)?;
        let k = &obj[i + 1..ke - 1];
        i = ws(obj, ke);
        if obj.get(i) != Some(&b':') {
            return None;
        }
        let vs = ws(obj, i + 1);
        let ve = skip_value(obj, vs)?;
        if k == key.as_bytes() {
            return Some(&obj[vs..ve]);
        }
        i = ve;
    }
}

/// A raw string value without escapes, as `&str` (for short tag values like `"text_delta"`).
pub fn plain_str(v: &[u8]) -> Option<&str> {
    if v.len() < 2 || v[0] != b'"' || v[v.len() - 1] != b'"' || v.contains(&b'\\') {
        return None;
    }
    core::str::from_utf8(&v[1..v.len() - 1]).ok()
}

fn hex4(b: &[u8]) -> Option<u32> {
    let mut v = 0u32;
    for &c in b.get(..4)? {
        v = v * 16 + (c as char).to_digit(16)?;
    }
    Some(v)
}

/// Decode the JSON string whose raw form (with quotes) occupies `buf[s..e]` IN PLACE (an escape is never
/// shorter than what it decodes to). Returns the decoded range `s..s+n`, or `None` if malformed.
/// A lone surrogate decodes to U+FFFD.
pub fn unescape_in_place(buf: &mut [u8], s: usize, e: usize) -> Option<core::ops::Range<usize>> {
    if e < s + 2 || buf[s] != b'"' || buf[e - 1] != b'"' {
        return None;
    }
    let (mut r, mut w, end) = (s + 1, s, e - 1);
    while r < end {
        let c = buf[r];
        if c != b'\\' {
            buf[w] = c;
            w += 1;
            r += 1;
            continue;
        }
        let k = *buf.get(r + 1)?;
        r += 2;
        let simple = match k {
            b'"' => Some(b'"'),
            b'\\' => Some(b'\\'),
            b'/' => Some(b'/'),
            b'b' => Some(8),
            b'f' => Some(12),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b't' => Some(b'\t'),
            b'u' => None,
            _ => return None,
        };
        if let Some(ch) = simple {
            buf[w] = ch;
            w += 1;
            continue;
        }
        let mut cp = hex4(buf.get(r..end)?)?;
        r += 4;
        if (0xD800..0xDC00).contains(&cp) {
            let lo = if buf.get(r..r + 2) == Some(b"\\u") { buf.get(r + 2..end).and_then(hex4) } else { None };
            match lo {
                Some(lo) if (0xDC00..0xE000).contains(&lo) => {
                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                    r += 6;
                }
                _ => cp = 0xFFFD,
            }
        } else if (0xDC00..0xE000).contains(&cp) {
            cp = 0xFFFD;
        }
        let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
        let mut tmp = [0u8; 4];
        let enc = ch.encode_utf8(&mut tmp).as_bytes();
        buf[w..w + enc.len()].copy_from_slice(enc);
        w += enc.len();
    }
    Some(s..w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_round_trip_shapes() {
        let mut b = [0u8; 64];
        let mut o = Out::new(&mut b);
        escape_into(b"a\"b\\c\nd\x01", &mut o);
        let n = o.done().unwrap();
        assert_eq!(&b[..n], b"a\\\"b\\\\c\\nd\\u0001");
    }

    #[test]
    fn get_skips_nested_values_and_finds_keys() {
        let j = br#"{"type":"content_block_delta","index":0,"x":{"type":"no","t":[1,{"a":"}"}]},"delta":{"type":"text_delta","text":"Hi \"there\""}}"#;
        assert_eq!(plain_str(get(j, "type").unwrap()), Some("content_block_delta"));
        let d = get(j, "delta").unwrap();
        assert_eq!(plain_str(get(d, "type").unwrap()), Some("text_delta"));
        assert_eq!(get(d, "text").unwrap(), br#""Hi \"there\"""#);
        assert!(get(j, "missing").is_none());
        assert_eq!(get(j, "index").unwrap(), b"0");
    }

    #[test]
    fn unescape_handles_utf16_pairs_and_simple_escapes() {
        let mut b = *br#""a\n\u00e9\ud83d\ude00\"z""#;
        let n = b.len();
        let r = unescape_in_place(&mut b, 0, n).unwrap();
        assert_eq!(core::str::from_utf8(&b[r]).unwrap(), "a\n\u{e9}\u{1F600}\"z");
        let mut bad = *br#""\ud83dx""#;
        let n = bad.len();
        let r = unescape_in_place(&mut bad, 0, n).unwrap();
        assert_eq!(core::str::from_utf8(&bad[r]).unwrap(), "\u{FFFD}x");
    }
}
