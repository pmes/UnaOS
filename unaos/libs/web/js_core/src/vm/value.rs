//! ECMAScript language values (§6.1), property keys, symbols and BigInts.

use crate::bignum::BigUint;
use crate::string::JsStr;
use alloc::rc::Rc;
use core::cell::Cell;
use core::hash::{Hash, Hasher};

/// A handle to a heap cell (object, environment record, …). Index into `Heap::cells`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Obj(pub u32);

pub struct SymData {
    pub desc: Option<JsStr>,
    /// A Private Name (§6.2.12), never visible to user code as a Symbol.
    pub private: bool,
    /// Registered via Symbol.for (cannot be held weakly).
    pub registered: Cell<bool>,
}

#[derive(Clone)]
pub struct Sym(pub Rc<SymData>);

impl Sym {
    pub fn new(desc: Option<JsStr>) -> Sym {
        Sym(Rc::new(SymData { desc, private: false, registered: Cell::new(false) }))
    }
    pub fn private(desc: JsStr) -> Sym {
        Sym(Rc::new(SymData { desc: Some(desc), private: true, registered: Cell::new(false) }))
    }
    pub fn id(&self) -> usize {
        Rc::as_ptr(&self.0) as *const u8 as usize
    }
    pub fn desc(&self) -> Option<&JsStr> {
        self.0.desc.as_ref()
    }
}
impl PartialEq for Sym {
    fn eq(&self, o: &Sym) -> bool {
        Rc::ptr_eq(&self.0, &o.0)
    }
}
impl Eq for Sym {}
impl Hash for Sym {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.id().hash(h)
    }
}
impl core::fmt::Debug for Sym {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Symbol({:?})", self.0.desc)
    }
}

/// A BigInt: sign and magnitude (zero is never negative).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BigInt {
    pub neg: bool,
    pub mag: BigUint,
}

impl BigInt {
    pub fn zero() -> BigInt {
        BigInt { neg: false, mag: BigUint::zero() }
    }
    pub fn from_i64(v: i64) -> BigInt {
        BigInt { neg: v < 0, mag: BigUint::from_u64(v.unsigned_abs()) }
    }
    pub fn from_mag(neg: bool, mag: BigUint) -> BigInt {
        let neg = neg && !mag.is_zero();
        BigInt { neg, mag }
    }
    pub fn is_zero(&self) -> bool {
        self.mag.is_zero()
    }
}

#[derive(Clone, Debug)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(JsStr),
    Symbol(Sym),
    BigInt(Rc<BigInt>),
    Object(Obj),
    /// Internal: an array hole / an uninitialised binding (TDZ). Never observable.
    Empty,
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::String(JsStr::from_str(s))
    }
    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    pub fn is_nullish(&self) -> bool {
        matches!(self, Value::Undefined | Value::Null)
    }
    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }
    pub fn as_object(&self) -> Option<Obj> {
        match self {
            Value::Object(o) => Some(*o),
            _ => None,
        }
    }
    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }
    /// SameValue (§7.2.10).
    pub fn same_value(&self, o: &Value) -> bool {
        match (self, o) {
            (Value::Number(a), Value::Number(b)) => {
                if a.is_nan() && b.is_nan() {
                    return true;
                }
                a.to_bits() == b.to_bits()
            }
            _ => self.strict_eq(o),
        }
    }
    /// SameValueZero (§7.2.11).
    pub fn same_value_zero(&self, o: &Value) -> bool {
        match (self, o) {
            (Value::Number(a), Value::Number(b)) => (a.is_nan() && b.is_nan()) || a == b,
            _ => self.strict_eq(o),
        }
    }
    /// IsStrictlyEqual (§7.2.15).
    pub fn strict_eq(&self, o: &Value) -> bool {
        match (self, o) {
            (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Symbol(a), Value::Symbol(b)) => a == b,
            (Value::BigInt(a), Value::BigInt(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => a == b,
            (Value::Empty, Value::Empty) => true,
            _ => false,
        }
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Value {
        Value::Number(v)
    }
}
impl From<JsStr> for Value {
    fn from(s: JsStr) -> Value {
        Value::String(s)
    }
}
impl From<Obj> for Value {
    fn from(o: Obj) -> Value {
        Value::Object(o)
    }
}

/// A property key (§6.1.7): canonical array indices are kept as integers.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PropertyKey {
    Index(u32),
    Str(JsStr),
    Sym(Sym),
}

impl PropertyKey {
    pub fn from_str(s: &str) -> PropertyKey {
        PropertyKey::from_js(JsStr::from_str(s))
    }
    pub fn from_js(s: JsStr) -> PropertyKey {
        match s.as_array_index() {
            Some(i) => PropertyKey::Index(i),
            None => PropertyKey::Str(s),
        }
    }
    pub fn from_f64(v: f64) -> PropertyKey {
        if v >= 0.0 && v < 4294967295.0 && (v as u32) as f64 == v {
            PropertyKey::Index(v as u32)
        } else {
            PropertyKey::Str(JsStr::from_str(&crate::numconv::f64_to_js_string(v)))
        }
    }
    pub fn hash32(&self) -> u32 {
        match self {
            PropertyKey::Index(i) => i.wrapping_mul(0x9E3779B1),
            PropertyKey::Str(s) => s.hash32(),
            PropertyKey::Sym(s) => (s.id() as u32).wrapping_mul(0x85EBCA77) ^ ((s.id() >> 32) as u32),
        }
    }
    pub fn is_symbol(&self) -> bool {
        matches!(self, PropertyKey::Sym(_))
    }
    pub fn to_value(&self) -> Value {
        match self {
            PropertyKey::Index(i) => Value::String(JsStr::from_str(&crate::numconv::f64_to_js_string(*i as f64))),
            PropertyKey::Str(s) => Value::String(s.clone()),
            PropertyKey::Sym(s) => Value::Symbol(s.clone()),
        }
    }
    /// The key as a string (symbols use their descriptive form, for function names / messages).
    pub fn to_js_string(&self) -> JsStr {
        match self {
            PropertyKey::Index(i) => JsStr::from_str(&crate::numconv::f64_to_js_string(*i as f64)),
            PropertyKey::Str(s) => s.clone(),
            PropertyKey::Sym(s) => match s.desc() {
                Some(d) => JsStr::from_str("[").concat(d).concat(&JsStr::from_str("]")),
                None => JsStr::empty(),
            },
        }
    }
    pub fn eq_str(&self, s: &str) -> bool {
        match self {
            PropertyKey::Str(x) => x.eq_str(s),
            PropertyKey::Index(i) => crate::numconv::f64_to_js_string(*i as f64) == s,
            _ => false,
        }
    }
}

impl From<&str> for PropertyKey {
    fn from(s: &str) -> PropertyKey {
        PropertyKey::from_str(s)
    }
}
impl From<u32> for PropertyKey {
    fn from(i: u32) -> PropertyKey {
        if i == u32::MAX {
            PropertyKey::Str(JsStr::from_str("4294967295"))
        } else {
            PropertyKey::Index(i)
        }
    }
}
