//! Abstract operations (§7): type conversion, testing and comparison, and the operators.

use super::*;
use crate::bytecode::Op;
use crate::numconv;

pub fn to_int32(n: f64) -> i32 {
    if !n.is_finite() || n == 0.0 {
        return 0;
    }
    if n.abs() < 2147483648.0 {
        return n as i32;
    }
    let t = numconv::libm_floor(n.abs()) * n.signum();
    let m = t % 4294967296.0;
    let m = if m < 0.0 { m + 4294967296.0 } else { m };
    (m as u64 as u32) as i32
}
pub fn to_uint32(n: f64) -> u32 {
    to_int32(n) as u32
}
pub fn to_uint16(n: f64) -> u16 {
    to_int32(n) as u16
}

/// ToIntegerOrInfinity on a number.
pub fn integer_or_infinity(n: f64) -> f64 {
    if n.is_nan() || n == 0.0 {
        return 0.0;
    }
    if !n.is_finite() {
        return n;
    }
    let t = numconv::libm_floor(n.abs());
    if n < 0.0 {
        -t
    } else {
        t
    }
}

pub fn js_mod(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() || a.is_infinite() || b == 0.0 {
        return f64::NAN;
    }
    if b.is_infinite() || a == 0.0 {
        return a;
    }
    let r = a % b;
    if r == 0.0 {
        0.0f64.copysign(a)
    } else {
        r
    }
}

impl Vm {
    pub fn to_boolean(&self, v: &Value) -> bool {
        match v {
            Value::Undefined | Value::Null | Value::Empty => false,
            Value::Bool(b) => *b,
            Value::Number(n) => !(*n == 0.0 || n.is_nan()),
            Value::String(s) => !s.is_empty(),
            Value::Symbol(_) => true,
            Value::BigInt(b) => !b.is_zero(),
            Value::Object(_) => true,
        }
    }

    pub fn type_of(&self, v: &Value) -> JsStr {
        JsStr::from_str(match v {
            Value::Undefined | Value::Empty => "undefined",
            Value::Null => "object",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Symbol(_) => "symbol",
            Value::BigInt(_) => "bigint",
            Value::Object(o) => {
                if self.obj_is_callable(*o) {
                    "function"
                } else {
                    "object"
                }
            }
        })
    }

    /// ToPrimitive (§7.1.1). hint: 0 default, 1 number, 2 string.
    pub fn to_primitive(&mut self, v: &Value, hint: u8) -> JsResult<Value> {
        let o = match v {
            Value::Object(o) => *o,
            _ => return Ok(v.clone()),
        };
        let tp = self.wk.to_primitive.clone();
        let ex = self.get_method(v, &PropertyKey::Sym(tp))?;
        if let Some(f) = ex {
            let h = Value::str(match hint {
                1 => "number",
                2 => "string",
                _ => "default",
            });
            let r = self.call(&f, v, &[h])?;
            if r.is_object() {
                return self.throw_type("Cannot convert object to primitive value");
            }
            return Ok(r);
        }
        self.ordinary_to_primitive(o, hint)
    }

    /// OrdinaryToPrimitive (§7.1.1.1): hint 2 = string, otherwise number.
    pub fn ordinary_to_primitive(&mut self, o: Obj, hint: u8) -> JsResult<Value> {
        let v = &Value::Object(o);
        let order = if hint == 2 { ["toString", "valueOf"] } else { ["valueOf", "toString"] };
        for name in order {
            let m = self.get(o, &PropertyKey::from_str(name))?;
            if self.is_callable(&m) {
                let r = self.call(&m, v, &[])?;
                if !r.is_object() {
                    return Ok(r);
                }
            }
        }
        self.throw_type("Cannot convert object to primitive value")
    }

    pub fn to_number(&mut self, v: &Value) -> JsResult<f64> {
        Ok(match v {
            Value::Number(n) => *n,
            Value::Undefined | Value::Empty => f64::NAN,
            Value::Null => 0.0,
            Value::Bool(b) => *b as u8 as f64,
            Value::String(s) => string_to_number(s),
            Value::Symbol(_) => return self.throw_type("Cannot convert a Symbol value to a number"),
            Value::BigInt(_) => return self.throw_type("Cannot convert a BigInt value to a number"),
            Value::Object(_) => {
                let p = self.to_primitive(v, 1)?;
                return self.to_number(&p);
            }
        })
    }

    pub fn to_numeric(&mut self, v: &Value) -> JsResult<Value> {
        match v {
            Value::Number(_) | Value::BigInt(_) => Ok(v.clone()),
            Value::Object(_) => {
                let p = self.to_primitive(v, 1)?;
                if let Value::BigInt(_) = p {
                    return Ok(p);
                }
                Ok(Value::Number(self.to_number(&p)?))
            }
            _ => Ok(Value::Number(self.to_number(v)?)),
        }
    }

    pub fn to_integer_or_infinity(&mut self, v: &Value) -> JsResult<f64> {
        let n = self.to_number(v)?;
        Ok(integer_or_infinity(n))
    }

    pub fn to_length(&mut self, v: &Value) -> JsResult<f64> {
        let n = self.to_integer_or_infinity(v)?;
        Ok(n.clamp(0.0, 9007199254740991.0))
    }

    /// ToIndex (§7.1.22).
    pub fn to_index(&mut self, v: &Value) -> JsResult<usize> {
        if v.is_undefined() {
            return Ok(0);
        }
        let i = self.to_integer_or_infinity(v)?;
        if !(0.0..=9007199254740991.0).contains(&i) {
            return self.throw_range("Invalid index");
        }
        Ok(i as usize)
    }

    pub fn to_int32(&mut self, v: &Value) -> JsResult<i32> {
        Ok(to_int32(self.to_number(v)?))
    }
    pub fn to_uint32(&mut self, v: &Value) -> JsResult<u32> {
        Ok(to_uint32(self.to_number(v)?))
    }

    pub fn to_string(&mut self, v: &Value) -> JsResult<JsStr> {
        Ok(match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => number_to_jsstr(*n),
            Value::Undefined | Value::Empty => JsStr::from_str("undefined"),
            Value::Null => JsStr::from_str("null"),
            Value::Bool(b) => JsStr::from_str(if *b { "true" } else { "false" }),
            Value::BigInt(b) => JsStr::from_str(&crate::builtins::bigint::to_string_radix(b, 10)),
            Value::Symbol(_) => return self.throw_type("Cannot convert a Symbol value to a string"),
            Value::Object(_) => {
                let p = self.to_primitive(v, 2)?;
                return self.to_string(&p);
            }
        })
    }

    pub fn to_object(&mut self, v: &Value) -> JsResult<Value> {
        let intr = self.intr();
        let (proto, kind) = match v {
            Value::Object(_) => return Ok(v.clone()),
            Value::Undefined | Value::Null | Value::Empty => return self.throw_type("Cannot convert undefined or null to object"),
            Value::Bool(b) => (intr.boolean_proto, Kind::Boolean(*b)),
            Value::Number(n) => (intr.number_proto, Kind::Number(*n)),
            Value::String(s) => (intr.string_proto, Kind::String(s.clone())),
            Value::Symbol(s) => (intr.symbol_proto, Kind::Symbol(s.clone())),
            Value::BigInt(b) => (intr.bigint_proto, Kind::BigInt(b.clone())),
        };
        let mut d = ObjectData::new(Some(proto), kind);
        if let Value::String(s) = v {
            d.props.insert(PropertyKey::from_str("length"), Prop::data(Value::Number(s.len() as f64), 0));
        }
        Ok(Value::Object(self.alloc(d)))
    }

    pub fn to_property_key(&mut self, v: &Value) -> JsResult<PropertyKey> {
        match v {
            Value::String(s) => Ok(PropertyKey::from_js(s.clone())),
            Value::Symbol(s) => Ok(PropertyKey::Sym(s.clone())),
            Value::Number(n) => Ok(PropertyKey::from_f64(*n)),
            Value::Object(_) => {
                let p = self.to_primitive(v, 2)?;
                self.to_property_key(&p)
            }
            _ => Ok(PropertyKey::from_js(self.to_string(v)?)),
        }
    }

    pub fn concat(&mut self, a: &JsStr, b: &JsStr) -> JsResult<JsStr> {
        self.check_string_len(a.len() + b.len())?;
        Ok(a.concat(b))
    }

    /// The + operator (§13.15.3 ApplyStringOrNumericBinaryOperator).
    pub fn add(&mut self, a: &Value, b: &Value) -> JsResult<Value> {
        let pa = self.to_primitive(a, 0)?;
        let pb = self.to_primitive(b, 0)?;
        if matches!(pa, Value::String(_)) || matches!(pb, Value::String(_)) {
            let sa = self.to_string(&pa)?;
            let sb = self.to_string(&pb)?;
            return Ok(Value::String(self.concat(&sa, &sb)?));
        }
        let na = self.to_numeric(&pa)?;
        let nb = self.to_numeric(&pb)?;
        match (na, nb) {
            (Value::Number(x), Value::Number(y)) => Ok(Value::Number(x + y)),
            (Value::BigInt(x), Value::BigInt(y)) => Ok(Value::BigInt(Rc::new(crate::builtins::bigint::bigint_add(&x, &y)))),
            _ => self.throw_type("Cannot mix BigInt and other types, use explicit conversions"),
        }
    }

    pub fn arith(&mut self, op: Op, a: &Value, b: &Value) -> JsResult<Value> {
        let na = self.to_numeric(a)?;
        let nb = self.to_numeric(b)?;
        match (na, nb) {
            (Value::Number(x), Value::Number(y)) => Ok(Value::Number(match op {
                Op::Sub => x - y,
                Op::Mul => x * y,
                Op::Div => x / y,
                Op::Mod => js_mod(x, y),
                Op::Exp => crate::builtins::math::pow(x, y),
                Op::BitAnd => (to_int32(x) & to_int32(y)) as f64,
                Op::BitOr => (to_int32(x) | to_int32(y)) as f64,
                Op::BitXor => (to_int32(x) ^ to_int32(y)) as f64,
                Op::Shl => to_int32(x).wrapping_shl(to_uint32(y) & 31) as f64,
                Op::Shr => (to_int32(x) >> (to_uint32(y) & 31)) as f64,
                Op::UShr => (to_uint32(x) >> (to_uint32(y) & 31)) as f64,
                _ => f64::NAN,
            })),
            (Value::BigInt(x), Value::BigInt(y)) => crate::builtins::bigint::bigint_binop(self, op, &x, &y),
            _ => self.throw_type("Cannot mix BigInt and other types, use explicit conversions"),
        }
    }

    /// IsLooselyEqual (§7.2.14).
    pub fn loose_eq(&mut self, a: &Value, b: &Value) -> JsResult<bool> {
        use Value::*;
        Ok(match (a, b) {
            (Undefined | Null, Undefined | Null) => true,
            (Number(x), Number(y)) => x == y,
            (String(x), String(y)) => x == y,
            (Bool(x), Bool(y)) => x == y,
            (Symbol(x), Symbol(y)) => x == y,
            (Object(x), Object(y)) => x == y,
            (BigInt(x), BigInt(y)) => x == y,
            (Undefined | Null, _) | (_, Undefined | Null) => false,
            (Number(x), String(s)) => *x == string_to_number(s),
            (String(s), Number(y)) => string_to_number(s) == *y,
            (BigInt(x), String(s)) => match crate::builtins::bigint::string_to_bigint(s) {
                Some(y) => **x == y,
                None => false,
            },
            (String(_), BigInt(_)) => return self.loose_eq(b, a),
            (Bool(x), _) => {
                let n = Value::Number(*x as u8 as f64);
                return self.loose_eq(&n, b);
            }
            (_, Bool(y)) => {
                let n = Value::Number(*y as u8 as f64);
                return self.loose_eq(a, &n);
            }
            (Object(_), Number(_) | String(_) | BigInt(_) | Symbol(_)) => {
                let p = self.to_primitive(a, 0)?;
                return self.loose_eq(&p, b);
            }
            (Number(_) | String(_) | BigInt(_) | Symbol(_), Object(_)) => {
                let p = self.to_primitive(b, 0)?;
                return self.loose_eq(a, &p);
            }
            (BigInt(x), Number(y)) | (Number(y), BigInt(x)) => crate::builtins::bigint::compare_with_number(x, *y) == Some(core::cmp::Ordering::Equal),
            _ => false,
        })
    }

    pub fn compare_op(&mut self, op: Op, a: &Value, b: &Value) -> JsResult<bool> {
        // IsLessThan with LeftFirst ordering of ToPrimitive.
        let (pa, pb) = match op {
            Op::Lt | Op::Ge => {
                let pa = self.to_primitive(a, 1)?;
                let pb = self.to_primitive(b, 1)?;
                (pa, pb)
            }
            _ => {
                // Gt / Le: evaluate as b < a / !(b < a) but with left-first ToPrimitive
                let pa = self.to_primitive(a, 1)?;
                let pb = self.to_primitive(b, 1)?;
                (pa, pb)
            }
        };
        let r = match op {
            Op::Lt => self.less_than(&pa, &pb)?,
            Op::Gt => self.less_than(&pb, &pa)?,
            Op::Le => match self.less_than(&pb, &pa)? {
                Some(true) | None => Some(false),
                Some(false) => Some(true),
            },
            _ => match self.less_than(&pa, &pb)? {
                Some(true) | None => Some(false),
                Some(false) => Some(true),
            },
        };
        Ok(r.unwrap_or(false))
    }

    /// IsLessThan on primitives; None means undefined (NaN involved).
    fn less_than(&mut self, a: &Value, b: &Value) -> JsResult<Option<bool>> {
        if let (Value::String(x), Value::String(y)) = (a, b) {
            return Ok(Some(x.units() < y.units()));
        }
        match (a, b) {
            (Value::BigInt(x), Value::String(s)) => {
                return Ok(crate::builtins::bigint::string_to_bigint(s).map(|y| crate::builtins::bigint::cmp(x, &y) == core::cmp::Ordering::Less));
            }
            (Value::String(s), Value::BigInt(y)) => {
                return Ok(crate::builtins::bigint::string_to_bigint(s).map(|x| crate::builtins::bigint::cmp(&x, y) == core::cmp::Ordering::Less));
            }
            _ => {}
        }
        let na = self.to_numeric(a)?;
        let nb = self.to_numeric(b)?;
        Ok(match (&na, &nb) {
            (Value::Number(x), Value::Number(y)) => {
                if x.is_nan() || y.is_nan() {
                    None
                } else {
                    Some(x < y)
                }
            }
            (Value::BigInt(x), Value::BigInt(y)) => Some(crate::builtins::bigint::cmp(x, y) == core::cmp::Ordering::Less),
            (Value::BigInt(x), Value::Number(y)) => crate::builtins::bigint::compare_with_number(x, *y).map(|o| o == core::cmp::Ordering::Less),
            (Value::Number(x), Value::BigInt(y)) => crate::builtins::bigint::compare_with_number(y, *x).map(|o| o == core::cmp::Ordering::Greater),
            _ => None,
        })
    }

    // ------------------------------------------------------------------------------------- property access on any value

    /// GetV (§7.3.3): property lookup on a value (primitives use their prototype).
    pub fn get_v(&mut self, v: &Value, key: &PropertyKey) -> JsResult<Value> {
        match v {
            Value::Object(o) => self.get_with_receiver(*o, key, v),
            Value::String(s) => {
                match key {
                    PropertyKey::Index(i) if (*i as usize) < s.len() => return Ok(Value::String(s.slice(*i as usize, *i as usize + 1))),
                    PropertyKey::Str(k) if k.eq_str("length") => return Ok(Value::Number(s.len() as f64)),
                    _ => {}
                }
                let p = self.intr().string_proto;
                self.get_with_receiver(p, key, v)
            }
            Value::Undefined | Value::Null | Value::Empty => {
                let what = if v.is_null() { "null" } else { "undefined" };
                self.throw_type(&alloc::format!("Cannot read properties of {} (reading '{}')", what, key.to_js_string()))
            }
            _ => {
                let intr = self.intr();
                let p = match v {
                    Value::Bool(_) => intr.boolean_proto,
                    Value::Number(_) => intr.number_proto,
                    Value::Symbol(_) => intr.symbol_proto,
                    Value::BigInt(_) => intr.bigint_proto,
                    _ => unreachable!(),
                };
                self.get_with_receiver(p, key, v)
            }
        }
    }

    pub fn get_elem(&mut self, o: &Value, k: &Value) -> JsResult<Value> {
        // Fast path: dense array index.
        if let (Value::Object(ob), Value::Number(n)) = (o, k) {
            let i = *n as u32;
            if i as f64 == *n && i != u32::MAX {
                if let Kind::Array(a) = &self.heap.get(*ob).kind {
                    if a.dense && (i as usize) < a.elems.len() {
                        let v = &a.elems[i as usize];
                        if !v.is_empty() {
                            return Ok(v.clone());
                        }
                    }
                }
            }
        }
        if o.is_nullish() {
            let what = if o.is_null() { "null" } else { "undefined" };
            return self.throw_type(&alloc::format!("Cannot read properties of {}", what));
        }
        let key = self.to_property_key(k)?;
        self.get_v(o, &key)
    }

    /// PutValue for a property reference (§6.2.5.6).
    pub fn put_value(&mut self, base: &Value, key: PropertyKey, v: Value, strict: bool) -> JsResult<()> {
        match base {
            Value::Object(o) => {
                // Fast path: dense array element.
                if let PropertyKey::Index(i) = key {
                    if let Kind::Array(a) = &mut self.heap.get_mut(*o).kind {
                        if a.dense && (i as usize) < a.elems.len() && !a.elems[i as usize].is_empty() {
                            a.elems[i as usize] = v;
                            return Ok(());
                        }
                    }
                }
                let ok = self.set(*o, key.clone(), v, base)?;
                if !ok && strict {
                    return self.throw_type(&alloc::format!("Cannot assign to read only property '{}' of object", key.to_js_string()));
                }
                Ok(())
            }
            Value::Undefined | Value::Null | Value::Empty => {
                let what = if base.is_null() { "null" } else { "undefined" };
                self.throw_type(&alloc::format!("Cannot set properties of {} (setting '{}')", what, key.to_js_string()))
            }
            _ => {
                let o = self.to_object(base)?.as_object().unwrap();
                let ok = self.set(o, key.clone(), v, base)?;
                if !ok && strict {
                    return self.throw_type(&alloc::format!("Cannot create property '{}' on primitive", key.to_js_string()));
                }
                Ok(())
            }
        }
    }

    pub fn delete_value(&mut self, base: &Value, key: PropertyKey, strict: bool) -> JsResult<bool> {
        let o = self.to_object(base)?.as_object().unwrap();
        let ok = self.delete(o, &key)?;
        if !ok && strict {
            return self.throw_type(&alloc::format!("Cannot delete property '{}'", key.to_js_string()));
        }
        Ok(ok)
    }
}

pub fn number_to_jsstr(n: f64) -> JsStr {
    if n >= 0.0 && n < 1e9 && (n as u32) as f64 == n {
        let mut s = alloc::string::String::new();
        numconv::push_int(&mut s, n as u64);
        return JsStr::from_str(&s);
    }
    JsStr::from_str(&numconv::f64_to_js_string(n))
}

/// StringToNumber (§7.1.4.1.1).
pub fn string_to_number(s: &JsStr) -> f64 {
    let u = s.units();
    let mut a = 0;
    let mut b = u.len();
    while a < b && crate::unicode::is_str_whitespace(u[a] as u32) {
        a += 1;
    }
    while b > a && crate::unicode::is_str_whitespace(u[b - 1] as u32) {
        b -= 1;
    }
    let t = &u[a..b];
    if t.is_empty() {
        return 0.0;
    }
    if t.iter().any(|&c| c > 127) {
        return f64::NAN;
    }
    let bytes: alloc::vec::Vec<u8> = t.iter().map(|&c| c as u8).collect();
    if bytes.len() > 2 && bytes[0] == b'0' {
        let radix = match bytes[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            let digits = &bytes[2..];
            if digits.iter().all(|&c| numconv::digit_val(c as u32).map(|d| d < radix).unwrap_or(false)) {
                return numconv::parse_radix_int(digits, radix);
            }
            return f64::NAN;
        }
    }
    match numconv::parse_decimal_prefix(&bytes) {
        Some((v, used)) if used == bytes.len() => {
            // Reject forms StrDecimalLiteral does not allow: "infinity", hex already handled.
            v
        }
        _ => f64::NAN,
    }
}
