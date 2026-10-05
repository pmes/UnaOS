//! BigInt (§21.2) and the BigInt arithmetic of §6.1.6.2.

use super::*;
use crate::bignum::BigUint;
use crate::bytecode::Op;
use core::cmp::Ordering;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let proto = vm.new_object(Some(op));
    let c = ctor(vm, "BigInt", 1, bigint_ctor, proto);
    method(vm, c, "asIntN", 2, as_int_n);
    method(vm, c, "asUintN", 2, as_uint_n);
    method(vm, proto, "toLocaleString", 0, to_locale_string);
    method(vm, proto, "toString", 0, to_string_m);
    method(vm, proto, "valueOf", 0, value_of);
    to_str_tag(vm, proto, "BigInt");
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.bigint_proto = proto;
    vm.realms[r].intrinsics.bigint_ctor = c;
    global(vm, "BigInt", Value::Object(c));
}

pub fn to_string_radix(b: &BigInt, radix: u32) -> alloc::string::String {
    let digits = b.mag.to_string_radix(radix);
    let mut s = alloc::string::String::new();
    if b.neg {
        s.push('-');
    }
    s.push_str(core::str::from_utf8(&digits).unwrap());
    s
}

pub fn to_f64(b: &BigInt) -> f64 {
    let v = b.mag.to_f64();
    if b.neg {
        -v
    } else {
        v
    }
}

/// StringToBigInt (§7.1.14): None for a syntax error.
pub fn string_to_bigint(s: &JsStr) -> Option<BigInt> {
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
        return Some(BigInt::zero());
    }
    if t.iter().any(|&c| c > 127) {
        return None;
    }
    let bytes: Vec<u8> = t.iter().map(|&c| c as u8).collect();
    if bytes.len() > 2 && bytes[0] == b'0' {
        let radix = match bytes[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            let d = &bytes[2..];
            if d.iter().all(|&c| crate::numconv::digit_val(c as u32).map(|x| x < radix).unwrap_or(false)) {
                return Some(BigInt::from_mag(false, BigUint::from_digits(d, radix)));
            }
            return None;
        }
    }
    let (neg, d) = match bytes[0] {
        b'-' => (true, &bytes[1..]),
        b'+' => (false, &bytes[1..]),
        _ => (false, &bytes[..]),
    };
    if d.is_empty() || !d.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(BigInt::from_mag(neg, BigUint::from_digits(d, 10)))
}

pub fn cmp(a: &BigInt, b: &BigInt) -> Ordering {
    match (a.neg, b.neg) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (false, false) => a.mag.cmp(&b.mag),
        (true, true) => b.mag.cmp(&a.mag),
    }
}

/// Compare a BigInt with a Number exactly; None if the number is NaN.
pub fn compare_with_number(a: &BigInt, n: f64) -> Option<Ordering> {
    if n.is_nan() {
        return None;
    }
    if n == f64::INFINITY {
        return Some(Ordering::Less);
    }
    if n == f64::NEG_INFINITY {
        return Some(Ordering::Greater);
    }
    // Compare a with floor(n), then account for the fraction.
    let fl = crate::numconv::libm_floor(n);
    let nb = from_integral_f64(fl);
    match cmp(a, &nb) {
        Ordering::Equal => {
            if fl == n {
                Some(Ordering::Equal)
            } else {
                Some(Ordering::Less)
            }
        }
        o => Some(o),
    }
}

/// An integral finite double as a BigInt.
pub fn from_integral_f64(n: f64) -> BigInt {
    if n == 0.0 {
        return BigInt::zero();
    }
    let neg = n < 0.0;
    let a = n.abs();
    let bits = a.to_bits();
    let e = ((bits >> 52) & 0x7FF) as i64 - 1075;
    let m = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let mag = if e >= 0 { BigUint::from_u64(m).shl(e as u64) } else { BigUint::from_u64(m).shr((-e) as u64) };
    BigInt::from_mag(neg, mag)
}

pub fn bigint_add(a: &BigInt, b: &BigInt) -> BigInt {
    if a.neg == b.neg {
        return BigInt::from_mag(a.neg, a.mag.add(&b.mag));
    }
    match a.mag.cmp(&b.mag) {
        Ordering::Equal => BigInt::zero(),
        Ordering::Greater => BigInt::from_mag(a.neg, a.mag.sub(&b.mag)),
        Ordering::Less => BigInt::from_mag(b.neg, b.mag.sub(&a.mag)),
    }
}

pub fn bigint_neg(a: &BigInt) -> BigInt {
    BigInt::from_mag(!a.neg, a.mag.clone())
}

pub fn bigint_not(a: &BigInt) -> BigInt {
    // ~a = -a - 1
    bigint_add(&bigint_neg(a), &BigInt::from_i64(-1))
}

/// Two's complement limbs (little endian) of `a` with `n` limbs.
fn to_twos(a: &BigInt, n: usize) -> Vec<u32> {
    let mut v = a.mag.limbs.clone();
    v.resize(n, 0);
    if a.neg {
        let mut carry = 1u64;
        for l in v.iter_mut() {
            let x = (!*l) as u64 + carry;
            *l = x as u32;
            carry = x >> 32;
        }
    }
    v
}

fn from_twos(mut v: Vec<u32>) -> BigInt {
    let neg = v.last().map(|&l| l & 0x8000_0000 != 0).unwrap_or(false);
    if neg {
        let mut carry = 1u64;
        for l in v.iter_mut() {
            let x = (!*l) as u64 + carry;
            *l = x as u32;
            carry = x >> 32;
        }
    }
    let mut m = BigUint { limbs: v };
    m.trim();
    BigInt::from_mag(neg, m)
}

fn bitop(a: &BigInt, b: &BigInt, f: impl Fn(u32, u32) -> u32) -> BigInt {
    let n = a.mag.limbs.len().max(b.mag.limbs.len()) + 1;
    let x = to_twos(a, n);
    let y = to_twos(b, n);
    from_twos(x.iter().zip(y.iter()).map(|(p, q)| f(*p, *q)).collect())
}

fn shift_left(a: &BigInt, n: i64) -> BigInt {
    if n >= 0 {
        BigInt::from_mag(a.neg, a.mag.shl(n as u64))
    } else {
        let s = (-n) as u64;
        if !a.neg {
            BigInt::from_mag(false, a.mag.shr(s))
        } else {
            // floor division for negatives
            let q = a.mag.shr(s);
            let exact = !a.mag.low_bits_nonzero(s);
            let q = if exact { q } else { q.add(&BigUint::from_u64(1)) };
            BigInt::from_mag(true, q)
        }
    }
}

pub fn bigint_binop(vm: &mut Vm, op: Op, a: &BigInt, b: &BigInt) -> JsResult<Value> {
    let r = match op {
        Op::Sub => bigint_add(a, &bigint_neg(b)),
        Op::Mul => BigInt::from_mag(a.neg != b.neg, a.mag.mul(&b.mag)),
        Op::Div => {
            if b.is_zero() {
                return vm.throw_range("Division by zero");
            }
            let (q, _) = a.mag.divrem(&b.mag);
            BigInt::from_mag(a.neg != b.neg, q)
        }
        Op::Mod => {
            if b.is_zero() {
                return vm.throw_range("Division by zero");
            }
            let (_, r) = a.mag.divrem(&b.mag);
            BigInt::from_mag(a.neg, r)
        }
        Op::Exp => {
            if b.neg {
                return vm.throw_range("Exponent must be non-negative");
            }
            if b.is_zero() {
                return Ok(Value::BigInt(Rc::new(BigInt::from_i64(1))));
            }
            if a.mag.is_zero() || (a.mag.limbs.len() == 1 && a.mag.limbs[0] == 1) {
                let odd = b.mag.bit(0);
                return Ok(Value::BigInt(Rc::new(BigInt::from_mag(a.neg && odd, a.mag.clone()))));
            }
            let e = match b.mag.to_u64() {
                Some(e) if e <= 1 << 24 => e,
                _ => return vm.throw_range("Maximum BigInt size exceeded"),
            };
            if a.mag.bits() * e > 1 << 30 {
                return vm.throw_range("Maximum BigInt size exceeded");
            }
            BigInt::from_mag(a.neg && e & 1 == 1, a.mag.pow(e))
        }
        Op::BitAnd => bitop(a, b, |x, y| x & y),
        Op::BitOr => bitop(a, b, |x, y| x | y),
        Op::BitXor => bitop(a, b, |x, y| x ^ y),
        Op::Shl | Op::Shr => {
            let n = match b.mag.to_u64() {
                Some(n) if n < 1 << 32 => n as i64,
                _ => {
                    // Huge shifts.
                    let left = (op == Op::Shl) != b.neg;
                    if left && !a.is_zero() {
                        return vm.throw_range("Maximum BigInt size exceeded");
                    }
                    return Ok(Value::BigInt(Rc::new(if a.neg { BigInt::from_i64(-1) } else { BigInt::zero() })));
                }
            };
            let n = if b.neg { -n } else { n };
            let n = if op == Op::Shr { -n } else { n };
            if n > (1 << 30) {
                return vm.throw_range("Maximum BigInt size exceeded");
            }
            shift_left(a, n)
        }
        Op::UShr => return vm.throw_type("BigInts have no unsigned right shift, use >> instead"),
        _ => return vm.throw_type("invalid BigInt operation"),
    };
    Ok(Value::BigInt(Rc::new(r)))
}

impl Vm {
    /// ToBigInt (§7.1.13)
    pub fn to_bigint(&mut self, v: &Value) -> JsResult<Rc<BigInt>> {
        let p = self.to_primitive(v, 1)?;
        match p {
            Value::BigInt(b) => Ok(b),
            Value::Bool(b) => Ok(Rc::new(BigInt::from_i64(b as i64))),
            Value::String(s) => match string_to_bigint(&s) {
                Some(b) => Ok(Rc::new(b)),
                None => self.throw_syntax(&alloc::format!("Cannot convert {} to a BigInt", s)),
            },
            Value::Number(_) => self.throw_type("Cannot convert a Number to a BigInt"),
            Value::Symbol(_) => self.throw_type("Cannot convert a Symbol to a BigInt"),
            _ => self.throw_type("Cannot convert undefined or null to a BigInt"),
        }
    }
}

fn bigint_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if !ctx.new_target.is_undefined() {
        return vm.throw_type("BigInt is not a constructor");
    }
    let v = vm.arg(ctx, 0);
    let p = vm.to_primitive(&v, 1)?;
    if let Value::Number(n) = p {
        if !n.is_finite() || crate::numconv::libm_floor(n) != n {
            return vm.throw_range("The number cannot be converted to a BigInt because it is not an integer");
        }
        return Ok(Value::BigInt(Rc::new(from_integral_f64(n))));
    }
    Ok(Value::BigInt(vm.to_bigint(&p)?))
}

fn as_n(vm: &mut Vm, ctx: &CallCtx, signed: bool) -> JsResult<Value> {
    let ba = vm.arg(ctx, 0);
    let bits = vm.to_index(&ba)?;
    let xa = vm.arg(ctx, 1);
    let x = vm.to_bigint(&xa)?;
    if bits == 0 {
        return Ok(Value::BigInt(Rc::new(BigInt::zero())));
    }
    // mod = x mod 2^bits (two's complement truncation)
    let limbs = bits.div_ceil(32) + 1;
    if limbs > (1 << 25) {
        // Only representable if x is small; compute via magnitude comparisons.
        if !x.neg && x.mag.bits() < bits as u64 {
            return Ok(Value::BigInt(x));
        }
        if signed && x.mag.bits() < bits as u64 {
            return Ok(Value::BigInt(x));
        }
        return vm.throw_range("Maximum BigInt size exceeded");
    }
    let n = limbs.max(x.mag.limbs.len() + 1);
    let mut v = to_twos(&x, n);
    // keep low `bits` bits
    let full = bits / 32;
    let rem = bits % 32;
    for (i, l) in v.iter_mut().enumerate() {
        if i > full || (i == full && rem == 0) {
            *l = 0;
        } else if i == full {
            *l &= (1u32 << rem) - 1;
        }
    }
    let mut m = BigUint { limbs: v };
    m.trim();
    let mut r = BigInt::from_mag(false, m);
    if signed && r.mag.bit(bits as u64 - 1) {
        // subtract 2^bits
        let two = BigUint::from_u64(1).shl(bits as u64);
        r = BigInt::from_mag(true, two.sub(&r.mag));
    }
    Ok(Value::BigInt(Rc::new(r)))
}
fn as_int_n(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    as_n(vm, ctx, true)
}
fn as_uint_n(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    as_n(vm, ctx, false)
}

fn this_bigint(vm: &mut Vm, v: &Value) -> JsResult<Rc<BigInt>> {
    match v {
        Value::BigInt(b) => Ok(b.clone()),
        Value::Object(o) => match &vm.heap.get(*o).kind {
            Kind::BigInt(b) => Ok(b.clone()),
            _ => vm.throw_type("BigInt.prototype method called on incompatible receiver"),
        },
        _ => vm.throw_type("BigInt.prototype method called on incompatible receiver"),
    }
}

fn to_string_m(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = this_bigint(vm, &ctx.this)?;
    let r = vm.arg(ctx, 0);
    let radix = if r.is_undefined() { 10.0 } else { vm.to_integer_or_infinity(&r)? };
    if !(2.0..=36.0).contains(&radix) {
        return vm.throw_range("toString() radix must be between 2 and 36");
    }
    Ok(Value::String(JsStr::from_str(&to_string_radix(&b, radix as u32))))
}
fn to_locale_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = this_bigint(vm, &ctx.this)?;
    Ok(Value::String(JsStr::from_str(&to_string_radix(&b, 10))))
}
fn value_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::BigInt(this_bigint(vm, &ctx.this)?))
}
