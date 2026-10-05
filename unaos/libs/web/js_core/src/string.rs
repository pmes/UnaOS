//! JavaScript strings: immutable sequences of UTF-16 code units, reference counted.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::hash::{Hash, Hasher};

#[derive(Clone)]
pub struct JsStr(Rc<[u16]>);

impl JsStr {
    pub fn empty() -> JsStr {
        JsStr(Rc::from(&[][..]))
    }
    pub fn from_units(v: Vec<u16>) -> JsStr {
        JsStr(Rc::from(v))
    }
    pub fn from_slice(v: &[u16]) -> JsStr {
        JsStr(Rc::from(v))
    }
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> JsStr {
        let v: Vec<u16> = s.encode_utf16().collect();
        JsStr(Rc::from(v))
    }
    pub fn units(&self) -> &[u16] {
        &self.0
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn ptr_eq(a: &JsStr, b: &JsStr) -> bool {
        Rc::ptr_eq(&a.0, &b.0)
    }
    pub fn concat(&self, o: &JsStr) -> JsStr {
        if self.is_empty() {
            return o.clone();
        }
        if o.is_empty() {
            return self.clone();
        }
        let mut v = Vec::with_capacity(self.len() + o.len());
        v.extend_from_slice(&self.0);
        v.extend_from_slice(&o.0);
        JsStr::from_units(v)
    }
    pub fn slice(&self, a: usize, b: usize) -> JsStr {
        if a == 0 && b == self.len() {
            return self.clone();
        }
        JsStr::from_slice(&self.0[a..b])
    }
    /// Lossy conversion (lone surrogates become U+FFFD).
    pub fn to_rust(&self) -> String {
        char::decode_utf16(self.0.iter().copied()).map(|r| r.unwrap_or('\u{FFFD}')).collect()
    }
    pub fn eq_str(&self, s: &str) -> bool {
        let mut it = s.encode_utf16();
        for &u in self.0.iter() {
            if it.next() != Some(u) {
                return false;
            }
        }
        it.next().is_none()
    }
    /// Code point at index (surrogate pairs combined), with its length in code units.
    pub fn code_point_at(&self, i: usize) -> (u32, usize) {
        code_point_at(&self.0, i)
    }
    pub fn hash32(&self) -> u32 {
        hash_units(&self.0)
    }
    /// Canonical numeric index: Some(n) if this string is the canonical form of an array index (< 2^32 - 1).
    pub fn as_array_index(&self) -> Option<u32> {
        units_as_array_index(&self.0)
    }
}

pub fn units_as_array_index(u: &[u16]) -> Option<u32> {
    if u.is_empty() || u.len() > 10 {
        return None;
    }
    if u[0] == b'0' as u16 {
        return if u.len() == 1 { Some(0) } else { None };
    }
    let mut n: u64 = 0;
    for &c in u {
        if !(0x30..=0x39).contains(&c) {
            return None;
        }
        n = n * 10 + (c - 0x30) as u64;
    }
    if n < 4294967295 {
        Some(n as u32)
    } else {
        None
    }
}

pub fn code_point_at(u: &[u16], i: usize) -> (u32, usize) {
    let c = u[i];
    if (0xD800..0xDC00).contains(&c) && i + 1 < u.len() {
        let d = u[i + 1];
        if (0xDC00..0xE000).contains(&d) {
            return (0x10000 + (((c as u32) - 0xD800) << 10) + (d as u32 - 0xDC00), 2);
        }
    }
    (c as u32, 1)
}

pub fn push_code_point(v: &mut Vec<u16>, cp: u32) {
    if cp < 0x10000 {
        v.push(cp as u16);
    } else {
        let c = cp - 0x10000;
        v.push(0xD800 + (c >> 10) as u16);
        v.push(0xDC00 + (c & 0x3FF) as u16);
    }
}

pub fn hash_units(u: &[u16]) -> u32 {
    // FNV-1a over code units.
    let mut h: u32 = 0x811c9dc5;
    for &c in u {
        h ^= c as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

impl PartialEq for JsStr {
    fn eq(&self, o: &JsStr) -> bool {
        Rc::ptr_eq(&self.0, &o.0) || self.0[..] == o.0[..]
    }
}
impl Eq for JsStr {}
impl PartialOrd for JsStr {
    fn partial_cmp(&self, o: &JsStr) -> Option<core::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for JsStr {
    fn cmp(&self, o: &JsStr) -> core::cmp::Ordering {
        self.0[..].cmp(&o.0[..])
    }
}
impl Hash for JsStr {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.0[..].hash(h)
    }
}
impl fmt::Debug for JsStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.to_rust())
    }
}
impl fmt::Display for JsStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_rust())
    }
}
impl From<&str> for JsStr {
    fn from(s: &str) -> JsStr {
        JsStr::from_str(s)
    }
}
