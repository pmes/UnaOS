//! JavaScript strings: immutable sequences of UTF-16 code units, reference counted.
//!
//! AETHERPERF (SR118): a `JsStr` is one thin `Rc` (8 bytes, so `Value` is 16) to a node that is either FLAT —
//! up to `INLINE` units stored in the node itself, longer ones in one boxed slice — or a ROPE: `concat` of two
//! strings whose joint length reaches `ROPE_MIN` allocates a node holding the two halves and copies nothing. A
//! rope is flattened, iteratively (an explicit stack, never recursion: `s += x` in a loop builds a chain as deep
//! as the loop), the first time anything reads its units (index, compare, hash, slice, print); the halves are then
//! released. Dropping a deep rope is iterative too (`Drop for Node`). The FNV hash a property-map lookup takes is
//! cached in the node after the first computation. Every API kept its old signature and meaning.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, OnceCell, RefCell};
use core::fmt;
use core::hash::{Hash, Hasher};

/// Strings of at most this many code units live inside the node (one allocation, no slice box).
const INLINE: usize = 8;
/// A concatenation at least this long becomes a rope node instead of a copy.
const ROPE_MIN: usize = 32;

struct Node {
    len: usize,
    /// Cached `hash_units` of the units (None until first asked).
    hash: Cell<Option<u32>>,
    small: [u16; INLINE],
    /// The flat units of a string longer than `INLINE` (set at creation, or when a rope is flattened).
    heap: OnceCell<alloc::boxed::Box<[u16]>>,
    /// The two halves of an unflattened rope.
    rope: RefCell<Option<(JsStr, JsStr)>>,
}

impl Drop for Node {
    fn drop(&mut self) {
        // Unlink rope children iteratively so a deep left-leaning chain does not recurse once per level.
        let mut st: Vec<JsStr> = Vec::new();
        if let Some((a, b)) = self.rope.get_mut().take() {
            st.push(a);
            st.push(b);
        }
        while let Some(s) = st.pop() {
            if let Ok(mut n) = Rc::try_unwrap(s.0) {
                if let Some((a, b)) = n.rope.get_mut().take() {
                    st.push(a);
                    st.push(b);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct JsStr(Rc<Node>);

impl JsStr {
    fn flat(v: &[u16]) -> JsStr {
        let len = v.len();
        let mut small = [0u16; INLINE];
        let heap = OnceCell::new();
        if len <= INLINE {
            small[..len].copy_from_slice(v);
        } else {
            let _ = heap.set(alloc::boxed::Box::from(v));
        }
        JsStr(Rc::new(Node { len, hash: Cell::new(None), small, heap, rope: RefCell::new(None) }))
    }
    pub fn empty() -> JsStr {
        JsStr::flat(&[])
    }
    pub fn from_units(v: Vec<u16>) -> JsStr {
        let len = v.len();
        if len <= INLINE {
            return JsStr::flat(&v);
        }
        JsStr(Rc::new(Node { len, hash: Cell::new(None), small: [0; INLINE], heap: OnceCell::from(v.into_boxed_slice()), rope: RefCell::new(None) }))
    }
    pub fn from_slice(v: &[u16]) -> JsStr {
        JsStr::flat(v)
    }
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> JsStr {
        let v: Vec<u16> = s.encode_utf16().collect();
        JsStr::from_units(v)
    }
    /// The units if this node is already flat (no flattening).
    fn flat_units(&self) -> Option<&[u16]> {
        let n = &*self.0;
        if n.len <= INLINE {
            return Some(&n.small[..n.len]);
        }
        n.heap.get().map(|b| &b[..])
    }
    /// The code units; a rope is flattened here, once.
    pub fn units(&self) -> &[u16] {
        if let Some(u) = self.flat_units() {
            return u;
        }
        let n = &*self.0;
        let mut out: Vec<u16> = Vec::with_capacity(n.len);
        let mut st: Vec<JsStr> = Vec::new();
        if let Some((a, b)) = n.rope.borrow().as_ref() {
            st.push(b.clone());
            st.push(a.clone());
        }
        while let Some(s) = st.pop() {
            if let Some(u) = s.flat_units() {
                out.extend_from_slice(u);
                continue;
            }
            if let Some((a, b)) = s.0.rope.borrow().as_ref() {
                st.push(b.clone());
                st.push(a.clone());
            }
        }
        drop(st);
        let _ = n.heap.set(out.into_boxed_slice());
        // The halves are no longer needed.
        let halves = n.rope.borrow_mut().take();
        drop(halves);
        n.heap.get().map(|b| &b[..]).unwrap_or(&[])
    }
    pub fn len(&self) -> usize {
        self.0.len
    }
    pub fn is_empty(&self) -> bool {
        self.0.len == 0
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
        let len = self.len() + o.len();
        if len >= ROPE_MIN {
            // Rope: no copy until something reads the units.
            return JsStr(Rc::new(Node { len, hash: Cell::new(None), small: [0; INLINE], heap: OnceCell::new(), rope: RefCell::new(Some((self.clone(), o.clone()))) }));
        }
        let mut v = Vec::with_capacity(len);
        v.extend_from_slice(self.units());
        v.extend_from_slice(o.units());
        JsStr::from_units(v)
    }
    pub fn slice(&self, a: usize, b: usize) -> JsStr {
        if a == 0 && b == self.len() {
            return self.clone();
        }
        JsStr::from_slice(&self.units()[a..b])
    }
    /// Lossy conversion (lone surrogates become U+FFFD).
    pub fn to_rust(&self) -> String {
        char::decode_utf16(self.units().iter().copied()).map(|r| r.unwrap_or('\u{FFFD}')).collect()
    }
    pub fn eq_str(&self, s: &str) -> bool {
        let mut it = s.encode_utf16();
        for &u in self.units().iter() {
            if it.next() != Some(u) {
                return false;
            }
        }
        it.next().is_none()
    }
    /// Code point at index (surrogate pairs combined), with its length in code units.
    pub fn code_point_at(&self, i: usize) -> (u32, usize) {
        code_point_at(self.units(), i)
    }
    pub fn hash32(&self) -> u32 {
        if let Some(h) = self.0.hash.get() {
            return h;
        }
        let h = hash_units(self.units());
        self.0.hash.set(Some(h));
        h
    }
    /// Canonical numeric index: Some(n) if this string is the canonical form of an array index (< 2^32 - 1).
    pub fn as_array_index(&self) -> Option<u32> {
        if self.0.len > 10 || self.0.len == 0 {
            return None;
        }
        units_as_array_index(self.units())
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
        Rc::ptr_eq(&self.0, &o.0) || (self.len() == o.len() && self.units() == o.units())
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
        self.units().cmp(o.units())
    }
}
impl Hash for JsStr {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.units().hash(h)
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
