//! The Indexed DB 3.0 key model (§2.6 Keys) and its order-preserving byte encoding.
//!
//! A key is a number, a date, a string, a binary or an array of keys. Comparison (§2.6 "compare two keys"):
//! keys of different types order number < date < string < binary < array; numbers and dates by value
//! (never NaN — conversion refuses it); strings by UTF-16 code unit; binaries by byte; arrays element-wise,
//! a proper prefix sorting first.
//!
//! Encoding (each self-delimiting, so a concatenation of encodings sorts element-wise):
//!
//! ```text
//! number  0x10 f64-as-sortable-u64 (8 bytes, big-endian)
//! date    0x20 f64-as-sortable-u64
//! string  0x30 escaped(UTF-16BE code units) 0x00 0x01
//! binary  0x40 escaped(bytes) 0x00 0x01
//! array   0x50 element* 0x00
//! escaped: every 0x00 byte becomes 0x00 0xFF
//! ```
//!
//! The sortable u64 flips the sign bit of a non-negative double and every bit of a negative one, so unsigned
//! byte order is numeric order (−0 is normalised to +0: IndexedDB compares them equal). In the escaped forms
//! the terminator `00 01` sorts below any continuation (`00 FF` or a non-zero byte), so a shorter string
//! sorts first; an array's terminator `00` sorts below every element tag.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

const T_NUMBER: u8 = 0x10;
const T_DATE: u8 = 0x20;
const T_STRING: u8 = 0x30;
const T_BINARY: u8 = 0x40;
const T_ARRAY: u8 = 0x50;

/// An IndexedDB key.
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Number(f64),
    /// Milliseconds since the epoch (a `Date`'s time value).
    Date(f64),
    /// UTF-16 code units (a JS string as IndexedDB compares it).
    String(Vec<u16>),
    Binary(Vec<u8>),
    Array(Vec<Key>),
}

impl Key {
    /// A string key from Rust text.
    pub fn string(s: &str) -> Key {
        Key::String(s.encode_utf16().collect())
    }

    /// The string key as Rust text (lone surrogates replaced), `None` for another type.
    pub fn as_text(&self) -> Option<String> {
        match self {
            Key::String(u) => Some(String::from_utf16_lossy(u)),
            _ => None,
        }
    }

    /// The encoding (see the module docs).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Key::Number(n) => {
                out.push(T_NUMBER);
                out.extend_from_slice(&sortable(*n).to_be_bytes());
            }
            Key::Date(n) => {
                out.push(T_DATE);
                out.extend_from_slice(&sortable(*n).to_be_bytes());
            }
            Key::String(units) => {
                out.push(T_STRING);
                for u in units {
                    for b in u.to_be_bytes() {
                        escape(out, b);
                    }
                }
                out.extend_from_slice(&[0, 1]);
            }
            Key::Binary(bytes) => {
                out.push(T_BINARY);
                for &b in bytes {
                    escape(out, b);
                }
                out.extend_from_slice(&[0, 1]);
            }
            Key::Array(items) => {
                out.push(T_ARRAY);
                for k in items {
                    k.encode_into(out);
                }
                out.push(0);
            }
        }
    }

    /// Decodes one key from the front of `b`, answering it and the bytes it used.
    pub fn decode(b: &[u8]) -> Option<(Key, usize)> {
        let tag = *b.first()?;
        match tag {
            T_NUMBER | T_DATE => {
                let raw = b.get(1..9)?;
                let n = unsortable(u64::from_be_bytes([raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7]]));
                Some((if tag == T_NUMBER { Key::Number(n) } else { Key::Date(n) }, 9))
            }
            T_STRING | T_BINARY => {
                let (bytes, used) = unescape(&b[1..])?;
                if tag == T_BINARY {
                    return Some((Key::Binary(bytes), used + 1));
                }
                if bytes.len() % 2 != 0 {
                    return None;
                }
                let units = bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                Some((Key::String(units), used + 1))
            }
            T_ARRAY => {
                let mut i = 1;
                let mut items = Vec::new();
                loop {
                    if *b.get(i)? == 0 {
                        return Some((Key::Array(items), i + 1));
                    }
                    let (k, used) = Key::decode(&b[i..])?;
                    items.push(k);
                    i += used;
                }
            }
            _ => None,
        }
    }

    /// Decodes a buffer holding exactly one key.
    pub fn decode_all(b: &[u8]) -> Option<Key> {
        let (k, used) = Key::decode(b)?;
        (used == b.len()).then_some(k)
    }

    /// IndexedDB §2.6 "compare two keys".
    pub fn compare(&self, other: &Key) -> Ordering {
        self.encode().cmp(&other.encode())
    }
}

fn escape(out: &mut Vec<u8>, b: u8) {
    out.push(b);
    if b == 0 {
        out.push(0xFF);
    }
}

fn unescape(b: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let c = *b.get(i)?;
        if c == 0 {
            match *b.get(i + 1)? {
                0x01 => return Some((out, i + 2)),
                0xFF => out.push(0),
                _ => return None,
            }
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
}

fn sortable(n: f64) -> u64 {
    let n = if n == 0.0 { 0.0 } else { n };
    let bits = n.to_bits();
    if bits >> 63 == 0 { bits | (1 << 63) } else { !bits }
}

fn unsortable(u: u64) -> f64 {
    let bits = if u >> 63 == 1 { u & !(1 << 63) } else { !u };
    f64::from_bits(bits)
}

/// The smallest byte string greater than every encoding that starts with `prefix` (for "all index entries
/// whose index key is K": the range `[enc(K), after_prefix(enc(K)))`).
pub fn after_prefix(prefix: &[u8]) -> Vec<u8> {
    let mut v = prefix.to_vec();
    v.push(0xFF);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn s(t: &str) -> Key {
        Key::string(t)
    }

    #[test]
    fn idb_order_kat() {
        // IndexedDB §2.6: ascending order across and within types.
        let ordered = vec![
            Key::Number(f64::NEG_INFINITY),
            Key::Number(-1.5),
            Key::Number(0.0),
            Key::Number(1.0),
            Key::Number(f64::INFINITY),
            Key::Date(-1.0),
            Key::Date(0.0),
            s(""),
            s("\u{0}"),
            s("A"),
            s("a"),
            s("a\u{0}"),
            s("ab"),
            s("\u{FFFF}"),
            Key::Binary(vec![]),
            Key::Binary(vec![0]),
            Key::Binary(vec![0, 0]),
            Key::Binary(vec![1]),
            Key::Array(vec![]),
            Key::Array(vec![Key::Number(1.0)]),
            Key::Array(vec![Key::Number(1.0), Key::Number(0.0)]),
            Key::Array(vec![s("a")]),
            Key::Array(vec![Key::Array(vec![])]),
        ];
        for w in ordered.windows(2) {
            assert_eq!(w[0].compare(&w[1]), Ordering::Less, "{:?} < {:?}", w[0], w[1]);
        }
        for k in &ordered {
            assert_eq!(Key::decode_all(&k.encode()).as_ref(), Some(k), "round trip {k:?}");
        }
        assert_eq!(Key::Number(-0.0).compare(&Key::Number(0.0)), Ordering::Equal);
    }

    #[test]
    fn utf16_order_not_utf8() {
        // U+FF61 (one unit 0xFF61) sorts after U+1F600 (surrogates 0xD83D 0xDE00) in UTF-16, before it in UTF-8.
        assert_eq!(s("\u{1F600}").compare(&s("\u{FF61}")), Ordering::Less);
    }

    #[test]
    fn concatenation_sorts_by_index_then_primary() {
        let e = |ik: Key, pk: Key| {
            let mut v = ik.encode();
            pk.encode_into(&mut v);
            v
        };
        let a = e(s("x"), Key::Number(9.0));
        let b = e(s("xa"), Key::Number(1.0));
        let c = e(s("x"), Key::Number(10.0));
        assert!(a < c && c < b);
        let p = s("x").encode();
        assert!(a.as_slice() >= p.as_slice() && a < after_prefix(&p) && b > after_prefix(&p));
    }
}
