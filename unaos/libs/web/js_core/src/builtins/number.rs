//! Number (§21.1).

use super::*;
use crate::numconv;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let proto = vm.alloc(ObjectData::new(Some(op), Kind::Number(0.0)));
    let c = ctor(vm, "Number", 1, number_ctor, proto);
    for (n, v) in [
        ("EPSILON", f64::EPSILON),
        ("MAX_SAFE_INTEGER", 9007199254740991.0),
        ("MAX_VALUE", f64::MAX),
        ("MIN_SAFE_INTEGER", -9007199254740991.0),
        ("MIN_VALUE", 5e-324),
        ("NaN", f64::NAN),
        ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
        ("POSITIVE_INFINITY", f64::INFINITY),
    ] {
        value(vm, c, n, Value::Number(v), 0);
    }
    method(vm, c, "isFinite", 1, is_finite);
    method(vm, c, "isInteger", 1, is_integer);
    method(vm, c, "isNaN", 1, is_nan);
    method(vm, c, "isSafeInteger", 1, is_safe_integer);
    method(vm, proto, "toExponential", 1, to_exponential);
    method(vm, proto, "toFixed", 1, to_fixed);
    method(vm, proto, "toLocaleString", 0, to_locale_string);
    method(vm, proto, "toPrecision", 1, to_precision);
    method(vm, proto, "toString", 1, to_string);
    method(vm, proto, "valueOf", 0, value_of);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.number_proto = proto;
    vm.realms[r].intrinsics.number_ctor = c;
    global(vm, "Number", Value::Object(c));
}

fn number_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let n = if ctx.argc == 0 {
        0.0
    } else {
        let a = vm.arg(ctx, 0);
        match vm.to_numeric(&a)? {
            Value::BigInt(b) => crate::builtins::bigint::to_f64(&b),
            Value::Number(n) => n,
            _ => 0.0,
        }
    };
    if ctx.new_target.is_undefined() {
        return Ok(Value::Number(n));
    }
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.number_proto)?;
    Ok(Value::Object(vm.alloc(ObjectData::new(Some(p), Kind::Number(n)))))
}

fn is_finite(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(matches!(vm.arg(ctx, 0), Value::Number(n) if n.is_finite())))
}
fn is_integer(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(matches!(vm.arg(ctx, 0), Value::Number(n) if n.is_finite() && numconv::libm_floor(n) == n)))
}
fn is_nan(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(matches!(vm.arg(ctx, 0), Value::Number(n) if n.is_nan())))
}
fn is_safe_integer(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Bool(matches!(vm.arg(ctx, 0), Value::Number(n) if n.is_finite() && numconv::libm_floor(n) == n && n.abs() <= 9007199254740991.0)))
}

fn this_num(vm: &mut Vm, v: &Value) -> JsResult<f64> {
    match v {
        Value::Number(n) => Ok(*n),
        Value::Object(o) => match &vm.heap.get(*o).kind {
            Kind::Number(n) => Ok(*n),
            _ => vm.throw_type("Number.prototype method called on incompatible receiver"),
        },
        _ => vm.throw_type("Number.prototype method called on incompatible receiver"),
    }
}

fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let x = this_num(vm, &ctx.this)?;
    let r = vm.arg(ctx, 0);
    let radix = if r.is_undefined() { 10.0 } else { vm.to_integer_or_infinity(&r)? };
    if !(2.0..=36.0).contains(&radix) {
        return vm.throw_range("toString() radix must be between 2 and 36");
    }
    if radix == 10.0 {
        return Ok(Value::String(crate::vm::ops::number_to_jsstr(x)));
    }
    Ok(Value::String(JsStr::from_str(&numconv::f64_to_radix_string(x, radix as u32))))
}

fn to_locale_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let x = this_num(vm, &ctx.this)?;
    Ok(Value::String(crate::vm::ops::number_to_jsstr(x)))
}

fn value_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    Ok(Value::Number(this_num(vm, &ctx.this)?))
}

fn to_fixed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let x = this_num(vm, &ctx.this)?;
    let fa = vm.arg(ctx, 0);
    let f = vm.to_integer_or_infinity(&fa)?;
    if !f.is_finite() || !(0.0..=100.0).contains(&f) {
        return vm.throw_range("toFixed() digits argument must be between 0 and 100");
    }
    if !x.is_finite() {
        return Ok(Value::String(crate::vm::ops::number_to_jsstr(x)));
    }
    if x.abs() >= 1e21 {
        return Ok(Value::String(crate::vm::ops::number_to_jsstr(x)));
    }
    let mut s = alloc::string::String::new();
    if x < 0.0 {
        s.push('-');
    }
    let body = numconv::to_fixed(x.abs(), f as usize);
    // -0.00 stays "0.00" (x < 0 is false for -0)
    if x < 0.0 && body.chars().all(|c| c == '0' || c == '.') {
        // toFixed(-0.0000001, 2) is "-0.00" per spec (x < 0)
    }
    s.push_str(&body);
    Ok(Value::String(JsStr::from_str(&s)))
}

fn to_exponential(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let x = this_num(vm, &ctx.this)?;
    let fa = vm.arg(ctx, 0);
    let f = vm.to_integer_or_infinity(&fa)?;
    if !x.is_finite() {
        return Ok(Value::String(crate::vm::ops::number_to_jsstr(x)));
    }
    if !f.is_finite() || !(0.0..=100.0).contains(&f) {
        return vm.throw_range("toExponential() argument must be between 0 and 100");
    }
    let mut s = alloc::string::String::new();
    let mut v = x;
    if v < 0.0 {
        s.push('-');
        v = -v;
    }
    let (digits, e) = if v == 0.0 {
        (alloc::vec![b'0'; if fa.is_undefined() { 1 } else { f as usize + 1 }], 0)
    } else if fa.is_undefined() {
        let (d, k) = numconv::shortest_digits(v);
        (d, k - 1)
    } else {
        numconv::precision_digits(v, f as usize + 1)
    };
    s.push(digits[0] as char);
    if digits.len() > 1 {
        s.push('.');
        s.push_str(core::str::from_utf8(&digits[1..]).unwrap());
    }
    s.push('e');
    s.push(if e >= 0 { '+' } else { '-' });
    numconv::push_int(&mut s, e.unsigned_abs() as u64);
    Ok(Value::String(JsStr::from_str(&s)))
}

fn to_precision(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let x = this_num(vm, &ctx.this)?;
    let pa = vm.arg(ctx, 0);
    if pa.is_undefined() {
        return Ok(Value::String(crate::vm::ops::number_to_jsstr(x)));
    }
    let p = vm.to_integer_or_infinity(&pa)?;
    if !x.is_finite() {
        return Ok(Value::String(crate::vm::ops::number_to_jsstr(x)));
    }
    if !p.is_finite() || !(1.0..=100.0).contains(&p) {
        return vm.throw_range("toPrecision() argument must be between 1 and 100");
    }
    let p = p as usize;
    let mut s = alloc::string::String::new();
    let mut v = x;
    if v < 0.0 {
        s.push('-');
        v = -v;
    }
    let (digits, e) = if v == 0.0 { (alloc::vec![b'0'; p], 0) } else { numconv::precision_digits(v, p) };
    if e < -6 || e >= p as i32 {
        s.push(digits[0] as char);
        if p > 1 {
            s.push('.');
            s.push_str(core::str::from_utf8(&digits[1..]).unwrap());
        }
        s.push('e');
        s.push(if e >= 0 { '+' } else { '-' });
        numconv::push_int(&mut s, e.unsigned_abs() as u64);
    } else if e == p as i32 - 1 {
        s.push_str(core::str::from_utf8(&digits).unwrap());
    } else if e >= 0 {
        let k = e as usize + 1;
        s.push_str(core::str::from_utf8(&digits[..k]).unwrap());
        s.push('.');
        s.push_str(core::str::from_utf8(&digits[k..]).unwrap());
    } else {
        s.push_str("0.");
        for _ in 0..(-(e + 1)) {
            s.push('0');
        }
        s.push_str(core::str::from_utf8(&digits).unwrap());
    }
    Ok(Value::String(JsStr::from_str(&s)))
}
